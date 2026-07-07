//! NovaProxy engine: a hudsucker-based intercepting proxy that streams captured
//! [`Flow`](nova_proto::Flow)s to a [`FlowSink`]. Kept free of any Tauri
//! dependency so it can be tested and reused in isolation.

pub mod breakpoint;
pub mod ca;
pub mod flow;
pub mod intercept;
pub mod rules;
pub mod scripting;
pub mod sysproxy;
pub mod trust;

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};

use anyhow::Result;
use hudsucker::rustls::crypto::aws_lc_rs;
use hudsucker::Proxy;
use nova_proto::{NetworkConditions, Rule};
use tokio::sync::oneshot;

pub use flow::{FlowSink, Shared};

use crate::breakpoint::Breakpoints;
use crate::ca::CaMaterial;
use crate::intercept::{NovaHandler, NovaWsHandler};
use crate::scripting::ScriptEngine;

/// The live, app-shared hook state threaded into the engine: traffic rules,
/// breakpoints, and the scripting sandbox. Held in `Arc`s so edits from the UI
/// take effect without restarting the proxy.
#[derive(Clone)]
pub struct EngineHooks {
    pub rules: Arc<RwLock<Vec<Rule>>>,
    pub breakpoints: Arc<Breakpoints>,
    pub scripts: Arc<ScriptEngine>,
    pub net: Arc<RwLock<NetworkConditions>>,
}

/// Default cap on how many body bytes we retain per message for the inspector.
pub const DEFAULT_BODY_CAP: usize = 4 * 1024 * 1024;

pub struct EngineConfig {
    pub addr: SocketAddr,
    pub body_cap: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            addr: SocketAddr::from(([127, 0, 0, 1], 9090)),
            body_cap: DEFAULT_BODY_CAP,
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
        self.shared.total_captured.load(Ordering::Relaxed)
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
    hooks: EngineHooks,
) -> Result<EngineHandle> {
    // rustls 0.23 wants a process-default provider; explicit providers are also
    // passed below, but installing one keeps any internal defaults happy.
    let _ = aws_lc_rs::default_provider().install_default();

    let authority = ca.authority()?;
    let shared = Arc::new(Shared::new(
        sink,
        config.body_cap,
        hooks.rules,
        hooks.breakpoints,
        hooks.scripts,
        hooks.net,
    ));
    let handler = NovaHandler::new(shared.clone());
    let (stop_tx, stop_rx) = oneshot::channel::<()>();

    let proxy = Proxy::builder()
        .with_addr(config.addr)
        .with_ca(authority)
        .with_rustls_connector(aws_lc_rs::default_provider())
        .with_http_handler(handler)
        .with_websocket_handler(NovaWsHandler)
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
