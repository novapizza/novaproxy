//! End-to-end test of the capture pipeline over a real socket: a local upstream
//! server, the NovaProxy engine in front of it, and a hand-rolled HTTP proxy
//! client. Asserts the engine forwards the exchange AND records a completed flow
//! with both bodies captured.

use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use nova_core::{ca::CaMaterial, EngineConfig, FlowSink};
use nova_proto::{Flow, FlowState};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone)]
struct VecSink(Arc<Mutex<Vec<Flow>>>);
impl FlowSink for VecSink {
    fn emit(&self, flow: Flow) {
        self.0.lock().unwrap().push(flow);
    }
}

const RESPONSE_BODY: &str = r#"{"ok":true,"msg":"hello from upstream"}"#;

/// Minimal HTTP/1.1 upstream that replies to the first request with a JSON body.
async fn spawn_upstream() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await; // consume request headers
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    RESPONSE_BODY.len(),
                    RESPONSE_BODY
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.flush().await;
            });
        }
    });
    port
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn captures_http_flow_end_to_end() {
    let upstream_port = spawn_upstream().await;

    let ca_dir = std::env::temp_dir().join(format!("novaproxy-test-ca-{upstream_port}"));
    let _ = std::fs::remove_dir_all(&ca_dir);
    let ca = CaMaterial::load_or_create(&ca_dir).unwrap();

    let sink = Arc::new(VecSink(Arc::new(Mutex::new(Vec::new()))));
    let flows = sink.0.clone();

    let proxy_port = 39_099u16;
    let handle = nova_core::start(
        EngineConfig {
            addr: ([127, 0, 0, 1], proxy_port).into(),
            body_cap: nova_core::DEFAULT_BODY_CAP,
        },
        &ca,
        sink,
        nova_core::EngineHooks {
            rules: Arc::new(RwLock::new(Vec::new())),
            breakpoints: Arc::new(nova_core::breakpoint::Breakpoints::new(Arc::new(
                nova_core::breakpoint::NoopBreakpointSink,
            ))),
            scripts: nova_core::scripting::ScriptEngine::new(),
            net: Arc::new(RwLock::new(Default::default())),
        },
    )
    .unwrap();

    // Give the listener a moment to bind.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Speak HTTP-proxy to the engine: absolute-form request line.
    let mut stream = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
    let target = format!("http://127.0.0.1:{upstream_port}/hello");
    let req = format!(
        "GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{upstream_port}\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await.unwrap();

    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    let response = String::from_utf8_lossy(&response);

    // 1. The proxy forwarded the real upstream body to the client.
    assert!(
        response.contains(RESPONSE_BODY),
        "client did not receive upstream body; got:\n{response}"
    );

    // 2. Body finalization is async — wait for the completed snapshot.
    let mut completed: Option<Flow> = None;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        completed = flows
            .lock()
            .unwrap()
            .iter()
            .find(|f| f.state == FlowState::Completed)
            .cloned();
        if completed.is_some() {
            break;
        }
    }

    let flow = completed.expect("engine never emitted a completed flow");
    assert_eq!(flow.method, "GET");
    assert_eq!(flow.host, "127.0.0.1");
    assert_eq!(flow.status, Some(200));
    assert_eq!(flow.scheme, "http");
    let body = flow.response_body.expect("no response body captured");
    assert!(
        body.text.as_deref().unwrap_or("").contains("hello from upstream"),
        "response body not captured: {:?}",
        body.text
    );
    assert!(
        flow.content_type.as_deref().unwrap_or("").contains("json"),
        "content-type not recorded"
    );

    handle.stop();
    let _ = std::fs::remove_dir_all(&ca_dir);
}
