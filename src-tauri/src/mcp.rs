//! The MCP server: NovaProxy's captured traffic, exposed to Claude and other AI
//! tooling over the Model Context Protocol.
//!
//! Runs inside the Tauri backend on loopback with the **streamable HTTP**
//! transport, because a GUI app has no stdin to hand an agent. Point a client at
//! it with:
//!
//! ```text
//! claude mcp add --transport http novaproxy http://127.0.0.1:9091/
//! ```
//!
//! Three things shape the tool surface, all of them about not wasting the agent's
//! context window:
//!
//! * **Summaries by default.** `list_flows`/`search_flows` return compact rows;
//!   headers and bodies only come back from `get_flow`/`get_body`, and even then
//!   capped — a single large payload would otherwise eat the whole window. The
//!   on-disk body store makes the full bytes available when explicitly asked for.
//! * **MCP-only filtering.** Every listing tool takes `mcp_only`, matching the
//!   flows [`nova_core::mcp`] recognised as Model Context Protocol traffic. That
//!   is the point of the feature: debugging MCP servers with an MCP client.
//! * **NovaProxy's own traffic is excluded by default.** Calls to *this* endpoint
//!   and requests this server replayed are marked internal, so an agent reading
//!   traffic does not mostly see itself. `include_internal` opts back in.

use std::net::SocketAddr;
use std::sync::Arc;

use nova_core::flow::Side;
use nova_proto::{Flow, Rule, RuleKind};
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{Implementation, ServerCapabilities, ServerInfo};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{tool, tool_handler, tool_router, ErrorData, ServerHandler};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

/// Default port for the MCP endpoint (the proxy itself listens on 9090).
pub const DEFAULT_MCP_PORT: u16 = 9091;

/// Characters of each body returned by `get_flow`. Small on purpose: a flow
/// listing that dumps two 500 KB bodies is useless to an agent.
const FLOW_BODY_CHARS: usize = 2_000;
/// Ceiling on `get_body`, which exists precisely to ask for more.
const MAX_BODY_CHARS: usize = 200_000;
/// Default and maximum number of rows a listing tool returns.
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 500;

/* ------------------------------ tool payloads ------------------------------ */

/// Filters shared by every listing tool.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct Filters {
    /// Only flows recognised as Model Context Protocol traffic (JSON-RPC over
    /// HTTP or SSE). Use this to debug an MCP server.
    #[serde(default)]
    pub mcp_only: bool,
    /// Only flows from this app/process name (substring, case-insensitive), e.g.
    /// "Claude" or "node".
    pub app: Option<String>,
    /// Only flows to this host (substring, case-insensitive).
    pub host: Option<String>,
    /// Only this HTTP method.
    pub method: Option<String>,
    /// Only this response status code.
    pub status: Option<u16>,
    /// Only flows that failed (transport error, aborted, or status >= 400).
    #[serde(default)]
    pub errors_only: bool,
    /// Include NovaProxy's own traffic — calls to this MCP endpoint and requests
    /// it replayed. Excluded by default.
    #[serde(default)]
    pub include_internal: bool,
    /// Maximum rows to return (default 50, max 500).
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    /// Case-insensitive substring to look for in the URL, headers and captured
    /// body previews.
    pub query: String,
    #[serde(flatten)]
    pub filters: Filters,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FlowIdParams {
    /// Flow id, as returned by `list_flows`.
    pub flow_id: String,
    /// Characters of each body to include (default 2000).
    pub body_chars: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BodyParams {
    /// Flow id, as returned by `list_flows`.
    pub flow_id: String,
    /// Which half of the exchange: "request" or "response".
    pub side: String,
    /// Characters to return (default 20000, max 200000).
    pub max_chars: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetRuleParams {
    /// Rule id. Reusing an existing id replaces that rule.
    pub id: String,
    /// One of: MapRemote, MapLocal, Block, Rewrite.
    pub kind: String,
    /// Human-readable name.
    pub name: Option<String>,
    /// URL glob to match, tested against `scheme://host/path`. `*` is a wildcard.
    pub pattern: String,
    /// MapRemote: replacement base URL. MapLocal: absolute file path.
    pub target: Option<String>,
    /// Rewrite: header name to set.
    pub header_name: Option<String>,
    /// Rewrite: header value to set.
    pub header_value: Option<String>,
    /// Whether the rule is active (default true).
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeleteRuleParams {
    /// Id of the rule to remove.
    pub id: String,
}

/// One row of a flow listing — everything needed to decide what to look at next,
/// and nothing else.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct FlowSummary {
    pub id: String,
    pub method: String,
    pub url: String,
    pub host: String,
    pub status: Option<u16>,
    pub state: String,
    /// Originating app/process, when it could be attributed.
    pub app: Option<String>,
    pub request_bytes: u64,
    pub response_bytes: u64,
    pub duration_ms: Option<f64>,
    pub started_at: f64,
    /// `tools/call → search_flows` when this is MCP traffic.
    pub mcp: Option<String>,
    pub error: Option<String>,
    /// True when NovaProxy produced this flow itself.
    pub internal: bool,
}

/// A body, capped, with the numbers needed to know what was left out.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct BodyOut {
    pub media_type: Option<String>,
    /// Full size on the wire, in bytes.
    pub size_bytes: u64,
    /// True when `text` is shorter than the full body.
    pub truncated: bool,
    /// Text body, capped. Absent for binary bodies.
    pub text: Option<String>,
    /// Set for binary bodies instead of `text`, describing what was captured.
    pub binary: Option<String>,
    /// True when the full body is on disk and `get_body` can return more of it.
    pub available_in_full: bool,
}

/// Full detail for one flow.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct FlowDetail {
    #[serde(flatten)]
    pub summary: FlowSummary,
    pub scheme: String,
    pub path: String,
    pub http_version: String,
    pub request_headers: Vec<(String, String)>,
    pub response_headers: Vec<(String, String)>,
    pub request_body: Option<BodyOut>,
    pub response_body: Option<BodyOut>,
    /// Measured phases in milliseconds; absent phases were not measured.
    pub timings: TimingsOut,
    /// Set when this is MCP traffic: method, tool and JSON-RPC id.
    pub mcp_detail: Option<McpDetail>,
    pub tunneled: bool,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct McpDetail {
    pub method: Option<String>,
    pub tool: Option<String>,
    pub jsonrpc_id: Option<String>,
    pub transport: String,
}

/// Measured timing phases, mirrored from [`nova_proto::Timings`] so the shared
/// wire crate does not need a JSON-Schema dependency. `None` means the phase was
/// not measured (see `nova_core::timing`) — never a fabricated zero.
#[derive(Debug, Default, Serialize, schemars::JsonSchema)]
pub struct TimingsOut {
    pub dns_ms: Option<f64>,
    pub connect_ms: Option<f64>,
    pub tls_ms: Option<f64>,
    /// The request reused an open connection, so no setup cost is attributable.
    pub connection_reused: bool,
    pub request_ms: Option<f64>,
    pub ttfb_ms: Option<f64>,
    pub download_ms: Option<f64>,
}

impl From<nova_proto::Timings> for TimingsOut {
    fn from(t: nova_proto::Timings) -> Self {
        Self {
            dns_ms: t.dns_ms,
            connect_ms: t.connect_ms,
            tls_ms: t.tls_ms,
            connection_reused: t.connection_reused,
            request_ms: t.request_ms,
            ttfb_ms: t.ttfb_ms,
            download_ms: t.download_ms,
        }
    }
}

/// A traffic-control rule as returned to an agent.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct RuleOut {
    pub id: String,
    pub enabled: bool,
    pub kind: String,
    pub name: String,
    pub pattern: String,
    pub target: Option<String>,
    pub header_name: Option<String>,
    pub header_value: Option<String>,
}

impl From<&Rule> for RuleOut {
    fn from(r: &Rule) -> Self {
        Self {
            id: r.id.clone(),
            enabled: r.enabled,
            kind: format!("{:?}", r.kind),
            name: r.name.clone(),
            pattern: r.pattern.clone(),
            target: r.target.clone(),
            header_name: r.header_name.clone(),
            header_value: r.header_value.clone(),
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct StatusOut {
    pub proxy_running: bool,
    pub proxy_address: Option<String>,
    pub system_proxy: bool,
    pub flows_captured_this_session: u64,
    pub flows_retained: u64,
    pub retention_window: u64,
    pub ca_trusted: bool,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ActionOut {
    pub ok: bool,
    pub detail: String,
}

/* -------------------------------- the server -------------------------------- */

#[derive(Clone)]
pub struct NovaMcp {
    state: Arc<AppState>,
}

#[tool_router]
impl NovaMcp {
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// List captured HTTP flows, newest first. Returns compact summaries; use
    /// get_flow for headers and bodies. Pass mcp_only=true to see only Model
    /// Context Protocol traffic.
    #[tool(description = "List captured HTTP flows, newest first (compact summaries). \
Filters: mcp_only (MCP/JSON-RPC traffic only), app, host, method, status, errors_only, \
include_internal (NovaProxy's own traffic, excluded by default), limit.")]
    pub fn list_flows(&self, Parameters(filters): Parameters<Filters>) -> Json<Vec<FlowSummary>> {
        let limit = clamp_limit(filters.limit);
        let flows = self
            .state
            .flows
            .find(|f| matches_filters(f, &filters), limit);
        Json(flows.iter().map(summarize).collect())
    }

    /// Search captured flows by substring across URL, headers and body previews.
    #[tool(description = "Search captured flows for a case-insensitive substring in the URL, \
headers or captured body previews. Takes the same filters as list_flows.")]
    pub fn search_flows(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Json<Vec<FlowSummary>> {
        let needle = params.query.to_lowercase();
        let limit = clamp_limit(params.filters.limit);
        let flows = self.state.flows.find(
            |f| matches_filters(f, &params.filters) && matches_query(f, &needle),
            limit,
        );
        Json(flows.iter().map(summarize).collect())
    }

    /// Full detail for one flow: headers, capped bodies, measured timings.
    #[tool(description = "Get one flow in full: headers, measured timings, and both bodies \
capped to body_chars characters each (default 2000). Use get_body for more of a body.")]
    pub fn get_flow(
        &self,
        Parameters(params): Parameters<FlowIdParams>,
    ) -> Result<Json<FlowDetail>, ErrorData> {
        let flow = self
            .state
            .flows
            .get(&params.flow_id)
            .ok_or_else(|| unknown_flow(&params.flow_id))?;
        let cap = params.body_chars.unwrap_or(FLOW_BODY_CHARS).min(MAX_BODY_CHARS);
        Ok(Json(detail(&flow, cap)))
    }

    /// Read one side's body, pulling the full bytes from the body store when the
    /// preview was truncated.
    #[tool(description = "Read a flow's request or response body, up to max_chars characters \
(default 20000, max 200000). Large bodies are stored on disk and fetched on demand.")]
    pub fn get_body(
        &self,
        Parameters(params): Parameters<BodyParams>,
    ) -> Result<Json<BodyOut>, ErrorData> {
        let side = match params.side.to_lowercase().as_str() {
            "request" | "req" => Side::Request,
            "response" | "res" => Side::Response,
            other => {
                return Err(ErrorData::invalid_params(
                    format!("side must be \"request\" or \"response\", got {other:?}"),
                    None,
                ))
            }
        };
        let flow = self
            .state
            .flows
            .get(&params.flow_id)
            .ok_or_else(|| unknown_flow(&params.flow_id))?;
        let preview = match side {
            Side::Request => flow.request_body.clone(),
            Side::Response => flow.response_body.clone(),
        };
        let Some(preview) = preview else {
            return Err(ErrorData::invalid_params(
                format!("flow {} has no {} body", params.flow_id, params.side),
                None,
            ));
        };
        let cap = params.max_chars.unwrap_or(20_000).min(MAX_BODY_CHARS);

        // The preview is capped in memory; when the full body was spilled, read
        // it back so a request for more than the preview can be honoured.
        if preview.spilled {
            if let Ok((bytes, total, _)) =
                self.state
                    .bodies
                    .read(&params.flow_id, side, (cap as u64).saturating_mul(4))
            {
                let full = nova_core::flow::build_preview(
                    bytes,
                    total,
                    true,
                    preview.media_type.clone(),
                    preview.decoded_from.clone(),
                );
                return Ok(Json(body_out(&full, cap, true)));
            }
        }
        Ok(Json(body_out(&preview, cap, preview.spilled)))
    }

    /// Re-issue a captured request through the proxy.
    #[tool(description = "Replay a captured request through the proxy. The replay is captured \
as a new flow, marked as NovaProxy's own traffic so it does not pollute unfiltered listings.")]
    pub async fn replay_request(
        &self,
        Parameters(params): Parameters<FlowIdParams>,
    ) -> Result<Json<ActionOut>, ErrorData> {
        let flow = self
            .state
            .flows
            .get(&params.flow_id)
            .ok_or_else(|| unknown_flow(&params.flow_id))?;
        crate::commands::replay(&self.state, flow, true)
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        Ok(Json(ActionOut {
            ok: true,
            detail: format!("replayed {} through the proxy", params.flow_id),
        }))
    }

    /// The current traffic-control rules.
    #[tool(description = "List the traffic-control rules (Map Remote, Map Local, Block, Rewrite).")]
    pub fn list_rules(&self) -> Json<Vec<RuleOut>> {
        Json(self.state.rules.read().unwrap().iter().map(RuleOut::from).collect())
    }

    /// Add or replace a traffic-control rule.
    #[tool(description = "Add or replace a traffic-control rule. kind is one of MapRemote \
(redirect to target base URL), MapLocal (serve target file), Block (403), Rewrite (set a \
request header). Reusing an id replaces that rule.")]
    pub fn set_rule(
        &self,
        Parameters(params): Parameters<SetRuleParams>,
    ) -> Result<Json<ActionOut>, ErrorData> {
        let kind = parse_rule_kind(&params.kind)?;
        let rule = Rule {
            id: params.id.clone(),
            enabled: params.enabled.unwrap_or(true),
            kind,
            name: params.name.unwrap_or_else(|| params.id.clone()),
            pattern: params.pattern,
            target: params.target,
            header_name: params.header_name,
            header_value: params.header_value,
        };
        validate_rule(&rule)?;

        let next = {
            let mut rules = self.state.rules.write().unwrap();
            match rules.iter_mut().find(|r| r.id == rule.id) {
                Some(existing) => *existing = rule.clone(),
                None => rules.push(rule.clone()),
            }
            rules.clone()
        };
        self.state
            .persist_rules(&next)
            .map_err(|e| ErrorData::internal_error(e, None))?;
        Ok(Json(ActionOut {
            ok: true,
            detail: format!("rule {} saved ({} total)", rule.id, next.len()),
        }))
    }

    /// Remove a traffic-control rule.
    #[tool(description = "Delete a traffic-control rule by id.")]
    pub fn delete_rule(
        &self,
        Parameters(params): Parameters<DeleteRuleParams>,
    ) -> Result<Json<ActionOut>, ErrorData> {
        let (next, removed) = {
            let mut rules = self.state.rules.write().unwrap();
            let before = rules.len();
            rules.retain(|r| r.id != params.id);
            (rules.clone(), before != rules.len())
        };
        if !removed {
            return Err(ErrorData::invalid_params(
                format!("no rule with id {}", params.id),
                None,
            ));
        }
        self.state
            .persist_rules(&next)
            .map_err(|e| ErrorData::internal_error(e, None))?;
        Ok(Json(ActionOut {
            ok: true,
            detail: format!("rule {} deleted", params.id),
        }))
    }

    /// Proxy and capture status.
    #[tool(description = "Proxy state: whether it is running and where, whether the system \
proxy points at it, how many flows were captured and how many are still retained.")]
    pub fn proxy_status(&self) -> Json<StatusOut> {
        // Read the engine, release it, then read the CA — never hold two of
        // AppState's locks at once, so no ordering can deadlock.
        let engine_addr = self
            .state
            .engine
            .lock()
            .unwrap()
            .as_ref()
            .map(|h| h.addr.to_string());
        let ca_trusted = self
            .state
            .ca
            .lock()
            .unwrap()
            .as_ref()
            .map(|ca| nova_core::trust::is_trusted(&nova_core::trust::CaId::of(ca)))
            .unwrap_or(false);
        Json(StatusOut {
            proxy_running: engine_addr.is_some(),
            proxy_address: engine_addr,
            system_proxy: *self.state.system_proxy.lock().unwrap(),
            flows_captured_this_session: self.state.flows.total_captured(),
            flows_retained: self.state.flows.retained() as u64,
            retention_window: self.state.flows.max_flows() as u64,
            ca_trusted,
        })
    }
}

#[tool_handler]
impl ServerHandler for NovaMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("novaproxy", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "NovaProxy is an HTTP/HTTPS debugging proxy. These tools expose the traffic it \
has captured on this machine.\n\n\
Start with list_flows (newest first, compact rows), then get_flow for headers and timings, \
then get_body when a body is truncated.\n\n\
To debug an MCP server, pass mcp_only=true: only JSON-RPC-over-HTTP/SSE exchanges are \
returned, labelled with their method and tool. MCP over stdio never reaches the network and \
therefore cannot be captured.\n\n\
Your own calls to this endpoint are marked internal and excluded unless you pass \
include_internal=true. Bodies are capped in every response; ask for more with get_body.",
            )
    }
}

/* --------------------------------- filtering --------------------------------- */

fn clamp_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

/// Does `flow` pass every requested filter?
pub fn matches_filters(flow: &Flow, f: &Filters) -> bool {
    if flow.internal && !f.include_internal {
        return false;
    }
    if f.mcp_only && flow.mcp.is_none() {
        return false;
    }
    if let Some(app) = &f.app {
        let hit = flow
            .process
            .as_deref()
            .map(|p| p.to_lowercase().contains(&app.to_lowercase()))
            .unwrap_or(false);
        if !hit {
            return false;
        }
    }
    if let Some(host) = &f.host {
        if !flow.host.to_lowercase().contains(&host.to_lowercase()) {
            return false;
        }
    }
    if let Some(method) = &f.method {
        if !flow.method.eq_ignore_ascii_case(method) {
            return false;
        }
    }
    if let Some(status) = f.status {
        if flow.status != Some(status) {
            return false;
        }
    }
    if f.errors_only {
        let failed = flow.error.is_some()
            || flow.state == nova_proto::FlowState::Error
            || flow.status.map(|s| s >= 400).unwrap_or(false);
        if !failed {
            return false;
        }
    }
    true
}

/// Substring search over the parts of a flow a human would grep.
pub fn matches_query(flow: &Flow, needle_lowercase: &str) -> bool {
    if needle_lowercase.is_empty() {
        return true;
    }
    let hit = |s: &str| s.to_lowercase().contains(needle_lowercase);
    if hit(&flow.url) || hit(&flow.method) {
        return true;
    }
    if flow
        .request_headers
        .iter()
        .chain(flow.response_headers.iter())
        .any(|h| hit(&h.name) || hit(&h.value))
    {
        return true;
    }
    [&flow.request_body, &flow.response_body]
        .into_iter()
        .flatten()
        .any(|b| b.text.as_deref().map(hit).unwrap_or(false))
}

/* -------------------------------- projection -------------------------------- */

fn summarize(flow: &Flow) -> FlowSummary {
    FlowSummary {
        id: flow.id.clone(),
        method: flow.method.clone(),
        url: flow.url.clone(),
        host: flow.host.clone(),
        status: flow.status,
        state: format!("{:?}", flow.state),
        app: flow.process.clone(),
        request_bytes: flow.request_size,
        response_bytes: flow.response_size,
        duration_ms: flow.duration_ms,
        started_at: flow.started_at,
        mcp: flow.mcp.as_ref().map(nova_core::mcp::summary),
        error: flow.error.clone(),
        internal: flow.internal,
    }
}

fn detail(flow: &Flow, body_chars: usize) -> FlowDetail {
    let headers = |hs: &[nova_proto::Header]| {
        hs.iter()
            .map(|h| (h.name.clone(), h.value.clone()))
            .collect::<Vec<_>>()
    };
    FlowDetail {
        summary: summarize(flow),
        scheme: flow.scheme.clone(),
        path: flow.path.clone(),
        http_version: flow.http_version.clone(),
        request_headers: headers(&flow.request_headers),
        response_headers: headers(&flow.response_headers),
        request_body: flow
            .request_body
            .as_ref()
            .map(|b| body_out(b, body_chars, b.spilled)),
        response_body: flow
            .response_body
            .as_ref()
            .map(|b| body_out(b, body_chars, b.spilled)),
        timings: flow.timings.into(),
        mcp_detail: flow.mcp.as_ref().map(|m| McpDetail {
            method: m.method.clone(),
            tool: m.tool.clone(),
            jsonrpc_id: m.id.clone(),
            transport: format!("{:?}", m.transport),
        }),
        tunneled: flow.tunneled,
    }
}

/// Project a captured body into a capped tool response.
pub fn body_out(body: &nova_proto::BodyPreview, cap: usize, available_in_full: bool) -> BodyOut {
    let mut truncated = body.truncated;
    let text = body.text.as_ref().map(|t| {
        if t.chars().count() > cap {
            truncated = true;
            t.chars().take(cap).collect::<String>()
        } else {
            t.clone()
        }
    });
    BodyOut {
        media_type: body.media_type.clone(),
        size_bytes: body.size,
        truncated,
        text,
        binary: body.base64.as_ref().map(|_| {
            format!(
                "binary body, {} bytes ({})",
                body.size,
                body.media_type.as_deref().unwrap_or("unknown type")
            )
        }),
        available_in_full,
    }
}

fn parse_rule_kind(kind: &str) -> Result<RuleKind, ErrorData> {
    match kind.to_lowercase().replace(['_', '-', ' '], "").as_str() {
        "mapremote" => Ok(RuleKind::MapRemote),
        "maplocal" => Ok(RuleKind::MapLocal),
        "block" => Ok(RuleKind::Block),
        "rewrite" => Ok(RuleKind::Rewrite),
        _ => Err(ErrorData::invalid_params(
            format!("unknown rule kind {kind:?}; expected MapRemote, MapLocal, Block or Rewrite"),
            None,
        )),
    }
}

/// Reject rules that would silently do nothing — a rule the agent believes it
/// installed but which cannot act is worse than an error.
fn validate_rule(rule: &Rule) -> Result<(), ErrorData> {
    let missing = |what: &str| {
        Err(ErrorData::invalid_params(
            format!("{:?} rules need {what}", rule.kind),
            None,
        ))
    };
    match rule.kind {
        RuleKind::MapRemote | RuleKind::MapLocal
            if rule.target.as_deref().unwrap_or("").is_empty() =>
        {
            missing("a target")
        }
        RuleKind::Rewrite if rule.header_name.as_deref().unwrap_or("").is_empty() => {
            missing("a header_name")
        }
        _ if rule.pattern.is_empty() => missing("a pattern"),
        _ => Ok(()),
    }
}

fn unknown_flow(id: &str) -> ErrorData {
    ErrorData::invalid_params(
        format!("no flow with id {id}; call list_flows for current ids (older flows are evicted)"),
        None,
    )
}

/* --------------------------------- transport --------------------------------- */

/// A running MCP endpoint. Dropping it (or calling [`McpHandle::stop`]) shuts the
/// listener down.
pub struct McpHandle {
    pub addr: SocketAddr,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl McpHandle {
    pub fn stop(mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for McpHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
    }
}

/// Bind the MCP endpoint on loopback and serve it until stopped.
///
/// Loopback only, deliberately: this endpoint can read every captured request
/// and replay traffic, so it must not be reachable from the network. rmcp also
/// validates the inbound `Host` header against loopback names by default, which
/// blocks DNS-rebinding attacks from a browser page.
pub async fn start(state: Arc<AppState>, port: u16) -> Result<McpHandle, String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("cannot bind MCP endpoint on {addr}: {e}"))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;

    let service = StreamableHttpService::new(
        move || Ok(NovaMcp::new(state.clone())),
        Arc::new(LocalSessionManager::default()),
        // Stateless with plain JSON replies: this server never initiates messages,
        // so sessions and SSE framing would add bookkeeping and parsing work for
        // every client without buying anything. Host validation stays on (rmcp
        // defaults to loopback only), which is what blocks a browser page from
        // driving this endpoint via DNS rebinding.
        StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true),
    );
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();

    tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                _ = &mut stop_rx => break,
                accepted = listener.accept() => accepted,
            };
            let Ok((stream, _peer)) = accepted else { continue };
            let svc = service.clone();
            tokio::spawn(async move {
                let io = hyper_util::rt::TokioIo::new(stream);
                let handler = hyper::service::service_fn(
                    move |req: hyper::Request<hyper::body::Incoming>| {
                        let mut svc = svc.clone();
                        async move { tower_service::Service::call(&mut svc, req).await }
                    },
                );
                if let Err(e) = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, handler)
                    .await
                {
                    tracing::debug!("MCP connection ended: {e}");
                }
            });
        }
        tracing::info!("MCP endpoint stopped");
    });

    tracing::info!("MCP endpoint listening on http://{addr}/");
    Ok(McpHandle {
        addr,
        stop: Some(stop_tx),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nova_proto::{BodyPreview, FlowState, Header, McpInfo, McpTransport};

    fn flow(id: &str) -> Flow {
        nova_core::flow::new_flow(
            id.into(),
            0,
            "GET".into(),
            "https".into(),
            "api.example.com".into(),
            "/v1/ping".into(),
            "https://api.example.com/v1/ping".into(),
            "127.0.0.1:1".into(),
            "HTTP/1.1".into(),
            vec![Header { name: "accept".into(), value: "application/json".into() }],
        )
    }

    fn mcp_flow(id: &str) -> Flow {
        let mut f = flow(id);
        f.mcp = Some(McpInfo {
            method: Some("tools/call".into()),
            tool: Some("search_flows".into()),
            id: Some("1".into()),
            transport: McpTransport::Http,
        });
        f
    }

    fn text_body(text: &str) -> BodyPreview {
        BodyPreview {
            size: text.len() as u64,
            truncated: false,
            media_type: Some("application/json".into()),
            decoded_from: None,
            text: Some(text.into()),
            base64: None,
            spilled: false,
        }
    }

    #[test]
    fn internal_flows_are_hidden_unless_asked_for() {
        let mut f = flow("f0");
        f.internal = true;
        assert!(
            !matches_filters(&f, &Filters::default()),
            "an agent must not see its own traffic by default"
        );
        assert!(matches_filters(
            &f,
            &Filters { include_internal: true, ..Default::default() }
        ));
    }

    #[test]
    fn mcp_only_keeps_just_the_tagged_flows() {
        let filters = Filters { mcp_only: true, ..Default::default() };
        assert!(matches_filters(&mcp_flow("f1"), &filters));
        assert!(!matches_filters(&flow("f0"), &filters));
    }

    #[test]
    fn mcp_only_still_excludes_our_own_mcp_traffic() {
        // The interesting case: debugging someone else's MCP server while the
        // agent's own calls are also MCP traffic.
        let mut ours = mcp_flow("f2");
        ours.internal = true;
        assert!(!matches_filters(&ours, &Filters { mcp_only: true, ..Default::default() }));
        assert!(matches_filters(
            &ours,
            &Filters { mcp_only: true, include_internal: true, ..Default::default() }
        ));
    }

    #[test]
    fn app_host_and_method_filters_are_case_insensitive_substrings() {
        let mut f = flow("f0");
        f.process = Some("Claude Code".into());
        assert!(matches_filters(&f, &Filters { app: Some("claude".into()), ..Default::default() }));
        assert!(!matches_filters(&f, &Filters { app: Some("chrome".into()), ..Default::default() }));
        assert!(matches_filters(&f, &Filters { host: Some("EXAMPLE".into()), ..Default::default() }));
        assert!(matches_filters(&f, &Filters { method: Some("get".into()), ..Default::default() }));
        assert!(!matches_filters(&f, &Filters { method: Some("post".into()), ..Default::default() }));
    }

    #[test]
    fn app_filter_excludes_unattributed_flows() {
        // `process` is best-effort; a filter for one app must not sweep in flows
        // whose origin was never resolved.
        assert!(!matches_filters(
            &flow("f0"),
            &Filters { app: Some("claude".into()), ..Default::default() }
        ));
    }

    #[test]
    fn errors_only_covers_transport_errors_and_4xx_5xx() {
        let mut ok = flow("f0");
        ok.status = Some(200);
        assert!(!matches_filters(&ok, &Filters { errors_only: true, ..Default::default() }));

        let mut http_error = flow("f1");
        http_error.status = Some(404);
        assert!(matches_filters(&http_error, &Filters { errors_only: true, ..Default::default() }));

        let mut transport_error = flow("f2");
        transport_error.state = FlowState::Error;
        transport_error.error = Some("connection refused".into());
        assert!(matches_filters(
            &transport_error,
            &Filters { errors_only: true, ..Default::default() }
        ));
    }

    #[test]
    fn status_filter_is_exact() {
        let mut f = flow("f0");
        f.status = Some(500);
        assert!(matches_filters(&f, &Filters { status: Some(500), ..Default::default() }));
        assert!(!matches_filters(&f, &Filters { status: Some(200), ..Default::default() }));
    }

    #[test]
    fn query_searches_url_headers_and_bodies() {
        let mut f = flow("f0");
        f.request_body = Some(text_body(r#"{"query":"needle in the body"}"#));
        assert!(matches_query(&f, "v1/ping"), "url");
        assert!(matches_query(&f, "application/json"), "header value");
        assert!(matches_query(&f, "needle"), "body preview");
        assert!(!matches_query(&f, "absent"));
        assert!(matches_query(&f, ""), "an empty query matches everything");
    }

    #[test]
    fn body_out_caps_text_and_says_so() {
        let long = "x".repeat(100);
        let out = body_out(&text_body(&long), 10, false);
        assert_eq!(out.text.as_deref().unwrap().len(), 10);
        assert!(out.truncated, "the agent must know it did not get the whole body");
        assert_eq!(out.size_bytes, 100, "the true size is still reported");
        assert!(!out.available_in_full);
    }

    #[test]
    fn body_out_keeps_short_bodies_whole() {
        let out = body_out(&text_body("hi"), 100, false);
        assert_eq!(out.text.as_deref(), Some("hi"));
        assert!(!out.truncated);
    }

    #[test]
    fn body_out_never_splits_a_multibyte_character() {
        // Capping by chars, not bytes: a cap that lands mid-character would
        // otherwise produce invalid text.
        let out = body_out(&text_body("héllo wörld"), 4, false);
        assert_eq!(out.text.as_deref(), Some("héll"));
    }

    #[test]
    fn body_out_describes_binary_instead_of_dumping_base64() {
        let mut b = text_body("");
        b.text = None;
        b.base64 = Some("AAAA".into());
        b.media_type = Some("image/png".into());
        b.size = 4096;
        let out = body_out(&b, 100, false);
        assert!(out.text.is_none(), "base64 in an agent's context is waste");
        assert!(out.binary.unwrap().contains("4096 bytes"));
    }

    #[test]
    fn body_out_reports_when_more_is_available_on_disk() {
        let mut b = text_body("first page");
        b.spilled = true;
        b.truncated = true;
        let out = body_out(&b, 1_000, true);
        assert!(out.available_in_full, "get_body can return the rest");
        assert!(out.truncated);
    }

    #[test]
    fn summaries_label_mcp_traffic_readably() {
        let s = summarize(&mcp_flow("f1"));
        assert_eq!(s.mcp.as_deref(), Some("tools/call → search_flows"));
        let plain = summarize(&flow("f0"));
        assert!(plain.mcp.is_none());
    }

    #[test]
    fn detail_caps_both_bodies() {
        let mut f = flow("f0");
        f.request_body = Some(text_body(&"a".repeat(50)));
        f.response_body = Some(text_body(&"b".repeat(50)));
        let d = detail(&f, 5);
        assert_eq!(d.request_body.unwrap().text.unwrap().len(), 5);
        assert_eq!(d.response_body.unwrap().text.unwrap().len(), 5);
    }

    #[test]
    fn limits_are_clamped_to_a_sane_window() {
        assert_eq!(clamp_limit(None), DEFAULT_LIMIT);
        assert_eq!(clamp_limit(Some(0)), 1, "zero would return nothing at all");
        assert_eq!(clamp_limit(Some(10_000)), MAX_LIMIT);
        assert_eq!(clamp_limit(Some(7)), 7);
    }

    #[test]
    fn rule_kinds_parse_forgivingly() {
        for (input, expected) in [
            ("MapRemote", RuleKind::MapRemote),
            ("map_remote", RuleKind::MapRemote),
            ("map local", RuleKind::MapLocal),
            ("BLOCK", RuleKind::Block),
            ("rewrite", RuleKind::Rewrite),
        ] {
            assert_eq!(parse_rule_kind(input).unwrap(), expected, "{input}");
        }
        assert!(parse_rule_kind("mapsideways").is_err());
    }

    #[test]
    fn rules_that_could_not_act_are_rejected() {
        let base = Rule {
            id: "r1".into(),
            enabled: true,
            kind: RuleKind::MapRemote,
            name: "r1".into(),
            pattern: "https://api.example.com/*".into(),
            target: None,
            header_name: None,
            header_value: None,
        };
        assert!(validate_rule(&base).is_err(), "MapRemote without a target");
        assert!(validate_rule(&Rule { target: Some("https://staging".into()), ..base.clone() }).is_ok());
        assert!(
            validate_rule(&Rule { kind: RuleKind::Rewrite, ..base.clone() }).is_err(),
            "Rewrite without a header name"
        );
        assert!(
            validate_rule(&Rule { kind: RuleKind::Block, pattern: String::new(), ..base.clone() })
                .is_err(),
            "no pattern at all"
        );
        assert!(validate_rule(&Rule { kind: RuleKind::Block, ..base }).is_ok());
    }
}
