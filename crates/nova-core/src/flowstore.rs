//! The retained flow store.
//!
//! Extracted from the engine's `Shared` so that it can be **owned by the app**
//! rather than by a running proxy: the MCP server and the Tauri commands need to
//! read captured traffic whether or not the engine is currently running, and
//! stopping/starting the proxy must not drop what was captured.
//!
//! Retention lives here too: the window bounds how many flows are kept, and
//! evicting a flow also deletes its spilled bodies (see [`BodyStore`]) — without
//! that, disk keeps growing for flows nothing can reach any more.
//!
//! Body previews are held compressed. A retained flow is read far less often
//! than it is kept — the list streams to the UI without bodies, and the bytes
//! are only wanted when someone opens the flow, exports, or an MCP tool asks —
//! so the text and base64 of each preview sit here as zstd frames and are
//! inflated on the way out. Everything that reads the store gets plain flows
//! back; nothing outside this module sees the packed form.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use nova_proto::{BodyPreview, Flow};

use crate::bodystore::BodyStore;

/// Bounded, insertion-ordered store of captured flows.
pub struct FlowStore {
    flows: Mutex<HashMap<String, Stored>>,
    /// Ids in insertion order, oldest at the front. Drives eviction and gives
    /// callers a stable newest-first read order.
    order: Mutex<VecDeque<String>>,
    max_flows: usize,
    bodies: Arc<BodyStore>,
    /// Flows captured this session, including ones since evicted.
    total_captured: AtomicU64,
    /// Preview bytes held, as stored and as they would be inflated. Changed
    /// only under the `flows` lock, so the pair moves together.
    held_bytes: AtomicI64,
    raw_bytes: AtomicI64,
}

impl FlowStore {
    pub fn new(bodies: Arc<BodyStore>, max_flows: usize) -> Self {
        Self {
            flows: Mutex::new(HashMap::new()),
            order: Mutex::new(VecDeque::new()),
            max_flows: max_flows.max(1),
            bodies,
            total_captured: AtomicU64::new(0),
            held_bytes: AtomicI64::new(0),
            raw_bytes: AtomicI64::new(0),
        }
    }

    /// Account for a stored flow arriving (`sign` 1) or leaving (`sign` -1).
    fn count(&self, stored: &Stored, sign: i64) {
        let (held, raw) = stored.sizes();
        self.held_bytes.fetch_add(sign * held as i64, Ordering::Relaxed);
        self.raw_bytes.fetch_add(sign * raw as i64, Ordering::Relaxed);
    }

    /// Bytes of each body retained inline (and therefore streamed to clients).
    pub fn body_cap(&self) -> usize {
        self.bodies.inline_cap()
    }

    pub fn bodies(&self) -> &Arc<BodyStore> {
        &self.bodies
    }

    /// Store a freshly-seen flow, evicting the oldest flows (and their spilled
    /// bodies) once the retention window is full.
    pub fn insert(&self, flow: Flow) {
        self.total_captured.fetch_add(1, Ordering::Relaxed);
        let evicted = {
            let mut map = self.flows.lock().unwrap();
            let mut order = self.order.lock().unwrap();
            let id = flow.id.clone();
            let stored = Stored::pack(flow, None);
            self.count(&stored, 1);
            // Only a first insert extends the order queue; re-inserting an id
            // (never expected, but cheap to be safe about) must not double-count
            // it and shrink the effective window.
            match map.insert(id.clone(), stored) {
                None => order.push_back(id),
                Some(old) => self.count(&old, -1),
            }
            let mut evicted = Vec::new();
            while order.len() > self.max_flows {
                let Some(old) = order.pop_front() else { break };
                if let Some(gone) = map.remove(&old) {
                    self.count(&gone, -1);
                }
                evicted.push(old);
            }
            evicted
        };
        for id in &evicted {
            self.bodies.remove_flow(id);
        }
    }

    /// Apply `f` to a stored flow and return the updated snapshot. `None` when
    /// the flow is unknown (already evicted, or never recorded).
    ///
    /// `f` sees the flow with its bodies inflated, like any other reader. A
    /// preview it leaves alone keeps its existing frame rather than being
    /// compressed again — most updates touch status and timings, not bodies.
    pub fn update<F: FnOnce(&mut Flow)>(&self, id: &str, f: F) -> Option<Flow> {
        let mut map = self.flows.lock().unwrap();
        let stored = map.get_mut(id)?;
        let mut flow = stored.unpack();
        let before = Bodies::copy(&flow);
        f(&mut flow);
        let snapshot = flow.clone();
        self.count(stored, -1);
        let next = Stored::pack(flow, Some((&*stored, before)));
        *stored = next;
        self.count(stored, 1);
        Some(snapshot)
    }

    pub fn get(&self, id: &str) -> Option<Flow> {
        self.flows.lock().unwrap().get(id).map(Stored::unpack)
    }

    /// Retained flows, newest first.
    pub fn newest_first(&self) -> Vec<Flow> {
        let map = self.flows.lock().unwrap();
        let order = self.order.lock().unwrap();
        order.iter().rev().filter_map(|id| map.get(id).map(Stored::unpack)).collect()
    }

    /// Newest-first flows matching `pred`, at most `limit` of them. Evaluated
    /// while walking backwards, so a small `limit` does not clone the whole
    /// window.
    pub fn find<P: Fn(&Flow) -> bool>(&self, pred: P, limit: usize) -> Vec<Flow> {
        let map = self.flows.lock().unwrap();
        let order = self.order.lock().unwrap();
        let mut out = Vec::new();
        for id in order.iter().rev() {
            if out.len() >= limit {
                break;
            }
            if let Some(stored) = map.get(id) {
                let flow = stored.unpack();
                if pred(&flow) {
                    out.push(flow);
                }
            }
        }
        out
    }

    /// Drop every retained flow and its spilled bodies (the UI's Clear action).
    /// The session capture counter is left alone — it counts what happened, not
    /// what is still held.
    pub fn clear(&self) {
        let ids = {
            let mut map = self.flows.lock().unwrap();
            let mut order = self.order.lock().unwrap();
            order.clear();
            self.held_bytes.store(0, Ordering::Relaxed);
            self.raw_bytes.store(0, Ordering::Relaxed);
            map.drain().map(|(id, _)| id).collect::<Vec<_>>()
        };
        for id in &ids {
            self.bodies.remove_flow(id);
        }
    }

    pub fn retained(&self) -> usize {
        self.flows.lock().unwrap().len()
    }

    pub fn total_captured(&self) -> u64 {
        self.total_captured.load(Ordering::Relaxed)
    }

    pub fn max_flows(&self) -> usize {
        self.max_flows
    }

    /// Body preview bytes held in memory: `(as stored, inflated)`.
    pub fn preview_bytes(&self) -> (u64, u64) {
        let held = self.held_bytes.load(Ordering::Relaxed).max(0) as u64;
        let raw = self.raw_bytes.load(Ordering::Relaxed).max(0) as u64;
        (held, raw)
    }
}

/* ------------------------------ packed bodies ------------------------------ */

/// Shorter strings are kept as they are: a zstd frame has a fixed overhead,
/// and inflating it costs more than the bytes it saves.
const PACK_MIN: usize = 256;
/// Fastest level. The store compresses on the capture path, and level 1 already
/// takes JSON and HTML down to a fraction of their size.
const PACK_LEVEL: i32 = 1;

/// One preview string, compressed when that made it smaller.
#[derive(Clone)]
struct Packed {
    data: Box<[u8]>,
    zstd: bool,
    raw_len: usize,
}

impl Packed {
    fn new(s: String) -> Self {
        let raw = s.into_bytes();
        let raw_len = raw.len();
        if raw_len >= PACK_MIN {
            if let Ok(frame) = zstd::bulk::compress(&raw, PACK_LEVEL) {
                if frame.len() < raw_len {
                    return Packed { data: frame.into_boxed_slice(), zstd: true, raw_len };
                }
            }
        }
        Packed { data: raw.into_boxed_slice(), zstd: false, raw_len }
    }

    fn unpack(&self) -> String {
        let bytes = if self.zstd {
            // The frame was made here from `raw_len` bytes, so this cannot fail
            // short of memory corruption; an empty string is the safe answer.
            zstd::bulk::decompress(&self.data, self.raw_len).unwrap_or_default()
        } else {
            self.data.to_vec()
        };
        // The bytes came out of a `String`.
        String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
    }
}

/// The text and base64 of one body preview, packed.
#[derive(Clone, Default)]
struct PackedBody {
    text: Option<Packed>,
    base64: Option<Packed>,
}

/// A flow as the store holds it: previews stripped of their bytes, which sit
/// alongside in packed form.
struct Stored {
    flow: Flow,
    request: PackedBody,
    response: PackedBody,
}

/// Copies of the four preview strings a flow carries, taken before an update so
/// that the ones it did not change can keep their frames.
struct Bodies([Option<String>; 4]);

impl Bodies {
    fn copy(flow: &Flow) -> Self {
        let get = |b: &Option<BodyPreview>| {
            b.as_ref().map(|b| (b.text.clone(), b.base64.clone())).unwrap_or_default()
        };
        let (rq_text, rq_b64) = get(&flow.request_body);
        let (rs_text, rs_b64) = get(&flow.response_body);
        Bodies([rq_text, rq_b64, rs_text, rs_b64])
    }
}

impl Stored {
    /// Pack `flow`. With `prev` — the stored form it was unpacked from, and its
    /// strings as they were then — an unchanged string reuses its old frame.
    fn pack(mut flow: Flow, prev: Option<(&Stored, Bodies)>) -> Self {
        let (old, before) = match prev {
            Some((old, before)) => (Some(old), Some(before)),
            None => (None, None),
        };
        let reuse = |now: &Option<String>, then: Option<&Option<String>>, frame: Option<&Option<Packed>>| {
            match (then, frame) {
                (Some(then), Some(Some(frame))) if then == now => Some(frame.clone()),
                _ => None,
            }
        };
        let side = |body: &mut Option<BodyPreview>, at: usize, packed: Option<&PackedBody>| {
            let Some(body) = body.as_mut() else { return PackedBody::default() };
            let text = reuse(&body.text, before.as_ref().map(|b| &b.0[at]), packed.map(|p| &p.text))
                .or_else(|| body.text.clone().map(Packed::new));
            let base64 = reuse(&body.base64, before.as_ref().map(|b| &b.0[at + 1]), packed.map(|p| &p.base64))
                .or_else(|| body.base64.clone().map(Packed::new));
            body.text = None;
            body.base64 = None;
            PackedBody { text, base64 }
        };
        let request = side(&mut flow.request_body, 0, old.map(|o| &o.request));
        let response = side(&mut flow.response_body, 2, old.map(|o| &o.response));
        Stored { flow, request, response }
    }

    fn unpack(&self) -> Flow {
        let mut flow = self.flow.clone();
        for (body, packed) in [(&mut flow.request_body, &self.request), (&mut flow.response_body, &self.response)] {
            if let Some(body) = body.as_mut() {
                body.text = packed.text.as_ref().map(Packed::unpack);
                body.base64 = packed.base64.as_ref().map(Packed::unpack);
            }
        }
        flow
    }

    /// Preview bytes held, and what they inflate to.
    fn sizes(&self) -> (usize, usize) {
        [&self.request.text, &self.request.base64, &self.response.text, &self.response.base64]
            .into_iter()
            .flatten()
            .fold((0, 0), |(held, raw), p| (held + p.data.len(), raw + p.raw_len))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::new_flow;

    fn store(max_flows: usize) -> FlowStore {
        FlowStore::new(Arc::new(BodyStore::memory_only(1024)), max_flows)
    }

    fn flow(id: &str, host: &str) -> Flow {
        new_flow(
            id.into(),
            0,
            "GET".into(),
            "https".into(),
            host.into(),
            "/".into(),
            format!("https://{host}/"),
            "127.0.0.1:1".into(),
            "HTTP/1.1".into(),
            Vec::new(),
        )
    }

    #[test]
    fn insert_get_and_update_round_trip() {
        let s = store(10);
        s.insert(flow("f0", "example.com"));
        assert_eq!(s.get("f0").unwrap().host, "example.com");

        let updated = s.update("f0", |f| f.status = Some(200)).expect("updated");
        assert_eq!(updated.status, Some(200));
        assert_eq!(s.get("f0").unwrap().status, Some(200));
    }

    #[test]
    fn update_of_an_unknown_flow_reports_none() {
        let s = store(10);
        assert!(s.update("ghost", |f| f.status = Some(500)).is_none());
    }

    #[test]
    fn newest_first_is_reverse_insertion_order() {
        let s = store(10);
        for id in ["f0", "f1", "f2"] {
            s.insert(flow(id, "example.com"));
        }
        let ids: Vec<String> = s.newest_first().into_iter().map(|f| f.id).collect();
        assert_eq!(ids, vec!["f2", "f1", "f0"]);
    }

    #[test]
    fn retention_evicts_oldest_but_keeps_the_session_total() {
        let s = store(2);
        for id in ["f0", "f1", "f2"] {
            s.insert(flow(id, "example.com"));
        }
        assert_eq!(s.retained(), 2);
        assert!(s.get("f0").is_none(), "oldest evicted");
        assert!(s.get("f2").is_some());
        assert_eq!(s.total_captured(), 3, "the counter records what was captured");
    }

    #[test]
    fn find_returns_newest_matches_up_to_the_limit() {
        let s = store(10);
        s.insert(flow("f0", "a.com"));
        s.insert(flow("f1", "b.com"));
        s.insert(flow("f2", "a.com"));
        s.insert(flow("f3", "a.com"));

        let hits = s.find(|f| f.host == "a.com", 2);
        let ids: Vec<&str> = hits.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, vec!["f3", "f2"], "newest matches first, limited");
    }

    #[test]
    fn find_with_no_matches_is_empty() {
        let s = store(10);
        s.insert(flow("f0", "a.com"));
        assert!(s.find(|f| f.host == "nope.com", 10).is_empty());
    }

    #[test]
    fn clear_drops_everything_retained() {
        let s = store(10);
        s.insert(flow("f0", "a.com"));
        s.insert(flow("f1", "a.com"));
        s.clear();
        assert_eq!(s.retained(), 0);
        assert!(s.newest_first().is_empty());
        assert_eq!(s.total_captured(), 2, "clearing the view is not un-capturing");
    }

    #[test]
    fn eviction_deletes_spilled_bodies() {
        let dir = std::env::temp_dir().join(format!("novaproxy-flowstore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let bodies = Arc::new(BodyStore::new(dir, 4, 1024, 4096));
        let s = FlowStore::new(bodies.clone(), 1);

        // Spill a body for f0, then push it out of the window.
        let mut cap = crate::bodystore::BodyCapture::new(&bodies, "f0", crate::flow::Side::Response);
        cap.push(b"0123456789");
        cap.finish();
        assert!(bodies.read("f0", crate::flow::Side::Response, 1024).is_ok());

        s.insert(flow("f0", "a.com"));
        s.insert(flow("f1", "a.com"));
        assert!(
            bodies.read("f0", crate::flow::Side::Response, 1024).is_err(),
            "an evicted flow's spilled body must be deleted"
        );
    }

    fn with_bodies(id: &str, text: &str, base64: &str) -> Flow {
        let mut f = flow(id, "a.com");
        f.request_body = Some(BodyPreview { text: Some(text.into()), ..Default::default() });
        f.response_body = Some(BodyPreview { base64: Some(base64.into()), ..Default::default() });
        f
    }

    #[test]
    fn bodies_come_back_exactly_as_stored() {
        let s = store(10);
        let json = format!("{{\"items\":[{}]}}", vec!["{\"id\":1,\"name\":\"héllo\"}"; 200].join(","));
        let b64 = "QUJD".repeat(500);
        s.insert(with_bodies("f0", &json, &b64));

        let got = s.get("f0").unwrap();
        assert_eq!(got.request_body.unwrap().text.as_deref(), Some(json.as_str()));
        assert_eq!(got.response_body.unwrap().base64.as_deref(), Some(b64.as_str()));
        let (held, raw) = s.preview_bytes();
        assert_eq!(raw as usize, json.len() + b64.len());
        assert!(held * 5 < raw, "repetitive previews are held compressed: {held} of {raw}");
    }

    #[test]
    fn short_and_empty_previews_survive() {
        let s = store(10);
        s.insert(with_bodies("f0", "ok", ""));
        let got = s.get("f0").unwrap();
        assert_eq!(got.request_body.unwrap().text.as_deref(), Some("ok"));
        assert_eq!(got.response_body.unwrap().base64.as_deref(), Some(""));
    }

    #[test]
    fn update_sees_bodies_and_keeps_or_replaces_them() {
        let s = store(10);
        let text = "x".repeat(4096);
        s.insert(with_bodies("f0", &text, "QUJD"));

        // An update that leaves bodies alone still sees them, and returns them.
        let snap = s
            .update("f0", |f| {
                assert_eq!(f.request_body.as_ref().unwrap().text.as_deref(), Some(text.as_str()));
                f.status = Some(200);
            })
            .unwrap();
        assert_eq!(snap.request_body.unwrap().text.as_deref(), Some(text.as_str()));

        s.update("f0", |f| f.response_body.as_mut().unwrap().base64 = Some("REVG".into()));
        let got = s.get("f0").unwrap();
        assert_eq!(got.status, Some(200));
        assert_eq!(got.request_body.unwrap().text.as_deref(), Some(text.as_str()));
        assert_eq!(got.response_body.unwrap().base64.as_deref(), Some("REVG"));

        s.update("f0", |f| f.request_body = None);
        assert!(s.get("f0").unwrap().request_body.is_none());
        assert_eq!(s.preview_bytes().1, 4, "only the response preview is left");
    }

    #[test]
    fn find_matches_on_body_text() {
        let s = store(10);
        s.insert(with_bodies("f0", &format!("{}needle", "a".repeat(1000)), ""));
        s.insert(with_bodies("f1", "haystack", ""));
        let hits = s.find(
            |f| f.request_body.as_ref().and_then(|b| b.text.as_deref()).is_some_and(|t| t.contains("needle")),
            10,
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "f0");
    }

    #[test]
    fn eviction_and_clear_release_preview_bytes() {
        let s = store(1);
        s.insert(with_bodies("f0", &"a".repeat(1000), ""));
        s.insert(with_bodies("f1", "bb", ""));
        assert_eq!(s.preview_bytes().1, 2, "f0's preview went with it");
        s.clear();
        assert_eq!(s.preview_bytes(), (0, 0));
    }

    #[test]
    fn max_flows_is_never_zero() {
        // A zero window would evict every flow the instant it arrived.
        assert_eq!(store(0).max_flows(), 1);
    }
}
