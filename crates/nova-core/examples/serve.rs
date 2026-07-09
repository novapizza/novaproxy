//! Headless engine runner for manual / scripted verification:
//!   cargo run -p nova-core --example serve
//! then route a client through 127.0.0.1:39190 trusting the printed ca.pem.

use std::sync::{Arc, RwLock};

use nova_core::{ca::CaMaterial, EngineConfig, FlowSink};
use nova_proto::{Flow, FlowState};

struct Printer;
impl FlowSink for Printer {
    fn emit(&self, f: Flow) {
        if matches!(f.state, FlowState::Completed | FlowState::Error) {
            println!(
                "FLOW {} {} -> {:?} ({} resp bytes) [app {}]{}{}{}",
                f.method,
                f.url,
                f.status,
                f.response_size,
                f.process
                    .as_deref()
                    .map(|p| format!("{p}:{}", f.pid.unwrap_or(0)))
                    .unwrap_or_else(|| "?".into()),
                if f.resent { " [resent]" } else { "" },
                f.mapped_from
                    .map(|h| format!(" [mapped-from {h}]"))
                    .unwrap_or_default(),
                f.error.map(|e| format!(" ERR:{e}")).unwrap_or_default(),
            );
        }
    }
}

#[tokio::main]
async fn main() {
    let dir = std::env::temp_dir().join("novaproxy-example-ca");
    let ca = CaMaterial::load_or_create(&dir).unwrap();
    println!("CA_PATH {}", ca.cert_path.display());

    // Optional rules for headless testing: NOVA_RULES=/path/to/rules.json
    let rules = match std::env::var("NOVA_RULES").ok() {
        Some(path) => {
            let text = std::fs::read_to_string(&path).expect("read NOVA_RULES");
            serde_json::from_str(&text).expect("parse NOVA_RULES")
        }
        None => Vec::new(),
    };
    let rules = Arc::new(RwLock::new(rules));

    // Optional script for headless testing: NOVA_SCRIPT=/path/to/tamper.js
    let scripts = nova_core::scripting::ScriptEngine::new();
    if let Ok(path) = std::env::var("NOVA_SCRIPT") {
        let src = std::fs::read_to_string(&path).expect("read NOVA_SCRIPT");
        scripts.set_script(src);
        scripts.set_enabled(true);
    }

    // Optional network conditions for testing: NOVA_LATENCY=ms NOVA_DOWN_KBPS=kbps
    let latency_ms = std::env::var("NOVA_LATENCY").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let down_kbps = std::env::var("NOVA_DOWN_KBPS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let net = Arc::new(RwLock::new(nova_proto::NetworkConditions {
        enabled: latency_ms > 0 || down_kbps > 0,
        latency_ms,
        down_kbps,
    }));

    // Optional TLS passthrough for testing: NOVA_TLS_EXCLUDE=host1,*.host2
    let tls_scope = Arc::new(RwLock::new(nova_proto::TlsScope {
        intercept_all: true,
        include: Vec::new(),
        exclude: std::env::var("NOVA_TLS_EXCLUDE")
            .ok()
            .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default(),
    }));

    let handle = nova_core::start(
        EngineConfig {
            addr: ([127, 0, 0, 1], 39_190).into(),
            body_cap: nova_core::DEFAULT_BODY_CAP,
        },
        &ca,
        Arc::new(Printer),
        Arc::new(nova_core::NoopWsSink),
        nova_core::EngineHooks {
            rules,
            breakpoints: Arc::new(nova_core::breakpoint::Breakpoints::new(Arc::new(
                nova_core::breakpoint::NoopBreakpointSink,
            ))),
            scripts,
            net,
            tls_scope,
        },
    )
    .unwrap();
    println!("LISTENING {}", handle.addr);

    tokio::signal::ctrl_c().await.ok();
}
