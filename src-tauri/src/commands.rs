//! Tauri commands: the frontend's entire surface onto the engine.

use std::net::SocketAddr;

use nova_core::breakpoint::Resume;
use nova_core::{ca::CaMaterial, sysproxy, trust, EngineConfig};
use nova_proto::{CaStatus, Flow, Header, Interception, NetworkConditions, ProxyStatus, Rule};
use tauri::ipc::Channel;
use tauri::State;

use crate::state::AppState;

/// Register the frontend channel that receives streamed flow updates.
#[tauri::command]
pub fn subscribe_flows(state: State<'_, AppState>, channel: Channel<Flow>) {
    state.sink.set_channel(channel);
}

#[tauri::command]
pub fn proxy_status(state: State<'_, AppState>) -> ProxyStatus {
    make_status(&state)
}

#[tauri::command]
pub async fn start_proxy(
    state: State<'_, AppState>,
    port: Option<u16>,
) -> Result<ProxyStatus, String> {
    ensure_engine(&state, port)?;
    Ok(make_status(&state))
}

#[tauri::command]
pub fn stop_proxy(state: State<'_, AppState>) -> ProxyStatus {
    if let Some(handle) = state.engine.lock().unwrap().take() {
        handle.stop();
    }
    make_status(&state)
}

/* ------------------------------- rules ------------------------------- */

#[tauri::command]
pub fn get_rules(state: State<'_, AppState>) -> Vec<Rule> {
    state.rules.read().unwrap().clone()
}

#[tauri::command]
pub fn set_rules(state: State<'_, AppState>, rules: Vec<Rule>) -> Result<(), String> {
    // Update the live set the engine reads, then persist.
    *state.rules.write().unwrap() = rules.clone();
    let json = serde_json::to_string_pretty(&rules).map_err(|e| e.to_string())?;
    std::fs::write(state.rules_path(), json).map_err(|e| e.to_string())?;
    Ok(())
}

/* --------------------------- session / export --------------------------- */

/// Write text to an absolute path chosen by the user via the save dialog.
#[tauri::command]
pub fn write_file(path: String, contents: String) -> Result<(), String> {
    std::fs::write(&path, contents).map_err(|e| e.to_string())
}

/// Read text from an absolute path chosen via the open dialog.
#[tauri::command]
pub fn read_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| e.to_string())
}

/* ------------------------------- scripts ------------------------------- */

/// Return the persisted script source (empty string if none yet).
#[tauri::command]
pub fn get_script(state: State<'_, AppState>) -> String {
    std::fs::read_to_string(state.script_path()).unwrap_or_default()
}

/// Set the script source and whether it runs against live traffic; persist it.
#[tauri::command]
pub fn set_script(
    state: State<'_, AppState>,
    source: String,
    enabled: bool,
) -> Result<(), String> {
    state.scripts.set_script(source.clone());
    state.scripts.set_enabled(enabled);
    std::fs::write(state.script_path(), source).map_err(|e| e.to_string())?;
    Ok(())
}

/* -------------------------- network conditions -------------------------- */

#[tauri::command]
pub fn get_network_conditions(state: State<'_, AppState>) -> NetworkConditions {
    *state.net.read().unwrap()
}

#[tauri::command]
pub fn set_network_conditions(
    state: State<'_, AppState>,
    net: NetworkConditions,
) -> Result<(), String> {
    *state.net.write().unwrap() = net;
    let json = serde_json::to_string_pretty(&net).map_err(|e| e.to_string())?;
    std::fs::write(state.net_path(), json).map_err(|e| e.to_string())?;
    Ok(())
}

/* ----------------------------- breakpoints ----------------------------- */

/// Register the channel that receives paused-request notifications.
#[tauri::command]
pub fn subscribe_breakpoints(state: State<'_, AppState>, channel: Channel<Interception>) {
    state.bp_sink.set_channel(channel);
}

/// Arm (with an optional URL glob) or disarm the breakpoint.
#[tauri::command]
pub fn set_breakpoint(state: State<'_, AppState>, armed: bool, pattern: Option<String>) {
    if armed {
        state.breakpoints.arm(pattern.unwrap_or_else(|| "*".into()));
    } else {
        state.breakpoints.disarm();
    }
}

/// Resolve a paused request: continue (with edited headers) or abort.
#[tauri::command]
pub fn resume_breakpoint(
    state: State<'_, AppState>,
    id: String,
    cont: bool,
    headers: Vec<Header>,
) {
    let resume = if cont {
        Resume::Continue(headers)
    } else {
        Resume::Abort
    };
    state.breakpoints.resume(&id, resume);
}

/* -------------------------- system proxy -------------------------- */

#[tauri::command]
pub async fn set_system_proxy(
    state: State<'_, AppState>,
    enable: bool,
) -> Result<ProxyStatus, String> {
    if enable {
        let addr = ensure_engine(&state, None)?;
        let backup = tauri::async_runtime::spawn_blocking(sysproxy::snapshot)
            .await
            .map_err(|e| e.to_string())?;
        // Persist the snapshot BEFORE mutating, so a crash mid-session is
        // recoverable on next launch.
        let backup_json = serde_json::to_string_pretty(&backup).map_err(|e| e.to_string())?;
        std::fs::write(state.sysproxy_backup_path(), backup_json).map_err(|e| e.to_string())?;

        let host = addr.ip().to_string();
        let port = addr.port();
        tauri::async_runtime::spawn_blocking(move || sysproxy::enable(&host, port, &backup))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        *state.system_proxy.lock().unwrap() = true;
    } else {
        let backup = read_backup(&state);
        if let Some(backup) = backup {
            tauri::async_runtime::spawn_blocking(move || sysproxy::disable(&backup))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
        }
        let _ = std::fs::remove_file(state.sysproxy_backup_path());
        *state.system_proxy.lock().unwrap() = false;
        if let Some(handle) = state.engine.lock().unwrap().take() {
            handle.stop();
        }
    }
    Ok(make_status(&state))
}

/* ------------------------------- resend ------------------------------- */

/// Replay a captured flow by re-issuing it *through* the proxy, so it is
/// recaptured as a fresh flow (tagged `resent` via the `x-nova-resend` header).
#[tauri::command]
pub async fn resend_flow(state: State<'_, AppState>, flow: Flow) -> Result<(), String> {
    let addr = ensure_engine(&state, None)?;
    let proxy_url = format!("http://{}:{}", addr.ip(), addr.port());

    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(&proxy_url).map_err(|e| e.to_string())?)
        // The proxy presents our MITM leaf; trusting it here is expected.
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| e.to_string())?;

    let method =
        reqwest::Method::from_bytes(flow.method.as_bytes()).map_err(|e| e.to_string())?;
    let mut req = client.request(method, &flow.url).header("x-nova-resend", "1");

    for h in &flow.request_headers {
        let lname = h.name.to_ascii_lowercase();
        if matches!(
            lname.as_str(),
            "host" | "content-length" | "connection" | "transfer-encoding" | "accept-encoding"
        ) || lname.starts_with(':')
        {
            continue;
        }
        req = req.header(&h.name, &h.value);
    }

    if let Some(body) = &flow.request_body {
        if let Some(text) = &body.text {
            req = req.body(text.clone());
        } else if let Some(b64) = &body.base64 {
            use base64::Engine;
            if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) {
                req = req.body(bytes);
            }
        }
    }

    req.send().await.map_err(|e| e.to_string())?;
    Ok(())
}

/* ----------------------------- certificate ----------------------------- */

#[tauri::command]
pub fn ca_status(state: State<'_, AppState>) -> Result<CaStatus, String> {
    ca_status_inner(&state)
}

#[tauri::command]
pub async fn install_ca(state: State<'_, AppState>) -> Result<CaStatus, String> {
    let cert_path = ca_cert_path(&state)?;
    tauri::async_runtime::spawn_blocking(move || trust::install(&cert_path))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    ca_status_inner(&state)
}

#[tauri::command]
pub async fn uninstall_ca(state: State<'_, AppState>) -> Result<CaStatus, String> {
    let cert_path = ca_cert_path(&state)?;
    tauri::async_runtime::spawn_blocking(move || trust::uninstall(&cert_path))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    ca_status_inner(&state)
}

#[tauri::command]
pub async fn regenerate_ca(state: State<'_, AppState>) -> Result<CaStatus, String> {
    let (data_dir, old_path) = {
        let guard = state.ca.lock().unwrap();
        (state.data_dir.clone(), guard.as_ref().map(|c| c.cert_path.clone()))
    };
    if let Some(old) = old_path {
        let _ = tauri::async_runtime::spawn_blocking(move || trust::uninstall(&old)).await;
    }
    let _ = std::fs::remove_file(data_dir.join("ca.pem"));
    let _ = std::fs::remove_file(data_dir.join("ca.key"));
    let fresh = CaMaterial::load_or_create(&data_dir).map_err(|e| e.to_string())?;
    *state.ca.lock().unwrap() = Some(fresh);
    ca_status_inner(&state)
}

/* ------------------------------- helpers ------------------------------- */

/// Start the engine if it isn't already running; return its listen address.
fn ensure_engine(state: &AppState, port: Option<u16>) -> Result<SocketAddr, String> {
    if let Some(handle) = state.engine.lock().unwrap().as_ref() {
        return Ok(handle.addr);
    }
    let addr = SocketAddr::from(([127, 0, 0, 1], port.unwrap_or(9090)));
    let handle = {
        let ca_guard = state.ca.lock().unwrap();
        let ca = ca_guard
            .as_ref()
            .ok_or_else(|| "Certificate authority not initialized".to_string())?;
        nova_core::start(
            EngineConfig {
                addr,
                body_cap: nova_core::DEFAULT_BODY_CAP,
            },
            ca,
            state.sink.clone(),
            nova_core::EngineHooks {
                rules: state.rules.clone(),
                breakpoints: state.breakpoints.clone(),
                scripts: state.scripts.clone(),
                net: state.net.clone(),
            },
        )
        .map_err(|e| format!("failed to start proxy: {e}"))?
    };
    *state.engine.lock().unwrap() = Some(handle);
    Ok(addr)
}

fn read_backup(state: &AppState) -> Option<sysproxy::Backup> {
    let text = std::fs::read_to_string(state.sysproxy_backup_path()).ok()?;
    serde_json::from_str(&text).ok()
}

fn make_status(state: &AppState) -> ProxyStatus {
    let system_proxy = *state.system_proxy.lock().unwrap();
    match state.engine.lock().unwrap().as_ref() {
        Some(handle) => ProxyStatus {
            running: true,
            host: Some(handle.addr.ip().to_string()),
            port: Some(handle.addr.port()),
            flows_captured: handle.flows_captured(),
            system_proxy,
        },
        None => ProxyStatus {
            system_proxy,
            ..Default::default()
        },
    }
}

fn ca_cert_path(state: &AppState) -> Result<std::path::PathBuf, String> {
    state
        .ca
        .lock()
        .unwrap()
        .as_ref()
        .map(|c| c.cert_path.clone())
        .ok_or_else(|| "Certificate authority not initialized".to_string())
}

fn ca_status_inner(state: &AppState) -> Result<CaStatus, String> {
    let guard = state.ca.lock().unwrap();
    let ca = guard
        .as_ref()
        .ok_or_else(|| "Certificate authority not initialized".to_string())?;
    let fingerprint = ca.fingerprint();
    let trusted = trust::is_trusted(&fingerprint);
    Ok(CaStatus {
        cert_path: ca.cert_path.display().to_string(),
        fingerprint,
        trusted,
        subject: ca.subject(),
    })
}
