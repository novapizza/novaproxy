//! End-to-end checks for the two things that are impossible to verify from unit
//! tests alone: that the **timing** numbers come from a real connection, and that
//! a body larger than the inline preview really lands in the on-disk body store
//! (and disappears again when the flow is evicted).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use nova_core::bodystore::BodyStore;
use nova_core::flow::Side;
use nova_core::{ca::CaMaterial, EngineConfig, EngineHooks, FlowSink};
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

/// Upstream that answers every request with `body`, chunk-streamed so the
/// response takes more than one frame (the shape a download timing has to cope
/// with).
async fn spawn_upstream(body: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 8192];
                let _ = sock.read(&mut buf).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                for chunk in body.as_bytes().chunks(16 * 1024) {
                    let _ = sock.write_all(chunk).await;
                    let _ = sock.flush().await;
                }
            });
        }
    });
    port
}

/// Drive one plaintext request through the proxy and return the client's view.
async fn through_proxy(proxy_port: u16, url: &str, host: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
    let req = format!(
        "GET {url} HTTP/1.1\r\nHost: {host}\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    String::from_utf8_lossy(&response).into_owned()
}

/// Poll the sink until a flow satisfying `pred` shows up.
async fn wait_for(flows: &Arc<Mutex<Vec<Flow>>>, pred: impl Fn(&Flow) -> bool) -> Option<Flow> {
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let found = flows.lock().unwrap().iter().rev().find(|f| pred(f)).cloned();
        if found.is_some() {
            return found;
        }
    }
    None
}

/// 720 KB of text: comfortably past the 64 KB inline cap used below.
fn big_body() -> String {
    "0123456789abcdef".repeat(45_000)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn timings_are_measured_and_large_bodies_spill_to_disk() {
    let body: &'static str = Box::leak(big_body().into_boxed_str());
    let upstream_port = spawn_upstream(body).await;

    let tmp = std::env::temp_dir().join(format!("novaproxy-spill-e2e-{upstream_port}"));
    let _ = std::fs::remove_dir_all(&tmp);
    let ca = CaMaterial::load_or_create(&tmp.join("ca")).unwrap();

    let sink = Arc::new(VecSink(Arc::new(Mutex::new(Vec::new()))));
    let flows = sink.0.clone();

    // 64 KB inline preview, real spill dir, retention window of 2 flows.
    let bodies = Arc::new(BodyStore::new(
        tmp.join("bodies"),
        64 * 1024,
        nova_core::bodystore::DEFAULT_PER_BODY_CAP,
        nova_core::bodystore::DEFAULT_DISK_BUDGET,
    ));
    let mut hooks = EngineHooks::in_memory(Arc::new(nova_core::breakpoint::Breakpoints::new(
        Arc::new(nova_core::breakpoint::NoopBreakpointSink),
    )));
    hooks.bodies = bodies.clone();
    // Retention window of 2 flows, so eviction is observable below.
    hooks.flows = Arc::new(nova_core::flowstore::FlowStore::new(bodies.clone(), 2));

    let proxy_port = 39_301u16;
    let handle = nova_core::start(
        EngineConfig { addr: ([127, 0, 0, 1], proxy_port).into() },
        &ca,
        sink,
        Arc::new(nova_core::NoopWsSink),
        hooks,
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Addressed by name so the request actually goes through the resolver.
    let url = format!("http://localhost:{upstream_port}/big");
    let host = format!("localhost:{upstream_port}");
    let response = through_proxy(proxy_port, &url, &host).await;
    assert!(response.len() > body.len(), "client got the whole body through");

    let flow = wait_for(&flows, |f| f.state == FlowState::Completed)
        .await
        .expect("no completed flow");

    /* ----------------------------- timings ----------------------------- */

    let t = flow.timings;
    let total = flow.duration_ms.expect("completed flow has a duration");
    assert!(t.ttfb_ms.unwrap() > 0.0, "TTFB must be measured: {t:?}");
    assert!(
        t.ttfb_ms.unwrap() <= total + 1.0,
        "TTFB cannot exceed the total duration: {t:?} vs {total}"
    );
    assert!(t.download_ms.is_some(), "download phase must be measured: {t:?}");
    assert!(
        (t.ttfb_ms.unwrap() + t.download_ms.unwrap() - total).abs() < 2.0,
        "TTFB + download should account for the total: {t:?} vs {total}"
    );
    // This was the very first request to this host, so the engine opened the
    // connection and must be able to attribute its phases.
    assert!(
        !t.connection_reused,
        "the first request to a host cannot have reused a connection: {t:?}"
    );
    assert!(t.connect_ms.is_some(), "TCP connect must be measured: {t:?}");
    assert!(t.dns_ms.is_some(), "DNS resolution must be measured: {t:?}");
    assert!(
        t.dns_ms.unwrap() + t.connect_ms.unwrap() <= t.ttfb_ms.unwrap() + 1.0,
        "connection setup happens within TTFB: {t:?}"
    );
    assert!(
        t.tls_ms.is_none(),
        "plaintext HTTP has no handshake to report: {t:?}"
    );
    // A GET has no request body, so there is no request-streaming phase.
    assert!(t.request_ms.is_none(), "no request body → no request phase: {t:?}");

    /* ------------------------------ spill ------------------------------ */

    let preview = flow.response_body.clone().expect("response body captured");
    assert_eq!(preview.size, body.len() as u64, "true wire size is recorded");
    assert!(preview.truncated, "the preview is capped");
    assert!(preview.spilled, "the full body went to the body store");
    assert!(
        preview.text.as_deref().unwrap().len() <= 64 * 1024,
        "preview must respect the inline cap"
    );
    assert!(
        preview.text.as_deref().unwrap().starts_with("0123456789abcdef"),
        "the preview is the START of the body"
    );

    let (bytes, stored_total, truncated) = bodies
        .read(&flow.id, Side::Response, 8 * 1024 * 1024)
        .expect("spilled body is readable");
    assert!(!truncated);
    assert_eq!(stored_total, body.len() as u64);
    assert_eq!(bytes, body.as_bytes(), "the stored body is byte-exact");

    /* ------------------- nothing measured is invented ------------------- */

    // An IP literal needs no name resolution, and the connector must report that
    // honestly as "no DNS phase" instead of folding the connect time into it.
    let ip_url = format!("http://127.0.0.1:{upstream_port}/ip");
    let ip_host = format!("127.0.0.1:{upstream_port}");
    through_proxy(proxy_port, &ip_url, &ip_host).await;
    let ip_flow = wait_for(&flows, |f| {
        f.state == FlowState::Completed && f.path == "/ip"
    })
    .await
    .expect("no completed flow for the IP-literal request");
    let t = ip_flow.timings;
    assert!(t.dns_ms.is_none(), "no lookup happened, so no DNS phase: {t:?}");
    assert!(t.connect_ms.is_some(), "the TCP connect is still measured: {t:?}");
    assert!(!t.connection_reused, "a different host means a new connection: {t:?}");

    /* ---------------------------- eviction ---------------------------- */

    // Retention is 2 flows: two more requests push this one out, and its spill
    // file must go with it or disk grows for the whole session.
    for _ in 0..2 {
        through_proxy(proxy_port, &url, &host).await;
    }
    let mut gone = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if bodies.read(&flow.id, Side::Response, 1024).is_err() {
            gone = true;
            break;
        }
    }
    assert!(gone, "an evicted flow's spilled body must be deleted");

    handle.stop();
    let _ = std::fs::remove_dir_all(&tmp);
}
