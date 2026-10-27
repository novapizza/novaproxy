//! WinINet proxy settings, per connection.
//!
//! The *model* — what to write given what was there — is pure data in
//! [`crate::sysproxy::windows`]; this module only reads and applies it. See
//! that module for why these calls replaced writing `Internet Settings` with
//! `reg.exe`.
//!
//! Everything here is the current user's: no elevation, no prompt.

use std::ffi::c_void;
use std::mem::size_of;

use anyhow::{bail, Result};
use windows_sys::Win32::Foundation::GlobalFree;
use windows_sys::Win32::NetworkManagement::Rras::{RasEnumEntriesW, ERROR_BUFFER_TOO_SMALL, RASENTRYNAMEW};
use windows_sys::Win32::Networking::WinInet::{
    InternetQueryOptionW, InternetSetOptionW, INTERNET_OPTION_PER_CONNECTION_OPTION,
    INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED, INTERNET_PER_CONN_AUTOCONFIG_URL,
    INTERNET_PER_CONN_FLAGS, INTERNET_PER_CONN_FLAGS_UI, INTERNET_PER_CONN_OPTIONW,
    INTERNET_PER_CONN_OPTION_LISTW, INTERNET_PER_CONN_PROXY_BYPASS, INTERNET_PER_CONN_PROXY_SERVER,
};

use super::{from_wide_buf, from_wide_ptr, registry, wide};
use crate::sysproxy::windows::{
    enabled, run_once_command, to_restore, ConnectionSettings, RUN_ONCE_KEY, RUN_ONCE_VALUE,
};
use crate::sysproxy::WindowsBackup;

/// Every connection with its own proxy settings: the LAN default first, then
/// each RAS/VPN phonebook entry.
///
/// Third-party VPN clients that present as a network adapter (AnyConnect,
/// GlobalProtect, WireGuard) are not RAS entries and use the LAN settings,
/// so they are covered by the first item.
pub fn connection_names() -> Vec<Option<String>> {
    let mut names = vec![None];
    names.extend(ras_entries().into_iter().map(Some));
    names
}

fn ras_entries() -> Vec<String> {
    let entry_size = size_of::<RASENTRYNAMEW>() as u32;
    let blank = || {
        // SAFETY: a plain-old-data struct; all-zero is a valid value.
        let mut e: RASENTRYNAMEW = unsafe { std::mem::zeroed() };
        e.dwSize = entry_size;
        e
    };
    let mut buf = vec![blank()];
    let mut bytes = entry_size;
    let mut count = 0u32;
    // SAFETY: `buf` holds `bytes` bytes of entries with `dwSize` set, as the
    // API requires; null phonebook means "every phonebook".
    let mut rc = unsafe {
        RasEnumEntriesW(std::ptr::null(), std::ptr::null(), buf.as_mut_ptr(), &mut bytes, &mut count)
    };
    if rc == ERROR_BUFFER_TOO_SMALL {
        let n = (bytes / entry_size).max(1) as usize;
        buf = (0..n).map(|_| blank()).collect();
        bytes = n as u32 * entry_size;
        // SAFETY: as above, with the size the first call asked for.
        rc = unsafe {
            RasEnumEntriesW(std::ptr::null(), std::ptr::null(), buf.as_mut_ptr(), &mut bytes, &mut count)
        };
    }
    if rc != 0 {
        // No RAS service (Server Core, some sandboxes) means no RAS entries,
        // not a failed toggle.
        tracing::debug!(rc, "RasEnumEntriesW failed; treating as no VPN entries");
        return Vec::new();
    }
    buf.iter()
        .take(count as usize)
        .map(|e| from_wide_buf(&e.szEntryName))
        .filter(|n| !n.is_empty())
        .collect()
}

/// Read one connection's settings.
pub fn query(name: Option<&str>) -> Result<ConnectionSettings> {
    // `FLAGS_UI` first: since Windows 7 it is the one that reports the
    // "Automatically detect settings" checkbox truthfully. Older systems reject
    // it, and `FLAGS` is the fallback.
    match query_with(name, INTERNET_PER_CONN_FLAGS_UI) {
        Ok(s) => Ok(s),
        Err(_) => query_with(name, INTERNET_PER_CONN_FLAGS),
    }
}

fn query_with(name: Option<&str>, flags_option: u32) -> Result<ConnectionSettings> {
    let mut conn = name.map(wide);
    let mut opts = [
        option(flags_option),
        option(INTERNET_PER_CONN_PROXY_SERVER),
        option(INTERNET_PER_CONN_PROXY_BYPASS),
        option(INTERNET_PER_CONN_AUTOCONFIG_URL),
    ];
    let mut list = list(&mut conn, &mut opts);
    let mut len = size_of::<INTERNET_PER_CONN_OPTION_LISTW>() as u32;
    // SAFETY: `list` points at `opts` and `conn`, both alive for the call.
    let ok = unsafe {
        InternetQueryOptionW(
            std::ptr::null(),
            INTERNET_OPTION_PER_CONNECTION_OPTION,
            (&mut list as *mut INTERNET_PER_CONN_OPTION_LISTW).cast(),
            &mut len,
        )
    };
    if ok == 0 {
        bail!("cannot read proxy settings: {}", std::io::Error::last_os_error());
    }
    // SAFETY: on success WinINet filled each union with the member matching
    // its `dwOption`; string members are GlobalAlloc'd and ours to free.
    unsafe {
        Ok(ConnectionSettings {
            name: name.map(str::to_string),
            flags: opts[0].Value.dwValue,
            server: take_string(opts[1].Value.pszValue),
            bypass: take_string(opts[2].Value.pszValue),
            autoconfig_url: take_string(opts[3].Value.pszValue),
        })
    }
}

/// Write one connection's settings exactly.
///
/// `None` strings are written as null, which WinINet takes as "no value" —
/// what a restore needs when the value did not exist before.
pub fn apply(settings: &ConnectionSettings) -> Result<()> {
    let mut conn = settings.name.as_deref().map(wide);
    let mut server = settings.server.as_deref().map(wide);
    let mut bypass = settings.bypass.as_deref().map(wide);
    let mut pac = settings.autoconfig_url.as_deref().map(wide);
    let mut opts = [
        option(INTERNET_PER_CONN_FLAGS),
        option(INTERNET_PER_CONN_PROXY_SERVER),
        option(INTERNET_PER_CONN_PROXY_BYPASS),
        option(INTERNET_PER_CONN_AUTOCONFIG_URL),
    ];
    opts[0].Value.dwValue = settings.flags;
    opts[1].Value.pszValue = ptr(&mut server);
    opts[2].Value.pszValue = ptr(&mut bypass);
    opts[3].Value.pszValue = ptr(&mut pac);
    let list = list(&mut conn, &mut opts);
    // SAFETY: every pointer in `list` refers to a local that outlives the call.
    let ok = unsafe {
        InternetSetOptionW(
            std::ptr::null(),
            INTERNET_OPTION_PER_CONNECTION_OPTION,
            (&list as *const INTERNET_PER_CONN_OPTION_LISTW).cast(),
            size_of::<INTERNET_PER_CONN_OPTION_LISTW>() as u32,
        )
    };
    if ok == 0 {
        bail!("cannot write proxy settings: {}", std::io::Error::last_os_error());
    }
    Ok(())
}

/// Tell every WinINet client in the session that the settings changed.
///
/// This is the step whose absence made the first Windows version look broken:
/// without it, browsers that were already running kept their cached settings
/// and went direct until restarted.
pub fn notify() {
    // SAFETY: both options take no buffer.
    unsafe {
        InternetSetOptionW(std::ptr::null(), INTERNET_OPTION_SETTINGS_CHANGED, std::ptr::null(), 0);
        InternetSetOptionW(std::ptr::null(), INTERNET_OPTION_REFRESH, std::ptr::null(), 0);
    }
}

/// Every connection's current settings.
///
/// A connection that cannot be read is left out — and so is left alone by
/// enable and restore — rather than failing the toggle. The LAN connection
/// always reads unless WinINet itself is broken.
pub fn snapshot() -> WindowsBackup {
    let mut connections = Vec::new();
    for name in connection_names() {
        match query(name.as_deref()) {
            Ok(s) => connections.push(s),
            Err(e) => tracing::warn!(lan = name.is_none(), "skipping a connection: {e}"),
        }
    }
    WindowsBackup { connections, ..Default::default() }
}

/// Point every snapshotted connection at `host:port`.
///
/// The `RunOnce` safety net goes in *first*: if anything below succeeds, a
/// restart before the user turns it off must not strand them.
pub fn enable(host: &str, port: u16, backup: &WindowsBackup) -> Result<()> {
    arm_restore_at_sign_in();
    let mut targets = backup.connections.clone();
    if targets.is_empty() {
        // A snapshot taken by an older build: the LAN connection is the one it
        // described.
        targets = vec![query(None)?];
    }
    let mut first_error = None;
    for prior in &targets {
        if let Err(e) = apply(&enabled(host, port, prior)) {
            // The LAN connection is the one that matters; failing it fails the
            // toggle. A VPN entry that will not take the setting is logged.
            if prior.name.is_none() {
                first_error.get_or_insert(e);
            } else {
                tracing::warn!("could not set the proxy on a VPN connection: {e}");
            }
        }
    }
    notify();
    match first_error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Put every connection back as it was. Best effort: one connection failing
/// must not leave the others pointed at a proxy that is about to stop.
pub fn disable(backup: &WindowsBackup) -> Result<()> {
    let mut first_error = None;
    for settings in to_restore(backup) {
        if let Err(e) = apply(&settings) {
            tracing::warn!(lan = settings.name.is_none(), "restore step failed (continuing): {e}");
            first_error.get_or_insert(e);
        }
    }
    notify();
    match first_error {
        // Kept armed on failure: the next sign-in gets another attempt.
        Some(e) => Err(e),
        None => {
            disarm_restore_at_sign_in();
            Ok(())
        }
    }
}

/// Register `NovaProxy --restore-system-proxy` to run once at next sign-in.
fn arm_restore_at_sign_in() {
    let Ok(exe) = std::env::current_exe() else { return };
    if let Err(e) = registry::set_user_string(RUN_ONCE_KEY, RUN_ONCE_VALUE, &run_once_command(&exe)) {
        tracing::warn!("could not register the sign-in proxy restore: {e}");
    }
}

/// Remove the sign-in restore; nothing is owed any more.
pub fn disarm_restore_at_sign_in() {
    if let Err(e) = registry::delete_user_value(RUN_ONCE_KEY, RUN_ONCE_VALUE) {
        tracing::warn!("could not remove the sign-in proxy restore: {e}");
    }
}

fn option(id: u32) -> INTERNET_PER_CONN_OPTIONW {
    // SAFETY: plain-old-data; the union is filled per option by the caller.
    let mut o: INTERNET_PER_CONN_OPTIONW = unsafe { std::mem::zeroed() };
    o.dwOption = id;
    o
}

fn list(
    conn: &mut Option<Vec<u16>>,
    opts: &mut [INTERNET_PER_CONN_OPTIONW],
) -> INTERNET_PER_CONN_OPTION_LISTW {
    INTERNET_PER_CONN_OPTION_LISTW {
        dwSize: size_of::<INTERNET_PER_CONN_OPTION_LISTW>() as u32,
        pszConnection: ptr(conn),
        dwOptionCount: opts.len() as u32,
        dwOptionError: 0,
        pOptions: opts.as_mut_ptr(),
    }
}

fn ptr(s: &mut Option<Vec<u16>>) -> *mut u16 {
    s.as_mut().map_or(std::ptr::null_mut(), |v| v.as_mut_ptr())
}

/// Copy out and free a string WinINet allocated for us.
unsafe fn take_string(p: *mut u16) -> Option<String> {
    let s = from_wide_ptr(p);
    if !p.is_null() {
        GlobalFree(p as *mut c_void);
    }
    s.filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lan_connection_is_always_listed_first() {
        assert_eq!(connection_names().first(), Some(&None));
    }

    /// Puts the original settings back however the test ends, panics included.
    struct Restore(ConnectionSettings);
    impl Drop for Restore {
        fn drop(&mut self) {
            apply(&self.0).expect("restore the original LAN proxy settings");
            notify();
        }
    }

    /// Writes the real proxy settings for well under a second, so it is
    /// opt-in: `cargo test -p nova-os -- --ignored live_`. Uses `apply` rather
    /// than `enable` so it never arms the sign-in restore with a test binary.
    #[test]
    #[ignore]
    fn live_enable_and_restore_round_trip_the_lan_settings() {
        let original = query(None).unwrap();
        let _restore = Restore(original.clone());

        apply(&enabled("127.0.0.1", 47_999, &original)).unwrap();
        notify();
        let on = query(None).unwrap();
        assert_eq!(on.server.as_deref(), Some("127.0.0.1:47999"));
        assert_eq!(on.flags & 0xf, crate::sysproxy::windows::DIRECT | crate::sysproxy::windows::PROXY);
        // The WinINet-facing registry mirror follows, which is what apps that
        // read the registry directly (and the Settings page) see.
        assert_eq!(
            registry::read_dword(
                registry::Hive::CurrentUser,
                r"Software\Microsoft\Windows\CurrentVersion\Internet Settings",
                "ProxyEnable"
            ),
            Some(1)
        );

        apply(&original).unwrap();
        notify();
        let back = query(None).unwrap();
        assert_eq!(back.flags, original.flags);
        assert_eq!(back.server, original.server);
        assert_eq!(back.bypass, original.bypass);
        assert_eq!(back.autoconfig_url, original.autoconfig_url);
    }

    #[test]
    fn the_current_settings_can_be_read() {
        // Read-only: this runs on developer machines, so it must never change
        // the proxy. Proves the FFI layout is right — a wrong `dwSize` or union
        // layout makes WinINet reject the call.
        let lan = query(None).expect("WinINet answers for the LAN connection");
        assert!(lan.name.is_none());
        assert_ne!(lan.flags & crate::sysproxy::windows::DIRECT, 0, "DIRECT is always set");
    }
}
