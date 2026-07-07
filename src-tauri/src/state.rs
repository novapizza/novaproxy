//! App-wide state: the CA material, the running engine handle, and the sink that
//! bridges engine flow updates onto the frontend IPC channel.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use nova_core::breakpoint::{BreakpointSink, Breakpoints};
use nova_core::scripting::ScriptEngine;
use nova_core::{ca::CaMaterial, EngineHandle, FlowSink, WsSink};
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
}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Self {
        let bp_sink = Arc::new(BreakpointChannelSink::default());
        let breakpoints = Arc::new(Breakpoints::new(bp_sink.clone()));
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
        }
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
}
