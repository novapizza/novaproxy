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
use hudsucker::tokio_tungstenite::tungstenite::{Error as WsError, Message};
use hudsucker::{Body, HttpContext, HttpHandler, RequestOrResponse, WebSocketContext, WebSocketHandler};
use nova_proto::{Fault, FaultMode, FlowState, Interception, WsDirection, WsMessage, WsOpcode};

use crate::bodystore::BodyCapture;
use crate::breakpoint::Resume;
use crate::fault;
use crate::flow::{build_preview, collect_headers, header_value, new_flow, now_ms, Shared, Side, WsRoute};
use crate::rules::{apply_request_with, connect_fault, guess_media_type, MatchCtx, Outcome};
use crate::scripting::{self, ScriptFlow};
use crate::tlsscope::should_intercept_host;

/// Flatten a header map into owned name/value pairs for scripts.
fn header_pairs(map: &hudsucker::hyper::HeaderMap) -> Vec<(String, String)> {
    map.iter().map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect()
}

/// Replace a header map with the script-provided set (supports add + delete).
fn apply_headers(map: &mut hudsucker::hyper::HeaderMap, headers: &[(String, String)]) {
    map.clear();
    for (k, v) in headers {
        if let (Ok(n), Ok(val)) = (HeaderName::try_from(k.as_str()), HeaderValue::try_from(v.as_str())) {
            map.insert(n, val);
        }
    }
}

/// Derive `(scheme, host, path, url)` from request parts. origin-form URIs
/// (path only) reach us after a CONNECT, i.e. TLS → default scheme https.
fn describe(parts: &Parts) -> (String, String, String, String) {
    let host =
        parts.uri.host().map(|h| h.to_string()).or_else(|| header_value(&parts.headers, "host")).unwrap_or_default();
    let scheme = parts.uri.scheme_str().map(|s| s.to_string()).unwrap_or_else(|| "https".to_string());
    let path = parts.uri.path_and_query().map(|p| p.to_string()).unwrap_or_else(|| "/".to_string());
    let url = if parts.uri.scheme_str().is_some() { parts.uri.to_string() } else { format!("{scheme}://{host}{path}") };
    (scheme, host, path, url)
}

#[derive(Clone)]
pub struct NovaHandler {
    shared: Arc<Shared>,
    /// Per-connection queue of flow ids awaiting their response. Not wrapped in
    /// `Arc`: each connection gets its own clone, which is exactly the isolation
    /// FIFO correlation needs. Each entry carries the response-phase fault
    /// (PassCut / CutBody / Delay) a rule attached to that request, if any.
    pending: VecDeque<(String, Option<Fault>)>,
}

impl NovaHandler {
    pub fn new(shared: Arc<Shared>) -> Self {
        Self { shared, pending: VecDeque::new() }
    }
}

impl NovaHandler {
    /// A CONNECT: answer it with 407 when a ProxyAuth407 Fault rule matches the
    /// host, else let hudsucker open the tunnel. Only a refused CONNECT becomes
    /// a flow; an accepted one is recorded through its inner requests.
    fn connect(&mut self, ctx: &HttpContext, req: Request<Body>) -> RequestOrResponse {
        let Some(host) = req.uri().host().map(str::to_string) else {
            return RequestOrResponse::Request(req);
        };
        let rule_set = self.shared.rules.read().unwrap().clone();
        let fault_hosts = self.shared.fault.read().unwrap().allow_hosts.clone();
        let Some((rule_id, fault)) = connect_fault(&rule_set, &host, Some(&self.shared.hits), &fault_hosts) else {
            return RequestOrResponse::Request(req);
        };
        tracing::info!(rule = %rule_id, %host, "fault injected: 407 to CONNECT");
        let seq = self.shared.seq.fetch_add(1, Ordering::Relaxed);
        let port = req.uri().port_u16().unwrap_or(443);
        let mut flow = new_flow(
            format!("f{seq}"),
            seq,
            "CONNECT".into(),
            "https".into(),
            host.clone(),
            String::new(),
            format!("{host}:{port}"),
            ctx.client_addr.to_string(),
            format!("{:?}", req.version()),
            collect_headers(req.headers()),
        );
        if let Some(p) = crate::procinfo::global().resolve(&ctx.client_addr.to_string()) {
            flow.pid = Some(p.pid);
            flow.process = Some(p.name);
        }
        flow.edits.rule = true;
        flow.edits.fault = Some(fault.mode);
        RequestOrResponse::Response(self.canned(flow, &fault, false))
    }

    /// Answer a request-phase fault without forwarding. `flow` is not stored
    /// yet; it is stored here, before any hold, so the table shows the
    /// request waiting.
    async fn answer_fault(&mut self, mut flow: nova_proto::Flow, fault: Fault) -> RequestOrResponse {
        let id = flow.id.clone();
        let hold = fault::delay(&fault);
        let holds = matches!(fault.mode, FaultMode::HoldClose | FaultMode::HoldStatus) && !hold.is_zero();
        if holds {
            flow.state = FlowState::Started;
            self.shared.insert(flow.clone());
            let mut guard = HoldGuard { shared: self.shared.clone(), id: id.clone(), armed: true };
            tokio::time::sleep(hold).await;
            guard.armed = false;
        }
        let resp = match fault.mode {
            FaultMode::DropBefore | FaultMode::HoldClose => {
                let why = if fault.mode == FaultMode::DropBefore {
                    "Fault (drop-before): connection closed, request not forwarded".to_string()
                } else {
                    format!("Fault (hold-close): held {} ms, then closed; request not forwarded", hold.as_millis())
                };
                if holds {
                    self.fail_flow(&id, why);
                } else {
                    flow.state = FlowState::Error;
                    flow.error = Some(why);
                    flow.duration_ms = Some(now_ms() - flow.started_at);
                    self.shared.insert(flow);
                }
                Response::builder().status(200).body(fault::abort_body()).expect("static response")
            }
            _ => {
                if holds {
                    // Re-read: the stored copy is the one the table shows.
                    flow = self.shared.flow(&id).unwrap_or(flow);
                }
                self.canned(flow, &fault, holds)
            }
        };
        RequestOrResponse::Response(resp)
    }

    /// Store `flow` as answered with the fault's canned response, and build it.
    /// `stored`: the flow is already in the store (it was held), so update it.
    fn canned(&self, mut flow: nova_proto::Flow, fault: &Fault, stored: bool) -> Response<Body> {
        let (status, content_type, body) =
            fault::canned_answer(fault).unwrap_or((502, "text/plain".into(), String::new()));
        let len = body.len() as u64;
        flow.status = Some(status);
        flow.state = FlowState::Completed;
        flow.response_size = len;
        flow.content_type = Some(content_type.clone());
        let mut headers = vec![nova_proto::Header { name: "content-type".into(), value: content_type.clone() }];
        if fault.mode == FaultMode::ProxyAuth407 {
            headers.push(nova_proto::Header {
                name: "proxy-authenticate".into(),
                value: "Basic realm=\"NovaProxy fault\"".into(),
            });
        }
        flow.response_headers = headers.clone();
        if fault.mode == FaultMode::ProxyAuth407 {
            // A refused CONNECT opens no tunnel; close rather than leave the
            // client a keep-alive it will not reuse.
            headers.push(nova_proto::Header { name: "connection".into(), value: "close".into() });
        }
        flow.response_body = Some(build_preview(body.clone().into_bytes(), len, false, Some(content_type), None));
        flow.duration_ms = Some(now_ms() - flow.started_at);
        if stored {
            let id = flow.id.clone();
            self.shared.update(&id, move |f| *f = flow);
        } else {
            self.shared.insert(flow);
        }
        let mut builder = Response::builder().status(status);
        for h in headers {
            builder = builder.header(h.name, h.value);
        }
        builder.body(Body::from(body)).unwrap_or_else(|_| Response::new(Body::empty()))
    }

    /// Mark a stored flow failed by a fault.
    fn fail_flow(&self, id: &str, why: String) {
        let started = self.shared.flow(id).map(|f| f.started_at).unwrap_or_else(now_ms);
        self.shared.update(id, |f| {
            f.state = FlowState::Error;
            f.error = Some(why.clone());
            f.duration_ms = Some(now_ms() - started);
        });
    }
}

/// While a fault holds a request, the client may give up first (its own
/// timeout) and hyper drops the handler mid-sleep. Then the flow still says
/// so, rather than staying "started" forever.
struct HoldGuard {
    shared: Arc<Shared>,
    id: String,
    armed: bool,
}

impl Drop for HoldGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let started = self.shared.flow(&self.id).map(|f| f.started_at).unwrap_or_else(now_ms);
        self.shared.update(&self.id, |f| {
            if f.state == FlowState::Started {
                f.state = FlowState::Error;
                f.error = Some("Fault: the client gave up while the request was held".into());
                f.duration_ms = Some(now_ms() - started);
            }
        });
    }
}

impl HttpHandler for NovaHandler {
    async fn handle_request(&mut self, ctx: &HttpContext, req: Request<Body>) -> RequestOrResponse {
        // CONNECT establishes the TLS tunnel; hudsucker then re-delivers the
        // decrypted inner requests here. Don't record the tunnel itself — doing
        // so would desync the request↔response FIFO below.
        if req.method() == hudsucker::hyper::Method::CONNECT {
            return self.connect(ctx, req);
        }

        let seq = self.shared.seq.fetch_add(1, Ordering::Relaxed);
        let id = format!("f{seq}");

        let (mut parts, body) = req.into_parts();
        let resent = parts.headers.remove("x-nova-resend").is_some();
        // Requests NovaProxy issued on its own behalf (an MCP replay) say so, and
        // the marker never reaches the origin.
        let internal_marker = parts.headers.remove("x-nova-internal").is_some();
        let target_port = parts.uri.port_u16();

        // Match rules against the original destination, then let them mutate the
        // request (header rewrite / remap) before we describe what we forward.
        let (_, _, _, orig_url) = describe(&parts);
        let rule_set = self.shared.rules.read().unwrap().clone();
        let fault_hosts = self.shared.fault.read().unwrap().allow_hosts.clone();
        let method = parts.method.to_string();
        let match_ctx =
            MatchCtx { url: &orig_url, method: &method, hits: Some(&self.shared.hits), fault_hosts: &fault_hosts };
        let outcome = apply_request_with(&rule_set, &match_ctx, &mut parts);

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
        flow.internal = internal_marker
            || crate::mcp::is_internal_endpoint(
                &flow.host,
                target_port,
                self.shared.internal_port.load(Ordering::Relaxed),
            );
        if let Some(p) = crate::procinfo::global().resolve(&ctx.client_addr.to_string()) {
            flow.pid = Some(p.pid);
            flow.process = Some(p.name);
        }

        // A fault that acts on the response forwards like any request and is
        // carried to `handle_response`; the others answer here.
        let mut response_fault: Option<Fault> = None;
        let outcome = match outcome {
            Outcome::Fault { rule_id, fault, mapped_from, rewritten } => {
                flow.edits.rule = true;
                flow.edits.fault = Some(fault.mode);
                tracing::info!(rule = %rule_id, mode = fault::label(fault.mode), url = %flow.url, "fault injected");
                if !fault::acts_on_response(fault.mode) {
                    flow.mapped_from = mapped_from;
                    return self.answer_fault(flow, fault).await;
                }
                response_fault = Some(fault);
                Outcome::Forward { mapped_from, rewritten }
            }
            other => other,
        };

        match outcome {
            Outcome::Block => {
                flow.edits.rule = true;
                let msg = b"Blocked by NovaProxy rule".to_vec();
                let len = msg.len() as u64;
                flow.status = Some(403);
                flow.state = FlowState::Completed;
                flow.response_size = len;
                flow.content_type = Some("text/plain; charset=utf-8".into());
                flow.response_body = Some(build_preview(msg, len, false, Some("text/plain".into()), None));
                flow.duration_ms = Some(now_ms() - flow.started_at);
                self.shared.insert(flow);
                let resp = Response::builder()
                    .status(403)
                    .header("content-type", "text/plain; charset=utf-8")
                    .body(Body::from("Blocked by NovaProxy rule"))
                    .expect("static response");
                RequestOrResponse::Response(resp)
            }
            Outcome::Fault { .. } => unreachable!("faults are resolved above"),
            Outcome::MapLocal(file) => {
                flow.edits.rule = true;
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
                        flow.response_body = Some(build_preview(bytes.clone(), len, false, Some(ct.into()), None));
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
            Outcome::Forward { mapped_from, rewritten } => {
                flow.edits.rule = flow.edits.rule || mapped_from.is_some() || rewritten;
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
                    self.shared.ws_routes.lock().unwrap().insert(key, Arc::new(WsRoute::new(id.clone())));
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
                            let started = self.shared.flow(&id).map(|f| f.started_at).unwrap_or_else(now_ms);
                            self.shared.update(&id, |f| {
                                f.state = FlowState::Error;
                                f.error = Some("Aborted at breakpoint".into());
                                f.duration_ms = Some(now_ms() - started);
                                f.edits.breakpoint = true;
                            });
                            let resp = Response::builder()
                                .status(502)
                                .body(Body::from("NovaProxy: aborted at breakpoint"))
                                .expect("static response");
                            return RequestOrResponse::Response(resp);
                        }
                        Resume::Continue(edits) => {
                            for h in &edits {
                                if let (Ok(n), Ok(v)) =
                                    (HeaderName::try_from(h.name.as_str()), HeaderValue::try_from(h.value.as_str()))
                                {
                                    parts.headers.insert(n, v);
                                }
                            }
                            let new_headers = collect_headers(&parts.headers);
                            self.shared.update(&id, |f| {
                                f.state = FlowState::Started;
                                // Resuming without touching anything is not an
                                // edit: the pause itself changed nothing.
                                f.edits.breakpoint = f.request_headers != new_headers;
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
                            let started = self.shared.flow(&id).map(|f| f.started_at).unwrap_or_else(now_ms);
                            self.shared.update(&id, |f| {
                                f.state = FlowState::Error;
                                f.status = Some(403);
                                f.error = Some("Aborted by script".into());
                                f.duration_ms = Some(now_ms() - started);
                                f.edits.script = true;
                            });
                            let resp = Response::builder()
                                .status(403)
                                .body(Body::from("NovaProxy: aborted by script"))
                                .expect("static response");
                            return RequestOrResponse::Response(resp);
                        }
                        apply_headers(&mut parts.headers, &res.headers);
                        let nh = collect_headers(&parts.headers);
                        self.shared.update(&id, |f| {
                            // A hook that hands back the headers it was given
                            // changed nothing, and should leave no marker.
                            f.edits.script = f.edits.script || f.request_headers != nh;
                            f.request_headers = nh;
                        });
                    }
                }

                self.pending.push_back((id.clone(), response_fault));
                let teed = tee(body, self.shared.clone(), id, Side::Request, media_type, content_encoding);
                RequestOrResponse::Request(Request::from_parts(parts, teed))
            }
        }
    }

    async fn handle_response(&mut self, _ctx: &HttpContext, res: Response<Body>) -> Response<Body> {
        let Some((id, response_fault)) = self.pending.pop_front() else {
            return res;
        };

        let (mut parts, body) = res.into_parts();
        let status = parts.status.as_u16();
        let response_headers = collect_headers(&parts.headers);
        // Captured as the server sent it; what the app receives loses its
        // HTTP/3 advertisements (see `EngineHooks::keep_on_tcp`).
        let h3_removed = self.shared.keep_on_tcp.load(Ordering::Relaxed) && strip_h3_alt_svc(&mut parts.headers);
        let content_type = header_value(&parts.headers, "content-type");
        let content_encoding = header_value(&parts.headers, "content-encoding");

        // Time to first byte, stamped *before* any simulated latency below so the
        // measurement reflects the upstream, not our own throttling.
        let at_headers = now_ms();
        // Claim the connection this flow opened, if it opened one. Done here
        // because the connect happens while the request is in flight upstream.
        let (host, started_at) = self.shared.flow(&id).map(|f| (f.host.clone(), f.started_at)).unwrap_or_default();
        let connect = self.shared.connects.claim(&host, started_at);

        // The protocol actually spoken to the server: the app may talk
        // HTTP/1.1 to the proxy while the server answers over h2.
        let upstream_version = format!("{:?}", parts.version);
        self.shared.update(&id, |f| {
            f.status = Some(status);
            f.http_version = upstream_version.clone();
            f.alt_svc_h3_removed = h3_removed;
            f.response_headers = response_headers.clone();
            if f.content_type.is_none() {
                f.content_type = content_type.clone();
            }
            f.timings.ttfb_ms = Some(at_headers - f.started_at);
            match &connect {
                Some(c) => {
                    f.timings.dns_ms = c.dns_ms;
                    f.timings.connect_ms = c.connect_ms;
                    f.timings.tls_ms = c.tls_ms;
                    f.timings.connection_reused = false;
                }
                // No new connection was opened for this request: it went out on a
                // pooled keep-alive/HTTP-2 connection.
                None => f.timings.connection_reused = true,
            }
        });

        // onResponse script hook: mutate response headers (abort is a no-op here).
        if self.shared.scripts.wants_response() {
            let base = self.shared.flow(&id).map(|f| (f.method.clone(), f.host.clone(), f.path.clone(), f.url.clone()));
            if let Some((method, host, path, url)) = base {
                let sf =
                    ScriptFlow { method, host, path, url, status: Some(status), headers: header_pairs(&parts.headers) };
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

        let body = match response_fault {
            None => body,
            Some(f) => match f.mode {
                FaultMode::PassCut => {
                    // The server did the work; the client never hears of it.
                    drop(body);
                    self.fail_flow(
                        &id,
                        format!("Fault (pass-cut): the server answered {status}, the response was dropped"),
                    );
                    return Response::from_parts(parts, fault::abort_body());
                }
                FaultMode::CutBody => {
                    let n = f.cut_after_bytes.unwrap_or(0) as u64;
                    self.fail_flow(&id, format!("Fault (cut-body): connection closed after {n} body bytes"));
                    fault::cut_body(body, n)
                }
                FaultMode::Delay => {
                    tokio::time::sleep(fault::delay(&f)).await;
                    body
                }
                _ => body,
            },
        };

        let teed = tee(body, self.shared.clone(), id, Side::Response, content_type, content_encoding);
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
        let mut reason = crate::upstream::describe(&err);
        // Out of descriptors, the lookup fails first and reads as a DNS error.
        if crate::fdlimit::exhausted() {
            reason = crate::fdlimit::exhausted_reason(&reason);
        }
        let mut host = None;
        if let Some((id, _)) = self.pending.pop_front() {
            let flow = self.shared.flow(&id);
            let started = flow.as_ref().map(|f| f.started_at).unwrap_or_else(now_ms);
            host = flow.map(|f| f.host);
            self.shared.update(&id, |f| {
                f.state = FlowState::Error;
                f.error = Some(reason.clone());
                f.duration_ms = Some(now_ms() - started);
            });
        }
        let host = host.unwrap_or_default();
        if upstream_error_is_news(&host) {
            tracing::warn!(host = %host, error = %reason, "upstream request failed");
        }
        // The page the client sees is often the only place the user looks, so
        // it says why rather than just that.
        Response::builder()
            .status(502)
            .header("content-type", "text/plain; charset=utf-8")
            .body(Body::from(format!("NovaProxy: upstream error: {reason}")))
            .expect("static response")
    }
}

/// Whether an upstream failure for `host` should be logged: once per host a
/// minute. A site that is down fails every request a page makes, and the log
/// needs the host and the reason, not hundreds of copies of them.
fn upstream_error_is_news(host: &str) -> bool {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static SEEN: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);
    const EVERY: Duration = Duration::from_secs(60);
    let mut seen = SEEN.lock().unwrap();
    let seen = seen.get_or_insert_with(HashMap::new);
    let now = Instant::now();
    if seen.len() > 1_000 {
        seen.retain(|_, at| now.duration_since(*at) < EVERY);
    }
    match seen.get(host) {
        Some(at) if now.duration_since(*at) < EVERY => false,
        _ => {
            seen.insert(host.to_string(), now);
            true
        }
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
fn classify_message(msg: &Message, cap: usize) -> (WsOpcode, Option<String>, Option<String>, u64, bool) {
    use base64::Engine;
    let b64 = |bytes: &[u8]| {
        let keep = bytes.len().min(cap);
        (Some(base64::engine::general_purpose::STANDARD.encode(&bytes[..keep])), bytes.len() > cap)
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
            let reason = frame.as_ref().map(|f| format!("{} {}", u16::from(f.code), f.reason)).unwrap_or_default();
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
    let key = ws_route_key(uri.host().unwrap_or_default(), uri.path_and_query().map(|p| p.as_str()).unwrap_or("/"));
    record_ws_frame(shared, &key, direction, msg);
}

/// Attach a frame to the route registered under `key` and stream it to the UI.
/// Frames whose route is unknown (no matching handshake) are dropped rather
/// than guessed. A close frame finalizes the flow and retires the route.
fn record_ws_frame(shared: &Arc<Shared>, key: &str, direction: WsDirection, msg: &Message) {
    let route = shared.ws_routes.lock().unwrap().get(key).cloned();
    let Some(route) = route else { return };

    let (opcode, text, base64, size, truncated) = classify_message(msg, crate::WS_FRAME_CAP);
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
        retire_ws_route(shared, key, &route);
    }
}

/// The socket behind `route` is over: complete its flow and drop the route --
/// only if it is still the one registered under `key`, since a new socket to
/// the same URL replaces it there.
fn retire_ws_route(shared: &Shared, key: &str, route: &Arc<WsRoute>) {
    {
        let mut routes = shared.ws_routes.lock().unwrap();
        if routes.get(key).is_some_and(|r| Arc::ptr_eq(r, route)) {
            routes.remove(key);
        }
    }
    let started = shared.flow(&route.flow_id).map(|f| f.started_at).unwrap_or_else(now_ms);
    shared.update(&route.flow_id, |f| {
        if !matches!(f.state, FlowState::Error | FlowState::Completed) {
            f.state = FlowState::Completed;
            f.duration_ms = Some(now_ms() - started);
        }
    });
}

/// The route a WebSocket context belongs to, and its key.
fn ws_route_for(shared: &Shared, ctx: &WebSocketContext) -> (String, Option<Arc<WsRoute>>) {
    let uri = match ctx {
        WebSocketContext::ClientToServer { dst, .. } => dst,
        WebSocketContext::ServerToClient { src, .. } => src,
    };
    let key = ws_route_key(uri.host().unwrap_or_default(), uri.path_and_query().map(|p| p.as_str()).unwrap_or("/"));
    let route = shared.ws_routes.lock().unwrap().get(&key).cloned();
    (key, route)
}

/// One direction of a socket stopped relaying. When both have, the socket is
/// gone even if no Close frame went by (a dropped connection, a killed app):
/// without this its route would stay forever and its flow stay in progress.
fn ws_direction_ended(shared: &Shared, key: &str, route: &Arc<WsRoute>) {
    if route.open_directions.fetch_sub(1, Ordering::AcqRel) == 1 {
        retire_ws_route(shared, key, route);
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
    /// hudsucker's relay loop, plus noticing when it ends: see
    /// [`ws_direction_ended`].
    async fn handle_websocket(
        mut self,
        ctx: WebSocketContext,
        mut stream: impl futures::Stream<Item = Result<Message, WsError>> + Unpin + Send + 'static,
        mut sink: impl futures::Sink<Message, Error = WsError> + Unpin + Send + 'static,
    ) {
        use futures::SinkExt;
        // Looked up once, at the start: by the end a new socket to the same URL
        // may have taken the key.
        let (key, route) = ws_route_for(&self.shared, &ctx);
        while let Some(message) = stream.next().await {
            match message {
                Ok(message) => {
                    let Some(message) = self.handle_message(&ctx, message).await else { continue };
                    match sink.send(message).await {
                        Ok(()) | Err(WsError::ConnectionClosed) => {}
                        Err(e) => tracing::debug!("WebSocket send error: {e}"),
                    }
                }
                Err(e) => {
                    tracing::debug!("WebSocket message error: {e}");
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                }
            }
        }
        if let Some(route) = route {
            ws_direction_ended(&self.shared, &key, &route);
        }
    }

    async fn handle_message(&mut self, ctx: &WebSocketContext, msg: Message) -> Option<Message> {
        capture_ws_message(&self.shared, ctx, &msg);
        Some(msg)
    }
}

/// Take the HTTP/3 alternatives out of every `Alt-Svc` header, dropping a
/// header that has none left. Returns whether anything was removed.
///
/// `Alt-Svc: h3=":443"; ma=86400, h2=":443"` becomes `h2=":443"`. A `clear`
/// is kept: it withdraws earlier advertisements, which is what we want anyway.
pub fn strip_h3_alt_svc(headers: &mut hudsucker::hyper::HeaderMap) -> bool {
    use hudsucker::hyper::header::ALT_SVC;
    let values: Vec<String> =
        headers.get_all(ALT_SVC).iter().filter_map(|v| v.to_str().ok().map(str::to_string)).collect();
    if values.is_empty() {
        return false;
    }
    let mut removed = false;
    let mut kept: Vec<String> = Vec::new();
    for value in &values {
        for entry in split_alt_svc(value) {
            let proto = entry.split('=').next().unwrap_or("").trim().trim_matches('"').to_ascii_lowercase();
            if proto == "h3" || proto.starts_with("h3-") || proto == "quic" {
                removed = true;
            } else if !entry.trim().is_empty() {
                kept.push(entry.trim().to_string());
            }
        }
    }
    if !removed {
        return false;
    }
    headers.remove(ALT_SVC);
    if !kept.is_empty() {
        if let Ok(v) = HeaderValue::from_str(&kept.join(", ")) {
            headers.insert(ALT_SVC, v);
        }
    }
    true
}

/// Split an `Alt-Svc` value on the commas between alternatives, not the ones
/// inside quoted strings.
fn split_alt_svc(value: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut quoted) = (0, false);
    for (i, c) in value.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                out.push(&value[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&value[start..]);
    out
}

/// Bytes of one body that may wait between forwarding and the capture task.
const TEE_QUEUE_BYTES: usize = 8 * 1024 * 1024;

/// What the forwarding side of a [`tee`] tells its capture task.
#[derive(Default)]
struct TeeState {
    /// Every byte forwarded, captured or not: the body's real size.
    total: AtomicU64,
    /// Bytes sent to the capture task and not yet taken in.
    queued: std::sync::atomic::AtomicUsize,
    /// The capture fell [`TEE_QUEUE_BYTES`] behind and stopped taking chunks.
    abandoned: std::sync::atomic::AtomicBool,
}

/// A capture whose blocking task died: the flow still completes, with its size
/// and nothing else.
fn lost_capture(total: u64) -> crate::bodystore::Captured {
    crate::bodystore::Captured { inline: Vec::new(), total, truncated: total > 0, spilled: false }
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
    // Chunks go to the capture task over a channel bounded by bytes, not by
    // count: `TeeState::queued` is what waits in it. Forwarding never waits for
    // the capture; when the capture falls that far behind (a disk slower than
    // the network), it gives up on the rest of this body instead.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Bytes>();
    let state = Arc::new(TeeState::default());
    let throttle_shared = shared.clone();
    let producer_state = state.clone();
    // Past this many bytes nothing more is kept (the preview is full and the
    // spill file at its cap), so the rest is counted, never queued.
    let keep_limit = if shared.bodies.spills() {
        shared.bodies.per_body_cap().max(shared.bodies.inline_cap() as u64)
    } else {
        shared.bodies.inline_cap() as u64
    };

    // Consumer: retain a capped preview (spilling the rest to the body store),
    // then finalize when the sender drops.
    tokio::spawn(async move {
        let mut capture = Some(BodyCapture::new(shared.bodies.clone(), &id, side));
        while let Some(first) = rx.recv().await {
            let mut batch = vec![first];
            while batch.len() < 64 {
                match rx.try_recv() {
                    Ok(chunk) => batch.push(chunk),
                    Err(_) => break,
                }
            }
            let bytes: usize = batch.iter().map(Bytes::len).sum();
            if let Some(mut c) = capture.take() {
                // Writing the spill file blocks; keep it off the async workers.
                capture = if c.needs_disk(bytes) {
                    tokio::task::spawn_blocking(move || {
                        batch.iter().for_each(|chunk| c.push(chunk));
                        c
                    })
                    .await
                    .ok()
                } else {
                    batch.iter().for_each(|chunk| c.push(chunk));
                    Some(c)
                };
            }
            state.queued.fetch_sub(bytes, Ordering::Relaxed);
        }
        let total = state.total.load(Ordering::Relaxed);
        let captured = match capture {
            Some(c) if state.abandoned.load(Ordering::Relaxed) => {
                tracing::debug!(flow = %id, ?side, "body capture fell behind; kept the preview only");
                c.abandon(total)
            }
            Some(c) => {
                let mut done = if c.needs_disk(0) {
                    tokio::task::spawn_blocking(move || c.finish()).await.ok()
                } else {
                    Some(c.finish())
                }
                .unwrap_or_else(|| lost_capture(total));
                // Bytes past `keep_limit` were counted but never sent here.
                done.total = done.total.max(total);
                done.truncated |= done.total > done.inline.len() as u64;
                done
            }
            None => lost_capture(total),
        };
        let total = captured.total;

        let mut preview = build_preview(captured.inline, total, captured.truncated, media_type, content_encoding);
        preview.spilled = captured.spilled;
        let done = now_ms();

        // Is this an MCP exchange? Recognised from the captured body, so it works
        // for any MCP endpoint on any host — see `crate::mcp`.
        let mcp = detect_mcp(side, &preview);

        shared.update(&id, |f| match side {
            Side::Request => {
                f.request_size = total;
                f.request_body = Some(preview);
                if total > 0 {
                    f.timings.request_ms = Some((done - f.started_at).max(0.0));
                }
                if f.mcp.is_none() {
                    f.mcp = mcp;
                }
            }
            Side::Response => {
                f.response_size = total;
                f.response_body = Some(preview);
                // The request half usually names the exchange; an SSE stream the
                // client opened with GET only shows up on the response.
                if f.mcp.is_none() {
                    f.mcp = mcp;
                }
                if f.state != FlowState::Error {
                    f.state = FlowState::Completed;
                    let elapsed = (done - f.started_at).max(0.0);
                    f.duration_ms = Some(elapsed);
                    // Download is what remains after time-to-first-byte.
                    if let Some(ttfb) = f.timings.ttfb_ms {
                        f.timings.download_ms = Some((elapsed - ttfb).max(0.0));
                    }
                }
            }
        });
    });

    let stream = BodyStream::new(body).filter_map(move |frame_res| {
        let tx = tx.clone();
        let shared = throttle_shared.clone();
        let producer_state = producer_state.clone();
        async move {
            match frame_res {
                Ok(frame) => match frame.into_data() {
                    Ok(data) => {
                        let st = &producer_state;
                        let len = data.len();
                        let before = st.total.fetch_add(len as u64, Ordering::Relaxed);
                        if before < keep_limit && !st.abandoned.load(Ordering::Relaxed) {
                            if st.queued.fetch_add(len, Ordering::Relaxed) + len > TEE_QUEUE_BYTES {
                                st.queued.fetch_sub(len, Ordering::Relaxed);
                                st.abandoned.store(true, Ordering::Relaxed);
                            } else {
                                let _ = tx.send(data.clone());
                            }
                        }
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

/// Classify a finished body half as MCP traffic, if it is any.
///
/// Runs on the *captured preview*, which is already decoded and capped — so a
/// giant body costs nothing extra here, and a body truncated at the cap simply
/// fails to parse rather than being mis-tagged.
fn detect_mcp(side: Side, preview: &nova_proto::BodyPreview) -> Option<nova_proto::McpInfo> {
    let text = preview.text.as_deref()?;
    let media = preview.media_type.as_deref();
    match side {
        Side::Request => crate::mcp::detect_request(media, text),
        Side::Response => crate::mcp::detect_sse(media, text),
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
        apply_headers(&mut map, &[("bad name".into(), "x".into()), ("good".into(), "y".into())]);
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
        let (op, text, b64, size, trunc) = classify_message(&Message::binary(vec![1u8, 2, 3, 4, 5]), 3);
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
        let mut hooks = crate::EngineHooks::in_memory(Arc::new(crate::breakpoint::Breakpoints::new(Arc::new(
            crate::breakpoint::NoopBreakpointSink,
        ))));
        hooks.bodies = Arc::new(crate::bodystore::BodyStore::memory_only(1024));
        Arc::new(Shared::new(Arc::new(NoopFlowSink), ws_sink, hooks))
    }

    fn register_route(shared: &Arc<Shared>, key: &str, flow_id: &str) {
        shared.ws_routes.lock().unwrap().insert(key.to_string(), Arc::new(WsRoute::new(flow_id.to_string())));
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
            "f9".into(),
            0,
            "GET".into(),
            "https".into(),
            "host".into(),
            "/ws".into(),
            "https://host/ws".into(),
            "127.0.0.1:1".into(),
            "HTTP/1.1".into(),
            Vec::new(),
        );
        flow.is_websocket = true;
        shared.insert(flow);

        record_ws_frame(&shared, "host/ws", WsDirection::Received, &Message::Close(None));

        let stored = shared.flow("f9").unwrap();
        assert_eq!(stored.state, FlowState::Completed);
        assert!(stored.duration_ms.is_some());
        assert!(shared.ws_routes.lock().unwrap().get("host/ws").is_none(), "route is retired after close");
        // A frame arriving after close finds no route and is dropped.
        record_ws_frame(&shared, "host/ws", WsDirection::Sent, &Message::text("late"));
        assert_eq!(ws_sink.0.lock().unwrap().len(), 1, "only the close frame was recorded");
    }

    // ---- body tee ----

    #[tokio::test]
    async fn tee_counts_every_byte_but_queues_only_what_it_keeps() {
        use http_body_util::BodyExt;
        let shared = shared_for_ws(Arc::new(VecWsSink(Mutex::new(Vec::new()))));
        shared.insert(new_flow(
            "f1".into(),
            1,
            "GET".into(),
            "https".into(),
            "h".into(),
            "/".into(),
            "https://h/".into(),
            "127.0.0.1:1".into(),
            "HTTP/1.1".into(),
            Vec::new(),
        ));
        let chunks: Vec<Result<Bytes, hudsucker::Error>> = (0..64).map(|_| Ok(Bytes::from(vec![b'a'; 1024]))).collect();
        let body = Body::from_stream(futures::stream::iter(chunks));
        let teed = tee(body, shared.clone(), "f1".into(), Side::Response, Some("text/plain".into()), None);
        let forwarded = teed.collect().await.unwrap().to_bytes();
        assert_eq!(forwarded.len(), 64 * 1024, "forwarding is never cut short");

        let mut flow = None;
        for _ in 0..200 {
            flow = shared.flow("f1").filter(|f| f.response_body.is_some());
            if flow.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let flow = flow.expect("capture finalized the flow");
        assert_eq!(flow.response_size, 64 * 1024, "the real size, though only the preview was queued");
        let body = flow.response_body.unwrap();
        assert!(body.truncated && !body.spilled);
    }

    #[test]
    fn a_socket_that_drops_without_close_is_retired_when_both_directions_end() {
        let ws_sink = Arc::new(VecWsSink(Mutex::new(Vec::new())));
        let shared = shared_for_ws(ws_sink.clone());
        shared.insert(new_flow(
            "f3".into(),
            3,
            "GET".into(),
            "https".into(),
            "host".into(),
            "/ws".into(),
            "https://host/ws".into(),
            "127.0.0.1:1".into(),
            "HTTP/1.1".into(),
            Vec::new(),
        ));
        register_route(&shared, "host/ws", "f3");
        let route = shared.ws_routes.lock().unwrap().get("host/ws").cloned().unwrap();

        ws_direction_ended(&shared, "host/ws", &route);
        assert!(shared.ws_routes.lock().unwrap().contains_key("host/ws"), "one direction is still open");

        // Meanwhile a new socket to the same URL takes the key...
        register_route(&shared, "host/ws", "f4");
        ws_direction_ended(&shared, "host/ws", &route);
        assert_eq!(shared.flow("f3").unwrap().state, FlowState::Completed);
        // ...and keeps it: only the old route was retired.
        assert_eq!(shared.ws_routes.lock().unwrap().get("host/ws").unwrap().flow_id, "f4");
    }

    #[test]
    fn http3_advertisements_are_stripped_and_the_rest_kept() {
        use hudsucker::hyper::header::ALT_SVC;
        let mut h = HeaderMap::new();
        h.insert(
            ALT_SVC,
            HeaderValue::from_static("h3=\":443\"; ma=86400, h3-29=\":443\"; ma=86400, h2=\"alt.test:443\""),
        );
        assert!(strip_h3_alt_svc(&mut h));
        assert_eq!(h.get(ALT_SVC).unwrap(), "h2=\"alt.test:443\"");

        let mut only = HeaderMap::new();
        only.insert(ALT_SVC, HeaderValue::from_static("h3=\":443\""));
        assert!(strip_h3_alt_svc(&mut only));
        assert!(only.get(ALT_SVC).is_none(), "nothing left: the header goes");

        let mut none = HeaderMap::new();
        none.insert(ALT_SVC, HeaderValue::from_static("clear"));
        assert!(!strip_h3_alt_svc(&mut none));
        assert_eq!(none.get(ALT_SVC).unwrap(), "clear");
        assert!(!strip_h3_alt_svc(&mut HeaderMap::new()));
    }

    #[test]
    fn quoted_commas_do_not_split_an_alternative() {
        assert_eq!(split_alt_svc(r#"h2="a,b:443"; ma=1, h3=":443""#), vec![r#"h2="a,b:443"; ma=1"#, r#" h3=":443""#]);
    }
}
