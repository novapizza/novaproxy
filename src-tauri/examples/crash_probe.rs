//! Crash on purpose, to prove the handler catches it.
//!
//! `kill -SEGV` cannot verify this: a BSD signal is not a Mach exception, and
//! the exception port is the whole reason `crash-handler` is here. Only a real
//! hardware fault exercises the path the app relies on.
//!
//! Run it against a throwaway home, so the marker it leaves does not make the
//! next real launch report a crash that never happened:
//!
//! ```text
//! HOME=$(mktemp -d) cargo run -p novaproxy --example crash_probe
//! # then: cat "$HOME/Library/Logs/NovaProxy/crash.marker"
//! ```
//!
//! Expected: the process dies (that is the point) and the marker holds
//! `kind=1 code=1` — EXC_BAD_ACCESS / KERN_INVALID_ADDRESS.

fn main() {
    let marker = novaproxy_lib::crash::marker_path();
    // The handler writes into the log directory, which normally exists because
    // `logging::init` made it. This probe does not start logging.
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
    let _ = std::fs::remove_file(&marker);

    let guard = novaproxy_lib::crash::install();
    assert!(guard.is_some(), "the handler did not attach");
    println!("attached; marker will be {}", marker.display());

    // Volatile so it survives optimisation: a plain null deref is UB the
    // compiler is free to delete, which would make this probe pass by doing
    // nothing at all.
    let fault: *const u8 = std::ptr::null();
    let value = unsafe { std::ptr::read_volatile(fault) };
    println!("still alive, read {value} — the handler did NOT fire");
}
