//! Best-effort attribution of a captured flow to the local process that opened
//! the connection.
//!
//! When an app sends a request through the proxy, the connection's source port
//! (the `client_addr` we record) is the app socket's *local* port from the OS's
//! point of view. We look that port up in the kernel's TCP table to find the
//! owning PID, then resolve the PID to a process name. Local connections are
//! keep-alive, so a port maps to one process for its lifetime; we still expire
//! the cache so reused ephemeral ports don't misattribute over time.
//!
//! macOS-only for now (uses `netstat2` + `libproc`). Other platforms always
//! return `None`, leaving `Flow::process`/`Flow::pid` unset.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The process that owns a connection's source port.
#[derive(Clone, Debug)]
pub struct ProcInfo {
    pub pid: u32,
    pub name: String,
}

/// Caches a source-port → owning-process map, refreshed lazily.
pub struct ProcResolver {
    cache: Mutex<Cache>,
}

struct Cache {
    ports: HashMap<u16, ProcInfo>,
    refreshed_at: Option<Instant>,
}

/// How long a port→process snapshot is trusted before we rebuild it.
const TTL: Duration = Duration::from_millis(1000);

impl ProcResolver {
    fn new() -> Self {
        Self {
            cache: Mutex::new(Cache {
                ports: HashMap::new(),
                refreshed_at: None,
            }),
        }
    }

    /// Resolve the process behind a `client_addr` of the form `ip:port`.
    /// Returns `None` if the port can't be parsed, isn't found, or the platform
    /// is unsupported.
    pub fn resolve(&self, client_addr: &str) -> Option<ProcInfo> {
        let port = port_of(client_addr)?;

        let mut cache = self.cache.lock().unwrap();
        let stale = cache
            .refreshed_at
            .map_or(true, |t| t.elapsed() > TTL);
        // Rebuild if the port is unknown (likely a brand-new connection) or the
        // snapshot has aged out.
        if stale || !cache.ports.contains_key(&port) {
            cache.ports = snapshot();
            cache.refreshed_at = Some(Instant::now());
        }
        cache.ports.get(&port).cloned()
    }
}

/// Extract the port from a socket-address string. Handles both IPv4
/// (`127.0.0.1:52341`) and bracketed IPv6 (`[::1]:52341`) renderings, since
/// the port is always the segment after the last `:`.
fn port_of(client_addr: &str) -> Option<u16> {
    client_addr.rsplit(':').next()?.parse().ok()
}

/// Process-global resolver, shared across all in-flight requests.
pub fn global() -> &'static ProcResolver {
    static RESOLVER: OnceLock<ProcResolver> = OnceLock::new();
    RESOLVER.get_or_init(ProcResolver::new)
}

#[cfg(target_os = "macos")]
fn snapshot() -> HashMap<u16, ProcInfo> {
    use netstat2::{
        get_sockets_info, AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo,
    };

    let af = AddressFamilyFlags::IPV4 | AddressFamilyFlags::IPV6;
    let mut map: HashMap<u16, ProcInfo> = HashMap::new();

    let Ok(sockets) = get_sockets_info(af, ProtocolFlags::TCP) else {
        return map;
    };

    // Cache pid→name within one snapshot so multiple sockets of one app don't
    // re-query the process table.
    let mut names: HashMap<u32, String> = HashMap::new();
    for si in sockets {
        let ProtocolSocketInfo::Tcp(tcp) = si.protocol_socket_info else {
            continue;
        };
        let Some(&pid) = si.associated_pids.first() else {
            continue;
        };
        let name = names
            .entry(pid)
            .or_insert_with(|| app_name(pid).unwrap_or_else(|| format!("pid {pid}")))
            .clone();
        // Keep the first process seen for a given local port.
        map.entry(tcp.local_port)
            .or_insert(ProcInfo { pid, name });
    }
    map
}

/// Resolve a PID to the app the user would recognize.
///
/// Many apps do their networking in a child process (Chrome's "Google Chrome
/// Helper", Electron/Safari helpers, XPC services). Those helper binaries live
/// *inside* the parent app's `.app` bundle, so we roll the socket-owning PID up
/// to its outermost bundle name. Standalone executables that aren't part of a
/// bundle (CLI tools, daemons, dev servers) keep their own process name.
#[cfg(target_os = "macos")]
fn app_name(pid: u32) -> Option<String> {
    if let Ok(path) = libproc::proc_pid::pidpath(pid as i32) {
        if let Some(app) = bundle_app_name(&path) {
            return Some(app);
        }
    }
    libproc::proc_pid::name(pid as i32).ok().filter(|n| !n.is_empty())
}

/// The outermost `*.app` bundle name in an executable path, if any. The *first*
/// `.app` in the path is the top-level application even when a helper bundle is
/// nested deeper, e.g.
/// `/Applications/Google Chrome.app/…/Google Chrome Helper.app/…` → `Google Chrome`.
#[cfg(target_os = "macos")]
fn bundle_app_name(path: &str) -> Option<String> {
    let idx = path.find(".app/")?;
    let name = path[..idx].rsplit('/').next()?;
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::{port_of, ProcResolver};

    #[test]
    fn port_of_parses_ipv4_and_ipv6_renderings() {
        assert_eq!(port_of("127.0.0.1:52341"), Some(52341));
        assert_eq!(port_of("[::1]:52341"), Some(52341));
        assert_eq!(port_of("[fe80::1%lo0]:80"), Some(80));
    }

    #[test]
    fn port_of_rejects_malformed_addresses() {
        assert_eq!(port_of(""), None);
        assert_eq!(port_of("127.0.0.1"), None); // no port segment
        assert_eq!(port_of("127.0.0.1:"), None); // empty port
        assert_eq!(port_of("127.0.0.1:notaport"), None);
        assert_eq!(port_of("127.0.0.1:70000"), None); // > u16::MAX
    }

    #[test]
    fn resolve_returns_none_for_unparseable_addr() {
        let r = ProcResolver::new();
        assert!(r.resolve("garbage").is_none());
        assert!(r.resolve("127.0.0.1:notaport").is_none());
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::bundle_app_name;

    #[test]
    fn rolls_helper_up_to_top_level_app() {
        let chrome = "/Applications/Google Chrome.app/Contents/Frameworks/\
            Google Chrome Framework.framework/Versions/1/Helpers/\
            Google Chrome Helper.app/Contents/MacOS/Google Chrome Helper";
        assert_eq!(bundle_app_name(chrome).as_deref(), Some("Google Chrome"));

        let electron = "/Applications/Slack.app/Contents/Frameworks/\
            Slack Helper.app/Contents/MacOS/Slack Helper";
        assert_eq!(bundle_app_name(electron).as_deref(), Some("Slack"));
    }

    #[test]
    fn standalone_binary_has_no_bundle() {
        assert_eq!(bundle_app_name("/usr/bin/curl"), None);
        assert_eq!(bundle_app_name("/opt/homebrew/bin/node"), None);
    }
}

#[cfg(not(target_os = "macos"))]
fn snapshot() -> HashMap<u16, ProcInfo> {
    HashMap::new()
}
