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

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use nova_proto::Flow;

use crate::bodystore::BodyStore;

/// Bounded, insertion-ordered store of captured flows.
pub struct FlowStore {
    flows: Mutex<HashMap<String, Flow>>,
    /// Ids in insertion order, oldest at the front. Drives eviction and gives
    /// callers a stable newest-first read order.
    order: Mutex<VecDeque<String>>,
    max_flows: usize,
    bodies: Arc<BodyStore>,
    /// Flows captured this session, including ones since evicted.
    total_captured: AtomicU64,
}

impl FlowStore {
    pub fn new(bodies: Arc<BodyStore>, max_flows: usize) -> Self {
        Self {
            flows: Mutex::new(HashMap::new()),
            order: Mutex::new(VecDeque::new()),
            max_flows: max_flows.max(1),
            bodies,
            total_captured: AtomicU64::new(0),
        }
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
            // Only a first insert extends the order queue; re-inserting an id
            // (never expected, but cheap to be safe about) must not double-count
            // it and shrink the effective window.
            if map.insert(id.clone(), flow).is_none() {
                order.push_back(id);
            }
            let mut evicted = Vec::new();
            while order.len() > self.max_flows {
                let Some(old) = order.pop_front() else { break };
                map.remove(&old);
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
    pub fn update<F: FnOnce(&mut Flow)>(&self, id: &str, f: F) -> Option<Flow> {
        let mut map = self.flows.lock().unwrap();
        let flow = map.get_mut(id)?;
        f(flow);
        Some(flow.clone())
    }

    pub fn get(&self, id: &str) -> Option<Flow> {
        self.flows.lock().unwrap().get(id).cloned()
    }

    /// Retained flows, newest first.
    pub fn newest_first(&self) -> Vec<Flow> {
        let map = self.flows.lock().unwrap();
        let order = self.order.lock().unwrap();
        order.iter().rev().filter_map(|id| map.get(id).cloned()).collect()
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
            if let Some(flow) = map.get(id) {
                if pred(flow) {
                    out.push(flow.clone());
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

    #[test]
    fn max_flows_is_never_zero() {
        // A zero window would evict every flow the instant it arrived.
        assert_eq!(store(0).max_flows(), 1);
    }
}
