//! NovaProxy Tauri shell: owns app state, initializes the CA, and exposes the
//! engine to the frontend through commands + a flow-streaming channel.

// Public so the integration tests can drive the MCP endpoint directly.
pub mod commands;
pub mod crash;
pub mod logbundle;
pub mod logging;
pub mod mcp;
pub mod menu;
pub mod state;
pub mod update;

use std::path::PathBuf;
use std::sync::Arc;

use nova_core::ca::CaMaterial;
use state::AppState;

/// Where the CA, settings and captured bodies live.
///
/// On Windows that is **Local** AppData. Roaming AppData — where builds before
/// 0.7 put it — is copied to a profile server at every sign-out on domain
/// machines, and the CA private key and other people's response bodies are the
/// last things that should travel there. See [`migrate_data_dir`].
///
/// Named after the bundle identifier there, not "NovaProxy":
/// `%LOCALAPPDATA%\NovaProxy` is where the per-user NSIS installer puts the
/// program itself. The identifier directory is Tauri's own app-data location
/// (WebView2 keeps its profile in it too), and the uninstaller's "delete
/// application data" option removes exactly it. Data and logs are siblings
/// inside it (see [`logging::log_dir`]), so clearing one never takes the other.
pub fn data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        windows_app_dir().join("data")
    }
    #[cfg(not(windows))]
    {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("NovaProxy")
    }
}

/// `%LOCALAPPDATA%\<identifier from tauri.conf.json>`.
#[cfg(windows)]
pub fn windows_app_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("dev.novaproxy.app")
}

/// Move a pre-0.7 Roaming AppData directory to Local AppData, once.
///
/// Runs before logging starts (the old directory holds the old logs), so it
/// returns what happened for the caller to log. A rename is the normal case;
/// when Roaming is redirected to a network share the two are on different
/// volumes and the rename fails, so the top-level files — CA, rules, script,
/// settings, a pending proxy restore — are copied instead. Captured bodies
/// and old logs are not: bodies are wiped at every launch anyway.
///
/// "Already migrated" is told by the CA certificate rather than by the
/// directory, which other code may have created first.
#[cfg(windows)]
fn migrate_data_dir() -> Option<String> {
    let old = dirs::data_dir()?.join("NovaProxy");
    migrate_dir(&old, &data_dir(), &logging::log_dir())
}

/// [`migrate_data_dir`] on explicit paths, so it can be tested on temp dirs.
#[cfg_attr(not(windows), allow(dead_code))]
fn migrate_dir(old: &std::path::Path, new: &std::path::Path, logs: &std::path::Path) -> Option<String> {
    if !old.is_dir() || new.join("ca.pem").exists() {
        return None;
    }
    // `rename` needs the parent to exist; on an upgrade nothing has made it yet.
    if let Some(parent) = new.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if !new.exists() && std::fs::rename(old, new).is_ok() {
        // The old layout kept logs inside the data dir; they belong beside it.
        if !logs.exists() {
            let _ = std::fs::rename(new.join("logs"), logs);
        }
        return Some("moved the data directory from Roaming to Local AppData".into());
    }
    if let Err(e) = std::fs::create_dir_all(new) {
        return Some(format!("could not create the Local AppData directory: {e}"));
    }
    let mut failed = 0;
    for entry in std::fs::read_dir(old).ok()?.flatten() {
        let path = entry.path();
        if path.is_file() && std::fs::copy(&path, new.join(entry.file_name())).is_err() {
            failed += 1;
        }
    }
    if failed == 0 {
        let _ = std::fs::remove_dir_all(old);
        Some("copied the data directory from Roaming to Local AppData".into())
    } else {
        Some(format!("copied the data directory to Local AppData with {failed} file(s) failing; the Roaming copy is kept"))
    }
}

/// Restore the system proxy if NovaProxy changed it and undoing that needs no
/// prompt — see [`nova_core::sysproxy::restore_is_silent`]. Called at quit and
/// before an update replaces the app; otherwise the machine is left pointed at
/// a port nothing is listening on until NovaProxy is opened again.
pub fn release_system_proxy(state: &AppState, why: &str) {
    if !state.sysproxy_backup_path().exists() || !nova_core::sysproxy::restore_is_silent() {
        return;
    }
    match restore_from_backup(state) {
        Ok(()) => tracing::info!(why, "restored the system proxy"),
        Err(e) => tracing::error!(why, "could not restore the system proxy: {e}"),
    }
}

/// `NovaProxy.exe --restore-system-proxy`, run by the `RunOnce` entry at the
/// first sign-in after the machine restarted with the proxy on. Restores and
/// exits without opening a window.
///
/// Deliberately no `AppState`: building one starts housekeeping (the body
/// store is wiped) that has no business running at sign-in.
#[cfg(windows)]
fn restore_at_sign_in() {
    let path = data_dir().join(state::SYSPROXY_BACKUP_FILE);
    let restored = std::fs::read_to_string(&path)
        .map_err(anyhow::Error::from)
        .and_then(|text| Ok(serde_json::from_str::<nova_core::sysproxy::Backup>(&text)?))
        .and_then(|backup| nova_core::sysproxy::disable(&backup));
    match restored {
        Ok(()) => {
            tracing::warn!("restored the system proxy left on by a session that did not quit");
            let _ = std::fs::remove_file(&path);
        }
        // No backup: nothing is owed, so the entry just has to go.
        Err(_) if !path.exists() => nova_core::windows::inet::disarm_restore_at_sign_in(),
        Err(e) => tracing::error!("could not restore the system proxy at sign-in: {e}"),
    }
}

/// Apply the persisted snapshot and drop it. Shared by crash recovery and the
/// `restore_system_proxy` command so both leave the same state behind: settings
/// back as they were, no backup file, nothing pending.
fn restore_from_backup(state: &AppState) -> anyhow::Result<()> {
    if !state.sysproxy_backup_path().exists() {
        // Someone got there first (a second window, or a toggle). Nothing owed.
        state
            .pending_restore
            .store(false, std::sync::atomic::Ordering::Relaxed);
        return Ok(());
    }
    let text = std::fs::read_to_string(state.sysproxy_backup_path())?;
    let backup: nova_core::sysproxy::Backup = serde_json::from_str(&text)?;
    nova_core::sysproxy::disable(&backup)?;
    let _ = std::fs::remove_file(state.sysproxy_backup_path());
    state
        .pending_restore
        .store(false, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

/// What the frontend is told about the privileged helper.
pub fn helper_status_now() -> nova_proto::HelperStatus {
    use nova_core::helper;
    let version = helper::ping().ok();
    nova_proto::HelperStatus {
        supported: cfg!(target_os = "macos"),
        running: version.is_some(),
        version,
        expected_version: helper::PROTOCOL_VERSION,
        installable: helper::source_binary().is_some(),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Before logging: on Windows the logs live in the data dir being moved.
    #[cfg(windows)]
    let migrated = migrate_data_dir();

    // Held for the whole of `run` — the file writers are non-blocking, so
    // dropping this stops the background threads and loses whatever has not
    // been flushed, the panic hook's last words included.
    let _log_guard = logging::init();
    if _log_guard.is_none() {
        logging::init_stdout_only();
    }
    logging::install_panic_hook();

    #[cfg(windows)]
    {
        if let Some(what) = migrated {
            tracing::info!("{what}");
        }
        if std::env::args().any(|a| a == nova_core::sysproxy::windows::RESTORE_ARG) {
            restore_at_sign_in();
            return;
        }
    }

    // Read the previous run's crash before arming this run's handler, so a
    // crash inside `install` cannot hide the one before it.
    crash::report_previous();
    // Held for the whole of `run` for the same reason as the log guard: dropping
    // it detaches the exception port and the next crash goes unrecorded.
    let _crash_guard = crash::install();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        os = std::env::consts::OS,
        logs = %_log_guard.as_ref().map(|g| logging::redact_path(g.dir())).unwrap_or_else(|| "<stdout only>".into()),
        "NovaProxy starting"
    );
    usage!(
        "app.start",
        ver = env!("CARGO_PKG_VERSION"),
        os = std::env::consts::OS
    );

    // Shared as an `Arc` because the MCP server holds the same state the commands
    // do — one flow store, one rule set, one engine handle.
    let state = Arc::new(AppState::new(data_dir()));

    // Configured only in release builds, where the workflow injects the endpoint
    // and the public key; see `update.rs`. The plugin's initializer rejects a
    // missing `plugins.updater` outright — registering it unconditionally takes
    // down every build without that injection, dev and unsigned release alike —
    // so registration follows the configuration.
    let context = tauri::generate_context!();
    let updater_configured = context.config().plugins.0.contains_key("updater");

    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init());
    if updater_configured {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }

    builder
        .manage(state)
        .manage(update::PendingUpdate::default())
        .manage(update::UpdaterConfigured(updater_configured))
        .setup(|app| {
            use tauri::Manager;
            let st: Arc<AppState> = (*app.state::<Arc<AppState>>()).clone();

            // The whole menu is ours, default included — see `menu`. Best
            // effort: a failure here costs the menu bar, never the launch.
            if let Err(e) = menu::install(app.handle()) {
                tracing::warn!("could not install the menu: {e}");
            }
            app.on_menu_event(|app, event| menu::on_event(app, event.id().as_ref()));

            match CaMaterial::load_or_create(&st.data_dir) {
                Ok(ca) => {
                    tracing::info!(
                        path = %logging::redact_path(&ca.cert_path),
                        "root CA ready"
                    );
                    *st.ca.lock().unwrap() = Some(ca);
                }
                Err(e) => tracing::error!("failed to initialize root CA: {e}"),
            }

            // Load persisted rules.
            if let Ok(text) = std::fs::read_to_string(st.rules_path()) {
                if let Ok(rules) = serde_json::from_str(&text) {
                    *st.rules.write().unwrap() = rules;
                }
            }

            // Load persisted script (kept disabled until the user enables it).
            if let Ok(src) = std::fs::read_to_string(st.script_path()) {
                st.scripts.set_script(src);
            }

            // Load persisted network conditions.
            if let Ok(text) = std::fs::read_to_string(st.net_path()) {
                if let Ok(net) = serde_json::from_str(&text) {
                    *st.net.write().unwrap() = net;
                }
            }

            // Load persisted TLS scope.
            if let Ok(text) = std::fs::read_to_string(st.tls_scope_path()) {
                if let Ok(scope) = serde_json::from_str(&text) {
                    *st.tls_scope.write().unwrap() = scope;
                }
            }

            // Restore the MCP endpoint if it was enabled when the app last ran.
            if let Ok(text) = std::fs::read_to_string(st.mcp_path()) {
                if let Ok(saved) = serde_json::from_str::<nova_proto::McpStatus>(&text) {
                    if saved.running {
                        let app_state = st.clone();
                        let port = saved.port;
                        tauri::async_runtime::spawn(async move {
                            match crate::mcp::start(app_state.clone(), port).await {
                                Ok(handle) => {
                                    app_state
                                        .mcp_port
                                        .store(handle.addr.port(), std::sync::atomic::Ordering::Relaxed);
                                    *app_state.mcp.lock().unwrap() = Some(handle);
                                }
                                Err(e) => tracing::error!("could not restore MCP endpoint: {e}"),
                            }
                        });
                    }
                }
            }

            // System-proxy safety net: a leftover backup means we were mutating
            // the OS proxy when the app last exited (likely a crash), so the
            // machine may still be pointed at a NovaProxy that is not listening.
            //
            // How that gets undone depends on what it costs. On Windows and
            // Linux, and on macOS with the privileged helper, it is free, so
            // just do it. Without the helper macOS needs an administrator
            // password, and raising that dialog unprompted during launch —
            // before any window exists — is exactly the behaviour this flag
            // replaces: the UI offers a "Restore" button instead.
            if st.sysproxy_backup_path().exists() {
                if nova_core::sysproxy::restore_is_silent() {
                    let app_state = st.clone();
                    std::thread::spawn(move || {
                        tracing::warn!("restoring system proxy after unclean exit");
                        if let Err(e) = restore_from_backup(&app_state) {
                            tracing::error!("could not restore system proxy: {e}");
                            app_state
                                .pending_restore
                                .store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                    });
                } else {
                    tracing::warn!(
                        "system proxy settings from the last session are still pending a restore"
                    );
                    st.pending_restore
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::subscribe_flows,
            commands::subscribe_ws,
            commands::proxy_status,
            commands::start_proxy,
            commands::stop_proxy,
            commands::get_rules,
            commands::set_rules,
            commands::get_script,
            commands::set_script,
            commands::get_network_conditions,
            commands::set_network_conditions,
            commands::get_tls_scope,
            commands::set_tls_scope,
            commands::write_file,
            commands::read_file,
            commands::read_body,
            commands::retained_flows,
            commands::clear_flows,
            commands::mcp_status,
            commands::set_mcp_enabled,
            commands::subscribe_breakpoints,
            commands::set_breakpoint,
            commands::resume_breakpoint,
            commands::set_system_proxy,
            commands::restore_system_proxy,
            commands::log_from_ui,
            commands::track_ui,
            commands::app_icon,
            commands::helper_status,
            commands::install_helper,
            commands::uninstall_helper,
            commands::resend_flow,
            commands::ca_status,
            commands::install_ca,
            commands::uninstall_ca,
            commands::regenerate_ca,
            update::check_update,
            update::install_update,
        ])
        .build(context)
        .expect("error while building NovaProxy")
        .run(|app, event| {
            // Every orderly way out — the last window closed, Quit, Ctrl+C in
            // dev — passes through here. Leaving the system proxy pointed at a
            // port that is about to close is what made quitting NovaProxy
            // look like it broke the internet.
            if let tauri::RunEvent::Exit = event {
                use tauri::Manager;
                let st: Arc<AppState> = (*app.state::<Arc<AppState>>()).clone();
                release_system_proxy(&st, "quit");
            }
        });

    // Reached on a clean quit. A launch with no matching `app.stop` in the
    // usage stream is how a crash shows up in the counts — the panic hook
    // covers Rust panics, and this covers the difference.
    tracing::info!("NovaProxy exiting");
    usage!("app.stop");
}

#[cfg(test)]
mod tests {
    use super::migrate_dir;
    use std::fs;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("novaproxy-migrate-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn old_layout(root: &std::path::Path) -> std::path::PathBuf {
        let old = root.join("Roaming").join("NovaProxy");
        fs::create_dir_all(old.join("logs")).unwrap();
        fs::write(old.join("ca.pem"), "cert").unwrap();
        fs::write(old.join("ca.key"), "key").unwrap();
        fs::write(old.join("logs").join("novaproxy.log"), "log").unwrap();
        old
    }

    #[test]
    fn an_old_data_dir_moves_with_its_logs_put_beside_it() {
        let root = tmp("move");
        let old = old_layout(&root);
        let app = root.join("Local").join("dev.novaproxy.app");
        let (new, logs) = (app.join("data"), app.join("logs"));
        assert!(migrate_dir(&old, &new, &logs).unwrap().starts_with("moved"));
        assert!(!old.exists());
        assert_eq!(fs::read_to_string(new.join("ca.key")).unwrap(), "key");
        assert!(logs.join("novaproxy.log").exists(), "logs end up beside the data dir");
        assert!(!new.join("logs").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_existing_target_is_merged_into_not_replaced() {
        // WebView2 or the logger may have created the app dir first.
        let root = tmp("merge");
        let old = old_layout(&root);
        let new = root.join("Local").join("dev.novaproxy.app").join("data");
        fs::create_dir_all(&new).unwrap();
        fs::write(new.join("other"), "kept").unwrap();
        assert!(migrate_dir(&old, &new, &root.join("logs")).unwrap().starts_with("copied"));
        assert_eq!(fs::read_to_string(new.join("ca.pem")).unwrap(), "cert");
        assert_eq!(fs::read_to_string(new.join("other")).unwrap(), "kept");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn nothing_happens_once_migrated_or_with_nothing_to_migrate() {
        let root = tmp("noop");
        let new = root.join("data");
        assert!(migrate_dir(&root.join("missing"), &new, &root.join("logs")).is_none());
        let old = old_layout(&root);
        fs::create_dir_all(&new).unwrap();
        fs::write(new.join("ca.pem"), "newer").unwrap();
        assert!(migrate_dir(&old, &new, &root.join("logs")).is_none());
        assert_eq!(fs::read_to_string(new.join("ca.pem")).unwrap(), "newer", "never overwritten");
        let _ = fs::remove_dir_all(&root);
    }
}