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
//! | Windows | `HKCU\…\Internet Settings` via `reg.exe` | none — the settings are per-user |
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

/// Saved `Internet Settings` values (Windows). Absent values mean the registry
/// value did not exist and must not exist again after a restore.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowsBackup {
    pub proxy_enable: Option<u32>,
    pub proxy_server: Option<String>,
    pub proxy_override: Option<String>,
    /// A PAC/auto-config URL, which takes precedence over manual settings and is
    /// therefore cleared while we are active — and put back on restore.
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

    pub fn snapshot() -> Backup {
        let services = parse_services(&capture("networksetup", &["-listallnetworkservices"]));
        let mut out = Vec::new();
        for service in services {
            let (we, wh, wp) = parse_proxy(&capture("networksetup", &["-getwebproxy", &service]));
            let (se, sh, sp) =
                parse_proxy(&capture("networksetup", &["-getsecurewebproxy", &service]));
            out.push(ServiceBackup {
                service,
                web_enabled: we,
                web_host: wh,
                web_port: wp,
                secure_enabled: se,
                secure_host: sh,
                secure_port: sp,
            });
        }
        Backup { services: out, ..Default::default() }
    }

    /// Point every snapshotted service at `host:port`.
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
            for flag in ["-setwebproxystate", "-setsecurewebproxystate"] {
                steps.push(Step::with_args(
                    "networksetup",
                    vec![flag.to_string(), s.service.clone(), "on".to_string()],
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
            vec![
                Step::with_args(
                    "networksetup",
                    vec![
                        set_flag.to_string(),
                        service.to_string(),
                        host.to_string(),
                        port.to_string(),
                    ],
                ),
                Step::with_args(
                    "networksetup",
                    vec![state_flag.to_string(), service.to_string(), "on".to_string()],
                ),
            ]
        } else {
            vec![Step::with_args(
                "networksetup",
                vec![state_flag.to_string(), service.to_string(), "off".to_string()],
            )]
        }
    }
}

/* -------------------------------- Windows -------------------------------- */

pub mod windows {
    use super::*;

    pub const INTERNET_SETTINGS: &str =
        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";

    /// Bypass list used when the user had none. `<local>` is the Windows idiom for
    /// "don't proxy plain hostnames", which keeps localhost traffic direct.
    pub const DEFAULT_BYPASS: &str = "<local>";

    /// Extract a value from `reg query … /v <name>` output.
    ///
    /// Lines look like `    ProxyServer    REG_SZ    127.0.0.1:9090`; the value
    /// itself may contain spaces, so only the first two columns are split off.
    pub fn parse_reg_value(output: &str, name: &str) -> Option<String> {
        for line in output.lines() {
            // `reg query` prints a blank line, the key path, then the values, so
            // non-matching lines are skipped rather than ending the search.
            let Some(rest) = line.trim().strip_prefix(name) else { continue };
            // The type column is next; skip it and keep everything after, since
            // the value itself may contain spaces.
            let mut parts = rest.trim_start().splitn(2, char::is_whitespace);
            let Some(ty) = parts.next() else { continue };
            if !ty.starts_with("REG_") {
                continue;
            }
            return Some(parts.next().unwrap_or("").trim().to_string());
        }
        None
    }

    /// `REG_DWORD` values print as `0x1`.
    pub fn parse_dword(value: &str) -> Option<u32> {
        let v = value.trim();
        let hex = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X"));
        match hex {
            Some(h) => u32::from_str_radix(h, 16).ok(),
            None => v.parse().ok(),
        }
    }

    fn query(name: &str) -> Option<String> {
        let out = capture("reg", &["query", INTERNET_SETTINGS, "/v", name]);
        parse_reg_value(&out, name).filter(|v| !v.is_empty())
    }

    pub fn snapshot() -> Backup {
        let win = WindowsBackup {
            proxy_enable: query("ProxyEnable").and_then(|v| parse_dword(&v)),
            proxy_server: query("ProxyServer"),
            proxy_override: query("ProxyOverride"),
            auto_config_url: query("AutoConfigURL"),
        };
        Backup { windows: Some(win), ..Default::default() }
    }

    fn set_sz(name: &str, value: &str) -> Step {
        Step::with_args(
            "reg",
            vec![
                "add".to_string(),
                INTERNET_SETTINGS.to_string(),
                "/v".to_string(),
                name.to_string(),
                "/t".to_string(),
                "REG_SZ".to_string(),
                "/d".to_string(),
                value.to_string(),
                "/f".to_string(),
            ],
        )
    }

    fn set_dword(name: &str, value: u32) -> Step {
        Step::with_args(
            "reg",
            vec![
                "add".to_string(),
                INTERNET_SETTINGS.to_string(),
                "/v".to_string(),
                name.to_string(),
                "/t".to_string(),
                "REG_DWORD".to_string(),
                "/d".to_string(),
                value.to_string(),
                "/f".to_string(),
            ],
        )
    }

    fn delete(name: &str) -> Step {
        Step::with_args(
            "reg",
            vec![
                "delete".to_string(),
                INTERNET_SETTINGS.to_string(),
                "/v".to_string(),
                name.to_string(),
                "/f".to_string(),
            ],
        )
    }

    /// Turn on the manual proxy, preserving the user's bypass list.
    ///
    /// A PAC URL wins over manual settings in WinINET, so an existing
    /// `AutoConfigURL` is removed while we are active — it is in the snapshot and
    /// comes back on restore.
    pub fn enable_plan(host: &str, port: u16, backup: &WindowsBackup) -> Plan {
        let mut steps = vec![
            set_sz("ProxyServer", &format!("{host}:{port}")),
            set_dword("ProxyEnable", 1),
        ];
        let bypass = backup
            .proxy_override
            .clone()
            .unwrap_or_else(|| DEFAULT_BYPASS.to_string());
        steps.push(set_sz("ProxyOverride", &bypass));
        if backup.auto_config_url.is_some() {
            steps.push(delete("AutoConfigURL"));
        }
        // HKCU: the user owns these keys, so no elevation is needed at all.
        Plan::new(steps, Elevation::None)
    }

    /// Restore exactly what was there — including a corporate PAC URL, and
    /// including *removing* values that did not exist before.
    pub fn disable_plan(backup: &WindowsBackup) -> Plan {
        let mut steps = Vec::new();
        match backup.proxy_enable {
            Some(v) => steps.push(set_dword("ProxyEnable", v)),
            None => steps.push(delete("ProxyEnable")),
        }
        match &backup.proxy_server {
            Some(v) => steps.push(set_sz("ProxyServer", v)),
            None => steps.push(delete("ProxyServer")),
        }
        match &backup.proxy_override {
            Some(v) => steps.push(set_sz("ProxyOverride", v)),
            None => steps.push(delete("ProxyOverride")),
        }
        if let Some(url) = &backup.auto_config_url {
            steps.push(set_sz("AutoConfigURL", url));
        }
        // Deleting a value that is already absent fails; every other step must
        // still run.
        Plan::best_effort(steps, Elevation::None)
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
    let backup = snapshot_inner();
    // The count, never the names: which VPN or Wi-Fi network someone is on is
    // theirs. A count is still the thing that matters — a snapshot with zero
    // services means the later restore will silently put nothing back, which
    // is exactly the failure that looks like "NovaProxy broke my internet".
    tracing::info!(
        services = backup.services.len(),
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
        windows::snapshot()
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

/// Point the OS proxy at `host:port`.
///
/// On macOS the privileged helper does the work whenever one is installed, which
/// is the difference between a silent change and an administrator password
/// dialog; see [`crate::helper`]. Without a helper this falls back to the
/// prompting plan, so the feature keeps working before the helper is set up.
pub fn enable(host: &str, port: u16, backup: &Backup) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        if crate::helper::usable() {
            return crate::helper::enable(host, port, backup);
        }
        macos::enable_plan(host, port, backup).run()
    }
    #[cfg(target_os = "windows")]
    {
        let win = backup.windows.clone().unwrap_or_default();
        windows::enable_plan(host, port, &win).run()
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
        if crate::helper::usable() {
            return crate::helper::disable(backup);
        }
        macos::disable_plan(backup).run()
    }
    #[cfg(target_os = "windows")]
    {
        let win = backup.windows.clone().unwrap_or_default();
        windows::disable_plan(&win).run()
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
    fn macos_enable_sets_then_turns_on_every_service() {
        let backup = Backup {
            services: vec![service("Wi-Fi", false, "", ""), service("USB LAN", false, "", "")],
            ..Default::default()
        };
        let plan = macos::enable_plan("127.0.0.1", 9090, &backup);
        assert_eq!(plan.elevation, Elevation::MacAdmin, "networksetup needs admin");
        let lines = shell_of(&plan);
        assert_eq!(lines.len(), 8, "two services × (http, https) × (set, state)");
        assert!(lines[0].contains("-setwebproxy Wi-Fi 127.0.0.1 9090"));
        assert!(lines.iter().any(|l| l.contains("-setsecurewebproxystate 'USB LAN' on")));
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
        assert!(lines.iter().any(|l| l.contains("-setwebproxystate Wi-Fi on")));
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

    #[test]
    fn windows_parses_reg_query_output() {
        let out = "\r\nHKEY_CURRENT_USER\\Software\\...\\Internet Settings\r\n    ProxyServer    REG_SZ    127.0.0.1:9090\r\n\r\n";
        assert_eq!(
            windows::parse_reg_value(out, "ProxyServer").as_deref(),
            Some("127.0.0.1:9090")
        );
        assert!(windows::parse_reg_value(out, "AutoConfigURL").is_none());
    }

    #[test]
    fn windows_parses_values_containing_spaces() {
        // A bypass list is semicolon-separated but can contain spaces.
        let out = "    ProxyOverride    REG_SZ    *.corp.example;<local>; 10.0.0.1\r\n";
        assert_eq!(
            windows::parse_reg_value(out, "ProxyOverride").as_deref(),
            Some("*.corp.example;<local>; 10.0.0.1")
        );
    }

    #[test]
    fn windows_parses_dwords_in_hex_or_decimal() {
        assert_eq!(windows::parse_dword("0x1"), Some(1));
        assert_eq!(windows::parse_dword("0x0"), Some(0));
        assert_eq!(windows::parse_dword(" 1 "), Some(1));
        assert_eq!(windows::parse_dword("nonsense"), None);
    }

    #[test]
    fn windows_enable_writes_server_and_flag_without_elevation() {
        let plan = windows::enable_plan("127.0.0.1", 9090, &WindowsBackup::default());
        assert_eq!(
            plan.elevation,
            Elevation::None,
            "HKCU is the user's own key — no UAC prompt at all"
        );
        let lines = shell_of(&plan);
        assert!(lines.iter().any(|l| l.contains("ProxyServer") && l.contains("127.0.0.1:9090")));
        assert!(lines.iter().any(|l| l.contains("ProxyEnable") && l.contains("REG_DWORD")));
        assert!(
            lines.iter().any(|l| l.contains("ProxyOverride") && l.contains("<local>")),
            "localhost stays direct by default: {lines:?}"
        );
    }

    #[test]
    fn windows_enable_preserves_an_existing_bypass_list() {
        let backup = WindowsBackup {
            proxy_override: Some("*.corp.example;<local>".into()),
            ..Default::default()
        };
        let lines = shell_of(&windows::enable_plan("127.0.0.1", 9090, &backup));
        assert!(
            lines.iter().any(|l| l.contains("*.corp.example;<local>")),
            "the user's own bypass list must survive: {lines:?}"
        );
    }

    #[test]
    fn windows_enable_clears_a_pac_url_because_it_would_win() {
        let backup = WindowsBackup {
            auto_config_url: Some("http://corp/proxy.pac".into()),
            ..Default::default()
        };
        let lines = shell_of(&windows::enable_plan("127.0.0.1", 9090, &backup));
        assert!(
            lines.iter().any(|l| l.contains("delete") && l.contains("AutoConfigURL")),
            "WinINET prefers a PAC URL, so manual settings would be ignored: {lines:?}"
        );
    }

    #[test]
    fn windows_enable_leaves_pac_alone_when_there_was_none() {
        let lines = shell_of(&windows::enable_plan("127.0.0.1", 9090, &WindowsBackup::default()));
        assert!(!lines.iter().any(|l| l.contains("AutoConfigURL")));
    }

    #[test]
    fn windows_disable_restores_a_corporate_pac_setup() {
        let backup = WindowsBackup {
            proxy_enable: Some(0),
            proxy_server: None,
            proxy_override: Some("<local>".into()),
            auto_config_url: Some("http://corp/proxy.pac".into()),
        };
        let lines = shell_of(&windows::disable_plan(&backup));
        assert!(lines.iter().any(|l| l.contains("AutoConfigURL") && l.contains("http://corp/proxy.pac")));
        assert!(
            lines.iter().any(|l| l.contains("delete") && l.contains("ProxyServer")),
            "a value that did not exist before must not exist after: {lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains("ProxyEnable") && l.ends_with("/f")));
        assert!(windows::disable_plan(&backup).best_effort);
    }

    #[test]
    fn windows_disable_from_a_clean_machine_removes_everything_we_added() {
        let lines = shell_of(&windows::disable_plan(&WindowsBackup::default()));
        for value in ["ProxyEnable", "ProxyServer", "ProxyOverride"] {
            assert!(
                lines.iter().any(|l| l.contains("delete") && l.contains(value)),
                "{value} was absent before and must be absent after: {lines:?}"
            );
        }
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
                proxy_enable: Some(1),
                proxy_server: Some("127.0.0.1:1".into()),
                proxy_override: None,
                auto_config_url: Some("http://corp/p.pac".into()),
            }),
            gnome: Some(GnomeBackup { mode: "manual".into(), ..Default::default() }),
        };
        let json = serde_json::to_string(&backup).unwrap();
        let back: Backup = serde_json::from_str(&json).unwrap();
        assert_eq!(back.services[0].web_host, "proxy.corp");
        assert_eq!(back.windows.unwrap().auto_config_url.as_deref(), Some("http://corp/p.pac"));
        assert_eq!(back.gnome.unwrap().mode, "manual");
    }

    #[test]
    fn every_platform_restore_is_best_effort() {
        // The contract that matters: a partial failure must never stop a restore
        // halfway, on any platform.
        assert!(macos::disable_plan(&Backup { services: vec![service("Wi-Fi", false, "", "")], ..Default::default() }).best_effort);
        assert!(windows::disable_plan(&WindowsBackup::default()).best_effort);
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
        assert_eq!(windows::enable_plan("127.0.0.1", 1, &WindowsBackup::default()).elevation, Elevation::None);
        assert_eq!(linux::enable_plan("127.0.0.1", 1).elevation, Elevation::None);
    }
}
