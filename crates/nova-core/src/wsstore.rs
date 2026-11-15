//! WebSocket frames, kept by the engine rather than the webview.
//!
//! Frames used to be streamed to the UI with their payload (up to
//! [`crate::WS_FRAME_CAP`] each) and held there, so a busy socket filled the
//! webview's memory; the UI's byte budget only capped the damage. Now the
//! payloads live here, under one byte budget across every socket, and the UI
//! receives a short preview per frame — the full payload is fetched for the
//! frame someone opens, the way bodies are.
//!
//! Holding them here also means frames keep being captured while the UI is
//! paused: the panel backfills from this store when a socket is opened.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use nova_proto::WsMessage;

/// Payload held across every socket, at most; past it the oldest frames go,
/// whichever socket they belong to.
pub const DEFAULT_WS_BUDGET: usize = 64 * 1024 * 1024;
/// Frames held per socket, at most.
pub const DEFAULT_WS_FRAMES_PER_SOCKET: usize = 10_000;
/// Characters of text a preview frame carries.
pub const PREVIEW_CHARS: usize = 200;

#[derive(Default)]
struct Inner {
    by_flow: HashMap<String, VecDeque<WsMessage>>,
    /// Arrival order across sockets, for evicting the oldest first.
    order: VecDeque<(String, u64)>,
    bytes: usize,
    dropped: HashMap<String, u64>,
}

pub struct WsFrameStore {
    inner: Mutex<Inner>,
    budget: usize,
    per_socket: usize,
}

impl Default for WsFrameStore {
    fn default() -> Self {
        Self::new(DEFAULT_WS_BUDGET, DEFAULT_WS_FRAMES_PER_SOCKET)
    }
}

/// Payload bytes a frame holds.
pub fn frame_bytes(m: &WsMessage) -> usize {
    m.text.as_ref().map_or(0, String::len) + m.base64.as_ref().map_or(0, String::len)
}

/// The frame as the list shows it: text cut to [`PREVIEW_CHARS`], binary
/// payload left out (its size and opcode say what it is). `truncated` is set
/// when anything was left out, which is what tells the UI to fetch the rest.
pub fn preview(m: &WsMessage) -> WsMessage {
    let mut p = m.clone();
    if let Some(t) = &m.text {
        if t.chars().count() > PREVIEW_CHARS {
            p.text = Some(t.chars().take(PREVIEW_CHARS).collect());
            p.truncated = true;
        }
    }
    if m.base64.is_some() {
        p.base64 = None;
        p.truncated = true;
    }
    p
}

impl WsFrameStore {
    pub fn new(budget: usize, per_socket: usize) -> Self {
        Self { inner: Mutex::new(Inner::default()), budget, per_socket: per_socket.max(1) }
    }

    /// Keep a frame, evicting the oldest ones (of this socket first, if it is
    /// over its own cap; then across sockets) until both limits hold.
    pub fn push(&self, m: WsMessage) {
        let mut st = self.inner.lock().unwrap();
        st.bytes += frame_bytes(&m);
        st.order.push_back((m.flow_id.clone(), m.seq));
        let id = m.flow_id.clone();
        let q = st.by_flow.entry(id.clone()).or_default();
        q.push_back(m);
        if q.len() > self.per_socket {
            let old = q.pop_front().expect("over cap");
            st.bytes -= frame_bytes(&old);
            *st.dropped.entry(id).or_default() += 1;
        }
        while st.bytes > self.budget {
            let Some((flow, seq)) = st.order.pop_front() else { break };
            let Some(q) = st.by_flow.get_mut(&flow) else { continue };
            // `order` can name frames already evicted by the per-socket cap.
            if let Some(old) = q.pop_front_if(|f| f.seq == seq) {
                let empty = q.is_empty();
                st.bytes -= frame_bytes(&old);
                *st.dropped.entry(flow.clone()).or_default() += 1;
                if empty {
                    st.by_flow.remove(&flow);
                }
            }
        }
        // Stale entries pile up behind the per-socket cap; trim them now and
        // then so `order` stays proportional to what is held.
        if st.order.len() > 2 * self.per_socket.max(1024) && st.order.len() > 4 * frames_held(&st) {
            let Inner { order, by_flow, .. } = &mut *st;
            order.retain(|(flow, seq)| by_flow.get(flow).is_some_and(|q| q.front().is_some_and(|f| f.seq <= *seq)));
        }
    }

    /// Previews of the frames of one socket, oldest first, with how many of
    /// its frames were evicted.
    pub fn previews(&self, flow_id: &str) -> (Vec<WsMessage>, u64) {
        let st = self.inner.lock().unwrap();
        let frames = st.by_flow.get(flow_id).map(|q| q.iter().map(preview).collect()).unwrap_or_default();
        (frames, st.dropped.get(flow_id).copied().unwrap_or(0))
    }

    /// One frame in full, if it is still held.
    pub fn frame(&self, flow_id: &str, seq: u64) -> Option<WsMessage> {
        let st = self.inner.lock().unwrap();
        st.by_flow.get(flow_id)?.iter().find(|f| f.seq == seq).cloned()
    }

    pub fn clear(&self) {
        *self.inner.lock().unwrap() = Inner::default();
    }

    /// Payload bytes held, for the perf line.
    pub fn bytes(&self) -> usize {
        self.inner.lock().unwrap().bytes
    }
}

fn frames_held(st: &Inner) -> usize {
    st.by_flow.values().map(VecDeque::len).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nova_proto::{WsDirection, WsOpcode};

    fn frame(flow: &str, seq: u64, text: &str) -> WsMessage {
        WsMessage {
            flow_id: flow.into(),
            seq,
            direction: WsDirection::Received,
            opcode: WsOpcode::Text,
            size: text.len() as u64,
            truncated: false,
            text: Some(text.into()),
            base64: None,
            at: 0.0,
        }
    }

    #[test]
    fn previews_cut_long_text_and_leave_binary_out() {
        let long = "x".repeat(PREVIEW_CHARS + 5);
        let p = preview(&frame("f", 0, &long));
        assert_eq!(p.text.as_ref().map(|t| t.len()), Some(PREVIEW_CHARS));
        assert!(p.truncated);
        let short = preview(&frame("f", 1, "hi"));
        assert!(!short.truncated && short.text.as_deref() == Some("hi"));
        let mut bin = frame("f", 2, "");
        bin.text = None;
        bin.opcode = WsOpcode::Binary;
        bin.base64 = Some("AAEC".into());
        let p = preview(&bin);
        assert!(p.base64.is_none() && p.truncated);
    }

    #[test]
    fn the_budget_evicts_the_oldest_frames_across_sockets() {
        let s = WsFrameStore::new(10, 100);
        s.push(frame("a", 0, "aaaa"));
        s.push(frame("b", 0, "bbbb"));
        s.push(frame("a", 1, "cccc"));
        assert_eq!(s.bytes(), 8);
        let (a, dropped_a) = s.previews("a");
        assert_eq!((a.len(), dropped_a), (1, 1), "a's first frame was the oldest");
        assert_eq!(s.previews("b").0.len(), 1);
        assert_eq!(s.frame("a", 1).and_then(|f| f.text).as_deref(), Some("cccc"));
        assert!(s.frame("a", 0).is_none());
    }

    #[test]
    fn a_socket_over_its_own_cap_loses_its_own_oldest() {
        let s = WsFrameStore::new(1_000, 2);
        for i in 0..5 {
            s.push(frame("a", i, "x"));
        }
        s.push(frame("b", 0, "y"));
        let (a, dropped) = s.previews("a");
        assert_eq!(a.iter().map(|f| f.seq).collect::<Vec<_>>(), vec![3, 4]);
        assert_eq!(dropped, 3);
        assert_eq!(s.bytes(), 3);
        s.clear();
        assert_eq!((s.bytes(), s.previews("a").0.len()), (0, 0));
    }
}
