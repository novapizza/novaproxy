//! The native menu bar.
//!
//! Composed explicitly rather than taken from [`Menu::default`], so the same
//! structure is described in one place for every platform instead of being
//! assembled differently by `cfg` inside Tauri. What NovaProxy adds to it is two
//! items:
//!
//! - **Collecting logs**, here rather than in a settings panel because it is
//!   what a user needs when the window itself is misbehaving.
//! - **Checking for updates**, because that is where every desktop user looks
//!   for it first. The menu does not implement the check: it emits
//!   [`CHECK_UPDATES_EVENT`] and the Updates card in Settings answers, so a
//!   found version, a download and a failure all read the same whether the
//!   check came from the menu or from the card's own button.
//!
//! # How far "the same on every platform" actually goes
//!
//! Not all the way, and not for want of trying. Two things are decided by the
//! OS, not by us:
//!
//! - **macOS owns the first submenu.** The menu bar is global and application-
//!   wide, its first entry is the app menu titled with the app's name, and
//!   About / Services / Hide / Quit belong there by platform convention. It
//!   cannot be removed or renamed, so it exists only on macOS and the other
//!   platforms put About at the bottom of Help instead.
//! - **Most predefined items exist on one platform only.** `undo`, `redo`,
//!   `fullscreen` and `services` are macOS-only; `minimize`, `maximize`,
//!   `close_window`, `quit` and `hide` are unsupported on Linux/BSD. `muda`
//!   still *constructs* them everywhere — nothing fails, the backend just
//!   ignores them — which is worse than an error, because the menu ends up
//!   holding entries that look real and do nothing. So the items are always
//!   built and conditionally *added*; see [`PLATFORM_HAS_WINDOW_ITEMS`].
//!
//! What is identical everywhere: Edit's clipboard items and Help. Linux is left
//! with those two submenus, which is as much of this as Linux implements.
//!
//! # Two ids that are not decoration
//!
//! [`WINDOW_SUBMENU_ID`] and [`HELP_SUBMENU_ID`] are looked up by Tauri's own
//! `init_app_menu` on macOS and handed to NSApp via
//! `set_as_windows_menu_for_nsapp` / `set_as_help_menu_for_nsapp`. That is what
//! gives the Window menu its list of open windows and Help its search field.
//! Building those two submenus without those ids loses both, silently.
//!
//! # The Edit submenu is not cosmetic
//!
//! On macOS ⌘C/⌘V inside the webview are delivered *through the menu*, not by a
//! key handler. An app with no Edit submenu has no working clipboard in any text
//! field. On Windows WebView2 handles Ctrl+C itself, so there the same items are
//! a convention rather than a requirement.

use std::sync::Arc;

use tauri::menu::{
    AboutMetadata, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu, HELP_SUBMENU_ID,
    WINDOW_SUBMENU_ID,
};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::state::AppState;

/// Menu id of the log-collecting item. Matched in [`on_event`].
const SHOW_LOGS: &str = "help.show-logs";

/// Menu id of the update-checking item. Matched in [`on_event`].
const CHECK_UPDATES: &str = "app.check-updates";

/// Event the update item emits to the frontend, which owns the Updates card and
/// therefore every sentence a check can end in.
pub const CHECK_UPDATES_EVENT: &str = "menu://check-updates";

/// Whether this platform implements window controls and Quit as menu items.
///
/// False on Linux and the BSDs, where `muda`'s GTK backend has no equivalent —
/// so the File and Window submenus would be empty shells, and are dropped whole
/// rather than shown containing nothing.
const PLATFORM_HAS_WINDOW_ITEMS: bool = !cfg!(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
));

/// Whether this is the platform whose first submenu the OS dictates.
const PLATFORM_HAS_APP_MENU: bool = cfg!(target_os = "macos");

/// What the log item is called, which is the name of the file manager it opens.
fn show_logs_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Show Log in Finder"
    } else if cfg!(target_os = "windows") {
        "Show Log in Explorer"
    } else {
        "Show Log Folder"
    }
}

/// Compose the whole menu.
///
/// Every item is constructed unconditionally and added conditionally. The cost
/// is a handful of unused objects on platforms that ignore them; what it buys is
/// one readable description of the menu, instead of the same structure spelled
/// out three times under `cfg` attributes.
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let pkg = app.package_info();
    let config = app.config();
    let about_metadata = AboutMetadata {
        name: Some(pkg.name.clone()),
        version: Some(pkg.version.to_string()),
        copyright: config.bundle.copyright.clone(),
        authors: config.bundle.publisher.clone().map(|p| vec![p]),
        ..Default::default()
    };

    let about = PredefinedMenuItem::about(app, None, Some(about_metadata))?;
    let services = PredefinedMenuItem::services(app, None)?;
    let hide = PredefinedMenuItem::hide(app, None)?;
    let hide_others = PredefinedMenuItem::hide_others(app, None)?;
    let quit = PredefinedMenuItem::quit(app, None)?;
    let close_window = PredefinedMenuItem::close_window(app, None)?;
    let undo = PredefinedMenuItem::undo(app, None)?;
    let redo = PredefinedMenuItem::redo(app, None)?;
    let cut = PredefinedMenuItem::cut(app, None)?;
    let copy = PredefinedMenuItem::copy(app, None)?;
    let paste = PredefinedMenuItem::paste(app, None)?;
    let select_all = PredefinedMenuItem::select_all(app, None)?;
    let fullscreen = PredefinedMenuItem::fullscreen(app, None)?;
    let minimize = PredefinedMenuItem::minimize(app, None)?;
    let maximize = PredefinedMenuItem::maximize(app, None)?;
    // One item cannot sit in two places, so each separator position needs its
    // own rather than sharing one.
    let seps: Vec<PredefinedMenuItem<R>> = (0..8)
        .map(|_| PredefinedMenuItem::separator(app))
        .collect::<tauri::Result<_>>()?;

    let show_logs = MenuItem::with_id(app, SHOW_LOGS, show_logs_label(), true, None::<&str>)?;
    // Enabled even in a build that cannot update itself: the honest answer to
    // "am I current?" is a sentence in the card, and an item greyed out for
    // reasons the user cannot see reads as a bug.
    let check_updates = MenuItem::with_id(
        app,
        CHECK_UPDATES,
        "Check for Updates…",
        true,
        None::<&str>,
    )?;

    // The app submenu: macOS's, and macOS's alone.
    let app_menu = Submenu::with_items(
        app,
        pkg.name.clone(),
        true,
        &[
            &about as &dyn IsMenuItem<R>,
            &seps[0],
            &check_updates,
            &seps[6],
            &services,
            &seps[1],
            &hide,
            &hide_others,
            &seps[2],
            &quit,
        ],
    )?;

    // File. Quit lives here on the platforms with no app menu to hold it.
    let mut file_items: Vec<&dyn IsMenuItem<R>> = vec![&close_window];
    if !PLATFORM_HAS_APP_MENU {
        file_items.push(&quit);
    }
    let file = Submenu::with_items(app, "File", true, &file_items)?;

    // Edit. The clipboard half is the same everywhere; undo/redo are not.
    let mut edit_items: Vec<&dyn IsMenuItem<R>> = Vec::new();
    if PLATFORM_HAS_APP_MENU {
        edit_items.extend([&undo as &dyn IsMenuItem<R>, &redo, &seps[3]]);
    }
    edit_items.extend([&cut as &dyn IsMenuItem<R>, &copy, &paste, &select_all]);
    let edit = Submenu::with_items(app, "Edit", true, &edit_items)?;

    // View exists for one macOS-only item, so it exists only on macOS.
    let view = Submenu::with_items(app, "View", true, &[&fullscreen as &dyn IsMenuItem<R>])?;

    let window = Submenu::with_id_and_items(
        app,
        WINDOW_SUBMENU_ID,
        "Window",
        true,
        &[
            &minimize as &dyn IsMenuItem<R>,
            &maximize,
            &seps[4],
            &close_window,
        ],
    )?;

    // Help carries our items, plus About on the platforms with no app menu —
    // which are also the platforms whose Help submenu holds the update check,
    // there being no app menu to put it in.
    let mut help_items: Vec<&dyn IsMenuItem<R>> = Vec::new();
    if !PLATFORM_HAS_APP_MENU {
        help_items.extend([&check_updates as &dyn IsMenuItem<R>, &seps[7]]);
    }
    help_items.push(&show_logs);
    if !PLATFORM_HAS_APP_MENU {
        help_items.extend([&seps[5] as &dyn IsMenuItem<R>, &about]);
    }
    let help = Submenu::with_id_and_items(app, HELP_SUBMENU_ID, "Help", true, &help_items)?;

    let mut top: Vec<&dyn IsMenuItem<R>> = Vec::new();
    if PLATFORM_HAS_APP_MENU {
        top.push(&app_menu);
    }
    if PLATFORM_HAS_WINDOW_ITEMS {
        top.push(&file);
    }
    top.push(&edit);
    if PLATFORM_HAS_APP_MENU {
        top.push(&view);
    }
    if PLATFORM_HAS_WINDOW_ITEMS {
        top.push(&window);
    }
    top.push(&help);

    Menu::with_items(app, &top)
}

/// Attach the menu. Call once, from `setup`.
///
/// On macOS this becomes the global menu bar. Everywhere else the menu belongs
/// to each window, and `set_menu` reaches only the windows that already exist
/// when it is called — so this has to run after the configured windows are
/// created, which is what `setup` guarantees.
pub fn install<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    app.set_menu(build(app)?)?;
    Ok(())
}

/// What one of our menu items asks for.
///
/// Separated from [`on_event`] so the id-to-intent mapping is testable without a
/// running app: the ids are strings that arrive from the OS, and a typo in one
/// makes its item silently do nothing.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    ShowLogs,
    CheckUpdates,
}

fn action_for(id: &str) -> Option<Action> {
    match id {
        SHOW_LOGS => Some(Action::ShowLogs),
        CHECK_UPDATES => Some(Action::CheckUpdates),
        // Every predefined item — About, Quit, the clipboard — is handled by the
        // platform, so anything unrecognised here is not a failure.
        _ => None,
    }
}

/// Handle a menu click.
pub fn on_event<R: Runtime>(app: &AppHandle<R>, id: &str) {
    match action_for(id) {
        Some(Action::ShowLogs) => {
            let state: Arc<AppState> = (*app.state::<Arc<AppState>>()).clone();
            // Off the menu thread: this reads and deflates the whole log
            // directory, which on a heavy debugging week is not instant, and
            // blocking here freezes the menu bar.
            std::thread::spawn(move || export(&state));
        }
        Some(Action::CheckUpdates) => check_updates(app),
        None => {}
    }
}

/// Hand the update check to the window that can show its result.
///
/// The menu is reachable on macOS with the window hidden or minimised — the menu
/// bar belongs to the application, not to the window — so the window is brought
/// back before the event goes out. Emitting to a window nobody can see would
/// answer a question into the void.
fn check_updates<R: Runtime>(app: &AppHandle<R>) {
    crate::usage!("update.menu");
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    // A frontend that is not listening yet — a launch racing the menu — simply
    // misses this. Costing the user a second click beats holding the menu.
    if let Err(e) = app.emit(CHECK_UPDATES_EVENT, ()) {
        tracing::warn!("could not ask the window to check for updates: {e}");
    }
}

/// Collect the logs into the user's downloads folder and reveal the result.
///
/// Downloads rather than a save dialog: the next step is always "attach this to
/// an issue", and a dialog only adds a decision to a flow that has one obvious
/// answer. Revealing the file is the confirmation — there is no toast, because
/// this is reached when the window may be the thing that is broken.
fn export(state: &AppState) {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let dest = dirs::download_dir()
        // A machine with no Downloads folder is unusual but not a reason to
        // fail; the log directory is somewhere the user can certainly reach.
        .unwrap_or_else(crate::logging::log_dir);

    match crate::logbundle::write(state, &dest, &stamp) {
        Ok(bundle) => {
            tracing::info!(
                files = bundle.files,
                bytes = bundle.bytes,
                path = %crate::logging::redact_path(&bundle.path),
                "collected a support bundle"
            );
            // `result` matches the convention `commands::record` uses, so ok and
            // fail lines for the same event can be counted against each other.
            crate::usage!(
                "logs.export",
                result = "ok",
                files = bundle.files,
                bytes = bundle.bytes
            );
            if let Err(e) = crate::logbundle::reveal(&bundle.path) {
                // The zip is written either way, so this is a degraded success:
                // say where it is rather than reporting a failure.
                tracing::warn!(
                    "could not reveal the support bundle: {}",
                    crate::logging::redact(&e.to_string())
                );
            }
        }
        Err(e) => {
            tracing::error!(
                "could not collect a support bundle: {}",
                crate::logging::redact(&e.to_string())
            );
            crate::usage!("logs.export", result = "fail");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_label_names_the_platforms_file_manager() {
        // Not a tautology: the point is that the string is chosen per-platform
        // rather than saying "Finder" on Windows.
        let label = show_logs_label();
        if cfg!(target_os = "macos") {
            assert_eq!(label, "Show Log in Finder");
        } else if cfg!(target_os = "windows") {
            assert_eq!(label, "Show Log in Explorer");
        } else {
            assert_eq!(label, "Show Log Folder");
        }
    }

    #[test]
    fn every_id_we_own_maps_to_the_action_it_names() {
        assert_eq!(action_for(SHOW_LOGS), Some(Action::ShowLogs));
        assert_eq!(action_for(CHECK_UPDATES), Some(Action::CheckUpdates));
        // The two must stay distinct, or one item would run the other's code.
        assert_ne!(SHOW_LOGS, CHECK_UPDATES);
        // Predefined items and typos alike: no action, no panic.
        assert_eq!(action_for("app.check-update"), None);
        assert_eq!(action_for(""), None);
    }

    #[test]
    fn quit_never_ends_up_with_nowhere_to_live() {
        // Quit is in the app menu on macOS and in File everywhere else. A
        // platform with neither an app menu nor window items would drop it
        // entirely, and the menu is built at runtime so nothing else would
        // notice. Today that combination does not exist; this is the guard for
        // the day someone adds a platform.
        assert!(
            PLATFORM_HAS_APP_MENU || PLATFORM_HAS_WINDOW_ITEMS,
            "no app menu and no File submenu means no way to quit from the menu"
        );
    }
}
