//! The notification-area icon's Windows half (#69): icons keyed to the taskbar's own theme (not the app's), the
//! menu's bold default item, and the flyout's size. Linux follows Windows here (its panels are mostly dark), minus
//! the registry and the menu handle.

use crate::tray::Badge;
use tauri::{AppHandle, WebviewWindowBuilder, Wry};

/// The flyout's width, logical pixels (design: 360).
pub const WIDTH: f64 = 360.0;
/// Between the taskbar and the flyout, as Windows 11's own flyouts.
pub const GAP: f64 = 12.0;
/// The least room between the flyout and a screen edge.
pub const MARGIN: f64 = 12.0;
/// Windows measures screens in physical pixels.
pub const LOGICAL: bool = false;

/// A dark mark for a light taskbar, a white one for a dark taskbar (scripts/make-icons.mjs).
pub fn icon_bytes(badge: Badge, light_taskbar: bool) -> &'static [u8] {
    match (light_taskbar, badge) {
        (true, Badge::None) => include_bytes!("../icons/tray/win/light/base.png"),
        (true, Badge::Warning) => include_bytes!("../icons/tray/win/light/warning.png"),
        (true, Badge::Error) => include_bytes!("../icons/tray/win/light/error.png"),
        (true, Badge::Paused) => include_bytes!("../icons/tray/win/light/paused.png"),
        (true, Badge::Offline) => include_bytes!("../icons/tray/win/light/offline.png"),
        (false, Badge::None) => include_bytes!("../icons/tray/win/dark/base.png"),
        (false, Badge::Warning) => include_bytes!("../icons/tray/win/dark/warning.png"),
        (false, Badge::Error) => include_bytes!("../icons/tray/win/dark/error.png"),
        (false, Badge::Paused) => include_bytes!("../icons/tray/win/dark/paused.png"),
        (false, Badge::Offline) => include_bytes!("../icons/tray/win/dark/offline.png"),
    }
}

/// Whether the taskbar is light: Settings > Personalization > Colors > "Choose your default Windows mode".
#[cfg(windows)]
pub fn light_taskbar() -> bool {
    windows_registry::CURRENT_USER
        .open(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|key| key.get_u32("SystemUsesLightTheme"))
        .is_ok_and(|v| v != 0)
}

/// Windows tells apps about a new taskbar theme only in a broadcast to their windows; a look every few seconds is
/// cheaper than listening for it, and the icon follows within moments.
#[cfg(windows)]
pub fn watch_taskbar_theme(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut last = light_taskbar();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            let now = light_taskbar();
            if now != last {
                last = now;
                crate::tray::refresh(&app);
            }
        }
    });
}

/// "Open Wasl" drawn bold, as the item a double click would run.
#[cfg(windows)]
pub fn bold_default_item(menu: &tauri::menu::Menu<Wry>) {
    use tauri::menu::ContextMenu;
    if let Ok(hmenu) = menu.hpopupmenu() {
        crate::chrome_windows::bold_first_item(hmenu);
    }
}

/// The flyout's window: Windows 11 rounds a frameless window and draws its border and shadow when it has one.
pub fn popover_window(builder: WebviewWindowBuilder<'_, Wry, AppHandle>) -> WebviewWindowBuilder<'_, Wry, AppHandle> {
    builder.shadow(true)
}

/// The tray measures its icon in physical pixels, which are Windows' screen units already.
pub fn icon_scale(_app: &AppHandle) -> f64 {
    1.0
}

/// Nothing to remember before the flyout shows: Windows brings the next window forward itself when it hides.
pub fn before_show() {}

/// Windows brings the next window forward itself when the flyout hides.
pub fn after_hide(_app: &AppHandle) {}
