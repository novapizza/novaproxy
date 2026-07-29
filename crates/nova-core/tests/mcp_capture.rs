//! MCP recognition through the live proxy: a real JSON-RPC exchange with a local
//! "MCP server" must come out of the engine tagged with its method and tool,
//! while ordinary JSON traffic must not — that distinction is what makes the
//! "MCP only" filter trustworthy.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use nova_core::{ca::CaMaterial, EngineConfig, EngineHooks, FlowSink};
use nova_proto::{Flow, FlowState, McpTransport};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone)]
struct VecSink(Arc<Mutex<Vec<Flow>>>);
impl FlowSink for VecSink {
    fn emit(&self, flow: Flow) {
        self.0.lock().unwrap().push(flow);
    }
}

/// Upstream that answers any request with a JSON-RPC result.
async fn spawn_upstream() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 8192];
                let _ = sock.read(&mut buf).await;
                let body = r#"{"jsonrpc":"2.0","id":1,"result":{"content":[]}}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.flush().await;
            });
        }
    });
    port
}

/// POST `body` through the proxy, optionally marking it as NovaProxy's own.
async fn post_through_proxy(
    proxy_port: u16,
    upstream_port: u16,
    path: &str,
    content_type: &str,
    body: &str,
    internal: bool,
) {
    let mut stream = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
    let marker = if internal { "X-Nova-Internal: 1\r\n" } else { "" };
    let req = format!(
        "POST http://127.0.0.1:{upstream_port}{path} HTTP/1.1\r\n\
         Host: 127.0.0.1:{upstream_port}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         {marker}Connection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = stream.read_to_end(&mut out).await;
}

async fn wait_for(flows: &Arc<Mutex<Vec<Flow>>>, path: &str) -> Option<Flow> {
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let found = flows
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|f| f.path == path && f.state == FlowState::Completed)
            .cloned();
        if found.is_some() {
            return found;
        }
    }
    None
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_traffic_is_recognised_and_tagged() {
    let upstream = spawn_upstream().await;

    let dir = std::env::temp_dir().join(format!("novaproxy-mcp-capture-{upstream}"));
    let _ = std::fs::remove_dir_all(&dir);
    let ca = CaMaterial::load_or_create(&dir).unwrap();

    let sink = Arc::new(VecSink(Arc::new(Mutex::new(Vec::new()))));
    let flows = sink.0.clone();
    let hooks = EngineHooks::in_memory(Arc::new(nova_core::breakpoint::Breakpoints::new(
        Arc::new(nova_core::breakpoint::NoopBreakpointSink),
    )));

    let proxy_port = 39_401u16;
    let handle = nova_core::start(
        EngineConfig { addr: ([127, 0, 0, 1], proxy_port).into() },
        &ca,
        sink,
        Arc::new(nova_core::NoopWsSink),
        hooks,
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    // 1. A real MCP tools/call.
    post_through_proxy(
        proxy_port,
        upstream,
        "/mcp",
        "application/json",
        r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"read_file","arguments":{"path":"/tmp/x"}}}"#,
        false,
    )
    .await;

    // 2. Ordinary JSON that happens to be JSON-RPC, but not MCP.
    post_through_proxy(
        proxy_port,
        upstream,
        "/rpc",
        "application/json",
        r#"{"jsonrpc":"2.0","id":1,"method":"eth_blockNumber","params":[]}"#,
        false,
    )
    .await;

    // 3. A request NovaProxy issued itself (as the MCP server's replay does).
    post_through_proxy(
        proxy_port,
        upstream,
        "/internal",
        "application/json",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        true,
    )
    .await;

    let mcp_flow = wait_for(&flows, "/mcp").await.expect("no flow for /mcp");
    let info = mcp_flow.mcp.expect("the tools/call must be tagged as MCP");
    assert_eq!(info.method.as_deref(), Some("tools/call"));
    assert_eq!(info.tool.as_deref(), Some("read_file"));
    assert_eq!(info.id.as_deref(), Some("7"));
    assert_eq!(info.transport, McpTransport::Http);
    assert!(!mcp_flow.internal, "someone else's MCP server is not our traffic");

    let other = wait_for(&flows, "/rpc").await.expect("no flow for /rpc");
    assert!(
        other.mcp.is_none(),
        "non-MCP JSON-RPC must not be tagged, or the MCP filter means nothing"
    );

    let ours = wait_for(&flows, "/internal").await.expect("no flow for /internal");
    assert!(ours.internal, "the x-nova-internal marker must be honoured");
    assert!(
        !ours
            .request_headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("x-nova-internal")),
        "the marker is ours and must be stripped before forwarding"
    );

    handle.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
