//! Shared wire types between the Rust backend and the TypeScript frontend.
//!
//! Every type derives [`ts_rs::TS`] with `#[ts(export)]`, so running
//! `cargo test -p nova-proto` regenerates the matching `.ts` files into
//! `src/bindings/`. Keep this crate free of engine dependencies so the
//! contract stays small and stable.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A single HTTP header, preserving order and duplicates (unlike a map).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/bindings/")]
pub struct Header {
    pub name: String,
    pub value: String,
}

/// A captured (possibly truncated, possibly decoded) message body.
///
/// Bodies are never required to live fully in memory: `size` is the true
/// wire size, while `text`/`base64` hold at most a capped preview.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/bindings/")]
pub struct BodyPreview {
    /// Full decoded size in bytes (may exceed the captured preview).
    pub size: u64,
    /// True when the preview was cut off at the memory cap.
    pub truncated: bool,
    /// The `Content-Type` media type, if known.
    pub media_type: Option<String>,
    /// The `Content-Encoding` that was decoded away, if any.
    pub decoded_from: Option<String>,
    /// UTF-8 preview when the (decoded) body is text.
    pub text: Option<String>,
    /// Base64 preview when the (decoded) body is binary.
    pub base64: Option<String>,
}

/// Lifecycle of a flow as it streams through the proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/bindings/")]
pub enum FlowState {
    /// Request seen; response not yet complete.
    Started,
    /// Held at a breakpoint, awaiting the user's continue/abort decision.
    Paused,
    /// Request and response fully captured.
    Completed,
    /// The exchange failed (connect error, TLS abort, upstream error).
    Error,
}

/// Simulated network conditions applied to responses (throttling / latency).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/bindings/")]
pub struct NetworkConditions {
    pub enabled: bool,
    /// Extra delay added before each response starts, in milliseconds.
    pub latency_ms: u32,
    /// Downlink cap in kilobits/sec applied to response bodies (0 = unlimited).
    pub down_kbps: u32,
}

/// A request held at a breakpoint, streamed to the UI so it can be edited and
/// then continued or aborted.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/bindings/")]
pub struct Interception {
    pub id: String,
    pub method: String,
    pub url: String,
    pub request_headers: Vec<Header>,
}

/// One request/response exchange captured by the proxy engine.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/bindings/")]
pub struct Flow {
    /// Stable unique id assigned in `handle_request`.
    pub id: String,
    /// Monotonic sequence number for stable ordering / display index.
    pub seq: u64,
    pub method: String,
    pub scheme: String,
    pub host: String,
    pub path: String,
    pub url: String,
    /// Remote client socket address (`ip:port`).
    pub client_addr: String,
    pub http_version: String,
    pub state: FlowState,
    pub status: Option<u16>,
    pub request_headers: Vec<Header>,
    pub response_headers: Vec<Header>,
    pub request_body: Option<BodyPreview>,
    pub response_body: Option<BodyPreview>,
    /// Bytes seen on the wire for the request body.
    pub request_size: u64,
    /// Bytes seen on the wire for the response body.
    pub response_size: u64,
    /// Response `Content-Type`, convenient for filtering.
    pub content_type: Option<String>,
    /// Epoch milliseconds when the request was first seen.
    pub started_at: f64,
    /// Total wall-clock duration once completed.
    pub duration_ms: Option<f64>,
    pub error: Option<String>,
    /// True when this flow was produced by a Resend/Replay action.
    pub resent: bool,
    /// Original host if a Map Remote rule rewrote this request's destination.
    pub mapped_from: Option<String>,
}

/// Current state of the proxy engine, returned by the `proxy_status` command.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/bindings/")]
pub struct ProxyStatus {
    pub running: bool,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub flows_captured: u64,
    /// Whether the OS system proxy is currently pointed at NovaProxy.
    pub system_proxy: bool,
}

/// A traffic-control rule. The action is selected by [`RuleKind`]; the relevant
/// optional fields are interpreted per kind (documented on each variant).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/bindings/")]
pub struct Rule {
    pub id: String,
    pub enabled: bool,
    pub kind: RuleKind,
    pub name: String,
    /// URL glob to match, `*` = wildcard, tested against `scheme://host/path`.
    pub pattern: String,
    /// MapRemote: replacement base URL. MapLocal: absolute file path.
    pub target: Option<String>,
    /// Rewrite: header name to set.
    pub header_name: Option<String>,
    /// Rewrite: header value to set.
    pub header_value: Option<String>,
}

/// What a matching [`Rule`] does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/bindings/")]
pub enum RuleKind {
    /// Rewrite the request's destination host/scheme to `target`.
    MapRemote,
    /// Short-circuit the request, serving the file at `target` as the response.
    MapLocal,
    /// Short-circuit the request with a 403.
    Block,
    /// Set `header_name: header_value` on the request before forwarding.
    Rewrite,
}

/// Status of NovaProxy's root CA and its trust in the OS store.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/bindings/")]
pub struct CaStatus {
    /// Absolute path of the persisted `ca.pem`.
    pub cert_path: String,
    /// SHA-256 fingerprint (uppercase hex, colon-separated).
    pub fingerprint: String,
    /// Whether the CA is currently trusted in the system store.
    pub trusted: bool,
    /// Human-readable subject line.
    pub subject: String,
}
