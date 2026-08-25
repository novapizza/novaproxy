//! Hard crashes: the ones the panic hook never sees.
//!
//! [`crate::logging::install_panic_hook`] catches Rust panics. A SIGSEGV, an
//! abort inside aws-lc, or a webview that dies taking the process with it all
//! bypass it, and until this module existed the only trace of one was a launch
//! with an `app.start` and no matching `app.stop` — which is indistinguishable
//! from a force quit.
//!
//! # What this deliberately does not do
//!
//! It does not write a minidump. A minidump is a copy of process memory, and
//! this process's memory is other people's traffic: URLs, headers, bodies, and
//! the bearer tokens in them. Writing one to disk would put the single worst
//! possible file inside the support bundle. So the record is two integers — the
//! Mach exception kind and code — which say *what* went wrong without saying
//! anything about what was being proxied at the time. The faulting address
//! (`subcode`) is dropped for the same reason: it describes this process's
//! memory layout and needs a symbolised build to mean anything anyway.
//!
//! # Why it is written by hand rather than formatted
//!
//! `crash-handler` runs the callback on a dedicated handler thread **with every
//! other thread suspended**. If one of those threads held the allocator's lock
//! when it was stopped, anything that allocates here deadlocks — and a deadlock
//! in a crash handler turns a crash into a hang. So the marker is built in a
//! stack buffer and written with `open`/`write`/`close`, and nothing in
//! [`write_marker`] allocates.

use std::path::PathBuf;
use std::sync::OnceLock;

/// Where the marker is dropped, as a C string so the handler does not have to
/// build one. Set once by [`install`], read by the handler.
static MARKER: OnceLock<std::ffi::CString> = OnceLock::new();

/// The file a crash leaves behind, read and deleted by the next launch.
///
/// Beside the logs rather than in the data dir: it is a record, and it belongs
/// with the other records — including in the support bundle's directory, though
/// [`crate::logbundle`] filters it out by name because two integers in a file
/// are already in the log by the time anyone collects one.
pub fn marker_path() -> PathBuf {
    crate::logging::log_dir().join("crash.marker")
}

/// Report a crash left behind by the previous run, then forget it.
///
/// Called before [`install`], at launch. A marker with unreadable contents is
/// still worth a line — that it exists at all is the fact that matters.
pub fn report_previous() {
    let path = marker_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    match parse_marker(&text) {
        Some((kind, code)) => {
            tracing::error!(kind, code, "the previous run ended in a hard crash");
            crate::usage!("app.crash", kind = kind, code = code);
        }
        None => {
            tracing::error!("the previous run ended in a hard crash (unreadable marker)");
            crate::usage!("app.crash", kind = 0, code = 0);
        }
    }
    // Removed whether or not it parsed: a marker that survives is a crash
    // reported on every launch from now on.
    let _ = std::fs::remove_file(&path);
}

/// Catch hard crashes for the rest of the process's life.
///
/// The returned handle **must be kept alive**: dropping it detaches the
/// exception port and the next crash goes unrecorded, exactly as before.
///
/// macOS only. `crash-handler` supports Linux and Windows too, but a Mach
/// exception port is what makes this worth having — it sees crashes on threads
/// the app does not own, the webview's included — and neither of the other
/// platforms is bundled today. Wiring them up without being able to run them
/// would be guessing.
#[cfg(target_os = "macos")]
pub fn install() -> Option<crash_handler::CrashHandler> {
    use crash_handler::{CrashEventResult, CrashHandler};

    use std::os::unix::ffi::OsStringExt;
    let path = std::ffi::CString::new(marker_path().into_os_string().into_vec()).ok()?;
    // Set before attaching, or a crash in the window between the two finds no
    // path to write to.
    let _ = MARKER.set(path);

    // SAFETY: the callback allocates nothing, takes no locks, and calls only
    // `open`/`write`/`close` — see the module note on suspended threads.
    let handler = unsafe {
        CrashHandler::attach(crash_handler::make_crash_event(
            |cc: &crash_handler::CrashContext| {
                let (kind, code) = match &cc.exception {
                    Some(e) => (e.kind, e.code),
                    // A crash with no exception info still happened.
                    None => (0, 0),
                };
                write_marker(kind, code);
                // `false` so the kernel forwards to the previous handler, which
                // is the OS default: macOS still writes its own crash report,
                // and the process still dies. Claiming to have handled it would
                // suppress the one artifact a maintainer can symbolise.
                CrashEventResult::Handled(false)
            },
        ))
    };
    match handler {
        Ok(h) => Some(h),
        Err(e) => {
            tracing::warn!("could not install the crash handler: {e}");
            None
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn install() -> Option<()> {
    None
}

/// Write `kind=<n> code=<n>` into the marker without allocating.
#[cfg(target_os = "macos")]
fn write_marker(kind: u32, code: u64) {
    let Some(path) = MARKER.get() else {
        return;
    };
    let mut buf = [0u8; MARKER_MAX];
    let n = format_marker(&mut buf, kind, code);

    // SAFETY: `path` is a live NUL-terminated string for the call, and `buf` is
    // valid for `n` bytes. All three calls are async-signal-safe.
    unsafe {
        let fd = libc::open(
            path.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC,
            0o600 as libc::c_int,
        );
        if fd < 0 {
            return;
        }
        libc::write(fd, buf.as_ptr().cast(), n);
        libc::close(fd);
    }
}

/// Enough for both labels plus two 20-digit integers and a newline.
const MARKER_MAX: usize = 64;

/// Render the marker into `buf`, returning how many bytes were written.
///
/// Split out from [`write_marker`] so the formatting — the part with an
/// off-by-one in it, if there is one — can be tested without crashing a
/// process.
fn format_marker(buf: &mut [u8; MARKER_MAX], kind: u32, code: u64) -> usize {
    let mut n = 0;
    n += put(buf, n, b"kind=");
    n += put_u64(buf, n, kind as u64);
    n += put(buf, n, b" code=");
    n += put_u64(buf, n, code);
    n += put(buf, n, b"\n");
    n
}

/// Copy `src` in at `at`, returning how much was copied. Truncates rather than
/// panicking: a short marker beats a panic inside a crash handler.
fn put(buf: &mut [u8; MARKER_MAX], at: usize, src: &[u8]) -> usize {
    let room = MARKER_MAX.saturating_sub(at);
    let n = src.len().min(room);
    buf[at..at + n].copy_from_slice(&src[..n]);
    n
}

/// Decimal-format `value` at `at`. No allocation, no `write!`.
fn put_u64(buf: &mut [u8; MARKER_MAX], at: usize, value: u64) -> usize {
    // Digits come out backwards, so they are staged and then reversed in.
    let mut digits = [0u8; 20];
    let mut len = 0;
    let mut v = value;
    loop {
        digits[len] = b'0' + (v % 10) as u8;
        len += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    let mut written = 0;
    for i in (0..len).rev() {
        written += put(buf, at + written, &digits[i..i + 1]);
    }
    written
}

/// Read back what [`format_marker`] wrote.
///
/// Forgiving about order and whitespace and strict about nothing else: this
/// parses a file we wrote to ourselves, and the alternative to a lenient parse
/// is losing the crash report over a stray byte.
fn parse_marker(text: &str) -> Option<(u32, u64)> {
    let mut kind = None;
    let mut code = None;
    for field in text.split_whitespace() {
        let (name, value) = field.split_once('=')?;
        match name {
            "kind" => kind = value.parse().ok(),
            "code" => code = value.parse().ok(),
            _ => {}
        }
    }
    Some((kind?, code.unwrap_or(0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(kind: u32, code: u64) -> String {
        let mut buf = [0u8; MARKER_MAX];
        let n = format_marker(&mut buf, kind, code);
        String::from_utf8(buf[..n].to_vec()).unwrap()
    }

    #[test]
    fn a_marker_round_trips() {
        // EXC_BAD_ACCESS with a KERN_INVALID_ADDRESS code — the common case.
        assert_eq!(rendered(1, 1), "kind=1 code=1\n");
        assert_eq!(parse_marker(&rendered(1, 1)), Some((1, 1)));
        assert_eq!(parse_marker(&rendered(10, 0)), Some((10, 0)));
    }

    #[test]
    fn large_values_are_not_truncated_or_reversed() {
        // The digits are staged backwards before being written, which is the
        // one place this could silently produce nonsense.
        assert_eq!(rendered(u32::MAX, 1234567890), "kind=4294967295 code=1234567890\n");
        assert_eq!(
            parse_marker(&rendered(u32::MAX, u64::MAX)),
            Some((u32::MAX, u64::MAX)),
            "both extremes still fit in {MARKER_MAX} bytes"
        );
    }

    #[test]
    fn a_zero_renders_as_one_digit() {
        // The digit loop is do-while for exactly this case; a plain while would
        // write nothing at all.
        assert_eq!(rendered(0, 0), "kind=0 code=0\n");
    }

    #[test]
    fn junk_is_reported_as_a_crash_rather_than_parsed() {
        // A half-written marker — the process died mid-`write` — must not be
        // mistaken for a clean run.
        assert_eq!(parse_marker("kind=1 cod"), None);
        assert_eq!(parse_marker(""), None);
        assert_eq!(parse_marker("garbage"), None);
        // Missing `code` is survivable; missing `kind` is not, because kind is
        // the whole content of the report.
        assert_eq!(parse_marker("kind=4"), Some((4, 0)));
        assert_eq!(parse_marker("code=4"), None);
    }
}
