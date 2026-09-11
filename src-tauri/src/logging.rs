//! Two log files, written for two different readers.
//!
//! `novaproxy.<date>.log` is diagnostics: prose a person reads when something
//! went wrong on a machine they cannot touch. `usage.<date>.jsonl` is one JSON
//! object per line, meant to be counted rather than read.
//!
//! They are separate files rather than one stream with a filter because their
//! retention differs (a week of diagnostics is plenty; a month of counts is
//! not), and because mixing them means every analysis starts by parsing prose.
//!
//! # What must never reach either file
//!
//! NovaProxy proxies other people's traffic. **No captured URL, host, header,
//! body, or the name of an app being captured may be logged**, at any level,
//! in either stream. Log counts, durations, outcomes and error kinds; never
//! content. `redact_path` exists because even our own file paths carry the
//! user's home directory, and a support log that leaks a username is a leak.
//!
//! This is not a style preference — it is the reason the usage stream records
//! `{"ev":"flows.captured","n":412}` rather than anything about those 412
//! requests.
//!
//! # Lifetime
//!
//! [`init`] returns a [`Guard`] that **must stay alive for the whole process**.
//! The file writer is non-blocking, which means a background thread owns the
//! buffer; dropping the guard flushes and stops it, and anything logged after
//! that is silently lost — including the panic hook's final message.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// Target that routes an event to the usage stream instead of diagnostics.
///
/// A literal rather than a `tracing` field because the filter has to work on
/// metadata alone, before the event is formatted.
pub const USAGE_TARGET: &str = "nova_usage";

/// Days of diagnostics kept. A week covers "it broke on Friday, I filed it on
/// Monday" without turning the log directory into an archive.
const DIAG_KEEP_DAYS: usize = 7;

/// Days of usage events kept. Longer than diagnostics because the point of
/// counting is comparing this month against last.
const USAGE_KEEP_DAYS: usize = 30;

/// Ceiling on the whole log directory.
///
/// `max_log_files` bounds the *count* of files, not their size — one heavy
/// debugging day can produce a single enormous file and the count-based cap
/// will not notice. This is swept once at launch.
const MAX_LOG_BYTES: u64 = 200 * 1024 * 1024;

/// Everything the app logs, at the level it logs it.
///
/// `nova_os` is named explicitly: it does not share a prefix with `novaproxy`
/// or `nova_core`, so the old default silently dropped every `sysproxy`,
/// `oscmd` and `helper` event — exactly the privileged operations a support
/// log exists to explain.
///
/// It sits at `info`, which carries the per-operation totals — `sysproxy.toggle`
/// with its stage breakdown, and one "ran plan" line per sweep. The line *per
/// child process* is `debug`, because a system-proxy toggle spawns `6N + 2` of
/// them for `N` network services and thirty lines a click would bury everything
/// else. Run with `RUST_LOG=nova_os=debug` to see which `networksetup`
/// subcommand is the slow one.
const DEFAULT_FILTER: &str =
    "novaproxy=info,novaproxy_lib=info,novaproxy_ui=info,nova_core=info,nova_os=info";

/// The two identifiers every usage event carries.
///
/// Both are random. Neither is derived from hardware, MAC address, username or
/// anything else about the person — a device id that can be recomputed from the
/// machine is a fingerprint, and this file is meant to be shareable.
pub struct Ids {
    /// New on every launch. Distinguishes one run from the next, which is what
    /// makes a sequence of events readable as a session.
    pub session: String,
    /// Written once and kept. Without it, fifty launches by one person and one
    /// launch by fifty people are the same data.
    pub device: String,
}

static IDS: OnceLock<Ids> = OnceLock::new();

/// Session and device ids, initialized on first use.
pub fn ids() -> &'static Ids {
    IDS.get_or_init(|| Ids {
        session: short_id(),
        device: load_or_create_device_id(),
    })
}

/// A UUID with the dashes removed, truncated — long enough not to collide
/// across one person's log files, short enough to read in a line of JSON.
fn short_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
}

/// Path of the persisted device id. Lives in the data dir rather than beside
/// the logs, because it is state rather than a record: rotating logs away must
/// not silently turn one returning user into a new one.
pub fn device_id_path() -> PathBuf {
    crate::data_dir().join("device_id")
}

/// Read the device id, creating it the first time. A failure to persist is not
/// worth failing a launch over — the run just counts as a new device.
fn load_or_create_device_id() -> String {
    let path = device_id_path();
    if let Ok(text) = std::fs::read_to_string(&path) {
        let trimmed = text.trim();
        if is_short_id(trimmed) {
            return trimmed.to_string();
        }
    }
    let fresh = short_id();
    let _ = std::fs::create_dir_all(crate::data_dir());
    let _ = std::fs::write(&path, &fresh);
    fresh
}

/// Whether a stored value is one of ours, so a truncated or hand-edited file is
/// replaced rather than written into every event.
fn is_short_id(text: &str) -> bool {
    text.len() == 12 && text.chars().all(|c| c.is_ascii_hexdigit())
}

/// Keeps the background writer threads alive. See the module note on lifetime.
pub struct Guard {
    _diagnostics: WorkerGuard,
    _usage: WorkerGuard,
    dir: PathBuf,
}

impl Guard {
    /// Where the two files live, for "Reveal logs" and for support instructions.
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// The directory both streams are written to.
///
/// `~/Library/Logs/NovaProxy` on macOS — the platform's own convention, and
/// where Console.app looks — rather than beside the data dir, so that clearing
/// application data does not destroy the record of why it needed clearing.
/// Windows and Linux have no equivalent, so logs sit under the data dir there.
pub fn log_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = dirs::home_dir() {
            return home.join("Library/Logs/NovaProxy");
        }
    }
    crate::data_dir().join("logs")
}

/// Start both streams. Returns `None` if the log directory cannot be created,
/// in which case the caller falls back to stdout — a missing log is worth a
/// degraded app, never a failed launch.
pub fn init() -> Option<Guard> {
    let dir = log_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("could not create log directory {}: {e}", dir.display());
        return None;
    }

    let diagnostics = rolling::Builder::new()
        .rotation(rolling::Rotation::DAILY)
        .filename_prefix("novaproxy")
        .filename_suffix("log")
        .max_log_files(DIAG_KEEP_DAYS)
        .build(&dir)
        .ok()?;
    let usage = rolling::Builder::new()
        .rotation(rolling::Rotation::DAILY)
        .filename_prefix("usage")
        .filename_suffix("jsonl")
        .max_log_files(USAGE_KEEP_DAYS)
        .build(&dir)
        .ok()?;

    // Swept before the writers open, so today's file is never a candidate.
    let pruned = prune_by_size(&dir, MAX_LOG_BYTES);

    let (diag_writer, diag_guard) = tracing_appender::non_blocking(diagnostics);
    let (usage_writer, usage_guard) = tracing_appender::non_blocking(usage);

    // Diagnostics: everything *except* usage events, which would otherwise
    // appear twice — once as prose here and once as JSON there.
    let diag_layer = fmt::layer()
        .with_writer(diag_writer)
        .with_ansi(false)
        .with_target(true)
        .with_filter(filter())
        .with_filter(tracing_subscriber::filter::filter_fn(|meta| {
            meta.target() != USAGE_TARGET
        }));

    // Usage: only the usage target, as JSON lines. Flattened so each line is a
    // flat object rather than `{"fields":{…}}` wrapped in span machinery.
    let usage_layer = fmt::layer()
        .json()
        .flatten_event(true)
        .with_writer(usage_writer)
        .with_target(false)
        .with_span_list(false)
        .with_current_span(false)
        .with_filter(tracing_subscriber::filter::filter_fn(|meta| {
            meta.target() == USAGE_TARGET
        }));

    // stdout stays for `cargo tauri dev`; in a bundled app nothing reads it,
    // which is the whole reason the file layers exist.
    let stdout_layer = fmt::layer()
        .with_filter(filter())
        .with_filter(tracing_subscriber::filter::filter_fn(|meta| {
            meta.target() != USAGE_TARGET
        }));

    tracing_subscriber::registry()
        .with(diag_layer)
        .with(usage_layer)
        .with(stdout_layer)
        .init();

    if !pruned.is_empty() {
        tracing::warn!(
            count = pruned.len(),
            "log directory was over {} MB; removed the oldest files",
            MAX_LOG_BYTES / 1024 / 1024
        );
    }

    Some(Guard {
        _diagnostics: diag_guard,
        _usage: usage_guard,
        dir,
    })
}

/// Fall back to the old stdout-only subscriber when the files cannot be opened.
pub fn init_stdout_only() {
    tracing_subscriber::fmt().with_env_filter(filter()).init();
}

fn filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| DEFAULT_FILTER.into())
}

/// Record a usage event.
///
/// Deliberately awkward to pass free text to: every call site should be a fixed
/// event name plus counted fields. If you find yourself wanting to interpolate
/// something the user typed or something the proxy captured, that is the module
/// note in the header saying no.
#[macro_export]
macro_rules! usage {
    ($event:expr) => {
        ::tracing::info!(
            target: $crate::logging::USAGE_TARGET,
            sid = $crate::logging::ids().session.as_str(),
            did = $crate::logging::ids().device.as_str(),
            ev = $event
        )
    };
    ($event:expr, $($field:tt)*) => {
        ::tracing::info!(
            target: $crate::logging::USAGE_TARGET,
            sid = $crate::logging::ids().session.as_str(),
            did = $crate::logging::ids().device.as_str(),
            ev = $event,
            $($field)*
        )
    };
}

/// Delete the oldest files until the directory fits under `max_bytes`.
///
/// Complements `max_log_files`, which counts files and cannot see that one of
/// them is a gigabyte. Deliberately oldest-first and never touching the file
/// currently being written (the newest by name, which sorts last because both
/// prefixes are followed by an ISO date).
///
/// Returns what it removed, so the caller can say so in the log.
pub fn prune_by_size(dir: &Path, max_bytes: u64) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(PathBuf, u64)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            let meta = e.metadata().ok()?;
            meta.is_file().then(|| (path, meta.len()))
        })
        .collect();
    // By name, which is by date: both streams are `<prefix>.<YYYY-MM-DD>.<ext>`.
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut total: u64 = files.iter().map(|(_, len)| len).sum();
    let mut removed = Vec::new();
    for (path, len) in &files {
        if total <= max_bytes {
            break;
        }
        // Never the last one: it is today's, and something is holding it open.
        if files.len() - removed.len() <= 1 {
            break;
        }
        if std::fs::remove_file(path).is_ok() {
            total -= len;
            removed.push(path.clone());
        }
    }
    removed
}

/// Replace the user's home directory with `~` wherever it appears.
///
/// Applied to error strings, not just paths, because that is where the leak
/// actually happens: `security` and `networksetup` quote the full path back at
/// us on failure, and those messages are exactly what gets pasted into an
/// issue. `/Users/jane.doe/Library/…` names a real person.
pub fn redact(text: &str) -> String {
    let Some(home) = dirs::home_dir() else {
        return text.to_string();
    };
    let home = home.to_string_lossy();
    // An empty or `/` home would rewrite every absolute path in the message.
    if home.is_empty() || home == "/" {
        return text.to_string();
    }
    text.replace(home.as_ref(), "~")
}

/// [`redact`] for a path.
pub fn redact_path(path: &Path) -> String {
    redact(&path.to_string_lossy())
}

/// Install a panic hook that writes the panic into the diagnostics log before
/// the process dies.
///
/// This catches Rust panics only. A hard crash — SIGSEGV, an abort inside
/// aws-lc, a webview kill — bypasses it entirely and leaves nothing behind;
/// catching those needs a native crash handler, which this does not attempt.
/// The previous behaviour was worse in every case: panics went to a stdout
/// nothing was attached to.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown".into());
        // `payload_as_str` is not stable across our MSRV, so both common
        // payload shapes are tried by hand.
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".into());
        tracing::error!(target: "novaproxy_lib", location = %location, "panic: {message}");
        usage!("app.panic", location = %location);
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_the_home_directory() {
        let Some(home) = dirs::home_dir() else { return };
        let inside = home.join("Library/Application Support/NovaProxy/ca.pem");
        let out = redact_path(&inside);
        assert!(out.starts_with('~'), "{out}");
        assert!(
            !out.contains(&*home.to_string_lossy()),
            "the home path survived redaction: {out}"
        );
        assert!(out.ends_with("ca.pem"));
    }

    #[test]
    fn redacts_the_home_anywhere_in_a_message() {
        // The real leak vector: `security`/`networksetup` quote the path back
        // at us on failure, and that message is what gets pasted into an issue.
        let Some(home) = dirs::home_dir() else { return };
        let home = home.to_string_lossy().into_owned();
        let message = format!("SecKeychainOpen: {home}/Library/Keychains/login.keychain-db not found");
        let out = redact(&message);
        assert!(!out.contains(&home), "the home path survived: {out}");
        assert!(out.contains("~/Library/Keychains"), "{out}");
    }

    #[test]
    fn leaves_paths_outside_the_home_alone() {
        let out = redact_path(Path::new("/Library/LaunchDaemons/dev.novaproxy.helper.plist"));
        assert_eq!(out, "/Library/LaunchDaemons/dev.novaproxy.helper.plist");
    }

    #[test]
    fn the_default_filter_names_every_crate_that_logs() {
        // `nova_os` shares no prefix with the other two, so omitting it — as the
        // original default did — silently drops every privileged operation.
        for crate_name in ["novaproxy_lib", "novaproxy_ui", "nova_core", "nova_os"] {
            assert!(
                DEFAULT_FILTER.contains(crate_name),
                "{crate_name} is not in the default filter, so its events are dropped"
            );
        }
    }

    #[test]
    fn short_ids_are_twelve_hex_characters() {
        let id = short_id();
        assert!(is_short_id(&id), "{id}");
        assert_ne!(short_id(), short_id(), "ids must not repeat");
    }

    #[test]
    fn a_hand_edited_device_id_is_rejected() {
        // A truncated or edited file would otherwise be written into every
        // event for the life of the install.
        assert!(!is_short_id(""));
        assert!(!is_short_id("abc"));
        assert!(!is_short_id("zzzzzzzzzzzz"));
        assert!(!is_short_id("0123456789abcdef"));
        assert!(is_short_id("0123456789ab"));
    }

    #[test]
    fn prune_removes_oldest_first_and_spares_the_newest() {
        let dir = std::env::temp_dir().join(format!("nova-prune-{}", short_id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Names sort by date, which is the order they were written in.
        for day in ["2026-08-01", "2026-08-02", "2026-08-03"] {
            std::fs::write(dir.join(format!("novaproxy.{day}.log")), vec![b'x'; 1000]).unwrap();
        }

        // 3000 bytes against a 2500 budget: one file is enough to get under it,
        // and it must be the oldest.
        let removed = prune_by_size(&dir, 2500);
        let names: Vec<String> = removed
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["novaproxy.2026-08-01.log"], "oldest goes first");
        assert!(dir.join("novaproxy.2026-08-03.log").exists(), "newest is kept");

        // Tighter budget: it keeps going until it fits, still oldest-first.
        std::fs::write(dir.join("novaproxy.2026-08-01.log"), vec![b'x'; 1000]).unwrap();
        let removed = prune_by_size(&dir, 1500);
        assert_eq!(removed.len(), 2, "removes as many as it takes to fit");
        assert!(dir.join("novaproxy.2026-08-03.log").exists(), "newest still kept");

        // Even a budget of zero leaves the file currently being written to.
        prune_by_size(&dir, 0);
        let left = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(left, 1, "the newest file is never deleted");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn prune_is_a_no_op_under_budget() {
        let dir = std::env::temp_dir().join(format!("nova-prune-{}", short_id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("novaproxy.2026-08-01.log"), b"small").unwrap();
        assert!(prune_by_size(&dir, MAX_LOG_BYTES).is_empty());
        assert!(dir.join("novaproxy.2026-08-01.log").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn log_dir_is_not_the_data_dir() {
        // Clearing application data must not destroy the record of why it
        // needed clearing — and `BodyStore` wipes its directory at every launch.
        assert_ne!(log_dir(), crate::data_dir());
        assert!(!log_dir().starts_with(crate::data_dir()));
    }
}
