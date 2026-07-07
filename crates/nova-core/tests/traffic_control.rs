//! End-to-end traffic-control tests over a real socket. Each spins up the
//! NovaProxy engine and speaks HTTP-proxy (absolute-form request line) to it,
//! asserting rule short-circuits (Block / Map Local), body truncation at the
//! capture cap, and transparent decoding of a compressed response.

use std::io::Write as _;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use nova_core::{ca::CaMaterial, EngineConfig, EngineHooks, FlowSink};
use nova_proto::{Flow, FlowState, Rule, RuleKind};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone)]
struct VecSink(Arc<Mutex<Vec<Flow>>>);
impl FlowSink for VecSink {
    fn emit(&self, flow: Flow) {
        self.0.lock().unwrap().push(flow);
    }
}

/// Start the engine on `port` with the given rules and body cap; return the
/// handle and the shared flow log.
fn start_engine(
    port: u16,
    body_cap: usize,
    rules: Vec<Rule>,
    tag: &str,
) -> (nova_core::EngineHandle, Arc<Mutex<Vec<Flow>>>, std::path::PathBuf) {
    let ca_dir = std::env::temp_dir().join(format!("novaproxy-tc-ca-{tag}"));
    let _ = std::fs::remove_dir_all(&ca_dir);
    let ca = CaMaterial::load_or_create(&ca_dir).unwrap();

    let sink = Arc::new(VecSink(Arc::new(Mutex::new(Vec::new()))));
    let flows = sink.0.clone();

    let handle = nova_core::start(
        EngineConfig { addr: ([127, 0, 0, 1], port).into(), body_cap },
        &ca,
        sink,
        Arc::new(nova_core::NoopWsSink),
        EngineHooks {
            rules: Arc::new(RwLock::new(rules)),
            breakpoints: Arc::new(nova_core::breakpoint::Breakpoints::new(Arc::new(
                nova_core::breakpoint::NoopBreakpointSink,
            ))),
            scripts: nova_core::scripting::ScriptEngine::new(),
            net: Arc::new(RwLock::new(Default::default())),
            tls_scope: Arc::new(RwLock::new(Default::default())),
        },
    )
    .unwrap();
    (handle, flows, ca_dir)
}

fn rule(kind: RuleKind, pattern: &str) -> Rule {
    Rule {
        id: "r".into(),
        enabled: true,
        kind,
        name: "test".into(),
        pattern: pattern.into(),
        target: None,
        header_name: None,
        header_value: None,
    }
}

/// Send one absolute-form proxy request and read the full response.
async fn proxy_get(port: u16, target: &str, host: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let req = format!(
        "GET {target} HTTP/1.1\r\nHost: {host}\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    String::from_utf8_lossy(&response).into_owned()
}

/// Poll the flow log for a snapshot matching `pred`.
async fn wait_for(flows: &Arc<Mutex<Vec<Flow>>>, pred: impl Fn(&Flow) -> bool) -> Option<Flow> {
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if let Some(f) = flows.lock().unwrap().iter().find(|f| pred(f)).cloned() {
            return Some(f);
        }
    }
    None
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn block_rule_short_circuits_with_403() {
    let (handle, flows, ca_dir) =
        start_engine(39_101, nova_core::DEFAULT_BODY_CAP, vec![rule(RuleKind::Block, "*blocked*")], "block");
    tokio::time::sleep(Duration::from_millis(300)).await;

    // No upstream exists; a Block must short-circuit before any forward.
    let response = proxy_get(39_101, "http://blocked.example/blocked/path", "blocked.example").await;
    assert!(response.contains("403"), "expected 403 status line, got:\n{response}");
    assert!(response.contains("Blocked by NovaProxy rule"), "missing block body:\n{response}");

    let flow = wait_for(&flows, |f| f.status == Some(403)).await.expect("no 403 flow");
    assert_eq!(flow.state, FlowState::Completed);

    handle.stop();
    let _ = std::fs::remove_dir_all(&ca_dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn map_local_serves_file_with_guessed_type() {
    let stub_dir = std::env::temp_dir().join("novaproxy-tc-maplocal");
    let _ = std::fs::create_dir_all(&stub_dir);
    let stub = stub_dir.join("stub.json");
    std::fs::write(&stub, r#"{"stub":true}"#).unwrap();

    let mut r = rule(RuleKind::MapLocal, "*needs-stub*");
    r.target = Some(stub.to_string_lossy().into_owned());
    let (handle, flows, ca_dir) = start_engine(39_102, nova_core::DEFAULT_BODY_CAP, vec![r], "maplocal");
    tokio::time::sleep(Duration::from_millis(300)).await;

    let response = proxy_get(39_102, "http://api.example/needs-stub", "api.example").await;
    assert!(response.contains("200"), "expected 200, got:\n{response}");
    assert!(response.contains(r#"{"stub":true}"#), "file body not served:\n{response}");
    assert!(
        response.to_ascii_lowercase().contains("application/json"),
        "content-type not guessed from extension:\n{response}"
    );

    let flow = wait_for(&flows, |f| f.status == Some(200)).await.expect("no 200 flow");
    assert_eq!(flow.content_type.as_deref(), Some("application/json"));

    handle.stop();
    let _ = std::fs::remove_dir_all(&ca_dir);
    let _ = std::fs::remove_dir_all(&stub_dir);
}

/// Upstream that serves a fixed body with the given headers, once per connection.
async fn spawn_upstream(body: Vec<u8>, extra_headers: &str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let headers = extra_headers.to_string();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { break };
            let body = body.clone();
            let headers = headers.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n",
                    body.len(),
                    headers
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(&body).await;
                let _ = sock.flush().await;
            });
        }
    });
    port
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_body_is_truncated_but_fully_forwarded() {
    let body = vec![b'A'; 100];
    let up = spawn_upstream(body.clone(), "Content-Type: text/plain\r\n").await;
    // Cap capture at 16 bytes; the client must still receive all 100.
    let (handle, flows, ca_dir) = start_engine(39_103, 16, Vec::new(), "trunc");
    tokio::time::sleep(Duration::from_millis(300)).await;

    let response = proxy_get(39_103, &format!("http://127.0.0.1:{up}/big"), &format!("127.0.0.1:{up}")).await;
    assert_eq!(response.matches('A').count(), 100, "client did not get the full body");

    let flow = wait_for(&flows, |f| f.state == FlowState::Completed).await.expect("no completed flow");
    let rb = flow.response_body.expect("no response body");
    assert_eq!(rb.size, 100, "true wire size should be recorded in full");
    assert!(rb.truncated, "capture should be marked truncated");
    assert_eq!(rb.text.as_deref().map(str::len), Some(16), "only cap bytes retained");

    handle.stop();
    let _ = std::fs::remove_dir_all(&ca_dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gzip_response_is_decoded_in_preview() {
    let plain = "hello gzipped world";
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(plain.as_bytes()).unwrap();
    let gz = enc.finish().unwrap();

    let up = spawn_upstream(gz.clone(), "Content-Type: text/plain\r\nContent-Encoding: gzip\r\n").await;
    let (handle, flows, ca_dir) = start_engine(39_104, nova_core::DEFAULT_BODY_CAP, Vec::new(), "gzip");
    tokio::time::sleep(Duration::from_millis(300)).await;

    let _ = proxy_get(39_104, &format!("http://127.0.0.1:{up}/gz"), &format!("127.0.0.1:{up}")).await;

    let flow = wait_for(&flows, |f| f.state == FlowState::Completed).await.expect("no completed flow");
    let rb = flow.response_body.expect("no response body");
    assert_eq!(rb.text.as_deref(), Some(plain), "gzip body not decoded in preview");
    assert_eq!(rb.decoded_from.as_deref(), Some("gzip"));

    handle.stop();
    let _ = std::fs::remove_dir_all(&ca_dir);
}
