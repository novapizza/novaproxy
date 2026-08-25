//! Shared wire types between the Rust backend and the TypeScript frontend.
//!
//! Every type derives [`ts_rs::TS`] with `#[ts(export)]`, so running
//! `cargo test -p nova-proto` regenerates the matching `.ts` files into
//! `src/bindings/`. Keep this crate free of engine dependencies so the
//! contract stays small and stable.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A single HTTP header, preserving order and duplicates (unlike a map).
///
/// `PartialEq` so a caller can ask whether a header set actually changed —
/// which is what separates "a script ran" from "a script edited this".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
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
    /// True when the full body was written to the on-disk body store and can be
    /// fetched with the `read_body` command, rather than only existing as the
    /// truncated preview above.
    pub spilled: bool,
}

/// Measured timing breakdown of one exchange, in milliseconds.
///
/// Every field is a real measurement or `None` — nothing here is estimated. A
/// phase is `None` when it genuinely did not happen or could not be observed:
/// DNS/connect/TLS are absent when the flow reused a pooled connection (see
/// `connection_reused`), and `tls` is absent on plaintext HTTP.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/bindings/")]
pub struct Timings {
    /// Resolving the host name.
    pub dns_ms: Option<f64>,
    /// TCP handshake to the origin (or upstream proxy).
    pub connect_ms: Option<f64>,
    /// TLS handshake with the origin.
    pub tls_ms: Option<f64>,
    /// True when the request went out on an already-open connection, so no
    /// DNS/connect/TLS cost is attributable to it.
    pub connection_reused: bool,
    /// Streaming the request body upstream (absent when there was no body).
    pub request_ms: Option<f64>,
    /// Request first seen → upstream response headers received.
    pub ttfb_ms: Option<f64>,
    /// Response headers → last response body byte.
    pub download_ms: Option<f64>,
}

/// Which MCP transport a captured exchange used. The stdio transport never
/// reaches the network, so it can never appear here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/bindings/")]
pub enum McpTransport {
    /// Streamable HTTP: a JSON-RPC message posted to an MCP endpoint.
    Http,
    /// Server-sent events: JSON-RPC messages inside `data:` frames.
    Sse,
}

/// What a captured flow carries when it is a Model Context Protocol exchange.
/// Present only on flows recognised as MCP (see `nova_core::mcp`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/bindings/")]
pub struct McpInfo {
    /// JSON-RPC method, e.g. `tools/call`.
    pub method: Option<String>,
    /// For `tools/*` and `prompts/*`, the tool or prompt being invoked.
    pub tool: Option<String>,
    /// JSON-RPC id, rendered as a string. Absent for notifications.
    pub id: Option<String>,
    pub transport: McpTransport,
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

/// Which hosts NovaProxy decrypts (MITM) versus tunnels through untouched.
///
/// A host that pins its certificate or requires mTLS will abort NovaProxy's
/// leaf cert; tunneling it raw keeps the app working at the cost of visibility.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/bindings/")]
pub struct TlsScope {
    /// When true, decrypt everything except hosts matching `exclude`. When
    /// false, decrypt only hosts matching `include` (tunnel everything else).
    pub intercept_all: bool,
    /// Host globs to decrypt (used when `intercept_all` is false).
    pub include: Vec<String>,
    /// Host globs to tunnel without decrypting (used when `intercept_all`).
    pub exclude: Vec<String>,
}

impl Default for TlsScope {
    /// Decrypt everything, exclude nothing — the least-surprising default.
    fn default() -> Self {
        Self {
            intercept_all: true,
            include: Vec::new(),
            exclude: Vec::new(),
        }
    }
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
    /// PID of the local process that originated this request, if resolved.
    pub pid: Option<u32>,
    /// Name of the originating process (e.g. "Google Chrome"), if resolved.
    /// Best-effort; `None` when attribution failed or on unsupported platforms.
    pub process: Option<String>,
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
    /// Measured per-phase breakdown (DNS/connect/TLS/request/TTFB/download).
    pub timings: Timings,
    pub error: Option<String>,
    /// True when this flow was produced by a Resend/Replay action.
    pub resent: bool,
    /// Original host if a Map Remote rule rewrote this request's destination.
    pub mapped_from: Option<String>,
    /// True when this flow is a WebSocket upgrade; its frames stream on the
    /// dedicated WS channel keyed by [`Flow::id`].
    pub is_websocket: bool,
    /// True when this CONNECT was tunneled without decryption (per the TLS
    /// scope): only the host is known, no request/response bodies are captured.
    pub tunneled: bool,
    /// Set when this exchange is Model Context Protocol traffic, so MCP work can
    /// be isolated from everything else in the capture.
    pub mcp: Option<McpInfo>,
    /// True when NovaProxy itself produced this flow — a call to its own MCP
    /// endpoint, or a request its MCP server replayed. Excluded by default from
    /// the flow list and from MCP tool results, so an agent inspecting traffic
    /// does not mostly see itself.
    pub internal: bool,
    /// What NovaProxy changed about this exchange on its way through.
    pub edits: Edits,
}

/// What changed an exchange after it left the client.
///
/// Three flags rather than one boolean, because "why is the response wrong?"
/// has three different answers and the table's Edited column is where that
/// question starts. Each is set only when something actually changed: a rule
/// that matched but had nothing to rewrite, or a script hook that returned the
/// headers it was given, leaves no mark — a marker that means "a rule exists"
/// rather than "this request was altered" would be noise on every row.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/bindings/")]
pub struct Edits {
    /// A rule rewrote this: Block, Map Local, Map Remote, or a header Rewrite.
    pub rule: bool,
    /// The script hook changed headers, or aborted the request.
    pub script: bool,
    /// A person edited it while it was paused at a breakpoint.
    pub breakpoint: bool,
}

impl Edits {
    /// True when anything at all touched the exchange.
    pub fn any(&self) -> bool {
        self.rule || self.script || self.breakpoint
    }
}

/// Direction of a captured WebSocket frame, from the client's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/bindings/")]
pub enum WsDirection {
    /// Client → server (a frame the app under test sent).
    Sent,
    /// Server → client (a frame the app under test received).
    Received,
}

/// The kind of a captured WebSocket frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/bindings/")]
pub enum WsOpcode {
    Text,
    Binary,
    Ping,
    Pong,
    Close,
}

/// A single WebSocket frame captured on an upgraded [`Flow`]. Frames are
/// streamed to the UI on their own channel (keyed by [`WsMessage::flow_id`])
/// rather than folded into the flow snapshot, so a chatty socket doesn't
/// re-emit the whole flow per message.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/bindings/")]
pub struct WsMessage {
    /// Id of the upgrade [`Flow`] this frame belongs to.
    pub flow_id: String,
    /// Monotonic per-flow sequence for stable ordering.
    pub seq: u64,
    pub direction: WsDirection,
    pub opcode: WsOpcode,
    /// Full payload size in bytes (may exceed the retained preview).
    pub size: u64,
    /// True when the preview was cut off at the memory cap.
    pub truncated: bool,
    /// UTF-8 preview for text frames.
    pub text: Option<String>,
    /// Base64 preview for binary/control payloads.
    pub base64: Option<String>,
    /// Epoch milliseconds when the frame was seen.
    pub at: f64,
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
    /// A snapshot from a previous session is still waiting to be put back — the
    /// app exited uncleanly while the system proxy was on, and restoring it
    /// needs a privilege the app does not have unattended. The UI offers the
    /// restore instead of the old behaviour, which raised a password dialog
    /// during launch.
    pub pending_restore: bool,
}

/// State of the macOS privileged helper, which applies system-proxy changes
/// without an administrator password.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/bindings/")]
pub struct HelperStatus {
    /// Whether this platform needs a helper at all (macOS only; elsewhere the
    /// proxy settings are per-user and need no privileges).
    pub supported: bool,
    /// A helper is installed and answering.
    pub running: bool,
    /// Protocol version it answered with, when running.
    pub version: Option<u32>,
    /// Protocol version this build speaks. A mismatch means "reinstall".
    pub expected_version: u32,
    /// A helper binary was found to install from. False in a build tree that
    /// never built `nova-helper`.
    pub installable: bool,
}

/// State of the MCP endpoint that exposes captured traffic to AI tooling.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/bindings/")]
pub struct McpStatus {
    pub running: bool,
    /// Port it is (or would be) served on.
    pub port: u16,
    /// Endpoint URL to hand an MCP client, when running.
    pub url: Option<String>,
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
///
/// Trust is reported *per domain* because both can hold the cert: the default
/// install targets the current user's login keychain (no admin password), while
/// "install for all users" targets the machine-wide system store. Four states
/// are reachable — user / system / both / neither.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/bindings/")]
pub struct CaStatus {
    /// Absolute path of the persisted `ca.pem`.
    pub cert_path: String,
    /// SHA-256 fingerprint (uppercase hex, colon-separated).
    pub fingerprint: String,
    /// Whether the CA is trusted in *at least one* domain, i.e. whether HTTPS
    /// interception works for this user's apps.
    pub trusted: bool,
    /// Trusted in the current user's login keychain (installed without admin).
    pub trusted_user: bool,
    /// Trusted machine-wide, for every user and root-owned daemon.
    pub trusted_system: bool,
    /// Human-readable subject line.
    pub subject: String,
    /// Host platform (`macos`, `windows`, `linux`, …). The two trust domains mean
    /// materially different things per platform — notably, Linux's user domain
    /// covers browsers only — so the UI phrases them accordingly.
    pub platform: String,
}

/// What an update check found.
///
/// `configured` is the honest answer to "can this build update itself at all":
/// the updater endpoint and public key are injected at release time, so a
/// development build has neither. Reporting it lets the UI say so instead of
/// offering a button that can only fail.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/bindings/")]
pub struct UpdateStatus {
    /// Version this build reports, i.e. what an update would replace.
    pub current_version: String,
    /// Whether this build has an updater endpoint and a signing key to verify
    /// against.
    pub configured: bool,
    /// A newer version is published and passes signature verification.
    pub available: bool,
    /// Version on offer, when one is.
    pub version: Option<String>,
    /// Release notes from the manifest.
    pub notes: Option<String>,
    /// Publication date from the manifest, as the manifest spelled it.
    pub date: Option<String>,
}

/// Progress of an update download, streamed while it runs.
///
/// `total` is optional because it comes from a `Content-Length` the server is
/// not obliged to send; the UI shows a determinate bar only when it is present.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/bindings/")]
pub struct UpdateProgress {
    pub downloaded: u64,
    pub total: Option<u64>,
    /// The bytes are in; installing (and then restarting) is next.
    pub done: bool,
}
