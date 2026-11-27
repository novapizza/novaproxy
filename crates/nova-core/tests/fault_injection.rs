//! Fault rules end to end over real sockets: the engine in front of a local
//! upstream, a raw HTTP-proxy client, and what that client actually receives.
//! "Dropped" must mean zero bytes before end-of-stream (no status line at all),
//! which is what makes an app report status 0. If a hyper / hudsucker bump
//! changes when the response head is flushed, these tests are what notice.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use nova_core::{ca::CaMaterial, EngineConfig, EngineHooks, FlowSink};
use nova_proto::{Fault, FaultMode, FaultSettings, Flow, FlowState, Rule, RuleKind};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone)]
struct VecSink(Arc<Mutex<Vec<Flow>>>);
impl FlowSink for VecSink {
    fn emit(&self, flow: Flow) {
        self.0.lock().unwrap().push(flow);
    }
}

const BODY: &str =
    "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopqrstuvwxyz!!";

/// Local upstream answering every request with `BODY`; counts requests.
async fn upstream() -> (u16, Arc<AtomicU32>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicU32::new(0));
    let counter = hits.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let counter = counter.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 8192];
                if sock.read(&mut buf).await.unwrap_or(0) == 0 {
                    return;
                }
                counter.fetch_add(1, Ordering::SeqCst);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{BODY}",
                    BODY.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    (port, hits)
}

struct Harness {
    _engine: nova_core::EngineHandle,
    port: u16,
    flows: Arc<Mutex<Vec<Flow>>>,
    hits: Arc<nova_core::rules::RuleHits>,
}

async fn start(port: u16, rules: Vec<Rule>, allow: &[&str]) -> Harness {
    let ca_dir = std::env::temp_dir().join(format!("novaproxy-fault-ca-{port}"));
    let _ = std::fs::remove_dir_all(&ca_dir);
    let ca = CaMaterial::load_or_create(&ca_dir).unwrap();
    let sink = Arc::new(VecSink(Arc::new(Mutex::new(Vec::new()))));
    let flows = sink.0.clone();
    let mut hooks = EngineHooks::in_memory(Arc::new(nova_core::breakpoint::Breakpoints::new(Arc::new(
        nova_core::breakpoint::NoopBreakpointSink,
    ))));
    hooks.rules = Arc::new(RwLock::new(rules));
    hooks.fault = Arc::new(RwLock::new(FaultSettings { allow_hosts: allow.iter().map(|s| s.to_string()).collect() }));
    let hits = hooks.hits.clone();
    let engine = nova_core::start(
        EngineConfig { addr: ([127, 0, 0, 1], port).into() },
        &ca,
        sink,
        Arc::new(nova_core::NoopWsSink),
        hooks,
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    Harness { _engine: engine, port, flows, hits }
}

fn fault_rule(mode: FaultMode) -> Rule {
    Rule {
        id: "f1".into(),
        enabled: true,
        kind: RuleKind::Fault,
        name: "fault".into(),
        pattern: "http://127.0.0.1*/api/*".into(),
        fault: Some(Fault { mode, ..Default::default() }),
        ..Default::default()
    }
}

/// Send `raw` to the proxy; return every byte received before the connection
/// ended (cleanly or by reset) and how long that took.
async fn exchange(port: u16, raw: String) -> (Vec<u8>, Duration) {
    let started = Instant::now();
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(raw.as_bytes()).await.unwrap();
    let mut got = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match tokio::time::timeout(Duration::from_secs(10), s.read(&mut buf)).await {
            Ok(Ok(0)) | Ok(Err(_)) => break,
            Ok(Ok(n)) => got.extend_from_slice(&buf[..n]),
            Err(_) => panic!("no end of stream within 10 s"),
        }
    }
    (got, started.elapsed())
}

async fn request(h: &Harness, method: &str, up: u16, path: &str) -> (String, Duration) {
    let body = if method == "POST" { "{\"a\":1}" } else { "" };
    let raw = format!(
        "{method} http://127.0.0.1:{up}{path} HTTP/1.1\r\nHost: 127.0.0.1:{up}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let (bytes, took) = exchange(h.port, raw).await;
    (String::from_utf8_lossy(&bytes).into_owned(), took)
}

/// The last snapshot of the flow for `path`.
fn flow_for(h: &Harness, path: &str) -> Flow {
    h.flows.lock().unwrap().iter().rev().find(|f| f.path == path).cloned().expect("flow recorded")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_before_sends_nothing_and_never_forwards() {
    let (up, upstream_hits) = upstream().await;
    let h = start(39_301, vec![fault_rule(FaultMode::DropBefore)], &["127.0.0.1"]).await;
    let (resp, _) = request(&h, "POST", up, "/api/save").await;
    assert_eq!(resp, "", "no status line, no bytes");
    assert_eq!(upstream_hits.load(Ordering::SeqCst), 0, "server never saw it");
    let f = flow_for(&h, "/api/save");
    assert_eq!(f.state, FlowState::Error);
    assert_eq!(f.edits.fault, Some(FaultMode::DropBefore));
    assert!(f.edits.rule);
    assert_eq!(h.hits.get("f1"), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hold_close_waits_then_drops() {
    let (up, upstream_hits) = upstream().await;
    let mut r = fault_rule(FaultMode::HoldClose);
    r.fault.as_mut().unwrap().delay_ms = Some(400);
    let h = start(39_302, vec![r], &["127.0.0.1"]).await;
    let (resp, took) = request(&h, "GET", up, "/api/list").await;
    assert_eq!(resp, "");
    assert!(took >= Duration::from_millis(400), "held {took:?}");
    assert_eq!(upstream_hits.load(Ordering::SeqCst), 0);
    let f = flow_for(&h, "/api/list");
    assert_eq!(f.state, FlowState::Error);
    assert!(f.error.unwrap().contains("hold-close"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hold_status_answers_after_the_hold() {
    let (up, _) = upstream().await;
    let mut r = fault_rule(FaultMode::HoldStatus);
    r.fault.as_mut().unwrap().delay_ms = Some(300);
    let h = start(39_303, vec![r], &["127.0.0.1"]).await;
    let (resp, took) = request(&h, "GET", up, "/api/list").await;
    assert!(resp.starts_with("HTTP/1.1 504"), "{resp}");
    assert!(took >= Duration::from_millis(300));
    let f = flow_for(&h, "/api/list");
    assert_eq!(f.status, Some(504));
    assert_eq!(f.edits.fault, Some(FaultMode::HoldStatus));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fake_response_uses_status_body_and_type() {
    let (up, upstream_hits) = upstream().await;
    let mut r = fault_rule(FaultMode::FakeResponse);
    {
        let f = r.fault.as_mut().unwrap();
        f.status = Some(500);
        f.body = Some("{\"errorCode\":\"X\"}".into());
        f.content_type = Some("application/json".into());
    }
    let h = start(39_304, vec![r], &["127.0.0.1"]).await;
    let (resp, _) = request(&h, "GET", up, "/api/item").await;
    assert!(resp.starts_with("HTTP/1.1 500"), "{resp}");
    assert!(resp.to_ascii_lowercase().contains("content-type: application/json"));
    assert!(resp.ends_with("{\"errorCode\":\"X\"}"));
    assert_eq!(upstream_hits.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waf_403_is_an_html_page() {
    let (up, _) = upstream().await;
    let h = start(39_305, vec![fault_rule(FaultMode::Waf403)], &["127.0.0.1"]).await;
    let (resp, _) = request(&h, "POST", up, "/api/save").await;
    assert!(resp.starts_with("HTTP/1.1 403"), "{resp}");
    assert!(resp.to_ascii_lowercase().contains("content-type: text/html"));
    assert!(resp.contains("<title>403 Forbidden</title>"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_auth_407_refuses_the_connect() {
    let mut r = fault_rule(FaultMode::ProxyAuth407);
    r.pattern = "https://auth.fault.test/*".into();
    let h = start(39_306, vec![r], &["*.fault.test"]).await;
    let (bytes, _) =
        exchange(h.port, "CONNECT auth.fault.test:443 HTTP/1.1\r\nHost: auth.fault.test:443\r\n\r\n".into()).await;
    let resp = String::from_utf8_lossy(&bytes);
    assert!(resp.starts_with("HTTP/1.1 407"), "{resp}");
    assert!(resp.to_ascii_lowercase().contains("proxy-authenticate: basic"));
    let f = h.flows.lock().unwrap().iter().rev().find(|f| f.method == "CONNECT").cloned().unwrap();
    assert_eq!(f.status, Some(407));
    assert_eq!(f.edits.fault, Some(FaultMode::ProxyAuth407));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pass_cut_forwards_once_and_drops_the_answer() {
    let (up, upstream_hits) = upstream().await;
    let h = start(39_307, vec![fault_rule(FaultMode::PassCut)], &["127.0.0.1"]).await;
    let (resp, _) = request(&h, "POST", up, "/api/save").await;
    assert_eq!(resp, "", "the client hears nothing");
    assert_eq!(upstream_hits.load(Ordering::SeqCst), 1, "the server did the write, once");
    // The flow finishes on the response side; give the capture task a moment.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let f = flow_for(&h, "/api/save");
    assert_eq!(f.state, FlowState::Error);
    assert_eq!(f.status, Some(200), "what the server answered is kept");
    assert!(f.error.unwrap().contains("pass-cut"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cut_body_sends_the_head_and_n_bytes_then_closes() {
    let (up, _) = upstream().await;
    let mut r = fault_rule(FaultMode::CutBody);
    r.fault.as_mut().unwrap().cut_after_bytes = Some(10);
    let h = start(39_308, vec![r], &["127.0.0.1"]).await;
    let (resp, _) = request(&h, "GET", up, "/api/file").await;
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    assert!(resp.to_ascii_lowercase().contains(&format!("content-length: {}", BODY.len())));
    let body = resp.split("\r\n\r\n").nth(1).unwrap_or_default();
    assert_eq!(body, &BODY[..10], "exactly the first 10 bytes");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let f = flow_for(&h, "/api/file");
    assert_eq!(f.state, FlowState::Error);
    assert!(f.error.unwrap().contains("cut-body"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delay_holds_the_response_and_passes_it_intact() {
    let (up, _) = upstream().await;
    let mut r = fault_rule(FaultMode::Delay);
    r.fault.as_mut().unwrap().delay_ms = Some(300);
    let h = start(39_309, vec![r], &["127.0.0.1"]).await;
    let (resp, took) = request(&h, "GET", up, "/api/list").await;
    assert!(resp.starts_with("HTTP/1.1 200"));
    assert!(resp.ends_with(BODY));
    assert!(took >= Duration::from_millis(300), "{took:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_an_allowed_host_faults_do_nothing() {
    let (up, upstream_hits) = upstream().await;
    let h = start(39_310, vec![fault_rule(FaultMode::DropBefore)], &[]).await;
    let (resp, _) = request(&h, "GET", up, "/api/list").await;
    assert!(resp.starts_with("HTTP/1.1 200"));
    assert_eq!(upstream_hits.load(Ordering::SeqCst), 1);
    assert_eq!(h.hits.get("f1"), 0);
    assert_eq!(flow_for(&h, "/api/list").edits.fault, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_only_and_method_scope_the_fault() {
    let (up, upstream_hits) = upstream().await;
    let mut r = fault_rule(FaultMode::DropBefore);
    r.times = Some(1);
    r.method = Some("POST".into());
    let h = start(39_311, vec![r], &["127.0.0.1"]).await;
    let (get, _) = request(&h, "GET", up, "/api/save").await;
    assert!(get.starts_with("HTTP/1.1 200"), "GET is not the method");
    let (first, _) = request(&h, "POST", up, "/api/save").await;
    assert_eq!(first, "", "first POST dropped");
    let (second, _) = request(&h, "POST", up, "/api/save").await;
    assert!(second.starts_with("HTTP/1.1 200"), "second POST passes");
    assert_eq!(upstream_hits.load(Ordering::SeqCst), 2);
    assert_eq!(h.hits.get("f1"), 1);
}
