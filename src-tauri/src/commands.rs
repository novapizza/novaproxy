//! Tauri commands: the frontend's entire surface onto the engine.

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use nova_core::breakpoint::Resume;
use nova_core::{ca::CaMaterial, helper, sysproxy, trust, EngineConfig};
use nova_proto::{
    CaStatus, Flow, Header, HelperStatus, Interception, McpStatus, NetworkConditions, ProxyStatus,
    Rule, TlsScope, WsMessage,
};
use tauri::ipc::Channel;
use tauri::State;

use crate::logging::redact;
use crate::state::AppState;

/// Record the outcome of an operation that changes something outside the app.
///
/// Every privileged path funnels through the commands in this file, so this is
/// the one place that can answer "what did the app do to this machine, and did
/// it work?" — the question a support log exists for.
///
/// Read-only commands are deliberately **not** wrapped. `proxy_status` is
/// polled every two seconds by the UI; a line per poll would bury every line
/// that matters and blow through the log budget in a day.
///
/// The error text goes to diagnostics only, and redacted: `security` and
/// `networksetup` quote the user's home path back on failure. The usage stream
/// gets the outcome and nothing else — free text there is a leak waiting to
/// happen, and counts are all it is for.
fn record<T>(op: &'static str, started: std::time::Instant, result: Result<T, String>) -> Result<T, String> {
    let ms = started.elapsed().as_millis() as u64;
    match &result {
        Ok(_) => {
            tracing::info!(op, ms, "ok");
            crate::usage!(op, result = "ok", ms = ms);
        }
        Err(e) => {
            tracing::error!(op, ms, "failed: {}", redact(e));
            crate::usage!(op, result = "fail", ms = ms);
        }
    }
    result
}

/// Marks the start of a recorded operation, so the call sites read as a pair.
fn started() -> std::time::Instant {
    std::time::Instant::now()
}

/// Register the frontend channel that receives streamed flow updates.
#[tauri::command]
pub fn subscribe_flows(state: State<'_, Arc<AppState>>, channel: Channel<Flow>) {
    state.sink.set_channel(channel);
}

/// Register the frontend channel that receives captured WebSocket frames.
#[tauri::command]
pub fn subscribe_ws(state: State<'_, Arc<AppState>>, channel: Channel<WsMessage>) {
    state.ws_sink.set_channel(channel);
}

#[tauri::command]
pub fn proxy_status(state: State<'_, Arc<AppState>>) -> ProxyStatus {
    make_status(&state)
}

#[tauri::command]
pub async fn start_proxy(
    state: State<'_, Arc<AppState>>,
    port: Option<u16>,
) -> Result<ProxyStatus, String> {
    let t = started();
    record("proxy.start", t, ensure_engine(&state, port).map(|_| ()))?;
    Ok(make_status(&state))
}

#[tauri::command]
pub fn stop_proxy(state: State<'_, Arc<AppState>>) -> ProxyStatus {
    // Only when something was actually running: the UI calls this on paths
    // where the engine may already be down, and "stopped nothing" is noise.
    if let Some(handle) = state.engine.lock().unwrap().take() {
        handle.stop();
        tracing::info!(op = "proxy.stop", "ok");
        crate::usage!("proxy.stop", result = "ok");
    }
    make_status(&state)
}

/* ------------------------------- rules ------------------------------- */

#[tauri::command]
pub fn get_rules(state: State<'_, Arc<AppState>>) -> Vec<Rule> {
    state.rules.read().unwrap().clone()
}

#[tauri::command]
pub fn set_rules(state: State<'_, Arc<AppState>>, rules: Vec<Rule>) -> Result<(), String> {
    // How many and of what kind — never the patterns. A rule's match pattern is
    // a URL from the traffic the user is debugging, which is theirs.
    let enabled = rules.iter().filter(|r| r.enabled).count();
    tracing::info!(total = rules.len(), enabled, "rule set changed");
    crate::usage!("rules.set", total = rules.len(), enabled = enabled);

    // Update the live set the engine reads, then persist.
    *state.rules.write().unwrap() = rules.clone();
    state.persist_rules(&rules)
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

/* ---------------------------- flows / capture ---------------------------- */

/// Drop every retained flow (and its spilled bodies). The UI's Clear action calls
/// this so the engine, the UI and the MCP server agree on what is still captured
/// — clearing only the frontend list would leave an agent reading flows the user
/// believes they discarded.
#[tauri::command]
pub fn clear_flows(state: State<'_, Arc<AppState>>) {
    // The count answers "where did my capture go?", which is a real support
    // question now that clearing also empties what the MCP server serves.
    let had = state.flows.retained();
    state.flows.clear();
    tracing::info!(cleared = had, "flows cleared");
    crate::usage!("flows.clear", n = had);
}

/* --------------------------------- MCP --------------------------------- */

#[tauri::command]
pub fn mcp_status(state: State<'_, Arc<AppState>>) -> McpStatus {
    mcp_status_of(&state)
}

/// Start or stop the MCP endpoint, persisting the choice for next launch.
#[tauri::command]
pub async fn set_mcp_enabled(
    state: State<'_, Arc<AppState>>,
    enable: bool,
    port: Option<u16>,
) -> Result<McpStatus, String> {
    let app: Arc<AppState> = (*state).clone();
    if enable {
        // Restarting on a port change is the only way to move the listener.
        let running_port = app.mcp.lock().unwrap().as_ref().map(|h| h.addr.port());
        let wanted = port.unwrap_or(crate::mcp::DEFAULT_MCP_PORT);
        if running_port == Some(wanted) {
            return Ok(mcp_status_of(&app));
        }
        stop_mcp(&app);
        let handle = crate::mcp::start(app.clone(), wanted).await?;
        app.mcp_port.store(handle.addr.port(), Ordering::Relaxed);
        *app.mcp.lock().unwrap() = Some(handle);
    } else {
        stop_mcp(&app);
    }
    let status = mcp_status_of(&app);
    let json = serde_json::to_string_pretty(&status).map_err(|e| e.to_string())?;
    std::fs::write(app.mcp_path(), json).map_err(|e| e.to_string())?;
    Ok(status)
}

/// Stop the endpoint if it is running, and clear the port the engine sees.
pub fn stop_mcp(state: &AppState) {
    if let Some(handle) = state.mcp.lock().unwrap().take() {
        handle.stop();
    }
    state.mcp_port.store(0, Ordering::Relaxed);
}

fn mcp_status_of(state: &AppState) -> McpStatus {
    match state.mcp.lock().unwrap().as_ref() {
        Some(handle) => McpStatus {
            running: true,
            port: handle.addr.port(),
            url: Some(format!("http://{}/", handle.addr)),
        },
        None => McpStatus {
            running: false,
            port: crate::mcp::DEFAULT_MCP_PORT,
            url: None,
        },
    }
}

/* -------------------------------- bodies -------------------------------- */

/// Largest body the Inspector will pull back into the UI in one go. Bodies can
/// be far larger on disk; the returned preview reports the true size and flags
/// the truncation.
const MAX_READ_BODY: u64 = 16 * 1024 * 1024;

/// Fetch a body the UI is not holding: from the on-disk store when it spilled,
/// otherwise from the retained flow itself.
///
/// Two things ask for this. Large bodies keep only a capped preview on the
/// [`Flow`] with the complete bytes on disk, and the Inspector shows the rest on
/// demand. Smaller bodies never spill — but the UI does not keep every preview
/// in the webview either (that is what made a long capture exhaust its memory),
/// so it comes back here for those too. `media_type` and `encoding` come from
/// the flow's own preview, so stored bytes are decoded exactly as they were live.
#[tauri::command]
pub fn read_body(
    state: State<'_, Arc<AppState>>,
    flow_id: String,
    side: String,
    media_type: Option<String>,
    encoding: Option<String>,
) -> Result<nova_proto::BodyPreview, String> {
    read_body_from(&state, &flow_id, &side, media_type, encoding)
}

/// The body of `read_body`, taking `&AppState` so it is reachable from tests.
pub fn read_body_from(
    state: &AppState,
    flow_id: &str,
    side: &str,
    media_type: Option<String>,
    encoding: Option<String>,
) -> Result<nova_proto::BodyPreview, String> {
    let which = match side {
        "request" => nova_core::flow::Side::Request,
        "response" => nova_core::flow::Side::Response,
        other => return Err(format!("unknown body side {other:?}")),
    };
    match state.bodies.read(flow_id, which, MAX_READ_BODY) {
        Ok((bytes, total, truncated)) => {
            let mut preview =
                nova_core::flow::build_preview(bytes, total, truncated, media_type, encoding);
            // The body is still on disk; the UI can ask again.
            preview.spilled = true;
            Ok(preview)
        }
        // Nothing on disk: the body was small enough to stay inline, so the copy
        // the retained flow carries *is* the whole body. Reported as unspilled,
        // because it is — the UI must not offer to "load the full body" from a
        // store that does not have it.
        Err(disk_err) => {
            // Only "there is nothing on disk" may fall through to the inline
            // preview. A body that spilled but failed to read back must stay an
            // error: the preview is capped, and handing it out here would pass
            // off a truncated body as the whole thing.
            let nothing_stored = disk_err
                .downcast_ref::<std::io::Error>()
                .map_or(true, |io| io.kind() == std::io::ErrorKind::NotFound);
            if !nothing_stored {
                return Err(format!(
                    "reading the stored {side} body of flow {flow_id}: {disk_err:#}"
                ));
            }
            let flow = state
                .flows
                .get(flow_id)
                .ok_or_else(|| format!("flow {flow_id} is no longer retained: {disk_err}"))?;
            let body = match which {
                nova_core::flow::Side::Request => flow.request_body,
                nova_core::flow::Side::Response => flow.response_body,
            };
            body.ok_or_else(|| format!("flow {flow_id} has no {side} body"))
        }
    }
}

/// Every flow the engine still retains, newest first.
///
/// The UI drops body previews it is not showing, so anything that needs *all* of
/// them — saving a session, exporting HAR — asks for this rather than reading its
/// own list.
#[tauri::command]
pub fn retained_flows(state: State<'_, Arc<AppState>>) -> Vec<Flow> {
    state.flows.newest_first()
}

/* ------------------------------- scripts ------------------------------- */

/// Return the persisted script source (empty string if none yet).
#[tauri::command]
pub fn get_script(state: State<'_, Arc<AppState>>) -> String {
    std::fs::read_to_string(state.script_path()).unwrap_or_default()
}

/// Set the script source and whether it runs against live traffic; persist it.
#[tauri::command]
pub fn set_script(
    state: State<'_, Arc<AppState>>,
    source: String,
    enabled: bool,
) -> Result<(), String> {
    // Its length, not its text: the script is the user's own code and can
    // easily contain a token they pasted in to reproduce something.
    tracing::info!(enabled, bytes = source.len(), "script changed");
    crate::usage!("script.set", enabled = enabled);

    state.scripts.set_script(source.clone());
    state.scripts.set_enabled(enabled);
    std::fs::write(state.script_path(), source).map_err(|e| e.to_string())?;
    Ok(())
}

/* -------------------------- network conditions -------------------------- */

#[tauri::command]
pub fn get_network_conditions(state: State<'_, Arc<AppState>>) -> NetworkConditions {
    *state.net.read().unwrap()
}

#[tauri::command]
pub fn set_network_conditions(
    state: State<'_, Arc<AppState>>,
    net: NetworkConditions,
) -> Result<(), String> {
    tracing::info!(
        enabled = net.enabled,
        latency_ms = net.latency_ms,
        down_kbps = net.down_kbps,
        "network conditions changed"
    );
    crate::usage!("network.set", enabled = net.enabled);

    *state.net.write().unwrap() = net;
    let json = serde_json::to_string_pretty(&net).map_err(|e| e.to_string())?;
    std::fs::write(state.net_path(), json).map_err(|e| e.to_string())?;
    Ok(())
}

/* ------------------------------ TLS scope ------------------------------ */

#[tauri::command]
pub fn get_tls_scope(state: State<'_, Arc<AppState>>) -> TlsScope {
    state.tls_scope.read().unwrap().clone()
}

#[tauri::command]
pub fn set_tls_scope(state: State<'_, Arc<AppState>>, scope: TlsScope) -> Result<(), String> {
    *state.tls_scope.write().unwrap() = scope.clone();
    let json = serde_json::to_string_pretty(&scope).map_err(|e| e.to_string())?;
    std::fs::write(state.tls_scope_path(), json).map_err(|e| e.to_string())?;
    Ok(())
}

/* ----------------------------- breakpoints ----------------------------- */

/// Register the channel that receives paused-request notifications.
#[tauri::command]
pub fn subscribe_breakpoints(state: State<'_, Arc<AppState>>, channel: Channel<Interception>) {
    state.bp_sink.set_channel(channel);
}

/// Arm (with an optional URL glob) or disarm the breakpoint.
#[tauri::command]
pub fn set_breakpoint(state: State<'_, Arc<AppState>>, armed: bool, pattern: Option<String>) {
    if armed {
        state.breakpoints.arm(pattern.unwrap_or_else(|| "*".into()));
    } else {
        state.breakpoints.disarm();
    }
}

/// Resolve a paused request: continue (with edited headers) or abort.
#[tauri::command]
pub fn resume_breakpoint(
    state: State<'_, Arc<AppState>>,
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
    state: State<'_, Arc<AppState>>,
    enable: bool,
) -> Result<ProxyStatus, String> {
    if enable {
        let addr = ensure_engine(&state, None)?;
        // A backup already on disk is a snapshot of the machine *before*
        // NovaProxy touched it, left behind by an unclean exit. Snapshotting
        // again would record "proxied to NovaProxy" as the state to return to,
        // so the user could never get their settings back — keep the old one.
        let backup = match read_backup(&state) {
            Some(existing) => existing,
            None => {
                let fresh = tauri::async_runtime::spawn_blocking(sysproxy::snapshot)
                    .await
                    .map_err(|e| e.to_string())?;
                // Persist the snapshot BEFORE mutating, so a crash mid-session is
                // recoverable on next launch. Worth a line of its own: if this
                // write is the step that failed, the machine was never touched,
                // and that is a different support conversation.
                let backup_json =
                    serde_json::to_string_pretty(&fresh).map_err(|e| e.to_string())?;
                let t = started();
                record(
                    "sysproxy.backup",
                    t,
                    std::fs::write(state.sysproxy_backup_path(), backup_json)
                        .map_err(|e| e.to_string()),
                )?;
                fresh
            }
        };

        let host = addr.ip().to_string();
        let port = addr.port();
        // Whether the helper is answering decides what this costs the user: a
        // silent apply, or an administrator password prompt. Support cannot
        // read a "it asked for my password again" report without it.
        let via_helper = helper::usable();
        let t = started();
        let outcome = tauri::async_runtime::spawn_blocking(move || sysproxy::enable(&host, port, &backup))
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r.map_err(|e| e.to_string()));
        record("sysproxy.enable", t, outcome)?;
        crate::usage!("sysproxy.enable.path", via_helper = via_helper);
        *state.system_proxy.lock().unwrap() = true;
        // Whatever was outstanding is now this session's business to undo.
        state.pending_restore.store(false, Ordering::Relaxed);
    } else {
        let backup = read_backup(&state);
        if let Some(backup) = backup {
            let t = started();
            let outcome = tauri::async_runtime::spawn_blocking(move || sysproxy::disable(&backup))
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()));
            record("sysproxy.disable", t, outcome)?;
        } else {
            // No backup means nothing to put back — either a clean no-op or
            // evidence the snapshot was lost. Say which.
            tracing::warn!(op = "sysproxy.disable", "no backup on disk; nothing to restore");
        }
        let _ = std::fs::remove_file(state.sysproxy_backup_path());
        *state.system_proxy.lock().unwrap() = false;
        state.pending_restore.store(false, Ordering::Relaxed);
        if let Some(handle) = state.engine.lock().unwrap().take() {
            handle.stop();
        }
    }
    Ok(make_status(&state))
}

/// Put back proxy settings left over from a session that ended uncleanly.
///
/// Separate from `set_system_proxy(false)` because it is offered, not implied:
/// without a helper this raises the administrator prompt, and the app must never
/// do that on its own — least of all during launch, which is what it used to do.
#[tauri::command]
pub async fn restore_system_proxy(state: State<'_, Arc<AppState>>) -> Result<ProxyStatus, String> {
    let owned = (*state).clone();
    let t = started();
    let outcome = tauri::async_runtime::spawn_blocking(move || crate::restore_from_backup(&owned))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r.map_err(|e| e.to_string()));
    record("sysproxy.restore", t, outcome)?;
    *state.system_proxy.lock().unwrap() = false;
    Ok(make_status(&state))
}

/* ------------------------------ ui logging ------------------------------ */

/// Take a message the webview could not otherwise record.
///
/// The frontend has no file to write to and, in a bundled app, no console
/// anyone reads — a render crash or a rejected promise used to blank the window
/// and leave nothing behind. This is the one way those reach disk.
///
/// Everything here is untrusted text from a process that renders captured
/// traffic, so it is redacted and truncated before it is written, and it never
/// reaches the usage stream. `kind` is a fixed vocabulary from the caller, not
/// free text, which is what keeps it countable.
#[tauri::command]
pub fn log_from_ui(level: String, kind: String, message: String) {
    let text = sanitize_ui_message(&message);
    let kind = ui_kind(&kind);
    match level.as_str() {
        "error" => tracing::error!(target: "novaproxy_ui", kind, "{text}"),
        "warn" => tracing::warn!(target: "novaproxy_ui", kind, "{text}"),
        _ => tracing::info!(target: "novaproxy_ui", kind, "{text}"),
    }
    if level == "error" {
        crate::usage!("ui.error", kind = kind);
    }
}

/// Long enough for a React component stack, short enough that an error loop —
/// a component that throws on every render, say — cannot fill the disk before
/// anyone notices.
const MAX_UI_MESSAGE: usize = 4096;

/// Make a webview string safe to write down: home directory removed, length
/// bounded.
fn sanitize_ui_message(message: &str) -> String {
    let mut text = redact(message);
    if text.len() > MAX_UI_MESSAGE {
        // On a char boundary, or this panics on the multi-byte text that a
        // captured response body is full of.
        let mut cut = MAX_UI_MESSAGE;
        while cut > 0 && !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str(" …[truncated]");
    }
    text
}

/// Fold an arbitrary string into the fixed vocabulary the usage counts use.
///
/// A closed set rather than free text: an open one makes the counts
/// unaggregatable, and lets a caller widen the schema by typo.
fn ui_kind(kind: &str) -> &'static str {
    match kind {
        "render" => "render",
        "unhandled-rejection" => "unhandled-rejection",
        "window-error" => "window-error",
        "command" => "command",
        _ => "other",
    }
}

#[cfg(test)]
mod ui_log_tests {
    use super::*;

    #[test]
    fn unknown_kinds_fold_to_other() {
        assert_eq!(ui_kind("render"), "render");
        assert_eq!(ui_kind("command"), "command");
        assert_eq!(ui_kind("whatever-a-caller-invented"), "other");
        assert_eq!(ui_kind(""), "other");
    }

    #[test]
    fn long_messages_are_bounded() {
        let huge = "x".repeat(MAX_UI_MESSAGE * 3);
        let out = sanitize_ui_message(&huge);
        assert!(out.len() < MAX_UI_MESSAGE + 32, "{}", out.len());
        assert!(out.ends_with("[truncated]"));
    }

    #[test]
    fn truncation_does_not_split_a_character() {
        // The webview renders captured traffic, so its error text is full of
        // multi-byte characters; cutting one in half panics `truncate`.
        let multibyte = "é".repeat(MAX_UI_MESSAGE);
        let out = sanitize_ui_message(&multibyte);
        assert!(out.ends_with("[truncated]"));
        assert!(out.len() <= MAX_UI_MESSAGE + 32);
    }

    #[test]
    fn short_messages_pass_through_whole() {
        assert_eq!(sanitize_ui_message("boom"), "boom");
    }

    #[test]
    fn the_home_directory_is_stripped_from_ui_text() {
        let Some(home) = dirs::home_dir() else { return };
        let home = home.to_string_lossy().into_owned();
        let out = sanitize_ui_message(&format!("failed to read {home}/Documents/x.har"));
        assert!(!out.contains(&home), "{out}");
    }
}

/* -------------------------- privileged helper -------------------------- */

#[tauri::command]
pub fn helper_status() -> HelperStatus {
    crate::helper_status_now()
}

/// Install the helper — one administrator prompt, and then no more.
#[tauri::command]
pub async fn install_helper(state: State<'_, Arc<AppState>>) -> Result<HelperStatus, String> {
    let source = helper::source_binary().ok_or_else(|| {
        "The helper binary was not found next to the app. Build it with \
         `cargo build -p nova-helper`, or set NOVAPROXY_HELPER_BIN."
            .to_string()
    })?;
    let staged = state.data_dir.clone();
    let t = started();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        helper::install(&source, &staged, helper::current_uid())
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r.map_err(|e| e.to_string()));
    record("helper.install", t, outcome)?;
    Ok(crate::helper_status_now())
}

#[tauri::command]
pub async fn uninstall_helper() -> Result<HelperStatus, String> {
    let t = started();
    let outcome = tauri::async_runtime::spawn_blocking(helper::uninstall)
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r.map_err(|e| e.to_string()));
    record("helper.uninstall", t, outcome)?;
    Ok(crate::helper_status_now())
}

/* ------------------------------- resend ------------------------------- */

/// Replay a captured flow by re-issuing it *through* the proxy, so it is
/// recaptured as a fresh flow (tagged `resent` via the `x-nova-resend` header).
#[tauri::command]
pub async fn resend_flow(state: State<'_, Arc<AppState>>, mut flow: Flow) -> Result<(), String> {
    hydrate_request_body(&state, &mut flow);
    replay(&state, flow, false).await
}

/// Put the request body back on a flow that arrived without one.
///
/// The UI sends the flow it holds, and it does not hold body previews for flows
/// it is not showing — replaying one must still send the body that was captured,
/// so it is taken from the retained flow here.
pub fn hydrate_request_body(state: &AppState, flow: &mut Flow) {
    let missing = match &flow.request_body {
        None => false, // no body was captured at all: nothing to put back
        Some(b) => b.text.is_none() && b.base64.is_none() && b.size > 0,
    };
    if !missing {
        return;
    }
    let media = flow.request_body.as_ref().and_then(|b| b.media_type.clone());
    let encoding = flow.request_body.as_ref().and_then(|b| b.decoded_from.clone());
    if let Ok(body) = read_body_from(state, &flow.id, "request", media, encoding) {
        flow.request_body = Some(body);
    }
}

/// Re-issue `flow` through the proxy.
///
/// `internal` marks the replay as NovaProxy's own traffic — set when the MCP
/// server replays on an agent's behalf, so those flows stay out of unfiltered
/// listings instead of polluting the capture the agent is reading.
pub async fn replay(state: &AppState, flow: Flow, internal: bool) -> Result<(), String> {
    let addr = ensure_engine(state, None)?;
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
    if internal {
        req = req.header("x-nova-internal", "1");
    }

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
pub fn ca_status(state: State<'_, Arc<AppState>>) -> Result<CaStatus, String> {
    ca_status_inner(&state)
}

/// Install the CA and trust it. Defaults to the current user's trust domain,
/// which needs no administrator password; `all_users` opts into the machine-wide
/// system store (and its admin prompt) instead.
#[tauri::command]
pub async fn install_ca(
    state: State<'_, Arc<AppState>>,
    all_users: Option<bool>,
) -> Result<CaStatus, String> {
    let ca = ca_id(&state)?;
    let domain = if all_users.unwrap_or(false) {
        trust::TrustDomain::System
    } else {
        trust::TrustDomain::User
    };
    // The trust domain is the whole story of this operation: the user domain
    // needs no password, the system domain costs an admin prompt.
    let scope = if matches!(domain, trust::TrustDomain::System) { "system" } else { "user" };
    let t = started();
    let outcome = tauri::async_runtime::spawn_blocking(move || trust::install(&ca, domain))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r.map_err(|e| e.to_string()));
    record("cert.install", t, outcome)?;
    crate::usage!("cert.install.scope", scope = scope);
    ca_status_inner(&state)
}

#[tauri::command]
pub async fn uninstall_ca(state: State<'_, Arc<AppState>>) -> Result<CaStatus, String> {
    let ca = ca_id(&state)?;
    let t = started();
    let outcome = tauri::async_runtime::spawn_blocking(move || trust::uninstall(&ca))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r.map_err(|e| e.to_string()));
    record("cert.uninstall", t, outcome)?;
    ca_status_inner(&state)
}

#[tauri::command]
pub async fn regenerate_ca(state: State<'_, Arc<AppState>>) -> Result<CaStatus, String> {
    let (data_dir, old) = {
        let guard = state.ca.lock().unwrap();
        (
            state.data_dir.clone(),
            guard.as_ref().map(trust::CaId::of),
        )
    };
    if let Some(old) = old {
        let _ = tauri::async_runtime::spawn_blocking(move || trust::uninstall(&old)).await;
    }
    let _ = std::fs::remove_file(data_dir.join("ca.pem"));
    let _ = std::fs::remove_file(data_dir.join("ca.key"));
    let t = started();
    let fresh = record(
        "cert.regenerate",
        t,
        CaMaterial::load_or_create(&data_dir).map_err(|e| e.to_string()),
    )?;
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
            EngineConfig { addr },
            ca,
            state.sink.clone(),
            state.ws_sink.clone(),
            state.hooks(),
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
    let flows_captured = state.flows.total_captured();
    let pending_restore = state.pending_restore.load(Ordering::Relaxed);
    match state.engine.lock().unwrap().as_ref() {
        Some(handle) => ProxyStatus {
            running: true,
            host: Some(handle.addr.ip().to_string()),
            port: Some(handle.addr.port()),
            flows_captured,
            system_proxy,
            pending_restore,
        },
        None => ProxyStatus {
            flows_captured,
            system_proxy,
            pending_restore,
            ..Default::default()
        },
    }
}

/// Identity of the loaded CA — the two digests plus the cert path that the
/// per-platform trust stores need.
fn ca_id(state: &AppState) -> Result<trust::CaId, String> {
    state
        .ca
        .lock()
        .unwrap()
        .as_ref()
        .map(trust::CaId::of)
        .ok_or_else(|| "Certificate authority not initialized".to_string())
}

fn ca_status_inner(state: &AppState) -> Result<CaStatus, String> {
    let guard = state.ca.lock().unwrap();
    let ca = guard
        .as_ref()
        .ok_or_else(|| "Certificate authority not initialized".to_string())?;
    let id = trust::CaId::of(ca);
    let state = trust::trust_state(&id);
    Ok(CaStatus {
        cert_path: ca.cert_path.display().to_string(),
        fingerprint: id.sha256,
        trusted: state.any(),
        trusted_user: state.user,
        trusted_system: state.system,
        subject: ca.subject(),
        platform: std::env::consts::OS.to_string(),
    })
}
