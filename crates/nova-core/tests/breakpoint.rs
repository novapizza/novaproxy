//! End-to-end breakpoint test: arm a breakpoint, fire a request through the
//! proxy (which blocks), observe the paused interception, then resume with an
//! edited header and confirm the request completes and the edit took effect.

use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use nova_core::breakpoint::{BreakpointSink, Breakpoints, Resume};
use nova_core::{ca::CaMaterial, EngineConfig, FlowSink};
use nova_proto::{Flow, FlowState, Header, Interception};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone)]
struct VecSink(Arc<Mutex<Vec<Flow>>>);
impl FlowSink for VecSink {
    fn emit(&self, flow: Flow) {
        self.0.lock().unwrap().push(flow);
    }
}

struct CaptureSink(Arc<Mutex<Option<Interception>>>);
impl BreakpointSink for CaptureSink {
    fn paused(&self, interception: Interception) {
        *self.0.lock().unwrap() = Some(interception);
    }
}

/// Upstream that echoes the received request headers back in the body.
async fn spawn_echo() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let received = String::from_utf8_lossy(&buf[..n]).into_owned();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    received.len(),
                    received
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    port
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn breakpoint_pause_edit_resume() {
    let upstream = spawn_echo().await;

    let ca_dir = std::env::temp_dir().join(format!("novaproxy-bp-ca-{upstream}"));
    let _ = std::fs::remove_dir_all(&ca_dir);
    let ca = CaMaterial::load_or_create(&ca_dir).unwrap();

    let sink = Arc::new(VecSink(Arc::new(Mutex::new(Vec::new()))));
    let flows = sink.0.clone();

    let paused_slot = Arc::new(Mutex::new(None));
    let breakpoints = Arc::new(Breakpoints::new(Arc::new(CaptureSink(paused_slot.clone()))));
    breakpoints.arm("*".to_string());

    let proxy_port = 39_071u16;
    let _handle = nova_core::start(
        EngineConfig {
            addr: ([127, 0, 0, 1], proxy_port).into(),
            body_cap: nova_core::DEFAULT_BODY_CAP,
        },
        &ca,
        sink,
        Arc::new(nova_core::NoopWsSink),
        nova_core::EngineHooks {
            rules: Arc::new(RwLock::new(Vec::new())),
            breakpoints: breakpoints.clone(),
            scripts: nova_core::scripting::ScriptEngine::new(),
            net: Arc::new(RwLock::new(Default::default())),
            tls_scope: Arc::new(RwLock::new(Default::default())),
        },
    )
    .unwrap();

    tokio::time::sleep(Duration::from_millis(300)).await;

    // Fire the request on a background task — it should block at the breakpoint.
    let target = format!("http://127.0.0.1:{upstream}/hi");
    let client = tokio::spawn(async move {
        let mut stream = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
        let req = format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{upstream}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut resp = Vec::new();
        stream.read_to_end(&mut resp).await.unwrap();
        String::from_utf8_lossy(&resp).into_owned()
    });

    // Wait for the pause to register.
    let mut interception = None;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        interception = paused_slot.lock().unwrap().clone();
        if interception.is_some() {
            break;
        }
    }
    let interception = interception.expect("request never paused at breakpoint");
    assert_eq!(interception.method, "GET");

    // The flow should currently be in the Paused state.
    assert!(
        flows
            .lock()
            .unwrap()
            .iter()
            .any(|f| f.id == interception.id && f.state == FlowState::Paused),
        "no paused flow recorded"
    );

    // Resume with an injected header.
    breakpoints.resume(
        &interception.id,
        Resume::Continue(vec![Header {
            name: "x-nova-edited".into(),
            value: "yes".into(),
        }]),
    );

    let response = tokio::time::timeout(Duration::from_secs(5), client)
        .await
        .expect("client did not finish after resume")
        .unwrap();

    // The echo upstream reflects request headers; our injected header must appear.
    assert!(
        response.to_lowercase().contains("x-nova-edited"),
        "edited header was not forwarded; upstream saw:\n{response}"
    );

    let _ = std::fs::remove_dir_all(&ca_dir);
}
