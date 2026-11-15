//! What the higher-level MCP tools work out: a summary of a whole capture, the
//! differences between two flows, and why a flow failed.
//!
//! Fixed rules over what was captured, like the Insights checks — an agent gets
//! numbers and evidence it can verify with `get_flow`, not a guess. Each takes
//! flows and returns plain data; `mcp.rs` wraps them as tools and decides what
//! the client may see.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use nova_proto::{Flow, FlowState, Header, InsightReport, InsightSeverity};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

/// `GET /users/:id` — the grouping the Insights checks and the report use.
fn endpoint(f: &Flow) -> String {
    format!("{} {}", f.method, nova_core::insights::normalize_path(&f.path))
}

fn failed(f: &Flow) -> bool {
    f.state == FlowState::Error || f.error.is_some() || f.status.is_some_and(|s| s >= 400)
}

fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    Some(sorted[rank.clamp(1, sorted.len()) - 1])
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/* ------------------------------ summarize_session ------------------------------ */

#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct StatusCounts {
    pub ok_2xx: u32,
    pub redirect_3xx: u32,
    pub client_error_4xx: u32,
    pub server_error_5xx: u32,
    /// Ended without any response: refused, reset, timed out, blocked.
    pub no_response: u32,
    pub in_progress: u32,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AppRow {
    pub app: String,
    pub flows: u32,
    pub failed: u32,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct HostRow {
    pub host: String,
    pub flows: u32,
    pub failed: u32,
    pub bytes_received: u64,
    /// Median and 95th-percentile total duration, completed flows only.
    pub p50_ms: Option<f64>,
    pub p95_ms: Option<f64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct EndpointRow {
    pub host: String,
    /// `METHOD /path/:id` — ids, uuids and hashes folded together.
    pub endpoint: String,
    pub calls: u32,
    /// Server wait (time to first byte less connection setup and upload).
    pub p50_server_wait_ms: Option<f64>,
    pub p95_server_wait_ms: Option<f64>,
    /// Failures and their statuses, e.g. `3× 503, 1× no response`.
    pub failures: Option<String>,
    pub example_flow_id: String,
}

#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct InsightCounts {
    pub high: u32,
    pub warning: u32,
    pub info: u32,
    /// Titles of the most severe findings; get_insights has the detail.
    pub top: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SessionSummary {
    pub flows: u32,
    /// Epoch milliseconds of the first and last request, and the span between.
    pub first_started_at: Option<f64>,
    pub last_started_at: Option<f64>,
    pub span_seconds: f64,
    pub status: StatusCounts,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub websockets: u32,
    pub mcp_calls: u32,
    /// Busiest first, at most 10.
    pub apps: Vec<AppRow>,
    /// Busiest first, at most 10.
    pub hosts: Vec<HostRow>,
    /// Most calls first, at most 5.
    pub most_called: Vec<EndpointRow>,
    /// Slowest p95 server wait first, at most 5.
    pub slowest: Vec<EndpointRow>,
    /// Most failures first, at most 5.
    pub failing: Vec<EndpointRow>,
    pub insights: InsightCounts,
}

/// One pass over `flows` (any order, no bodies needed).
pub fn summarize_session(flows: &[Flow], report: &InsightReport) -> SessionSummary {
    let mut status = StatusCounts::default();
    let (mut sent, mut received, mut ws, mut mcp) = (0u64, 0u64, 0u32, 0u32);
    let mut apps: HashMap<String, (u32, u32)> = HashMap::new();
    let mut hosts: HashMap<String, (u32, u32, u64, Vec<f64>)> = HashMap::new();
    let mut eps: HashMap<(String, String), Vec<&Flow>> = HashMap::new();
    let (mut first, mut last) = (f64::INFINITY, f64::NEG_INFINITY);

    for f in flows {
        first = first.min(f.started_at);
        last = last.max(f.started_at);
        sent += f.request_size;
        received += f.response_size;
        ws += u32::from(f.is_websocket);
        mcp += u32::from(f.mcp.is_some());
        let bad = failed(f);
        match (f.state, f.status) {
            (FlowState::Started | FlowState::Paused, None) => status.in_progress += 1,
            (_, Some(s)) if s >= 500 => status.server_error_5xx += 1,
            (_, Some(s)) if s >= 400 => status.client_error_4xx += 1,
            (_, Some(s)) if s >= 300 => status.redirect_3xx += 1,
            (_, Some(_)) => status.ok_2xx += 1,
            (_, None) => status.no_response += 1,
        }
        let app = apps.entry(f.process.clone().unwrap_or_else(|| "(unattributed)".into())).or_default();
        app.0 += 1;
        app.1 += u32::from(bad);
        let host = hosts.entry(f.host.clone()).or_default();
        host.0 += 1;
        host.1 += u32::from(bad);
        host.2 += f.response_size;
        if let (Some(d), FlowState::Completed) = (f.duration_ms, f.state) {
            if !f.is_websocket && !f.tunneled {
                host.3.push(d);
            }
        }
        if !f.is_websocket && !f.tunneled {
            eps.entry((f.host.clone(), endpoint(f))).or_default().push(f);
        }
    }

    let mut apps: Vec<AppRow> = apps.into_iter().map(|(app, (flows, failed))| AppRow { app, flows, failed }).collect();
    apps.sort_by(|a, b| b.flows.cmp(&a.flows).then(a.app.cmp(&b.app)));
    apps.truncate(10);

    let mut hosts: Vec<HostRow> = hosts
        .into_iter()
        .map(|(host, (flows, failed, bytes, mut d))| {
            d.sort_by(f64::total_cmp);
            HostRow {
                host,
                flows,
                failed,
                bytes_received: bytes,
                p50_ms: percentile(&d, 50.0).map(round1),
                p95_ms: percentile(&d, 95.0).map(round1),
            }
        })
        .collect();
    hosts.sort_by(|a, b| b.flows.cmp(&a.flows).then(a.host.cmp(&b.host)));
    hosts.truncate(10);

    let rows: Vec<EndpointRow> = eps.into_iter().map(|((host, ep), group)| endpoint_row(host, ep, &group)).collect();
    let top = |mut v: Vec<&EndpointRow>, key: &dyn Fn(&EndpointRow) -> f64| -> Vec<EndpointRow> {
        v.sort_by(|a, b| key(b).total_cmp(&key(a)).then(a.endpoint.cmp(&b.endpoint)));
        v.into_iter().take(5).map(clone_row).collect()
    };
    let most_called = top(rows.iter().collect(), &|r| r.calls as f64);
    let slowest =
        top(rows.iter().filter(|r| r.p95_server_wait_ms.is_some()).collect(), &|r| r.p95_server_wait_ms.unwrap_or(0.0));
    let failing = top(rows.iter().filter(|r| r.failures.is_some()).collect(), &|r| failure_count(r) as f64);

    let mut insights = InsightCounts::default();
    for i in &report.insights {
        match i.severity {
            InsightSeverity::High => insights.high += 1,
            InsightSeverity::Warning => insights.warning += 1,
            InsightSeverity::Info => insights.info += 1,
        }
    }
    insights.top = report.insights.iter().take(5).map(|i| i.title.clone()).collect();

    let any = !flows.is_empty();
    SessionSummary {
        flows: flows.len() as u32,
        first_started_at: any.then_some(first),
        last_started_at: any.then_some(last),
        span_seconds: if any { round1((last - first) / 1000.0) } else { 0.0 },
        status,
        bytes_sent: sent,
        bytes_received: received,
        websockets: ws,
        mcp_calls: mcp,
        apps,
        hosts,
        most_called,
        slowest,
        failing,
        insights,
    }
}

fn endpoint_row(host: String, endpoint: String, group: &[&Flow]) -> EndpointRow {
    let mut waits: Vec<f64> = group
        .iter()
        .filter(|f| !f.edits.breakpoint)
        .filter_map(|f| nova_core::timing::server_wait_ms(&f.timings))
        .collect();
    waits.sort_by(f64::total_cmp);
    let mut statuses: BTreeMap<String, u32> = BTreeMap::new();
    for f in group.iter().filter(|f| failed(f)) {
        *statuses.entry(f.status.map(|s| s.to_string()).unwrap_or_else(|| "no response".into())).or_default() += 1;
    }
    let failures = (!statuses.is_empty()).then(|| {
        let mut v: Vec<_> = statuses.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.iter().map(|(s, n)| format!("{n}× {s}")).collect::<Vec<_>>().join(", ")
    });
    // The newest failure is the one worth opening; otherwise the newest call.
    let example = group.iter().rev().find(|f| failed(f)).or(group.last()).map(|f| f.id.clone()).unwrap_or_default();
    EndpointRow {
        host,
        endpoint,
        calls: group.len() as u32,
        p50_server_wait_ms: percentile(&waits, 50.0).map(round1),
        p95_server_wait_ms: percentile(&waits, 95.0).map(round1),
        failures,
        example_flow_id: example,
    }
}

fn failure_count(r: &EndpointRow) -> u32 {
    r.failures
        .as_deref()
        .unwrap_or("")
        .split(", ")
        .filter_map(|p| p.split('×').next()?.trim().parse::<u32>().ok())
        .sum()
}

fn clone_row(r: &EndpointRow) -> EndpointRow {
    EndpointRow {
        host: r.host.clone(),
        endpoint: r.endpoint.clone(),
        calls: r.calls,
        p50_server_wait_ms: r.p50_server_wait_ms,
        p95_server_wait_ms: r.p95_server_wait_ms,
        failures: r.failures.clone(),
        example_flow_id: r.example_flow_id.clone(),
    }
}

/* ---------------------------------- diff_flows ---------------------------------- */

/// Headers that differ on every exchange and say nothing about why two
/// requests behave differently.
const NOISY_HEADERS: [&str; 17] = [
    "date",
    "age",
    "expires",
    "last-modified",
    "etag",
    "content-length",
    "x-request-id",
    "x-correlation-id",
    "x-amzn-requestid",
    "x-amzn-trace-id",
    "x-amz-cf-id",
    "cf-ray",
    "traceparent",
    "tracestate",
    "server-timing",
    "report-to",
    "nel",
];

#[derive(Debug, Serialize, JsonSchema)]
pub struct ValueDiff {
    /// Header name, query parameter or JSON path (`$.items[0].id`).
    pub name: String,
    /// Absent when only the other flow has it.
    pub a: Option<String>,
    pub b: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct BodyDiff {
    pub a_bytes: Option<u64>,
    pub b_bytes: Option<u64>,
    pub identical: bool,
    /// For JSON bodies: paths that were added, removed or changed (at most 50).
    pub json_changes: Vec<ValueDiff>,
    /// For other text: where the two first differ, with a little context.
    pub first_difference: Option<String>,
    /// Set when the comparison only covers the captured previews.
    pub note: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SideDiff {
    pub headers: Vec<ValueDiff>,
    pub body: BodyDiff,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FlowDiff {
    pub a: String,
    pub b: String,
    /// The differences most likely to matter, in plain words.
    pub summary: Vec<String>,
    pub method: Option<ValueDiff>,
    pub url: Option<ValueDiff>,
    pub query: Vec<ValueDiff>,
    pub status: Option<ValueDiff>,
    pub request: SideDiff,
    pub response: SideDiff,
    /// Durations in milliseconds: total, time to first byte, server wait.
    pub timing: Vec<ValueDiff>,
}

/// Compare `a` and `b`. `show` maps a raw value to what the client may see
/// (redaction); differences are found on the raw values, so two different
/// tokens still count as a difference even when both read `[redacted]`.
pub fn diff_flows(a: &Flow, b: &Flow, include_noisy: bool, show: &dyn Fn(&str, &str) -> String) -> FlowDiff {
    let mut summary = Vec::new();
    let pair = |name: &str, x: Option<String>, y: Option<String>| {
        (x != y).then(|| ValueDiff { name: name.into(), a: x, b: y })
    };

    let method = pair("method", Some(a.method.clone()), Some(b.method.clone()));
    let (a_base, b_base) = (a.url.split('?').next().unwrap_or(""), b.url.split('?').next().unwrap_or(""));
    let url = pair("url", Some(show("url", a_base)), Some(show("url", b_base))).or_else(|| {
        (a_base != b_base).then(|| ValueDiff {
            name: "url".into(),
            a: Some(show("url", a_base)),
            b: Some(show("url", b_base)),
        })
    });
    let query = map_diff(&query_pairs(&a.url), &query_pairs(&b.url), |k, v| show(&format!("?{k}"), v));
    let status = pair("status", a.status.map(|s| s.to_string()), b.status.map(|s| s.to_string()));
    if let Some(s) = &status {
        summary.push(format!("Status {} → {}.", s.a.as_deref().unwrap_or("none"), s.b.as_deref().unwrap_or("none")));
    }
    if method.is_some() {
        summary.push(format!("Method {} → {}.", a.method, b.method));
    }
    if url.is_some() {
        summary.push("Different URL (scheme, host or path).".into());
    }
    if !query.is_empty() {
        let names: Vec<&str> = query.iter().map(|d| d.name.as_str()).collect();
        summary.push(format!("Query parameters differ: {}.", names.join(", ")));
    }

    let header_map = |hs: &[Header]| -> BTreeMap<String, String> {
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        for h in hs {
            let k = h.name.to_ascii_lowercase();
            if !include_noisy && NOISY_HEADERS.contains(&k.as_str()) {
                continue;
            }
            m.entry(k)
                .and_modify(|v| {
                    v.push_str(", ");
                    v.push_str(&h.value);
                })
                .or_insert_with(|| h.value.clone());
        }
        m
    };
    let req_headers = map_diff(&header_map(&a.request_headers), &header_map(&b.request_headers), |k, v| show(k, v));
    let res_headers = map_diff(&header_map(&a.response_headers), &header_map(&b.response_headers), |k, v| show(k, v));
    for d in &req_headers {
        match (&d.a, &d.b) {
            (Some(_), None) => summary.push(format!("Only {} sends `{}`.", a.id, d.name)),
            (None, Some(_)) => summary.push(format!("Only {} sends `{}`.", b.id, d.name)),
            _ if nova_core::redact::is_secret_header(&d.name) => {
                summary.push(format!("`{}` differs between the two (values hidden).", d.name))
            }
            _ => {}
        }
    }

    let req_body = body_diff(a.request_body.as_ref(), b.request_body.as_ref(), show);
    let res_body = body_diff(a.response_body.as_ref(), b.response_body.as_ref(), show);
    if !req_body.identical {
        summary.push("Request bodies differ.".into());
    }
    if !res_body.identical {
        summary.push("Response bodies differ.".into());
    }

    let ms = |v: Option<f64>| v.map(|x| format!("{:.0}", x));
    let mut timing = Vec::new();
    for (name, x, y) in [
        ("duration_ms", a.duration_ms, b.duration_ms),
        ("ttfb_ms", a.timings.ttfb_ms, b.timings.ttfb_ms),
        (
            "server_wait_ms",
            nova_core::timing::server_wait_ms(&a.timings),
            nova_core::timing::server_wait_ms(&b.timings),
        ),
    ] {
        timing.push(ValueDiff { name: name.into(), a: ms(x), b: ms(y) });
    }
    if let (Some(x), Some(y)) = (a.duration_ms, b.duration_ms) {
        if (x - y).abs() >= 100.0 && (x.max(y) / x.min(y).max(1.0)) >= 1.5 {
            summary.push(format!("{} took {:.0} ms, {} took {:.0} ms.", a.id, x, b.id, y));
        }
    }
    if summary.is_empty() {
        summary.push("No differences beyond headers that change on every exchange.".into());
    }

    FlowDiff {
        a: a.id.clone(),
        b: b.id.clone(),
        summary,
        method,
        url,
        query,
        status,
        request: SideDiff { headers: req_headers, body: req_body },
        response: SideDiff { headers: res_headers, body: res_body },
        timing,
    }
}

fn query_pairs(url: &str) -> BTreeMap<String, String> {
    let qs = url.split_once('?').map(|(_, q)| q.split('#').next().unwrap_or(q)).unwrap_or("");
    let mut m = BTreeMap::new();
    for p in qs.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = p.split_once('=').unwrap_or((p, ""));
        m.insert(nova_core::redact::decode_component(k), nova_core::redact::decode_component(v));
    }
    m
}

fn map_diff(
    a: &BTreeMap<String, String>,
    b: &BTreeMap<String, String>,
    show: impl Fn(&str, &str) -> String,
) -> Vec<ValueDiff> {
    let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    keys.into_iter()
        .filter(|k| a.get(*k) != b.get(*k))
        .map(|k| ValueDiff { name: k.clone(), a: a.get(k).map(|v| show(k, v)), b: b.get(k).map(|v| show(k, v)) })
        .collect()
}

fn body_diff(
    a: Option<&nova_proto::BodyPreview>,
    b: Option<&nova_proto::BodyPreview>,
    show: &dyn Fn(&str, &str) -> String,
) -> BodyDiff {
    let size = |x: Option<&nova_proto::BodyPreview>| x.map(|p| p.size);
    let text = |x: Option<&nova_proto::BodyPreview>| x.and_then(|p| p.text.clone().or_else(|| p.base64.clone()));
    let (ta, tb) = (text(a), text(b));
    let truncated = a.is_some_and(|p| p.truncated) || b.is_some_and(|p| p.truncated);
    let mut out = BodyDiff {
        a_bytes: size(a),
        b_bytes: size(b),
        identical: ta == tb && size(a) == size(b),
        json_changes: Vec::new(),
        first_difference: None,
        note: truncated.then(|| "compared on the captured previews; get_body has the full bodies".to_string()),
    };
    if out.identical {
        return out;
    }
    let (Some(ta), Some(tb)) = (ta, tb) else { return out };
    match (serde_json::from_str::<Value>(&ta), serde_json::from_str::<Value>(&tb)) {
        (Ok(ja), Ok(jb)) => json_diff("$", &ja, &jb, &mut out.json_changes, show),
        _ => {
            let at = ta
                .char_indices()
                .zip(tb.chars())
                .find(|((_, x), y)| x != y)
                .map(|((i, _), _)| i)
                .unwrap_or(ta.len().min(tb.len()));
            let snippet = |t: &str| {
                let start = t.floor_char_boundary_compat(at.saturating_sub(20));
                t[start..].chars().take(60).collect::<String>()
            };
            out.first_difference = Some(format!(
                "at character {at}: a «{}» vs b «{}»",
                show("body", &snippet(&ta)),
                show("body", &snippet(&tb))
            ));
        }
    }
    out
}

/// `str::floor_char_boundary` is unstable; this is the same thing.
trait FloorBoundary {
    fn floor_char_boundary_compat(&self, i: usize) -> usize;
}
impl FloorBoundary for str {
    fn floor_char_boundary_compat(&self, i: usize) -> usize {
        let mut i = i.min(self.len());
        while !self.is_char_boundary(i) {
            i -= 1;
        }
        i
    }
}

fn json_diff(path: &str, a: &Value, b: &Value, out: &mut Vec<ValueDiff>, show: &dyn Fn(&str, &str) -> String) {
    if out.len() >= 50 || a == b {
        return;
    }
    let short = |key: &str, v: &Value| {
        let s = match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let s = show(key, &s);
        if s.chars().count() > 120 {
            format!("{}…", s.chars().take(119).collect::<String>())
        } else {
            s
        }
    };
    let key_of = |p: &str| p.rsplit(['.', '[']).next().unwrap_or("").trim_end_matches(']').to_string();
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let keys: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            for k in keys {
                let p = format!("{path}.{k}");
                match (x.get(k), y.get(k)) {
                    (Some(va), Some(vb)) => json_diff(&p, va, vb, out, show),
                    (va, vb) => out.push(ValueDiff { name: p, a: va.map(|v| short(k, v)), b: vb.map(|v| short(k, v)) }),
                }
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            for i in 0..x.len().max(y.len()) {
                let p = format!("{path}[{i}]");
                match (x.get(i), y.get(i)) {
                    (Some(va), Some(vb)) => json_diff(&p, va, vb, out, show),
                    (va, vb) => {
                        out.push(ValueDiff { name: p, a: va.map(|v| short("", v)), b: vb.map(|v| short("", v)) })
                    }
                }
                if out.len() >= 50 {
                    break;
                }
            }
        }
        _ => {
            let k = key_of(path);
            out.push(ValueDiff { name: path.into(), a: Some(short(&k, a)), b: Some(short(&k, b)) });
        }
    }
}

/* -------------------------------- explain_failure -------------------------------- */

#[derive(Debug, Serialize, JsonSchema)]
pub struct Explanation {
    pub flow_id: String,
    pub failed: bool,
    /// transport | proxy | client | server | cors | none
    pub category: String,
    /// The likeliest reading, in a sentence or two.
    pub diagnosis: String,
    /// What in the flow supports it.
    pub evidence: Vec<String>,
    pub next_steps: Vec<String>,
    /// The newest earlier call to the same endpoint that succeeded, if any;
    /// pass both ids to diff_flows.
    pub compare_with: Option<String>,
    /// Request headers that call sent and this one did not.
    pub missing_vs_success: Vec<String>,
}

/// Why `f` failed, as far as the capture can tell. `history` is the rest of
/// the capture (no bodies needed), used to find a successful twin.
pub fn explain_failure(f: &Flow, history: &[Flow]) -> Explanation {
    let mut evidence = Vec::new();
    let mut next = Vec::new();
    let header = |hs: &[Header], n: &str| hs.iter().find(|h| h.name.eq_ignore_ascii_case(n)).map(|h| h.value.clone());
    let sent = |n: &str| header(&f.request_headers, n).is_some();
    let message = error_message(f);
    if let Some(m) = &message {
        evidence.push(format!("Response body says: {m}"));
    }

    let (category, diagnosis): (&str, String) = if f.status.is_none()
        && (f.error.is_some() || f.state == FlowState::Error)
    {
        let err = f.error.clone().unwrap_or_default();
        let first_line = err.lines().next().unwrap_or("").to_string();
        evidence.push(format!("Error: {first_line}"));
        let lower = err.to_ascii_lowercase();
        let (cat, why, step) = if lower.contains("certificate")
            || lower.contains("unknownissuer")
            || lower.contains("invalid peer")
        {
            ("proxy", "NovaProxy could not verify the server's TLS certificate, so it refused to connect upstream.", "If a corporate proxy or antivirus re-signs traffic, make sure its root is in the OS store; otherwise the server's certificate chain is broken or expired.")
        } else if lower.contains("dns")
            || lower.contains("lookup")
            || lower.contains("no such host")
            || lower.contains("name or service not known")
            || lower.contains("11001")
        {
            (
                "transport",
                "The host name did not resolve.",
                "Check the spelling of the host and the DNS / VPN the machine is on.",
            )
        } else if lower.contains("refused") || lower.contains("10061") {
            (
                "transport",
                "Nothing accepted the connection: the server is down or listening on another port.",
                "Check that the service is running and the port is right.",
            )
        } else if lower.contains("timed out") || lower.contains("timeout") || lower.contains("10060") {
            (
                "transport",
                "The connection or the response timed out.",
                "The server is unreachable from this network or too slow; try it outside the proxy to tell which.",
            )
        } else if lower.contains("reset")
            || lower.contains("10054")
            || lower.contains("broken pipe")
            || lower.contains("connection closed")
        {
            ("transport", "The connection was dropped mid-exchange.", "Often a firewall, a load balancer idle timeout or a server crash; check the server's logs for the same time.")
        } else if lower.contains("aborted by script") || lower.contains("blocked") {
            (
                "proxy",
                "One of your NovaProxy rules or scripts stopped this request.",
                "Check the Rules and Scripts pages for a rule matching this URL.",
            )
        } else {
            ("transport", "The request got no response.", "Open the flow's error for the full cause chain.")
        };
        next.push(step.to_string());
        (cat, why.to_string())
    } else if let Some(s) = f.status {
        let www = header(&f.response_headers, "www-authenticate");
        match s {
            _ if f.edits.rule && s == 403 => {
                next.push("Disable or narrow the Block rule if this request should go through.".into());
                ("proxy", "A NovaProxy Block rule answered this request; it never reached the server.".to_string())
            }
            401 => {
                if let Some(w) = &www {
                    evidence.push(format!("WWW-Authenticate: {w}"));
                }
                if sent("authorization") || sent("cookie") {
                    evidence.push("The request carried credentials (Authorization or Cookie).".into());
                    next.push("The credentials were rejected: refresh or re-issue the token, and check it is for this environment (audience, issuer, expiry).".into());
                    ("client", "Authentication failed: credentials were sent but not accepted — usually expired or for the wrong environment.".to_string())
                } else {
                    evidence.push("No Authorization header or cookie was sent.".into());
                    next.push("Send the credential the API expects; WWW-Authenticate names the scheme.".into());
                    ("client", "Authentication required, and the request carried no credentials.".to_string())
                }
            }
            403 => {
                let waf = header(&f.response_headers, "server")
                    .is_some_and(|v| v.to_ascii_lowercase().contains("cloudflare"))
                    || header(&f.response_headers, "cf-mitigated").is_some();
                if waf {
                    evidence.push("The response comes from Cloudflare's edge.".into());
                    next.push("A bot or WAF rule blocked it; try the same request from a browser, or check the zone's firewall events.".into());
                    ("client", "Blocked at the edge (WAF / bot protection), not by the application.".to_string())
                } else {
                    next.push("The caller is authenticated but not allowed: check the token's scopes or the user's role; for form posts, a missing CSRF token gives 403 too.".into());
                    ("client", "Forbidden: the server knows who is calling and refuses this action.".to_string())
                }
            }
            404 | 410 => {
                next.push("Check the path and the ids in it; after a deploy, a renamed route or stale client is the usual cause.".into());
                ("client", "The server has nothing at this URL.".to_string())
            }
            405 => {
                if let Some(a) = header(&f.response_headers, "allow") {
                    evidence.push(format!("Allow: {a}"));
                }
                next.push("Use one of the methods the server allows.".into());
                ("client", format!("{} is not allowed on this URL.", f.method))
            }
            408 | 504 => {
                next.push("The server (or a gateway in front of it) gave up waiting; look for a slow query or upstream call at this time.".into());
                ("server", "A timeout between the gateway and the application.".to_string())
            }
            413 => {
                evidence.push(format!("Request body: {} bytes.", f.request_size));
                next.push(
                    "Send less per request (chunk or paginate the upload), or raise the server's body limit.".into(),
                );
                ("client", "The request body is larger than the server accepts.".to_string())
            }
            415 => {
                evidence.push(format!(
                    "Content-Type sent: {}",
                    header(&f.request_headers, "content-type").unwrap_or_else(|| "(none)".into())
                ));
                next.push("Send the body in the format the API expects and say so in Content-Type.".into());
                ("client", "The server does not accept this body format.".to_string())
            }
            400 | 409 | 422 => {
                next.push(
                    "Fix the request body or parameters the server named; compare with a call that worked.".into(),
                );
                ("client", "The server rejected the request's content as invalid.".to_string())
            }
            429 => {
                if let Some(r) = header(&f.response_headers, "retry-after") {
                    evidence.push(format!("Retry-After: {r}"));
                }
                next.push("Slow down: back off exponentially and honour Retry-After; get_insights shows whether this client bursts.".into());
                ("client", "Rate-limited: too many requests in too short a time.".to_string())
            }
            502 | 503 if f.error.is_some() => {
                evidence.push(format!("NovaProxy: {}", f.error.as_deref().unwrap_or("").lines().next().unwrap_or("")));
                next.push(
                    "The cause chain in the flow's error says what failed between NovaProxy and the server.".into(),
                );
                (
                    "proxy",
                    "NovaProxy produced this response because it could not complete the request upstream.".to_string(),
                )
            }
            500..=599 => {
                next.push("The fault is on the server: look at its logs for this time; if it is intermittent, compare with a call that worked.".into());
                ("server", format!("The server failed ({s})."))
            }
            _ => ("none", format!("Status {s} is not a failure.")),
        }
    } else {
        ("none", "This flow has not finished.".to_string())
    };

    // CORS: a browser drops a cross-origin response without the right header,
    // whatever its status, and the page only sees a network error.
    let mut category = category.to_string();
    let mut diagnosis = diagnosis;
    if let Some(origin) = header(&f.request_headers, "origin") {
        let cross = !f.url.starts_with(origin.trim_end_matches('/'));
        let allowed = header(&f.response_headers, "access-control-allow-origin");
        if cross && f.status.is_some() && allowed.is_none() {
            evidence
                .push(format!("Origin {origin} is cross-origin and the response has no Access-Control-Allow-Origin."));
            next.push("Have the server answer with Access-Control-Allow-Origin for this origin (and the preflight with the allowed methods and headers).".into());
            if category == "none" {
                category = "cors".into();
                diagnosis = "The server answered, but a browser will block the response: it is cross-origin and not allowed by CORS.".into();
            }
        }
    }

    // A successful call to the same endpoint is the best evidence there is.
    let ep = endpoint(f);
    let twin = history
        .iter()
        .rev()
        .filter(|h| h.id != f.id && h.host == f.host && h.started_at <= f.started_at && endpoint(h) == ep)
        .find(|h| h.status.is_some_and(|s| (200..400).contains(&s)));
    let missing_vs_success: Vec<String> = twin
        .map(|t| {
            let have: BTreeSet<String> = f.request_headers.iter().map(|h| h.name.to_ascii_lowercase()).collect();
            let mut m: Vec<String> = t
                .request_headers
                .iter()
                .map(|h| h.name.to_ascii_lowercase())
                .filter(|n| !have.contains(n) && !NOISY_HEADERS.contains(&n.as_str()))
                .collect();
            m.sort();
            m.dedup();
            m
        })
        .unwrap_or_default();
    if let Some(t) = twin {
        if category != "none" {
            next.push(format!(
                "{} called the same endpoint successfully ({}); diff_flows {} {} shows what changed.",
                t.id,
                t.status.unwrap_or(0),
                t.id,
                f.id
            ));
        }
    }
    if !missing_vs_success.is_empty() {
        evidence.push(format!("The successful call also sent: {}.", missing_vs_success.join(", ")));
    }

    Explanation {
        flow_id: f.id.clone(),
        failed: failed(f),
        category,
        diagnosis,
        evidence,
        next_steps: next,
        compare_with: twin.map(|t| t.id.clone()),
        missing_vs_success,
    }
}

/// The error text an API put in its response body, if it used a common shape:
/// `message`, `error` (string or `{message}`), `error_description`, `detail`,
/// `title`, or the first entry of `errors`.
fn error_message(f: &Flow) -> Option<String> {
    let text = f.response_body.as_ref()?.text.as_deref()?;
    let v: Value = serde_json::from_str(text).ok()?;
    let pick = |v: &Value| -> Option<String> {
        for k in ["message", "error_description", "detail", "error", "title", "msg"] {
            match v.get(k) {
                Some(Value::String(s)) if !s.is_empty() => return Some(s.clone()),
                Some(Value::Object(o)) => {
                    if let Some(Value::String(s)) = o.get("message") {
                        return Some(s.clone());
                    }
                }
                _ => {}
            }
        }
        None
    };
    let found = pick(&v).or_else(|| {
        v.get("errors").and_then(|e| e.get(0)).and_then(|e| pick(e).or_else(|| e.as_str().map(str::to_string)))
    })?;
    Some(if found.chars().count() > 300 {
        format!("{}…", found.chars().take(299).collect::<String>())
    } else {
        found
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nova_proto::BodyPreview;

    fn flow(id: &str, path: &str, status: Option<u16>) -> Flow {
        let mut f = nova_core::flow::new_flow(
            id.into(),
            0,
            "GET".into(),
            "https".into(),
            "api.test".into(),
            path.into(),
            format!("https://api.test{path}"),
            "127.0.0.1:1".into(),
            "HTTP/1.1".into(),
            Vec::new(),
        );
        f.status = status;
        f.state = FlowState::Completed;
        f.duration_ms = Some(100.0);
        f.process = Some("Chrome".into());
        f
    }

    fn h(n: &str, v: &str) -> Header {
        Header { name: n.into(), value: v.into() }
    }

    fn json_body(t: &str) -> BodyPreview {
        BodyPreview {
            size: t.len() as u64,
            truncated: false,
            media_type: Some("application/json".into()),
            decoded_from: None,
            text: Some(t.into()),
            base64: None,
            spilled: false,
        }
    }

    fn plain(_: &str, v: &str) -> String {
        v.to_string()
    }

    #[test]
    fn a_session_summary_counts_and_ranks() {
        let mut flows: Vec<Flow> = (0..6).map(|i| flow(&format!("f{i}"), &format!("/users/{i}"), Some(200))).collect();
        flows[5].status = Some(503);
        flows.push(flow("e", "/pay", None));
        flows[6].state = FlowState::Error;
        flows[6].error = Some("connection refused".into());
        let s = summarize_session(&flows, &InsightReport { insights: Vec::new(), analyzed: 0, generation: 0.0 });
        assert_eq!(s.flows, 7);
        assert_eq!((s.status.ok_2xx, s.status.server_error_5xx, s.status.no_response), (5, 1, 1));
        assert_eq!(s.most_called[0].endpoint, "GET /users/:id");
        assert_eq!(s.most_called[0].calls, 6);
        let users = s.failing.iter().find(|r| r.endpoint == "GET /users/:id").unwrap();
        assert_eq!(users.failures.as_deref(), Some("1× 503"));
        assert_eq!(users.example_flow_id, "f5", "the failing call, not just the newest");
        assert_eq!(s.failing.len(), 2);
        assert_eq!(s.apps[0].failed, 2);
    }

    #[test]
    fn a_diff_names_what_matters_and_skips_noise() {
        let mut a = flow("a", "/x?page=1&q=s", Some(200));
        let mut b = flow("b", "/x?page=2&q=s", Some(401));
        a.request_headers = vec![h("Authorization", "Bearer one"), h("Date", "Mon"), h("Accept", "*/*")];
        b.request_headers = vec![h("Date", "Tue"), h("Accept", "*/*")];
        a.response_body = Some(json_body(r#"{"items":[{"id":1}],"total":1}"#));
        b.response_body = Some(json_body(r#"{"error":"unauthorized"}"#));
        let d = diff_flows(&a, &b, false, &plain);
        assert!(d.summary.iter().any(|s| s == "Status 200 → 401."), "{:?}", d.summary);
        assert!(d.summary.iter().any(|s| s == "Only a sends `authorization`."), "{:?}", d.summary);
        assert_eq!(d.query.len(), 1);
        assert_eq!(d.query[0].name, "page");
        assert!(d.request.headers.iter().all(|x| x.name != "date"), "Date is noise");
        let paths: Vec<&str> = d.response.body.json_changes.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(paths, vec!["$.error", "$.items", "$.total"]);
    }

    #[test]
    fn different_secrets_still_count_as_a_difference_when_hidden() {
        let mut a = flow("a", "/x", Some(200));
        let mut b = flow("b", "/x", Some(401));
        a.request_headers = vec![h("Authorization", "Bearer old")];
        b.request_headers = vec![h("Authorization", "Bearer new")];
        let hide = |k: &str, v: &str| {
            if nova_core::redact::is_secret_header(k) {
                nova_core::redact::header_value(k, v)
            } else {
                v.to_string()
            }
        };
        let d = diff_flows(&a, &b, false, &hide);
        assert_eq!(d.request.headers[0].a.as_deref(), Some("Bearer [redacted]"));
        assert!(d.summary.iter().any(|s| s.contains("`authorization` differs")), "{:?}", d.summary);
    }

    #[test]
    fn a_401_without_credentials_points_at_the_call_that_worked() {
        let mut ok = flow("ok", "/me", Some(200));
        ok.request_headers = vec![h("Authorization", "Bearer t"), h("Accept", "*/*")];
        ok.started_at = 0.0;
        let mut bad = flow("bad", "/me", Some(401));
        bad.started_at = 10.0;
        bad.request_headers = vec![h("Accept", "*/*")];
        bad.response_headers = vec![h("WWW-Authenticate", "Bearer error=\"invalid_token\"")];
        bad.response_body = Some(json_body(r#"{"error":{"message":"token missing"}}"#));
        let e = explain_failure(&bad, &[ok, bad.clone()]);
        assert_eq!(e.category, "client");
        assert!(e.diagnosis.contains("no credentials"), "{}", e.diagnosis);
        assert_eq!(e.compare_with.as_deref(), Some("ok"));
        assert_eq!(e.missing_vs_success, vec!["authorization"]);
        assert!(e.evidence.iter().any(|x| x.contains("token missing")), "{:?}", e.evidence);
    }

    #[test]
    fn transport_errors_are_read_from_the_cause() {
        let mut f = flow("t", "/", None);
        f.state = FlowState::Error;
        f.error = Some("upstream error: invalid peer certificate: UnknownIssuer".into());
        let e = explain_failure(&f, &[]);
        assert_eq!(e.category, "proxy");
        assert!(e.diagnosis.contains("certificate"), "{}", e.diagnosis);
    }

    #[test]
    fn a_cross_origin_200_without_allow_origin_is_a_cors_failure() {
        let mut f = flow("c", "/data", Some(200));
        f.request_headers = vec![h("Origin", "https://app.test")];
        let e = explain_failure(&f, &[]);
        assert_eq!(e.category, "cors");
        assert!(!e.failed, "the exchange itself succeeded");
    }
}
