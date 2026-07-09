//! The hudsucker handler. Two things matter here and both are called out in
//! the design doc:
//!
//! * **Bodies are teed, never buffered.** Each frame is forwarded downstream
//!   the instant it arrives while a *copy* is accumulated (up to a cap) for the
//!   inspector. SSE, gRPC streams and large downloads keep flowing.
//! * **We do the request↔response correlation.** hudsucker hands us no flow id,
//!   so `handle_request` assigns one and pushes it onto a per-connection FIFO
//!   that `handle_response` pops. Correct for HTTP/1.1 keep-alive; a known
//!   approximation under heavy HTTP/2 multiplexing (see doc).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt;
use http_body_util::BodyStream;
use hudsucker::hyper::header::{HeaderName, HeaderValue};
use hudsucker::hyper::http::request::Parts;
use hudsucker::hyper::{Request, Response};
use hudsucker::tokio_tungstenite::tungstenite::Message;
use hudsucker::{
    Body, HttpContext, HttpHandler, RequestOrResponse, WebSocketContext, WebSocketHandler,
};
use nova_proto::{FlowState, Interception, WsDirection, WsMessage, WsOpcode};

use crate::breakpoint::Resume;
use crate::flow::{
    build_preview, collect_headers, header_value, new_flow, now_ms, Shared, Side, WsRoute,
};
use crate::rules::{apply_request, guess_media_type, Outcome};
use crate::scripting::{self, ScriptFlow};
use crate::tlsscope::should_intercept_host;

/// Flatten a header map into owned name/value pairs for scripts.
fn header_pairs(map: &hudsucker::hyper::HeaderMap) -> Vec<(String, String)> {
    map.iter()
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect()
}

/// Replace a header map with the script-provided set (supports add + delete).
fn apply_headers(map: &mut hudsucker::hyper::HeaderMap, headers: &[(String, String)]) {
    map.clear();
    for (k, v) in headers {
        if let (Ok(n), Ok(val)) = (
            HeaderName::try_from(k.as_str()),
            HeaderValue::try_from(v.as_str()),
        ) {
            map.insert(n, val);
        }
    }
}

/// Derive `(scheme, host, path, url)` from request parts. origin-form URIs
/// (path only) reach us after a CONNECT, i.e. TLS → default scheme https.
fn describe(parts: &Parts) -> (String, String, String, String) {
    let host = parts
        .uri
        .host()
        .map(|h| h.to_string())
        .or_else(|| header_value(&parts.headers, "host"))
        .unwrap_or_default();
    let scheme = parts
        .uri
        .scheme_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "https".to_string());
    let path = parts
        .uri
        .path_and_query()
        .map(|p| p.to_string())
        .unwrap_or_else(|| "/".to_string());
    let url = if parts.uri.scheme_str().is_some() {
        parts.uri.to_string()
    } else {
        format!("{scheme}://{host}{path}")
    };
    (scheme, host, path, url)
}

#[derive(Clone)]
pub struct NovaHandler {
    shared: Arc<Shared>,
    /// Per-connection queue of flow ids awaiting their response. Not wrapped in
    /// `Arc`: each connection gets its own clone, which is exactly the isolation
    /// FIFO correlation needs.
    pending: VecDeque<String>,
}

impl NovaHandler {
    pub fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            pending: VecDeque::new(),
        }
    }
}

impl HttpHandler for NovaHandler {
    async fn handle_request(
        &mut self,
        ctx: &HttpContext,
        req: Request<Body>,
    ) -> RequestOrResponse {
        // CONNECT establishes the TLS tunnel; hudsucker then re-delivers the
        // decrypted inner requests here. Don't record the tunnel itself — doing
        // so would desync the request↔response FIFO below.
        if req.method() == hudsucker::hyper::Method::CONNECT {
            return RequestOrResponse::Request(req);
        }

        let seq = self.shared.seq.fetch_add(1, Ordering::Relaxed);
        let id = format!("f{seq}");

        let (mut parts, body) = req.into_parts();
        let resent = parts.headers.remove("x-nova-resend").is_some();

        // Match rules against the original destination, then let them mutate the
        // request (header rewrite / remap) before we describe what we forward.
        let (_, _, _, orig_url) = describe(&parts);
        let rule_set = self.shared.rules.read().unwrap().clone();
        let outcome = apply_request(&rule_set, &orig_url, &mut parts);

        let (scheme, host, path, url) = describe(&parts);
        let request_headers = collect_headers(&parts.headers);
        let content_encoding = header_value(&parts.headers, "content-encoding");
        let media_type = header_value(&parts.headers, "content-type");

        let mut flow = new_flow(
            id.clone(),
            seq,
            parts.method.to_string(),
            scheme,
            host,
            path,
            url,
            ctx.client_addr.to_string(),
            format!("{:?}", parts.version),
            request_headers,
        );
        flow.resent = resent;
        if let Some(p) = crate::procinfo::global().resolve(&ctx.client_addr.to_string()) {
            flow.pid = Some(p.pid);
            flow.process = Some(p.name);
        }

        match outcome {
            Outcome::Block => {
                let msg = b"Blocked by NovaProxy rule".to_vec();
                let len = msg.len() as u64;
                flow.status = Some(403);
                flow.state = FlowState::Completed;
                flow.response_size = len;
                flow.content_type = Some("text/plain; charset=utf-8".into());
                flow.response_body =
                    Some(build_preview(msg, len, false, Some("text/plain".into()), None));
                flow.duration_ms = Some(now_ms() - flow.started_at);
                self.shared.insert(flow);
                let resp = Response::builder()
                    .status(403)
                    .header("content-type", "text/plain; charset=utf-8")
                    .body(Body::from("Blocked by NovaProxy rule"))
                    .expect("static response");
                RequestOrResponse::Response(resp)
            }
            Outcome::MapLocal(file) => {
                match std::fs::read(&file) {
                    Ok(bytes) => {
                        let ct = guess_media_type(&file);
                        let len = bytes.len() as u64;
                        flow.status = Some(200);
                        flow.state = FlowState::Completed;
                        flow.response_size = len;
                        flow.content_type = Some(ct.to_string());
                        flow.response_headers =
                            vec![nova_proto::Header { name: "content-type".into(), value: ct.into() }];
                        flow.response_body =
                            Some(build_preview(bytes.clone(), len, false, Some(ct.into()), None));
                        flow.duration_ms = Some(now_ms() - flow.started_at);
                        self.shared.insert(flow);
                        let resp = Response::builder()
                            .status(200)
                            .header("content-type", ct)
                            .body(Body::from(bytes))
                            .expect("file response");
                        RequestOrResponse::Response(resp)
                    }
                    Err(e) => {
                        flow.status = Some(404);
                        flow.state = FlowState::Error;
                        flow.error = Some(format!("Map Local: {e}"));
                        flow.duration_ms = Some(now_ms() - flow.started_at);
                        self.shared.insert(flow);
                        let resp = Response::builder()
                            .status(404)
                            .body(Body::from("NovaProxy Map Local: file not found"))
                            .expect("static response");
                        RequestOrResponse::Response(resp)
                    }
                }
            }
            Outcome::Forward { mapped_from } => {
                flow.mapped_from = mapped_from;

                // WebSocket upgrade: record the flow and register a route so the
                // WS handler can attach frames, then forward untouched. hudsucker
                // never calls `handle_response` for an upgrade, so we must NOT
                // push onto the response FIFO or tee the body (doing so would
                // desync correlation for later requests on the connection).
                if is_ws_upgrade(&parts.headers) {
                    let key = ws_route_key(&flow.host, &flow.path);
                    flow.is_websocket = true;
                    self.shared.insert(flow);
                    self.shared.ws_routes.lock().unwrap().insert(
                        key,
                        Arc::new(WsRoute {
                            flow_id: id.clone(),
                            seq: AtomicU64::new(0),
                        }),
                    );
                    return RequestOrResponse::Request(Request::from_parts(parts, body));
                }

                let bp_url = flow.url.clone();
                let bp_method = flow.method.clone();
                let bp_host = flow.host.clone();
                let bp_path = flow.path.clone();
                let bp_headers = flow.request_headers.clone();
                self.shared.insert(flow);

                if self.shared.breakpoints.should_break(&bp_url) {
                    self.shared.update(&id, |f| f.state = FlowState::Paused);
                    let interception = Interception {
                        id: id.clone(),
                        method: bp_method.clone(),
                        url: bp_url.clone(),
                        request_headers: bp_headers,
                    };
                    match self.shared.breakpoints.wait(interception).await {
                        Resume::Abort => {
                            let started = self
                                .shared
                                .flows
                                .lock()
                                .unwrap()
                                .get(&id)
                                .map(|f| f.started_at)
                                .unwrap_or_else(now_ms);
                            self.shared.update(&id, |f| {
                                f.state = FlowState::Error;
                                f.error = Some("Aborted at breakpoint".into());
                                f.duration_ms = Some(now_ms() - started);
                            });
                            let resp = Response::builder()
                                .status(502)
                                .body(Body::from("NovaProxy: aborted at breakpoint"))
                                .expect("static response");
                            return RequestOrResponse::Response(resp);
                        }
                        Resume::Continue(edits) => {
                            for h in &edits {
                                if let (Ok(n), Ok(v)) = (
                                    HeaderName::try_from(h.name.as_str()),
                                    HeaderValue::try_from(h.value.as_str()),
                                ) {
                                    parts.headers.insert(n, v);
                                }
                            }
                            let new_headers = collect_headers(&parts.headers);
                            self.shared.update(&id, |f| {
                                f.state = FlowState::Started;
                                f.request_headers = new_headers;
                            });
                        }
                    }
                }

                // onRequest script hook: mutate headers or abort.
                if self.shared.scripts.wants_request() {
                    let sf = ScriptFlow {
                        method: bp_method,
                        host: bp_host,
                        path: bp_path,
                        url: bp_url,
                        status: None,
                        headers: header_pairs(&parts.headers),
                    };
                    if let Some(res) = self.shared.scripts.run(scripting::Hook::Request, sf).await {
                        if res.abort {
                            let started = self
                                .shared
                                .flows
                                .lock()
                                .unwrap()
                                .get(&id)
                                .map(|f| f.started_at)
                                .unwrap_or_else(now_ms);
                            self.shared.update(&id, |f| {
                                f.state = FlowState::Error;
                                f.status = Some(403);
                                f.error = Some("Aborted by script".into());
                                f.duration_ms = Some(now_ms() - started);
                            });
                            let resp = Response::builder()
                                .status(403)
                                .body(Body::from("NovaProxy: aborted by script"))
                                .expect("static response");
                            return RequestOrResponse::Response(resp);
                        }
                        apply_headers(&mut parts.headers, &res.headers);
                        let nh = collect_headers(&parts.headers);
                        self.shared.update(&id, |f| f.request_headers = nh);
                    }
                }

                self.pending.push_back(id.clone());
                let teed = tee(
                    body,
                    self.shared.clone(),
                    id,
                    Side::Request,
                    media_type,
                    content_encoding,
                );
                RequestOrResponse::Request(Request::from_parts(parts, teed))
            }
        }
    }

    async fn handle_response(
        &mut self,
        _ctx: &HttpContext,
        res: Response<Body>,
    ) -> Response<Body> {
        let Some(id) = self.pending.pop_front() else {
            return res;
        };

        let (mut parts, body) = res.into_parts();
        let status = parts.status.as_u16();
        let response_headers = collect_headers(&parts.headers);
        let content_type = header_value(&parts.headers, "content-type");
        let content_encoding = header_value(&parts.headers, "content-encoding");

        self.shared.update(&id, |f| {
            f.status = Some(status);
            f.response_headers = response_headers.clone();
            if f.content_type.is_none() {
                f.content_type = content_type.clone();
            }
        });

        // onResponse script hook: mutate response headers (abort is a no-op here).
        if self.shared.scripts.wants_response() {
            let base = self
                .shared
                .flows
                .lock()
                .unwrap()
                .get(&id)
                .map(|f| (f.method.clone(), f.host.clone(), f.path.clone(), f.url.clone()));
            if let Some((method, host, path, url)) = base {
                let sf = ScriptFlow {
                    method,
                    host,
                    path,
                    url,
                    status: Some(status),
                    headers: header_pairs(&parts.headers),
                };
                if let Some(res) = self.shared.scripts.run(scripting::Hook::Response, sf).await {
                    apply_headers(&mut parts.headers, &res.headers);
                    let nh = collect_headers(&parts.headers);
                    self.shared.update(&id, |f| f.response_headers = nh);
                }
            }
        }

        // Simulated latency: delay before the response starts streaming back.
        {
            let net = *self.shared.net.read().unwrap();
            if net.enabled && net.latency_ms > 0 {
                tokio::time::sleep(Duration::from_millis(net.latency_ms as u64)).await;
            }
        }

        let teed = tee(
            body,
            self.shared.clone(),
            id,
            Side::Response,
            content_type,
            content_encoding,
        );
        Response::from_parts(parts, teed)
    }

    /// Decide, per CONNECT, whether to MITM (decrypt) or tunnel raw. hudsucker
    /// only calls this for CONNECT requests; returning `false` makes it splice
    /// the bytes through untouched (no leaf cert presented — the fix for hosts
    /// that pin certs or require mTLS).
    async fn should_intercept(&mut self, ctx: &HttpContext, req: &Request<Body>) -> bool {
        let Some(host) = req.uri().host().map(|h| h.to_string()) else {
            return true;
        };
        let scope = self.shared.tls_scope.read().unwrap().clone();
        if should_intercept_host(&scope, &host) {
            return true;
        }

        // Tunneled: no decryption, so we never see the inner requests. Record a
        // single lightweight flow so the host is at least visible in the list.
        let seq = self.shared.seq.fetch_add(1, Ordering::Relaxed);
        let id = format!("f{seq}");
        let port = req.uri().port_u16().unwrap_or(443);
        let mut flow = new_flow(
            id,
            seq,
            "CONNECT".into(),
            "https".into(),
            host.clone(),
            String::new(),
            format!("{host}:{port}"),
            ctx.client_addr.to_string(),
            format!("{:?}", req.version()),
            Vec::new(),
        );
        flow.tunneled = true;
        flow.state = FlowState::Completed;
        if let Some(p) = crate::procinfo::global().resolve(&ctx.client_addr.to_string()) {
            flow.pid = Some(p.pid);
            flow.process = Some(p.name);
        }
        self.shared.insert(flow);
        false
    }

    async fn handle_error(
        &mut self,
        _ctx: &HttpContext,
        err: hudsucker::hyper_util::client::legacy::Error,
    ) -> Response<Body> {
        if let Some(id) = self.pending.pop_front() {
            let started = self
                .shared
                .flows
                .lock()
                .unwrap()
                .get(&id)
                .map(|f| f.started_at)
                .unwrap_or_else(now_ms);
            self.shared.update(&id, |f| {
                f.state = FlowState::Error;
                f.error = Some(err.to_string());
                f.duration_ms = Some(now_ms() - started);
            });
        }
        Response::builder()
            .status(502)
            .body(Body::from("NovaProxy: upstream error"))
            .expect("static response")
    }
}

/// True when request headers carry a WebSocket upgrade handshake
/// (`Connection: Upgrade` + `Upgrade: websocket`).
fn is_ws_upgrade(headers: &hudsucker::hyper::HeaderMap) -> bool {
    let upgrade_ws = headers
        .get(hudsucker::hyper::header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false);
    let connection_upgrade = headers
        .get(hudsucker::hyper::header::CONNECTION)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case("upgrade")))
        .unwrap_or(false);
    upgrade_ws && connection_upgrade
}

/// Correlation key shared between the HTTP handshake (`handle_request`) and the
/// upgraded socket's frames (`handle_message`): host + path (+ query).
///
/// The handshake host may carry a `:port` (from a Host header after a CONNECT
/// tunnel) while the WS context's `uri.host()` never does, so a numeric port
/// suffix is stripped to keep both sides agreeing on the key.
fn ws_route_key(host: &str, path: &str) -> String {
    let host = match host.rsplit_once(':') {
        Some((h, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => host,
    };
    format!("{host}{path}")
}

/// Decode a tungstenite [`Message`] into the fields of a [`WsMessage`],
/// retaining at most `cap` bytes of payload for the preview.
fn classify_message(
    msg: &Message,
    cap: usize,
) -> (WsOpcode, Option<String>, Option<String>, u64, bool) {
    use base64::Engine;
    let b64 = |bytes: &[u8]| {
        let keep = bytes.len().min(cap);
        (
            Some(base64::engine::general_purpose::STANDARD.encode(&bytes[..keep])),
            bytes.len() > cap,
        )
    };
    match msg {
        Message::Text(t) => {
            let s = t.as_str();
            if s.len() <= cap {
                (WsOpcode::Text, Some(s.to_string()), None, s.len() as u64, false)
            } else {
                let mut end = cap;
                while end > 0 && !s.is_char_boundary(end) {
                    end -= 1;
                }
                (WsOpcode::Text, Some(s[..end].to_string()), None, s.len() as u64, true)
            }
        }
        Message::Binary(b) => {
            let (b64s, truncated) = if b.is_empty() { (None, false) } else { b64(b) };
            (WsOpcode::Binary, None, b64s, b.len() as u64, truncated)
        }
        Message::Ping(b) => {
            let (b64s, truncated) = if b.is_empty() { (None, false) } else { b64(b) };
            (WsOpcode::Ping, None, b64s, b.len() as u64, truncated)
        }
        Message::Pong(b) => {
            let (b64s, truncated) = if b.is_empty() { (None, false) } else { b64(b) };
            (WsOpcode::Pong, None, b64s, b.len() as u64, truncated)
        }
        Message::Close(frame) => {
            let reason = frame
                .as_ref()
                .map(|f| format!("{} {}", u16::from(f.code), f.reason))
                .unwrap_or_default();
            let size = reason.len() as u64;
            let text = if reason.is_empty() { None } else { Some(reason) };
            (WsOpcode::Close, text, None, size, false)
        }
        // Raw frames are never surfaced while reading a stream.
        Message::Frame(_) => (WsOpcode::Binary, None, None, 0, false),
    }
}

/// Record one WebSocket frame against the flow its socket was opened on, and
/// stream it to the UI. Derives the correlation key and direction from the
/// hudsucker context, then delegates to [`record_ws_frame`].
fn capture_ws_message(shared: &Arc<Shared>, ctx: &WebSocketContext, msg: &Message) {
    let (direction, uri) = match ctx {
        WebSocketContext::ClientToServer { dst, .. } => (WsDirection::Sent, dst),
        WebSocketContext::ServerToClient { src, .. } => (WsDirection::Received, src),
    };
    let key = ws_route_key(
        uri.host().unwrap_or_default(),
        uri.path_and_query().map(|p| p.as_str()).unwrap_or("/"),
    );
    record_ws_frame(shared, &key, direction, msg);
}

/// Attach a frame to the route registered under `key` and stream it to the UI.
/// Frames whose route is unknown (no matching handshake) are dropped rather
/// than guessed. A close frame finalizes the flow and retires the route.
fn record_ws_frame(shared: &Arc<Shared>, key: &str, direction: WsDirection, msg: &Message) {
    let route = shared.ws_routes.lock().unwrap().get(key).cloned();
    let Some(route) = route else { return };

    let (opcode, text, base64, size, truncated) = classify_message(msg, shared.body_cap);
    let seq = route.seq.fetch_add(1, Ordering::Relaxed);

    shared.ws_sink.emit(WsMessage {
        flow_id: route.flow_id.clone(),
        seq,
        direction,
        opcode,
        size,
        truncated,
        text,
        base64,
        at: now_ms(),
    });

    // A close frame ends the socket: finalize the flow and retire the route.
    if matches!(opcode, WsOpcode::Close) {
        let started = shared
            .flows
            .lock()
            .unwrap()
            .get(&route.flow_id)
            .map(|f| f.started_at)
            .unwrap_or_else(now_ms);
        shared.update(&route.flow_id, |f| {
            if f.state != FlowState::Error {
                f.state = FlowState::Completed;
                f.duration_ms = Some(now_ms() - started);
            }
        });
        shared.ws_routes.lock().unwrap().remove(key);
    }
}

/// WebSocket handler that captures each frame (teed like bodies) while
/// forwarding it untouched.
#[derive(Clone)]
pub struct NovaWsHandler {
    shared: Arc<Shared>,
}

impl NovaWsHandler {
    pub fn new(shared: Arc<Shared>) -> Self {
        Self { shared }
    }
}

impl WebSocketHandler for NovaWsHandler {
    async fn handle_message(
        &mut self,
        ctx: &WebSocketContext,
        msg: Message,
    ) -> Option<Message> {
        capture_ws_message(&self.shared, ctx, &msg);
        Some(msg)
    }
}

/// Wrap `body` so every data frame is forwarded immediately while a capped copy
/// is accumulated off to the side and, on completion, folded into the flow.
fn tee(
    body: Body,
    shared: Arc<Shared>,
    id: String,
    side: Side,
    media_type: Option<String>,
    content_encoding: Option<String>,
) -> Body {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Bytes>();
    let throttle_shared = shared.clone();

    // Consumer: accumulate up to the cap, then finalize when the sender drops.
    tokio::spawn(async move {
        let mut acc = BodyAccum::new(shared.body_cap);
        while let Some(chunk) = rx.recv().await {
            acc.push(&chunk);
        }
        let (buf, total, truncated) = acc.finish();

        let preview = build_preview(buf, total, truncated, media_type, content_encoding);
        let started = shared
            .flows
            .lock()
            .unwrap()
            .get(&id)
            .map(|f| f.started_at)
            .unwrap_or_else(now_ms);

        shared.update(&id, |f| match side {
            Side::Request => {
                f.request_size = total;
                f.request_body = Some(preview);
            }
            Side::Response => {
                f.response_size = total;
                f.response_body = Some(preview);
                if f.state != FlowState::Error {
                    f.state = FlowState::Completed;
                    f.duration_ms = Some(now_ms() - started);
                }
            }
        });
    });

    let stream = BodyStream::new(body).filter_map(move |frame_res| {
        let tx = tx.clone();
        let shared = throttle_shared.clone();
        async move {
            match frame_res {
                Ok(frame) => match frame.into_data() {
                    Ok(data) => {
                        let _ = tx.send(data.clone());
                        // Downlink throttle: pace response chunks to down_kbps.
                        if matches!(side, Side::Response) {
                            let net = *shared.net.read().unwrap();
                            if net.enabled && net.down_kbps > 0 {
                                let bytes_per_sec = (net.down_kbps as f64) * 128.0; // kbit→byte
                                let secs = data.len() as f64 / bytes_per_sec;
                                if secs > 0.0 {
                                    tokio::time::sleep(Duration::from_secs_f64(secs)).await;
                                }
                            }
                        }
                        Some(Ok::<Bytes, hudsucker::Error>(data))
                    }
                    // Trailer frames aren't captured in the preview; drop them.
                    Err(_) => None,
                },
                Err(e) => Some(Err(e)),
            }
        }
    });

    Body::from_stream(stream)
}

/// Accumulates a capped copy of a streamed body for the inspector, tracking the
/// true wire size and whether the preview was cut off. Forwarding is unaffected:
/// this only bounds what we *retain*.
struct BodyAccum {
    buf: Vec<u8>,
    total: u64,
    truncated: bool,
    cap: usize,
}

impl BodyAccum {
    fn new(cap: usize) -> Self {
        Self { buf: Vec::new(), total: 0, truncated: false, cap }
    }

    fn push(&mut self, chunk: &[u8]) {
        self.total += chunk.len() as u64;
        if self.buf.len() < self.cap {
            let room = self.cap - self.buf.len();
            if chunk.len() <= room {
                self.buf.extend_from_slice(chunk);
            } else {
                self.buf.extend_from_slice(&chunk[..room]);
                self.truncated = true;
            }
        } else {
            self.truncated = true;
        }
    }

    fn finish(self) -> (Vec<u8>, u64, bool) {
        (self.buf, self.total, self.truncated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hudsucker::hyper::{HeaderMap, Request};

    fn parts(uri: &str, host: Option<&str>) -> Parts {
        let mut b = Request::builder().uri(uri);
        if let Some(h) = host {
            b = b.header("host", h);
        }
        b.body(()).unwrap().into_parts().0
    }

    #[test]
    fn describe_absolute_uri() {
        let (scheme, host, path, url) = describe(&parts("http://api.example.com/v1?x=1", None));
        assert_eq!(scheme, "http");
        assert_eq!(host, "api.example.com");
        assert_eq!(path, "/v1?x=1");
        assert_eq!(url, "http://api.example.com/v1?x=1");
    }

    #[test]
    fn describe_origin_form_defaults_to_https() {
        // Path-only URI + Host header: what we get after a CONNECT tunnel.
        let (scheme, host, path, url) = describe(&parts("/path?y=2", Some("example.com")));
        assert_eq!(scheme, "https");
        assert_eq!(host, "example.com");
        assert_eq!(path, "/path?y=2");
        assert_eq!(url, "https://example.com/path?y=2");
    }

    #[test]
    fn describe_missing_host_is_empty_with_root_path() {
        let (scheme, host, path, _url) = describe(&parts("/", None));
        assert_eq!(scheme, "https");
        assert_eq!(host, "");
        assert_eq!(path, "/");
    }

    #[test]
    fn apply_headers_replaces_whole_set() {
        let mut map = HeaderMap::new();
        map.insert("x-old", "1".parse().unwrap());
        map.insert("x-keep", "2".parse().unwrap());
        // The new set fully replaces the old one (deletes x-old, x-keep).
        apply_headers(&mut map, &[("x-new".into(), "9".into())]);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("x-new").unwrap(), "9");
        assert!(map.get("x-old").is_none());
    }

    #[test]
    fn apply_headers_skips_invalid_names() {
        let mut map = HeaderMap::new();
        apply_headers(
            &mut map,
            &[("bad name".into(), "x".into()), ("good".into(), "y".into())],
        );
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("good").unwrap(), "y");
    }

    #[test]
    fn header_pairs_flattens_map() {
        let mut map = HeaderMap::new();
        map.insert("content-type", "application/json".parse().unwrap());
        let pairs = header_pairs(&map);
        assert!(pairs.contains(&("content-type".into(), "application/json".into())));
    }

    #[test]
    fn body_accum_under_cap_keeps_everything() {
        let mut a = BodyAccum::new(1024);
        a.push(b"hello ");
        a.push(b"world");
        let (buf, total, truncated) = a.finish();
        assert_eq!(buf, b"hello world");
        assert_eq!(total, 11);
        assert!(!truncated);
    }

    #[test]
    fn body_accum_truncates_at_cap_boundary() {
        let mut a = BodyAccum::new(8);
        a.push(b"1234"); // fills 4
        a.push(b"5678ABCD"); // only 4 more fit; rest dropped
        let (buf, total, truncated) = a.finish();
        assert_eq!(buf, b"12345678"); // exactly cap bytes retained
        assert_eq!(total, 12); // true wire size still counted in full
        assert!(truncated);
    }

    #[test]
    fn body_accum_exact_fit_is_not_truncated() {
        let mut a = BodyAccum::new(5);
        a.push(b"12345");
        let (buf, total, truncated) = a.finish();
        assert_eq!(buf, b"12345");
        assert_eq!(total, 5);
        assert!(!truncated, "a chunk that exactly fills the cap is not a truncation");
    }

    #[test]
    fn body_accum_chunks_after_cap_only_bump_total() {
        let mut a = BodyAccum::new(4);
        a.push(b"1234");
        a.push(b"5"); // buf already full
        let (buf, total, truncated) = a.finish();
        assert_eq!(buf, b"1234");
        assert_eq!(total, 5);
        assert!(truncated);
    }

    #[test]
    fn body_accum_zero_cap_keeps_nothing() {
        let mut a = BodyAccum::new(0);
        a.push(b"anything");
        let (buf, total, truncated) = a.finish();
        assert!(buf.is_empty());
        assert_eq!(total, 8);
        assert!(truncated);
    }

    // ---- WebSocket helpers ----

    #[test]
    fn ws_upgrade_detected_from_headers() {
        let mut m = HeaderMap::new();
        m.insert("upgrade", "websocket".parse().unwrap());
        m.insert("connection", "Upgrade".parse().unwrap());
        assert!(is_ws_upgrade(&m));
    }

    #[test]
    fn ws_upgrade_matches_connection_token_list() {
        // Browsers send "keep-alive, Upgrade"; the token must still match.
        let mut m = HeaderMap::new();
        m.insert("upgrade", "websocket".parse().unwrap());
        m.insert("connection", "keep-alive, Upgrade".parse().unwrap());
        assert!(is_ws_upgrade(&m));
    }

    #[test]
    fn ws_upgrade_rejects_plain_request_and_partial_headers() {
        assert!(!is_ws_upgrade(&HeaderMap::new()));
        let mut only_upgrade = HeaderMap::new();
        only_upgrade.insert("upgrade", "websocket".parse().unwrap());
        assert!(!is_ws_upgrade(&only_upgrade), "Upgrade without Connection is not a handshake");
        let mut wrong_proto = HeaderMap::new();
        wrong_proto.insert("upgrade", "h2c".parse().unwrap());
        wrong_proto.insert("connection", "Upgrade".parse().unwrap());
        assert!(!is_ws_upgrade(&wrong_proto));
    }

    #[test]
    fn ws_route_key_is_host_plus_path() {
        assert_eq!(ws_route_key("echo.example.com", "/ws?token=1"), "echo.example.com/ws?token=1");
    }

    #[test]
    fn ws_route_key_strips_numeric_port() {
        // Host header "host:443" (handshake) must key the same as uri.host()
        // "host" (the WS context), so frames correlate after a CONNECT tunnel.
        assert_eq!(ws_route_key("host:443", "/ws"), ws_route_key("host", "/ws"));
        // A non-numeric suffix after ':' is not a port and must be preserved.
        assert_eq!(ws_route_key("weird:name", "/x"), "weird:name/x");
    }

    #[test]
    fn classify_text_frame() {
        let (op, text, b64, size, trunc) = classify_message(&Message::text("hi there"), 1024);
        assert_eq!(op, WsOpcode::Text);
        assert_eq!(text.as_deref(), Some("hi there"));
        assert!(b64.is_none());
        assert_eq!(size, 8);
        assert!(!trunc);
    }

    #[test]
    fn classify_text_frame_truncates_on_char_boundary() {
        // "héllo": 'é' is two bytes, so a cap of 2 must back off to 1 byte.
        let (_op, text, _b64, size, trunc) = classify_message(&Message::text("héllo"), 2);
        assert_eq!(text.as_deref(), Some("h"), "must not split a multibyte char");
        assert_eq!(size, 6); // true byte length: h + é(2) + l + l + o
        assert!(trunc);
    }

    #[test]
    fn classify_binary_frame_is_base64_and_tracks_truncation() {
        let (op, text, b64, size, trunc) =
            classify_message(&Message::binary(vec![1u8, 2, 3, 4, 5]), 3);
        assert_eq!(op, WsOpcode::Binary);
        assert!(text.is_none());
        assert!(b64.is_some(), "binary payload is previewed as base64");
        assert_eq!(size, 5); // full wire size
        assert!(trunc); // only 3 of 5 bytes retained
    }

    #[test]
    fn classify_close_frame_carries_no_payload_when_empty() {
        let (op, text, b64, size, trunc) = classify_message(&Message::Close(None), 1024);
        assert_eq!(op, WsOpcode::Close);
        assert!(text.is_none());
        assert!(b64.is_none());
        assert_eq!(size, 0);
        assert!(!trunc);
    }

    // ---- WS frame capture: route → sink → finalize ----

    use std::sync::Mutex;

    struct VecWsSink(Mutex<Vec<WsMessage>>);
    impl crate::flow::WsSink for VecWsSink {
        fn emit(&self, msg: WsMessage) {
            self.0.lock().unwrap().push(msg);
        }
    }

    struct NoopFlowSink;
    impl crate::flow::FlowSink for NoopFlowSink {
        fn emit(&self, _flow: nova_proto::Flow) {}
    }

    fn shared_for_ws(ws_sink: Arc<VecWsSink>) -> Arc<Shared> {
        use std::sync::RwLock;
        Arc::new(Shared::new(
            Arc::new(NoopFlowSink),
            ws_sink,
            1024,
            Arc::new(RwLock::new(Vec::new())),
            Arc::new(crate::breakpoint::Breakpoints::new(Arc::new(
                crate::breakpoint::NoopBreakpointSink,
            ))),
            crate::scripting::ScriptEngine::new(),
            Arc::new(RwLock::new(Default::default())),
            Arc::new(RwLock::new(Default::default())),
        ))
    }

    fn register_route(shared: &Arc<Shared>, key: &str, flow_id: &str) {
        shared.ws_routes.lock().unwrap().insert(
            key.to_string(),
            Arc::new(WsRoute {
                flow_id: flow_id.to_string(),
                seq: AtomicU64::new(0),
            }),
        );
    }

    #[test]
    fn record_ws_frame_emits_with_flow_id_and_incrementing_seq() {
        let ws_sink = Arc::new(VecWsSink(Mutex::new(Vec::new())));
        let shared = shared_for_ws(ws_sink.clone());
        register_route(&shared, "host/ws", "f7");

        record_ws_frame(&shared, "host/ws", WsDirection::Sent, &Message::text("first"));
        record_ws_frame(&shared, "host/ws", WsDirection::Received, &Message::text("second"));

        let msgs = ws_sink.0.lock().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].flow_id, "f7");
        assert_eq!(msgs[0].seq, 0);
        assert_eq!(msgs[0].direction, WsDirection::Sent);
        assert_eq!(msgs[0].text.as_deref(), Some("first"));
        assert_eq!(msgs[1].seq, 1, "seq is monotonic across both directions");
        assert_eq!(msgs[1].direction, WsDirection::Received);
    }

    #[test]
    fn record_ws_frame_without_route_is_dropped() {
        let ws_sink = Arc::new(VecWsSink(Mutex::new(Vec::new())));
        let shared = shared_for_ws(ws_sink.clone());
        // No route registered for this key.
        record_ws_frame(&shared, "unknown/ws", WsDirection::Sent, &Message::text("x"));
        assert!(ws_sink.0.lock().unwrap().is_empty());
    }

    #[test]
    fn close_frame_finalizes_flow_and_retires_route() {
        let ws_sink = Arc::new(VecWsSink(Mutex::new(Vec::new())));
        let shared = shared_for_ws(ws_sink.clone());
        register_route(&shared, "host/ws", "f9");
        // The flow must exist for finalization to update its state.
        let mut flow = new_flow(
            "f9".into(), 0, "GET".into(), "https".into(), "host".into(),
            "/ws".into(), "https://host/ws".into(), "127.0.0.1:1".into(),
            "HTTP/1.1".into(), Vec::new(),
        );
        flow.is_websocket = true;
        shared.insert(flow);

        record_ws_frame(&shared, "host/ws", WsDirection::Received, &Message::Close(None));

        let stored = shared.flows.lock().unwrap().get("f9").cloned().unwrap();
        assert_eq!(stored.state, FlowState::Completed);
        assert!(stored.duration_ms.is_some());
        assert!(
            shared.ws_routes.lock().unwrap().get("host/ws").is_none(),
            "route is retired after close"
        );
        // A frame arriving after close finds no route and is dropped.
        record_ws_frame(&shared, "host/ws", WsDirection::Sent, &Message::text("late"));
        assert_eq!(ws_sink.0.lock().unwrap().len(), 1, "only the close frame was recorded");
    }
}
