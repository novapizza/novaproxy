//! NovaProxy engine: a hudsucker-based intercepting proxy that streams captured
//! [`Flow`](nova_proto::Flow)s to a [`FlowSink`]. Kept free of any Tauri
//! dependency so it can be tested and reused in isolation.

pub mod bodystore;
pub mod breakpoint;
pub mod ca;
pub mod flow;
pub mod flowstore;
pub mod intercept;
pub mod mcp;
pub mod procinfo;
pub mod rules;
pub mod scripting;
pub mod timing;
pub mod tlsscope;
pub mod trust;

// OS plumbing lives in its own crate so the privileged helper — which runs as
// root — links none of the engine. Re-exported so callers still say
// `nova_core::sysproxy::…`.
pub use nova_os::{helper, oscmd, sysproxy};

use std::net::SocketAddr;
use std::sync::atomic::AtomicU16;
use std::sync::{Arc, RwLock};

use anyhow::Result;
use hudsucker::rustls::crypto::aws_lc_rs;
use hudsucker::rustls::ClientConfig;
use hudsucker::Proxy;
use nova_proto::{NetworkConditions, Rule, TlsScope};
use tokio::sync::oneshot;

pub use flow::{FlowSink, NoopWsSink, Shared, WsSink};

use crate::bodystore::BodyStore;
use crate::breakpoint::Breakpoints;
use crate::flowstore::FlowStore;
use crate::ca::CaMaterial;
use crate::intercept::{NovaHandler, NovaWsHandler};
use crate::scripting::ScriptEngine;
use crate::timing::ConnectLog;

/// The live, app-shared hook state threaded into the engine: traffic rules,
/// breakpoints, and the scripting sandbox. Held in `Arc`s so edits from the UI
/// take effect without restarting the proxy.
#[derive(Clone)]
pub struct EngineHooks {
    pub rules: Arc<RwLock<Vec<Rule>>>,
    pub breakpoints: Arc<Breakpoints>,
    pub scripts: Arc<ScriptEngine>,
    pub net: Arc<RwLock<NetworkConditions>>,
    pub tls_scope: Arc<RwLock<TlsScope>>,
    /// Body storage (inline preview cap + spill-to-disk). Owned by the app, not
    /// the engine, so stored bodies stay readable after the proxy is stopped.
    pub bodies: Arc<BodyStore>,
    /// Retained flows. Also owned by the app so the UI, the Tauri commands and
    /// the MCP server all read one truth, across proxy stop/start.
    pub flows: Arc<FlowStore>,
    /// Where the instrumented connector publishes real DNS/connect/TLS timings.
    pub connects: Arc<ConnectLog>,
    /// Port of NovaProxy's own MCP endpoint (0 when it is not running). Flows to
    /// it are marked internal so an agent inspecting traffic does not mostly see
    /// its own tool calls.
    pub internal_port: Arc<AtomicU16>,
}

impl EngineHooks {
    /// Hooks with everything defaulted and no persistence — for tests, examples
    /// and any embedder that only wants capture.
    pub fn in_memory(breakpoints: Arc<Breakpoints>) -> Self {
        let bodies = Arc::new(BodyStore::memory_only(bodystore::DEFAULT_INLINE_CAP));
        Self {
            rules: Arc::new(RwLock::new(Vec::new())),
            breakpoints,
            scripts: ScriptEngine::new(),
            net: Arc::new(RwLock::new(NetworkConditions::default())),
            tls_scope: Arc::new(RwLock::new(TlsScope::default())),
            flows: Arc::new(FlowStore::new(bodies.clone(), DEFAULT_MAX_FLOWS)),
            bodies,
            connects: Arc::new(ConnectLog::default()),
            internal_port: Arc::new(AtomicU16::new(0)),
        }
    }
}

/// Default cap on how many body bytes we retain *in memory* per message. Bodies
/// larger than this keep streaming to the client and are spilled to the on-disk
/// body store, so the Inspector can still show them in full on request.
pub const DEFAULT_BODY_CAP: usize = bodystore::DEFAULT_INLINE_CAP;

/// Default retention window: how many flows we keep before evicting the oldest
/// (together with their spilled bodies).
pub const DEFAULT_MAX_FLOWS: usize = 10_000;

pub struct EngineConfig {
    pub addr: SocketAddr,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            addr: SocketAddr::from(([127, 0, 0, 1], 9090)),
        }
    }
}

/// A running engine. Dropping it (or calling [`EngineHandle::stop`]) triggers a
/// graceful shutdown of the proxy task.
pub struct EngineHandle {
    stop: Option<oneshot::Sender<()>>,
    pub addr: SocketAddr,
    shared: Arc<Shared>,
}

impl EngineHandle {
    pub fn stop(mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
    }

    pub fn flows_captured(&self) -> u64 {
        self.shared.flows.total_captured()
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
    }
}

/// Start the proxy on `config.addr`, signing leaf certs with `ca` and streaming
/// flows to `sink`. Returns immediately; the proxy runs as a tokio task.
pub fn start(
    config: EngineConfig,
    ca: &CaMaterial,
    sink: Arc<dyn FlowSink>,
    ws_sink: Arc<dyn WsSink>,
    hooks: EngineHooks,
) -> Result<EngineHandle> {
    // rustls 0.23 wants a process-default provider; explicit providers are also
    // passed below, but installing one keeps any internal defaults happy.
    let _ = aws_lc_rs::default_provider().install_default();

    let authority = ca.authority()?;
    let connects = hooks.connects.clone();
    let shared = Arc::new(Shared::new(sink, ws_sink, hooks));
    let handler = NovaHandler::new(shared.clone());
    let ws_handler = NovaWsHandler::new(shared.clone());
    let (stop_tx, stop_rx) = oneshot::channel::<()>();

    // Our own connector chain instead of `with_rustls_connector`: it is the only
    // place DNS / TCP / TLS timings can be measured (see `timing`). The same TLS
    // config is handed to the WebSocket connector, which `with_rustls_connector`
    // would otherwise have configured for us.
    let tls_config = client_tls_config()?;
    let connector = timing::instrumented_connector(tls_config.clone(), connects);

    let proxy = Proxy::builder()
        .with_addr(config.addr)
        .with_ca(authority)
        .with_http_connector(connector)
        .with_websocket_connector(hudsucker::tokio_tungstenite::Connector::Rustls(tls_config))
        .with_http_handler(handler)
        .with_websocket_handler(ws_handler)
        .with_graceful_shutdown(async move {
            let _ = stop_rx.await;
        })
        .build()?;

    tokio::spawn(async move {
        if let Err(e) = proxy.start().await {
            tracing::error!("proxy engine stopped with error: {e}");
        }
    });

    Ok(EngineHandle {
        stop: Some(stop_tx),
        addr: config.addr,
        shared,
    })
}

/// TLS configuration for connections we make *upstream* (to origin servers),
/// trusting the webpki root set — unrelated to the CA we present downstream.
fn client_tls_config() -> Result<Arc<ClientConfig>> {
    use hyper_rustls::ConfigBuilderExt;
    let config = ClientConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_webpki_roots()
        .with_no_client_auth();
    Ok(Arc::new(config))
}
