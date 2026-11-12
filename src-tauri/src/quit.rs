//! Quitting with the system proxy on.
//!
//! Restoring the OS proxy takes about a second on macOS (a `networksetup`
//! process per setting, run one after another), and it used to run inside
//! `RunEvent::Exit`, after the window was gone: a quick ⌘Q looked like a hung
//! app, and a quit right after a toggle could restore under a half-applied
//! enable. Now every way out — ⌘Q, the menu, closing the window — comes here
//! first: the exit is held, the window says what is happening, the restore
//! waits for any toggle in progress, and only then does the app exit.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::state::AppState;

/// Sent to the window while the restore runs, so it can show the dialog.
pub const QUIT_PROGRESS_EVENT: &str = "app://quit-progress";

const IDLE: u8 = 0;
const RELEASING: u8 = 1;
const DONE: u8 = 2;

static PHASE: AtomicU8 = AtomicU8::new(IDLE);

/// Whether an exit may go ahead now. `false` means the caller must hold it:
/// the restore has been started (or already is running) and will exit itself.
pub fn may_exit<R: Runtime>(app: &AppHandle<R>) -> bool {
    match PHASE.load(Ordering::SeqCst) {
        DONE => true,
        RELEASING => false,
        _ => !begin(app),
    }
}

/// Start the quit. Returns whether a restore was started; when it was not,
/// there is nothing to undo and the caller exits straight away.
fn begin<R: Runtime>(app: &AppHandle<R>) -> bool {
    let st: Arc<AppState> = (*app.state::<Arc<AppState>>()).clone();
    let toggling = st.sysproxy_lock.try_lock().is_err();
    let owed = st.sysproxy_backup_path().exists() && nova_core::sysproxy::restore_is_silent();
    if !toggling && !owed {
        PHASE.store(DONE, Ordering::SeqCst);
        return false;
    }
    if PHASE.compare_exchange(IDLE, RELEASING, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return true;
    }
    tracing::info!(toggling, owed, at_ms = crate::since_boot_ms(), "quit: turning the system proxy off first");
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    if let Err(e) = app.emit(QUIT_PROGRESS_EVENT, ()) {
        tracing::warn!("could not show the quit dialog: {e}");
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let t = std::time::Instant::now();
        {
            // A toggle in flight finishes first; its result is what we undo.
            let _guard = st.sysproxy_lock.blocking_lock();
            crate::release_system_proxy(&st, "quit");
        }
        tracing::info!(ms = t.elapsed().as_millis() as u64, "quit: done, exiting");
        PHASE.store(DONE, Ordering::SeqCst);
        app.exit(0);
    });
    true
}

/// The Quit menu item and ⌘Q.
pub fn request<R: Runtime>(app: &AppHandle<R>) {
    if may_exit(app) {
        app.exit(0);
    }
}
