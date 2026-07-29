//! MCP traffic detection — recognising Model Context Protocol exchanges in
//! captured HTTP flows so they can be isolated from everything else.
//!
//! MCP is JSON-RPC 2.0 with a fixed method vocabulary. Over the wire we can see
//! two of its transports:
//!
//! * **Streamable HTTP** — a `POST` whose body is a JSON-RPC request/notification
//!   (or a batch of them), answered with JSON or an SSE stream.
//! * **SSE** — `text/event-stream` responses whose `data:` frames each carry one
//!   JSON-RPC message.
//!
//! The **stdio** transport never touches the network, so a proxy cannot see it at
//! all; that limitation is inherent, not a gap in this module.
//!
//! Detection is deliberately strict: a `jsonrpc: "2.0"` envelope alone is not
//! enough (plenty of non-MCP JSON-RPC APIs exist), so a request must also name a
//! known MCP method. Responses are only recognised as MCP when their id can be
//! matched to a request we already tagged, which the caller does by tagging the
//! flow, not this module.

use nova_proto::{McpInfo, McpTransport};

/// Method names defined by the MCP specification. Anything under a namespace we
/// know (`tools/`, `resources/`, `prompts/`, `notifications/`, `completion/`,
/// `logging/`, `roots/`, `sampling/`, `elicitation/`) counts, plus the two
/// top-level methods.
const MCP_NAMESPACES: &[&str] = &[
    "tools/",
    "resources/",
    "prompts/",
    "notifications/",
    "completion/",
    "logging/",
    "roots/",
    "sampling/",
    "elicitation/",
];
const MCP_METHODS: &[&str] = &["initialize", "ping"];

/// Is `method` an MCP method name?
pub fn is_mcp_method(method: &str) -> bool {
    MCP_METHODS.contains(&method) || MCP_NAMESPACES.iter().any(|ns| method.starts_with(ns))
}

/// Look for an MCP message in a request body.
///
/// `media_type` is the request's `Content-Type`; only JSON bodies are inspected.
/// Handles both a single JSON-RPC object and a batch array (the first MCP message
/// in a batch names the flow).
pub fn detect_request(media_type: Option<&str>, body: &str) -> Option<McpInfo> {
    if !is_json(media_type) {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(body.trim()).ok()?;
    let messages = match &value {
        serde_json::Value::Array(items) => items.iter().collect::<Vec<_>>(),
        other => vec![other],
    };
    messages
        .into_iter()
        .find_map(|m| from_message(m, McpTransport::Http))
}

/// Look for MCP messages in an SSE response body (`text/event-stream`), whose
/// `data:` frames each carry one JSON-RPC message. Used to catch server→client
/// traffic on a stream the client opened with `GET`.
pub fn detect_sse(media_type: Option<&str>, body: &str) -> Option<McpInfo> {
    if !media_type
        .map(|m| m.to_ascii_lowercase().contains("text/event-stream"))
        .unwrap_or(false)
    {
        return None;
    }
    for line in body.lines() {
        let Some(payload) = line.strip_prefix("data:") else { continue };
        let payload = payload.trim();
        if payload.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) {
            if let Some(info) = from_message(&value, McpTransport::Sse) {
                return Some(info);
            }
        }
    }
    None
}

/// Interpret one JSON-RPC message as MCP, if it is one.
fn from_message(msg: &serde_json::Value, transport: McpTransport) -> Option<McpInfo> {
    if msg.get("jsonrpc")?.as_str()? != "2.0" {
        return None;
    }
    let method = msg.get("method")?.as_str()?;
    if !is_mcp_method(method) {
        return None;
    }
    // `tools/call` carries the tool being invoked in `params.name`; surfacing it
    // is the difference between a list of identical `POST /mcp` rows and a
    // readable trace.
    let tool = msg
        .get("params")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .map(|s| s.to_string())
        .filter(|_| method.starts_with("tools/") || method.starts_with("prompts/"));
    Some(McpInfo {
        method: Some(method.to_string()),
        tool,
        // A message with no id is a notification, per JSON-RPC.
        id: msg.get("id").map(render_id),
        transport,
    })
}

/// JSON-RPC ids may be strings or numbers; render either as a string.
fn render_id(id: &serde_json::Value) -> String {
    match id {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn is_json(media_type: Option<&str>) -> bool {
    media_type
        .map(|m| {
            let m = m.to_ascii_lowercase();
            m.contains("json")
        })
        .unwrap_or(false)
}

/// Is this request aimed at NovaProxy's own MCP endpoint?
///
/// Traffic to our own endpoint is NovaProxy's own business, not the user's, so it
/// is marked internal and hidden from unfiltered views — otherwise an agent
/// reading the capture mostly sees the echo of its own tool calls.
///
/// `host` may or may not carry a `:port` (it does when it came from a `Host`
/// header), so a port embedded there is honoured when `port` is `None`.
/// `internal_port` of 0 means the endpoint is not running and nothing matches.
pub fn is_internal_endpoint(host: &str, port: Option<u16>, internal_port: u16) -> bool {
    if internal_port == 0 {
        return false;
    }
    let (name, embedded_port) = match host.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => {
            (h, p.parse::<u16>().ok())
        }
        _ => (host, None),
    };
    let name = name.trim_matches(|c| c == '[' || c == ']').to_ascii_lowercase();
    let loopback = name == "localhost" || name == "127.0.0.1" || name == "::1";
    loopback && port.or(embedded_port) == Some(internal_port)
}

/// Short display summary for a tagged flow: `tools/call → search_flows`.
pub fn summary(info: &McpInfo) -> String {
    match (&info.method, &info.tool) {
        (Some(m), Some(t)) => format!("{m} → {t}"),
        (Some(m), None) => m.clone(),
        (None, _) => "mcp".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: Option<&str> = Some("application/json");

    #[test]
    fn detects_a_tools_call_and_names_the_tool() {
        let body = r#"{"jsonrpc":"2.0","id":7,"method":"tools/call",
                       "params":{"name":"search_flows","arguments":{"query":"api"}}}"#;
        let info = detect_request(JSON, body).expect("detected");
        assert_eq!(info.method.as_deref(), Some("tools/call"));
        assert_eq!(info.tool.as_deref(), Some("search_flows"));
        assert_eq!(info.id.as_deref(), Some("7"));
        assert_eq!(info.transport, McpTransport::Http);
        assert_eq!(summary(&info), "tools/call → search_flows");
    }

    #[test]
    fn detects_initialize_and_notifications() {
        let init = detect_request(
            JSON,
            r#"{"jsonrpc":"2.0","id":"a","method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
        )
        .expect("initialize is MCP");
        assert_eq!(init.method.as_deref(), Some("initialize"));
        assert_eq!(init.id.as_deref(), Some("a"), "string ids render as themselves");

        let note = detect_request(JSON, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .expect("notification is MCP");
        assert!(note.id.is_none(), "a notification has no id");
    }

    #[test]
    fn does_not_claim_unrelated_json_rpc() {
        // A JSON-RPC 2.0 envelope is not enough — Ethereum, LSP-over-HTTP and
        // plenty of internal APIs use it. Requiring a known MCP method is what
        // keeps the "MCP only" filter meaningful.
        assert!(detect_request(JSON, r#"{"jsonrpc":"2.0","id":1,"method":"eth_blockNumber"}"#).is_none());
        assert!(detect_request(JSON, r#"{"jsonrpc":"2.0","id":1,"method":"textDocument/didOpen"}"#).is_none());
    }

    #[test]
    fn ignores_non_json_and_unparsable_bodies() {
        assert!(detect_request(Some("text/plain"), r#"{"jsonrpc":"2.0","method":"ping"}"#).is_none());
        assert!(detect_request(JSON, "not json at all").is_none());
        assert!(detect_request(JSON, "").is_none());
        assert!(detect_request(None, r#"{"jsonrpc":"2.0","method":"ping"}"#).is_none());
    }

    #[test]
    fn ignores_json_rpc_1_0() {
        assert!(detect_request(JSON, r#"{"jsonrpc":"1.0","id":1,"method":"tools/list"}"#).is_none());
        assert!(detect_request(JSON, r#"{"id":1,"method":"tools/list"}"#).is_none());
    }

    #[test]
    fn finds_the_first_mcp_message_in_a_batch() {
        let body = r#"[{"jsonrpc":"2.0","id":1,"method":"eth_call"},
                       {"jsonrpc":"2.0","id":2,"method":"tools/list"}]"#;
        let info = detect_request(JSON, body).expect("batch contains an MCP call");
        assert_eq!(info.method.as_deref(), Some("tools/list"));
        assert_eq!(info.id.as_deref(), Some("2"));
    }

    #[test]
    fn json_content_type_variants_are_accepted() {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        for ct in [
            "application/json",
            "application/json; charset=utf-8",
            "application/vnd.api+json",
        ] {
            assert!(detect_request(Some(ct), body).is_some(), "{ct} should be inspected");
        }
    }

    #[test]
    fn detects_mcp_inside_an_sse_stream() {
        let body = "event: message\n\
                    data: {\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"list_flows\"}}\n\
                    \n";
        let info = detect_sse(Some("text/event-stream"), body).expect("detected in SSE");
        assert_eq!(info.method.as_deref(), Some("tools/call"));
        assert_eq!(info.tool.as_deref(), Some("list_flows"));
        assert_eq!(info.transport, McpTransport::Sse, "transport is reported as SSE");
    }

    #[test]
    fn sse_detection_skips_keepalives_and_non_mcp_frames() {
        let body = ": keepalive\n\
                    data: \n\
                    data: {\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"eth_call\"}\n\
                    data: {\"hello\":true}\n";
        assert!(detect_sse(Some("text/event-stream"), body).is_none());
    }

    #[test]
    fn sse_detection_requires_the_event_stream_content_type() {
        let body = "data: {\"jsonrpc\":\"2.0\",\"method\":\"ping\"}\n";
        assert!(detect_sse(Some("application/json"), body).is_none());
        assert!(detect_sse(None, body).is_none());
    }

    #[test]
    fn method_vocabulary_covers_the_spec_namespaces() {
        for m in [
            "initialize",
            "ping",
            "tools/list",
            "tools/call",
            "resources/read",
            "resources/subscribe",
            "prompts/get",
            "notifications/cancelled",
            "completion/complete",
            "logging/setLevel",
            "roots/list",
            "sampling/createMessage",
            "elicitation/create",
        ] {
            assert!(is_mcp_method(m), "{m} should be recognised");
        }
        for m in ["eth_call", "tools", "toolsfoo", "", "subscribe"] {
            assert!(!is_mcp_method(m), "{m} should not be recognised");
        }
    }

    #[test]
    fn recognises_our_own_endpoint_on_loopback() {
        // Port from the URI authority, and from a Host header spelling.
        assert!(is_internal_endpoint("127.0.0.1", Some(9091), 9091));
        assert!(is_internal_endpoint("127.0.0.1:9091", None, 9091));
        assert!(is_internal_endpoint("localhost:9091", None, 9091));
        assert!(is_internal_endpoint("[::1]:9091", None, 9091));
    }

    #[test]
    fn other_hosts_and_ports_are_not_ours() {
        // A different port on loopback is somebody else's server — very much the
        // traffic the user wants to see.
        assert!(!is_internal_endpoint("127.0.0.1:3000", None, 9091));
        assert!(!is_internal_endpoint("example.com", Some(9091), 9091));
        assert!(!is_internal_endpoint("127.0.0.1", None, 9091), "no port, no match");
        // Disabled endpoint: nothing is internal.
        assert!(!is_internal_endpoint("127.0.0.1:9091", None, 0));
    }

    #[test]
    fn tool_name_is_only_taken_from_tool_and_prompt_calls() {
        // `resources/read` has params.uri, not params.name; a `name` on some other
        // method must not be mislabelled as a tool.
        let info = detect_request(
            JSON,
            r#"{"jsonrpc":"2.0","id":1,"method":"logging/setLevel","params":{"name":"debug"}}"#,
        )
        .unwrap();
        assert!(info.tool.is_none());
        assert_eq!(summary(&info), "logging/setLevel");
    }
}
