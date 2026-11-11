//! The menu bar icon's macOS half (#69): template icons, which macOS recolours for light and dark menu bars; the
//! popover's size and material; which display's scale the icon is measured in; giving focus back on Esc; and the
//! Dock icon, which stays while the menu bar icon is hidden so Wasl can still be reached.

use crate::tray::Badge;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSApplicationActivationOptions, NSEvent, NSRunningApplication, NSScreen, NSWorkspace};
use std::sync::Mutex;
use tauri::window::{Effect, EffectState, EffectsBuilder};
use tauri::{AppHandle, Manager, WebviewWindowBuilder, Wry};

/// The app that was in front when the popover opened, if it wasn't Wasl: Esc and a second click give focus back
/// to it.
static PREVIOUS_APP: Mutex<Option<Retained<NSRunningApplication>>> = Mutex::new(None);

/// The popover's width, logical pixels (design: 320).
pub const WIDTH: f64 = 320.0;
/// Between the menu bar and the popover.
pub const GAP: f64 = 6.0;
/// The least room between the popover and a screen edge.
pub const MARGIN: f64 = 8.0;
/// macOS measures screens in points.
pub const LOGICAL: bool = true;

/// Template images: black and alpha only (scripts/make-icons.mjs).
pub fn icon_bytes(badge: Badge) -> &'static [u8] {
    match badge {
        Badge::None => include_bytes!("../icons/tray/mac/base.png"),
        Badge::Warning => include_bytes!("../icons/tray/mac/warning.png"),
        Badge::Error => include_bytes!("../icons/tray/mac/error.png"),
        Badge::Paused => include_bytes!("../icons/tray/mac/paused.png"),
        Badge::Offline => include_bytes!("../icons/tray/mac/offline.png"),
    }
}

/// The popover's window: the system's popover material behind a transparent page, rounded like a popover, on
/// whichever Space is in front.
pub fn popover_window(builder: WebviewWindowBuilder<'_, Wry, AppHandle>) -> WebviewWindowBuilder<'_, Wry, AppHandle> {
    builder
        .transparent(true)
        .shadow(true)
        .visible_on_all_workspaces(true)
        .effects(
            EffectsBuilder::new()
                .effect(Effect::Popover)
                .state(EffectState::Active)
                .radius(16.0)
                .build(),
        )
}

/// The scale of the display the click was on. The tray measures its icon in that display's pixels (points times
/// the menu bar's backing scale), so dividing by it gives points, also with a 1x display beside a 2x one, where the
/// other display's scale would put the icon on the wrong screen. The pointer is on that display: AppKit says which.
pub fn icon_scale(app: &AppHandle) -> f64 {
    let under_pointer = MainThreadMarker::new().and_then(|mtm| {
        // Screen coordinates from the main display's bottom-left corner, in points, like the screens' frames.
        let p = NSEvent::mouseLocation();
        NSScreen::screens(mtm).iter().find_map(|screen| {
            let f = screen.frame();
            let inside = p.x >= f.origin.x
                && p.x <= f.origin.x + f.size.width
                && p.y >= f.origin.y
                && p.y <= f.origin.y + f.size.height;
            inside.then(|| screen.backingScaleFactor())
        })
    });
    under_pointer
        .or_else(|| app.primary_monitor().ok().flatten().map(|m| m.scale_factor()))
        .unwrap_or(1.0)
}

/// Before the popover shows (a click on the menu bar icon doesn't make Wasl active): remember the app in front,
/// unless it is Wasl itself.
pub fn before_show() {
    let previous = MainThreadMarker::new().and_then(|mtm| {
        if NSApplication::sharedApplication(mtm).isActive() {
            return None;
        }
        NSWorkspace::sharedWorkspace().frontmostApplication()
    });
    *PREVIOUS_APP.lock().unwrap_or_else(|e| e.into_inner()) = previous;
}

/// After Esc or a second click: hand focus back to the app that had it before the popover opened, so a Wasl window
/// open behind it stays behind it. Wasl hides itself when that app is gone or won't come forward, and when no Wasl
/// window is open; when Wasl was in front, its window takes focus back by itself.
pub fn after_hide(app: &AppHandle) {
    let previous = PREVIOUS_APP.lock().unwrap_or_else(|e| e.into_inner()).take();
    let main_open = app
        .get_webview_window("main")
        .is_some_and(|w| w.is_visible().unwrap_or(false));
    let had_other = previous.is_some();
    let gave_back = previous.is_some_and(|prev| {
        !prev.isTerminated() && prev.activateWithOptions(NSApplicationActivationOptions::empty())
    });
    if !gave_back && (had_other || !main_open) {
        let _ = app.hide();
    }
}

/// A Wasl window closed (`closing`; `main` when Wasl starts in the background): once neither the main window nor
/// Settings shows, the Dock icon and the menu bar go with them, unless the menu bar icon is hidden, when the Dock icon
/// is the way back. Settings keeps them while it is open: it would lose its menu bar, and ⌘, ⌘W and ⌘Q with it.
pub fn window_hidden(app: &AppHandle, closing: &str) {
    if crate::tray::visible(app) && !window_open(app, closing) {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    }
}

/// The menu bar icon was shown or hidden.
pub fn on_tray_visible(app: &AppHandle, visible: bool) {
    if !visible {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    } else if !window_open(app, "") {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    }
}

/// Whether the main window or the Settings window shows, leaving out the one labelled `except` (it is closing).
fn window_open(app: &AppHandle, except: &str) -> bool {
    ["main", crate::settings_window::LABEL]
        .into_iter()
        .filter(|label| *label != except)
        .any(|label| app.get_webview_window(label).is_some_and(|w| w.is_visible().unwrap_or(false)))
}
