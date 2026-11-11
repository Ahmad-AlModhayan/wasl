//! The tray icon (FR-B5, design D6): the mark with a badge for the states that need a look, a tooltip with the
//! state, a popover (macOS) or flyout (Windows) on left click (src/popover.rs), and a short native menu on right
//! click: Open Wasl, Pause all, Sync now, Settings…, Quit. The window sends the words in the user's language
//! (`set_tray_text`); English is used until it does. What differs per platform (template icons on macOS, icons
//! keyed to the taskbar's theme on Windows) lives in tray_macos.rs and tray_windows.rs.

use crate::daemon::{Conn, Daemon};
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};
use wasl_core::RunStatus;
use wasl_engine::Trigger;
use wasl_ipc::{DaemonStatus, Overall};

/// The tray's words, from the window, in the user's language.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayText {
    /// The notifications' words (#71), handed to src/notify.rs.
    #[serde(default)]
    pub notify: Option<crate::notify::NotifyText>,
    pub tooltip: String,
    pub open: String,
    pub pause_all: String,
    pub sync_now: String,
    pub settings: String,
    pub quit: String,
}

/// What the icon adds to the mark: nothing, or one badge whose shape says why to look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Badge {
    None,
    Warning,
    Error,
    Paused,
    Offline,
}

impl Badge {
    /// The icon file's name under icons/tray/.
    #[cfg(test)]
    pub fn name(self) -> &'static str {
        match self {
            Badge::None => "base",
            Badge::Warning => "warning",
            Badge::Error => "error",
            Badge::Paused => "paused",
            Badge::Offline => "offline",
        }
    }
}

pub struct Tray {
    icon: TrayIcon,
    menu: Menu<Wry>,
    open: MenuItem<Wry>,
    pause: CheckMenuItem<Wry>,
    sync: MenuItem<Wry>,
    settings: MenuItem<Wry>,
    quit: MenuItem<Wry>,
    text: Mutex<Option<TrayText>>,
    /// The badge and the taskbar theme the icon shows now.
    shown: Mutex<Option<(Badge, bool)>>,
    visible: AtomicBool,
}

/// The badge for an overall state: a failed pair or a Drive sign-in wins, then what the daemon says.
pub fn badge_for(overall: Overall, failed: bool) -> Badge {
    if failed {
        return Badge::Error;
    }
    match overall {
        Overall::NeedsYou => Badge::Warning,
        Overall::Paused => Badge::Paused,
        Overall::UpToDate | Overall::Syncing | Overall::Waiting | Overall::Idle => Badge::None,
    }
}

/// The badge for what the tray knows now. A pair counts as failed like the window counts it (src/format.ts
/// `pairStatus`): its last run failed and no new run is deciding it.
pub fn badge(conn: &Conn, status: Option<&DaemonStatus>) -> Badge {
    match (conn, status) {
        (Conn::Connected { .. }, Some(s)) => {
            let failed = s.accounts.iter().any(|a| a.auth_error.is_some())
                || s.pairs
                    .iter()
                    .any(|p| !p.running && p.pair.status.last_status == Some(RunStatus::Failed));
            // A pair held for its first preview needs approval, though the daemon calls that Waiting (src/format.ts
            // `overallLine`, #108, #116): the icon gets the warning badge, like Needs you. Syncing and Paused keep theirs.
            let held = s.pairs.iter().any(|p| {
                !p.running && !p.blocked && !p.pair.status.pending_approval && p.pair.status.hold_for_preview
            });
            match s.overall {
                Overall::Waiting | Overall::UpToDate if held && !failed => Badge::Warning,
                overall => badge_for(overall, failed),
            }
        }
        (Conn::NotRunning { .. }, _) => Badge::Offline,
        _ => Badge::None,
    }
}

/// Every pair paused: the menu's Pause all shows its check.
pub fn all_paused(status: Option<&DaemonStatus>) -> bool {
    status.is_some_and(|s| !s.pairs.is_empty() && s.pairs.iter().all(|p| p.pair.paused))
}

/// English words for when the window has not sent its own yet.
fn english(conn: &Conn, status: Option<&DaemonStatus>) -> TrayText {
    let state = match (conn, status) {
        (Conn::Connected { .. }, Some(s)) => match s.overall {
            Overall::UpToDate => "Synced",
            Overall::Syncing => "Syncing",
            Overall::Paused => "Paused",
            Overall::Waiting => "Waiting",
            Overall::NeedsYou => "Needs you",
            Overall::Idle => "No folder pairs yet",
        },
        (Conn::Connected { .. }, None) | (Conn::Connecting, _) => "Connecting…",
        (Conn::Starting, _) => "Starting the background service…",
        (Conn::NotRunning { .. }, _) => "Background service isn't running",
    };
    TrayText {
        notify: None,
        tooltip: format!("Wasl – {state}"),
        open: "Open Wasl".into(),
        pause_all: "Pause all".into(),
        sync_now: "Sync now".into(),
        settings: "Settings…".into(),
        quit: if cfg!(windows) { "Exit" } else { "Quit Wasl" }.into(),
    }
}

/// The icon's picture for a badge, on this platform.
fn image(badge: Badge, light_taskbar: bool) -> tauri::Result<Image<'static>> {
    #[cfg(target_os = "macos")]
    let bytes = {
        let _ = light_taskbar;
        crate::tray_macos::icon_bytes(badge)
    };
    #[cfg(not(target_os = "macos"))]
    let bytes = crate::tray_windows::icon_bytes(badge, light_taskbar);
    Image::from_bytes(bytes)
}

/// The taskbar's theme: light on Windows when the taskbar is light; macOS recolours template icons itself.
fn light_taskbar() -> bool {
    #[cfg(windows)]
    let light = crate::tray_windows::light_taskbar();
    #[cfg(not(windows))]
    let light = false;
    light
}

pub fn build(app: &AppHandle) -> tauri::Result<Tray> {
    let text = english(&Conn::Connecting, None);
    let open = MenuItem::with_id(app, "open", &text.open, true, None::<&str>)?;
    let pause = CheckMenuItem::with_id(app, "pause", &text.pause_all, false, false, None::<&str>)?;
    let sync = MenuItem::with_id(app, "sync", &text.sync_now, false, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", &text.settings, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", &text.quit, true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &PredefinedMenuItem::separator(app)?,
            &pause,
            &sync,
            &PredefinedMenuItem::separator(app)?,
            &settings,
            &quit,
        ],
    )?;
    let light = light_taskbar();
    let builder = TrayIconBuilder::with_id("wasl")
        .icon(image(Badge::None, light)?)
        .tooltip("Wasl")
        .menu(&menu)
        // Left click opens the popover; the menu is for right click.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| on_menu(app, event.id.as_ref()))
        .on_tray_icon_event(|tray, event| match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                position,
                ..
            } => crate::popover::toggle(tray.app_handle(), crate::popover::icon_area(&rect, position)),
            // Windows: a double click runs the menu's bold default item, Open Wasl. The flyout its first click
            // opened closes, and the click Windows sends after it leaves the flyout closed (popover::hide).
            TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } => crate::popover::open_in_main(tray.app_handle(), None),
            _ => {}
        });
    #[cfg(target_os = "macos")]
    let builder = builder.icon_as_template(true);
    let icon = builder.build(app)?;
    // Hidden at the last run: hidden from launch, before the daemon answers (or if it never does).
    let shown = remembered_visible(app);
    if !shown {
        let _ = icon.set_visible(false);
    }
    #[cfg(windows)]
    {
        crate::tray_windows::bold_default_item(&menu);
        crate::tray_windows::watch_taskbar_theme(app);
    }
    Ok(Tray {
        icon,
        menu,
        open,
        pause,
        sync,
        settings,
        quit,
        text: Mutex::new(None),
        shown: Mutex::new(Some((Badge::None, light))),
        visible: AtomicBool::new(shown),
    })
}

/// Runs a tray action: from the right-click menu, or from the popover's own menu (`popup_menu`).
fn on_menu(app: &AppHandle, id: &str) {
    match id {
        "open" => {
            crate::popover::hide(app, false);
            crate::show_main(app);
        }
        "settings" => crate::popover::open_in_main(app, Some("settings".into())),
        // Quits the tray only; wasld keeps syncing.
        "quit" => app.exit(0),
        "sync" => {
            let d = app.state::<Arc<Daemon>>().inner().clone();
            tauri::async_runtime::spawn(async move {
                if let Some(client) = d.client().await {
                    let _ = client.sync_all(Trigger::Manual).await;
                }
            });
        }
        "pause" => {
            let d = app.state::<Arc<Daemon>>().inner().clone();
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let Some(client) = d.client().await else { return };
                let status = d.status();
                let resume = all_paused(status.as_ref());
                for p in status.iter().flat_map(|s| s.pairs.iter()) {
                    if p.pair.paused != !resume {
                        let _ = client.set_paused(&p.pair.id, !resume, None).await;
                    }
                }
                // The check follows the pairs, not the click, until the daemon's status catches up.
                refresh(&app);
            });
        }
        _ => {}
    }
}

/// Opens the tray's menu under the popover's "More" button, so the popover never needs its own. `x` and `y` are
/// the button's corner in logical pixels from the window's top-left: the menu opens by the button also when it
/// was pressed from the keyboard, with the pointer elsewhere (even on another display).
pub fn popup_menu(window: &tauri::WebviewWindow, x: f64, y: f64) {
    if let Some(tray) = window.app_handle().try_state::<Tray>() {
        let _ = window.popup_menu_at(&tray.menu, tauri::LogicalPosition::new(x, y));
    }
}

/// Stores the window's words and redraws.
pub fn set_text(app: &AppHandle, text: TrayText) {
    if let Some(tray) = app.try_state::<Tray>()
        && let Ok(mut t) = tray.text.lock()
    {
        *t = Some(text);
    }
    refresh(app);
}

/// The daemon setting that keeps General's "Show Wasl in the menu bar / notification area" ("false": hidden).
pub const VISIBLE_SETTING: &str = "tray_visible";

/// A local copy of [`VISIBLE_SETTING`] in the app's config folder, read when the icon is built so a hidden icon
/// doesn't flash at launch. The daemon's setting stays the source of truth: it overwrites this on every connect.
const VISIBLE_FILE: &str = "tray_visible";

fn visible_file(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_config_dir().ok().map(|dir| dir.join(VISIBLE_FILE))
}

/// What the local copy says: shown unless it says "false" (no file, or one that can't be read, means shown).
fn parse_visible(contents: Option<&str>) -> bool {
    contents.is_none_or(|s| s.trim() != "false")
}

fn remembered_visible(app: &AppHandle) -> bool {
    let contents = visible_file(app).and_then(|path| std::fs::read_to_string(path).ok());
    parse_visible(contents.as_deref())
}

fn remember_visible(app: &AppHandle, visible: bool) {
    let Some(path) = visible_file(app) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, if visible { "true" } else { "false" });
}

/// Shows or hides the icon (General: "Show Wasl in the menu bar / notification area"), and keeps the local copy.
pub fn set_visible(app: &AppHandle, visible: bool) {
    remember_visible(app, visible);
    let Some(tray) = app.try_state::<Tray>() else {
        return;
    };
    if tray.visible.swap(visible, Ordering::SeqCst) == visible {
        return;
    }
    let _ = tray.icon.set_visible(visible);
    if !visible {
        crate::popover::hide(app, false);
    }
    #[cfg(target_os = "macos")]
    crate::tray_macos::on_tray_visible(app, visible);
}

/// Whether the icon shows (macOS keeps the Dock icon while it doesn't).
#[cfg(target_os = "macos")]
pub fn visible(app: &AppHandle) -> bool {
    app.try_state::<Tray>()
        .is_none_or(|t| t.visible.load(Ordering::SeqCst))
}

/// Redraws icon, tooltip and menu from the daemon's current state.
pub fn refresh(app: &AppHandle) {
    let (Some(tray), Some(d)) = (app.try_state::<Tray>(), app.try_state::<Arc<Daemon>>()) else {
        return;
    };
    let conn = d.conn();
    let status = d.status();
    let now = (badge(&conn, status.as_ref()), light_taskbar());
    if let Ok(mut shown) = tray.shown.lock()
        && *shown != Some(now)
        && let Ok(img) = image(now.0, now.1)
    {
        // macOS: `set_icon` alone would turn template mode off, leaving a black mark on a dark menu bar. Elsewhere
        // this is `set_icon`.
        let _ = tray.icon.set_icon_with_as_template(Some(img), cfg!(target_os = "macos"));
        *shown = Some(now);
    }
    let text = tray
        .text
        .lock()
        .ok()
        .and_then(|t| t.clone())
        .unwrap_or_else(|| english(&conn, status.as_ref()));
    let connected = matches!(conn, Conn::Connected { .. });
    let has_pairs = status.as_ref().is_some_and(|s| !s.pairs.is_empty());
    let _ = tray.icon.set_tooltip(Some(&text.tooltip));
    let _ = tray.open.set_text(&text.open);
    let _ = tray.pause.set_text(&text.pause_all);
    let _ = tray.pause.set_checked(all_paused(status.as_ref()));
    let _ = tray.pause.set_enabled(connected && has_pairs);
    let _ = tray.sync.set_text(&text.sync_now);
    let _ = tray.sync.set_enabled(connected && has_pairs);
    let _ = tray.settings.set_text(&text.settings);
    let _ = tray.quit.set_text(&text.quit);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_pair_shows_the_error_badge_whatever_else_is_going_on() {
        for overall in [Overall::UpToDate, Overall::Syncing, Overall::NeedsYou, Overall::Paused] {
            assert_eq!(badge_for(overall, true), Badge::Error);
        }
    }

    #[test]
    fn needs_you_and_paused_get_their_own_badges() {
        assert_eq!(badge_for(Overall::NeedsYou, false), Badge::Warning);
        assert_eq!(badge_for(Overall::Paused, false), Badge::Paused);
    }

    #[test]
    fn calm_states_show_the_bare_mark() {
        for overall in [Overall::UpToDate, Overall::Syncing, Overall::Waiting, Overall::Idle] {
            assert_eq!(badge_for(overall, false), Badge::None);
        }
    }

    #[test]
    fn a_background_service_that_is_not_running_shows_offline() {
        let conn = Conn::NotRunning { message: None };
        assert_eq!(badge(&conn, None), Badge::Offline);
        assert_eq!(badge(&Conn::Connecting, None), Badge::None);
    }

    fn status(overall: Overall, held: bool) -> DaemonStatus {
        let mut pair = wasl_store::Pair::new("p1", "Photos", "/home/me/Photos");
        pair.status.hold_for_preview = held;
        DaemonStatus {
            version: "test".into(),
            protocol: 1,
            accounts: vec![],
            pairs: vec![wasl_ipc::PairState {
                pair,
                running: false,
                progress: None,
                open_conflicts: 0,
                blocked: false,
                folder_bytes: None,
                skipped: None,
                peers: vec![],
                free_bytes: None,
            }],
            overall,
            agent_changes: 0,
        }
    }

    #[test]
    fn a_pair_held_for_its_first_preview_shows_the_warning_badge() {
        // The daemon calls a held preview Waiting; the footer and the tooltip say Needs approval (#108, #116).
        let conn = Conn::Connected { version: "test".into() };
        for overall in [Overall::Waiting, Overall::UpToDate, Overall::NeedsYou] {
            assert_eq!(badge(&conn, Some(&status(overall, true))), Badge::Warning, "{overall:?}");
        }
        assert_eq!(badge(&conn, Some(&status(Overall::Waiting, false))), Badge::None);
        // Syncing and Paused keep their own badge, as they keep their word in the footer.
        assert_eq!(badge(&conn, Some(&status(Overall::Syncing, true))), Badge::None);
        assert_eq!(badge(&conn, Some(&status(Overall::Paused, true))), Badge::Paused);
    }

    #[test]
    fn every_badge_has_its_own_icon_file() {
        let all = [Badge::None, Badge::Warning, Badge::Error, Badge::Paused, Badge::Offline];
        let names: std::collections::HashSet<_> = all.iter().map(|b| b.name()).collect();
        assert_eq!(names.len(), all.len());
    }

    #[test]
    fn the_local_copy_hides_the_icon_only_when_it_says_false() {
        assert!(parse_visible(None));
        assert!(parse_visible(Some("true")));
        assert!(parse_visible(Some("")));
        assert!(!parse_visible(Some("false")));
        assert!(!parse_visible(Some("false\n")));
    }
}
