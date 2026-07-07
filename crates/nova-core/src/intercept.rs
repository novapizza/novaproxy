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
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt;
use http_body_util::BodyStream;
use hudsucker::hyper::header::{HeaderName, HeaderValue};
use hudsucker::hyper::http::request::Parts;
use hudsucker::hyper::{Request, Response};
use hudsucker::{Body, HttpContext, HttpHandler, RequestOrResponse, WebSocketHandler};
use nova_proto::{FlowState, Interception};

use crate::breakpoint::Resume;
use crate::flow::{
    build_preview, collect_headers, header_value, new_flow, now_ms, Shared, Side,
};
use crate::rules::{apply_request, guess_media_type, Outcome};
use crate::scripting::{self, ScriptFlow};

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

/// Pass-through WebSocket handler (messages forwarded untouched). WS message
/// inspection is a Phase 3 item; this keeps sockets working today.
#[derive(Clone)]
pub struct NovaWsHandler;

impl WebSocketHandler for NovaWsHandler {}

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
        let cap = shared.body_cap;
        let mut buf: Vec<u8> = Vec::new();
        let mut total: u64 = 0;
        let mut truncated = false;

        while let Some(chunk) = rx.recv().await {
            total += chunk.len() as u64;
            if buf.len() < cap {
                let room = cap - buf.len();
                if chunk.len() <= room {
                    buf.extend_from_slice(&chunk);
                } else {
                    buf.extend_from_slice(&chunk[..room]);
                    truncated = true;
                }
            } else {
                truncated = true;
            }
        }

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
