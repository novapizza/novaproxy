//! End-to-end test of the SSL-proxying scope: a host on the exclude list must
//! be tunneled through a CONNECT WITHOUT decryption (hudsucker `should_intercept`
//! returns false), the bytes splice straight through, and a lightweight
//! `tunneled` flow is recorded for visibility.

use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use nova_core::{ca::CaMaterial, EngineConfig, FlowSink};
use nova_proto::{Flow, TlsScope};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone)]
struct VecSink(Arc<Mutex<Vec<Flow>>>);
impl FlowSink for VecSink {
    fn emit(&self, flow: Flow) {
        self.0.lock().unwrap().push(flow);
    }
}

/// A raw TCP echo server: echoes whatever it receives back to the sender.
async fn spawn_echo() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                loop {
                    match sock.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if sock.write_all(&buf[..n]).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        }
    });
    port
}

/// Read an HTTP response head (up to and including the blank line).
async fn read_http_head(sock: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match sock.read(&mut byte).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                buf.push(byte[0]);
                if buf.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn excluded_host_is_tunneled_without_decryption() {
    let echo_port = spawn_echo().await;

    let ca_dir = std::env::temp_dir().join(format!("novaproxy-test-tls-{echo_port}"));
    let _ = std::fs::remove_dir_all(&ca_dir);
    let ca = CaMaterial::load_or_create(&ca_dir).unwrap();

    let sink = Arc::new(VecSink(Arc::new(Mutex::new(Vec::new()))));
    let flows = sink.0.clone();

    // Exclude 127.0.0.1 from decryption: it must be tunneled raw.
    let tls_scope = Arc::new(RwLock::new(TlsScope {
        intercept_all: true,
        include: Vec::new(),
        exclude: vec!["127.0.0.1".to_string()],
    }));

    let proxy_port = 39_260u16;
    let handle = nova_core::start(
        EngineConfig {
            addr: ([127, 0, 0, 1], proxy_port).into(),
            body_cap: nova_core::DEFAULT_BODY_CAP,
        },
        &ca,
        sink,
        Arc::new(nova_core::NoopWsSink),
        nova_core::EngineHooks {
            rules: Arc::new(RwLock::new(Vec::new())),
            breakpoints: Arc::new(nova_core::breakpoint::Breakpoints::new(Arc::new(
                nova_core::breakpoint::NoopBreakpointSink,
            ))),
            scripts: nova_core::scripting::ScriptEngine::new(),
            net: Arc::new(RwLock::new(Default::default())),
            tls_scope,
        },
    )
    .unwrap();

    tokio::time::sleep(Duration::from_millis(150)).await;

    // CONNECT to the echo server through the proxy.
    let mut sock = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
    let connect = format!(
        "CONNECT 127.0.0.1:{echo_port} HTTP/1.1\r\nHost: 127.0.0.1:{echo_port}\r\n\r\n"
    );
    sock.write_all(connect.as_bytes()).await.unwrap();
    sock.flush().await.unwrap();

    let head = read_http_head(&mut sock).await;
    assert!(head.contains("200"), "CONNECT should be accepted, got:\n{head}");

    // Send bytes through the tunnel; the echo server (spliced raw) mirrors them.
    sock.write_all(b"ping").await.unwrap();
    sock.flush().await.unwrap();
    let mut reply = [0u8; 4];
    sock.read_exact(&mut reply).await.unwrap();
    assert_eq!(&reply, b"ping", "bytes must splice through the raw tunnel untouched");

    tokio::time::sleep(Duration::from_millis(200)).await;

    // A single tunneled flow is recorded for the excluded host.
    let flows = flows.lock().unwrap();
    let tunneled: Vec<_> = flows.iter().filter(|f| f.tunneled).collect();
    assert_eq!(tunneled.len(), 1, "one tunneled flow recorded; got {flows:?}");
    assert_eq!(tunneled[0].host, "127.0.0.1");
    assert_eq!(tunneled[0].method, "CONNECT");
    // Crucially, no decrypted request/response bodies were captured.
    assert!(tunneled[0].request_body.is_none());
    assert!(tunneled[0].response_body.is_none());

    handle.stop();
}
