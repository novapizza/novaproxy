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
