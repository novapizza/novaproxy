//! NovaProxy Tauri shell: owns app state, initializes the CA, and exposes the
//! engine to the frontend through commands + a flow-streaming channel.

// Public so the integration tests can drive the MCP endpoint directly.
pub mod commands;
pub mod logging;
pub mod mcp;
pub mod state;
pub mod update;

use std::path::PathBuf;
use std::sync::Arc;

use nova_core::ca::CaMaterial;
use state::AppState;

pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("NovaProxy")
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
    // Held for the whole of `run` — the file writers are non-blocking, so
    // dropping this stops the background threads and loses whatever has not
    // been flushed, the panic hook's last words included.
    let _log_guard = logging::init();
    if _log_guard.is_none() {
        logging::init_stdout_only();
    }
    logging::install_panic_hook();

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

    let mut builder = tauri::Builder::default().plugin(tauri_plugin_dialog::init());
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
            // How that gets undone depends on what it costs. With the privileged
            // helper installed it is free, so just do it. Without one it needs an
            // administrator password, and raising that dialog unprompted during
            // launch — before any window exists — is exactly the behaviour this
            // flag replaces: the UI offers a "Restore" button instead.
            if st.sysproxy_backup_path().exists() {
                if nova_core::helper::usable() {
                    let app_state = st.clone();
                    std::thread::spawn(move || {
                        tracing::warn!("restoring system proxy after unclean exit (via helper)");
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
        .run(context)
        .expect("error while running NovaProxy");

    // Reached on a clean quit. A launch with no matching `app.stop` in the
    // usage stream is how a crash shows up in the counts — the panic hook
    // covers Rust panics, and this covers the difference.
    tracing::info!("NovaProxy exiting");
    usage!("app.stop");
}
