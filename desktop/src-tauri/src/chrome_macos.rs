//! The macOS window chrome (#61, design D1): an overlay title bar with the traffic lights in a 52pt toolbar
//! strip over a glass sidebar. `tauri.macos.conf.json` describes the window and leaves it to us to create, so
//! two things can change first:
//! - When macOS runs Wasl in Arabic (the bundle declares `ar`), AppKit puts the traffic lights on the right.
//!   A fixed `trafficLightPosition` is measured from the left and would pull them back, so it is dropped.
//! - With Reduce Transparency or Increase Contrast on, the window effect is dropped and the sidebar is opaque.
//!
//! `chrome` tells the window where the traffic lights are, so the shell keeps that corner clear.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplication, NSUserInterfaceLayoutDirection, NSWindow, NSWindowButton, NSWorkspace,
};
use objc2_foundation::NSString;
use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, WebviewWindow, WebviewWindowBuilder};

/// The traffic lights' frame, in CSS pixels from the window's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// What the shell needs to lay out around the window's own controls.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Chrome {
    /// None in full screen, where the traffic lights leave the window.
    pub lights: Option<Frame>,
    pub window_width: f64,
    /// Reduce Transparency or Increase Contrast: no window effect, so the sidebar must be opaque.
    pub opaque: bool,
}

/// The window effect from the config and whether it is on now.
struct Effects {
    config: Option<tauri::utils::config::WindowEffectsConfig>,
    on: Mutex<bool>,
}

fn opaque() -> bool {
    let ws = NSWorkspace::sharedWorkspace();
    ws.accessibilityDisplayShouldReduceTransparency()
        || ws.accessibilityDisplayShouldIncreaseContrast()
}

fn system_rtl(mtm: MainThreadMarker) -> bool {
    NSApplication::sharedApplication(mtm).userInterfaceLayoutDirection()
        == NSUserInterfaceLayoutDirection::RightToLeft
}

/// Creates the main window from the config (`create: false` on macOS). Runs in `setup`, on the main thread.
pub fn create_main_window(app: &AppHandle) -> tauri::Result<()> {
    let Some(mut config) = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == "main")
        .cloned()
    else {
        return Ok(());
    };
    if let Some(mtm) = MainThreadMarker::new()
        && system_rtl(mtm)
    {
        config.traffic_light_position = None;
    }
    let effects = config.window_effects.clone();
    let on = !opaque();
    if !on {
        config.window_effects = None;
    }
    let mut builder = WebviewWindowBuilder::from_config(app, &config)?;
    if !on {
        // Opaque from the first paint, before the page can ask (`chrome`).
        builder = builder.initialization_script(
            "document.documentElement.toggleAttribute('data-chrome-opaque', true);",
        );
    }
    builder.build()?;
    app.manage(Effects {
        config: effects,
        on: Mutex::new(on),
    });
    Ok(())
}

/// The AppKit window behind a Tauri window, found among the app's windows without touching raw pointers.
fn ns_window(window: &WebviewWindow, mtm: MainThreadMarker) -> Option<Retained<NSWindow>> {
    let ptr = window.ns_window().ok()?.cast_const();
    NSApplication::sharedApplication(mtm)
        .windows()
        .iter()
        .find(|w| std::ptr::eq(Retained::as_ptr(w).cast(), ptr))
}

fn lights(ns: &NSWindow) -> Option<Frame> {
    let buttons = [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ];
    let height = ns.frame().size.height;
    let mut frame: Option<(f64, f64, f64, f64)> = None;
    for b in buttons {
        let button = ns.standardWindowButton(b)?;
        if button.isHidden() {
            continue;
        }
        // Window coordinates start at the bottom-left; the shell's start at the top-left.
        let r = button.convertRect_toView(button.bounds(), None);
        let (x0, x1) = (r.origin.x, r.origin.x + r.size.width);
        let (y0, y1) = (height - r.origin.y - r.size.height, height - r.origin.y);
        frame = Some(match frame {
            None => (x0, y0, x1, y1),
            Some((a, b, c, d)) => (a.min(x0), b.min(y0), c.max(x1), d.max(y1)),
        });
    }
    frame.map(|(x0, y0, x1, y1)| Frame {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    })
}

/// Where the traffic lights are now, and whether the sidebar must be opaque. Turns the window effect off or on
/// again when Reduce Transparency or Increase Contrast changed (the window asks when it gains focus).
pub fn chrome(window: &WebviewWindow) -> Chrome {
    let opaque = opaque();
    if let Some(effects) = window.app_handle().try_state::<Effects>() {
        let mut on = effects.on.lock().unwrap_or_else(|e| e.into_inner());
        let want = !opaque;
        if *on != want
            && window
                .set_effects(if want { effects.config.clone() } else { None })
                .is_ok()
        {
            *on = want;
        }
    }
    let width = window
        .inner_size()
        .ok()
        .zip(window.scale_factor().ok())
        .map(|(s, f)| f64::from(s.width) / f)
        .unwrap_or_default();
    let full_screen = window.is_fullscreen().unwrap_or(false);
    let lights = MainThreadMarker::new()
        .and_then(|mtm| ns_window(window, mtm))
        .filter(|_| !full_screen)
        .and_then(|ns| lights(&ns));
    Chrome {
        lights,
        window_width: width,
        opaque,
    }
}

/// A double-click on the toolbar strip does what System Settings says: zoom (the default), minimise or nothing.
pub fn title_bar_double_click(window: &WebviewWindow) {
    let defaults = objc2_foundation::NSUserDefaults::standardUserDefaults();
    let action = defaults
        .stringForKey(&NSString::from_str("AppleActionOnDoubleClick"))
        .map(|s| s.to_string());
    let minimise = match action.as_deref() {
        Some("Minimize") => true,
        Some("None") => return,
        Some(_) => false,
        // Older macOS: a separate yes/no for minimising.
        None => defaults.boolForKey(&NSString::from_str("AppleMiniaturizeOnDoubleClick")),
    };
    let _ = if minimise {
        window.minimize()
    } else if window.is_maximized().unwrap_or(false) {
        window.unmaximize()
    } else {
        window.maximize()
    };
}
