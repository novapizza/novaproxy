//! Breakpoints: pause a matching request mid-flight until the user decides to
//! continue (optionally with edited headers) or abort. The proxy task for that
//! request `.await`s a oneshot while the connection is held open.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use nova_proto::{Header, Interception};
use tokio::sync::oneshot;

use crate::rules::glob_match;

/// The user's decision for a paused request.
pub enum Resume {
    /// Forward the request, replacing/adding the given headers first.
    Continue(Vec<Header>),
    /// Reject the request with a synthetic error response.
    Abort,
}

/// Notified when a request is paused, so the UI can raise the intercept modal.
pub trait BreakpointSink: Send + Sync + 'static {
    fn paused(&self, interception: Interception);
}

pub struct Breakpoints {
    armed: AtomicBool,
    pattern: RwLock<String>,
    sink: Arc<dyn BreakpointSink>,
    pending: Mutex<HashMap<String, oneshot::Sender<Resume>>>,
}

impl Breakpoints {
    pub fn new(sink: Arc<dyn BreakpointSink>) -> Self {
        Self {
            armed: AtomicBool::new(false),
            pattern: RwLock::new("*".to_string()),
            sink,
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn arm(&self, pattern: String) {
        *self.pattern.write().unwrap() = if pattern.trim().is_empty() {
            "*".to_string()
        } else {
            pattern
        };
        self.armed.store(true, Ordering::Relaxed);
    }

    pub fn disarm(&self) {
        self.armed.store(false, Ordering::Relaxed);
        // Release anything currently paused so no request is left hanging.
        let mut pending = self.pending.lock().unwrap();
        for (_, tx) in pending.drain() {
            let _ = tx.send(Resume::Continue(Vec::new()));
        }
    }

    pub fn is_armed(&self) -> bool {
        self.armed.load(Ordering::Relaxed)
    }

    pub fn should_break(&self, url: &str) -> bool {
        self.is_armed() && glob_match(&self.pattern.read().unwrap(), url)
    }

    /// Deliver the user's decision to a paused request.
    pub fn resume(&self, id: &str, resume: Resume) {
        if let Some(tx) = self.pending.lock().unwrap().remove(id) {
            let _ = tx.send(resume);
        }
    }

    /// Register the pause, notify the UI, and await the decision. Arming is
    /// one-shot: we disarm here so only the *next* request is caught.
    pub async fn wait(&self, interception: Interception) -> Resume {
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap()
            .insert(interception.id.clone(), tx);
        self.armed.store(false, Ordering::Relaxed);
        self.sink.paused(interception);
        rx.await.unwrap_or(Resume::Continue(Vec::new()))
    }
}

/// A no-op sink for tests / headless engine use without breakpoints.
pub struct NoopBreakpointSink;
impl BreakpointSink for NoopBreakpointSink {
    fn paused(&self, _interception: Interception) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn breakpoints() -> Arc<Breakpoints> {
        Arc::new(Breakpoints::new(Arc::new(NoopBreakpointSink)))
    }

    fn interception(id: &str) -> Interception {
        Interception {
            id: id.into(),
            method: "GET".into(),
            url: "http://example.com/a".into(),
            request_headers: Vec::new(),
        }
    }

    #[test]
    fn starts_disarmed() {
        let bp = breakpoints();
        assert!(!bp.is_armed());
        assert!(!bp.should_break("http://anything/"));
    }

    #[test]
    fn arm_sets_pattern_and_matches() {
        let bp = breakpoints();
        bp.arm("https://*.example.com/*".into());
        assert!(bp.is_armed());
        assert!(bp.should_break("https://api.example.com/v1"));
        assert!(!bp.should_break("https://other.com/v1"));
    }

    #[test]
    fn empty_pattern_defaults_to_wildcard() {
        let bp = breakpoints();
        bp.arm("   ".into());
        assert!(bp.should_break("http://whatever/x"));
    }

    #[test]
    fn disarm_stops_matching() {
        let bp = breakpoints();
        bp.arm("*".into());
        bp.disarm();
        assert!(!bp.is_armed());
        assert!(!bp.should_break("http://x/"));
    }

    #[tokio::test]
    async fn resume_continue_delivers_edited_headers() {
        let bp = breakpoints();
        bp.arm("*".into());
        let waiter = bp.clone();
        let handle = tokio::spawn(async move { waiter.wait(interception("abc")).await });

        // Let wait() register the pending oneshot before resuming.
        tokio::time::sleep(Duration::from_millis(20)).await;
        bp.resume(
            "abc",
            Resume::Continue(vec![Header { name: "x-edit".into(), value: "1".into() }]),
        );

        match handle.await.unwrap() {
            Resume::Continue(headers) => {
                assert_eq!(headers.len(), 1);
                assert_eq!(headers[0].name, "x-edit");
            }
            Resume::Abort => panic!("expected Continue"),
        }
        // wait() is one-shot: arming is cleared once a request is caught.
        assert!(!bp.is_armed());
    }

    #[tokio::test]
    async fn resume_abort_is_delivered() {
        let bp = breakpoints();
        bp.arm("*".into());
        let waiter = bp.clone();
        let handle = tokio::spawn(async move { waiter.wait(interception("xyz")).await });

        tokio::time::sleep(Duration::from_millis(20)).await;
        bp.resume("xyz", Resume::Abort);

        assert!(matches!(handle.await.unwrap(), Resume::Abort));
    }

    #[tokio::test]
    async fn disarm_releases_paused_requests() {
        let bp = breakpoints();
        bp.arm("*".into());
        let waiter = bp.clone();
        let handle = tokio::spawn(async move { waiter.wait(interception("held")).await });

        tokio::time::sleep(Duration::from_millis(20)).await;
        // Disarming must not leave the paused request hanging forever.
        bp.disarm();

        assert!(matches!(handle.await.unwrap(), Resume::Continue(_)));
    }

    #[tokio::test]
    async fn resume_unknown_id_is_a_noop() {
        let bp = breakpoints();
        // No pending entry for this id; must not panic.
        bp.resume("nope", Resume::Abort);
    }
}
