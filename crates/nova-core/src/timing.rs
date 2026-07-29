//! Real timing instrumentation.
//!
//! The Timing tab used to render fabricated bars, which in a debugging tool is
//! worse than showing nothing. Everything reported now comes from a measurement:
//!
//! | Phase | Measured where |
//! |---|---|
//! | DNS | around the connector's resolver call |
//! | TCP connect | around the plaintext connector, minus DNS |
//! | TLS handshake | around the HTTPS connector, minus the plaintext connect |
//! | Request | request body streamed → last chunk teed |
//! | TTFB | request seen → upstream response headers |
//! | Download | response headers → last body chunk |
//!
//! hudsucker surfaces none of the first three, so this module wraps the client
//! connector chain ([`instrumented_connector`]) and publishes each *new*
//! connection's phases into a [`ConnectLog`]. A flow then claims the connection
//! it opened. Connection reuse is the common case on a keep-alive/HTTP-2
//! connection: there is genuinely no DNS/connect/TLS cost to report, and the
//! flow is marked `connection_reused` instead of being handed invented numbers.
//!
//! **Correlating a connect with a flow.** The three phases are measured per
//! *connection*, and hyper's pool decides when to open one. A sample is
//! therefore claimed by the first flow to the same host whose request started
//! before the connect began — the connect for flow F always begins after F is
//! handed to the client. Claims are one-shot, so two concurrent flows to one
//! host cannot both take credit for the same handshake.
//!
//! Within one connect the phases can't be mixed up at all: the split is carried
//! in a **task-local**, so the resolver and the plaintext connector write into
//! the very future that is establishing that connection, not into a shared
//! host-keyed map that concurrent connects would race on.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Instant;

use hudsucker::hyper::Uri;
use tower_service::Service;

use crate::flow::now_ms;

/// Samples older than this are dropped: they belong to connections whose flow
/// never claimed them (errored, aborted), and keeping them risks a later flow to
/// the same host claiming a stale handshake.
const MAX_SAMPLE_AGE_MS: f64 = 60_000.0;

/// Tolerance when comparing a connect's start against a flow's start, absorbing
/// the sub-millisecond skew between the two `now_ms()` reads.
const CLOCK_SLOP_MS: f64 = 2.0;

/// One measured connection setup, waiting to be claimed by the flow that
/// triggered it.
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectSample {
    /// Host the connection was opened to, lowercased and without a port.
    pub host: String,
    /// Epoch ms when the connect started.
    pub at_ms: f64,
    pub dns_ms: Option<f64>,
    pub connect_ms: Option<f64>,
    pub tls_ms: Option<f64>,
}

/// Bounded log of unclaimed connection measurements.
pub struct ConnectLog {
    samples: Mutex<VecDeque<ConnectSample>>,
    cap: usize,
}

impl Default for ConnectLog {
    fn default() -> Self {
        Self::new(512)
    }
}

impl ConnectLog {
    pub fn new(cap: usize) -> Self {
        Self {
            samples: Mutex::new(VecDeque::new()),
            cap: cap.max(1),
        }
    }

    /// Record a completed connection setup.
    pub fn record(&self, sample: ConnectSample) {
        let mut samples = self.samples.lock().unwrap();
        let cutoff = sample.at_ms - MAX_SAMPLE_AGE_MS;
        samples.retain(|s| s.at_ms >= cutoff);
        samples.push_back(sample);
        while samples.len() > self.cap {
            samples.pop_front();
        }
    }

    /// Take the connection a flow to `host` (started at `flow_started_ms`) opened,
    /// or `None` when it reused a pooled connection.
    pub fn claim(&self, host: &str, flow_started_ms: f64) -> Option<ConnectSample> {
        let host = normalize_host(host);
        let mut samples = self.samples.lock().unwrap();
        let idx = samples
            .iter()
            .position(|s| s.host == host && s.at_ms >= flow_started_ms - CLOCK_SLOP_MS)?;
        samples.remove(idx)
    }

    /// How many measurements are waiting to be claimed (diagnostics/tests).
    pub fn pending(&self) -> usize {
        self.samples.lock().unwrap().len()
    }
}

/// Lowercase a host and drop any `:port` suffix, so the connector's URI host and
/// a flow's `Host`-header-derived host agree.
pub fn normalize_host(host: &str) -> String {
    let host = match host.rsplit_once(':') {
        Some((h, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => host,
    };
    host.trim_matches(|c| c == '[' || c == ']').to_ascii_lowercase()
}

/// Split a connect's cumulative measurements into per-phase durations.
///
/// `total_ms` covers the whole connector call (DNS + TCP + TLS); `tcp_total_ms`
/// covers DNS + TCP. Each phase is the difference, clamped at zero so timer
/// granularity can never produce a negative bar.
pub fn split_phases(
    total_ms: f64,
    tcp_total_ms: Option<f64>,
    dns_ms: Option<f64>,
    tls: bool,
) -> (Option<f64>, Option<f64>, Option<f64>) {
    let connect = tcp_total_ms.map(|tcp| (tcp - dns_ms.unwrap_or(0.0)).max(0.0));
    let tls_ms = match (tls, tcp_total_ms) {
        (true, Some(tcp)) => Some((total_ms - tcp).max(0.0)),
        // Plaintext HTTP has no handshake, and without the inner measurement we
        // cannot separate one — report nothing rather than a guess.
        _ => None,
    };
    (dns_ms, connect, tls_ms)
}

/* ------------------------ the instrumented connector ------------------------ */

/// Per-connect scratch space. Lives in a task-local so the resolver and the
/// plaintext connector — both awaited inside the outer connector's future —
/// report into the connection they are actually establishing.
#[derive(Debug, Clone, Copy, Default)]
struct Partial {
    dns_ms: Option<f64>,
    tcp_total_ms: Option<f64>,
}

tokio::task_local! {
    static PARTIAL: RefCell<Partial>;
}

fn elapsed_ms(t0: Instant) -> f64 {
    t0.elapsed().as_secs_f64() * 1000.0
}

/// Where in the connector chain a [`Timed`] wrapper sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layer {
    /// Outermost: the whole connect, which finalizes and records the sample.
    Whole,
    /// The plaintext connect (DNS + TCP), reported into the task-local.
    Tcp,
}

/// Times an inner connector and reports according to its [`Layer`].
#[derive(Clone)]
pub struct Timed<C> {
    inner: C,
    log: Arc<ConnectLog>,
    layer: Layer,
}

impl<C> Service<Uri> for Timed<C>
where
    C: Service<Uri> + Clone + Send + 'static,
    C::Future: Send + 'static,
{
    type Response = C::Response;
    type Error = C::Error;
    type Future = Pin<Box<dyn Future<Output = Result<C::Response, C::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        // Clone the inner service into the future: hyper's pool drives many
        // connects concurrently, and each needs its own ready service.
        let mut inner = self.inner.clone();
        let log = self.log.clone();
        match self.layer {
            Layer::Tcp => Box::pin(async move {
                let t0 = Instant::now();
                let res = inner.call(uri).await;
                let ms = elapsed_ms(t0);
                let _ = PARTIAL.try_with(|p| p.borrow_mut().tcp_total_ms = Some(ms));
                res
            }),
            Layer::Whole => {
                let host = uri.host().map(normalize_host).unwrap_or_default();
                let tls = uri.scheme_str() != Some("http");
                Box::pin(PARTIAL.scope(RefCell::new(Partial::default()), async move {
                    let at_ms = now_ms();
                    let t0 = Instant::now();
                    let res = inner.call(uri).await;
                    let total = elapsed_ms(t0);
                    // Only successful connects become claimable samples: a failed
                    // connect carries no phases worth attributing to a flow.
                    if res.is_ok() {
                        let partial = PARTIAL.with(|p| *p.borrow());
                        let (dns_ms, connect_ms, tls_ms) =
                            split_phases(total, partial.tcp_total_ms, partial.dns_ms, tls);
                        log.record(ConnectSample { host, at_ms, dns_ms, connect_ms, tls_ms });
                    }
                    res
                }))
            }
        }
    }
}

/// Times DNS resolution, reporting into the current connect's task-local.
#[derive(Clone)]
pub struct TimedResolver<R> {
    inner: R,
}

impl<R> TimedResolver<R> {
    pub fn new(inner: R) -> Self {
        Self { inner }
    }
}

impl<R, N> Service<N> for TimedResolver<R>
where
    R: Service<N> + Clone + Send + 'static,
    R::Future: Send + 'static,
    N: Send + 'static,
{
    type Response = R::Response;
    type Error = R::Error;
    type Future = Pin<Box<dyn Future<Output = Result<R::Response, R::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, name: N) -> Self::Future {
        let mut inner = self.inner.clone();
        Box::pin(async move {
            let t0 = Instant::now();
            let res = inner.call(name).await;
            let ms = elapsed_ms(t0);
            let _ = PARTIAL.try_with(|p| p.borrow_mut().dns_ms = Some(ms));
            res
        })
    }
}

/// Build the client connector hudsucker will use, instrumented end to end:
/// `Timed(HTTPS handshake → Timed(TCP connect → TimedResolver(DNS)))`.
///
/// Returns the connector; measurements land in `log`.
pub fn instrumented_connector(
    tls_config: Arc<hudsucker::rustls::ClientConfig>,
    log: Arc<ConnectLog>,
) -> impl hudsucker::hyper_util::client::legacy::connect::Connect + Clone + Send + Sync + 'static {
    use hudsucker::hyper_util::client::legacy::connect::dns::GaiResolver;
    use hudsucker::hyper_util::client::legacy::connect::HttpConnector;

    let mut http = HttpConnector::new_with_resolver(TimedResolver::new(GaiResolver::new()));
    // The HTTPS connector layers TLS on top, so the inner connector must accept
    // `https://` URIs.
    http.enforce_http(false);
    let http = Timed { inner: http, log: log.clone(), layer: Layer::Tcp };

    // HTTP/1.1 only upstream, matching what hudsucker's own connector advertises
    // under its default features. Advertising h2 here without enabling it in
    // hudsucker would negotiate a protocol the forwarding client cannot speak,
    // and h2 multiplexing also weakens the request↔response correlation the
    // handler relies on (see `intercept`). Turning HTTP/2 on is a deliberate,
    // separate change — not a side effect of instrumenting timings.
    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config((*tls_config).clone())
        .https_or_http()
        .enable_http1()
        .wrap_connector(http);

    Timed { inner: https, log, layer: Layer::Whole }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(host: &str, at_ms: f64) -> ConnectSample {
        ConnectSample {
            host: host.into(),
            at_ms,
            dns_ms: Some(1.0),
            connect_ms: Some(2.0),
            tls_ms: Some(3.0),
        }
    }

    #[test]
    fn phases_are_differences_of_the_cumulative_timers() {
        // total = DNS+TCP+TLS = 100, dns+tcp = 40, dns = 10
        let (dns, connect, tls) = split_phases(100.0, Some(40.0), Some(10.0), true);
        assert_eq!(dns, Some(10.0));
        assert_eq!(connect, Some(30.0), "TCP is the plaintext connect minus DNS");
        assert_eq!(tls, Some(60.0), "TLS is the total minus the plaintext connect");
    }

    #[test]
    fn plaintext_http_reports_no_tls_phase() {
        let (_dns, connect, tls) = split_phases(40.0, Some(40.0), Some(10.0), false);
        assert_eq!(connect, Some(30.0));
        assert_eq!(tls, None, "there is no handshake on plain HTTP");
    }

    #[test]
    fn missing_inner_measurement_reports_no_phase_rather_than_a_guess() {
        // The task-local never got written (no DNS/TCP attribution available):
        // report nothing rather than attributing the whole total to one phase.
        let (dns, connect, tls) = split_phases(50.0, None, None, true);
        assert_eq!(dns, None);
        assert_eq!(connect, None);
        assert_eq!(tls, None);
    }

    #[test]
    fn phases_never_go_negative() {
        // Timer granularity can make the inner measurement look larger.
        let (_dns, connect, tls) = split_phases(9.9, Some(10.0), Some(10.1), true);
        assert_eq!(connect, Some(0.0));
        assert_eq!(tls, Some(0.0));
    }

    #[test]
    fn cached_dns_is_reported_as_zero_not_as_missing() {
        // A resolver hit is a real measurement of ~0ms; it must not be confused
        // with "we couldn't measure DNS".
        let (dns, connect, _tls) = split_phases(20.0, Some(5.0), Some(0.0), true);
        assert_eq!(dns, Some(0.0));
        assert_eq!(connect, Some(5.0));
    }

    #[test]
    fn host_normalization_matches_connector_and_flow_spellings() {
        assert_eq!(normalize_host("API.example.com:443"), "api.example.com");
        assert_eq!(normalize_host("api.example.com"), "api.example.com");
        // A non-numeric suffix after ':' is not a port and must survive.
        assert_eq!(normalize_host("weird:name"), "weird:name");
        // IPv6 literals arrive bracketed from a URI authority.
        assert_eq!(normalize_host("[::1]"), "::1");
    }

    #[test]
    fn claim_takes_the_connection_the_flow_opened() {
        let log = ConnectLog::default();
        log.record(sample("api.example.com", 1_000.0));
        // The flow started before the connect began, and matches on host.
        let got = log.claim("api.example.com:443", 900.0).expect("claimed");
        assert_eq!(got.at_ms, 1_000.0);
        assert_eq!(log.pending(), 0, "claims are one-shot");
    }

    #[test]
    fn claim_ignores_connections_opened_before_the_flow_started() {
        // A connect that predates the request cannot be that request's connect —
        // this is exactly the mis-attribution that would fabricate timings.
        let log = ConnectLog::default();
        log.record(sample("api.example.com", 1_000.0));
        assert!(log.claim("api.example.com", 5_000.0).is_none());
        assert_eq!(log.pending(), 1, "the unmatched sample stays available");
    }

    #[test]
    fn claim_of_a_reused_connection_finds_nothing() {
        let log = ConnectLog::default();
        assert!(log.claim("api.example.com", 0.0).is_none());
    }

    #[test]
    fn claim_matches_on_host() {
        let log = ConnectLog::default();
        log.record(sample("other.example.com", 1_000.0));
        assert!(log.claim("api.example.com", 0.0).is_none());
        assert!(log.claim("other.example.com", 0.0).is_some());
    }

    #[test]
    fn concurrent_flows_to_one_host_each_claim_their_own_connection() {
        let log = ConnectLog::default();
        log.record(sample("api.example.com", 1_000.0));
        log.record(sample("api.example.com", 1_100.0));
        let first = log.claim("api.example.com", 900.0).unwrap();
        let second = log.claim("api.example.com", 900.0).unwrap();
        assert_eq!(first.at_ms, 1_000.0, "oldest matching sample first");
        assert_eq!(second.at_ms, 1_100.0);
        assert!(log.claim("api.example.com", 900.0).is_none());
    }

    #[test]
    fn log_is_bounded_and_drops_the_oldest() {
        let log = ConnectLog::new(2);
        log.record(sample("h", 1.0));
        log.record(sample("h", 2.0));
        log.record(sample("h", 3.0));
        assert_eq!(log.pending(), 2);
        assert_eq!(log.claim("h", 0.0).unwrap().at_ms, 2.0);
    }

    #[test]
    fn stale_samples_are_pruned_so_they_cannot_be_mis_claimed() {
        let log = ConnectLog::default();
        log.record(sample("h", 1_000.0));
        // A connect a full MAX_SAMPLE_AGE_MS later evicts the abandoned one.
        log.record(sample("h", 1_000.0 + MAX_SAMPLE_AGE_MS + 1.0));
        assert_eq!(log.pending(), 1);
        assert!(log.claim("h", 0.0).unwrap().at_ms > 1_000.0);
    }

    #[test]
    fn claim_tolerates_sub_millisecond_clock_skew() {
        // The connect appears to start a hair before the request that caused it.
        let log = ConnectLog::default();
        log.record(sample("h", 999.5));
        assert!(log.claim("h", 1_000.0).is_some());
    }
}
