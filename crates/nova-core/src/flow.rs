//! Flow bookkeeping: the shared in-flight store, body decoding, and the sink
//! that pushes snapshots toward the UI.

use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use nova_proto::{BodyPreview, Flow, FlowState, Header, NetworkConditions, Rule};

use crate::breakpoint::Breakpoints;
use crate::scripting::ScriptEngine;

/// Which half of the exchange a captured body belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Request,
    Response,
}

/// Anything that wants to observe flow updates (the Tauri layer implements it).
pub trait FlowSink: Send + Sync + 'static {
    fn emit(&self, flow: Flow);
}

/// State shared across every per-connection handler clone.
pub struct Shared {
    pub seq: AtomicU64,
    pub sink: Arc<dyn FlowSink>,
    pub flows: Mutex<HashMap<String, Flow>>,
    pub body_cap: usize,
    pub total_captured: AtomicU64,
    /// Live traffic-control rules; shared with the app so edits take effect
    /// without restarting the engine.
    pub rules: Arc<RwLock<Vec<Rule>>>,
    /// Breakpoint state shared with the app (arm/disarm/resume).
    pub breakpoints: Arc<Breakpoints>,
    /// JavaScript scripting sandbox.
    pub scripts: Arc<ScriptEngine>,
    /// Simulated network conditions (latency / throttle).
    pub net: Arc<RwLock<NetworkConditions>>,
}

impl Shared {
    pub fn new(
        sink: Arc<dyn FlowSink>,
        body_cap: usize,
        rules: Arc<RwLock<Vec<Rule>>>,
        breakpoints: Arc<Breakpoints>,
        scripts: Arc<ScriptEngine>,
        net: Arc<RwLock<NetworkConditions>>,
    ) -> Self {
        Self {
            seq: AtomicU64::new(0),
            sink,
            flows: Mutex::new(HashMap::new()),
            body_cap,
            total_captured: AtomicU64::new(0),
            rules,
            breakpoints,
            scripts,
            net,
        }
    }

    /// Insert a freshly-seen flow and emit it.
    pub fn insert(&self, flow: Flow) {
        self.total_captured
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.flows.lock().unwrap().insert(flow.id.clone(), flow.clone());
        self.sink.emit(flow);
    }

    /// Mutate a flow in place and emit the updated snapshot.
    pub fn update<F: FnOnce(&mut Flow)>(&self, id: &str, f: F) {
        let snapshot = {
            let mut map = self.flows.lock().unwrap();
            let Some(flow) = map.get_mut(id) else { return };
            f(flow);
            flow.clone()
        };
        self.sink.emit(snapshot);
    }
}

/// Current epoch time in milliseconds (fractional).
pub fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// Collect headers preserving order and duplicates.
pub fn collect_headers(map: &hudsucker::hyper::HeaderMap) -> Vec<Header> {
    map.iter()
        .map(|(k, v)| Header {
            name: k.as_str().to_string(),
            value: String::from_utf8_lossy(v.as_bytes()).into_owned(),
        })
        .collect()
}

pub fn header_value(map: &hudsucker::hyper::HeaderMap, name: &str) -> Option<String> {
    map.get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// Build a [`BodyPreview`] from captured (still-encoded) bytes.
///
/// `total` is the true wire size; `raw` may already be capped.
pub fn build_preview(
    raw: Vec<u8>,
    total: u64,
    truncated: bool,
    media_type: Option<String>,
    content_encoding: Option<String>,
) -> BodyPreview {
    let decoded = decode(&raw, content_encoding.as_deref());
    let is_text = looks_textual(media_type.as_deref(), &decoded);

    let (text, base64) = if decoded.is_empty() {
        (None, None)
    } else if is_text {
        (Some(String::from_utf8_lossy(&decoded).into_owned()), None)
    } else {
        use base64::Engine;
        (
            None,
            Some(base64::engine::general_purpose::STANDARD.encode(&decoded)),
        )
    };

    BodyPreview {
        size: total,
        truncated,
        media_type,
        decoded_from: content_encoding.filter(|e| !e.eq_ignore_ascii_case("identity")),
        text,
        base64,
    }
}

/// Decode a `Content-Encoding` off a captured body copy. Best-effort: on any
/// failure the original bytes are returned unchanged.
fn decode(raw: &[u8], encoding: Option<&str>) -> Vec<u8> {
    let Some(enc) = encoding.map(|e| e.to_ascii_lowercase()) else {
        return raw.to_vec();
    };
    let attempt = |result: std::io::Result<Vec<u8>>| result.unwrap_or_else(|_| raw.to_vec());

    if enc.contains("gzip") || enc.contains("x-gzip") {
        let mut out = Vec::new();
        attempt(
            flate2::read::MultiGzDecoder::new(raw)
                .read_to_end(&mut out)
                .map(|_| out),
        )
    } else if enc.contains("deflate") {
        let mut out = Vec::new();
        attempt(
            flate2::read::ZlibDecoder::new(raw)
                .read_to_end(&mut out)
                .map(|_| out),
        )
    } else if enc.contains("br") {
        let mut out = Vec::new();
        attempt(
            brotli::Decompressor::new(raw, 4096)
                .read_to_end(&mut out)
                .map(|_| out),
        )
    } else {
        raw.to_vec()
    }
}

/// Heuristic: is this body better shown as text than as base64?
fn looks_textual(media_type: Option<&str>, bytes: &[u8]) -> bool {
    if let Some(mt) = media_type.map(|m| m.to_ascii_lowercase()) {
        if mt.starts_with("text/")
            || mt.contains("json")
            || mt.contains("xml")
            || mt.contains("javascript")
            || mt.contains("ecmascript")
            || mt.contains("html")
            || mt.contains("csv")
            || mt.contains("x-www-form-urlencoded")
            || mt.contains("graphql")
        {
            return true;
        }
        if mt.starts_with("image/")
            || mt.starts_with("video/")
            || mt.starts_with("audio/")
            || mt.contains("octet-stream")
            || mt.contains("protobuf")
            || mt.contains("grpc")
        {
            return false;
        }
    }
    // Unknown type: sniff for NUL bytes in the leading window.
    let window = &bytes[..bytes.len().min(2048)];
    !window.contains(&0)
}

/// Assemble a brand-new flow record for a request.
#[allow(clippy::too_many_arguments)]
pub fn new_flow(
    id: String,
    seq: u64,
    method: String,
    scheme: String,
    host: String,
    path: String,
    url: String,
    client_addr: String,
    http_version: String,
    request_headers: Vec<Header>,
) -> Flow {
    let content_type = None;
    Flow {
        id,
        seq,
        method,
        scheme,
        host,
        path,
        url,
        client_addr,
        http_version,
        state: FlowState::Started,
        status: None,
        request_headers,
        response_headers: Vec::new(),
        request_body: None,
        response_body: None,
        request_size: 0,
        response_size: 0,
        content_type,
        started_at: now_ms(),
        duration_ms: None,
        error: None,
        resent: false,
        mapped_from: None,
    }
}
