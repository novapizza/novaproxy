//! NovaProxy Tauri shell: owns app state, initializes the CA, and exposes the
//! engine to the frontend through commands + a flow-streaming channel.

mod commands;
mod state;

use std::path::PathBuf;

use nova_core::ca::CaMaterial;
use state::AppState;

fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("NovaProxy")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "novaproxy=info,nova_core=info".into()),
        )
        .init();

    let state = AppState::new(data_dir());

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .setup(|app| {
            use tauri::Manager;
            let st = app.state::<AppState>();
            match CaMaterial::load_or_create(&st.data_dir) {
                Ok(ca) => {
                    tracing::info!("root CA ready at {}", ca.cert_path.display());
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

            // System-proxy safety net: a leftover backup means we were mutating
            // the OS proxy when the app last exited (likely a crash). Restore the
            // saved settings in the background so the user isn't stranded.
            let backup_path = st.sysproxy_backup_path();
            if backup_path.exists() {
                std::thread::spawn(move || {
                    if let Ok(text) = std::fs::read_to_string(&backup_path) {
                        if let Ok(backup) = serde_json::from_str::<nova_core::sysproxy::Backup>(&text)
                        {
                            tracing::warn!("restoring system proxy after unclean exit");
                            let _ = nova_core::sysproxy::disable(&backup);
                        }
                    }
                    let _ = std::fs::remove_file(&backup_path);
                });
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
            commands::subscribe_breakpoints,
            commands::set_breakpoint,
            commands::resume_breakpoint,
            commands::set_system_proxy,
            commands::resend_flow,
            commands::ca_status,
            commands::install_ca,
            commands::uninstall_ca,
            commands::regenerate_ca,
        ])
        .run(tauri::generate_context!())
        .expect("error while running NovaProxy");
}
