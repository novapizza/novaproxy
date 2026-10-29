//! App-wide state: the CA material, the running engine handle, and the sink that
//! bridges engine flow updates onto the frontend IPC channel.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use serde::Serialize;

use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};

use nova_core::bodystore::BodyStore;
use nova_core::breakpoint::{BreakpointSink, Breakpoints};
use nova_core::flowstore::FlowStore;
use nova_core::scripting::ScriptEngine;
use nova_core::timing::ConnectLog;
use nova_core::{ca::CaMaterial, EngineHandle, FlowSink, WsSink};

use crate::mcp::McpHandle;
use nova_proto::{Flow, Interception, NetworkConditions, Rule, TlsScope, WsMessage};

/// The snapshot of the OS proxy settings taken before NovaProxy changed them.
/// Named here because the sign-in restore reads it without an `AppState`.
pub const SYSPROXY_BACKUP_FILE: &str = "sysproxy_backup.json";
use tauri::ipc::Channel;

/// Flows queued per message at most. A backlog (a resume, a burst) goes out as
/// several messages rather than one the webview has to parse in a single go.
const MAX_FLOWS_PER_MESSAGE: usize = 500;

/// How often queued snapshots go out. Ten a second is well past what a list
/// can usefully show, and each message costs the webview a parse whatever its
/// size, so coalescing here is most of the win under heavy capture.
const FLUSH_EVERY: Duration = Duration::from_millis(100);

/// What the UI receives each tick: the latest snapshot of every flow that
/// changed, then the WebSocket frames. One message for both, so a socket's
/// frames can never overtake the snapshot that introduces its flow.
#[derive(Serialize)]
pub struct StreamBatch {
    pub flows: Vec<Flow>,
    pub ws: Vec<WsMessage>,
}

/// The engine's flow and WebSocket sink: coalesces updates and streams them to
/// the UI in batches.
///
/// Three things it does that a plain forward did not, each because the webview
/// paid for it:
///
/// - **No body bytes.** Snapshots go out without `text`/`base64`. A flow sends
///   three to five snapshots and every one after its body was captured used to
///   carry the whole preview again (up to 512 KB a body), which the UI parsed and
///   then dropped — it fetches the bytes of the flow actually open through
///   `read_body`. Without them a snapshot is a few KB and goes through Tauri's
///   direct path instead of its fetch queue, which nothing bounds.
/// - **Latest snapshot only.** Within a tick a flow is sent once, as it now is.
/// - **Paused while the UI is not recording.** The UI dropped what arrived then
///   anyway. Flows that already existed are still noted when they change, and
///   their current state is queued on resume; flows that start while paused are
///   left out, which is what pausing has always meant in the list. The engine
///   keeps capturing either way — the MCP server still sees everything.
#[derive(Default)]
pub struct UiStream {
    state: Mutex<StreamState>,
    /// Set once the flush thread is running; a reload rebinds, it does not respawn.
    flusher: AtomicBool,
}

#[derive(Default)]
struct StreamState {
    channel: Option<Channel<StreamBatch>>,
    /// Queued snapshots, in the order their flows were first queued.
    flows: Vec<Flow>,
    /// Flow id → its slot in `flows`, so a newer snapshot replaces the queued one.
    slot: HashMap<String, usize>,
    ws: Vec<WsMessage>,
    /// Highest `seq` emitted so far: where "already existed" ends on a pause.
    last_seq: u64,
    paused: Option<Paused>,
}

#[derive(Default)]
struct Paused {
    /// Flows with a `seq` up to this one existed when the UI paused.
    through_seq: u64,
    /// Those of them that changed since, in first-change order.
    changed: Vec<String>,
    seen: HashSet<String>,
}

impl StreamState {
    fn queue(&mut self, flow: Flow) {
        match self.slot.get(&flow.id) {
            Some(&i) => self.flows[i] = flow,
            None => {
                self.slot.insert(flow.id.clone(), self.flows.len());
                self.flows.push(flow);
            }
        }
    }
}

/// A snapshot without its body bytes; size, media type and the truncated and
/// spilled flags stay so the UI knows what there is to fetch.
fn without_bytes(mut flow: Flow) -> Flow {
    for body in [&mut flow.request_body, &mut flow.response_body].into_iter().flatten() {
        body.text = None;
        body.base64 = None;
    }
    flow
}

impl UiStream {
    /// Bind the UI's channel (again, after a reload) and start the flush loop.
    pub fn set_channel(self: &Arc<Self>, channel: Channel<StreamBatch>) {
        self.bind(channel);
        if !self.flusher.swap(true, Ordering::SeqCst) {
            let me = Arc::clone(self);
            std::thread::Builder::new()
                .name("ui-stream".into())
                .spawn(move || loop {
                    std::thread::sleep(FLUSH_EVERY);
                    me.flush();
                })
                .expect("spawn ui-stream thread");
        }
    }

    fn bind(&self, channel: Channel<StreamBatch>) {
        let mut st = self.state.lock().unwrap();
        st.channel = Some(channel);
        // A reload starts from an empty list: whatever was paused for the
        // previous page has nobody to resume it.
        st.paused = None;
    }

    /// Stop sending flow snapshots, noting which existing flows change meanwhile.
    pub fn pause(&self) {
        let mut st = self.state.lock().unwrap();
        if st.paused.is_none() {
            let through_seq = st.last_seq;
            st.paused = Some(Paused { through_seq, ..Paused::default() });
        }
    }

    /// Resume, queueing the current state of every flow that changed while paused
    /// and is still retained.
    pub fn resume(&self, store: &FlowStore) {
        let mut st = self.state.lock().unwrap();
        let Some(paused) = st.paused.take() else { return };
        for id in paused.changed {
            if let Some(flow) = store.get(&id) {
                st.queue(without_bytes(flow));
            }
        }
    }

    /// Drop queued snapshots: the capture they belong to was just cleared, and
    /// sending them would bring cleared flows back as new ones.
    pub fn discard_flows(&self) {
        let mut st = self.state.lock().unwrap();
        st.flows.clear();
        st.slot.clear();
        if let Some(p) = st.paused.as_mut() {
            p.changed.clear();
            p.seen.clear();
        }
    }

    fn flush(&self) {
        let (mut flows, mut ws, channel) = {
            let mut st = self.state.lock().unwrap();
            if st.flows.is_empty() && st.ws.is_empty() {
                return;
            }
            let Some(channel) = st.channel.clone() else { return };
            st.slot.clear();
            (std::mem::take(&mut st.flows), std::mem::take(&mut st.ws), channel)
        };
        loop {
            let rest = flows.split_off(flows.len().min(MAX_FLOWS_PER_MESSAGE));
            let last = rest.is_empty();
            let batch = StreamBatch {
                flows,
                ws: if last { std::mem::take(&mut ws) } else { Vec::new() },
            };
            let _ = channel.send(batch);
            if last {
                break;
            }
            flows = rest;
        }
    }
}

impl FlowSink for UiStream {
    fn emit(&self, flow: Flow) {
        let mut st = self.state.lock().unwrap();
        // Nothing listening yet: nothing to queue for.
        if st.channel.is_none() {
            return;
        }
        st.last_seq = st.last_seq.max(flow.seq);
        if let Some(p) = st.paused.as_mut() {
            if flow.seq <= p.through_seq && p.seen.insert(flow.id.clone()) {
                p.changed.push(flow.id);
            }
            return;
        }
        st.queue(without_bytes(flow));
    }
}

impl WsSink for UiStream {
    fn emit(&self, msg: WsMessage) {
        let mut st = self.state.lock().unwrap();
        if st.channel.is_some() {
            st.ws.push(msg);
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
    /// Flow snapshots and WebSocket frames, batched toward the UI.
    pub stream: Arc<UiStream>,
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
            stream: Arc::new(UiStream::default()),
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
        self.data_dir.join(SYSPROXY_BACKUP_FILE)
    }

    pub fn mcp_path(&self) -> PathBuf {
        self.data_dir.join("mcp.json")
    }
}

#[cfg(test)]
mod stream_tests {
    use super::*;
    use nova_core::bodystore::BodyStore;
    use nova_proto::{BodyPreview, FlowState, WsDirection, WsOpcode};
    use tauri::ipc::InvokeResponseBody;

    fn flow(id: &str, seq: u64) -> Flow {
        nova_core::flow::new_flow(
            id.into(),
            seq,
            "GET".into(),
            "https".into(),
            "api.example.com".into(),
            "/v1".into(),
            "https://api.example.com/v1".into(),
            "127.0.0.1:1".into(),
            "HTTP/1.1".into(),
            vec![],
        )
    }

    fn with_body(mut f: Flow) -> Flow {
        f.response_body = Some(BodyPreview {
            size: 5,
            truncated: false,
            media_type: Some("application/json".into()),
            decoded_from: None,
            text: Some("{\"a\"}".into()),
            base64: Some("eyJhIn0=".into()),
            spilled: false,
        });
        f
    }

    fn frame(flow_id: &str) -> WsMessage {
        WsMessage {
            flow_id: flow_id.into(),
            seq: 0,
            direction: WsDirection::Received,
            opcode: WsOpcode::Text,
            size: 2,
            truncated: false,
            text: Some("hi".into()),
            base64: None,
            at: 0.0,
        }
    }

    /// A stream bound to a channel that records every message it is sent.
    fn bound() -> (UiStream, Arc<Mutex<Vec<serde_json::Value>>>) {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let sink = sent.clone();
        let stream = UiStream::default();
        stream.bind(Channel::new(move |body| {
            if let InvokeResponseBody::Json(json) = body {
                sink.lock().unwrap().push(serde_json::from_str(&json).unwrap());
            }
            Ok(())
        }));
        (stream, sent)
    }

    fn ids(msg: &serde_json::Value) -> Vec<String> {
        msg["flows"].as_array().unwrap().iter().map(|f| f["id"].as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn nothing_is_queued_before_the_ui_subscribes() {
        let stream = UiStream::default();
        FlowSink::emit(&stream, flow("a", 1));
        WsSink::emit(&stream, frame("a"));
        let st = stream.state.lock().unwrap();
        assert!(st.flows.is_empty() && st.ws.is_empty());
    }

    #[test]
    fn snapshots_go_out_without_body_bytes_but_with_their_metadata() {
        let (stream, sent) = bound();
        FlowSink::emit(&stream, with_body(flow("a", 1)));
        stream.flush();
        let sent = sent.lock().unwrap();
        let body = &sent[0]["flows"][0]["response_body"];
        assert!(body["text"].is_null() && body["base64"].is_null());
        assert_eq!(body["size"], 5);
        assert_eq!(body["media_type"], "application/json");
    }

    #[test]
    fn a_flow_goes_out_once_per_tick_as_it_last_was_in_first_seen_order() {
        let (stream, sent) = bound();
        FlowSink::emit(&stream, flow("a", 1));
        FlowSink::emit(&stream, flow("b", 2));
        let mut done = flow("a", 1);
        done.state = FlowState::Completed;
        FlowSink::emit(&stream, done);
        stream.flush();
        stream.flush(); // an empty tick sends nothing
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(ids(&sent[0]), ["a", "b"]);
        assert_eq!(sent[0]["flows"][0]["state"], "Completed");
    }

    #[test]
    fn a_backlog_is_split_and_frames_ride_with_the_last_part() {
        let (stream, sent) = bound();
        for i in 0..(MAX_FLOWS_PER_MESSAGE as u64 + 3) {
            FlowSink::emit(&stream, flow(&format!("f{i}"), i + 1));
        }
        WsSink::emit(&stream, frame("f0"));
        stream.flush();
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(ids(&sent[0]).len(), MAX_FLOWS_PER_MESSAGE);
        assert!(sent[0]["ws"].as_array().unwrap().is_empty());
        assert_eq!(ids(&sent[1]).len(), 3);
        assert_eq!(sent[1]["ws"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn resume_sends_what_changed_on_existing_flows_and_skips_ones_born_while_paused() {
        let store = FlowStore::new(Arc::new(BodyStore::memory_only(1024)), 100);
        let (stream, sent) = bound();
        for (id, seq) in [("old", 1), ("idle", 2)] {
            store.insert(flow(id, seq));
            FlowSink::emit(&stream, flow(id, seq));
        }
        stream.flush();

        stream.pause();
        let done = store.update("old", |f| f.state = FlowState::Completed).unwrap();
        FlowSink::emit(&stream, done);
        store.insert(flow("new", 3));
        FlowSink::emit(&stream, flow("new", 3));
        stream.flush();
        assert_eq!(sent.lock().unwrap().len(), 1, "nothing goes out while paused");

        stream.resume(&store);
        stream.flush();
        let sent = sent.lock().unwrap();
        assert_eq!(ids(&sent[1]), ["old"]);
        assert_eq!(sent[1]["flows"][0]["state"], "Completed");
    }

    #[test]
    fn frames_keep_flowing_while_paused() {
        let (stream, sent) = bound();
        stream.pause();
        WsSink::emit(&stream, frame("a"));
        stream.flush();
        assert_eq!(sent.lock().unwrap()[0]["ws"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn clearing_drops_queued_snapshots() {
        let (stream, sent) = bound();
        FlowSink::emit(&stream, flow("a", 1));
        stream.discard_flows();
        stream.flush();
        assert!(sent.lock().unwrap().is_empty());
    }
}
