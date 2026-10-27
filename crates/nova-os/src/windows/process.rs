//! PID → executable path → the name a person recognises.
//!
//! The socket table (`GetExtendedTcpTable`, via `netstat2` in `nova-core`)
//! gives a PID; this turns it into "Google Chrome" rather than `chrome.exe`.
//! Windows has no bundles to roll helpers up to, but it does not need them:
//! Chrome, Edge and Electron apps run their network helpers from the same
//! executable as the main process, so the executable *is* the app.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::{from_wide_ptr, wide};

/// Full path of a process's executable.
///
/// `PROCESS_QUERY_LIMITED_INFORMATION` is granted for other users' and most
/// elevated processes too, which is why it is used rather than reading the
/// module list. Protected processes (and PID 0/4) still refuse: `None`.
pub fn image_path(pid: u32) -> Option<String> {
    // SAFETY: the handle is checked and closed below.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return None;
    }
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` holds `len` UTF-16 units.
    let ok = unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) };
    unsafe { CloseHandle(h) };
    (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

/// The app's display name: the executable's `FileDescription` when it has a
/// sensible one, else its file stem.
///
/// Cached by path for the life of the process — version resources do not
/// change under a running executable, and the resolver asks for the same few
/// apps over and over.
pub fn display_name(path: &str) -> String {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().unwrap().get(path) {
        return hit.clone();
    }
    let stem = file_stem(path);
    let name = file_description(path)
        .filter(|d| usable_description(d))
        .unwrap_or(stem);
    cache.lock().unwrap().insert(path.to_string(), name.clone());
    name
}

/// `C:\…\chrome.exe` → `chrome`.
pub fn file_stem(path: &str) -> String {
    let file = path.rsplit(['\\', '/']).next().unwrap_or(path);
    let stem = file
        .len()
        .checked_sub(4)
        .filter(|&i| file[i..].eq_ignore_ascii_case(".exe"))
        .map_or(file, |i| &file[..i]);
    stem.to_string()
}

/// A description worth showing instead of the file name.
///
/// Rejects empty and overlong ones, and the generic descriptions Windows
/// hosts carry: "COM Surrogate" or "Host Process for Windows Services" tells
/// the user less than `svchost` does.
pub fn usable_description(desc: &str) -> bool {
    let d = desc.trim();
    if d.is_empty() || d.len() > 48 {
        return false;
    }
    const GENERIC: [&str; 4] = ["host process", "com surrogate", "runtime broker", "application frame host"];
    let lower = d.to_ascii_lowercase();
    !GENERIC.iter().any(|g| lower.starts_with(g))
}

fn file_description(path: &str) -> Option<String> {
    let wpath = wide(path);
    let mut handle = 0u32;
    // SAFETY: `wpath` is NUL-terminated.
    let size = unsafe { GetFileVersionInfoSizeW(wpath.as_ptr(), &mut handle) };
    if size == 0 {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    // SAFETY: `data` holds `size` bytes.
    if unsafe { GetFileVersionInfoW(wpath.as_ptr(), 0, size, data.as_mut_ptr().cast()) } == 0 {
        return None;
    }
    // The first language/codepage pair the file declares.
    let mut p: *mut std::ffi::c_void = std::ptr::null_mut();
    let mut len = 0u32;
    let query = wide(r"\VarFileInfo\Translation");
    // SAFETY: `data` is a version block from the call above; `p` points into it.
    if unsafe { VerQueryValueW(data.as_ptr().cast(), query.as_ptr(), &mut p, &mut len) } == 0 || len < 4 {
        return None;
    }
    let (lang, cp) = unsafe {
        let pair = p as *const u16;
        (*pair, *pair.add(1))
    };
    let query = wide(&format!(r"\StringFileInfo\{lang:04x}{cp:04x}\FileDescription"));
    // SAFETY: as above; the result is a NUL-terminated string inside `data`.
    if unsafe { VerQueryValueW(data.as_ptr().cast(), query.as_ptr(), &mut p, &mut len) } == 0 || len == 0 {
        return None;
    }
    unsafe { from_wide_ptr(p as *const u16) }.map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_stems_drop_the_directory_and_extension() {
        assert_eq!(file_stem(r"C:\Program Files\Google\Chrome\Application\chrome.exe"), "chrome");
        assert_eq!(file_stem(r"C:\tools\Node.EXE"), "Node");
        assert_eq!(file_stem("curl"), "curl");
    }

    #[test]
    fn generic_host_descriptions_are_not_shown() {
        assert!(!usable_description("Host Process for Windows Services"));
        assert!(!usable_description("COM Surrogate"));
        assert!(!usable_description(""));
        assert!(usable_description("Google Chrome"));
    }

    #[test]
    fn our_own_process_resolves_to_a_path_and_a_name() {
        let path = image_path(std::process::id()).expect("our own image path");
        assert!(path.to_ascii_lowercase().ends_with(".exe"), "{path}");
        assert!(!display_name(&path).is_empty());
    }

    #[test]
    fn a_system_binary_has_a_real_description() {
        let notepad = r"C:\Windows\System32\notepad.exe";
        if std::path::Path::new(notepad).exists() {
            assert_eq!(display_name(notepad), "Notepad");
        }
    }
}
