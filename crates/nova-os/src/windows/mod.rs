//! The Windows platform layer: every Win32 call NovaProxy makes lives here.
//!
//! The rest of the workspace reaches Windows only through this module, the
//! same way it reaches macOS through `networksetup`/`security` plans. Keeping
//! the FFI in one place is what lets someone own "NovaProxy on Windows" without
//! reading the macOS code, and lets a reviewer audit every `unsafe` block that
//! touches the OS in one directory.
//!
//! | Module | Replaces | What it does |
//! |---|---|---|
//! | [`inet`] | `reg.exe` on `Internet Settings` | WinINet per-connection proxy options (LAN + every RAS/VPN entry), with change notification |
//! | [`certstore`] | `certutil -user` | CurrentUser ROOT store via Crypt32, physical-store exact |
//! | [`process`] | — (was unsupported) | PID → executable path and friendly app name |
//! | [`icon`] | — (was unsupported) | executable → 32px PNG |
//! | [`acl`] | — (no-op before) | owner-only DACL for the CA private key |
//! | [`registry`] | `reg.exe` | the few plain registry reads and writes left |
//!
//! [`png`] is pure Rust and compiled everywhere so its tests run on every CI
//! runner; everything else is `cfg(windows)`.

pub mod png;

#[cfg(windows)]
pub mod acl;
#[cfg(windows)]
pub mod certstore;
#[cfg(windows)]
pub mod icon;
#[cfg(windows)]
pub mod inet;
#[cfg(windows)]
pub mod process;
#[cfg(windows)]
pub mod registry;

/// A NUL-terminated UTF-16 copy of `s`, for any `PCWSTR` parameter.
#[cfg(windows)]
pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// [`wide`] for a path, preserving non-UTF-8 (unpaired surrogate) names.
#[cfg(windows)]
pub(crate) fn wide_path(p: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

/// Read a NUL-terminated UTF-16 string the OS handed us.
///
/// # Safety
/// `p` must be null or point at a NUL-terminated UTF-16 string.
#[cfg(windows)]
pub(crate) unsafe fn from_wide_ptr(p: *const u16) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let mut len = 0;
    while *p.add(len) != 0 {
        len += 1;
    }
    Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)))
}

/// A UTF-16 buffer up to its first NUL.
#[cfg(windows)]
pub(crate) fn from_wide_buf(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// `Windows 11 23H2 (build 22631.4317)`, for the support bundle.
///
/// Read from the registry rather than `GetVersionEx`, which lies to any
/// process without a compatibility manifest. Build 22000 is where Windows 11
/// starts; `ProductName` still says "Windows 10" on 11, so it is not used.
#[cfg(windows)]
pub fn os_version() -> String {
    use registry::{read_dword, read_string, Hive};
    const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let build = read_string(Hive::LocalMachine, KEY, "CurrentBuild").unwrap_or_default();
    let ubr = read_dword(Hive::LocalMachine, KEY, "UBR");
    let display = read_string(Hive::LocalMachine, KEY, "DisplayVersion").unwrap_or_default();
    let major = match build.parse::<u32>() {
        Ok(b) if b >= 22000 => "Windows 11",
        Ok(_) => "Windows 10",
        Err(_) => "Windows",
    };
    let build = match ubr {
        Some(ubr) => format!("{build}.{ubr}"),
        None => build,
    };
    format!("{major} {display} (build {build})").replace("  ", " ")
}
