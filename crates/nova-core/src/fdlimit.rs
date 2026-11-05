//! The per-process limit on open file descriptors.
//!
//! Every connection the proxy carries costs two descriptors — the client's
//! socket and the one to the origin — and an app started from the Dock or
//! Finder inherits launchd's soft limit of 256. With the system proxy on, the
//! browser, Slack and everything else route through us, and 256 runs out: new
//! sockets fail, and the first thing to fail is the DNS lookup, which macOS
//! then reports as "nodename nor servname provided" rather than as the
//! exhaustion it is.
//!
//! The limit is raised to [`WANTED`] at start-up, not to the maximum: idle
//! connections are closed on both sides (see `start`), so a busy machine stays
//! well under it, and a ceiling that is reached still says something is
//! holding sockets it should have let go.

/// The soft limit the app asks for at start-up.
pub const WANTED: u64 = 512;

/// Raise the soft limit to `wanted`, or to the hard limit when that is lower.
/// Never lowers it. Returns the soft limit in force afterwards.
#[cfg(unix)]
pub fn raise(wanted: u64) -> std::io::Result<u64> {
    let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: `lim` is a valid, writable rlimit for the call to fill.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let target = (wanted as libc::rlim_t).min(lim.rlim_max);
    if lim.rlim_cur >= target {
        return Ok(lim.rlim_cur as u64);
    }
    lim.rlim_cur = target;
    // SAFETY: `lim` is a valid rlimit; only the soft limit changed.
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lim) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(target as u64)
}

/// Windows has no such per-process limit on sockets.
#[cfg(not(unix))]
pub fn raise(_wanted: u64) -> std::io::Result<u64> {
    Ok(u64::MAX)
}

/// The soft limit currently in force, if it can be read.
#[cfg(unix)]
pub fn current() -> Option<u64> {
    let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: as in `raise`.
    (unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } == 0).then_some(lim.rlim_cur as u64)
}

#[cfg(not(unix))]
pub fn current() -> Option<u64> {
    None
}

/// How many descriptors the process has open, for the perf log; 0 when it
/// cannot tell. Listing `/dev/fd` costs one descriptor of its own, not counted.
#[cfg(unix)]
pub fn open_count() -> u64 {
    std::fs::read_dir("/dev/fd").map(|d| d.count().saturating_sub(1) as u64).unwrap_or(0)
}

#[cfg(not(unix))]
pub fn open_count() -> u64 {
    0
}

/// Whether the process has no descriptor left to open, checked by trying to
/// open one. An upstream failure can hide this behind a misleading cause, so
/// the error path asks rather than trusting the message.
#[cfg(unix)]
pub fn exhausted() -> bool {
    match std::fs::File::open("/dev/null") {
        Ok(_) => false,
        Err(e) => e.raw_os_error() == Some(libc::EMFILE),
    }
}

#[cfg(not(unix))]
pub fn exhausted() -> bool {
    false
}

/// What the client and the flow are told when [`exhausted`] says so: the
/// cause, then the error the failure surfaced as.
pub fn exhausted_reason(surfaced: &str) -> String {
    let limit = current().map(|n| format!(" (limit {n})")).unwrap_or_default();
    format!(
        "NovaProxy has run out of file descriptors{limit}: too many connections are open, \
         so it could not open another one (reported as: {surfaced})"
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn raising_never_lowers_the_limit() {
        let before = current().unwrap();
        let after = raise(1).unwrap();
        assert_eq!(after, before);
        assert_eq!(current().unwrap(), before);
    }

    #[test]
    fn a_process_with_descriptors_left_is_not_exhausted() {
        assert!(!exhausted());
    }

    #[test]
    fn the_count_sees_the_standard_streams() {
        // Other tests run in parallel and open files too, so only a lower bound.
        assert!(open_count() >= 3, "{}", open_count());
    }

    #[test]
    fn the_reason_names_the_cause_and_keeps_what_surfaced() {
        let r = exhausted_reason("dns error: failed to lookup address information");
        assert!(r.contains("run out of file descriptors"), "{r}");
        assert!(r.ends_with("(reported as: dns error: failed to lookup address information)"), "{r}");
    }
}
