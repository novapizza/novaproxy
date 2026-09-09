//! The privileged helper: pay for the admin password **once**, not once per
//! proxy change.
//!
//! macOS is the only platform where pointing the OS at NovaProxy needs root
//! (`networksetup` writes machine-wide SystemConfiguration state). Prompting per
//! change is bad enough; the worst case was the crash-recovery restore, which
//! raised a password dialog during app launch before any window had appeared.
//!
//! So a small root daemon does the `networksetup` work instead:
//!
//! ```text
//!   NovaProxy (user)  --unix socket-->  nova-helper (root, launchd)  --> networksetup
//! ```
//!
//! Installing it costs one `osascript … with administrator privileges` prompt,
//! after which every enable/disable/restore is silent.
//!
//! **Why a LaunchDaemon and not `SMJobBless`.** `SMJobBless` requires the app and
//! the helper to be Developer ID signed with matching code requirements. A plain
//! LaunchDaemon works for unsigned local builds too, which is what a
//! self-hosted debugging proxy has to support.
//!
//! **Trust boundary.** The daemon is root, so its input is treated as hostile:
//!
//! * the socket is owned by the installing user, mode `0600`, and every
//!   connection additionally re-checks the peer's uid with `getpeereid` — a
//!   different local account cannot reach it even if the mode is tampered with;
//! * the only operations are "point the proxy at a loopback address" and "restore
//!   this snapshot" — never an arbitrary command;
//! * hosts must be loopback literals, ports must be non-zero, and service names
//!   must match services the machine actually has ([`validate_enable`],
//!   [`validate_backup`]);
//! * commands are executed as argv, never through a shell, so no value can be
//!   read as syntax.
//!
//! Everything except the socket I/O is a pure function, so the validation rules
//! and the install plan are unit-tested on any OS.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::oscmd::{Elevation, Plan, Step};
use crate::sysproxy::Backup;

/// Wire-format version. Bumped whenever [`Request`] or [`Response`] changes
/// meaning; the app refuses to talk to a helper that answers with a different
/// number and offers to reinstall instead.
pub const PROTOCOL_VERSION: u32 = 1;

/// launchd job label, also the plist's basename.
pub const LABEL: &str = "dev.novaproxy.helper";

/// Where the daemon listens. Under `/var/run` because only root may create
/// entries there — a user process cannot pre-create a fake socket for the app to
/// connect to.
pub const SOCKET_PATH: &str = "/var/run/novaproxy-helper.sock";

/// Machine-wide install location of the helper binary.
pub const HELPER_DIR: &str = "/Library/Application Support/NovaProxy";
pub const HELPER_BIN: &str = "/Library/Application Support/NovaProxy/nova-helper";
pub const PLIST_PATH: &str = "/Library/LaunchDaemons/dev.novaproxy.helper.plist";

/// Where the daemon's stdout and stderr land.
///
/// `/Library/Logs` rather than the app's own `~/Library/Logs/NovaProxy`: the
/// daemon runs as root before any user is logged in, and writing into a home
/// directory it does not own is both wrong and impossible at that point. The
/// app's log bundler reads this file back out, which is why it is made
/// world-readable — see `prepare_log`.
pub const HELPER_LOG_DIR: &str = "/Library/Logs/NovaProxy";
pub const HELPER_LOG: &str = "/Library/Logs/NovaProxy/helper.log";

/// Filename the app stages the plist under before the privileged copy.
pub const STAGED_PLIST: &str = "dev.novaproxy.helper.plist";

/// Where client and server actually meet.
///
/// `NOVAPROXY_HELPER_SOCKET` relocates it, but **only in debug builds**: the
/// daemon is otherwise the one process where an environment variable must not be
/// able to move a privileged endpoint. It exists so the protocol can be tested
/// end to end as an ordinary user — `/var/run` is root-only by design.
pub fn socket_path() -> PathBuf {
    if cfg!(debug_assertions) {
        if let Some(path) = std::env::var_os("NOVAPROXY_HELPER_SOCKET") {
            return PathBuf::from(path);
        }
    }
    PathBuf::from(SOCKET_PATH)
}

/* -------------------------------- protocol -------------------------------- */

/// One request, sent as a single JSON line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Liveness + version handshake.
    Ping,
    /// Point every service in `backup` at `host:port`.
    Enable { host: String, port: u16, backup: Backup },
    /// Put every service in `backup` back exactly as it was.
    Disable { backup: Backup },
}

impl Request {
    /// A fixed name for the log, so the variant is readable without printing
    /// the request — whose `backup` carries every network service name.
    pub fn op_name(&self) -> &'static str {
        match self {
            Request::Ping => "ping",
            Request::Enable { .. } => "enable",
            Request::Disable { .. } => "disable",
        }
    }
}

/// One reply, likewise a single JSON line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Response {
    pub ok: bool,
    /// The helper's [`PROTOCOL_VERSION`], so a stale install is detectable from
    /// any reply rather than only from `Ping`.
    pub version: u32,
    pub error: Option<String>,
}

impl Response {
    pub fn ok() -> Self {
        Self { ok: true, version: PROTOCOL_VERSION, error: None }
    }

    pub fn err(message: impl std::fmt::Display) -> Self {
        Self { ok: false, version: PROTOCOL_VERSION, error: Some(message.to_string()) }
    }
}

/* -------------------------------- validation ------------------------------- */

/// Longest plausible network service name; a bound keeps a malformed request
/// from being handed to `networksetup` in full.
const MAX_FIELD: usize = 255;

/// Reject anything that is not a plain, printable single-line value.
///
/// Nothing here is executed through a shell, so this is not injection defence —
/// it is a sanity bound that keeps garbage out of a root process's argv.
fn plain(value: &str, what: &str) -> Result<()> {
    if value.len() > MAX_FIELD {
        bail!("{what} is too long");
    }
    if value.chars().any(|c| c.is_control()) {
        bail!("{what} contains control characters");
    }
    Ok(())
}

/// The proxy the OS is pointed at must be *ours*: a loopback address.
///
/// NovaProxy only ever binds loopback, so this costs nothing and removes the
/// interesting attack — a caller talking the helper into routing the machine's
/// traffic through a remote host.
pub fn validate_host(host: &str) -> Result<()> {
    let ip: std::net::IpAddr = host
        .parse()
        .with_context(|| format!("`{host}` is not an IP address"))?;
    if !ip.is_loopback() {
        bail!("`{host}` is not a loopback address");
    }
    Ok(())
}

/// Check an enable request: loopback host, real port, known services.
pub fn validate_enable(host: &str, port: u16, backup: &Backup, known: &[String]) -> Result<()> {
    validate_host(host)?;
    if port == 0 {
        bail!("port 0 is not a listening port");
    }
    validate_backup(backup, known)
}

/// Check a snapshot before replaying it as root.
///
/// Service names must be services this machine actually has: the snapshot
/// arrives from a file on disk, and a bogus name would otherwise be passed
/// straight through to `networksetup`.
pub fn validate_backup(backup: &Backup, known: &[String]) -> Result<()> {
    if backup.services.is_empty() {
        bail!("snapshot lists no network services");
    }
    for s in &backup.services {
        plain(&s.service, "service name")?;
        if !known.iter().any(|k| k == &s.service) {
            bail!("`{}` is not a network service on this machine", s.service);
        }
        for (value, what) in [
            (&s.web_host, "proxy host"),
            (&s.secure_host, "proxy host"),
            (&s.web_port, "proxy port"),
            (&s.secure_port, "proxy port"),
        ] {
            plain(value, what)?;
        }
        for port in [&s.web_port, &s.secure_port] {
            if !port.is_empty() && port.parse::<u16>().is_err() {
                bail!("`{port}` is not a port number");
            }
        }
    }
    Ok(())
}

/* --------------------------------- install --------------------------------- */

/// The launchd job description.
///
/// `KeepAlive` + `RunAtLoad` because the daemon is the app's only way to change
/// proxy settings without a prompt: if it dies, launchd brings it straight back.
/// The owner uid is baked in at install time so the daemon serves exactly the
/// account that authorised it.
pub fn plist_xml(owner_uid: u32) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{HELPER_BIN}</string>
        <string>--owner-uid</string>
        <string>{owner_uid}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ProcessType</key>
    <string>Background</string>
    <key>StandardOutPath</key>
    <string>{HELPER_LOG}</string>
    <key>StandardErrorPath</key>
    <string>{HELPER_LOG}</string>
</dict>
</plist>
"#
    )
}

/// Copy the helper into place and load it — one password prompt for the lot.
///
/// `install(1)` rather than `cp` + `chown` + `chmod`: it sets owner and mode in
/// the same step, so the file is never briefly root-owned *and* user-writable.
///
/// Best-effort because `bootout` fails when nothing is loaded yet, which is the
/// normal first-install case; the outcome is judged by [`ping`], not by exit
/// codes (see [`install`]).
pub fn install_plan(source_bin: &Path, staged_plist: &Path) -> Plan {
    let job = format!("system/{LABEL}");
    let steps = vec![
        Step::new("/usr/bin/install", &["-d", "-o", "root", "-g", "wheel", "-m", "755", HELPER_DIR]),
        // launchd does not create the directory its StandardOutPath names: without
        // this step the plist points at nothing and the daemon's log is discarded
        // exactly as it was before. 755 so the app can read the file back.
        Step::new("/usr/bin/install", &["-d", "-o", "root", "-g", "wheel", "-m", "755", HELPER_LOG_DIR]),
        Step::with_args(
            "/usr/bin/install",
            vec![
                "-o".into(), "root".into(),
                "-g".into(), "wheel".into(),
                "-m".into(), "755".into(),
                source_bin.display().to_string(),
                HELPER_BIN.into(),
            ],
        ),
        Step::with_args(
            "/usr/bin/install",
            vec![
                "-o".into(), "root".into(),
                "-g".into(), "wheel".into(),
                "-m".into(), "644".into(),
                staged_plist.display().to_string(),
                PLIST_PATH.into(),
            ],
        ),
        // Unload any previous generation first, so a reinstall replaces a running
        // daemon rather than leaving the old binary serving the socket.
        Step::new("/bin/launchctl", &["bootout", &job]),
        Step::new("/bin/launchctl", &["bootstrap", "system", PLIST_PATH]),
    ];
    Plan::best_effort(steps, Elevation::MacAdmin)
}

/// Unload and delete every trace of the helper — also one prompt.
pub fn uninstall_plan() -> Plan {
    let job = format!("system/{LABEL}");
    Plan::best_effort(
        vec![
            Step::new("/bin/launchctl", &["bootout", &job]),
            Step::new("/bin/rm", &["-f", PLIST_PATH]),
            Step::new("/bin/rm", &["-f", HELPER_BIN]),
            Step::new("/bin/rm", &["-f", SOCKET_PATH]),
        ],
        Elevation::MacAdmin,
    )
}

/// Where the helper binary we would install lives.
///
/// Checked in order: an explicit override (used by the tests and by anyone
/// running the daemon from a build tree), next to the running executable (the
/// `cargo tauri dev` layout, and the bundle's `MacOS/` directory), then the
/// bundle's `Resources/`.
pub fn source_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("NOVAPROXY_HELPER_BIN") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [dir.join("nova-helper"), dir.join("../Resources/nova-helper")]
        .into_iter()
        .find(|p| p.is_file())
}

/// Install the helper, then confirm by talking to it.
///
/// The plan's exit codes are not the answer: `launchctl bootout` reports failure
/// on a first install, and `bootstrap` reports success before the daemon has
/// finished binding its socket. A successful [`ping`] is the only proof that
/// matters, so we poll for one and surface the plan's error only if none comes.
pub fn install(source_bin: &Path, staged_dir: &Path, owner_uid: u32) -> Result<()> {
    if !source_bin.is_file() {
        bail!("helper binary not found at {}", source_bin.display());
    }
    std::fs::create_dir_all(staged_dir)?;
    let staged_plist = staged_dir.join(STAGED_PLIST);
    std::fs::write(&staged_plist, plist_xml(owner_uid))
        .with_context(|| format!("cannot stage {}", staged_plist.display()))?;

    let ran = install_plan(source_bin, &staged_plist).run();
    let _ = std::fs::remove_file(&staged_plist);

    // launchd starts the daemon asynchronously; give it a moment to bind.
    for _ in 0..50 {
        if ping().is_ok() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    match ran {
        Err(e) => Err(e.context("the helper did not start")),
        Ok(()) => bail!("the helper was installed but is not answering on {SOCKET_PATH}"),
    }
}

/// Remove the helper. Succeeds when nothing answers afterwards, whatever the
/// individual steps reported.
pub fn uninstall() -> Result<()> {
    let ran = uninstall_plan().run();
    if ping().is_err() {
        return Ok(());
    }
    ran.and_then(|()| bail!("the helper is still running"))
}

/* ---------------------------------- client --------------------------------- */

#[cfg(unix)]
mod client {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    use super::*;

    /// A `networksetup` sweep over every service is not instant, and the caller
    /// blocks on it; generous, but not forever.
    const IO_TIMEOUT: Duration = Duration::from_secs(90);

    /// Longest reply we will read, so a wedged daemon cannot grow the app's heap.
    const MAX_REPLY: u64 = 64 * 1024;

    pub fn send(request: &Request) -> Result<Response> {
        let path = socket_path();
        let stream = UnixStream::connect(&path)
            .with_context(|| format!("the NovaProxy helper is not running ({})", path.display()))?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;

        let mut writer = &stream;
        let mut line = serde_json::to_string(request)?;
        line.push('\n');
        writer.write_all(line.as_bytes())?;
        writer.flush()?;

        let mut reply = String::new();
        BufReader::new((&stream).take(MAX_REPLY)).read_line(&mut reply)?;
        if reply.trim().is_empty() {
            bail!("the helper closed the connection without replying");
        }
        Ok(serde_json::from_str(&reply)?)
    }
}

#[cfg(not(unix))]
mod client {
    use super::*;

    pub fn send(_request: &Request) -> Result<Response> {
        bail!("the NovaProxy helper is macOS-only")
    }
}

/// Ask the helper for its protocol version, proving it is alive.
pub fn ping() -> Result<u32> {
    let reply = client::send(&Request::Ping)?;
    if !reply.ok {
        bail!(reply.error.unwrap_or_else(|| "the helper rejected the ping".into()));
    }
    Ok(reply.version)
}

/// Is a helper installed *and* speaking a version we understand?
///
/// A version mismatch counts as "not usable": the app falls back to prompting
/// rather than sending a request the daemon might read differently.
pub fn usable() -> bool {
    matches!(ping(), Ok(v) if v == PROTOCOL_VERSION)
}

fn call(request: Request) -> Result<()> {
    let reply = client::send(&request)?;
    if reply.version != PROTOCOL_VERSION {
        bail!(
            "the installed helper speaks protocol {} but this build speaks {PROTOCOL_VERSION} — \
             reinstall it from Settings › General",
            reply.version
        );
    }
    if !reply.ok {
        bail!(reply.error.unwrap_or_else(|| "the helper reported a failure".into()));
    }
    Ok(())
}

/// Point the OS proxy at `host:port` through the helper.
pub fn enable(host: &str, port: u16, backup: &Backup) -> Result<()> {
    call(Request::Enable { host: host.to_string(), port, backup: backup.clone() })
}

/// Restore a snapshot through the helper.
pub fn disable(backup: &Backup) -> Result<()> {
    call(Request::Disable { backup: backup.clone() })
}

/// The uid the helper should serve — the account running the app.
pub fn current_uid() -> u32 {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: `getuid` takes no arguments and cannot fail.
        unsafe { libc::getuid() as u32 }
    }
    #[cfg(not(target_os = "macos"))]
    {
        0
    }
}

/* ---------------------------------- server --------------------------------- */

/// The root side. Only compiled on macOS — it is the daemon's entire body.
#[cfg(target_os = "macos")]
pub mod server {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::io::AsRawFd;
    use std::os::unix::net::{UnixListener, UnixStream};

    use super::*;
    use crate::sysproxy::macos;

    /// Cap on one request line: a snapshot of every network service is a few KB.
    const MAX_REQUEST: u64 = 256 * 1024;

    /// Serve forever, one connection at a time.
    ///
    /// Serial by design: the operations mutate one global setting, so overlapping
    /// them would interleave `networksetup` calls for no benefit.
    pub fn serve(owner_uid: u32) -> Result<()> {
        // A leftover socket from an unclean exit would make `bind` fail. Only root
        // can have created it (`/var/run`), so removing it is safe.
        let path = socket_path();
        let _ = std::fs::remove_file(&path);
        let listener =
            UnixListener::bind(&path).with_context(|| format!("cannot bind {}", path.display()))?;
        restrict_socket(&path, owner_uid)?;
        prepare_log();
        tracing::info!("nova-helper listening on {} for uid {owner_uid}", path.display());

        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    if let Err(e) = handle(stream, owner_uid) {
                        tracing::warn!("connection failed: {e}");
                    }
                }
                Err(e) => tracing::warn!("accept failed: {e}"),
            }
        }
        Ok(())
    }

    /// Size at which the log is rotated, checked once per boot.
    ///
    /// launchd appends to `StandardOutPath` forever and rotates nothing. A quiet
    /// boot writes a handful of lines, but `rejected connection from uid N` is one
    /// line per refused connect — a process retrying in a loop is unbounded, and
    /// this daemon is the one thing on the machine that must not fill the disk.
    const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

    /// Make the log readable by the user and keep it from growing without end.
    ///
    /// launchd creates the file 0600 root, which the app's log bundler — running
    /// as the user — cannot read. A daemon log nobody can collect is the same as
    /// no daemon log, so it is widened to 0644: still root-only to write, which is
    /// what matters, because a user-writable root log is a place to forge entries.
    ///
    /// Best-effort throughout. Nothing here is worth refusing to serve over.
    fn prepare_log() {
        let path = std::path::Path::new(HELPER_LOG);
        // Widened *before* any rename, because the mode belongs to the inode:
        // chmod after the rename would target a path that no longer exists and
        // leave the rotated file unreadable — the one thing this is here to fix.
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644));
        if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) > MAX_LOG_BYTES {
            // Rename rather than truncate: launchd opened this fd before we ran,
            // so a truncate leaves it writing at the old offset into a sparse
            // file. The consequence of renaming is that *this* run's output keeps
            // flowing into `helper.log.1` — the fd follows the inode — and a fresh
            // `helper.log` appears on the next launch. Both names are collected
            // into the support bundle for exactly that reason.
            let _ = std::fs::rename(path, format!("{HELPER_LOG}.1"));
        }
    }

    /// Hand the socket to the installing user, and to nobody else.
    fn restrict_socket(path: &std::path::Path, owner_uid: u32) -> Result<()> {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        // Only root may give a file away, and only root has anything to give: a
        // test-mode daemon running as the user already owns its own socket.
        if unsafe { libc::getuid() } != 0 {
            return Ok(());
        }
        let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())?;
        // SAFETY: `c_path` is a valid NUL-terminated string for the duration of
        // the call; `chown` reports failure through its return value.
        let rc = unsafe { libc::chown(c_path.as_ptr(), owner_uid as libc::uid_t, 0) };
        if rc != 0 {
            bail!(
                "cannot hand {} to uid {owner_uid}: {}",
                path.display(),
                std::io::Error::last_os_error()
            );
        }
        Ok(())
    }

    /// The uid on the other end of a connected socket.
    fn peer_uid(stream: &UnixStream) -> Result<u32> {
        let mut uid: libc::uid_t = 0;
        let mut gid: libc::gid_t = 0;
        // SAFETY: both out-params are valid for the call, and the fd is owned by
        // `stream` and open for its duration.
        let rc = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
        if rc != 0 {
            bail!("cannot read peer credentials: {}", std::io::Error::last_os_error());
        }
        Ok(uid as u32)
    }

    fn handle(stream: UnixStream, owner_uid: u32) -> Result<()> {
        // The socket mode already limits this, but the check is what actually
        // enforces the boundary: file modes can be changed, a uid cannot be faked.
        let peer = peer_uid(&stream)?;
        if peer != owner_uid {
            reply(&stream, Response::err(format!("uid {peer} is not the owner of this helper")))?;
            bail!("rejected connection from uid {peer}");
        }

        let mut line = String::new();
        BufReader::new((&stream).take(MAX_REQUEST)).read_line(&mut line)?;
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => run(request),
            Err(e) => Response::err(format!("malformed request: {e}")),
        };
        reply(&stream, response)
    }

    fn reply(mut stream: &UnixStream, response: Response) -> Result<()> {
        let mut line = serde_json::to_string(&response)?;
        line.push('\n');
        stream.write_all(line.as_bytes())?;
        stream.flush()?;
        Ok(())
    }

    /// Validate, then apply. Errors come back as a reply, never as a panic: the
    /// daemon must survive whatever it is sent.
    fn run(request: Request) -> Response {
        // Timed here rather than inside `apply`, because the app measures this
        // as one blocking IPC round trip and cannot see the split. The app's
        // `sysproxy.enable` minus this `ms` is what the socket and the JSON
        // cost; the two lines only mean something read together, one from
        // `~/Library/Logs/NovaProxy` and one from `/Library/Logs/NovaProxy`
        // (the support bundle collects both).
        let op = request.op_name();
        let t = std::time::Instant::now();
        let before = crate::oscmd::exec_stats();
        let result = apply(request);
        let (spawns, spawn_ms) = {
            let now = crate::oscmd::exec_stats();
            (now.0.saturating_sub(before.0), now.1.saturating_sub(before.1))
        };
        tracing::info!(
            op,
            ms = t.elapsed().as_millis() as u64,
            spawns,
            spawn_ms,
            ok = result.is_ok(),
            "served request"
        );
        match result {
            Ok(()) => Response::ok(),
            Err(e) => Response::err(e),
        }
    }

    fn apply(request: Request) -> Result<()> {
        // The validation sweep is its own `networksetup` call, and it is paid
        // before any change is made — worth a line of its own so a slow
        // "enable" is not blamed on the writes when the read is the cost.
        let known = || {
            let t = std::time::Instant::now();
            let services = macos::parse_services(
                crate::oscmd::capture("networksetup", &["-listallnetworkservices"]).as_str(),
            );
            tracing::info!(
                count = services.len(),
                ms = t.elapsed().as_millis() as u64,
                "listed network services for validation"
            );
            services
        };
        match request {
            Request::Ping => Ok(()),
            Request::Enable { host, port, backup } => {
                validate_enable(&host, port, &backup, &known())?;
                tracing::info!("enabling system proxy at {host}:{port}");
                // Already root: run the commands directly instead of asking
                // `osascript` for privileges we have.
                macos::enable_plan(&host, port, &backup).without_elevation().run()
            }
            Request::Disable { backup } => {
                validate_backup(&backup, &known())?;
                tracing::info!("restoring {} network services", backup.services.len());
                macos::disable_plan(&backup).without_elevation().run()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysproxy::ServiceBackup;

    fn service(name: &str) -> ServiceBackup {
        ServiceBackup {
            service: name.to_string(),
            web_enabled: true,
            web_host: "10.0.0.1".into(),
            web_port: "8080".into(),
            secure_enabled: false,
            secure_host: String::new(),
            secure_port: String::new(),
        }
    }

    fn backup(names: &[&str]) -> Backup {
        Backup { services: names.iter().map(|n| service(n)).collect(), ..Default::default() }
    }

    #[test]
    fn only_loopback_addresses_are_accepted() {
        assert!(validate_host("127.0.0.1").is_ok());
        assert!(validate_host("::1").is_ok());
        // The whole point of the check: a root daemon must not be talked into
        // routing the machine's traffic somewhere off-box.
        assert!(validate_host("10.0.0.1").is_err());
        assert!(validate_host("evil.example.com").is_err());
        assert!(validate_host("").is_err());
    }

    #[test]
    fn enable_requires_a_real_port_and_known_services() {
        let known = vec!["Wi-Fi".to_string()];
        let b = backup(&["Wi-Fi"]);
        assert!(validate_enable("127.0.0.1", 9090, &b, &known).is_ok());
        assert!(validate_enable("127.0.0.1", 0, &b, &known).is_err());
        assert!(validate_enable("127.0.0.1", 9090, &backup(&["Ethernet"]), &known).is_err());
    }

    #[test]
    fn backups_naming_services_the_machine_lacks_are_rejected() {
        let known = vec!["Wi-Fi".to_string(), "USB 10/100 LAN".to_string()];
        assert!(validate_backup(&backup(&["Wi-Fi", "USB 10/100 LAN"]), &known).is_ok());
        assert!(validate_backup(&backup(&["Wi-Fi", "Fake"]), &known).is_err());
        // An empty snapshot would be a no-op, but it also means the caller sent
        // something it never captured — worth refusing rather than silently
        // succeeding.
        assert!(validate_backup(&Backup::default(), &known).is_err());
    }

    #[test]
    fn backups_with_junk_fields_are_rejected() {
        let known = vec!["Wi-Fi".to_string()];
        let mut b = backup(&["Wi-Fi"]);
        b.services[0].web_port = "not-a-port".into();
        assert!(validate_backup(&b, &known).is_err());

        let mut b = backup(&["Wi-Fi"]);
        b.services[0].web_host = "10.0.0.1\nrm -rf /".into();
        assert!(validate_backup(&b, &known).is_err());

        let mut b = backup(&["Wi-Fi"]);
        b.services[0].web_port = String::new(); // "no port recorded" is legitimate
        assert!(validate_backup(&b, &known).is_ok());
    }

    #[test]
    fn the_plist_names_the_installed_binary_and_its_owner() {
        let xml = plist_xml(501);
        assert!(xml.contains(&format!("<string>{LABEL}</string>")));
        assert!(xml.contains(HELPER_BIN));
        assert!(xml.contains("<string>501</string>"), "the owner uid is baked in");
        assert!(xml.contains("<key>KeepAlive</key>"), "launchd must restart it");
    }

    #[test]
    fn the_plist_sends_the_daemons_output_to_a_file() {
        // Without these keys launchd routes stdout and stderr to /dev/null, which
        // silently discarded `rejected connection from uid N`.
        let xml = plist_xml(501);
        assert!(xml.contains("<key>StandardOutPath</key>"));
        assert!(xml.contains("<key>StandardErrorPath</key>"));
        assert_eq!(
            xml.matches(HELPER_LOG).count(),
            2,
            "both streams go to {HELPER_LOG}"
        );
    }

    #[test]
    fn install_places_the_binary_as_root_then_loads_the_job() {
        let plan = install_plan(Path::new("/build/nova-helper"), Path::new("/staged/x.plist"));
        assert_eq!(plan.elevation, Elevation::MacAdmin, "one prompt, once");
        let line = plan.steps.iter().map(|s| s.to_shell()).collect::<Vec<_>>().join(" ; ");
        assert!(line.contains("-o root -g wheel -m 755 /build/nova-helper"));
        // launchd will not create the log directory the plist names.
        assert!(line.contains(&format!("-m 755 {HELPER_LOG_DIR}")));
        assert!(line.contains(HELPER_BIN));
        assert!(line.contains("-m 644 /staged/x.plist"));
        assert!(line.contains(&format!("bootout system/{LABEL}")));
        assert!(line.contains(&format!("bootstrap system {PLIST_PATH}")));
        // A first install has nothing to boot out; that step failing must not
        // stop the bootstrap.
        assert!(plan.best_effort);
    }

    #[test]
    fn uninstall_unloads_before_deleting() {
        let plan = uninstall_plan();
        let steps: Vec<String> = plan.steps.iter().map(|s| s.to_shell()).collect();
        assert!(steps[0].contains("bootout"), "deleting a loaded job leaves it running");
        assert!(steps.iter().any(|s| s.contains(HELPER_BIN)));
        assert!(steps.iter().any(|s| s.contains(PLIST_PATH)));
        assert!(steps.iter().any(|s| s.contains(SOCKET_PATH)));
    }

    #[test]
    fn requests_and_replies_round_trip_as_one_json_line() {
        let request = Request::Enable {
            host: "127.0.0.1".into(),
            port: 9090,
            backup: backup(&["Wi-Fi"]),
        };
        let line = serde_json::to_string(&request).unwrap();
        assert!(!line.contains('\n'), "the protocol is line-delimited");
        assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), request);

        let reply = serde_json::to_string(&Response::err("nope")).unwrap();
        let parsed: Response = serde_json::from_str(&reply).unwrap();
        assert!(!parsed.ok);
        assert_eq!(parsed.version, PROTOCOL_VERSION);
    }

    #[test]
    fn the_source_binary_can_be_overridden_for_local_builds() {
        // The bundled layout cannot be exercised in a unit test, but the override
        // path is what a dev build and the test harness both rely on.
        std::env::set_var("NOVAPROXY_HELPER_BIN", "/definitely/not/here/nova-helper");
        assert_eq!(source_binary(), None, "a missing override is not a source");
        std::env::set_var("NOVAPROXY_HELPER_BIN", std::env::current_exe().unwrap());
        assert!(source_binary().is_some());
        std::env::remove_var("NOVAPROXY_HELPER_BIN");
    }
}
