//! JavaScript scripting sandbox (QuickJS via rquickjs).
//!
//! QuickJS's `Runtime`/`Context` are `!Send`, so the interpreter lives on its
//! own dedicated thread. The async proxy handler talks to it over channels:
//! it ships a [`ScriptFlow`] snapshot and awaits the (possibly mutated) result.
//!
//! User scripts define `onRequest(flow)` / `onResponse(flow)`. Each `flow` has
//! `method`, `host`, `path`, `url`, `status` (responses), a mutable `headers`
//! map, and `abort()`. Header edits are read back and applied; `abort()`
//! short-circuits the request.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use rquickjs::{Context, Ctx, Function, Object, Runtime};
use tokio::sync::{mpsc, oneshot};

#[derive(Clone, Copy)]
pub enum Hook {
    Request,
    Response,
}

/// The view of a flow handed to a script hook.
#[derive(Debug, Clone)]
pub struct ScriptFlow {
    pub method: String,
    pub host: String,
    pub path: String,
    pub url: String,
    pub status: Option<u16>,
    pub headers: Vec<(String, String)>,
}

/// What a hook produced: the full header set to apply, and whether to abort.
#[derive(Debug, Clone, Default)]
pub struct ScriptResult {
    pub headers: Vec<(String, String)>,
    pub abort: bool,
}

enum Job {
    SetScript(String),
    Run {
        hook: Hook,
        flow: ScriptFlow,
        reply: oneshot::Sender<Option<ScriptResult>>,
    },
}

#[derive(Default)]
struct Flags {
    ok: AtomicBool,
    has_request: AtomicBool,
    has_response: AtomicBool,
}

/// Handle to the scripting thread. Cheap to clone via `Arc`.
pub struct ScriptEngine {
    tx: mpsc::UnboundedSender<Job>,
    enabled: AtomicBool,
    flags: Arc<Flags>,
}

const PRELUDE: &str = r#"
var console = { log:function(){}, warn:function(){}, error:function(){}, info:function(){}, debug:function(){} };
var __nova_abort = function(){ this.__aborted = true; };
"#;

impl ScriptEngine {
    pub fn new() -> Arc<Self> {
        let (tx, mut rx) = mpsc::unbounded_channel::<Job>();
        let flags = Arc::new(Flags::default());
        let flags_thread = flags.clone();

        thread::Builder::new()
            .name("nova-scripts".into())
            .spawn(move || {
                let Ok(rt) = Runtime::new() else { return };
                let Ok(ctx) = Context::full(&rt) else { return };

                while let Some(job) = rx.blocking_recv() {
                    match job {
                        Job::SetScript(src) => ctx.with(|ctx| set_script(&ctx, &flags_thread, &src)),
                        Job::Run { hook, flow, reply } => {
                            let result = ctx.with(|ctx| run_hook(&ctx, hook, flow));
                            let _ = reply.send(result);
                        }
                    }
                }
            })
            .expect("spawn scripting thread");

        Arc::new(Self {
            tx,
            enabled: AtomicBool::new(false),
            flags,
        })
    }

    pub fn set_script(&self, source: String) {
        let _ = self.tx.send(Job::SetScript(source));
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed) && self.flags.ok.load(Ordering::Relaxed)
    }

    pub fn wants_request(&self) -> bool {
        self.is_enabled() && self.flags.has_request.load(Ordering::Relaxed)
    }

    pub fn wants_response(&self) -> bool {
        self.is_enabled() && self.flags.has_response.load(Ordering::Relaxed)
    }

    /// Run a hook. `None` means "leave the flow unchanged" (hook absent, script
    /// error, or thread gone) — callers must not treat it as "clear headers".
    pub async fn run(&self, hook: Hook, flow: ScriptFlow) -> Option<ScriptResult> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(Job::Run { hook, flow, reply }).is_err() {
            return None;
        }
        rx.await.ok().flatten()
    }
}

fn set_script(ctx: &Ctx<'_>, flags: &Flags, source: &str) {
    // Clear any previous hooks so a new script that drops one takes effect.
    let _ = ctx.eval::<(), _>("onRequest=undefined;onResponse=undefined;".to_string());
    let _ = ctx.eval::<(), _>(PRELUDE.to_string());
    // We evaluate as a plain script; strip ES module `export ` so top-level
    // `function onRequest` lands as a global.
    let cleaned = source.replace("export ", "");
    let ok = ctx.eval::<(), _>(cleaned).is_ok();
    flags.ok.store(ok, Ordering::Relaxed);

    let g = ctx.globals();
    flags.has_request.store(
        g.get::<_, Function>("onRequest").is_ok(),
        Ordering::Relaxed,
    );
    flags.has_response.store(
        g.get::<_, Function>("onResponse").is_ok(),
        Ordering::Relaxed,
    );
}

fn run_hook(ctx: &Ctx<'_>, hook: Hook, flow: ScriptFlow) -> Option<ScriptResult> {
    let g = ctx.globals();
    let name = match hook {
        Hook::Request => "onRequest",
        Hook::Response => "onResponse",
    };
    let func: Function = g.get(name).ok()?;

    let obj = Object::new(ctx.clone()).ok()?;
    obj.set("method", flow.method).ok()?;
    obj.set("host", flow.host).ok()?;
    obj.set("path", flow.path).ok()?;
    obj.set("url", flow.url).ok()?;
    if let Some(status) = flow.status {
        obj.set("status", status).ok()?;
    }

    let headers = Object::new(ctx.clone()).ok()?;
    for (k, v) in &flow.headers {
        let _ = headers.set(k.as_str(), v.as_str());
    }
    obj.set("headers", headers).ok()?;
    obj.set("__aborted", false).ok()?;
    if let Ok(abort) = g.get::<_, Function>("__nova_abort") {
        let _ = obj.set("abort", abort);
    }

    // A throwing hook shouldn't wedge traffic: on error, leave the flow as-is.
    if func.call::<_, ()>((obj.clone(),)).is_err() {
        return None;
    }

    let aborted: bool = obj.get("__aborted").unwrap_or(false);
    let headers_obj: Object = obj.get("headers").ok()?;
    let headers_out: Vec<(String, String)> = headers_obj
        .props::<String, String>()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_default();

    Some(ScriptResult {
        headers: headers_out,
        abort: aborted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flow(status: Option<u16>) -> ScriptFlow {
        ScriptFlow {
            method: "GET".into(),
            host: "example.com".into(),
            path: "/a".into(),
            url: "http://example.com/a".into(),
            status,
            headers: vec![("x-orig".into(), "keep".into())],
        }
    }

    #[tokio::test]
    async fn on_request_mutates_headers() {
        let engine = ScriptEngine::new();
        engine.set_script("export function onRequest(f){ f.headers['x-test'] = 'hi'; }".into());

        let result = engine.run(Hook::Request, flow(None)).await.expect("result");
        assert!(!result.abort);
        let map: std::collections::HashMap<_, _> = result.headers.into_iter().collect();
        assert_eq!(map.get("x-test").map(String::as_str), Some("hi"));
        // Original headers survive.
        assert_eq!(map.get("x-orig").map(String::as_str), Some("keep"));
    }

    #[tokio::test]
    async fn abort_is_reported() {
        let engine = ScriptEngine::new();
        engine.set_script("export function onRequest(f){ f.abort(); }".into());
        let result = engine.run(Hook::Request, flow(None)).await.expect("result");
        assert!(result.abort);
    }

    #[tokio::test]
    async fn missing_hook_returns_none() {
        let engine = ScriptEngine::new();
        // Only onRequest is defined; asking for the response hook yields None.
        engine.set_script("export function onRequest(f){}".into());
        assert!(engine.run(Hook::Response, flow(Some(200))).await.is_none());
    }

    #[tokio::test]
    async fn throwing_hook_leaves_flow_unchanged() {
        let engine = ScriptEngine::new();
        engine.set_script("export function onRequest(f){ throw new Error('boom'); }".into());
        // A throwing hook must not wedge traffic: it resolves to None.
        assert!(engine.run(Hook::Request, flow(None)).await.is_none());
    }

    #[tokio::test]
    async fn response_hook_sees_status() {
        let engine = ScriptEngine::new();
        engine.set_script(
            "export function onResponse(f){ f.headers['x-status'] = String(f.status); }".into(),
        );
        let result = engine.run(Hook::Response, flow(Some(404))).await.expect("result");
        let map: std::collections::HashMap<_, _> = result.headers.into_iter().collect();
        assert_eq!(map.get("x-status").map(String::as_str), Some("404"));
    }

    #[tokio::test]
    async fn flags_track_defined_hooks() {
        let engine = ScriptEngine::new();
        engine.set_script("export function onRequest(f){}".into());
        // A completed run() is FIFO-ordered after set_script, so the flags are
        // guaranteed settled by the time it returns.
        let _ = engine.run(Hook::Request, flow(None)).await;
        engine.set_enabled(true);

        assert!(engine.is_enabled());
        assert!(engine.wants_request());
        assert!(!engine.wants_response());

        engine.set_enabled(false);
        assert!(!engine.is_enabled());
        assert!(!engine.wants_request());
    }

    #[tokio::test]
    async fn syntax_error_disables_engine() {
        let engine = ScriptEngine::new();
        engine.set_script("this is not valid javascript ^^^".into());
        // Force the SetScript job to complete before reading flags.
        let _ = engine.run(Hook::Request, flow(None)).await;
        engine.set_enabled(true);
        // Even though enabled was requested, a failed eval keeps is_enabled false.
        assert!(!engine.is_enabled());
    }

    #[tokio::test]
    async fn replacing_a_script_drops_old_hooks() {
        let engine = ScriptEngine::new();
        engine.set_script("export function onRequest(f){ f.headers['a']='1'; }".into());
        let _ = engine.run(Hook::Request, flow(None)).await;

        // New script defines only onResponse; the old onRequest must be gone.
        engine.set_script("export function onResponse(f){}".into());
        assert!(engine.run(Hook::Request, flow(None)).await.is_none());
    }
}
