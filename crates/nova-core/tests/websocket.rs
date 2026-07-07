//! End-to-end test of WebSocket message inspection over a real socket: a local
//! WS echo server, the NovaProxy engine acting as a forward proxy in front of
//! it, and a client that performs the proxy upgrade handshake by hand and then
//! frames the socket with tungstenite. Asserts the engine forwards frames in
//! both directions AND captures them against a single WebSocket flow.

use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use hudsucker::futures::{SinkExt, StreamExt};
use hudsucker::tokio_tungstenite::tungstenite::protocol::Role;
use hudsucker::tokio_tungstenite::tungstenite::Message;
use hudsucker::tokio_tungstenite::{accept_async, WebSocketStream};
use nova_core::{ca::CaMaterial, EngineConfig, FlowSink, WsSink};
use nova_proto::{Flow, WsDirection, WsMessage, WsOpcode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone)]
struct VecSink(Arc<Mutex<Vec<Flow>>>);
impl FlowSink for VecSink {
    fn emit(&self, flow: Flow) {
        self.0.lock().unwrap().push(flow);
    }
}

#[derive(Clone)]
struct VecWsSink(Arc<Mutex<Vec<WsMessage>>>);
impl WsSink for VecWsSink {
    fn emit(&self, msg: WsMessage) {
        self.0.lock().unwrap().push(msg);
    }
}

/// A WebSocket echo server: accepts one connection and echoes each text frame
/// back until the client closes.
async fn spawn_ws_echo() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let mut ws = accept_async(stream).await.unwrap();
            while let Some(Ok(msg)) = ws.next().await {
                match msg {
                    Message::Text(t) => {
                        let _ = ws.send(Message::text(format!("echo:{t}"))).await;
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    });
    port
}

/// Read an HTTP response head (up to and including the blank line) from `sock`,
/// one byte at a time so no bytes of the following WS stream are consumed.
async fn read_http_head(sock: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = sock.read(&mut byte).await.unwrap();
        if n == 0 {
            break;
        }
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn captures_websocket_frames_end_to_end() {
    let upstream_port = spawn_ws_echo().await;

    let ca_dir = std::env::temp_dir().join(format!("novaproxy-test-ws-{upstream_port}"));
    let _ = std::fs::remove_dir_all(&ca_dir);
    let ca = CaMaterial::load_or_create(&ca_dir).unwrap();

    let flow_sink = Arc::new(VecSink(Arc::new(Mutex::new(Vec::new()))));
    let ws_sink = Arc::new(VecWsSink(Arc::new(Mutex::new(Vec::new()))));
    let frames = ws_sink.0.clone();
    let flows = flow_sink.0.clone();

    let proxy_port = 39_240u16;
    let handle = nova_core::start(
        EngineConfig {
            addr: ([127, 0, 0, 1], proxy_port).into(),
            body_cap: nova_core::DEFAULT_BODY_CAP,
        },
        &ca,
        flow_sink,
        ws_sink,
        nova_core::EngineHooks {
            rules: Arc::new(RwLock::new(Vec::new())),
            breakpoints: Arc::new(nova_core::breakpoint::Breakpoints::new(Arc::new(
                nova_core::breakpoint::NoopBreakpointSink,
            ))),
            scripts: nova_core::scripting::ScriptEngine::new(),
            net: Arc::new(RwLock::new(Default::default())),
            tls_scope: Arc::new(RwLock::new(Default::default())),
        },
    )
    .unwrap();

    // Give the proxy a moment to bind.
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Connect to the proxy and send an absolute-form (http://) upgrade so the
    // forward proxy dials the plaintext ws:// upstream.
    let mut sock = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
    let handshake = format!(
        "GET http://127.0.0.1:{upstream_port}/ HTTP/1.1\r\n\
         Host: 127.0.0.1:{upstream_port}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n"
    );
    sock.write_all(handshake.as_bytes()).await.unwrap();
    sock.flush().await.unwrap();

    let head = read_http_head(&mut sock).await;
    assert!(head.starts_with("HTTP/1.1 101"), "expected switching protocols, got:\n{head}");

    // Frame the upgraded socket as a WS client and exchange a message.
    let mut ws = WebSocketStream::from_raw_socket(sock, Role::Client, None).await;
    ws.send(Message::text("hello")).await.unwrap();
    let reply = ws.next().await.unwrap().unwrap();
    assert_eq!(reply, Message::text("echo:hello"), "upstream echo must flow through untouched");
    ws.send(Message::Close(None)).await.unwrap();

    // Let the capture side settle.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let frames = frames.lock().unwrap();
    let sent: Vec<_> = frames.iter().filter(|m| m.direction == WsDirection::Sent).collect();
    let recv: Vec<_> = frames.iter().filter(|m| m.direction == WsDirection::Received).collect();

    let sent_hello = sent
        .iter()
        .find(|m| m.opcode == WsOpcode::Text && m.text.as_deref() == Some("hello"));
    assert!(sent_hello.is_some(), "the sent text frame must be captured; got {frames:?}");

    let recv_echo = recv
        .iter()
        .find(|m| m.opcode == WsOpcode::Text && m.text.as_deref() == Some("echo:hello"));
    assert!(recv_echo.is_some(), "the received echo frame must be captured; got {frames:?}");

    // Every captured frame is bound to the same upgrade flow, which is flagged
    // as a WebSocket.
    let flow_id = &sent_hello.unwrap().flow_id;
    assert!(frames.iter().all(|m| &m.flow_id == flow_id), "all frames share one flow id");
    let flows = flows.lock().unwrap();
    let ws_flow = flows.iter().find(|f| &f.id == flow_id).expect("upgrade flow recorded");
    assert!(ws_flow.is_websocket, "the upgrade flow is marked is_websocket");

    handle.stop();
}
