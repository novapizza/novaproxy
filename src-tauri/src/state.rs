//! App-wide state: the CA material, the running engine handle, and the sink that
//! bridges engine flow updates onto the frontend IPC channel.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use std::sync::atomic::{AtomicBool, AtomicU16};

use nova_core::bodystore::BodyStore;
use nova_core::breakpoint::{BreakpointSink, Breakpoints};
use nova_core::flowstore::FlowStore;
use nova_core::scripting::ScriptEngine;
use nova_core::timing::ConnectLog;
use nova_core::{ca::CaMaterial, EngineHandle, FlowSink, WsSink};

use crate::mcp::McpHandle;
use nova_proto::{Flow, Interception, NetworkConditions, Rule, TlsScope, WsMessage};
use tauri::ipc::Channel;

/// Forwards engine [`Flow`] updates to whichever frontend channel is currently
/// subscribed. Cheap no-op when nothing is listening yet.
#[derive(Default)]
pub struct ChannelSink {
    channel: Mutex<Option<Channel<Flow>>>,
}

impl ChannelSink {
    pub fn set_channel(&self, channel: Channel<Flow>) {
        *self.channel.lock().unwrap() = Some(channel);
    }
}

impl FlowSink for ChannelSink {
    fn emit(&self, flow: Flow) {
        if let Some(channel) = self.channel.lock().unwrap().as_ref() {
            let _ = channel.send(flow);
        }
    }
}

/// Forwards captured WebSocket frames onto the frontend WS channel.
#[derive(Default)]
pub struct WsChannelSink {
    channel: Mutex<Option<Channel<WsMessage>>>,
}

impl WsChannelSink {
    pub fn set_channel(&self, channel: Channel<WsMessage>) {
        *self.channel.lock().unwrap() = Some(channel);
    }
}

impl WsSink for WsChannelSink {
    fn emit(&self, msg: WsMessage) {
        if let Some(channel) = self.channel.lock().unwrap().as_ref() {
            let _ = channel.send(msg);
        }
    }
}

/// Forwards paused-request notifications onto the frontend breakpoint channel.
#[derive(Default)]
pub struct BreakpointChannelSink {
    channel: Mutex<Option<Channel<Interception>>>,
}

impl BreakpointChannelSink {
    pub fn set_channel(&self, channel: Channel<Interception>) {
        *self.channel.lock().unwrap() = Some(channel);
    }
}

impl BreakpointSink for BreakpointChannelSink {
    fn paused(&self, interception: Interception) {
        if let Some(channel) = self.channel.lock().unwrap().as_ref() {
            let _ = channel.send(interception);
        }
    }
}

pub struct AppState {
    pub data_dir: PathBuf,
    pub ca: Mutex<Option<CaMaterial>>,
    pub engine: Mutex<Option<EngineHandle>>,
    pub sink: Arc<ChannelSink>,
    /// Sink that bridges WebSocket frames onto the frontend WS channel.
    pub ws_sink: Arc<WsChannelSink>,
    /// Shared with the engine so rule edits apply live, without a restart.
    pub rules: Arc<RwLock<Vec<Rule>>>,
    /// Whether we currently own the OS system-proxy setting.
    pub system_proxy: Mutex<bool>,
    /// Breakpoint engine + its channel sink (kept so we can bind a channel).
    pub bp_sink: Arc<BreakpointChannelSink>,
    pub breakpoints: Arc<Breakpoints>,
    /// JavaScript scripting sandbox.
    pub scripts: Arc<ScriptEngine>,
    /// Simulated network conditions (latency / throttle).
    pub net: Arc<RwLock<NetworkConditions>>,
    /// Per-host SSL-proxying scope (decrypt vs tunnel).
    pub tls_scope: Arc<RwLock<TlsScope>>,
    /// Body storage. Owned here rather than by the engine so spilled bodies stay
    /// readable across proxy stop/start within a session.
    pub bodies: Arc<BodyStore>,
    /// Captured flows, likewise owned here: the UI, the commands and the MCP
    /// server all read this one store, and it survives proxy stop/start.
    pub flows: Arc<FlowStore>,
    /// Real DNS/connect/TLS measurements published by the instrumented connector.
    pub connects: Arc<ConnectLog>,
    /// The running MCP endpoint, when enabled.
    pub mcp: Mutex<Option<McpHandle>>,
    /// Port of that endpoint (0 = not running), shared with the engine so traffic
    /// to it can be marked internal.
    pub mcp_port: Arc<AtomicU16>,
    /// A snapshot from a previous session is waiting to be restored, and doing it
    /// needs a privilege we cannot get without asking. Surfaced in `ProxyStatus`
    /// so the UI can offer the restore rather than the app raising a password
    /// dialog by itself during launch.
    pub pending_restore: AtomicBool,
    /// App name → icon `data:` URL, or `None` when that app has no bundle to
    /// read one from. Cached both ways: extracting an icon shells out to `sips`,
    /// and a row that mentions a CLI tool must not pay for that on every repaint.
    pub app_icons: Mutex<HashMap<String, Option<String>>>,
}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Self {
        let bp_sink = Arc::new(BreakpointChannelSink::default());
        let breakpoints = Arc::new(Breakpoints::new(bp_sink.clone()));
        let bodies = Arc::new(BodyStore::new(
            data_dir.join("bodies"),
            nova_core::DEFAULT_BODY_CAP,
            nova_core::bodystore::DEFAULT_PER_BODY_CAP,
            nova_core::bodystore::DEFAULT_DISK_BUDGET,
        ));
        let flows = Arc::new(FlowStore::new(bodies.clone(), nova_core::DEFAULT_MAX_FLOWS));
        Self {
            data_dir,
            ca: Mutex::new(None),
            engine: Mutex::new(None),
            sink: Arc::new(ChannelSink::default()),
            ws_sink: Arc::new(WsChannelSink::default()),
            rules: Arc::new(RwLock::new(Vec::new())),
            system_proxy: Mutex::new(false),
            bp_sink,
            breakpoints,
            scripts: ScriptEngine::new(),
            net: Arc::new(RwLock::new(NetworkConditions::default())),
            tls_scope: Arc::new(RwLock::new(TlsScope::default())),
            bodies,
            flows,
            connects: Arc::new(ConnectLog::default()),
            mcp: Mutex::new(None),
            mcp_port: Arc::new(AtomicU16::new(0)),
            pending_restore: AtomicBool::new(false),
            app_icons: Mutex::new(HashMap::new()),
        }
    }

    /// The engine hooks assembled from this app state.
    pub fn hooks(&self) -> nova_core::EngineHooks {
        nova_core::EngineHooks {
            rules: self.rules.clone(),
            breakpoints: self.breakpoints.clone(),
            scripts: self.scripts.clone(),
            net: self.net.clone(),
            tls_scope: self.tls_scope.clone(),
            bodies: self.bodies.clone(),
            flows: self.flows.clone(),
            connects: self.connects.clone(),
            internal_port: self.mcp_port.clone(),
        }
    }

    /// Persist the rule set to the app data dir.
    pub fn persist_rules(&self, rules: &[Rule]) -> Result<(), String> {
        let json = serde_json::to_string_pretty(rules).map_err(|e| e.to_string())?;
        std::fs::write(self.rules_path(), json).map_err(|e| e.to_string())
    }

    pub fn script_path(&self) -> PathBuf {
        self.data_dir.join("script.js")
    }

    pub fn net_path(&self) -> PathBuf {
        self.data_dir.join("network.json")
    }

    pub fn tls_scope_path(&self) -> PathBuf {
        self.data_dir.join("tls_scope.json")
    }

    pub fn rules_path(&self) -> PathBuf {
        self.data_dir.join("rules.json")
    }

    pub fn sysproxy_backup_path(&self) -> PathBuf {
        self.data_dir.join("sysproxy_backup.json")
    }

    pub fn mcp_path(&self) -> PathBuf {
        self.data_dir.join("mcp.json")
    }
}
