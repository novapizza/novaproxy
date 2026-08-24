//! Self-update: check a signed manifest, download the new bundle, install it.
//!
//! Driven from Rust rather than from the plugin's JavaScript API so the frontend
//! keeps a single door onto the backend — `api.ts` and `invoke`, like every
//! other capability — and so the updater's permissions never have to be exposed
//! to the webview.
//!
//! The endpoint and the public key are *not* in `tauri.conf.json`. They are
//! injected at release time (see `.github/workflows/release.yml`), which means a
//! development build has no updater configuration at all. That is reported as
//! [`UpdateStatus::configured`] = false instead of as an error, because "this
//! build cannot update itself" is a normal state, not a failure.

use std::sync::Mutex;

use nova_proto::{UpdateProgress, UpdateStatus};
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tauri_plugin_updater::{Update, UpdaterExt};

/// The update found by the last check, kept so that installing it does not have
/// to re-fetch and re-verify the manifest.
///
/// One slot, not a queue: a second check simply replaces what the first found,
/// which is what the user means by checking again.
#[derive(Default)]
pub struct PendingUpdate(Mutex<Option<Update>>);

fn current_version(app: &AppHandle) -> String {
    app.package_info().version.to_string()
}

/// Ask the endpoint whether a newer version exists.
///
/// A missing updater configuration is not an error — see the module docs. A
/// network or signature failure is, and is returned verbatim so the UI can show
/// what actually went wrong.
#[tauri::command]
pub async fn check_update(
    app: AppHandle,
    pending: State<'_, PendingUpdate>,
) -> Result<UpdateStatus, String> {
    let mut status = UpdateStatus {
        current_version: current_version(&app),
        ..Default::default()
    };

    // `updater()` fails only on configuration: no endpoints, or no public key to
    // verify a manifest against.
    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(e) => {
            tracing::info!("updater not configured in this build: {e}");
            return Ok(status);
        }
    };
    status.configured = true;

    match updater.check().await.map_err(|e| e.to_string())? {
        Some(update) => {
            status.available = true;
            status.version = Some(update.version.clone());
            status.notes = update.body.clone();
            status.date = update.date.map(|d| d.to_string());
            *pending.0.lock().unwrap() = Some(update);
        }
        None => {
            status.available = false;
            *pending.0.lock().unwrap() = None;
        }
    }
    Ok(status)
}

/// Download and install the update the last check found, then restart.
///
/// Progress is streamed on `channel` so a slow download looks like progress
/// rather than a hung window. The function does not return on success: the
/// installer replaces this build and the app relaunches into it.
#[tauri::command]
pub async fn install_update(
    app: AppHandle,
    pending: State<'_, PendingUpdate>,
    channel: Channel<UpdateProgress>,
) -> Result<(), String> {
    // Taken, not cloned: once the bytes are installed the handle describes a
    // version that is no longer newer than what is running.
    let update = pending
        .0
        .lock()
        .unwrap()
        .take()
        .ok_or("no update is pending — check for updates first")?;

    let mut downloaded = 0u64;
    update
        .download_and_install(
            |chunk, total| {
                downloaded += chunk as u64;
                // A dropped channel means the window went away mid-download.
                // Not worth aborting the install over, so the send is ignored.
                let _ = channel.send(UpdateProgress {
                    downloaded,
                    total,
                    done: false,
                });
            },
            || {
                let _ = channel.send(UpdateProgress {
                    downloaded: 0,
                    total: None,
                    done: true,
                });
            },
        )
        .await
        .map_err(|e| e.to_string())?;

    // How the process leaves is platform-specific, because what "installed"
    // means is:
    //
    // macOS — the plugin has already replaced the .app in place, so the new
    // build is on disk and `restart` exec's straight into it.
    //
    // Windows — NSIS cannot overwrite an executable that is still running, and
    // the installer we just launched is waiting for exactly that. Restarting
    // would race the swap and surface as "file in use" or a half-installed app,
    // so this process exits and lets the installer finish and relaunch.
    #[cfg(target_os = "windows")]
    {
        tracing::info!("installer launched; exiting so it can replace this build");
        app.exit(0);
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        tracing::info!("update installed; restarting");
        app.restart();
    }
}
