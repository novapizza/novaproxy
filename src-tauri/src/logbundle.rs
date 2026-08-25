//! One zip a user can attach to an issue.
//!
//! `logging` writes the record; this module collects it. They are separate
//! because the constraints differ: writing has to be cheap and non-blocking on
//! a hot path, while collecting happens once, on a click, and its job is to
//! decide *what leaves the machine*.
//!
//! # What goes in, and what must never
//!
//! The two log streams and the daemon's log, and nothing else. Both streams are
//! already redacted at write time (see [`crate::logging`]), so the bundle adds
//! no new leak — but the app's own state files would. `rules.json` holds URL
//! patterns, which are the hosts the user is debugging; `script.js` is code they
//! may have pasted a bearer token into; the flow store is captured traffic
//! outright. **None of those may be added here, whatever a future bug report
//! would find convenient.**
//!
//! [`debug_information`] is the one thing in the bundle that is not a log, and
//! it is built from counts and booleans for the same reason: `TlsScope` carries
//! a host list, so the bundle records how many entries it has and never which.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::json;
use zip::write::SimpleFileOptions;

use crate::logging::{self, redact_path};
use crate::state::AppState;

/// Name of the metadata file inside the zip.
const INFO_NAME: &str = "debug-information.json";

/// Where the daemon's log lives, mirrored from `nova_os` so a non-macOS build
/// does not have to reach into a macOS-only module.
#[cfg(target_os = "macos")]
const HELPER_LOGS: [&str; 2] = [
    nova_core::helper::HELPER_LOG,
    // The rotated generation. `prepare_log` renames rather than truncates, so
    // the run that rotated is still writing into this one — skipping it would
    // drop the most recent daemon output on exactly the boot that produced a
    // lot of it.
    "/Library/Logs/NovaProxy/helper.log.1",
];

/// What the bundle turned out to contain.
pub struct Bundle {
    pub path: PathBuf,
    /// Files inside the zip, `debug-information.json` included.
    pub files: usize,
    /// Size of the zip itself, which is what the user has to attach.
    pub bytes: u64,
}

/// Build the zip and return where it landed.
///
/// Blocking: it reads the whole log directory and deflates it. Call it off the
/// UI thread.
pub fn write(state: &AppState, dest_dir: &Path, stamp: &str) -> anyhow::Result<Bundle> {
    std::fs::create_dir_all(dest_dir)?;
    let path = dest_dir.join(format!("novaproxy-{stamp}-logs.zip"));

    let file = std::fs::File::create(&path)?;
    let mut zip = zip::ZipWriter::new(file);
    // Deflate: these are text files, and the difference between 200 MB of log
    // and something a user can actually attach to an issue is this line.
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let mut files = 0;

    zip.start_file(INFO_NAME, options)?;
    zip.write_all(debug_information(state, stamp).as_bytes())?;
    files += 1;

    for source in collectable(&logging::log_dir()) {
        let Some(name) = source.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Best-effort per file: today's log is open and being appended to by the
        // non-blocking writer, and a mid-read rotation must not lose the bundle.
        let Ok(body) = std::fs::read(&source) else {
            tracing::warn!(file = name, "could not read a log file for the bundle");
            continue;
        };
        zip.start_file(format!("logs/{name}"), options)?;
        zip.write_all(&body)?;
        files += 1;
    }

    files += add_helper_logs(&mut zip, options)?;

    zip.finish()?;
    let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    Ok(Bundle { path, files, bytes })
}

/// The log files worth collecting, oldest first.
///
/// Named by prefix rather than "everything in the directory": the log directory
/// is ours, but `device_id`-style state or a stray `crash.marker` living beside
/// the logs is not something a support bundle needs to carry — by the time a
/// bundle is collected, the marker's two integers are already a line in the
/// diagnostics log.
fn collectable(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                return false;
            };
            p.is_file() && (name.starts_with("novaproxy.") || name.starts_with("usage."))
        })
        .collect();
    // Both prefixes are followed by an ISO date, so by name is by day.
    files.sort();
    files
}

/// Add the privileged daemon's log, if this platform has one and it is readable.
///
/// Silently skipped when absent: the helper is optional, and a machine that
/// never installed it has nothing to contribute here. Unreadable is worth a
/// line in the diagnostics, though — that is the `prepare_log` chmod having
/// failed, and it is the difference between a complete bundle and one missing
/// the record of every refused connection.
#[cfg(target_os = "macos")]
fn add_helper_logs<W: Write + std::io::Seek>(
    zip: &mut zip::ZipWriter<W>,
    options: SimpleFileOptions,
) -> anyhow::Result<usize> {
    let mut added = 0;
    for source in HELPER_LOGS {
        let source = Path::new(source);
        if !source.exists() {
            continue;
        }
        let Some(name) = source.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        match std::fs::read(source) {
            Ok(body) => {
                zip.start_file(format!("helper/{name}"), options)?;
                zip.write_all(&body)?;
                added += 1;
            }
            Err(e) => tracing::warn!(
                file = name,
                "the daemon log exists but is not readable: {e}; the bundle is missing it"
            ),
        }
    }
    Ok(added)
}

#[cfg(not(target_os = "macos"))]
fn add_helper_logs<W: Write + std::io::Seek>(
    _zip: &mut zip::ZipWriter<W>,
    _options: SimpleFileOptions,
) -> anyhow::Result<usize> {
    Ok(0)
}

/// The metadata a maintainer would otherwise have to ask three questions to get.
///
/// Every value here is a count, a boolean, a version or an id we generated
/// ourselves. Nothing is a host, a URL, a path outside our own directories, or
/// the name of a captured app — see the module note.
pub fn debug_information(state: &AppState, stamp: &str) -> String {
    let ids = logging::ids();
    let helper = crate::helper_status_now();
    let engine = state.engine.lock().unwrap();
    let rules = state.rules.read().unwrap();
    let scope = state.tls_scope.read().unwrap();
    let net = state.net.read().unwrap().clone();

    let info = json!({
        "collected_at": stamp,
        "app": {
            "version": env!("CARGO_PKG_VERSION"),
            "os": std::env::consts::OS,
            "os_version": os_version(),
            "arch": std::env::consts::ARCH,
        },
        "ids": {
            // The same pair the usage stream carries, so a bundle can be lined
            // up against the counts. Both are random and neither is derived
            // from the machine — see `logging::Ids`.
            "session": ids.session,
            "device": ids.device,
        },
        "log_dir": redact_path(&logging::log_dir()),
        "helper": {
            "supported": helper.supported,
            "running": helper.running,
            "version": helper.version,
            "expected_version": helper.expected_version,
            "installable": helper.installable,
        },
        "proxy": {
            "running": engine.is_some(),
            "port": engine.as_ref().map(|h| h.addr.port()),
            "system_proxy": *state.system_proxy.lock().unwrap(),
            "pending_restore": state.pending_restore.load(std::sync::atomic::Ordering::Relaxed),
            "flows_captured": state.flows.total_captured(),
        },
        "rules": {
            "total": rules.len(),
            "enabled": rules.iter().filter(|r| r.enabled).count(),
        },
        "script": {
            "enabled": state.scripts.is_enabled(),
            // Length, never the source: a script is where a user pastes the
            // token they are debugging with.
            "bytes": std::fs::metadata(state.script_path()).map(|m| m.len()).unwrap_or(0),
        },
        "tls_scope": {
            "intercept_all": scope.intercept_all,
            // Counts only. These lists are hostnames.
            "include": scope.include.len(),
            "exclude": scope.exclude.len(),
        },
        "network_conditions": {
            "enabled": net.enabled,
            "latency_ms": net.latency_ms,
            "down_kbps": net.down_kbps,
        },
        "mcp": {
            "running": state.mcp.lock().unwrap().is_some(),
            "port": state.mcp_port.load(std::sync::atomic::Ordering::Relaxed),
        },
    });
    // Pretty, because a human opens this file first and reads it by eye.
    serde_json::to_string_pretty(&info).unwrap_or_else(|_| "{}".into())
}

/// The OS build, best-effort. Empty rather than an error: a bundle without it
/// is still worth having.
fn os_version() -> String {
    #[cfg(target_os = "macos")]
    {
        nova_core::oscmd::capture("/usr/bin/sw_vers", &["-productVersion"])
            .trim()
            .to_string()
    }
    #[cfg(not(target_os = "macos"))]
    {
        String::new()
    }
}

/// Reveal the finished zip in the platform's file manager.
///
/// Selecting the file rather than opening it: the user's next move is to drag it
/// into an issue or an email, and an unzip window is not that.
pub fn reveal(path: &Path) -> anyhow::Result<()> {
    use nova_core::oscmd::{Elevation, Plan, Step};

    let file = path.display().to_string();
    let step = if cfg!(target_os = "macos") {
        Step::with_args("/usr/bin/open", vec!["-R".into(), file])
    } else if cfg!(target_os = "windows") {
        // `/select,<path>` is one argument to explorer, comma included.
        Step::with_args("explorer", vec![format!("/select,{file}")])
    } else {
        // No Linux file manager reliably selects a file, so open the folder.
        let dir = path
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or(file);
        Step::with_args("xdg-open", vec![dir])
    };
    Plan::new(vec![step], Elevation::None).run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_our_own_log_files_are_collected() {
        let dir = std::env::temp_dir().join(format!("novaproxy-bundle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in [
            "novaproxy.2026-08-24.log",
            "novaproxy.2026-08-25.log",
            "usage.2026-08-25.jsonl",
            // State living beside the logs, and somebody else's file. Neither
            // belongs in a bundle a user hands to a stranger.
            "device_id",
            "Screenshot.png",
        ] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }

        let names: Vec<String> = collectable(&dir)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "novaproxy.2026-08-24.log",
                "novaproxy.2026-08-25.log",
                "usage.2026-08-25.jsonl"
            ],
            "by name is by date, and nothing else comes along"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_log_directory_is_not_an_error() {
        // First launch, or a machine where `init` fell back to stdout.
        assert!(collectable(Path::new("/definitely/not/here")).is_empty());
    }

    #[test]
    fn debug_information_carries_counts_and_never_the_lists_themselves() {
        let state = AppState::new(std::env::temp_dir().join("novaproxy-info-test"));
        {
            let mut scope = state.tls_scope.write().unwrap();
            scope.include = vec!["api.internal.example".into(), "auth.example".into()];
            scope.exclude = vec!["telemetry.example".into()];
        }
        let text = debug_information(&state, "20260825-120000");

        // The whole point: a host list becomes a number.
        assert!(!text.contains("api.internal.example"), "{text}");
        assert!(!text.contains("telemetry.example"), "{text}");
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["tls_scope"]["include"], 2);
        assert_eq!(parsed["tls_scope"]["exclude"], 1);
        assert_eq!(parsed["app"]["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(parsed["collected_at"], "20260825-120000");
    }

    #[test]
    fn the_bundle_contains_the_logs_and_the_metadata() {
        let root = std::env::temp_dir().join(format!("novaproxy-zip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dest = root.join("Downloads");
        std::fs::create_dir_all(&dest).unwrap();

        let state = AppState::new(root.join("data"));
        let bundle = write(&state, &dest, "20260825-120000").unwrap();

        assert_eq!(
            bundle.path.file_name().unwrap(),
            "novaproxy-20260825-120000-logs.zip"
        );
        assert!(bundle.files >= 1, "the metadata file is always there");
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&bundle.path).unwrap()).unwrap();
        let names: Vec<String> = (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(names.contains(&INFO_NAME.to_string()), "{names:?}");
        // Nothing from the app's own state directory, whatever is in it.
        assert!(
            !names.iter().any(|n| n.contains("rules") || n.contains("script")),
            "state files must never be bundled: {names:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
