//! Flow bookkeeping: the shared in-flight store, body decoding, and the sink
//! that pushes snapshots toward the UI.

use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use nova_proto::{
    BodyPreview, Flow, FlowState, Header, NetworkConditions, Rule, TlsScope, WsMessage,
};

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

/// Anything that wants to observe WebSocket frames (the Tauri layer implements
/// it). Kept separate from [`FlowSink`] so frames stream on their own channel.
pub trait WsSink: Send + Sync + 'static {
    fn emit(&self, msg: WsMessage);
}

/// A no-op [`WsSink`] for contexts that don't inspect WebSocket traffic
/// (examples, tests).
pub struct NoopWsSink;
impl WsSink for NoopWsSink {
    fn emit(&self, _msg: WsMessage) {}
}

/// Correlation record for one upgraded WebSocket: the flow it belongs to and a
/// monotonic frame counter shared across both directions of the socket.
pub struct WsRoute {
    pub flow_id: String,
    pub seq: AtomicU64,
}

/// State shared across every per-connection handler clone.
pub struct Shared {
    pub seq: AtomicU64,
    pub sink: Arc<dyn FlowSink>,
    /// Sink for captured WebSocket frames.
    pub ws_sink: Arc<dyn WsSink>,
    /// Active WebSocket routes keyed by `host + path_and_query`, mapping the
    /// handshake URL to the flow its frames belong to. Best-effort correlation:
    /// concurrent sockets to an identical URL fold into the latest flow (same
    /// documented approximation as the HTTP/2 request↔response FIFO).
    pub ws_routes: Mutex<HashMap<String, Arc<WsRoute>>>,
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
    /// Per-host SSL-proxying scope (decrypt vs tunnel).
    pub tls_scope: Arc<RwLock<TlsScope>>,
}

impl Shared {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sink: Arc<dyn FlowSink>,
        ws_sink: Arc<dyn WsSink>,
        body_cap: usize,
        rules: Arc<RwLock<Vec<Rule>>>,
        breakpoints: Arc<Breakpoints>,
        scripts: Arc<ScriptEngine>,
        net: Arc<RwLock<NetworkConditions>>,
        tls_scope: Arc<RwLock<TlsScope>>,
    ) -> Self {
        Self {
            seq: AtomicU64::new(0),
            sink,
            ws_sink,
            ws_routes: Mutex::new(HashMap::new()),
            flows: Mutex::new(HashMap::new()),
            body_cap,
            total_captured: AtomicU64::new(0),
            rules,
            breakpoints,
            scripts,
            net,
            tls_scope,
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
        pid: None,
        process: None,
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
        is_websocket: false,
        tunneled: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hudsucker::hyper::HeaderMap;
    use std::io::Write;

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn deflate(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn preview_text_body_is_utf8() {
        let raw = b"hello world".to_vec();
        let p = build_preview(raw.clone(), raw.len() as u64, false, Some("text/plain".into()), None);
        assert_eq!(p.text.as_deref(), Some("hello world"));
        assert!(p.base64.is_none());
        assert_eq!(p.size, 11);
        assert!(!p.truncated);
        assert!(p.decoded_from.is_none());
    }

    #[test]
    fn preview_json_is_text() {
        let raw = br#"{"ok":true}"#.to_vec();
        let p = build_preview(raw.clone(), raw.len() as u64, false, Some("application/json".into()), None);
        assert_eq!(p.text.as_deref(), Some(r#"{"ok":true}"#));
        assert!(p.base64.is_none());
    }

    #[test]
    fn preview_binary_body_is_base64() {
        // PNG magic + a NUL byte: declared image/* => binary.
        let raw = vec![0x89, b'P', b'N', b'G', 0x00, 0x01];
        let p = build_preview(raw.clone(), raw.len() as u64, false, Some("image/png".into()), None);
        assert!(p.text.is_none());
        assert!(p.base64.is_some());
    }

    #[test]
    fn preview_empty_body_has_neither() {
        let p = build_preview(Vec::new(), 0, false, Some("text/plain".into()), None);
        assert!(p.text.is_none());
        assert!(p.base64.is_none());
    }

    #[test]
    fn preview_decodes_gzip() {
        let plain = "the quick brown fox";
        let raw = gzip(plain.as_bytes());
        let p = build_preview(raw, plain.len() as u64, false, Some("text/plain".into()), Some("gzip".into()));
        assert_eq!(p.text.as_deref(), Some(plain));
        assert_eq!(p.decoded_from.as_deref(), Some("gzip"));
    }

    #[test]
    fn preview_decodes_deflate() {
        let plain = "deflate me please";
        let raw = deflate(plain.as_bytes());
        let p = build_preview(raw, plain.len() as u64, false, Some("text/plain".into()), Some("deflate".into()));
        assert_eq!(p.text.as_deref(), Some(plain));
        assert_eq!(p.decoded_from.as_deref(), Some("deflate"));
    }

    #[test]
    fn preview_identity_encoding_is_not_reported() {
        let raw = b"plain".to_vec();
        let p = build_preview(raw, 5, false, Some("text/plain".into()), Some("identity".into()));
        assert_eq!(p.text.as_deref(), Some("plain"));
        assert!(p.decoded_from.is_none(), "identity should be filtered out");
    }

    #[test]
    fn preview_bad_gzip_falls_back_to_raw_bytes() {
        // Claims gzip but isn't: decode returns the original bytes unchanged.
        let raw = b"not actually gzip".to_vec();
        let p = build_preview(raw.clone(), raw.len() as u64, false, Some("text/plain".into()), Some("gzip".into()));
        assert_eq!(p.text.as_deref(), Some("not actually gzip"));
    }

    #[test]
    fn preview_unknown_type_sniffs_for_nul() {
        // No media type, no NUL bytes => treated as text.
        let text = build_preview(b"looks like text".to_vec(), 15, false, None, None);
        assert!(text.text.is_some());
        // No media type but contains a NUL => treated as binary.
        let bin = build_preview(vec![b'a', 0x00, b'b'], 3, false, None, None);
        assert!(bin.base64.is_some());
        assert!(bin.text.is_none());
    }

    #[test]
    fn collect_headers_preserves_order_and_duplicates() {
        let mut map = HeaderMap::new();
        map.append("x-a", "1".parse().unwrap());
        map.append("x-b", "2".parse().unwrap());
        map.append("x-a", "3".parse().unwrap());
        let headers = collect_headers(&map);
        // HeaderMap groups by name; both x-a values must survive.
        let x_a: Vec<&str> = headers
            .iter()
            .filter(|h| h.name == "x-a")
            .map(|h| h.value.as_str())
            .collect();
        assert_eq!(x_a, vec!["1", "3"]);
        assert_eq!(headers.iter().filter(|h| h.name == "x-b").count(), 1);
    }

    #[test]
    fn header_value_lookup() {
        let mut map = HeaderMap::new();
        map.insert("content-type", "application/json".parse().unwrap());
        assert_eq!(header_value(&map, "content-type").as_deref(), Some("application/json"));
        assert!(header_value(&map, "missing").is_none());
    }

    fn brotli(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut w = ::brotli::CompressorWriter::new(&mut out, 4096, 5, 22);
            w.write_all(data).unwrap();
        }
        out
    }

    #[test]
    fn preview_decodes_brotli() {
        let plain = "brotli compressed payload";
        let raw = brotli(plain.as_bytes());
        let p = build_preview(raw, plain.len() as u64, false, Some("text/plain".into()), Some("br".into()));
        assert_eq!(p.text.as_deref(), Some(plain));
        assert_eq!(p.decoded_from.as_deref(), Some("br"));
    }

    #[test]
    fn preview_truncated_flag_passes_through() {
        let p = build_preview(b"partial".to_vec(), 9_999, true, Some("text/plain".into()), None);
        assert!(p.truncated);
        assert_eq!(p.size, 9_999); // true size, not the retained slice length
    }

    #[test]
    fn now_ms_is_positive() {
        assert!(now_ms() > 0.0);
    }

    // ---- Shared store: insert/update semantics ----

    struct CountingSink(Mutex<Vec<Flow>>);
    impl FlowSink for CountingSink {
        fn emit(&self, flow: Flow) {
            self.0.lock().unwrap().push(flow);
        }
    }

    fn shared_with(sink: Arc<CountingSink>) -> Shared {
        Shared::new(
            sink,
            Arc::new(crate::flow::NoopWsSink),
            1024,
            Arc::new(RwLock::new(Vec::new())),
            Arc::new(crate::breakpoint::Breakpoints::new(Arc::new(
                crate::breakpoint::NoopBreakpointSink,
            ))),
            crate::scripting::ScriptEngine::new(),
            Arc::new(RwLock::new(NetworkConditions::default())),
            Arc::new(RwLock::new(TlsScope::default())),
        )
    }

    fn sample_flow(id: &str) -> Flow {
        new_flow(
            id.into(),
            0,
            "GET".into(),
            "https".into(),
            "example.com".into(),
            "/".into(),
            "https://example.com/".into(),
            "127.0.0.1:1".into(),
            "HTTP/1.1".into(),
            Vec::new(),
        )
    }

    #[test]
    fn insert_counts_stores_and_emits() {
        let sink = Arc::new(CountingSink(Mutex::new(Vec::new())));
        let shared = shared_with(sink.clone());

        shared.insert(sample_flow("f0"));
        assert_eq!(shared.total_captured.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert!(shared.flows.lock().unwrap().contains_key("f0"));
        assert_eq!(sink.0.lock().unwrap().len(), 1);
    }

    #[test]
    fn update_mutates_existing_and_emits_snapshot() {
        let sink = Arc::new(CountingSink(Mutex::new(Vec::new())));
        let shared = shared_with(sink.clone());
        shared.insert(sample_flow("f0"));

        shared.update("f0", |f| {
            f.state = FlowState::Completed;
            f.status = Some(200);
        });

        let stored = shared.flows.lock().unwrap().get("f0").cloned().unwrap();
        assert_eq!(stored.state, FlowState::Completed);
        assert_eq!(stored.status, Some(200));
        // insert emitted once, update emitted the new snapshot once more.
        let emitted = sink.0.lock().unwrap();
        assert_eq!(emitted.len(), 2);
        assert_eq!(emitted[1].status, Some(200));
    }

    #[test]
    fn update_missing_id_is_a_noop() {
        let sink = Arc::new(CountingSink(Mutex::new(Vec::new())));
        let shared = shared_with(sink.clone());

        shared.update("ghost", |f| f.status = Some(500));
        // No stored flow, and nothing emitted.
        assert!(shared.flows.lock().unwrap().is_empty());
        assert!(sink.0.lock().unwrap().is_empty());
    }

    #[test]
    fn new_flow_starts_in_started_state() {
        let f = new_flow(
            "id1".into(),
            7,
            "GET".into(),
            "https".into(),
            "example.com".into(),
            "/a".into(),
            "https://example.com/a".into(),
            "127.0.0.1:5000".into(),
            "HTTP/1.1".into(),
            vec![Header { name: "host".into(), value: "example.com".into() }],
        );
        assert_eq!(f.state, FlowState::Started);
        assert_eq!(f.seq, 7);
        assert!(f.status.is_none());
        assert!(f.response_headers.is_empty());
        assert!(!f.resent);
    }
}
