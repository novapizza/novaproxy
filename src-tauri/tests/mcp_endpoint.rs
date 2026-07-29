//! End-to-end test of the MCP endpoint: start the real server over HTTP, speak
//! JSON-RPC to it the way an MCP client does, and check the tool surface behaves
//! — including the MCP-only filter and the exclusion of NovaProxy's own traffic,
//! which is the whole point of the feature.

use std::sync::Arc;

use nova_proto::{BodyPreview, Flow, McpInfo, McpTransport};
use novaproxy_lib::mcp;
use novaproxy_lib::state::AppState;
use serde_json::{json, Value};

/// A JSON-RPC client for the endpoint, thin enough to show exactly what an MCP
/// client sends.
struct Client {
    http: reqwest::Client,
    url: String,
    next_id: std::cell::Cell<u64>,
}

impl Client {
    fn new(url: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            url,
            next_id: std::cell::Cell::new(1),
        }
    }

    async fn call(&self, method: &str, params: Value) -> Value {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let res = self
            .http
            .post(&self.url)
            .header("content-type", "application/json")
            // Both types, as the streamable-HTTP transport requires.
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", "2025-06-18")
            .body(body.to_string())
            .send()
            .await
            .expect("request reached the MCP endpoint");
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        assert!(status.is_success(), "{method} failed: {status} {text}");
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{method} returned non-JSON: {e}\n{text}"))
    }

    /// Call a tool and return its structured result.
    async fn tool(&self, name: &str, args: Value) -> Value {
        let res = self.call("tools/call", json!({"name": name, "arguments": args})).await;
        let result = res
            .get("result")
            .unwrap_or_else(|| panic!("{name} returned no result: {res}"));
        assert_ne!(
            result.get("isError"),
            Some(&json!(true)),
            "{name} reported an error: {result}"
        );
        // rmcp puts a typed tool return in structuredContent, with the JSON text
        // mirrored into content for clients that only read text.
        result
            .get("structuredContent")
            .cloned()
            .unwrap_or_else(|| result.clone())
    }

    /// Call a tool expecting failure, returning the error text.
    async fn tool_err(&self, name: &str, args: Value) -> String {
        let res = self.call("tools/call", json!({"name": name, "arguments": args})).await;
        res.to_string()
    }
}

fn flow(id: &str, host: &str, path: &str) -> Flow {
    nova_core::flow::new_flow(
        id.into(),
        0,
        "POST".into(),
        "https".into(),
        host.into(),
        path.into(),
        format!("https://{host}{path}"),
        "127.0.0.1:1".into(),
        "HTTP/1.1".into(),
        vec![nova_proto::Header {
            name: "content-type".into(),
            value: "application/json".into(),
        }],
    )
}

fn body(text: &str) -> BodyPreview {
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

/// Three flows: a plain API call, someone else's MCP server, and NovaProxy's own
/// MCP traffic.
fn seed(state: &AppState) {
    let mut plain = flow("f0", "api.example.com", "/v1/users");
    plain.status = Some(200);
    plain.process = Some("Google Chrome".into());
    plain.response_body = Some(body(r#"{"users":[{"name":"ada"}]}"#));
    state.flows.insert(plain);

    let mut theirs = flow("f1", "localhost", "/mcp");
    theirs.status = Some(200);
    theirs.process = Some("node".into());
    theirs.request_body = Some(body(
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"read_file"}}"#,
    ));
    theirs.mcp = Some(McpInfo {
        method: Some("tools/call".into()),
        tool: Some("read_file".into()),
        id: Some("4".into()),
        transport: McpTransport::Http,
    });
    state.flows.insert(theirs);

    let mut ours = flow("f2", "127.0.0.1", "/");
    ours.status = Some(200);
    ours.internal = true;
    ours.mcp = Some(McpInfo {
        method: Some("tools/call".into()),
        tool: Some("list_flows".into()),
        id: Some("9".into()),
        transport: McpTransport::Http,
    });
    state.flows.insert(ours);
}

fn ids(rows: &Value) -> Vec<String> {
    rows.as_array()
        .expect("tool returned an array")
        .iter()
        .map(|r| r["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_endpoint_serves_captured_traffic() {
    let dir = std::env::temp_dir().join(format!("novaproxy-mcp-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let state = Arc::new(AppState::new(dir.clone()));
    seed(&state);

    // Port 0: let the OS pick, so the test never collides with a real instance.
    let handle = mcp::start(state.clone(), 0).await.expect("MCP endpoint started");
    let client = Client::new(format!("http://127.0.0.1:{}/", handle.addr.port()));

    /* ------------------------------ handshake ------------------------------ */

    let init = client
        .call(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "novaproxy-test", "version": "0"}
            }),
        )
        .await;
    let result = &init["result"];
    assert_eq!(result["serverInfo"]["name"], "novaproxy");
    assert!(
        result["capabilities"]["tools"].is_object(),
        "server must advertise tools: {result}"
    );
    assert!(
        result["instructions"]
            .as_str()
            .unwrap_or_default()
            .contains("mcp_only"),
        "instructions should tell an agent how to isolate MCP traffic"
    );

    /* ------------------------------ tool list ------------------------------ */

    let listed = client.call("tools/list", json!({})).await;
    let names: Vec<String> = listed["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap_or_default().to_string())
        .collect();
    for expected in [
        "list_flows",
        "search_flows",
        "get_flow",
        "get_body",
        "replay_request",
        "list_rules",
        "set_rule",
        "delete_rule",
        "proxy_status",
    ] {
        assert!(names.contains(&expected.to_string()), "missing tool {expected}: {names:?}");
    }

    /* ------------------------------ list_flows ------------------------------ */

    let rows = client.tool("list_flows", json!({})).await;
    assert_eq!(
        ids(&rows),
        vec!["f1", "f0"],
        "newest first, and NovaProxy's own traffic (f2) is excluded by default"
    );

    let rows = client.tool("list_flows", json!({"include_internal": true})).await;
    assert_eq!(ids(&rows), vec!["f2", "f1", "f0"], "opt back in explicitly");

    /* ------------------------------- mcp_only ------------------------------- */

    let rows = client.tool("list_flows", json!({"mcp_only": true})).await;
    assert_eq!(ids(&rows), vec!["f1"], "only the MCP server being debugged");
    assert_eq!(
        rows[0]["mcp"], "tools/call → read_file",
        "the row names the method and tool, not just POST /mcp"
    );

    let rows = client
        .tool("list_flows", json!({"mcp_only": true, "include_internal": true}))
        .await;
    assert_eq!(ids(&rows), vec!["f2", "f1"]);

    /* ---------------------------- other filters ---------------------------- */

    let rows = client.tool("list_flows", json!({"app": "chrome"})).await;
    assert_eq!(ids(&rows), vec!["f0"]);
    let rows = client.tool("list_flows", json!({"host": "api.example"})).await;
    assert_eq!(ids(&rows), vec!["f0"]);
    let rows = client.tool("list_flows", json!({"limit": 1})).await;
    assert_eq!(rows.as_array().unwrap().len(), 1, "limit is honoured");

    /* ------------------------------- search -------------------------------- */

    let rows = client.tool("search_flows", json!({"query": "ada"})).await;
    assert_eq!(ids(&rows), vec!["f0"], "matches inside a captured body");
    let rows = client.tool("search_flows", json!({"query": "read_file"})).await;
    assert_eq!(ids(&rows), vec!["f1"]);
    let rows = client
        .tool("search_flows", json!({"query": "tools/call", "mcp_only": true}))
        .await;
    assert_eq!(ids(&rows), vec!["f1"], "filters compose with the query");

    /* ------------------------------ get_flow ------------------------------- */

    let detail = client.tool("get_flow", json!({"flow_id": "f1"})).await;
    assert_eq!(detail["mcp_detail"]["method"], "tools/call");
    assert_eq!(detail["mcp_detail"]["tool"], "read_file");
    assert_eq!(detail["mcp_detail"]["transport"], "Http");
    assert!(
        detail["request_headers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h[0] == "content-type"),
        "headers come back from get_flow: {detail}"
    );

    // Bodies are capped, and the cap is visible.
    let detail = client
        .tool("get_flow", json!({"flow_id": "f0", "body_chars": 5}))
        .await;
    let body = &detail["response_body"];
    assert_eq!(body["text"].as_str().unwrap().chars().count(), 5);
    assert_eq!(body["truncated"], true);
    assert_eq!(body["size_bytes"], 26, "the true size is still reported");

    /* ------------------------------ get_body ------------------------------- */

    let out = client
        .tool("get_body", json!({"flow_id": "f0", "side": "response"}))
        .await;
    assert!(out["text"].as_str().unwrap().contains("ada"));
    assert_eq!(out["truncated"], false, "the whole preview fits");

    /* -------------------------------- rules -------------------------------- */

    client
        .tool(
            "set_rule",
            json!({
                "id": "r-mcp",
                "kind": "map_remote",
                "pattern": "https://api.example.com/*",
                "target": "https://staging.example.com"
            }),
        )
        .await;
    let rules = client.tool("list_rules", json!({})).await;
    assert_eq!(rules.as_array().unwrap().len(), 1);
    assert_eq!(rules[0]["kind"], "MapRemote");
    assert_eq!(rules[0]["target"], "https://staging.example.com");
    assert!(
        state.rules_path().exists(),
        "a rule set by an agent must survive a restart, like one set in the UI"
    );

    client.tool("delete_rule", json!({"id": "r-mcp"})).await;
    let rules = client.tool("list_rules", json!({})).await;
    assert!(rules.as_array().unwrap().is_empty());

    /* --------------------------- proxy_status ------------------------------ */

    let status = client.tool("proxy_status", json!({})).await;
    assert_eq!(status["proxy_running"], false, "no engine started in this test");
    assert_eq!(status["flows_captured_this_session"], 3);
    assert_eq!(status["flows_retained"], 3);

    /* ------------------------------- errors -------------------------------- */

    let err = client.tool_err("get_flow", json!({"flow_id": "nope"})).await;
    assert!(
        err.contains("no flow with id nope"),
        "an unknown id must be a clear error, not empty data: {err}"
    );
    let err = client
        .tool_err("get_body", json!({"flow_id": "f0", "side": "sideways"}))
        .await;
    assert!(err.contains("request"), "bad side is rejected with guidance: {err}");
    let err = client
        .tool_err("set_rule", json!({"id": "bad", "kind": "MapRemote", "pattern": "*"}))
        .await;
    assert!(
        err.contains("target"),
        "a rule that could not act must not be silently accepted: {err}"
    );

    handle.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
