//! SPIKE (issue 0005): can hudsucker emulate the network faults that make an app
//! see HTTP status 0, without patching hudsucker?
//!
//!   cargo run -p nova-core --example fault_spike
//!
//! Prints the proxy port, upstream port and the CA path, then serves until killed.
//! Faults are picked by request path (`/fault/<mode>`), or by host for CONNECT:
//!
//! | mode           | phase    | what it tries                                          |
//! |----------------|----------|--------------------------------------------------------|
//! | `abort`        | request  | Response whose body errors on first poll (no head?)    |
//! | `hold-close`   | request  | sleep `ms`, then the same abort                        |
//! | `hold-504`     | request  | sleep `ms`, then a real 504                            |
//! | `waf-403`      | request  | 403 with an HTML body                                  |
//! | `pass-cut`     | response | forward upstream, then abort before the head           |
//! | `cut-body`     | response | forward head + first `n` body bytes, then error        |
//! | host `auth407` | CONNECT  | answer the CONNECT itself with 407                     |
//!
//! Nothing here talks to any real backend: upstream is a local server.

use std::time::Duration;

use bytes::Bytes;
use futures::{stream, StreamExt};
use hudsucker::{
    hyper::{Method, Request, Response, StatusCode},
    Body, HttpContext, HttpHandler, Proxy, RequestOrResponse,
};
use nova_core::ca::CaMaterial;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Clone, Debug, PartialEq)]
enum Mode {
    PassCut,
    CutBody(usize),
}

#[derive(Clone, Default)]
struct FaultHandler {
    pending: Option<Mode>,
}

fn query(req: &Request<Body>, key: &str) -> Option<u64> {
    req.uri().query()?.split('&').find_map(|kv| kv.strip_prefix(&format!("{key}=")).and_then(|v| v.parse().ok()))
}

/// A body that fails on its first poll: the candidate for "close without response".
fn erroring_body() -> Body {
    Body::from_stream(stream::once(async {
        Err::<Bytes, std::io::Error>(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "fault: abort"))
    }))
}

fn abort_response() -> Response<Body> {
    Response::builder().status(StatusCode::OK).body(erroring_body()).unwrap()
}

impl HttpHandler for FaultHandler {
    async fn handle_request(&mut self, _ctx: &HttpContext, req: Request<Body>) -> RequestOrResponse {
        if req.method() == Method::CONNECT {
            let host = req.uri().host().unwrap_or_default();
            if host.starts_with("auth407") {
                println!("FAULT connect-407 {host}");
                return Response::builder()
                    .status(StatusCode::PROXY_AUTHENTICATION_REQUIRED)
                    .header("Proxy-Authenticate", "Basic realm=\"nova-fault\"")
                    .header("Content-Length", "0")
                    .body(Body::empty())
                    .unwrap()
                    .into();
            }
            return req.into();
        }

        let path = req.uri().path().to_string();
        let Some(mode) = path.strip_prefix("/fault/") else { return req.into() };
        let ms = query(&req, "ms").unwrap_or(0);
        println!("FAULT {mode} ms={ms} {} {}", req.method(), req.uri());
        match mode {
            "abort" => abort_response().into(),
            "hold-close" => {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                abort_response().into()
            }
            "hold-504" => {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                Response::builder()
                    .status(StatusCode::GATEWAY_TIMEOUT)
                    .header("Content-Type", "text/html")
                    .body(Body::from("<html><body>504 Gateway Time-out</body></html>"))
                    .unwrap()
                    .into()
            }
            "waf-403" => Response::builder()
                .status(StatusCode::FORBIDDEN)
                .header("Content-Type", "text/html")
                .body(Body::from(
                    "<html><head><title>403 Forbidden</title></head><body><h1>403 Forbidden</h1></body></html>",
                ))
                .unwrap()
                .into(),
            "pass-cut" => {
                self.pending = Some(Mode::PassCut);
                req.into()
            }
            "cut-body" => {
                self.pending = Some(Mode::CutBody(query(&req, "n").unwrap_or(10) as usize));
                req.into()
            }
            _ => req.into(),
        }
    }

    async fn handle_response(&mut self, _ctx: &HttpContext, res: Response<Body>) -> Response<Body> {
        match self.pending.take() {
            None => res,
            Some(Mode::PassCut) => {
                // The upstream answered (the write happened); the client never sees it.
                println!("FAULT pass-cut: upstream status {} dropped", res.status());
                abort_response()
            }
            Some(Mode::CutBody(n)) => {
                println!("FAULT cut-body after {n} bytes");
                let (mut parts, body) = res.into_parts();
                // Keep Content-Length so the client knows the body is short.
                parts.headers.remove("transfer-encoding");
                let mut left = n;
                let data = http_body_util::BodyStream::new(body)
                    .filter_map(|f| async move { f.ok().and_then(|f| f.into_data().ok()) });
                let cut = data
                    .map(move |chunk: Bytes| {
                        if left == 0 {
                            return Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "fault: cut"));
                        }
                        let take = left.min(chunk.len());
                        left -= take;
                        Ok(chunk.slice(..take))
                    })
                    .take_while(|r| futures::future::ready(r.as_ref().map(|b| !b.is_empty()).unwrap_or(false)))
                    // hyper buffers the head + first chunk and only flushes when the body
                    // returns Pending; an immediate error drops the buffer (client sees
                    // nothing). Pause so the head and the n bytes reach the client first.
                    .chain(stream::once(async {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "fault: cut"))
                    }));
                Response::from_parts(parts, Body::from_stream(cut))
            }
        }
    }
}

const UPSTREAM_BODY: &str =
    "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopqrstuvwxyz!!";

async fn spawn_upstream() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let line = String::from_utf8_lossy(&buf[..n]).lines().next().unwrap_or_default().to_string();
                println!("UPSTREAM got: {line}");
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    UPSTREAM_BODY.len(),
                    UPSTREAM_BODY
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    port
}

#[tokio::main]
async fn main() {
    let _ = hudsucker::rustls::crypto::aws_lc_rs::default_provider().install_default();
    let dir = std::env::temp_dir().join("novaproxy-fault-spike-ca");
    let ca = CaMaterial::load_or_create(&dir).unwrap();
    let upstream = spawn_upstream().await;
    let port: u16 = std::env::var("SPIKE_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(39_191);
    println!("CA_PATH {}", ca.cert_path.display());
    println!("UPSTREAM_PORT {upstream}");
    println!("PROXY_PORT {port}");

    let proxy = Proxy::builder()
        .with_addr(([127, 0, 0, 1], port).into())
        .with_ca(ca.authority().unwrap())
        .with_rustls_connector(hudsucker::rustls::crypto::aws_lc_rs::default_provider())
        .with_http_handler(FaultHandler::default())
        .build()
        .unwrap();
    proxy.start().await.unwrap();
}
