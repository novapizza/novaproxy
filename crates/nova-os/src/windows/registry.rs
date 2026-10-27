//! The handful of plain registry reads and writes NovaProxy needs.
//!
//! Proxy settings do **not** go through here — see [`super::inet`] for why
//! writing `Internet Settings` values directly was the bug, not the fix.

use anyhow::{bail, Result};
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE,
    REG_SZ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};

use super::wide;

#[derive(Debug, Clone, Copy)]
pub enum Hive {
    CurrentUser,
    LocalMachine,
}

impl Hive {
    fn hkey(self) -> HKEY {
        match self {
            Hive::CurrentUser => HKEY_CURRENT_USER,
            Hive::LocalMachine => HKEY_LOCAL_MACHINE,
        }
    }
}

/// A `REG_SZ` value, or `None` if the key or value is absent.
pub fn read_string(hive: Hive, key: &str, name: &str) -> Option<String> {
    let key = wide(key);
    let name = wide(name);
    let mut bytes: u32 = 0;
    // SAFETY: both strings are NUL-terminated; a null buffer asks for the size.
    let rc = unsafe {
        RegGetValueW(
            hive.hkey(),
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut bytes,
        )
    };
    if rc != ERROR_SUCCESS || bytes == 0 {
        return None;
    }
    let mut buf = vec![0u16; (bytes as usize).div_ceil(2)];
    // SAFETY: `buf` holds `bytes` bytes, as the first call asked for.
    let rc = unsafe {
        RegGetValueW(
            hive.hkey(),
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    (rc == ERROR_SUCCESS).then(|| super::from_wide_buf(&buf))
}

/// A `REG_DWORD` value, or `None` if absent.
pub fn read_dword(hive: Hive, key: &str, name: &str) -> Option<u32> {
    let key = wide(key);
    let name = wide(name);
    let mut value: u32 = 0;
    let mut bytes: u32 = 4;
    // SAFETY: `value` is a 4-byte buffer and `bytes` says so.
    let rc = unsafe {
        RegGetValueW(
            hive.hkey(),
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut value as *mut u32).cast(),
            &mut bytes,
        )
    };
    (rc == ERROR_SUCCESS).then_some(value)
}

/// Does `key` exist? Used for exact certificate-store presence checks, where
/// the certificate's thumbprint *is* the key name.
pub fn key_exists(hive: Hive, key: &str) -> bool {
    let key = wide(key);
    let mut h: HKEY = std::ptr::null_mut();
    // SAFETY: `key` is NUL-terminated; `h` is closed below when opened.
    let rc = unsafe { RegOpenKeyExW(hive.hkey(), key.as_ptr(), 0, KEY_READ, &mut h) };
    if rc == ERROR_SUCCESS {
        unsafe { RegCloseKey(h) };
        true
    } else {
        false
    }
}

/// Write a `REG_SZ` under HKCU, creating the key if needed.
pub fn set_user_string(key: &str, name: &str, value: &str) -> Result<()> {
    let h = open_user_key_for_write(key)?;
    let name = wide(name);
    let data = wide(value);
    // SAFETY: `data` is the NUL-terminated value; its byte length includes the NUL.
    let rc = unsafe {
        RegSetValueExW(
            h,
            name.as_ptr(),
            0,
            REG_SZ,
            data.as_ptr().cast(),
            (data.len() * 2) as u32,
        )
    };
    unsafe { RegCloseKey(h) };
    if rc != ERROR_SUCCESS {
        bail!("cannot write registry value (error {rc})");
    }
    Ok(())
}

/// Delete a value under HKCU. An absent value (or key) is success: the goal is
/// "not there", and it isn't.
pub fn delete_user_value(key: &str, name: &str) -> Result<()> {
    let key_w = wide(key);
    let mut h: HKEY = std::ptr::null_mut();
    // SAFETY: as in `key_exists`.
    let rc = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, key_w.as_ptr(), 0, KEY_SET_VALUE, &mut h) };
    if rc == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    if rc != ERROR_SUCCESS {
        bail!("cannot open registry key (error {rc})");
    }
    let name = wide(name);
    let rc = unsafe { RegDeleteValueW(h, name.as_ptr()) };
    unsafe { RegCloseKey(h) };
    if rc != ERROR_SUCCESS && rc != ERROR_FILE_NOT_FOUND {
        bail!("cannot delete registry value (error {rc})");
    }
    Ok(())
}

fn open_user_key_for_write(key: &str) -> Result<HKEY> {
    let key = wide(key);
    let mut h: HKEY = std::ptr::null_mut();
    // SAFETY: `key` is NUL-terminated; the caller closes `h`.
    let rc = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut h,
            std::ptr::null_mut(),
        )
    };
    if rc != ERROR_SUCCESS {
        bail!("cannot open registry key for writing (error {rc})");
    }
    Ok(h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_value_every_windows_machine_has() {
        let build = read_string(
            Hive::LocalMachine,
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "CurrentBuild",
        );
        assert!(build.is_some_and(|b| b.parse::<u32>().is_ok()));
        assert!(read_string(Hive::CurrentUser, r"Software\NovaProxy-no-such-key", "x").is_none());
    }

    #[test]
    fn a_value_round_trips_and_deleting_twice_is_fine() {
        let key = format!(r"Software\NovaProxyTest-{}", std::process::id());
        set_user_string(&key, "v", "hello world").unwrap();
        assert_eq!(read_string(Hive::CurrentUser, &key, "v").as_deref(), Some("hello world"));
        delete_user_value(&key, "v").unwrap();
        delete_user_value(&key, "v").unwrap();
        assert!(read_string(Hive::CurrentUser, &key, "v").is_none());
        let k = wide(&key);
        unsafe { windows_sys::Win32::System::Registry::RegDeleteKeyW(HKEY_CURRENT_USER, k.as_ptr()) };
    }
}
