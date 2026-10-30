//! System-proxy control for macOS, Windows and Linux.
//!
//! Routing traffic through NovaProxy means changing an OS setting that the user
//! depends on for working internet, so every platform obeys the same contract:
//!
//! 1. **Snapshot first.** Read the prior state and hand it back to the caller,
//!    which persists it *before* anything is mutated.
//! 2. **Restore on disable** — put back exactly what was there, including a
//!    corporate PAC/auto-config URL, never a blanket "off".
//! 3. **Restore on next launch after a crash**, from that persisted snapshot.
//!
//! Privileges differ, and macOS is the odd one out:
//!
//! | Platform | Mechanism | Prompt |
//! |---|---|---|
//! | macOS | `networksetup` per network service | one admin password per change (batched into a single `osascript`) |
//! | Windows | WinINet per-connection options (LAN + each RAS/VPN entry), see [`crate::windows::inet`] | none — the settings are per-user |
//! | Linux | `gsettings org.gnome.system.proxy` | none — dconf is per-user |
//!
//! Command *plans* are pure data (see [`crate::oscmd`]), so what each platform
//! would run is unit-tested from any machine; only execution is platform-gated.
//!
//! **Linux coverage is honest, not universal.** `gsettings` covers GNOME and the
//! many apps that read those keys; there is no machine-wide switch that also
//! covers the `http_proxy` environment convention for already-running shells. The
//! launch-through-NovaProxy helper is the answer for those.

use anyhow::Result;
use serde::{Deserialize, Serialize};

// `bail!` and `have_tool` are referenced through full paths below: both are only
// reachable on some platforms, and importing them would warn on the others.
use crate::oscmd::{capture, Elevation, Plan, Step};

/// Saved proxy state for one macOS network service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceBackup {
    pub service: String,
    pub web_enabled: bool,
    pub web_host: String,
    pub web_port: String,
    pub secure_enabled: bool,
    pub secure_host: String,
    pub secure_port: String,
}

/// Saved WinINet proxy settings (Windows).
///
/// Current builds fill [`connections`](Self::connections): one entry for the
/// LAN default and one per RAS/VPN entry, each restored exactly. The four flat
/// fields are what builds before 0.7 wrote (raw `Internet Settings` registry
/// values); they are read only to restore a snapshot such a build left behind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowsBackup {
    #[serde(default)]
    pub connections: Vec<windows::ConnectionSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_enable: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_server: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_override: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_config_url: Option<String>,
}

/// Saved GNOME proxy settings (Linux).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GnomeBackup {
    /// `none`, `manual` or `auto`.
    pub mode: String,
    pub http_host: String,
    pub http_port: String,
    pub https_host: String,
    pub https_port: String,
}

/// A snapshot of the prior proxy state, persisted for a safe restore.
///
/// One struct for every platform, with each platform's data in its own optional
/// field: a snapshot written by an older build (macOS-only, `services` at the top
/// level) still deserializes, so upgrading mid-session cannot orphan a restore.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backup {
    #[serde(default)]
    pub services: Vec<ServiceBackup>,
    #[serde(default)]
    pub windows: Option<WindowsBackup>,
    #[serde(default)]
    pub gnome: Option<GnomeBackup>,
}

/* --------------------------------- macOS --------------------------------- */

pub mod macos {
    use super::*;

    /// Enabled network services, from `networksetup -listallnetworkservices`.
    /// The first line is a header and a leading `*` marks a disabled service.
    /// `backup` without the services this machine no longer has.
    ///
    /// A snapshot can outlive a service: one left behind by an unclean exit is
    /// reused on the next enable, and a VPN client removes its service when a
    /// profile is deleted or re-imported. The helper rightly refuses names it
    /// cannot find, so one vanished VPN used to fail every enable and restore.
    pub fn without_missing(backup: &Backup, known: &[String]) -> Backup {
        let mut kept = backup.clone();
        kept.services.retain(|s| {
            let present = known.iter().any(|k| k == &s.service);
            if !present {
                tracing::warn!(service = %s.service, "skipping a network service that no longer exists");
            }
            present
        });
        kept
    }

    /// [`without_missing`] against the services listed right now.
    pub fn current(backup: &Backup) -> Backup {
        let known = parse_services(&capture("networksetup", &["-listallnetworkservices"]));
        without_missing(backup, &known)
    }

    pub fn parse_services(output: &str) -> Vec<String> {
        output
            .lines()
            .skip(1)
            .filter(|l| !l.starts_with('*'))
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    }

    /// `(enabled, host, port)` from `networksetup -getwebproxy <service>`.
    pub fn parse_proxy(output: &str) -> (bool, String, String) {
        let mut enabled = false;
        let mut host = String::new();
        let mut port = String::new();
        for line in output.lines() {
            if let Some(v) = line.strip_prefix("Enabled: ") {
                enabled = v.trim().eq_ignore_ascii_case("yes");
            } else if let Some(v) = line.strip_prefix("Server: ") {
                host = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("Port: ") {
                port = v.trim().to_string();
            }
        }
        (enabled, host, port)
    }

    /// How many services are read at once.
    ///
    /// The reads are the half of a toggle that is safe to run concurrently:
    /// `-getwebproxy` opens the preferences store, reads and closes, and two
    /// readers do not interfere. The *writes* stay serial on purpose — they
    /// commit to that same store, and two commits racing can lose one
    /// service's change, which for a proxy means a machine left half-proxied
    /// with no working internet.
    ///
    /// Eight rather than one-thread-per-service: a machine with a dozen VPN
    /// and virtual adapters would otherwise spawn a dozen threads to wait on
    /// a dozen processes, and past eight the wall clock is bounded by
    /// `networksetup` itself, not by how many we start.
    const READ_CONCURRENCY: usize = 8;

    pub fn snapshot() -> Backup {
        let services = parse_services(&capture("networksetup", &["-listallnetworkservices"]));
        // Contiguous chunks, joined in chunk order, so the result is in the
        // same order as the services list — a backup is compared against that
        // list by the helper before it is applied.
        let per_thread = services.len().div_ceil(READ_CONCURRENCY).max(1);
        let mut out = Vec::with_capacity(services.len());
        std::thread::scope(|scope| {
            let handles: Vec<_> = services
                .chunks(per_thread)
                .map(|group| scope.spawn(move || group.iter().map(read_service).collect::<Vec<_>>()))
                .collect();
            for handle in handles {
                match handle.join() {
                    Ok(group) => out.extend(group),
                    // A dropped group is a service a later restore will not put
                    // back. Silence here would look like a machine that simply
                    // had fewer services, so it is said out loud.
                    Err(_) => tracing::error!(
                        "a proxy-read thread panicked; this snapshot is incomplete"
                    ),
                }
            }
        });
        Backup { services: out, ..Default::default() }
    }

    /// Both proxy settings for one service, two `networksetup` reads.
    fn read_service(service: &String) -> ServiceBackup {
        let (we, wh, wp) = parse_proxy(&capture("networksetup", &["-getwebproxy", service]));
        let (se, sh, sp) = parse_proxy(&capture("networksetup", &["-getsecurewebproxy", service]));
        ServiceBackup {
            service: service.clone(),
            web_enabled: we,
            web_host: wh,
            web_port: wp,
            secure_enabled: se,
            secure_host: sh,
            secure_port: sp,
        }
    }

    /// Point every snapshotted service at `host:port`.
    ///
    /// Two steps per service, not four. `networksetup -help` on
    /// `-setwebproxy`: "Set Web proxy for <networkservice> with <domain> and
    /// <port number>. **Turns proxy on.**" — so the `-setwebproxystate … on`
    /// that used to follow each one was a no-op, and paying for it doubled the
    /// only part of a toggle the user waits on. Every step here is a separate
    /// `networksetup` process (macOS offers no batch form), so halving the
    /// steps halves the wall clock.
    pub fn enable_plan(host: &str, port: u16, backup: &Backup) -> Plan {
        let port = port.to_string();
        let mut steps = Vec::new();
        for s in &backup.services {
            for flag in ["-setwebproxy", "-setsecurewebproxy"] {
                steps.push(Step::with_args(
                    "networksetup",
                    vec![flag.to_string(), s.service.clone(), host.to_string(), port.clone()],
                ));
            }
        }
        Plan::new(steps, Elevation::MacAdmin)
    }

    /// Put every service back exactly as it was.
    pub fn disable_plan(backup: &Backup) -> Plan {
        let mut steps = Vec::new();
        for s in &backup.services {
            steps.extend(restore_half(
                &s.service,
                s.web_enabled,
                &s.web_host,
                &s.web_port,
                "-setwebproxy",
                "-setwebproxystate",
            ));
            steps.extend(restore_half(
                &s.service,
                s.secure_enabled,
                &s.secure_host,
                &s.secure_port,
                "-setsecurewebproxy",
                "-setsecurewebproxystate",
            ));
        }
        // Best-effort: one service already in the target state must not abandon
        // the rest, or the user is left half-proxied with no internet.
        Plan::best_effort(steps, Elevation::MacAdmin)
    }

    fn restore_half(
        service: &str,
        was_enabled: bool,
        host: &str,
        port: &str,
        set_flag: &str,
        state_flag: &str,
    ) -> Vec<Step> {
        if was_enabled && !host.is_empty() {
            // One step: the setter turns the proxy on by itself (see
            // `enable_plan`), so the `on` that used to follow it did nothing.
            vec![Step::with_args(
                "networksetup",
                vec![
                    set_flag.to_string(),
                    service.to_string(),
                    host.to_string(),
                    port.to_string(),
                ],
            )]
        } else {
            vec![Step::with_args(
                "networksetup",
                vec![state_flag.to_string(), service.to_string(), "off".to_string()],
            )]
        }
    }
}

/* -------------------------------- Windows -------------------------------- */

/// The Windows proxy *model*: what to write, given what was there. Pure data,
/// compiled everywhere so it is tested on every CI runner; the WinINet calls
/// that read and apply it are in [`crate::windows::inet`].
///
/// Why WinINet's per-connection options rather than the `Internet Settings`
/// registry values the first version wrote with `reg.exe`:
///
/// - **Running apps never noticed.** WinINet caches its settings; a registry
///   write is invisible until something calls `InternetSetOption(…REFRESH)`.
///   Chrome and Edge kept going direct until restarted.
/// - **"Automatically detect settings" stayed on.** That flag lives in the
///   binary `DefaultConnectionSettings` blob, not in a named value, and WPAD
///   wins over a manual proxy. The per-connection API owns that blob.
/// - **VPN connections were missed.** Each RAS/VPN entry has its own settings,
///   used instead of the LAN ones while it is connected.
pub mod windows {
    use super::*;

    /// `PROXY_TYPE_*` bits of `INTERNET_PER_CONN_FLAGS`.
    pub const DIRECT: u32 = 0x1;
    pub const PROXY: u32 = 0x2;
    pub const AUTO_PROXY_URL: u32 = 0x4;
    pub const AUTO_DETECT: u32 = 0x8;

    /// Bypass list used when the user had none. `<local>` is the Windows idiom
    /// for "don't proxy plain hostnames", which keeps intranet names direct.
    pub const DEFAULT_BYPASS: &str = "<local>";

    /// One WinINet connection's proxy settings.
    #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ConnectionSettings {
        /// `None` for the LAN default; otherwise a RAS/VPN entry name.
        #[serde(default)]
        pub name: Option<String>,
        /// `PROXY_TYPE_*` bits.
        pub flags: u32,
        #[serde(default)]
        pub server: Option<String>,
        #[serde(default)]
        pub bypass: Option<String>,
        /// A PAC URL. Kept while we are active (only its flag is cleared), so
        /// restoring is a flag flip and the URL can never be lost.
        #[serde(default)]
        pub autoconfig_url: Option<String>,
    }

    /// Point one connection at `host:port`.
    ///
    /// Auto-detect and the PAC flag are cleared because both win over a manual
    /// proxy; the user's bypass list survives so their intranet stays direct.
    pub fn enabled(host: &str, port: u16, prior: &ConnectionSettings) -> ConnectionSettings {
        ConnectionSettings {
            name: prior.name.clone(),
            flags: DIRECT | PROXY,
            server: Some(format!("{host}:{port}")),
            bypass: Some(
                prior
                    .bypass
                    .clone()
                    .filter(|b| !b.trim().is_empty())
                    .unwrap_or_else(|| DEFAULT_BYPASS.to_string()),
            ),
            autoconfig_url: prior.autoconfig_url.clone(),
        }
    }

    /// What a restore writes back, connection by connection.
    ///
    /// A snapshot from a build before 0.7 has no `connections`, only the raw
    /// registry values; those describe the LAN connection, and are translated
    /// into the flags they meant. Such a build never touched auto-detect, so
    /// the restore leaves it off — the state that build left the machine in.
    pub fn to_restore(backup: &WindowsBackup) -> Vec<ConnectionSettings> {
        if !backup.connections.is_empty() {
            return backup.connections.clone();
        }
        let mut flags = DIRECT;
        if backup.proxy_enable == Some(1) {
            flags |= PROXY;
        }
        if backup.auto_config_url.is_some() {
            flags |= AUTO_PROXY_URL;
        }
        vec![ConnectionSettings {
            name: None,
            flags,
            server: backup.proxy_server.clone(),
            bypass: backup.proxy_override.clone(),
            autoconfig_url: backup.auto_config_url.clone(),
        }]
    }

    /// The `RunOnce` value that restores the proxy at the next sign-in, and the
    /// command-line flag it launches the app with.
    ///
    /// Belt and braces for the one exit no hook sees: a restart or sign-out
    /// while the proxy is on kills the process, and until the user happened to
    /// open NovaProxy again every app on the machine pointed at a dead port.
    /// The value is written on enable and removed on every clean restore.
    pub const RUN_ONCE_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\RunOnce";
    pub const RUN_ONCE_VALUE: &str = "NovaProxyRestoreSystemProxy";
    pub const RESTORE_ARG: &str = "--restore-system-proxy";

    /// `"C:\…\NovaProxy.exe" --restore-system-proxy`, quoted for `RunOnce`.
    pub fn run_once_command(exe: &std::path::Path) -> String {
        format!("\"{}\" {RESTORE_ARG}", exe.display())
    }
}

/* --------------------------------- Linux --------------------------------- */

pub mod linux {
    use super::*;

    pub const SCHEMA: &str = "org.gnome.system.proxy";

    /// `gsettings get` quotes strings (`'manual'`) and prints numbers bare.
    pub fn parse_gsettings(output: &str) -> String {
        output.trim().trim_matches('\'').to_string()
    }

    fn get(schema: &str, key: &str) -> String {
        parse_gsettings(&capture("gsettings", &["get", schema, key]))
    }

    pub fn snapshot() -> Backup {
        let gnome = GnomeBackup {
            mode: get(SCHEMA, "mode"),
            http_host: get("org.gnome.system.proxy.http", "host"),
            http_port: get("org.gnome.system.proxy.http", "port"),
            https_host: get("org.gnome.system.proxy.https", "host"),
            https_port: get("org.gnome.system.proxy.https", "port"),
        };
        Backup { gnome: Some(gnome), ..Default::default() }
    }

    fn set(schema: &str, key: &str, value: &str) -> Step {
        Step::with_args(
            "gsettings",
            vec![
                "set".to_string(),
                schema.to_string(),
                key.to_string(),
                value.to_string(),
            ],
        )
    }

    /// Switch GNOME to manual proxying at `host:port` for HTTP and HTTPS.
    pub fn enable_plan(host: &str, port: u16) -> Plan {
        let port = port.to_string();
        Plan::new(
            vec![
                set("org.gnome.system.proxy.http", "host", host),
                set("org.gnome.system.proxy.http", "port", &port),
                set("org.gnome.system.proxy.https", "host", host),
                set("org.gnome.system.proxy.https", "port", &port),
                // Mode last: nothing is proxied until the hosts are in place.
                set(SCHEMA, "mode", "manual"),
            ],
            // dconf is per-user; no root, no prompt.
            Elevation::None,
        )
    }

    /// Restore the previous mode and manual hosts. `mode` comes back last for the
    /// same reason it went in last.
    pub fn disable_plan(backup: &GnomeBackup) -> Plan {
        let mode = if backup.mode.is_empty() { "none" } else { &backup.mode };
        Plan::best_effort(
            vec![
                set("org.gnome.system.proxy.http", "host", &backup.http_host),
                set(
                    "org.gnome.system.proxy.http",
                    "port",
                    if backup.http_port.is_empty() { "0" } else { &backup.http_port },
                ),
                set("org.gnome.system.proxy.https", "host", &backup.https_host),
                set(
                    "org.gnome.system.proxy.https",
                    "port",
                    if backup.https_port.is_empty() { "0" } else { &backup.https_port },
                ),
                set(SCHEMA, "mode", mode),
            ],
            Elevation::None,
        )
    }
}

/* ------------------------------ entry points ------------------------------ */

/// Capture the current proxy state so it can be restored later.
pub fn snapshot() -> Backup {
    let t = std::time::Instant::now();
    let before = crate::oscmd::exec_stats();
    let backup = snapshot_inner();
    let (spawns, spawn_ms) = delta(before);
    // The count, never the names: which VPN or Wi-Fi network someone is on is
    // theirs. A count is still the thing that matters — a snapshot with zero
    // services means the later restore will silently put nothing back, which
    // is exactly the failure that looks like "NovaProxy broke my internet".
    //
    // The spawn count belongs next to it because macOS reads a service's proxy
    // one `networksetup` at a time, twice per service: `services` and `spawns`
    // together say whether a slow toggle is a machine with many services or a
    // machine where each call is slow. `spawn_ms` sums over threads and the
    // reads run in parallel, so it is expected to exceed `ms` — see
    // `oscmd::EXEC_COUNT`.
    tracing::info!(
        services = backup.services.len(),
        connections = backup.windows.as_ref().map_or(0, |w| w.connections.len()),
        ms = t.elapsed().as_millis() as u64,
        spawns,
        spawn_ms,
        "captured system proxy snapshot"
    );
    if backup.services.is_empty() && backup.windows.is_none() && backup.gnome.is_none() {
        tracing::warn!("snapshot is empty; a later restore will have nothing to put back");
    }
    backup
}

fn snapshot_inner() -> Backup {
    #[cfg(target_os = "macos")]
    {
        macos::snapshot()
    }
    #[cfg(target_os = "windows")]
    {
        Backup { windows: Some(crate::windows::inet::snapshot()), ..Default::default() }
    }
    #[cfg(target_os = "linux")]
    {
        linux::snapshot()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        Backup::default()
    }
}

/// `(spawns, ms)` since a reading taken by [`crate::oscmd::exec_stats`].
fn delta(before: (u64, u64)) -> (u64, u64) {
    let now = crate::oscmd::exec_stats();
    (now.0.saturating_sub(before.0), now.1.saturating_sub(before.1))
}

/// Point the OS proxy at `host:port`.
///
/// On macOS the privileged helper does the work whenever one is installed, which
/// is the difference between a silent change and an administrator password
/// dialog; see [`crate::helper`]. Without a helper this falls back to the
/// prompting plan, so the feature keeps working before the helper is set up.
pub fn enable(host: &str, port: u16, backup: &Backup) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let backup = &macos::current(backup);
        if crate::helper::usable() {
            return crate::helper::enable(host, port, backup);
        }
        macos::enable_plan(host, port, backup).run()
    }
    #[cfg(target_os = "windows")]
    {
        let win = backup.windows.clone().unwrap_or_default();
        crate::windows::inet::enable(host, port, &win)
    }
    #[cfg(target_os = "linux")]
    {
        let _ = backup;
        require_gsettings()?;
        linux::enable_plan(host, port).run()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = (host, port, backup);
        anyhow::bail!("System proxy control is not implemented for this platform");
    }
}

/// Restore the snapshotted state.
///
/// Prefers the helper for the same reason [`enable`] does — and it matters more
/// here, because this is what crash recovery calls.
pub fn disable(backup: &Backup) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let backup = &macos::current(backup);
        if crate::helper::usable() {
            return crate::helper::disable(backup);
        }
        macos::disable_plan(backup).run()
    }
    #[cfg(target_os = "windows")]
    {
        let win = backup.windows.clone().unwrap_or_default();
        crate::windows::inet::disable(&win)
    }
    #[cfg(target_os = "linux")]
    {
        require_gsettings()?;
        linux::disable_plan(&backup.gnome.clone().unwrap_or_default()).run()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = backup;
        anyhow::bail!("System proxy control is not implemented for this platform");
    }
}

/// Can [`disable`] run right now without putting a prompt in front of anyone?
///
/// Decides whether the app restores the proxy by itself — at quit, before an
/// update, and at launch after a crash — or leaves it to a "Restore" button.
/// Windows and Linux settings are per-user, so always; macOS only through the
/// privileged helper, because otherwise it means an administrator password
/// dialog appearing during quit or before any window exists.
pub fn restore_is_silent() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::helper::usable()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// Fail with an actionable message rather than silently doing nothing on a
/// desktop that has no `gsettings`.
#[cfg(target_os = "linux")]
fn require_gsettings() -> Result<()> {
    if crate::oscmd::have_tool("gsettings") {
        return Ok(());
    }
    anyhow::bail!(
        "`gsettings` was not found, so the GNOME proxy cannot be set. Either install it, or use \
         \"Launch app through NovaProxy\" to inject HTTP(S)_PROXY into the app you are debugging."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell_of(plan: &Plan) -> Vec<String> {
        plan.steps.iter().map(|s| s.to_shell()).collect()
    }

    /* -------------------------------- macOS -------------------------------- */

    #[test]
    fn macos_parses_enabled_services_and_skips_disabled_ones() {
        let out = "An asterisk (*) denotes that a network service is disabled.\nWi-Fi\n*Bridge\nThunderbolt Bridge\n";
        assert_eq!(
            macos::parse_services(out),
            vec!["Wi-Fi".to_string(), "Thunderbolt Bridge".to_string()],
            "the header is skipped and '*' means disabled"
        );
    }

    #[test]
    fn macos_parses_a_proxy_readout() {
        let out = "Enabled: Yes\nServer: proxy.corp\nPort: 3128\nAuthenticated Proxy Enabled: 0\n";
        assert_eq!(
            macos::parse_proxy(out),
            (true, "proxy.corp".to_string(), "3128".to_string())
        );
        let off = "Enabled: No\nServer: \nPort: 0\n";
        assert_eq!(macos::parse_proxy(off), (false, String::new(), "0".to_string()));
    }

    #[test]
    fn macos_enable_points_both_proxies_of_every_service_in_one_step_each() {
        let backup = Backup {
            services: vec![service("Wi-Fi", false, "", ""), service("USB LAN", false, "", "")],
            ..Default::default()
        };
        let plan = macos::enable_plan("127.0.0.1", 9090, &backup);
        assert_eq!(plan.elevation, Elevation::MacAdmin, "networksetup needs admin");
        let lines = shell_of(&plan);
        assert_eq!(lines.len(), 4, "two services × (http, https), setter only");
        assert!(lines[0].contains("-setwebproxy Wi-Fi 127.0.0.1 9090"));
        assert!(lines.iter().any(|l| l.contains("-setsecurewebproxy 'USB LAN' 127.0.0.1 9090")));
        // The setter turns the proxy on by itself, so a separate `state on` is
        // a second `networksetup` process bought for nothing. Asserted as an
        // absence because that is the regression worth catching.
        assert!(
            !lines.iter().any(|l| l.contains("proxystate")),
            "the setter already turns it on: {lines:?}"
        );
    }

    #[test]
    fn macos_disable_restores_a_previous_proxy_verbatim() {
        let backup = Backup {
            services: vec![service("Wi-Fi", true, "proxy.corp", "3128")],
            ..Default::default()
        };
        let lines = shell_of(&macos::disable_plan(&backup));
        assert!(
            lines.iter().any(|l| l.contains("-setwebproxy Wi-Fi proxy.corp 3128")),
            "a corporate proxy must come back, not be turned off: {lines:?}"
        );
        // Restoring the host is what turns it back on; no `state on` needed.
        assert!(
            !lines.iter().any(|l| l.contains("-setwebproxystate Wi-Fi on")),
            "the setter already turns it on: {lines:?}"
        );
    }

    #[test]
    fn macos_disable_turns_off_a_service_that_had_no_proxy() {
        let backup = Backup {
            services: vec![service("Wi-Fi", false, "", "")],
            ..Default::default()
        };
        let lines = shell_of(&macos::disable_plan(&backup));
        assert!(lines.iter().any(|l| l.contains("-setwebproxystate Wi-Fi off")));
        assert!(
            !lines.iter().any(|l| l.contains("-setwebproxy Wi-Fi ")),
            "nothing to restore, so no host is written: {lines:?}"
        );
    }

    #[test]
    fn macos_restore_is_best_effort() {
        let backup = Backup {
            services: vec![service("Wi-Fi", false, "", ""), service("USB LAN", false, "", "")],
            ..Default::default()
        };
        assert!(
            macos::disable_plan(&backup).best_effort,
            "one failing service must not strand the others without internet"
        );
    }

    #[test]
    fn macos_enable_of_an_empty_snapshot_does_nothing() {
        assert!(macos::enable_plan("127.0.0.1", 9090, &Backup::default()).is_empty());
    }

    #[test]
    fn macos_drops_services_that_no_longer_exist() {
        let backup = Backup {
            services: vec![
                service("Wi-Fi", false, "", ""),
                service("35.157.114.18 [u-vpn-haht]", false, "", ""),
            ],
            ..Default::default()
        };
        let known = vec!["Wi-Fi".to_string(), "Thunderbolt Bridge".to_string()];
        let kept = macos::without_missing(&backup, &known);
        let names: Vec<&str> = kept.services.iter().map(|s| s.service.as_str()).collect();
        assert_eq!(names, vec!["Wi-Fi"]);
    }

    fn service(name: &str, enabled: bool, host: &str, port: &str) -> ServiceBackup {
        ServiceBackup {
            service: name.to_string(),
            web_enabled: enabled,
            web_host: host.to_string(),
            web_port: port.to_string(),
            secure_enabled: enabled,
            secure_host: host.to_string(),
            secure_port: port.to_string(),
        }
    }

    /* ------------------------------- Windows ------------------------------- */

    fn lan(flags: u32) -> windows::ConnectionSettings {
        windows::ConnectionSettings { flags, ..Default::default() }
    }

    #[test]
    fn windows_enable_turns_on_the_manual_proxy_and_nothing_that_would_beat_it() {
        let prior = windows::ConnectionSettings {
            flags: windows::DIRECT | windows::AUTO_DETECT | windows::AUTO_PROXY_URL,
            autoconfig_url: Some("http://corp/proxy.pac".into()),
            ..Default::default()
        };
        let on = windows::enabled("127.0.0.1", 9090, &prior);
        assert_eq!(on.server.as_deref(), Some("127.0.0.1:9090"));
        assert_eq!(on.flags, windows::DIRECT | windows::PROXY);
        assert_eq!(
            on.flags & (windows::AUTO_DETECT | windows::AUTO_PROXY_URL),
            0,
            "WPAD and a PAC URL both win over a manual proxy"
        );
        assert_eq!(
            on.autoconfig_url.as_deref(),
            Some("http://corp/proxy.pac"),
            "the URL itself is kept, so a restore is a flag flip and it can never be lost"
        );
    }

    #[test]
    fn windows_enable_keeps_the_users_bypass_list_or_defaults_to_local() {
        let prior = windows::ConnectionSettings {
            bypass: Some("*.corp.example;<local>".into()),
            ..Default::default()
        };
        assert_eq!(
            windows::enabled("127.0.0.1", 1, &prior).bypass.as_deref(),
            Some("*.corp.example;<local>")
        );
        assert_eq!(windows::enabled("127.0.0.1", 1, &lan(1)).bypass.as_deref(), Some("<local>"));
        let blank = windows::ConnectionSettings { bypass: Some("  ".into()), ..Default::default() };
        assert_eq!(windows::enabled("127.0.0.1", 1, &blank).bypass.as_deref(), Some("<local>"));
    }

    #[test]
    fn windows_enable_keeps_the_connection_name() {
        let vpn = windows::ConnectionSettings { name: Some("Corp VPN".into()), ..lan(1) };
        assert_eq!(windows::enabled("127.0.0.1", 1, &vpn).name.as_deref(), Some("Corp VPN"));
    }

    #[test]
    fn windows_restore_puts_every_connection_back_verbatim() {
        let backup = WindowsBackup {
            connections: vec![
                windows::ConnectionSettings {
                    flags: windows::DIRECT | windows::AUTO_DETECT,
                    ..Default::default()
                },
                windows::ConnectionSettings {
                    name: Some("Corp VPN".into()),
                    flags: windows::DIRECT | windows::PROXY,
                    server: Some("proxy.corp:3128".into()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert_eq!(windows::to_restore(&backup), backup.connections);
    }

    #[test]
    fn windows_restores_a_snapshot_written_by_an_older_build() {
        // Pre-0.7 snapshots are raw `Internet Settings` values. A user who
        // upgrades while the proxy is on must still get their PAC setup back.
        let json = r#"{"windows":{"proxy_enable":0,"proxy_server":null,
                       "proxy_override":"<local>","auto_config_url":"http://corp/proxy.pac"}}"#;
        let backup: Backup = serde_json::from_str(json).expect("old snapshot parses");
        let restore = windows::to_restore(backup.windows.as_ref().unwrap());
        assert_eq!(restore.len(), 1);
        let lan = &restore[0];
        assert_eq!(lan.name, None, "the old build only ever touched the LAN settings");
        assert_eq!(lan.flags, windows::DIRECT | windows::AUTO_PROXY_URL);
        assert_eq!(lan.autoconfig_url.as_deref(), Some("http://corp/proxy.pac"));
        assert_eq!(lan.bypass.as_deref(), Some("<local>"));
    }

    #[test]
    fn windows_old_snapshot_with_a_manual_proxy_comes_back_on() {
        let old = WindowsBackup {
            proxy_enable: Some(1),
            proxy_server: Some("proxy.corp:3128".into()),
            ..Default::default()
        };
        let lan = &windows::to_restore(&old)[0];
        assert_eq!(lan.flags, windows::DIRECT | windows::PROXY);
        assert_eq!(lan.server.as_deref(), Some("proxy.corp:3128"));
    }

    #[test]
    fn windows_new_snapshots_do_not_write_the_legacy_fields() {
        let backup = WindowsBackup { connections: vec![lan(1)], ..Default::default() };
        let json = serde_json::to_string(&backup).unwrap();
        assert!(!json.contains("proxy_enable"), "got {json}");
    }

    #[test]
    fn windows_run_once_command_quotes_the_path() {
        let cmd = windows::run_once_command(std::path::Path::new(r"C:\Program Files\NovaProxy\NovaProxy.exe"));
        assert_eq!(
            cmd,
            r#""C:\Program Files\NovaProxy\NovaProxy.exe" --restore-system-proxy"#
        );
    }

    /* -------------------------------- Linux -------------------------------- */

    #[test]
    fn linux_parses_gsettings_quoting() {
        assert_eq!(linux::parse_gsettings("'manual'\n"), "manual");
        assert_eq!(linux::parse_gsettings("8080\n"), "8080");
        assert_eq!(linux::parse_gsettings("''\n"), "");
    }

    #[test]
    fn linux_enable_sets_hosts_before_switching_mode() {
        let plan = linux::enable_plan("127.0.0.1", 9090);
        assert_eq!(plan.elevation, Elevation::None, "dconf is per-user");
        let lines = shell_of(&plan);
        assert_eq!(lines.len(), 5);
        assert!(lines[0].contains("org.gnome.system.proxy.http host 127.0.0.1"));
        assert!(lines.iter().any(|l| l.contains("org.gnome.system.proxy.https port 9090")));
        assert!(
            lines.last().unwrap().contains("org.gnome.system.proxy mode manual"),
            "mode goes last so traffic is never pointed at an unset host: {lines:?}"
        );
    }

    #[test]
    fn linux_disable_restores_the_previous_mode_and_hosts() {
        let backup = GnomeBackup {
            mode: "auto".into(),
            http_host: "proxy.corp".into(),
            http_port: "3128".into(),
            https_host: "proxy.corp".into(),
            https_port: "3128".into(),
        };
        let lines = shell_of(&linux::disable_plan(&backup));
        assert!(lines.iter().any(|l| l.contains("http host proxy.corp")));
        assert!(lines.iter().any(|l| l.contains("http port 3128")));
        assert!(
            lines.last().unwrap().contains("mode auto"),
            "a PAC ('auto') setup must come back as auto, not none: {lines:?}"
        );
        assert!(linux::disable_plan(&backup).best_effort);
    }

    #[test]
    fn linux_disable_defaults_to_no_proxy_when_the_snapshot_was_empty() {
        let lines = shell_of(&linux::disable_plan(&GnomeBackup::default()));
        assert!(lines.last().unwrap().contains("mode none"));
        assert!(
            lines.iter().any(|l| l.contains("http port 0")),
            "an empty port would be rejected by gsettings: {lines:?}"
        );
    }

    /* ------------------------- cross-platform contract ------------------------- */

    #[test]
    fn a_macos_only_snapshot_from_an_older_build_still_deserializes() {
        // Upgrading mid-session must not orphan a pending restore.
        let json = r#"{"services":[{"service":"Wi-Fi","web_enabled":true,"web_host":"proxy.corp",
                       "web_port":"3128","secure_enabled":false,"secure_host":"","secure_port":"0"}]}"#;
        let backup: Backup = serde_json::from_str(json).expect("old snapshot parses");
        assert_eq!(backup.services.len(), 1);
        assert_eq!(backup.services[0].web_host, "proxy.corp");
        assert!(backup.windows.is_none() && backup.gnome.is_none());
    }

    #[test]
    fn a_snapshot_round_trips_through_json() {
        let backup = Backup {
            services: vec![service("Wi-Fi", true, "proxy.corp", "3128")],
            windows: Some(WindowsBackup {
                connections: vec![windows::ConnectionSettings {
                    name: Some("Corp VPN".into()),
                    flags: windows::DIRECT | windows::AUTO_PROXY_URL,
                    server: None,
                    bypass: Some("<local>".into()),
                    autoconfig_url: Some("http://corp/p.pac".into()),
                }],
                ..Default::default()
            }),
            gnome: Some(GnomeBackup { mode: "manual".into(), ..Default::default() }),
        };
        let json = serde_json::to_string(&backup).unwrap();
        let back: Backup = serde_json::from_str(&json).unwrap();
        assert_eq!(back.services[0].web_host, "proxy.corp");
        assert_eq!(back.windows, backup.windows);
        assert_eq!(back.gnome.unwrap().mode, "manual");
    }

    #[test]
    fn every_platform_restore_is_best_effort() {
        // The contract that matters: a partial failure must never stop a restore
        // halfway, on any platform. (Windows applies connections one at a time in
        // `windows::inet::disable`, which carries on past a failing one.)
        assert!(macos::disable_plan(&Backup { services: vec![service("Wi-Fi", false, "", "")], ..Default::default() }).best_effort);
        assert!(linux::disable_plan(&GnomeBackup::default()).best_effort);
    }

    #[test]
    fn only_macos_needs_elevation_to_set_the_system_proxy() {
        let mac = macos::enable_plan(
            "127.0.0.1",
            1,
            &Backup { services: vec![service("Wi-Fi", false, "", "")], ..Default::default() },
        );
        assert_eq!(mac.elevation, Elevation::MacAdmin);
        assert_eq!(linux::enable_plan("127.0.0.1", 1).elevation, Elevation::None);
    }
}
