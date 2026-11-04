//! Anti-pattern detection over captured traffic.
//!
//! Fixed rules, not a model: each check is cheap, explainable and testable, and
//! says what it saw in numbers. They judge traffic by its *shape* — timing,
//! status, headers, sizes — so they run over [`FlowStore::shells`] and never
//! inflate a body.
//!
//! Two choices keep the panel worth reading:
//!
//! * **Grouping is by app and host**, never across the whole capture. A proxy
//!   sees Chrome, Slack and the OS at once, so "the most-requested host is the
//!   app" (the usual HAR-analyser assumption) does not hold here.
//! * **Flows that cannot be judged are left out**: ones still in flight,
//!   tunneled ones (no headers), WebSockets, replays and NovaProxy's own
//!   traffic. Responses a rule or a breakpoint produced are not the origin's,
//!   so the checks about responses skip those as well.
//!
//! [`FlowStore::shells`]: crate::flowstore::FlowStore::shells

use std::collections::{BTreeMap, BTreeSet, HashMap};

use nova_proto::{Flow, FlowState, Header, Insight, InsightReport, InsightSeverity};

/// Flow ids carried per insight. `count` keeps the real number; the list is
/// for "show me these", and nobody reads past a few hundred rows of one finding.
const MAX_IDS: usize = 500;

/// A burst: this many calls to one endpoint inside [`BURST_WINDOW_MS`].
const BURST_MIN: usize = 5;
const BURST_WINDOW_MS: f64 = 2_000.0;
/// Polling: at least this many GETs, at an interval this regular (coefficient
/// of variation), between these bounds. Faster than a second is a burst;
/// slower than five minutes is a schedule, not polling.
const POLL_MIN: usize = 6;
const POLL_MAX_CV: f64 = 0.25;
const POLL_INTERVAL_MS: (f64, f64) = (1_000.0, 300_000.0);
/// A retry storm: this many failures of one endpoint inside the window.
const RETRY_MIN: usize = 3;
const RETRY_WINDOW_MS: f64 = 10_000.0;
/// Error rate: judged once a host has this many requests from one app.
const ERROR_RATE_MIN_FLOWS: usize = 10;
const ERROR_RATE_SHARE: f64 = 0.25;
/// Text responses smaller than this gain little from compression.
const COMPRESS_MIN_BYTES: u64 = 10 * 1024;
const LARGE_PAYLOAD_BYTES: u64 = 1024 * 1024;
/// Server wait (TTFB minus connection setup) that counts as slow.
const SLOW_MIN_SAMPLES: usize = 3;
const SLOW_WAIT_MS: f64 = 1_000.0;
const VERY_SLOW_WAIT_MS: f64 = 3_000.0;
/// Preflights per app and host before their cost is worth mentioning.
const PREFLIGHT_MIN: usize = 5;
/// A redirect is followed only to a request that starts this soon after it.
const REDIRECT_FOLLOW_MS: f64 = 10_000.0;
const REDIRECT_CHAIN_MIN: usize = 3;
/// JPEG/PNG/GIF this large has a cheaper modern encoding; any image this large
/// is heavy whatever the format.
const IMAGE_LEGACY_BYTES: u64 = 100 * 1024;
const IMAGE_HEAVY_BYTES: u64 = 500 * 1024;
/// A fingerprinted asset cached for less than this is re-downloaded for nothing.
const SHORT_CACHE_SECS: u64 = 7 * 24 * 3600;
/// HTML this large is slow to parse before anything renders.
const LARGE_HTML_BYTES: u64 = 500 * 1024;
/// One request waiting this long is worth a finding on its own.
const LONE_SLOW_WAIT_MS: f64 = 10_000.0;
/// Empty 200s from one endpoint before they count as a pattern.
const EMPTY_MIN: usize = 3;
/// Connection-setup phases whose median counts as slow, and the samples needed.
const SLOW_DNS_MS: f64 = 200.0;
const SLOW_CONNECT_MS: f64 = 300.0;
const SLOW_TLS_MS: f64 = 500.0;
const CONNECT_MIN_SAMPLES: usize = 2;
/// CDN-served static assets per host before the miss rate is judged.
const CDN_MIN_SAMPLES: usize = 10;
const CDN_MISS_SHARE: f64 = 0.5;

/// What the analysis needs to know that the flows do not carry.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Latency NovaProxy itself adds (Network Conditions), subtracted before a
    /// server is called slow — otherwise throttling makes every endpoint "slow".
    pub added_latency_ms: f64,
}

/// Run every check over `flows` (any order). `generation` is left at zero for
/// the caller to fill in.
pub fn analyze(flows: &[Flow], opts: &Options) -> InsightReport {
    let mut judged: Vec<&Flow> = flows.iter().filter(|f| judgeable(f)).collect();
    judged.sort_by(|a, b| a.started_at.total_cmp(&b.started_at));

    let mut out = Vec::new();
    burst(&judged, &mut out);
    polling(&judged, &mut out);
    retry_storm(&judged, &mut out);
    rate_limited(&judged, &mut out);
    error_rate(&judged, &mut out);
    uncompressed(&judged, &mut out);
    large_payload(&judged, &mut out);
    static_uncacheable(&judged, &mut out);
    redirect_chain(&judged, &mut out);
    slow_endpoint(&judged, opts, &mut out);
    cors_preflight(&judged, &mut out);
    token_in_url(&judged, &mut out);
    insecure_cookies(&judged, &mut out);
    plaintext_credentials(&judged, &mut out);
    image_unoptimized(&judged, &mut out);
    short_cache(&judged, &mut out);
    mixed_content(&judged, &mut out);
    missing_security_headers(&judged, &mut out);
    broken_asset(&judged, &mut out);
    empty_response(&judged, &mut out);
    slow_connect(&judged, &mut out);
    cdn_miss(&judged, &mut out);

    out.sort_by(|a, b| b.severity.cmp(&a.severity).then(b.count.cmp(&a.count)).then(a.id.cmp(&b.id)));
    InsightReport { insights: out, analyzed: judged.len() as u32, generation: 0.0 }
}

/* --------------------------------- filters --------------------------------- */

/// Whether a flow tells us anything about the app that sent it.
fn judgeable(f: &Flow) -> bool {
    !f.internal
        && !f.tunneled
        && !f.resent
        && !f.is_websocket
        && matches!(f.state, FlowState::Completed | FlowState::Error)
}

/// Whether the response came from the origin, untouched by a rule or a person.
fn origin_response(f: &Flow) -> bool {
    !f.edits.rule && !f.edits.breakpoint && f.status.is_some()
}

fn failed(f: &Flow) -> bool {
    f.state == FlowState::Error || f.error.is_some() || f.status.is_some_and(|s| s >= 400)
}

/// Loopback and `*.localhost`: dev servers, where caching, compression and
/// plain HTTP are normal and flagging them is noise.
fn is_loopback(host: &str) -> bool {
    let h = bare_host(host);
    h == "localhost" || h.ends_with(".localhost") || h == "::1" || h.starts_with("127.")
}

/// Host without its port (and without IPv6 brackets).
fn bare_host(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    match host.rsplit_once(':') {
        Some((h, port)) if port.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => host,
    }
}

/* --------------------------------- headers --------------------------------- */

fn header<'a>(headers: &'a [Header], name: &str) -> Option<&'a str> {
    headers.iter().find(|h| h.name.eq_ignore_ascii_case(name)).map(|h| h.value.as_str())
}

fn headers_named<'a>(headers: &'a [Header], name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    headers.iter().filter(move |h| h.name.eq_ignore_ascii_case(name)).map(|h| h.value.as_str())
}

/// Lowercase media type without parameters.
fn media_type(f: &Flow) -> String {
    let raw = f
        .content_type
        .as_deref()
        .or_else(|| header(&f.response_headers, "content-type"))
        .unwrap_or("");
    raw.split(';').next().unwrap_or("").trim().to_ascii_lowercase()
}

fn is_compressible(media: &str) -> bool {
    media.starts_with("text/")
        || media.ends_with("+json")
        || media.ends_with("+xml")
        || matches!(
            media,
            "application/json"
                | "application/javascript"
                | "application/x-javascript"
                | "application/xml"
                | "application/graphql-response+json"
                | "application/x-ndjson"
                | "image/svg+xml"
                | "application/wasm"
                | "font/ttf"
                | "font/otf"
                | "application/vnd.ms-fontobject"
        )
}

fn is_data(media: &str) -> bool {
    media == "application/json"
        || media.ends_with("+json")
        || media == "application/xml"
        || media == "text/xml"
        || media == "application/x-ndjson"
}

/// Scripts, styles, fonts and images: what a browser could keep for a while.
fn is_static(f: &Flow, media: &str) -> bool {
    if media.starts_with("image/")
        || media.starts_with("font/")
        || matches!(media, "text/css" | "application/javascript" | "text/javascript" | "application/x-javascript")
    {
        return true;
    }
    let path = path_only(&f.path).to_ascii_lowercase();
    const EXT: [&str; 12] =
        [".js", ".mjs", ".css", ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".woff", ".woff2", ".ico"];
    EXT.iter().any(|e| path.ends_with(e))
}

/* -------------------------------- endpoints -------------------------------- */

fn path_only(path: &str) -> &str {
    path.split(['?', '#']).next().unwrap_or(path)
}

fn query_of(path: &str) -> &str {
    path.split_once('?').map(|(_, q)| q.split('#').next().unwrap_or(q)).unwrap_or("")
}

/// `/users/42/posts/9f1c…` → `/users/:id/posts/:hash`, so calls that differ only
/// by an id group together.
pub fn normalize_path(path: &str) -> String {
    let path = path_only(path);
    let segs: Vec<String> = path.split('/').map(normalize_segment).collect();
    let joined = segs.join("/");
    if joined.is_empty() { "/".into() } else { joined }
}

fn normalize_segment(seg: &str) -> String {
    let b = seg.as_bytes();
    if seg.is_empty() {
        return String::new();
    }
    if b.iter().all(u8::is_ascii_digit) {
        return ":id".into();
    }
    let uuid = b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        });
    if uuid {
        return ":uuid".into();
    }
    if b.len() >= 12 && b.iter().all(u8::is_ascii_hexdigit) && b.iter().any(u8::is_ascii_digit) {
        return ":hash".into();
    }
    let tokenish = b.len() >= 20
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-' || *c == b'_')
        && b.iter().any(u8::is_ascii_digit)
        && b.iter().any(u8::is_ascii_alphabetic);
    if tokenish {
        return ":token".into();
    }
    seg.to_string()
}

/// What "the same call" means for a flow, as people would name it.
///
/// MCP calls are named by JSON-RPC method and tool — they all post to one URL.
/// GraphQL calls are named by operation; one with no visible name returns
/// `None`, because a single `/graphql` path for a whole app would make every
/// pattern check fire on ordinary traffic.
fn endpoint(f: &Flow) -> Option<String> {
    if let Some(m) = &f.mcp {
        let method = m.method.as_deref().unwrap_or("message");
        return Some(match &m.tool {
            Some(tool) => format!("MCP {method} → {tool}"),
            None => format!("MCP {method}"),
        });
    }
    let path = path_only(&f.path);
    if path.ends_with("/graphql") || path.ends_with("/gql") {
        let op = query_param(&f.path, "operationName")
            .or_else(|| header(&f.request_headers, "x-apollo-operation-name").map(str::to_string))
            .filter(|s| !s.is_empty())?;
        return Some(format!("GraphQL {op}"));
    }
    Some(format!("{} {}", f.method, normalize_path(&f.path)))
}

fn query_params(path: &str) -> impl Iterator<Item = (&str, &str)> {
    query_of(path)
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| p.split_once('=').unwrap_or((p, "")))
}

fn query_param(path: &str, name: &str) -> Option<String> {
    query_params(path).find(|(k, _)| *k == name).map(|(_, v)| v.to_string())
}

/* --------------------------------- grouping --------------------------------- */

/// `(app, host, endpoint)`; `app` is `""` when unattributed.
type Key = (String, String, String);

fn app_of(f: &Flow) -> String {
    f.process.clone().unwrap_or_default()
}

/// Group by app, host and endpoint, keeping time order inside each group.
fn by_endpoint<'a>(flows: &[&'a Flow], keep: impl Fn(&Flow) -> bool) -> BTreeMap<Key, Vec<&'a Flow>> {
    let mut groups: BTreeMap<Key, Vec<&Flow>> = BTreeMap::new();
    for f in flows.iter().copied().filter(|f| keep(f)) {
        if let Some(ep) = endpoint(f) {
            groups.entry((app_of(f), f.host.clone(), ep)).or_default().push(f);
        }
    }
    groups
}

fn by_host<'a>(flows: &[&'a Flow], keep: impl Fn(&Flow) -> bool) -> BTreeMap<(String, String), Vec<&'a Flow>> {
    let mut groups: BTreeMap<(String, String), Vec<&Flow>> = BTreeMap::new();
    for f in flows.iter().copied().filter(|f| keep(f)) {
        groups.entry((app_of(f), f.host.clone())).or_default().push(f);
    }
    groups
}

/// Like [`by_host`], for checks that work something out per flow: `judge` runs
/// once per flow, and what it found travels with the flow.
fn by_host_with<'a, T>(
    flows: &[&'a Flow],
    judge: impl Fn(&Flow) -> Option<T>,
) -> BTreeMap<(String, String), Vec<(&'a Flow, T)>> {
    let mut groups: BTreeMap<(String, String), Vec<(&Flow, T)>> = BTreeMap::new();
    for f in flows.iter().copied() {
        if let Some(t) = judge(f) {
            groups.entry((app_of(f), f.host.clone())).or_default().push((f, t));
        }
    }
    groups
}

struct Finding<'a> {
    rule: &'static str,
    /// What the rule grouped by, beyond the rule itself; makes the id stable.
    key: String,
    severity: InsightSeverity,
    title: String,
    detail: String,
    advice: &'static str,
    flows: Vec<&'a Flow>,
}

impl Finding<'_> {
    fn into_insight(self) -> Insight {
        let first = self.flows.first().map(|f| f.process.clone()).unwrap_or_default();
        let app = if self.flows.iter().all(|f| f.process == first) { first } else { None };
        let host = self.flows.first().map(|f| f.host.clone()).unwrap_or_default();
        let skip = self.flows.len().saturating_sub(MAX_IDS);
        Insight {
            id: format!("{}:{}", self.rule, self.key),
            rule: self.rule.into(),
            severity: self.severity,
            title: self.title,
            detail: self.detail,
            advice: self.advice.into(),
            app,
            host,
            flow_ids: self.flows.iter().skip(skip).map(|f| f.id.clone()).collect(),
            count: self.flows.len() as u32,
            last_seen: self.flows.iter().map(|f| f.started_at).fold(0.0, f64::max),
        }
    }
}

fn push(out: &mut Vec<Insight>, f: Finding<'_>) {
    out.push(f.into_insight());
}

fn key3((app, host, ep): &Key) -> String {
    format!("{app}|{host}|{ep}")
}

fn key2((app, host): &(String, String)) -> String {
    format!("{app}|{host}")
}

/// Longest run of `times` (sorted) that fits inside `window`, and every index
/// that belongs to some run of at least `min`.
fn dense_runs(times: &[f64], window: f64, min: usize) -> (usize, std::ops::Range<usize>, Vec<usize>) {
    let mut best = (0, 0..0);
    let mut members = Vec::new();
    let mut next_unmarked = 0;
    let mut j = 0;
    for i in 0..times.len() {
        while times[i] - times[j] > window {
            j += 1;
        }
        let size = i - j + 1;
        if size > best.0 {
            best = (size, j..i + 1);
        }
        if size >= min {
            for k in next_unmarked.max(j)..=i {
                members.push(k);
            }
            next_unmarked = i + 1;
        }
    }
    (best.0, best.1, members)
}

/* ---------------------------------- rules ---------------------------------- */

/// The same call fired many times in a moment: N+1 loops, missing de-duplication.
fn burst(flows: &[&Flow], out: &mut Vec<Insight>) {
    for (key, group) in by_endpoint(flows, |f| f.method != "OPTIONS") {
        let times: Vec<f64> = group.iter().map(|f| f.started_at).collect();
        let (max, best, members) = dense_runs(&times, BURST_WINDOW_MS, BURST_MIN);
        if max < BURST_MIN {
            continue;
        }
        let run = &group[best];
        // One after another — each waited for the last to finish — is the
        // expensive shape: the wall-clock cost is the sum, not the slowest.
        let serial = run.windows(2).all(|w| {
            w[0].duration_ms.is_some_and(|d| w[1].started_at + 1.0 >= w[0].started_at + d)
        });
        let distinct: BTreeSet<&str> = run.iter().map(|f| f.url.as_str()).collect();
        let what = if distinct.len() == 1 {
            "the identical URL".to_string()
        } else {
            format!("{} different URLs", distinct.len())
        };
        let shape = if serial { "one after another" } else { "in parallel" };
        let advice = if distinct.len() == 1 {
            "The same request repeats: de-duplicate it in the client (share one in-flight request, or cache the result) instead of asking again."
        } else if serial {
            "Looks like an N+1 loop: each call waits for the previous one. Fetch the items in one batched request, or at least issue them concurrently."
        } else {
            "Many calls to one endpoint at once. A batch endpoint (or a list call with the ids) usually replaces this fan-out."
        };
        push(out, Finding {
            rule: "burst",
            key: key3(&key),
            severity: if max >= 20 { InsightSeverity::High } else { InsightSeverity::Warning },
            title: format!("{} called {max}× within {} s", key.2, BURST_WINDOW_MS / 1000.0),
            detail: format!("{max} calls to {what}, {shape}; {} calls in bursts overall.", members.len()),
            advice,
            flows: members.iter().map(|&i| group[i]).collect(),
        });
    }
}

/// A GET on a fixed timer.
fn polling(flows: &[&Flow], out: &mut Vec<Insight>) {
    for (key, group) in by_endpoint(flows, |f| f.method == "GET" && f.mcp.is_none()) {
        if group.len() < POLL_MIN {
            continue;
        }
        let gaps: Vec<f64> = group.windows(2).map(|w| w[1].started_at - w[0].started_at).collect();
        let mean = gaps.iter().sum::<f64>() / gaps.len() as f64;
        if mean < POLL_INTERVAL_MS.0 || mean > POLL_INTERVAL_MS.1 {
            continue;
        }
        let sd = (gaps.iter().map(|g| (g - mean).powi(2)).sum::<f64>() / gaps.len() as f64).sqrt();
        if sd / mean > POLL_MAX_CV {
            continue;
        }
        let unchanged = group.iter().filter(|f| f.status == Some(304)).count();
        let detail = if unchanged > 0 {
            format!("{} requests at a steady interval; {unchanged} came back 304 Not Modified.", group.len())
        } else {
            format!("{} requests at a steady interval, none conditional (no 304s).", group.len())
        };
        push(out, Finding {
            rule: "polling",
            key: key3(&key),
            severity: InsightSeverity::Info,
            title: format!("{} polled every ~{}", key.2, fmt_ms(mean)),
            detail,
            advice: "If the data rarely changes, push it instead (SSE or WebSocket), back off while nothing changes, or make the poll conditional with ETag / If-None-Match.",
            flows: group,
        });
    }
}

/// Failures retried quickly, again and again.
fn retry_storm(flows: &[&Flow], out: &mut Vec<Insight>) {
    for (key, group) in by_endpoint(flows, |f| {
        f.state == FlowState::Error || f.error.is_some() || f.status.is_some_and(|s| s == 429 || s >= 500)
    }) {
        let times: Vec<f64> = group.iter().map(|f| f.started_at).collect();
        let (max, _, members) = dense_runs(&times, RETRY_WINDOW_MS, RETRY_MIN);
        if max < RETRY_MIN {
            continue;
        }
        let statuses = status_breakdown(members.iter().map(|&i| group[i]));
        push(out, Finding {
            rule: "retry-storm",
            key: key3(&key),
            severity: if max >= 10 { InsightSeverity::High } else { InsightSeverity::Warning },
            title: format!("{} failed {max}× within {} s", key.2, RETRY_WINDOW_MS / 1000.0),
            detail: format!("Failures: {statuses}. Retrying a failing server this fast adds load just when it can least take it."),
            advice: "Retry with exponential backoff and jitter, honour Retry-After, and stop (circuit-break) after a few attempts.",
            flows: members.iter().map(|&i| group[i]).collect(),
        });
    }
}

/// 429s, and whether the server said when to come back.
fn rate_limited(flows: &[&Flow], out: &mut Vec<Insight>) {
    for (key, group) in by_host(flows, |f| f.status == Some(429)) {
        let without = group.iter().filter(|f| header(&f.response_headers, "retry-after").is_none()).count();
        let detail = match without {
            0 => format!("{} responses were 429 Too Many Requests, each with Retry-After.", group.len()),
            n if n == group.len() => {
                format!("{n} responses were 429 Too Many Requests, none with Retry-After — the client has to guess when to retry.")
            }
            n => format!("{} responses were 429 Too Many Requests; {n} without Retry-After.", group.len()),
        };
        push(out, Finding {
            rule: "rate-limited",
            key: key2(&key),
            severity: InsightSeverity::Warning,
            title: format!("Rate-limited by {}", key.1),
            detail,
            advice: "Slow the client down: cache, batch, or space out requests, and wait as long as Retry-After asks before trying again.",
            flows: group,
        });
    }
}

/// A host where a large share of one app's requests fail.
fn error_rate(flows: &[&Flow], out: &mut Vec<Insight>) {
    for (key, group) in by_host(flows, |_| true) {
        if group.len() < ERROR_RATE_MIN_FLOWS {
            continue;
        }
        let bad: Vec<&Flow> = group.iter().copied().filter(|f| failed(f)).collect();
        let share = bad.len() as f64 / group.len() as f64;
        if share < ERROR_RATE_SHARE {
            continue;
        }
        push(out, Finding {
            rule: "error-rate",
            key: key2(&key),
            severity: if share >= 0.5 { InsightSeverity::High } else { InsightSeverity::Warning },
            title: format!("{:.0}% of requests to {} fail", share * 100.0, key.1),
            detail: format!(
                "{} of {} requests failed: {}.",
                bad.len(),
                group.len(),
                status_breakdown(bad.iter().copied())
            ),
            advice: "Open the failing flows: repeated 401/403 usually means an expired token being reused, 404 a stale URL, 5xx the server.",
            flows: bad,
        });
    }
}

/// Sizeable text sent without Content-Encoding.
fn uncompressed(flows: &[&Flow], out: &mut Vec<Insight>) {
    for (key, group) in by_host(flows, |f| {
        origin_response(f)
            && !is_loopback(&f.host)
            && f.status.is_some_and(|s| (200..300).contains(&s))
            && f.response_size >= COMPRESS_MIN_BYTES
            && is_compressible(&media_type(f))
            && header(&f.response_headers, "content-encoding").is_none_or(|e| e.eq_ignore_ascii_case("identity"))
    }) {
        let total: u64 = group.iter().map(|f| f.response_size).sum();
        let largest = group.iter().map(|f| f.response_size).max().unwrap_or(0);
        push(out, Finding {
            rule: "uncompressed",
            key: key2(&key),
            severity: if total >= LARGE_PAYLOAD_BYTES { InsightSeverity::Warning } else { InsightSeverity::Info },
            title: format!("{} text responses from {} sent uncompressed", group.len(), key.1),
            detail: format!("{} in total, largest {}. Text like this typically shrinks 70–90% with gzip or brotli.", fmt_bytes(total), fmt_bytes(largest)),
            advice: "Enable gzip or brotli on the server or CDN for text types (JSON, HTML, JS, CSS, SVG).",
            flows: group,
        });
    }
}

/// API responses in the megabytes.
fn large_payload(flows: &[&Flow], out: &mut Vec<Insight>) {
    let html = |f: &Flow| media_type(f) == "text/html";
    for (key, group) in by_endpoint(flows, |f| {
        origin_response(f)
            && ((is_data(&media_type(f)) && decoded_size(f) >= LARGE_PAYLOAD_BYTES)
                || (html(f) && decoded_size(f) >= LARGE_HTML_BYTES))
    }) {
        let largest = group.iter().map(|f| decoded_size(f)).max().unwrap_or(0);
        let is_html = group.iter().all(|f| html(f));
        push(out, Finding {
            rule: "large-payload",
            key: key3(&key),
            severity: InsightSeverity::Info,
            title: if is_html {
                format!("{} returns up to {} of HTML", key.2, fmt_bytes(largest))
            } else {
                format!("{} returns up to {} of data", key.2, fmt_bytes(largest))
            },
            detail: if is_html {
                format!("{} pages of 500 KB or more. The browser parses all of it before the page settles; inlined data and scripts are the usual cause.", group.len())
            } else {
                format!("{} responses of 1 MB or more. Parsing that much JSON blocks the client, too.", group.len())
            },
            advice: if is_html {
                "Move inlined JSON state, SVG sprites and scripts out into cacheable files, paginate long lists, or stream the page."
            } else {
                "Paginate, let the client pick the fields it needs, or stream the result."
            },
            flows: group,
        });
    }
}

/// Decoded response size: the preview knows it, the wire size is the fallback.
fn decoded_size(f: &Flow) -> u64 {
    f.response_body.as_ref().map(|b| b.size).unwrap_or(f.response_size).max(f.response_size)
}

/// Static assets the browser cannot keep.
fn static_uncacheable(flows: &[&Flow], out: &mut Vec<Insight>) {
    for (key, group) in by_host(flows, |f| {
        origin_response(f) && f.method == "GET" && f.status == Some(200) && !is_loopback(&f.host) && is_static(f, &media_type(f))
    }) {
        let mut no_store = Vec::new();
        let mut no_headers = Vec::new();
        for f in group {
            let h = &f.response_headers;
            let cc = header(h, "cache-control").unwrap_or("").to_ascii_lowercase();
            // `no-cache` is fine: it means "revalidate", which with a validator is
            // cheap. `no-store` forbids keeping it at all.
            if cc.contains("no-store") {
                no_store.push(f);
            } else if cc.is_empty()
                && header(h, "expires").is_none()
                && header(h, "etag").is_none()
                && header(h, "last-modified").is_none()
            {
                no_headers.push(f);
            }
        }
        let n = no_store.len() + no_headers.len();
        if n == 0 {
            continue;
        }
        let mut parts = Vec::new();
        if !no_store.is_empty() {
            parts.push(format!("{} marked no-store", no_store.len()));
        }
        if !no_headers.is_empty() {
            parts.push(format!("{} with no Cache-Control, Expires, ETag or Last-Modified", no_headers.len()));
        }
        let mut all = no_store;
        all.extend(no_headers);
        all.sort_by(|a, b| a.started_at.total_cmp(&b.started_at));
        push(out, Finding {
            rule: "static-uncacheable",
            key: key2(&key),
            severity: InsightSeverity::Info,
            title: format!("{n} static assets from {} can't be cached", key.1),
            detail: format!("{}. Every page load downloads them again.", parts.join("; ")),
            advice: "Serve fingerprinted assets with `Cache-Control: public, max-age=31536000, immutable`, and give the rest an ETag so revisits cost a 304.",
            flows: all,
        });
    }
}

/// Redirects followed through several hops, or in a circle.
fn redirect_chain(flows: &[&Flow], out: &mut Vec<Insight>) {
    let mut by_url: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, f) in flows.iter().enumerate() {
        by_url.entry(canonical_url(&f.url)).or_default().push(i);
    }
    let mut used = vec![false; flows.len()];
    for start in 0..flows.len() {
        if used[start] || !is_redirect(flows[start]) {
            continue;
        }
        let mut chain = vec![start];
        let mut seen = BTreeSet::from([canonical_url(&flows[start].url)]);
        let mut looped = false;
        let mut cur = start;
        while is_redirect(flows[cur]) {
            let Some(next_url) = header(&flows[cur].response_headers, "location").and_then(|l| resolve(&flows[cur].url, l)) else {
                break;
            };
            let (from, app) = (flows[cur].started_at, &flows[cur].process);
            let next = by_url.get(&next_url).and_then(|ids| {
                ids.iter().copied().find(|&i| {
                    !used[i] && !chain.contains(&i) && flows[i].process == *app
                        && flows[i].started_at >= from && flows[i].started_at - from <= REDIRECT_FOLLOW_MS
                })
            });
            let Some(next) = next else { break };
            chain.push(next);
            if !seen.insert(next_url) {
                looped = true;
                break;
            }
            cur = next;
        }
        let hops = chain.iter().filter(|&&i| is_redirect(flows[i])).count();
        if !looped && hops < REDIRECT_CHAIN_MIN {
            continue;
        }
        for &i in &chain {
            used[i] = true;
        }
        let first = flows[start];
        let detail = chain
            .iter()
            .map(|&i| format!("{} {}", flows[i].status.unwrap_or(0), short_url(&flows[i].url)))
            .collect::<Vec<_>>()
            .join(" → ");
        push(out, Finding {
            rule: if looped { "redirect-loop" } else { "redirect-chain" },
            key: format!("{}|{}", app_of(first), canonical_url(&first.url)),
            severity: if looped { InsightSeverity::High } else { InsightSeverity::Warning },
            title: if looped {
                format!("Redirect loop starting at {}", short_url(&first.url))
            } else {
                format!("{hops} redirects before {} answers", short_url(&first.url))
            },
            detail,
            advice: if looped {
                "The redirects come back to a URL already visited, so the client never gets an answer. Check the conditions each hop redirects on (often a cookie or auth check that never succeeds)."
            } else {
                "Each hop is a full round trip. Link to the final URL directly, and collapse http→https / www / trailing-slash redirects into one."
            },
            flows: chain.iter().map(|&i| flows[i]).collect(),
        });
    }
}

fn is_redirect(f: &Flow) -> bool {
    f.status.is_some_and(|s| matches!(s, 301 | 302 | 303 | 307 | 308))
}

/// `scheme://host/path?query` without a default port or a fragment.
fn canonical_url(url: &str) -> String {
    let url = url.split('#').next().unwrap_or(url);
    let Some((scheme, rest)) = url.split_once("://") else { return url.to_string() };
    let (auth, tail) = rest.find('/').map(|i| rest.split_at(i)).unwrap_or((rest, "/"));
    let scheme = scheme.to_ascii_lowercase();
    let mut auth = auth.to_ascii_lowercase();
    let default_port = if scheme == "https" { ":443" } else { ":80" };
    if auth.ends_with(default_port) {
        auth.truncate(auth.len() - default_port.len());
    }
    format!("{scheme}://{auth}{tail}")
}

/// Resolve a `Location` against the URL that sent it.
fn resolve(base: &str, location: &str) -> Option<String> {
    let location = location.trim();
    if location.contains("://") {
        return Some(canonical_url(location));
    }
    let (scheme, rest) = base.split_once("://")?;
    if let Some(net) = location.strip_prefix("//") {
        return Some(canonical_url(&format!("{scheme}://{net}")));
    }
    let auth_end = rest.find('/').unwrap_or(rest.len());
    let origin = format!("{scheme}://{}", &rest[..auth_end]);
    if location.starts_with('/') {
        return Some(canonical_url(&format!("{origin}{location}")));
    }
    let base_path = path_only(&rest[auth_end..]);
    let dir = base_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    Some(canonical_url(&format!("{origin}{dir}/{location}")))
}

/// The server, not the network, is what takes the time.
fn slow_endpoint(flows: &[&Flow], opts: &Options, out: &mut Vec<Insight>) {
    let wait = |f: &Flow| {
        let t = &f.timings;
        let setup = t.dns_ms.unwrap_or(0.0) + t.connect_ms.unwrap_or(0.0) + t.tls_ms.unwrap_or(0.0) + t.request_ms.unwrap_or(0.0);
        t.ttfb_ms.map(|ttfb| (ttfb - setup - opts.added_latency_ms).max(0.0))
    };
    for (key, group) in by_endpoint(flows, |f| origin_response(f) && !f.edits.script && f.timings.ttfb_ms.is_some()) {
        let mut waits: Vec<f64> = group.iter().filter_map(|f| wait(f)).collect();
        waits.sort_by(f64::total_cmp);
        let median = waits[waits.len() / 2];
        let max = waits.last().copied().unwrap_or(median);
        if group.len() < SLOW_MIN_SAMPLES {
            // Too few calls for a median, but one that hung this long is a
            // finding by itself (a timeout waiting to happen).
            if max >= LONE_SLOW_WAIT_MS {
                let slow: Vec<&Flow> = group.iter().copied().filter(|f| wait(f).is_some_and(|w| w >= LONE_SLOW_WAIT_MS)).collect();
                push(out, Finding {
                    rule: "slow-endpoint",
                    key: key3(&key),
                    severity: InsightSeverity::Warning,
                    title: format!("{} waited {} for the server", key.2, fmt_ms(max)),
                    detail: format!(
                        "{} of {} calls waited {} or more before the first response byte. Most clients time out around 30 s.",
                        slow.len(),
                        group.len(),
                        fmt_ms(LONE_SLOW_WAIT_MS)
                    ),
                    advice: "Find what the server waits on (a lock, a cold cache, an upstream call); for work that is slow by nature, answer 202 and let the client poll or subscribe for the result.",
                    flows: slow,
                });
            }
            continue;
        }
        if median < SLOW_WAIT_MS {
            continue;
        }
        push(out, Finding {
            rule: "slow-endpoint",
            key: key3(&key),
            severity: if median >= VERY_SLOW_WAIT_MS { InsightSeverity::Warning } else { InsightSeverity::Info },
            title: format!("{} waits ~{} for the server", key.2, fmt_ms(median)),
            detail: format!(
                "Median server wait {} over {} calls (slowest {}). Connection setup is already excluded, so this is time spent before the first response byte.",
                fmt_ms(median),
                group.len(),
                fmt_ms(max)
            ),
            advice: "The time is on the server: look at the queries or upstream calls behind this endpoint, or cache its result.",
            flows: group,
        });
    }
}

/// CORS preflights the browser is not allowed to remember.
fn cors_preflight(flows: &[&Flow], out: &mut Vec<Insight>) {
    for (key, group) in by_host(flows, |f| {
        f.method == "OPTIONS" && header(&f.request_headers, "access-control-request-method").is_some()
    }) {
        if group.len() < PREFLIGHT_MIN {
            continue;
        }
        let max_age = group
            .iter()
            .filter_map(|f| header(&f.response_headers, "access-control-max-age"))
            .filter_map(|v| v.trim().parse::<i64>().ok())
            .min();
        let urls: BTreeSet<&str> = group.iter().map(|f| f.url.as_str()).collect();
        let cache = match max_age {
            None => "No Access-Control-Max-Age, so browsers keep each answer for 5 seconds at most.".to_string(),
            Some(s) if s < 600 => format!("Access-Control-Max-Age is only {s} s."),
            // Cached properly; the count comes from many distinct URLs.
            Some(_) => "Answers are cached, but the cache is per URL, so every new URL pays again.".to_string(),
        };
        push(out, Finding {
            rule: "cors-preflight",
            key: key2(&key),
            severity: InsightSeverity::Info,
            title: format!("{} CORS preflights to {}", group.len(), key.1),
            detail: format!("{} preflights for {} distinct URLs. {cache} Each one is an extra round trip before the real request.", group.len(), urls.len()),
            advice: "Send Access-Control-Max-Age (e.g. 7200), avoid custom headers or content types that force a preflight, or serve the API from the same origin.",
            flows: group,
        });
    }
}

/// Credentials in query strings end up in logs, history and Referer headers.
fn token_in_url(flows: &[&Flow], out: &mut Vec<Insight>) {
    const NAMES: [&str; 17] = [
        "access_token", "id_token", "refresh_token", "token", "auth", "auth_token", "api_key", "apikey",
        "api-key", "key", "client_secret", "secret", "password", "passwd", "pwd", "session", "sessionid",
    ];
    let leaked = |f: &Flow| -> Option<Vec<String>> {
        if is_loopback(&f.host) {
            return None;
        }
        let names: Vec<String> = query_params(&f.path)
            .filter(|(k, v)| (v.len() >= 8 && NAMES.iter().any(|n| k.eq_ignore_ascii_case(n))) || looks_like_jwt(v))
            .map(|(k, _)| k.to_ascii_lowercase())
            .collect();
        (!names.is_empty()).then_some(names)
    };
    for (key, hits) in by_host_with(flows, leaked) {
        let names: BTreeSet<&str> = hits.iter().flat_map(|(_, n)| n.iter().map(String::as_str)).collect();
        let group: Vec<&Flow> = hits.iter().map(|(f, _)| *f).collect();
        push(out, Finding {
            rule: "token-in-url",
            key: key2(&key),
            severity: InsightSeverity::High,
            title: format!("Credentials in URLs sent to {}", key.1),
            detail: format!(
                "{} requests carry {} in the query string.",
                group.len(),
                names.iter().map(|n| format!("`{n}`")).collect::<Vec<_>>().join(", ")
            ),
            advice: "Send credentials in the Authorization header or the request body. URLs are written to server and proxy logs, browser history and Referer headers.",
            flows: group,
        });
    }
}

fn looks_like_jwt(v: &str) -> bool {
    v.starts_with("eyJ") && v.len() > 30 && v.matches('.').count() == 2
}

/// Cookies set over HTTPS without the attributes that keep them there.
fn insecure_cookies(flows: &[&Flow], out: &mut Vec<Insight>) {
    let problems = |f: &Flow| -> Option<Vec<(String, Vec<&'static str>)>> {
        if f.scheme != "https" || is_loopback(&f.host) || !origin_response(f) {
            return None;
        }
        let found: Vec<_> = headers_named(&f.response_headers, "set-cookie")
            .filter_map(|c| {
                let mut parts = c.split(';');
                let name = parts.next()?.split('=').next()?.trim();
                let (mut secure, mut http_only) = (false, false);
                for attr in parts {
                    let attr = attr.split('=').next().unwrap_or("").trim();
                    secure |= attr.eq_ignore_ascii_case("secure");
                    http_only |= attr.eq_ignore_ascii_case("httponly");
                }
                let mut missing = Vec::new();
                if !secure {
                    missing.push("Secure");
                }
                if !http_only {
                    let lname = name.to_ascii_lowercase();
                    if ["sess", "sid", "auth", "token", "jwt", "login"].iter().any(|s| lname.contains(s)) {
                        missing.push("HttpOnly");
                    }
                }
                (!missing.is_empty()).then(|| (name.to_string(), missing))
            })
            .collect();
        (!found.is_empty()).then_some(found)
    };
    for (key, hits) in by_host_with(flows, problems) {
        let mut cookies: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (_, found) in &hits {
            for (name, missing) in found {
                let seen = cookies.entry(name).or_default();
                for m in missing {
                    if !seen.contains(m) {
                        seen.push(m);
                    }
                }
            }
        }
        let group: Vec<&Flow> = hits.iter().map(|(f, _)| *f).collect();
        let list = cookies
            .iter()
            .map(|(n, m)| format!("`{n}` (no {})", m.join(", no ")))
            .collect::<Vec<_>>()
            .join(", ");
        push(out, Finding {
            rule: "insecure-cookie",
            key: key2(&key),
            severity: InsightSeverity::Warning,
            title: format!("{} cookies from {} lack Secure or HttpOnly", cookies.len(), key.1),
            detail: format!("{list}."),
            advice: "Mark cookies Secure so they never travel over plain HTTP, and HttpOnly when scripts don't need to read them (session and auth cookies).",
            flows: group,
        });
    }
}

/// Credentials over plain HTTP, readable by anyone on the path.
fn plaintext_credentials(flows: &[&Flow], out: &mut Vec<Insight>) {
    const REQUEST: [&str; 6] = ["authorization", "proxy-authorization", "cookie", "x-api-key", "api-key", "x-auth-token"];
    let carried = |f: &Flow| -> BTreeSet<String> {
        let mut names: BTreeSet<String> = f
            .request_headers
            .iter()
            .map(|h| h.name.to_ascii_lowercase())
            .filter(|n| REQUEST.contains(&n.as_str()))
            .collect();
        if header(&f.response_headers, "set-cookie").is_some() {
            names.insert("set-cookie".into());
        }
        names
    };
    for (key, group) in by_host(flows, |f| f.scheme == "http" && !is_loopback(&f.host) && !carried(f).is_empty()) {
        let names: BTreeSet<String> = group.iter().flat_map(|f| carried(f)).collect();
        push(out, Finding {
            rule: "plaintext-credentials",
            key: key2(&key),
            severity: InsightSeverity::High,
            title: format!("Credentials sent over plain HTTP to {}", key.1),
            detail: format!(
                "{} unencrypted requests carry {}.",
                group.len(),
                names.iter().map(|n| format!("`{n}`")).collect::<Vec<_>>().join(", ")
            ),
            advice: "Use HTTPS for anything that carries a credential or a session, and send HSTS so clients stop trying plain HTTP.",
            flows: group,
        });
    }
}

/// Images in legacy formats, or simply heavy: usually the largest bytes on a page.
fn image_unoptimized(flows: &[&Flow], out: &mut Vec<Insight>) {
    for (key, group) in by_host(flows, |f| {
        if !origin_response(f) || f.status != Some(200) || is_loopback(&f.host) {
            return false;
        }
        let media = media_type(f);
        let legacy = matches!(media.as_str(), "image/jpeg" | "image/jpg" | "image/png" | "image/gif" | "image/bmp" | "image/tiff");
        media.starts_with("image/")
            && media != "image/svg+xml"
            && ((legacy && f.response_size >= IMAGE_LEGACY_BYTES) || f.response_size >= IMAGE_HEAVY_BYTES)
    }) {
        let total: u64 = group.iter().map(|f| f.response_size).sum();
        let largest = group.iter().map(|f| f.response_size).max().unwrap_or(0);
        push(out, Finding {
            rule: "image-unoptimized",
            key: key2(&key),
            severity: if total >= 2 * LARGE_PAYLOAD_BYTES { InsightSeverity::Warning } else { InsightSeverity::Info },
            title: format!("{} heavy images from {}", group.len(), key.1),
            detail: format!(
                "{} in total, largest {}. JPEG/PNG/GIF of 100 KB or more, or any image of 500 KB or more.",
                fmt_bytes(total),
                fmt_bytes(largest)
            ),
            advice: "Serve WebP or AVIF (often 25–50% smaller), size images to how large they are displayed (`srcset`), and lazy-load the ones below the fold.",
            flows: group,
        });
    }
}

/// Fingerprinted assets — safe to keep forever — told to expire soon.
fn short_cache(flows: &[&Flow], out: &mut Vec<Insight>) {
    for (key, group) in by_host(flows, |f| {
        if !origin_response(f) || f.method != "GET" || f.status != Some(200) || is_loopback(&f.host) {
            return false;
        }
        if !is_static(f, &media_type(f)) || !fingerprinted(&f.path) {
            return false;
        }
        // No-store and header-less assets are `static-uncacheable`'s to report.
        max_age(header(&f.response_headers, "cache-control").unwrap_or("")).is_some_and(|s| s < SHORT_CACHE_SECS)
    }) {
        let shortest = group
            .iter()
            .filter_map(|f| max_age(header(&f.response_headers, "cache-control").unwrap_or("")))
            .min()
            .unwrap_or(0);
        push(out, Finding {
            rule: "short-cache",
            key: key2(&key),
            severity: InsightSeverity::Info,
            title: format!("{} fingerprinted assets from {} expire within a week", group.len(), key.1),
            detail: format!(
                "Their file names carry a content hash, so they can never go stale, yet the shortest max-age is {}. Returning visitors download them again.",
                fmt_ms(shortest as f64 * 1000.0)
            ),
            advice: "Give hashed assets `Cache-Control: public, max-age=31536000, immutable`; a new build ships new file names anyway.",
            flows: group,
        });
    }
}

/// `max-age` from a Cache-Control value, in seconds.
fn max_age(cache_control: &str) -> Option<u64> {
    cache_control.split(',').find_map(|d| {
        let (k, v) = d.trim().split_once('=')?;
        k.trim().eq_ignore_ascii_case("max-age").then(|| v.trim().trim_matches('"').parse().ok())?
    })
}

/// A file name with a build hash in it: `app.3f2a1b9c.js`, `index-BxY12kq9.css`.
fn fingerprinted(path: &str) -> bool {
    let name = path_only(path).rsplit('/').next().unwrap_or("");
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    stem.split(['.', '-', '_']).any(|part| {
        let b = part.as_bytes();
        b.len() >= 8
            && b.iter().all(u8::is_ascii_alphanumeric)
            && b.iter().any(u8::is_ascii_digit)
            && b.iter().any(u8::is_ascii_alphabetic)
    })
}

/// Plain-HTTP requests made by an HTTPS page.
fn mixed_content(flows: &[&Flow], out: &mut Vec<Insight>) {
    let from_https = |f: &Flow| {
        header(&f.request_headers, "referer")
            .or_else(|| header(&f.request_headers, "origin"))
            .is_some_and(|r| r.len() >= 8 && r[..8].eq_ignore_ascii_case("https://"))
    };
    for (key, group) in by_host(flows, |f| f.scheme == "http" && !is_loopback(&f.host) && from_https(f)) {
        push(out, Finding {
            rule: "mixed-content",
            key: key2(&key),
            severity: InsightSeverity::Warning,
            title: format!("HTTPS pages load {} over plain HTTP", key.1),
            detail: format!(
                "{} requests to http://{} came from HTTPS pages. Browsers block scripts and styles loaded this way and warn about images; anyone on the network can read or change them.",
                group.len(),
                key.1
            ),
            advice: "Load the resource over HTTPS (or a protocol-relative URL), and add `Content-Security-Policy: upgrade-insecure-requests` to catch the rest.",
            flows: group,
        });
    }
}

/// HTML documents served without the headers that protect the page.
fn missing_security_headers(flows: &[&Flow], out: &mut Vec<Insight>) {
    let missing = |f: &Flow| -> Option<Vec<&'static str>> {
        if f.scheme != "https" || f.method != "GET" || f.status != Some(200) || is_loopback(&f.host) || !origin_response(f) {
            return None;
        }
        let dest = header(&f.request_headers, "sec-fetch-dest");
        if media_type(f) != "text/html" || dest.is_some_and(|d| d != "document" && d != "iframe") {
            return None;
        }
        let h = &f.response_headers;
        let csp = header(h, "content-security-policy");
        let mut gaps = Vec::new();
        if header(h, "strict-transport-security").is_none() {
            gaps.push("Strict-Transport-Security");
        }
        if csp.is_none() {
            gaps.push("Content-Security-Policy");
        }
        if header(h, "x-content-type-options").is_none_or(|v| !v.trim().eq_ignore_ascii_case("nosniff")) {
            gaps.push("X-Content-Type-Options: nosniff");
        }
        if header(h, "x-frame-options").is_none() && !csp.is_some_and(|c| c.to_ascii_lowercase().contains("frame-ancestors")) {
            gaps.push("framing protection (X-Frame-Options or CSP frame-ancestors)");
        }
        (!gaps.is_empty()).then_some(gaps)
    };
    for (key, hits) in by_host_with(flows, missing) {
        let mut gaps: Vec<&str> = Vec::new();
        for (_, g) in &hits {
            for m in g {
                if !gaps.contains(m) {
                    gaps.push(m);
                }
            }
        }
        let group: Vec<&Flow> = hits.iter().map(|(f, _)| *f).collect();
        push(out, Finding {
            rule: "missing-security-headers",
            key: key2(&key),
            severity: InsightSeverity::Info,
            title: format!("Pages from {} lack {} security {}", key.1, gaps.len(), if gaps.len() == 1 { "header" } else { "headers" }),
            detail: format!("{} HTML documents without: {}.", group.len(), gaps.join(", ")),
            advice: "Send HSTS so browsers stop trying plain HTTP, a Content-Security-Policy to limit what the page may load, `nosniff`, and frame-ancestors (or X-Frame-Options) against clickjacking.",
            flows: group,
        });
    }
}

/// Scripts, styles, fonts and images that are not there.
fn broken_asset(flows: &[&Flow], out: &mut Vec<Insight>) {
    // The response of a 404 is usually an HTML error page, so what was asked
    // for is read off the request (Sec-Fetch-Dest) or the URL, not the reply.
    let wanted_asset = |f: &Flow| {
        if let Some(dest) = header(&f.request_headers, "sec-fetch-dest") {
            return matches!(dest, "script" | "style" | "image" | "font" | "audio" | "video" | "track" | "manifest");
        }
        let path = path_only(&f.path).to_ascii_lowercase();
        [".js", ".mjs", ".css", ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".avif", ".woff", ".woff2", ".ttf", ".ico"]
            .iter()
            .any(|e| path.ends_with(e))
    };
    for (key, group) in by_host(flows, |f| {
        origin_response(f) && f.method == "GET" && matches!(f.status, Some(404 | 410)) && wanted_asset(f)
    }) {
        let mut paths: Vec<&str> = Vec::new();
        for f in &group {
            let p = path_only(&f.path);
            if !paths.contains(&p) {
                paths.push(p);
            }
        }
        let shown = paths.iter().take(3).map(|p| format!("`{p}`")).collect::<Vec<_>>().join(", ");
        let more = if paths.len() > 3 { format!(" and {} more", paths.len() - 3) } else { String::new() };
        push(out, Finding {
            rule: "broken-asset",
            key: key2(&key),
            severity: InsightSeverity::Warning,
            title: format!("{} missing {} on {}", paths.len(), if paths.len() == 1 { "asset" } else { "assets" }, key.1),
            detail: format!("404/410 for {shown}{more}. A missing script or stylesheet changes how the page works, not just how it looks."),
            advice: "Fix the reference or deploy the file; after a release, stale HTML pointing at old hashed bundles is the usual cause, so keep the previous build's assets for a while.",
            flows: group,
        });
    }
}

/// An endpoint that keeps answering 200 with nothing in it.
fn empty_response(flows: &[&Flow], out: &mut Vec<Insight>) {
    // The checks see no body bytes, only sizes: empty, or JSON too short to be
    // more than `{}`, `[]`, `""` or `null`.
    let empty = |f: &Flow| {
        let size = decoded_size(f);
        size == 0 || (size <= 4 && is_data(&media_type(f)))
    };
    for (key, group) in by_endpoint(flows, |f| {
        origin_response(f) && f.status == Some(200) && f.mcp.is_none() && !matches!(f.method.as_str(), "HEAD" | "OPTIONS") && empty(f)
    }) {
        if group.len() < EMPTY_MIN {
            continue;
        }
        push(out, Finding {
            rule: "empty-response",
            key: key3(&key),
            severity: InsightSeverity::Info,
            title: format!("{} answered 200 with an empty body {}×", key.2, group.len()),
            detail: "Every one of these came back empty or as an empty JSON value. Either the call is not needed, or it fails without saying so.".to_string(),
            advice: "If nothing is expected back, answer 204 No Content; if the client calls to check for news, skip the call or make it conditional; if data was expected, this is a silent failure worth an error status.",
            flows: group,
        });
    }
}

/// DNS, TCP or TLS that is slow before a single byte is asked for.
fn slow_connect(flows: &[&Flow], out: &mut Vec<Insight>) {
    let median = |mut v: Vec<f64>| -> Option<f64> {
        if v.len() < CONNECT_MIN_SAMPLES {
            return None;
        }
        v.sort_by(f64::total_cmp);
        Some(v[v.len() / 2])
    };
    for (key, group) in by_host(flows, |f| {
        !f.timings.connection_reused && (f.timings.dns_ms.is_some() || f.timings.connect_ms.is_some() || f.timings.tls_ms.is_some())
    }) {
        let phase = |get: fn(&Flow) -> Option<f64>| median(group.iter().filter_map(|f| get(f)).collect());
        let checks = [
            ("DNS lookup", phase(|f| f.timings.dns_ms), SLOW_DNS_MS),
            ("TCP connect", phase(|f| f.timings.connect_ms), SLOW_CONNECT_MS),
            ("TLS handshake", phase(|f| f.timings.tls_ms), SLOW_TLS_MS),
        ];
        let slow: Vec<String> = checks
            .iter()
            .filter_map(|(name, m, limit)| m.filter(|m| m > limit).map(|m| format!("{name} ~{}", fmt_ms(m))))
            .collect();
        if slow.is_empty() {
            continue;
        }
        push(out, Finding {
            rule: "slow-connect",
            key: key2(&key),
            severity: InsightSeverity::Info,
            title: format!("Slow connection setup to {}", key.1),
            detail: format!(
                "Median {} over {} new connections. Measured on NovaProxy's own connection to the server, which takes the same network path as the app.",
                slow.join(", "),
                group.len()
            ),
            advice: "Reuse connections (keep-alive, HTTP/2), `<link rel=preconnect>` hosts the page will need, and for TLS enable 1.3 and session resumption; a slow TCP connect usually means the server is far away, which a CDN fixes.",
            flows: group,
        });
    }
}

/// A CDN in front of static assets that mostly does not serve them from cache.
fn cdn_miss(flows: &[&Flow], out: &mut Vec<Insight>) {
    // Each CDN names it differently. With several layers (`MISS, HIT`) the
    // last one is the edge that answered.
    const HEADERS: [&str; 4] = ["cf-cache-status", "x-cache", "x-cache-status", "x-vercel-cache"];
    let status = |f: &Flow| -> Option<(bool, String)> {
        let raw = HEADERS.iter().find_map(|h| header(&f.response_headers, h))?;
        let last = raw.rsplit(',').next().unwrap_or(raw).trim().to_ascii_uppercase();
        let hit = ["HIT", "REVALIDATED", "STALE", "UPDATING"].iter().any(|w| last.contains(w));
        let miss = ["MISS", "EXPIRED", "BYPASS", "DYNAMIC"].iter().any(|w| last.contains(w));
        (hit || miss).then(|| (hit, last.split_whitespace().next().unwrap_or("").trim_start_matches("TCP_").to_string()))
    };
    for (key, hits) in by_host_with(flows, |f| {
        let eligible = origin_response(f) && f.method == "GET" && f.status == Some(200) && is_static(f, &media_type(f));
        eligible.then(|| status(f)).flatten()
    }) {
        if hits.len() < CDN_MIN_SAMPLES {
            continue;
        }
        let misses: Vec<&(&Flow, (bool, String))> = hits.iter().filter(|(_, (hit, _))| !hit).collect();
        let share = misses.len() as f64 / hits.len() as f64;
        if share < CDN_MISS_SHARE {
            continue;
        }
        let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
        for (_, (_, k)) in &misses {
            *kinds.entry(k.as_str()).or_default() += 1;
        }
        let kinds = kinds.iter().map(|(k, n)| format!("{n}× {k}")).collect::<Vec<_>>().join(", ");
        push(out, Finding {
            rule: "cdn-miss",
            key: key2(&key),
            severity: InsightSeverity::Info,
            title: format!("{:.0}% of static assets from {} miss the CDN cache", share * 100.0, key.1),
            detail: format!("{} of {} CDN-served assets were not served from cache ({kinds}).", misses.len(), hits.len()),
            advice: "Check what makes them uncacheable at the edge: `Cache-Control: private` or `no-cache` from the origin, cookies or query strings in the cache key, or a broad `Vary`.",
            flows: misses.iter().map(|(f, _)| *f).collect(),
        });
    }
}

/* --------------------------------- wording --------------------------------- */

fn status_breakdown<'a>(flows: impl Iterator<Item = &'a Flow>) -> String {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for f in flows {
        let label = match f.status {
            Some(s) => s.to_string(),
            None => "no response".to_string(),
        };
        *counts.entry(label).or_default() += 1;
    }
    let mut v: Vec<(String, usize)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.iter().map(|(s, n)| format!("{n}× {s}")).collect::<Vec<_>>().join(", ")
}

fn fmt_ms(ms: f64) -> String {
    if ms >= 60_000.0 {
        format!("{:.1} min", ms / 60_000.0)
    } else if ms >= 1_000.0 {
        format!("{:.1} s", ms / 1_000.0)
    } else {
        format!("{} ms", ms.round())
    }
}

fn fmt_bytes(n: u64) -> String {
    let n = n as f64;
    if n >= 1024.0 * 1024.0 {
        format!("{:.1} MB", n / 1024.0 / 1024.0)
    } else if n >= 1024.0 {
        format!("{:.0} KB", n / 1024.0)
    } else {
        format!("{n} B")
    }
}

/// `host/path` with the query cut, for one-line titles.
fn short_url(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let rest = path_only(rest);
    if rest.chars().count() > 60 {
        format!("{}…", rest.chars().take(59).collect::<String>())
    } else {
        rest.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::new_flow;
    use nova_proto::McpInfo;

    /// A completed GET from "Chrome" to `https://api.test{path}`, at `t` ms.
    fn flow(id: &str, t: f64, path: &str) -> Flow {
        let mut f = new_flow(
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
        f.started_at = t;
        f.duration_ms = Some(50.0);
        f.state = FlowState::Completed;
        f.status = Some(200);
        f.process = Some("Chrome".into());
        // A small, ordinary body: an empty one is itself a finding.
        f.response_size = 512;
        f
    }

    fn h(name: &str, value: &str) -> Header {
        Header { name: name.into(), value: value.into() }
    }

    fn run(flows: &[Flow]) -> Vec<Insight> {
        analyze(flows, &Options::default()).insights
    }

    fn rules(flows: &[Flow]) -> Vec<String> {
        run(flows).into_iter().map(|i| i.rule).collect()
    }

    fn only(flows: &[Flow], rule: &str) -> Insight {
        let hits: Vec<Insight> = run(flows).into_iter().filter(|i| i.rule == rule).collect();
        assert_eq!(hits.len(), 1, "expected one {rule}, got {hits:?}");
        hits.into_iter().next().unwrap()
    }

    #[test]
    fn paths_normalize_ids_uuids_hashes_and_tokens() {
        assert_eq!(normalize_path("/users/42/posts?x=1"), "/users/:id/posts");
        assert_eq!(normalize_path("/o/3fa85f64-5717-4562-b3fc-2c963f66afa6"), "/o/:uuid");
        assert_eq!(normalize_path("/blob/9f86d081884c7d65"), "/blob/:hash");
        assert_eq!(normalize_path("/s/AbC123xyz789QweRty45"), "/s/:token");
        // Words and short version segments stay as they are.
        assert_eq!(normalize_path("/v2/search/deadbeef"), "/v2/search/deadbeef");
        assert_eq!(normalize_path("/"), "/");
    }

    #[test]
    fn a_burst_of_different_ids_run_serially_reads_as_n_plus_one() {
        let flows: Vec<Flow> = (0..6).map(|i| flow(&format!("f{i}"), 1000.0 + i as f64 * 60.0, &format!("/users/{i}"))).collect();
        let i = only(&flows, "burst");
        assert_eq!(i.count, 6);
        assert!(i.title.contains("GET /users/:id"), "{}", i.title);
        assert!(i.detail.contains("one after another"), "{}", i.detail);
        assert!(i.advice.contains("N+1"));
        assert_eq!(i.app.as_deref(), Some("Chrome"));
    }

    #[test]
    fn a_parallel_burst_of_the_same_url_says_de_duplicate() {
        let flows: Vec<Flow> = (0..5).map(|i| flow(&format!("f{i}"), 1000.0 + i as f64, "/me")).collect();
        let i = only(&flows, "burst");
        assert!(i.detail.contains("identical URL") && i.detail.contains("in parallel"), "{}", i.detail);
    }

    #[test]
    fn calls_spread_out_are_not_a_burst() {
        // Same five calls, but separated clusters never put five in one window —
        // the HAR analyser this replaces merged such clusters.
        let flows: Vec<Flow> = [0.0, 10.0, 5_000.0, 5_010.0, 10_000.0]
            .iter()
            .enumerate()
            .map(|(i, t)| flow(&format!("f{i}"), *t, "/users/1"))
            .collect();
        assert!(!rules(&flows).contains(&"burst".to_string()));
    }

    #[test]
    fn bursts_are_per_app() {
        let mut flows: Vec<Flow> = (0..6).map(|i| flow(&format!("f{i}"), 1000.0 + i as f64, "/x")).collect();
        for f in flows.iter_mut().skip(3) {
            f.process = Some("Slack".into());
        }
        assert!(!rules(&flows).contains(&"burst".to_string()), "3 + 3 from two apps is no burst");
    }

    #[test]
    fn steady_gets_read_as_polling_and_jittery_ones_do_not() {
        let steady: Vec<Flow> = (0..8).map(|i| flow(&format!("f{i}"), i as f64 * 5_000.0, "/status")).collect();
        let i = only(&steady, "polling");
        assert!(i.title.contains("every ~5.0 s"), "{}", i.title);
        assert_eq!(i.severity, InsightSeverity::Info);

        let jittery: Vec<Flow> = [0.0, 1_000.0, 9_000.0, 10_000.0, 30_000.0, 31_000.0, 60_000.0]
            .iter()
            .enumerate()
            .map(|(i, t)| flow(&format!("f{i}"), *t, "/status"))
            .collect();
        assert!(!rules(&jittery).contains(&"polling".to_string()));
    }

    #[test]
    fn rapid_failures_are_a_retry_storm_and_429_without_retry_after_is_named() {
        let flows: Vec<Flow> = (0..4)
            .map(|i| {
                let mut f = flow(&format!("f{i}"), i as f64 * 500.0, "/pay");
                f.status = Some(if i % 2 == 0 { 503 } else { 429 });
                f
            })
            .collect();
        let storm = only(&flows, "retry-storm");
        assert!(storm.detail.contains("2× 429") && storm.detail.contains("2× 503"), "{}", storm.detail);
        let limited = only(&flows, "rate-limited");
        assert!(limited.detail.contains("none with Retry-After"), "{}", limited.detail);
    }

    #[test]
    fn transport_failures_count_as_failures() {
        let flows: Vec<Flow> = (0..3)
            .map(|i| {
                let mut f = flow(&format!("f{i}"), i as f64 * 100.0, "/a");
                f.state = FlowState::Error;
                f.status = None;
                f.error = Some("connection refused".into());
                f
            })
            .collect();
        assert!(only(&flows, "retry-storm").detail.contains("3× no response"));
    }

    #[test]
    fn a_host_failing_a_third_of_its_calls_has_an_error_rate() {
        let flows: Vec<Flow> = (0..12)
            .map(|i| {
                let mut f = flow(&format!("f{i}"), i as f64 * 20_000.0, &format!("/r{i}"));
                if i % 3 == 0 {
                    f.status = Some(401);
                }
                f
            })
            .collect();
        let i = only(&flows, "error-rate");
        assert_eq!(i.count, 4);
        assert!(i.title.starts_with("33%"), "{}", i.title);
    }

    fn sized(id: &str, t: f64, ct: &str, size: u64) -> Flow {
        let mut f = flow(id, t, &format!("/{id}"));
        f.content_type = Some(ct.into());
        f.response_size = size;
        f
    }

    #[test]
    fn uncompressed_text_is_flagged_but_images_and_encoded_text_are_not() {
        let mut gz = sized("gz", 0.0, "application/json", 50_000);
        gz.response_headers.push(h("Content-Encoding", "gzip"));
        let flows = vec![
            sized("plain", 0.0, "application/json; charset=utf-8", 50_000),
            sized("img", 1.0, "image/jpeg", 5_000_000),
            sized("zip", 2.0, "application/zip", 5_000_000),
            gz,
        ];
        let i = only(&flows, "uncompressed");
        assert_eq!(i.flow_ids, vec!["plain"]);
    }

    #[test]
    fn loopback_dev_servers_are_not_judged_on_compression_or_caching() {
        let mut f = sized("dev", 0.0, "application/javascript", 500_000);
        f.host = "localhost:5173".into();
        assert!(run(&[f]).is_empty());
    }

    #[test]
    fn mapped_local_responses_are_not_the_origins_fault() {
        let mut f = sized("mapped", 0.0, "application/json", 500_000);
        f.edits.rule = true;
        assert!(!rules(&[f]).contains(&"uncompressed".to_string()));
    }

    #[test]
    fn megabyte_json_is_a_large_payload() {
        let i = only(&[sized("big", 0.0, "application/json", 3 * 1024 * 1024)], "large-payload");
        assert!(i.title.contains("3.0 MB"), "{}", i.title);
    }

    #[test]
    fn static_assets_without_cache_headers_or_with_no_store_are_flagged_but_no_cache_is_fine() {
        let bare = sized("a.js", 0.0, "application/javascript", 100);
        let mut no_store = sized("b.css", 1.0, "text/css", 100);
        no_store.response_headers.push(h("Cache-Control", "no-store"));
        let mut revalidate = sized("c.png", 2.0, "image/png", 100);
        revalidate.response_headers.push(h("Cache-Control", "no-cache"));
        revalidate.response_headers.push(h("ETag", "\"x\""));
        let mut cached = sized("d.woff2", 3.0, "font/woff2", 100);
        cached.response_headers.push(h("Cache-Control", "max-age=31536000, immutable"));
        let i = only(&[bare, no_store, revalidate, cached], "static-uncacheable");
        assert_eq!(i.count, 2);
        assert!(i.detail.contains("1 marked no-store") && i.detail.contains("1 with no Cache-Control"), "{}", i.detail);
    }

    fn redirect(id: &str, t: f64, url_path: &str, status: u16, location: &str) -> Flow {
        let mut f = flow(id, t, url_path);
        f.status = Some(status);
        f.response_headers.push(h("Location", location));
        f
    }

    #[test]
    fn redirects_are_followed_through_location_into_a_chain() {
        let flows = vec![
            redirect("r0", 0.0, "/a", 301, "https://api.test/b"),
            redirect("r1", 10.0, "/b", 302, "/c"),
            redirect("r2", 20.0, "/c", 307, "d"),
            flow("ok", 30.0, "/d"),
        ];
        let i = only(&flows, "redirect-chain");
        assert_eq!(i.flow_ids, vec!["r0", "r1", "r2", "ok"]);
        assert!(i.title.starts_with("3 redirects"), "{}", i.title);
        assert!(i.detail.contains("301 api.test/a → 302 api.test/b"), "{}", i.detail);
    }

    #[test]
    fn redirects_back_to_a_visited_url_are_a_loop() {
        let flows = vec![
            redirect("r0", 0.0, "/login", 302, "/sso"),
            redirect("r1", 10.0, "/sso", 302, "/login"),
            redirect("r2", 20.0, "/login", 302, "/sso"),
        ];
        let i = only(&flows, "redirect-loop");
        assert_eq!(i.severity, InsightSeverity::High);
    }

    #[test]
    fn a_single_redirect_is_not_worth_a_finding() {
        let flows = vec![redirect("r0", 0.0, "/a", 301, "/b"), flow("ok", 10.0, "/b")];
        assert!(run(&flows).is_empty());
    }

    #[test]
    fn locations_resolve_like_a_browser_would() {
        let base = "https://api.test:443/x/y?q=1";
        assert_eq!(resolve(base, "/z").as_deref(), Some("https://api.test/z"));
        assert_eq!(resolve(base, "z").as_deref(), Some("https://api.test/x/z"));
        assert_eq!(resolve(base, "//cdn.test/a").as_deref(), Some("https://cdn.test/a"));
        assert_eq!(resolve(base, "HTTP://Other.test:80/").as_deref(), Some("http://other.test/"));
    }

    fn timed(id: &str, t: f64, ttfb: f64, connect: Option<f64>) -> Flow {
        let mut f = flow(id, t, "/report");
        f.timings.ttfb_ms = Some(ttfb);
        f.timings.connect_ms = connect;
        f
    }

    #[test]
    fn slow_server_wait_excludes_connection_setup_and_added_latency() {
        let slow = vec![timed("a", 0.0, 1_500.0, None), timed("b", 1.0, 1_600.0, None), timed("c", 2.0, 1_700.0, None)];
        assert!(only(&slow, "slow-endpoint").title.contains("~1.6 s"));

        let setup = vec![timed("a", 0.0, 1_500.0, Some(900.0)), timed("b", 1.0, 1_600.0, Some(900.0)), timed("c", 2.0, 1_700.0, Some(900.0))];
        assert!(!rules(&setup).contains(&"slow-endpoint".to_string()), "the wait was mostly connecting");

        let throttled = analyze(&slow, &Options { added_latency_ms: 1_000.0 });
        assert!(throttled.insights.iter().all(|i| i.rule != "slow-endpoint"), "NovaProxy added that delay itself");
    }

    #[test]
    fn preflights_without_max_age_are_reported_per_host() {
        let flows: Vec<Flow> = (0..5)
            .map(|i| {
                let mut f = flow(&format!("p{i}"), i as f64 * 3_000.0, &format!("/items/{i}"));
                f.method = "OPTIONS".into();
                f.status = Some(204);
                f.request_headers.push(h("Access-Control-Request-Method", "POST"));
                f
            })
            .collect();
        let i = only(&flows, "cors-preflight");
        assert!(i.detail.contains("No Access-Control-Max-Age"), "{}", i.detail);
        assert!(!rules(&flows).contains(&"burst".to_string()), "preflights are not a burst of their own");
    }

    #[test]
    fn credentials_in_query_strings_are_high_severity() {
        let flows = vec![
            flow("t", 0.0, "/cb?access_token=abcdefgh12345&x=1"),
            flow("j", 1.0, "/x?q=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2lnbmF0dXJl"),
            flow("short", 2.0, "/x?key=1"),
            flow("signed", 3.0, "/file?X-Amz-Signature=abcdef0123456789"),
        ];
        let i = only(&flows, "token-in-url");
        assert_eq!(i.severity, InsightSeverity::High);
        assert_eq!(i.flow_ids, vec!["t", "j"]);
        assert!(i.detail.contains("`access_token`") && i.detail.contains("`q`"), "{}", i.detail);
    }

    #[test]
    fn cookies_missing_secure_or_httponly_are_named() {
        let mut f = flow("c", 0.0, "/login");
        f.response_headers.push(h("Set-Cookie", "session_id=abc; Path=/; SameSite=Lax"));
        f.response_headers.push(h("Set-Cookie", "theme=dark; Secure"));
        f.response_headers.push(h("Set-Cookie", "auth=x; Secure; HttpOnly"));
        let i = only(&[f], "insecure-cookie");
        assert_eq!(i.detail, "`session_id` (no Secure, no HttpOnly).");
    }

    #[test]
    fn credentials_over_plain_http_are_flagged_except_on_loopback() {
        let mut remote = flow("r", 0.0, "/api");
        remote.scheme = "http".into();
        remote.request_headers.push(h("Authorization", "Bearer x"));
        let mut local = remote.clone();
        local.id = "l".into();
        local.host = "127.0.0.1:3000".into();
        let i = only(&[remote, local], "plaintext-credentials");
        assert_eq!(i.flow_ids, vec!["r"]);
        assert!(i.detail.contains("`authorization`"));
    }

    fn image(id: &str, ct: &str, size: u64) -> Flow {
        sized(id, 0.0, ct, size)
    }

    #[test]
    fn legacy_or_heavy_images_are_flagged_and_small_or_modern_ones_are_not() {
        let flows = vec![
            image("big.jpg", "image/jpeg", 300 * 1024),
            image("ok.webp", "image/webp", 300 * 1024),
            image("tiny.png", "image/png", 20 * 1024),
            image("huge.avif", "image/avif", 900 * 1024),
            image("logo.svg", "image/svg+xml", 900 * 1024),
        ];
        let i = only(&flows, "image-unoptimized");
        assert_eq!(i.flow_ids, vec!["big.jpg", "huge.avif"]);
    }

    fn asset(path: &str, cache_control: &str) -> Flow {
        let mut f = flow(path, 0.0, path);
        f.content_type = Some("application/javascript".into());
        f.response_headers.push(h("Cache-Control", cache_control));
        f
    }

    #[test]
    fn fingerprinted_assets_with_short_max_age_are_flagged() {
        let flows = vec![
            asset("/assets/app.3f2a1b9c.js", "public, max-age=3600"),
            asset("/assets/index-BxY12kq9.js", "max-age=31536000, immutable"),
            asset("/app.js", "max-age=60"),
            asset("/assets/vendor.9e8d7c6b.js", "no-store"),
        ];
        let i = only(&flows, "short-cache");
        assert_eq!(i.flow_ids, vec!["/assets/app.3f2a1b9c.js"]);
        assert!(i.detail.contains("1.0 h") || i.detail.contains("60.0 min"), "{}", i.detail);
    }

    #[test]
    fn build_hashes_are_told_apart_from_versions_and_words() {
        assert!(fingerprinted("/a/main.3f2a1b9c.js"));
        assert!(fingerprinted("/a/chunk-BxY12kq9.css?v=1"));
        assert!(!fingerprinted("/a/jquery-3.6.0.min.js"));
        assert!(!fingerprinted("/a/background.png"));
        assert_eq!(max_age("public, Max-Age=600"), Some(600));
        assert_eq!(max_age("no-cache"), None);
    }

    #[test]
    fn plain_http_requests_from_https_pages_are_mixed_content() {
        let mut mixed = flow("m", 0.0, "/pixel.gif");
        mixed.scheme = "http".into();
        mixed.request_headers.push(h("Referer", "https://shop.test/cart"));
        let mut plain_page = mixed.clone();
        plain_page.id = "p".into();
        plain_page.request_headers = vec![h("Referer", "http://shop.test/")];
        let mut local = mixed.clone();
        local.id = "l".into();
        local.host = "localhost:8080".into();
        let i = only(&[mixed, plain_page, local], "mixed-content");
        assert_eq!(i.flow_ids, vec!["m"]);
    }

    fn page(id: &str, headers: Vec<Header>) -> Flow {
        let mut f = flow(id, 0.0, "/");
        f.content_type = Some("text/html; charset=utf-8".into());
        f.request_headers.push(h("Sec-Fetch-Dest", "document"));
        f.response_headers = headers;
        f
    }

    #[test]
    fn html_documents_without_security_headers_are_named() {
        let bare = page("bare", vec![]);
        let i = only(&[bare], "missing-security-headers");
        assert!(i.title.contains("4 security headers"), "{}", i.title);
        assert!(i.detail.contains("Strict-Transport-Security") && i.detail.contains("frame-ancestors"), "{}", i.detail);

        let hardened = page("ok", vec![
            h("Strict-Transport-Security", "max-age=63072000"),
            h("Content-Security-Policy", "default-src 'self'; frame-ancestors 'none'"),
            h("X-Content-Type-Options", "nosniff"),
        ]);
        assert!(!rules(&[hardened]).contains(&"missing-security-headers".to_string()));

        // Fetching HTML from script is not a page load.
        let mut fetched = page("xhr", vec![]);
        fetched.request_headers = vec![h("Sec-Fetch-Dest", "empty")];
        assert!(!rules(&[fetched]).contains(&"missing-security-headers".to_string()));
    }

    fn missing(id: &str, path: &str, dest: Option<&str>) -> Flow {
        let mut f = flow(id, 0.0, path);
        f.status = Some(404);
        f.content_type = Some("text/html".into());
        if let Some(d) = dest {
            f.request_headers.push(h("Sec-Fetch-Dest", d));
        }
        f
    }

    #[test]
    fn a_single_missing_script_or_image_is_a_broken_asset() {
        let flows = vec![
            missing("js", "/assets/app.js", None),
            missing("img", "/avatar", Some("image")),
            missing("api", "/api/users/1", Some("empty")),
            missing("page", "/old-page", Some("document")),
        ];
        let i = only(&flows, "broken-asset");
        assert_eq!(i.flow_ids, vec!["js", "img"]);
        assert!(i.title.starts_with("2 missing assets"), "{}", i.title);
    }

    #[test]
    fn repeated_empty_200s_are_flagged_but_204_and_head_are_not() {
        let mk = |id: &str, method: &str, status: u16, size: u64| {
            let mut f = sized(id, 0.0, "application/json", size);
            f.path = "/api/ping".into();
            f.url = "https://api.test/api/ping".into();
            f.method = method.into();
            f.status = Some(status);
            f
        };
        let flows = vec![mk("a", "GET", 200, 0), mk("b", "GET", 200, 2), mk("c", "GET", 200, 4), mk("d", "GET", 204, 0), mk("e", "HEAD", 200, 0)];
        let i = only(&flows, "empty-response");
        assert_eq!(i.flow_ids, vec!["a", "b", "c"]);

        let real = vec![mk("a", "GET", 200, 0), mk("b", "GET", 200, 0), mk("c", "GET", 200, 300)];
        assert!(!rules(&real).contains(&"empty-response".to_string()), "two empty of three is not the pattern");
    }

    #[test]
    fn html_over_half_a_megabyte_is_a_large_payload() {
        let i = only(&[sized("page", 0.0, "text/html", 700 * 1024)], "large-payload");
        assert!(i.title.contains("of HTML"), "{}", i.title);
        assert!(run(&[sized("page", 0.0, "text/html", 300 * 1024)]).iter().all(|i| i.rule != "large-payload"));
    }

    #[test]
    fn one_request_that_hung_for_ten_seconds_is_slow_on_its_own() {
        let i = only(&[timed("a", 0.0, 12_000.0, None)], "slow-endpoint");
        assert_eq!(i.severity, InsightSeverity::Warning);
        assert!(i.title.contains("12.0 s"), "{}", i.title);
        assert!(run(&[timed("a", 0.0, 4_000.0, None)]).is_empty(), "one 4 s call is not enough to judge");
    }

    fn connect(id: &str, dns: Option<f64>, tcp: Option<f64>, tls: Option<f64>) -> Flow {
        let mut f = flow(id, 0.0, &format!("/{id}"));
        f.timings.dns_ms = dns;
        f.timings.connect_ms = tcp;
        f.timings.tls_ms = tls;
        f
    }

    #[test]
    fn slow_dns_tcp_or_tls_medians_are_reported_per_host() {
        let flows = vec![connect("a", Some(350.0), Some(20.0), Some(600.0)), connect("b", Some(300.0), Some(30.0), Some(700.0))];
        let i = only(&flows, "slow-connect");
        assert!(i.detail.contains("DNS lookup") && i.detail.contains("TLS handshake"), "{}", i.detail);
        assert!(!i.detail.contains("TCP connect"), "{}", i.detail);

        let one = vec![connect("a", Some(900.0), None, None)];
        assert!(run(&one).is_empty(), "one connection is not a median");
        let mut reused = flows.clone();
        for f in &mut reused {
            f.timings.connection_reused = true;
        }
        assert!(run(&reused).is_empty());
    }

    fn cdn(id: usize, status: &str) -> Flow {
        let mut f = sized(&format!("a{id}.css"), id as f64, "text/css", 100);
        f.response_headers.push(h("Cache-Control", "max-age=60"));
        f.response_headers.push(h("X-Cache", status));
        f
    }

    #[test]
    fn mostly_missing_the_cdn_is_flagged_and_the_edge_layer_decides() {
        let mut flows: Vec<Flow> = (0..6).map(|i| cdn(i, "Miss from cloudfront")).collect();
        flows.extend((6..10).map(|i| cdn(i, "MISS, HIT")));
        let i = only(&flows, "cdn-miss");
        assert_eq!(i.count, 6);
        assert!(i.title.starts_with("60%"), "{}", i.title);

        let warm: Vec<Flow> = (0..10).map(|i| cdn(i, if i < 3 { "MISS" } else { "HIT" })).collect();
        assert!(!rules(&warm).contains(&"cdn-miss".to_string()));
    }

    #[test]
    fn mcp_calls_are_grouped_by_tool() {
        let flows: Vec<Flow> = (0..5)
            .map(|i| {
                let mut f = flow(&format!("m{i}"), i as f64 * 10.0, "/mcp");
                f.method = "POST".into();
                f.mcp = Some(McpInfo {
                    method: Some("tools/call".into()),
                    tool: Some("search".into()),
                    id: Some(i.to_string()),
                    transport: nova_proto::McpTransport::Http,
                });
                f
            })
            .collect();
        assert!(only(&flows, "burst").title.starts_with("MCP tools/call → search"));
    }

    #[test]
    fn unnamed_graphql_is_not_one_giant_endpoint() {
        let flows: Vec<Flow> = (0..6).map(|i| {
            let mut f = flow(&format!("g{i}"), i as f64 * 10.0, "/graphql");
            f.method = "POST".into();
            f
        }).collect();
        assert!(run(&flows).is_empty());

        let named: Vec<Flow> = (0..6).map(|i| flow(&format!("g{i}"), i as f64 * 10.0, "/graphql?operationName=Feed")).collect();
        assert!(only(&named, "burst").title.starts_with("GraphQL Feed"));
    }

    #[test]
    fn flows_that_cannot_be_judged_are_left_out() {
        let mut flows: Vec<Flow> = (0..6).map(|i| flow(&format!("f{i}"), i as f64, "/x")).collect();
        flows[0].internal = true;
        flows[1].tunneled = true;
        flows[2].resent = true;
        flows[3].is_websocket = true;
        flows[4].state = FlowState::Started;
        let report = analyze(&flows, &Options::default());
        assert_eq!(report.analyzed, 1);
        assert!(report.insights.is_empty());
    }

    #[test]
    fn ids_are_stable_and_results_sorted_by_severity() {
        let mut flows: Vec<Flow> = (0..8).map(|i| flow(&format!("f{i}"), i as f64 * 5_000.0, "/status")).collect();
        flows.push(flow("t", 50_000.0, "/cb?token=abcdefgh1234"));
        let a = run(&flows);
        let b = run(&flows);
        assert_eq!(a.iter().map(|i| &i.id).collect::<Vec<_>>(), b.iter().map(|i| &i.id).collect::<Vec<_>>());
        assert_eq!(a[0].rule, "token-in-url", "High sorts first");
        assert_eq!(a[1].id, "polling:Chrome|api.test|GET /status");
    }

    #[test]
    fn flow_ids_are_capped_but_count_is_not() {
        let flows: Vec<Flow> = (0..600).map(|i| flow(&format!("f{i}"), i as f64, "/x")).collect();
        let i = only(&flows, "burst");
        assert_eq!(i.count, 600);
        assert_eq!(i.flow_ids.len(), MAX_IDS);
        assert_eq!(i.flow_ids.last().map(String::as_str), Some("f599"), "the newest are kept");
    }
}
