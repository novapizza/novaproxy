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
use nova_proto::{Fault, FaultMode, Flow, Rule, RuleKind, FAULT_MAX_DELAY_MS};
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

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct SetRuleParams {
    /// Rule id. Reusing an existing id replaces that rule.
    pub id: String,
    /// One of: MapRemote, MapLocal, Block, Rewrite, Fault.
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
    /// Also match the HTTP method (GET, POST…); absent = any method.
    pub method: Option<String>,
    /// Scenario name; rules sharing it are switched and counted together.
    pub group: Option<String>,
    /// Act on at most this many matching requests (1 = first only).
    pub times: Option<u32>,
    /// Fault rules: the failure to inject.
    pub fault: Option<FaultParams>,
}

/// The failure a Fault rule injects.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FaultParams {
    /// DropBefore (close, never forwarded), HoldClose (hold delay_ms, close),
    /// HoldStatus (hold delay_ms, answer status, default 504), FakeResponse
    /// (answer status/body/content_type), Waf403 (HTML 403), ProxyAuth407
    /// (407; for HTTPS the CONNECT is refused), PassCut (forward, then drop the
    /// response: the server acts, the client sees a network error), CutBody
    /// (forward, pass the head and cut_after_bytes, then close), Delay (forward,
    /// hold the response delay_ms).
    pub mode: String,
    /// Hold time in ms (HoldClose, HoldStatus, Delay). At most 600000.
    pub delay_ms: Option<u32>,
    /// Status to answer (HoldStatus, FakeResponse).
    pub status: Option<u16>,
    /// Body to answer (HoldStatus, FakeResponse).
    pub body: Option<String>,
    /// Content-Type of body.
    pub content_type: Option<String>,
    /// CutBody: body bytes passed before the connection closes.
    pub cut_after_bytes: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ToggleGroupParams {
    /// Scenario (group) name, exactly as on the rules.
    pub group: String,
    /// Switch every rule of the group on (true) or off (false).
    pub enabled: bool,
    /// When switching on: switch every rule of every other group off, so one
    /// scenario runs at a time (default false).
    pub exclusive: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct HitsParams {
    /// Only this rule.
    pub rule_id: Option<String>,
    /// Only the rules of this group. Neither = every rule.
    pub group: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ImportRulesParams {
    /// Rules to add; a rule whose id exists replaces it.
    pub rules: Vec<SetRuleParams>,
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
    pub method: Option<String>,
    pub group: Option<String>,
    pub times: Option<u32>,
    pub fault: Option<FaultParams>,
}

impl From<&Rule> for RuleOut {
    fn from(r: &Rule) -> Self {
        Self {
            method: r.method.clone(),
            group: r.group.clone(),
            times: r.times,
            fault: r.fault.as_ref().map(|f| FaultParams {
                mode: format!("{:?}", f.mode),
                delay_ms: f.delay_ms,
                status: f.status,
                body: f.body.clone(),
                content_type: f.content_type.clone(),
                cut_after_bytes: f.cut_after_bytes,
            }),
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

/// One rule's action count.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct HitOut {
    pub rule_id: String,
    pub name: String,
    pub group: Option<String>,
    pub enabled: bool,
    pub hits: u32,
    /// The rule's `times` limit, if any; hits == times means it is used up.
    pub times: Option<u32>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct FaultSettingsOut {
    /// Host globs Fault rules may act on. Empty = Fault rules do nothing.
    /// Only the user can change this, in NovaProxy's Rules page.
    pub allow_hosts: Vec<String>,
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

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct InsightParams {
    /// Only insights about this app/process (substring, case-insensitive).
    pub app: Option<String>,
    /// Only insights about this host (substring, case-insensitive).
    pub host: Option<String>,
    /// Lowest severity to include: "info" (default), "warning" or "high".
    pub min_severity: Option<String>,
}

/// One anti-pattern, as returned to an agent. Mirrors [`nova_proto::Insight`]
/// with the flow list cut short: the ids are a starting point for `get_flow`.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct InsightOut {
    pub id: String,
    pub rule: String,
    /// "info", "warning" or "high".
    pub severity: String,
    pub title: String,
    pub detail: String,
    pub advice: String,
    pub app: Option<String>,
    pub host: String,
    /// How many flows show the problem.
    pub flow_count: u32,
    /// The newest of them, at most 20; pass one to get_flow for the evidence.
    pub example_flow_ids: Vec<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct InsightsOut {
    /// Flows the checks looked at (in-flight, tunneled, WebSocket and internal
    /// flows are left out).
    pub flows_analyzed: u32,
    /// Most severe first.
    pub insights: Vec<InsightOut>,
}

/// Example flow ids per insight returned to an agent.
const INSIGHT_EXAMPLES: usize = 20;

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct SummaryParams {
    /// Only flows from this app/process (substring, case-insensitive).
    pub app: Option<String>,
    /// Only flows to this host (substring, case-insensitive).
    pub host: Option<String>,
    /// Include NovaProxy's own traffic. Excluded by default.
    #[serde(default)]
    pub include_internal: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DiffParams {
    /// The flow to compare from (often one that worked).
    pub a: String,
    /// The flow to compare with it (often one that failed).
    pub b: String,
    /// Also compare headers that differ on every exchange (Date, request ids,
    /// trace headers). Off by default.
    #[serde(default)]
    pub include_noisy: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ExplainParams {
    /// Flow id, as returned by `list_flows`.
    pub flow_id: String,
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

impl NovaMcp {
    /// Whether credentials are hidden from the client (the default).
    fn redacting(&self) -> bool {
        self.state.mcp_redact.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// A flow as the client may see it.
    fn shown(&self, flow: Flow) -> Flow {
        if self.redacting() {
            nova_core::redact::flow(&flow)
        } else {
            flow
        }
    }

    fn shown_body(&self, body: nova_proto::BodyPreview) -> nova_proto::BodyPreview {
        if self.redacting() {
            nova_core::redact::body(&body)
        } else {
            body
        }
    }

    /// A listing row; only the URL can carry a secret there.
    fn summary(&self, flow: &Flow) -> FlowSummary {
        let mut row = summarize(flow);
        if self.redacting() {
            row.url = nova_core::redact::url(&row.url);
        }
        row
    }

    /// A warning for the agent when Fault rules it saved cannot act yet.
    fn fault_note(&self, rules: &[Rule]) -> String {
        if !rules.iter().any(|r| r.kind == RuleKind::Fault) {
            return String::new();
        }
        if self.state.fault.read().unwrap().allow_hosts.iter().all(|h| h.trim().is_empty()) {
            return "; note: no host is allowed for Fault rules yet, so they do nothing until the user adds \
the test hosts in NovaProxy (Rules page, Fault hosts)"
                .into();
        }
        String::new()
    }
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
        let flows = self.state.flows.find(|f| matches_filters(f, &filters), limit);
        Json(flows.iter().map(|f| self.summary(f)).collect())
    }

    /// Search captured flows by substring across URL, headers and body previews.
    #[tool(description = "Search captured flows for a case-insensitive substring in the URL, \
headers or captured body previews. Takes the same filters as list_flows.")]
    pub fn search_flows(&self, Parameters(params): Parameters<SearchParams>) -> Json<Vec<FlowSummary>> {
        let needle = params.query.to_lowercase();
        let limit = clamp_limit(params.filters.limit);
        let flows = self.state.flows.find(|f| matches_filters(f, &params.filters) && matches_query(f, &needle), limit);
        Json(flows.iter().map(|f| self.summary(f)).collect())
    }

    /// Full detail for one flow: headers, capped bodies, measured timings.
    #[tool(description = "Get one flow in full: headers, measured timings, and both bodies \
capped to body_chars characters each (default 2000). Use get_body for more of a body.")]
    pub fn get_flow(&self, Parameters(params): Parameters<FlowIdParams>) -> Result<Json<FlowDetail>, ErrorData> {
        let flow = self.state.flows.get(&params.flow_id).ok_or_else(|| unknown_flow(&params.flow_id))?;
        let cap = params.body_chars.unwrap_or(FLOW_BODY_CHARS).min(MAX_BODY_CHARS);
        Ok(Json(detail(&self.shown(flow), cap)))
    }

    /// Read one side's body, pulling the full bytes from the body store when the
    /// preview was truncated.
    #[tool(description = "Read a flow's request or response body, up to max_chars characters \
(default 20000, max 200000). Large bodies are stored on disk and fetched on demand.")]
    pub fn get_body(&self, Parameters(params): Parameters<BodyParams>) -> Result<Json<BodyOut>, ErrorData> {
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
        let flow = self.state.flows.get(&params.flow_id).ok_or_else(|| unknown_flow(&params.flow_id))?;
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
            if let Ok((bytes, total, _)) = self.state.bodies.read(&params.flow_id, side, (cap as u64).saturating_mul(4))
            {
                let full = nova_core::flow::build_preview(
                    bytes,
                    total,
                    true,
                    preview.media_type.clone(),
                    preview.decoded_from.clone(),
                );
                return Ok(Json(body_out(&self.shown_body(full), cap, true)));
            }
        }
        Ok(Json(body_out(&self.shown_body(preview.clone()), cap, preview.spilled)))
    }

    /// Re-issue a captured request through the proxy.
    #[tool(description = "Replay a captured request through the proxy. The replay is captured \
as a new flow, marked as NovaProxy's own traffic so it does not pollute unfiltered listings.")]
    pub async fn replay_request(
        &self,
        Parameters(params): Parameters<FlowIdParams>,
    ) -> Result<Json<ActionOut>, ErrorData> {
        let flow = self.state.flows.get(&params.flow_id).ok_or_else(|| unknown_flow(&params.flow_id))?;
        crate::commands::replay(&self.state, flow, true).await.map_err(|e| ErrorData::internal_error(e, None))?;
        Ok(Json(ActionOut { ok: true, detail: format!("replayed {} through the proxy", params.flow_id) }))
    }

    /// The current traffic-control rules.
    #[tool(description = "List the traffic-control rules (Map Remote, Map Local, Block, Rewrite, Fault), \
with their method, group (scenario) and times limit.")]
    pub fn list_rules(&self) -> Json<Vec<RuleOut>> {
        Json(self.state.rules.read().unwrap().iter().map(RuleOut::from).collect())
    }

    /// Add or replace a traffic-control rule.
    #[tool(description = "Add or replace a traffic-control rule. kind is one of MapRemote \
(redirect to target base URL), MapLocal (serve target file), Block (403), Rewrite (set a \
request header), Fault (inject a failure described by `fault`, to test how an app handles \
timeouts and network errors). Optional: method, group (scenario name), times (1 = first \
matching request only). Fault rules act only on hosts the user allowed in NovaProxy \
(see get_fault_settings). Reusing an id replaces that rule.")]
    pub fn set_rule(&self, Parameters(params): Parameters<SetRuleParams>) -> Result<Json<ActionOut>, ErrorData> {
        let rule = rule_from_params(params)?;
        let next = {
            let mut rules = self.state.rules.write().unwrap();
            upsert(&mut rules, rule.clone());
            rules.clone()
        };
        self.state.persist_rules(&next).map_err(|e| ErrorData::internal_error(e, None))?;
        Ok(Json(ActionOut {
            ok: true,
            detail: format!(
                "rule {} saved ({} total){}",
                rule.id,
                next.len(),
                self.fault_note(std::slice::from_ref(&rule))
            ),
        }))
    }

    /// Add or replace several rules at once.
    #[tool(description = "Add or replace several rules in one call (same fields as set_rule), \
e.g. a whole test scenario sharing one group. All are validated first; nothing is saved if \
one is invalid.")]
    pub fn import_rules(
        &self,
        Parameters(params): Parameters<ImportRulesParams>,
    ) -> Result<Json<ActionOut>, ErrorData> {
        let incoming = params.rules.into_iter().map(rule_from_params).collect::<Result<Vec<_>, _>>()?;
        let next = {
            let mut rules = self.state.rules.write().unwrap();
            incoming.iter().cloned().for_each(|r| upsert(&mut rules, r));
            rules.clone()
        };
        self.state.persist_rules(&next).map_err(|e| ErrorData::internal_error(e, None))?;
        Ok(Json(ActionOut {
            ok: true,
            detail: format!("{} rules saved ({} total){}", incoming.len(), next.len(), self.fault_note(&incoming)),
        }))
    }

    /// Switch a whole scenario on or off.
    #[tool(description = "Switch every rule of a group (scenario) on or off. With exclusive=true, \
switching one group on switches every other group off, so one scenario runs at a time. \
Rules without a group are left alone.")]
    pub fn toggle_group(
        &self,
        Parameters(params): Parameters<ToggleGroupParams>,
    ) -> Result<Json<ActionOut>, ErrorData> {
        let exclusive = params.exclusive.unwrap_or(false) && params.enabled;
        let (next, changed) = {
            let mut rules = self.state.rules.write().unwrap();
            let mut changed = 0;
            for r in rules.iter_mut() {
                let Some(g) = r.group.as_deref() else { continue };
                let want = if g == params.group {
                    params.enabled
                } else if exclusive {
                    false
                } else {
                    continue;
                };
                if r.enabled != want {
                    r.enabled = want;
                    changed += 1;
                }
            }
            (rules.clone(), changed)
        };
        if !next.iter().any(|r| r.group.as_deref() == Some(params.group.as_str())) {
            return Err(ErrorData::invalid_params(format!("no rule in group {:?}", params.group), None));
        }
        self.state.persist_rules(&next).map_err(|e| ErrorData::internal_error(e, None))?;
        let state = if params.enabled { "on" } else { "off" };
        Ok(Json(ActionOut { ok: true, detail: format!("group {:?} {state}; {changed} rules changed", params.group) }))
    }

    /// How often rules acted.
    #[tool(description = "How many requests each rule acted on since the last reset_hits: the \
way to check that a scenario's fault was actually injected, and how often (e.g. that an app \
did not retry a write: a PassCut on the write should show hits 1). Filter by rule_id or group.")]
    pub fn get_rule_hits(&self, Parameters(params): Parameters<HitsParams>) -> Json<Vec<HitOut>> {
        let rules = self.state.rules.read().unwrap().clone();
        Json(
            rules
                .iter()
                .filter(|r| selected(r, &params))
                .map(|r| HitOut {
                    rule_id: r.id.clone(),
                    name: r.name.clone(),
                    group: r.group.clone(),
                    enabled: r.enabled,
                    hits: self.state.hits.get(&r.id),
                    times: r.times,
                })
                .collect(),
        )
    }

    /// Zero hit counts.
    #[tool(description = "Zero the hit counts of a rule, a group, or every rule (no filter). \
This also re-arms rules limited by times (e.g. first-only).")]
    pub fn reset_hits(&self, Parameters(params): Parameters<HitsParams>) -> Json<ActionOut> {
        if params.rule_id.is_none() && params.group.is_none() {
            self.state.hits.reset(None);
            return Json(ActionOut { ok: true, detail: "every rule's count reset".into() });
        }
        let ids: Vec<String> =
            self.state.rules.read().unwrap().iter().filter(|r| selected(r, &params)).map(|r| r.id.clone()).collect();
        self.state.hits.reset(Some(&ids));
        Json(ActionOut { ok: true, detail: format!("{} rules reset", ids.len()) })
    }

    /// The hosts Fault rules may act on.
    #[tool(description = "The hosts Fault rules may act on (globs). Read-only: only the user can \
change it, in NovaProxy's Rules page. A Fault rule for a host outside it does nothing.")]
    pub fn get_fault_settings(&self) -> Json<FaultSettingsOut> {
        Json(FaultSettingsOut { allow_hosts: self.state.fault.read().unwrap().allow_hosts.clone() })
    }

    /// Remove a traffic-control rule.
    #[tool(description = "Delete a traffic-control rule by id.")]
    pub fn delete_rule(&self, Parameters(params): Parameters<DeleteRuleParams>) -> Result<Json<ActionOut>, ErrorData> {
        let (next, removed) = {
            let mut rules = self.state.rules.write().unwrap();
            let before = rules.len();
            rules.retain(|r| r.id != params.id);
            (rules.clone(), before != rules.len())
        };
        if !removed {
            return Err(ErrorData::invalid_params(format!("no rule with id {}", params.id), None));
        }
        self.state.persist_rules(&next).map_err(|e| ErrorData::internal_error(e, None))?;
        Ok(Json(ActionOut { ok: true, detail: format!("rule {} deleted", params.id) }))
    }

    /// Anti-patterns found in the captured traffic.
    #[tool(
        description = "Anti-patterns NovaProxy detected in the captured traffic, most severe first: request bursts / N+1 loops, polling, retry storms, rate limiting, high error rates, uncompressed or oversized responses, uncacheable static assets, redirect chains and loops, slow server responses, CORS preflight overhead, credentials in URLs or over plain HTTP, and cookies missing Secure/HttpOnly, heavy or legacy-format images, short cache lifetimes on fingerprinted \
assets, mixed content, HTML pages missing security headers, missing assets (404), empty 200 \
responses, slow DNS/TCP/TLS setup, and CDN cache misses. Each comes with numbers, advice and example flow ids for get_flow. Filters: app, host, min_severity (info | warning | high)."
    )]
    pub fn get_insights(&self, Parameters(params): Parameters<InsightParams>) -> Json<InsightsOut> {
        let report = self.state.insights();
        Json(InsightsOut {
            flows_analyzed: report.analyzed,
            insights: report.insights.into_iter().filter(|i| matches_insight(i, &params)).map(insight_out).collect(),
        })
    }

    /// The capture at a glance.
    #[tool(description = "Summarize the captured traffic in one call: counts by status class, \
bytes, the busiest apps and hosts (with p50/p95 duration), the most-called, slowest (p95 server \
wait) and most-failing endpoints with an example flow id each, and how many Insights findings \
there are by severity. Start here for \"what is this app doing?\". Filters: app, host, \
include_internal.")]
    pub fn summarize_session(&self, Parameters(p): Parameters<SummaryParams>) -> Json<crate::analysis::SessionSummary> {
        let filters = Filters {
            app: p.app.clone(),
            host: p.host.clone(),
            include_internal: p.include_internal,
            ..Filters::default()
        };
        let flows: Vec<Flow> = self.state.flows.shells().into_iter().filter(|f| matches_filters(f, &filters)).collect();
        let mut report = self.state.insights();
        let scope = InsightParams { app: p.app, host: p.host, min_severity: None };
        report.insights.retain(|i| matches_insight(i, &scope));
        Json(crate::analysis::summarize_session(&flows, &report))
    }

    /// What differs between two flows.
    #[tool(description = "Compare two flows: method, URL and query parameters, status, request \
and response headers (noise such as Date and trace ids left out unless include_noisy), bodies \
(JSON compared path by path), and timings, with a plain-words summary first. Typical use: a call \
that worked against one that failed (explain_failure suggests the pair).")]
    pub fn diff_flows(
        &self,
        Parameters(p): Parameters<DiffParams>,
    ) -> Result<Json<crate::analysis::FlowDiff>, ErrorData> {
        let a = self.state.flows.get(&p.a).ok_or_else(|| unknown_flow(&p.a))?;
        let b = self.state.flows.get(&p.b).ok_or_else(|| unknown_flow(&p.b))?;
        let redact = self.redacting();
        let show = move |k: &str, v: &str| shown_value(redact, k, v);
        Ok(Json(crate::analysis::diff_flows(&a, &b, p.include_noisy, &show)))
    }

    /// Why a flow failed.
    #[tool(description = "Explain why a flow failed, from what was captured: transport errors \
(DNS, refused, timeout, reset, TLS certificate), NovaProxy's own rules, each 4xx/5xx with its \
evidence (WWW-Authenticate, Allow, Retry-After, the API's error message), CORS problems a \
browser would hit, and the newest earlier call to the same endpoint that succeeded, with the \
request headers it sent that this one did not.")]
    pub fn explain_failure(
        &self,
        Parameters(p): Parameters<ExplainParams>,
    ) -> Result<Json<crate::analysis::Explanation>, ErrorData> {
        let flow = self.state.flows.get(&p.flow_id).ok_or_else(|| unknown_flow(&p.flow_id))?;
        let flow = self.shown(flow);
        Ok(Json(crate::analysis::explain_failure(&flow, &self.state.flows.shells())))
    }

    /// Proxy and capture status.
    #[tool(description = "Proxy state: whether it is running and where, whether the system \
proxy points at it, how many flows were captured and how many are still retained.")]
    pub fn proxy_status(&self) -> Json<StatusOut> {
        // Read the engine, release it, then read the CA — never hold two of
        // AppState's locks at once, so no ordering can deadlock.
        let engine_addr = self.state.engine.lock().unwrap().as_ref().map(|h| h.addr.to_string());
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
For an overview of a capture, call summarize_session; to find what is wrong with an app's traffic without reading every flow, call get_insights: it lists detected anti-patterns with example flow ids. For one failing request, explain_failure says why and names a successful call to compare it with in diff_flows.

To test how an app copes with timeouts and network errors, add Fault rules (set_rule or \
import_rules, kind Fault) grouped by scenario, switch a scenario with toggle_group, run the \
app, then read get_rule_hits and the flows (failed ones carry edits.fault). Fault rules act \
only on hosts the user allowed (get_fault_settings).

Your own calls to this endpoint are marked internal and excluded unless you pass \
include_internal=true. Bodies are capped in every response; ask for more with get_body.\n\n\
Unless the user turned it off, credentials are hidden: authorization headers, cookies and \
secret query parameters or body fields read `[redacted]`. replay_request still sends the \
original values.",
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
        let hit = flow.process.as_deref().map(|p| p.to_lowercase().contains(&app.to_lowercase())).unwrap_or(false);
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
    if flow.request_headers.iter().chain(flow.response_headers.iter()).any(|h| hit(&h.name) || hit(&h.value)) {
        return true;
    }
    [&flow.request_body, &flow.response_body].into_iter().flatten().any(|b| b.text.as_deref().map(hit).unwrap_or(false))
}

fn severity_rank(s: &str) -> Option<nova_proto::InsightSeverity> {
    use nova_proto::InsightSeverity::*;
    match s.trim().to_ascii_lowercase().as_str() {
        "info" => Some(Info),
        "warning" | "warn" => Some(Warning),
        "high" | "error" | "critical" => Some(High),
        _ => None,
    }
}

/// Does an insight pass the agent's filters? An unknown `min_severity` filters
/// nothing rather than everything.
pub fn matches_insight(i: &nova_proto::Insight, p: &InsightParams) -> bool {
    let has = |hay: &str, needle: &Option<String>| {
        needle.as_ref().is_none_or(|n| hay.to_lowercase().contains(&n.to_lowercase()))
    };
    let min = p.min_severity.as_deref().and_then(severity_rank);
    min.is_none_or(|m| i.severity >= m) && has(i.app.as_deref().unwrap_or(""), &p.app) && has(&i.host, &p.host)
}

/* -------------------------------- projection -------------------------------- */

fn insight_out(i: nova_proto::Insight) -> InsightOut {
    let skip = i.flow_ids.len().saturating_sub(INSIGHT_EXAMPLES);
    InsightOut {
        severity: format!("{:?}", i.severity).to_lowercase(),
        example_flow_ids: i.flow_ids.into_iter().skip(skip).collect(),
        id: i.id,
        rule: i.rule,
        title: i.title,
        detail: i.detail,
        advice: i.advice,
        app: i.app,
        host: i.host,
        flow_count: i.count,
    }
}

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
    let headers = |hs: &[nova_proto::Header]| hs.iter().map(|h| (h.name.clone(), h.value.clone())).collect::<Vec<_>>();
    FlowDetail {
        summary: summarize(flow),
        scheme: flow.scheme.clone(),
        path: flow.path.clone(),
        http_version: flow.http_version.clone(),
        request_headers: headers(&flow.request_headers),
        response_headers: headers(&flow.response_headers),
        request_body: flow.request_body.as_ref().map(|b| body_out(b, body_chars, b.spilled)),
        response_body: flow.response_body.as_ref().map(|b| body_out(b, body_chars, b.spilled)),
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
            format!("binary body, {} bytes ({})", body.size, body.media_type.as_deref().unwrap_or("unknown type"))
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
        "fault" => Ok(RuleKind::Fault),
        _ => Err(ErrorData::invalid_params(
            format!("unknown rule kind {kind:?}; expected MapRemote, MapLocal, Block, Rewrite or Fault"),
            None,
        )),
    }
}

fn parse_fault_mode(mode: &str) -> Result<FaultMode, ErrorData> {
    Ok(match mode.to_lowercase().replace(['_', '-', ' '], "").as_str() {
        "dropbefore" | "drop" => FaultMode::DropBefore,
        "holdclose" => FaultMode::HoldClose,
        "holdstatus" | "hold504" => FaultMode::HoldStatus,
        "fakeresponse" | "fakebody" | "fake" => FaultMode::FakeResponse,
        "waf403" | "waf" => FaultMode::Waf403,
        "proxyauth407" | "auth407" | "407" => FaultMode::ProxyAuth407,
        "passcut" => FaultMode::PassCut,
        "cutbody" => FaultMode::CutBody,
        "delay" => FaultMode::Delay,
        _ => {
            return Err(ErrorData::invalid_params(
                format!(
                    "unknown fault mode {mode:?}; expected DropBefore, HoldClose, HoldStatus, FakeResponse, \
Waf403, ProxyAuth407, PassCut, CutBody or Delay"
                ),
                None,
            ))
        }
    })
}

/// A validated rule from tool parameters.
fn rule_from_params(params: SetRuleParams) -> Result<Rule, ErrorData> {
    let kind = parse_rule_kind(&params.kind)?;
    let fault = match params.fault {
        Some(f) => Some(Fault {
            mode: parse_fault_mode(&f.mode)?,
            delay_ms: f.delay_ms,
            status: f.status,
            body: f.body,
            content_type: f.content_type,
            cut_after_bytes: f.cut_after_bytes,
        }),
        None => None,
    };
    let blank = |s: Option<String>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let rule = Rule {
        id: params.id.clone(),
        enabled: params.enabled.unwrap_or(true),
        kind,
        name: params.name.unwrap_or_else(|| params.id.clone()),
        pattern: params.pattern,
        target: params.target,
        header_name: params.header_name,
        header_value: params.header_value,
        method: blank(params.method).map(|m| m.to_uppercase()),
        group: blank(params.group),
        times: params.times,
        fault,
    };
    validate_rule(&rule)?;
    Ok(rule)
}

fn upsert(rules: &mut Vec<Rule>, rule: Rule) {
    match rules.iter_mut().find(|r| r.id == rule.id) {
        Some(existing) => *existing = rule,
        None => rules.push(rule),
    }
}

fn selected(rule: &Rule, params: &HitsParams) -> bool {
    params.rule_id.as_deref().is_none_or(|id| rule.id == id)
        && params.group.as_deref().is_none_or(|g| rule.group.as_deref() == Some(g))
}

/// Reject rules that would silently do nothing — a rule the agent believes it
/// installed but which cannot act is worse than an error.
fn validate_rule(rule: &Rule) -> Result<(), ErrorData> {
    let missing = |what: &str| Err(ErrorData::invalid_params(format!("{:?} rules need {what}", rule.kind), None));
    match rule.kind {
        RuleKind::MapRemote | RuleKind::MapLocal if rule.target.as_deref().unwrap_or("").is_empty() => {
            missing("a target")
        }
        RuleKind::Rewrite if rule.header_name.as_deref().unwrap_or("").is_empty() => missing("a header_name"),
        RuleKind::Fault if rule.fault.is_none() => missing("a fault"),
        RuleKind::Fault if rule.fault.as_ref().and_then(|f| f.delay_ms).is_some_and(|d| d > FAULT_MAX_DELAY_MS) => {
            missing("a delay_ms of at most 600000")
        }
        _ if rule.times == Some(0) => missing("times of at least 1 (or none)"),
        _ if rule.pattern.is_empty() => missing("a pattern"),
        _ => Ok(()),
    }
}

/// A value as the client may see it, for a diff: `key` is a header name, a
/// `?query` parameter, a JSON key, `url` or `body`.
fn shown_value(redact: bool, key: &str, value: &str) -> String {
    use nova_core::redact;
    if !redact {
        return value.to_string();
    }
    match key {
        "url" => redact::url(value),
        "body" => redact::body_text(value, None),
        _ => {
            if let Some(param) = key.strip_prefix('?') {
                if redact::is_secret_name(param) || redact::looks_like_jwt(value) {
                    return redact::MARK.to_string();
                }
                return value.to_string();
            }
            if redact::is_secret_header(key) {
                return redact::header_value(key, value);
            }
            if redact::is_secret_name(key) || redact::looks_like_jwt(value) {
                return redact::MARK.to_string();
            }
            value.to_string()
        }
    }
}

fn unknown_flow(id: &str) -> ErrorData {
    ErrorData::invalid_params(
        format!("no flow with id {id}; call list_flows for current ids (older flows are evicted)"),
        None,
    )
}

/* --------------------------------- transport --------------------------------- */

/// Whether a request carries the endpoint's token as `Authorization: Bearer`.
///
/// Loopback alone is not enough: any local process — a malicious npm script, a
/// browser extension's native host — could otherwise read decrypted traffic and
/// install redirect rules. The comparison takes the same time whatever the
/// input, so the token cannot be guessed a byte at a time.
pub fn authorized(headers: &hyper::HeaderMap, token: &str) -> bool {
    let Some(value) = headers.get(hyper::header::AUTHORIZATION).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let Some((scheme, given)) = value.trim().split_once(' ') else { return false };
    if !scheme.eq_ignore_ascii_case("bearer") {
        return false;
    }
    let (a, b) = (given.trim().as_bytes(), token.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn unauthorized() -> hyper::Response<http_body_util::combinators::BoxBody<hyper::body::Bytes, std::convert::Infallible>>
{
    use http_body_util::BodyExt;
    hyper::Response::builder()
        .status(hyper::StatusCode::UNAUTHORIZED)
        .header(hyper::header::WWW_AUTHENTICATE, "Bearer realm=\"novaproxy\"")
        .header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(
            http_body_util::Full::new(hyper::body::Bytes::from_static(
                b"NovaProxy MCP needs its access token: send `Authorization: Bearer <token>`. Copy the setup command from NovaProxy > Settings > MCP.",
            ))
            .boxed(),
        )
        .expect("static response")
}

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
    // Fail before binding when the token cannot be read or created: an endpoint
    // that cannot check a token must not serve.
    state.mcp_token()?;
    let auth_state = state.clone();
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener =
        tokio::net::TcpListener::bind(addr).await.map_err(|e| format!("cannot bind MCP endpoint on {addr}: {e}"))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;

    let service = StreamableHttpService::new(
        move || Ok(NovaMcp::new(state.clone())),
        Arc::new(LocalSessionManager::default()),
        // Stateless with plain JSON replies: this server never initiates messages,
        // so sessions and SSE framing would add bookkeeping and parsing work for
        // every client without buying anything. Host validation stays on (rmcp
        // defaults to loopback only), which is what blocks a browser page from
        // driving this endpoint via DNS rebinding.
        StreamableHttpServerConfig::default().with_legacy_session_mode(false).with_json_response(true),
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
            let auth_state = auth_state.clone();
            tokio::spawn(async move {
                let io = hyper_util::rt::TokioIo::new(stream);
                let handler = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                    let mut svc = svc.clone();
                    // Read per request, so a rotated token applies at once.
                    let allowed = auth_state.mcp_token().is_ok_and(|t| authorized(req.headers(), &t));
                    async move {
                        if !allowed {
                            return Ok(unauthorized());
                        }
                        tower_service::Service::call(&mut svc, req).await
                    }
                });
                if let Err(e) = hyper::server::conn::http1::Builder::new().serve_connection(io, handler).await {
                    tracing::debug!("MCP connection ended: {e}");
                }
            });
        }
        tracing::info!("MCP endpoint stopped");
    });

    tracing::info!("MCP endpoint listening on http://{addr}/");
    Ok(McpHandle { addr, stop: Some(stop_tx) })
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
        assert!(!matches_filters(&f, &Filters::default()), "an agent must not see its own traffic by default");
        assert!(matches_filters(&f, &Filters { include_internal: true, ..Default::default() }));
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
        assert!(matches_filters(&ours, &Filters { mcp_only: true, include_internal: true, ..Default::default() }));
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
        assert!(!matches_filters(&flow("f0"), &Filters { app: Some("claude".into()), ..Default::default() }));
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
        assert!(matches_filters(&transport_error, &Filters { errors_only: true, ..Default::default() }));
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

    fn insight(app: Option<&str>, host: &str, severity: nova_proto::InsightSeverity) -> nova_proto::Insight {
        nova_proto::Insight {
            id: "r:k".into(),
            rule: "r".into(),
            severity,
            title: String::new(),
            detail: String::new(),
            advice: String::new(),
            app: app.map(Into::into),
            host: host.into(),
            flow_ids: (0..30).map(|i| format!("f{i}")).collect(),
            count: 30,
            last_seen: 0.0,
        }
    }

    #[test]
    fn insight_filters_match_app_host_and_minimum_severity() {
        use nova_proto::InsightSeverity::*;
        let i = insight(Some("Google Chrome"), "api.github.com", Warning);
        let p = |app: Option<&str>, host: Option<&str>, min: Option<&str>| InsightParams {
            app: app.map(Into::into),
            host: host.map(Into::into),
            min_severity: min.map(Into::into),
        };
        assert!(matches_insight(&i, &p(None, None, None)));
        assert!(matches_insight(&i, &p(Some("chrome"), Some("GITHUB"), Some("warning"))));
        assert!(!matches_insight(&i, &p(None, None, Some("high"))));
        assert!(!matches_insight(&i, &p(Some("slack"), None, None)));
        assert!(matches_insight(&i, &p(None, None, Some("bogus"))), "an unknown level filters nothing");
        assert!(!matches_insight(&insight(None, "x", Info), &p(Some("chrome"), None, None)));
    }

    #[test]
    fn insights_for_agents_keep_only_the_newest_examples() {
        let out = insight_out(insight(None, "x", nova_proto::InsightSeverity::High));
        assert_eq!(out.severity, "high");
        assert_eq!(out.flow_count, 30);
        assert_eq!(out.example_flow_ids.len(), INSIGHT_EXAMPLES);
        assert_eq!(out.example_flow_ids.last().map(String::as_str), Some("f29"));
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
            ..Default::default()
        };
        assert!(validate_rule(&base).is_err(), "MapRemote without a target");
        assert!(validate_rule(&Rule { target: Some("https://staging".into()), ..base.clone() }).is_ok());
        assert!(
            validate_rule(&Rule { kind: RuleKind::Rewrite, ..base.clone() }).is_err(),
            "Rewrite without a header name"
        );
        assert!(
            validate_rule(&Rule { kind: RuleKind::Block, pattern: String::new(), ..base.clone() }).is_err(),
            "no pattern at all"
        );
        assert!(validate_rule(&Rule { kind: RuleKind::Block, ..base.clone() }).is_ok());
        assert!(validate_rule(&Rule { kind: RuleKind::Fault, ..base.clone() }).is_err(), "Fault without a fault");
        let fault = Some(Fault { mode: FaultMode::HoldClose, delay_ms: Some(1_000), ..Default::default() });
        assert!(validate_rule(&Rule { kind: RuleKind::Fault, fault: fault.clone(), ..base.clone() }).is_ok());
        let too_long = Some(Fault { delay_ms: Some(FAULT_MAX_DELAY_MS + 1), ..fault.clone().unwrap() });
        assert!(validate_rule(&Rule { kind: RuleKind::Fault, fault: too_long, ..base.clone() }).is_err());
        assert!(validate_rule(&Rule { kind: RuleKind::Block, times: Some(0), ..base }).is_err(), "times 0");
    }

    #[test]
    fn fault_modes_parse_forgivingly() {
        assert_eq!(parse_fault_mode("pass-cut").unwrap(), FaultMode::PassCut);
        assert_eq!(parse_fault_mode("Hold_Close").unwrap(), FaultMode::HoldClose);
        assert_eq!(parse_fault_mode("waf-403").unwrap(), FaultMode::Waf403);
        assert_eq!(parse_fault_mode("407").unwrap(), FaultMode::ProxyAuth407);
        assert!(parse_fault_mode("explode").is_err());
    }

    #[test]
    fn rule_params_normalise_method_and_group() {
        let p = SetRuleParams {
            id: "a".into(),
            kind: "fault".into(),
            name: None,
            pattern: "https://x/*".into(),
            target: None,
            header_name: None,
            header_value: None,
            enabled: None,
            method: Some(" post ".into()),
            group: Some("  ".into()),
            times: Some(1),
            fault: Some(FaultParams {
                mode: "drop-before".into(),
                delay_ms: None,
                status: None,
                body: None,
                content_type: None,
                cut_after_bytes: None,
            }),
        };
        let r = rule_from_params(p).unwrap();
        assert_eq!(r.method.as_deref(), Some("POST"));
        assert_eq!(r.group, None, "a blank group is no group");
        assert_eq!(r.fault.unwrap().mode, FaultMode::DropBefore);
    }
}
