//! The tray's popover (macOS, under the menu bar icon) and flyout (Windows, above the taskbar), design D6: a small
//! frameless window `tray` showing the same bundle's tray page (src/tray/), which it finds by its label. A left click
//! on the icon toggles it; it hides when it loses focus and on Esc (the page calls `tray_hide`). The page says how
//! tall it is (`tray_fit`); this module places it beside the icon on the icon's display, inside the work area.
//!
//! The window is made on the first click and shown once its page has said how tall it is, so it never flashes empty.
//! What differs per platform (size, material, screen units, giving focus back) comes from tray_macos.rs and
//! tray_windows.rs.

use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{
    AppHandle, Emitter, LogicalPosition, Manager, PhysicalPosition, Position, Size, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, Window, WindowEvent,
};

#[cfg(target_os = "macos")]
use crate::tray_macos as platform;
#[cfg(not(target_os = "macos"))]
use crate::tray_windows as platform;

/// The window's label; capabilities/tray.json names it.
pub const LABEL: &str = "tray";
/// Sent to the tray page each time it opens: it reloads what it shows and takes focus.
const SHOWN: &str = "tray://shown";
/// Sent to the main window: the screen to open (`problems`, `agents`, `settings` or `pair:<id>`).
const OPEN: &str = "tray://open";
/// A click on the icon this soon after the popover hid is the click that hid it: it stays closed.
const REOPEN_GUARD: Duration = Duration::from_millis(400);
/// The height to place by before the page has measured itself, logical pixels.
const FIRST_HEIGHT: f64 = 360.0;

/// A rectangle on the screens: physical pixels from the tray, or the platform's screen units (`platform::LOGICAL`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Area {
    fn right(&self) -> f64 {
        self.x + self.w
    }
    fn bottom(&self) -> f64 {
        self.y + self.h
    }
    fn cx(&self) -> f64 {
        self.x + self.w / 2.0
    }
    fn cy(&self) -> f64 {
        self.y + self.h / 2.0
    }
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
    fn scaled(&self, by: f64) -> Area {
        Area {
            x: self.x / by,
            y: self.y / by,
            w: self.w / by,
            h: self.h / by,
        }
    }
}

/// The screen edge an icon sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

/// The edge the icon's bar is on: the side where the icon lies outside the work area (the menu bar, or the taskbar),
/// or, for a taskbar that hides itself, the monitor edge nearest the icon.
pub fn edge_of(icon: Area, monitor: Area, work: Area) -> Edge {
    let (cx, cy) = (icon.cx(), icon.cy());
    if cy < work.y {
        return Edge::Top;
    }
    if cy >= work.bottom() {
        return Edge::Bottom;
    }
    if cx < work.x {
        return Edge::Left;
    }
    if cx >= work.right() {
        return Edge::Right;
    }
    let distances = [
        (cy - monitor.y, Edge::Top),
        (monitor.bottom() - cy, Edge::Bottom),
        (cx - monitor.x, Edge::Left),
        (monitor.right() - cx, Edge::Right),
    ];
    let mut best = distances[0];
    for d in &distances[1..] {
        if d.0 < best.0 {
            best = *d;
        }
    }
    best.1
}

/// Where the popover's top-left corner goes: inside the icon's edge, `gap` off the bar, centred on the icon along it
/// and kept `margin` inside the work area. So a taskbar's icons on the left (Arabic Windows) or a second display get
/// a popover by the icon, never off-screen.
pub fn place(icon: Area, size: (f64, f64), monitor: Area, work: Area, gap: f64, margin: f64) -> (f64, f64) {
    let (w, h) = size;
    let clamp = |v: f64, lo: f64, hi: f64| if hi < lo { lo } else { v.clamp(lo, hi) };
    let along_x = clamp(icon.cx() - w / 2.0, work.x + margin, work.right() - margin - w);
    let along_y = clamp(icon.cy() - h / 2.0, work.y + margin, work.bottom() - margin - h);
    match edge_of(icon, monitor, work) {
        Edge::Top => (along_x, icon.bottom().max(work.y) + gap),
        Edge::Bottom => (along_x, icon.y.min(work.bottom()) - gap - h),
        Edge::Left => (icon.right().max(work.x) + gap, along_y),
        Edge::Right => (icon.x.min(work.right()) - gap - w, along_y),
    }
}

/// Which screen the icon is on, with icon and screens in the same screen units: the one holding its centre.
pub fn monitor_at(icon: Area, screens: &[Area]) -> Option<usize> {
    screens.iter().position(|s| s.contains(icon.cx(), icon.cy()))
}

/// The tray icon's rectangle in physical pixels; the pointer when the tray can't say.
pub fn icon_area(rect: &tauri::Rect, pointer: PhysicalPosition<f64>) -> Area {
    // The tray reports physical pixels; a logical rect would need a scale it doesn't give, so it is taken as is.
    let (x, y) = match rect.position {
        Position::Physical(p) => (f64::from(p.x), f64::from(p.y)),
        Position::Logical(p) => (p.x, p.y),
    };
    let (w, h) = match rect.size {
        Size::Physical(s) => (f64::from(s.width), f64::from(s.height)),
        Size::Logical(s) => (s.width, s.height),
    };
    if w < 1.0 || h < 1.0 {
        return Area {
            x: pointer.x,
            y: pointer.y,
            w: 1.0,
            h: 1.0,
        };
    }
    Area { x, y, w, h }
}

#[derive(Default)]
struct State {
    /// The icon at the last click, in screen units (`platform::LOGICAL`).
    anchor: Option<Area>,
    /// The page's height, logical pixels; 0 until it has measured itself.
    height: f64,
    /// Opened before the page had measured itself: show once it has.
    show_on_fit: bool,
    hidden_at: Option<Instant>,
}

#[derive(Default)]
pub struct Popover(Mutex<State>);

fn state(app: &AppHandle) -> Option<std::sync::MutexGuard<'_, State>> {
    let p = app.try_state::<Popover>()?;
    // A poisoned lock only means a panic elsewhere mid-update; the fields stay usable.
    Some(p.inner().0.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Opens the popover by the icon, or closes it when it is open (or still opening). `icon` is in the tray's
/// physical pixels.
pub fn toggle(app: &AppHandle, icon: Area) {
    let existing = app.get_webview_window(LABEL);
    if existing.as_ref().is_some_and(|w| w.is_visible().unwrap_or(false)) {
        hide(app, true);
        return;
    }
    // On macOS the tray measures in the pixels of the icon's own display; screens are placed in points.
    let icon = icon.scaled(platform::icon_scale(app));
    let show_now = {
        let Some(mut s) = state(app) else { return };
        // A second click before the page measured itself closes the popover that was opening.
        if std::mem::take(&mut s.show_on_fit) {
            return;
        }
        if s.hidden_at.take().is_some_and(|t| t.elapsed() < REOPEN_GUARD) {
            return;
        }
        s.anchor = Some(icon);
        // Shown only once the page has measured itself, so it never opens empty.
        let measured = existing.is_some() && s.height > 0.0;
        s.show_on_fit = !measured;
        measured
    };
    match existing {
        Some(w) => {
            place_window(app, &w);
            if show_now {
                show(&w);
            }
        }
        None => {
            if let Err(e) = create(app) {
                if let Some(mut s) = state(app) {
                    s.show_on_fit = false;
                }
                eprintln!("wasl: the tray popover could not open: {e}");
            }
        }
    }
}

fn create(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::default())
        .title("Wasl")
        .inner_size(platform::WIDTH, FIRST_HEIGHT)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .visible(false)
        .focused(true);
    let window = platform::popover_window(builder).build()?;
    place_window(app, &window);
    Ok(window)
}

fn show(window: &WebviewWindow) {
    platform::before_show();
    let _ = window.show();
    let _ = window.set_focus();
    let _ = window.emit_to(LABEL, SHOWN, ());
}

/// Hides the popover, or cancels one still waiting for its page to measure itself. `give_back` hands focus back
/// to the app that had it (Esc, or a second click on the icon); a click elsewhere has already moved it.
pub fn hide(app: &AppHandle, give_back: bool) {
    let Some(w) = app.get_webview_window(LABEL) else {
        return;
    };
    let visible = w.is_visible().unwrap_or(false);
    if let Some(mut s) = state(app) {
        let opening = std::mem::take(&mut s.show_on_fit);
        // The click that comes with this (Windows sends one after a double click) leaves it closed.
        if visible || opening {
            s.hidden_at = Some(Instant::now());
        }
    }
    if !visible {
        return;
    }
    let _ = w.hide();
    if give_back {
        platform::after_hide(app);
    }
}

/// The page's height changed (logical pixels): resize, keep it by the icon, and show it if this click made it.
pub fn fit(app: &AppHandle, height: f64) {
    let show_now = {
        let Some(mut s) = state(app) else { return };
        s.height = height.clamp(80.0, 2000.0);
        std::mem::take(&mut s.show_on_fit)
    };
    if let Some(w) = app.get_webview_window(LABEL) {
        place_window(app, &w);
        if show_now {
            show(&w);
        }
    }
}

/// Opens the main window at a screen the popover or the tray menu named.
pub fn open_in_main(app: &AppHandle, route: Option<String>) {
    hide(app, false);
    crate::show_main(app);
    if let Some(route) = route {
        let _ = app.emit_to("main", OPEN, route);
    }
}

/// The popover's own window events (main.rs sends them here): it hides on losing focus and on close (Alt+F4).
pub fn on_window_event(window: &Window, event: &WindowEvent) {
    match event {
        WindowEvent::Focused(false) => hide(window.app_handle(), false),
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            hide(window.app_handle(), true);
        }
        _ => {}
    }
}

/// Sizes and places the window for the last click. On macOS screens are measured in points, on Windows in
/// physical pixels (`platform::LOGICAL`); everything is worked out in those units.
fn place_window(app: &AppHandle, window: &WebviewWindow) {
    let (anchor, height) = match state(app) {
        Some(s) => (s.anchor, if s.height > 0.0 { s.height } else { FIRST_HEIGHT }),
        None => return,
    };
    let Some(icon) = anchor else { return };
    let monitors = app.available_monitors().unwrap_or_default();
    let unit = |m: &tauri::Monitor| if platform::LOGICAL { m.scale_factor() } else { 1.0 };
    let area = |p: PhysicalPosition<i32>, w: u32, h: u32| Area {
        x: f64::from(p.x),
        y: f64::from(p.y),
        w: f64::from(w),
        h: f64::from(h),
    };
    let screens: Vec<Area> = monitors
        .iter()
        .map(|m| area(*m.position(), m.size().width, m.size().height).scaled(unit(m)))
        .collect();
    let on = monitor_at(icon, &screens)
        .and_then(|i| monitors.get(i).cloned())
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(m) = on else { return };
    let u = unit(&m);
    let scale = m.scale_factor();
    let monitor = area(*m.position(), m.size().width, m.size().height).scaled(u);
    let wa = m.work_area();
    let work = area(wa.position, wa.size.width, wa.size.height).scaled(u);
    // Logical pixels to screen units on this monitor.
    let k = scale / u;
    let margin = platform::MARGIN * k;
    let w = platform::WIDTH * k;
    let h = (height * k).min(work.h - 2.0 * margin).max(80.0 * k);
    let (x, y) = place(icon, (w, h), monitor, work, platform::GAP * k, margin);
    let position: Position = if platform::LOGICAL {
        LogicalPosition::new(x, y).into()
    } else {
        PhysicalPosition::new(x.round() as i32, y.round() as i32).into()
    };
    let size: Size = if platform::LOGICAL {
        tauri::LogicalSize::new(w, h).into()
    } else {
        tauri::PhysicalSize::new(w.round() as u32, h.round() as u32).into()
    };
    // Moving first lets Windows apply the new display's scale before the size is set; then move again, exactly.
    let _ = window.set_position(position);
    let _ = window.set_size(size);
    let _ = window.set_position(position);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(x: f64, y: f64, w: f64, h: f64) -> Area {
        Area { x, y, w, h }
    }

    #[test]
    fn the_macos_popover_hangs_under_its_menu_bar_icon() {
        let monitor = a(0.0, 0.0, 1512.0, 982.0);
        let work = a(0.0, 25.0, 1512.0, 957.0);
        let icon = a(1200.0, 0.0, 30.0, 24.0);
        let (x, y) = place(icon, (320.0, 400.0), monitor, work, 6.0, 8.0);
        assert_eq!((x, y), (1215.0 - 160.0, 31.0));
    }

    #[test]
    fn a_popover_near_the_screen_edge_stays_inside_the_work_area() {
        let monitor = a(0.0, 0.0, 1512.0, 982.0);
        let work = a(0.0, 25.0, 1512.0, 957.0);
        let icon = a(1490.0, 0.0, 22.0, 24.0);
        let (x, _) = place(icon, (320.0, 400.0), monitor, work, 6.0, 8.0);
        assert_eq!(x, 1512.0 - 8.0 - 320.0);
    }

    #[test]
    fn the_windows_flyout_sits_above_a_bottom_taskbar() {
        let monitor = a(0.0, 0.0, 1920.0, 1080.0);
        let work = a(0.0, 0.0, 1920.0, 1032.0);
        let icon = a(1700.0, 1040.0, 32.0, 32.0);
        let (x, y) = place(icon, (360.0, 420.0), monitor, work, 12.0, 12.0);
        assert_eq!((x, y), (1716.0 - 180.0, 1032.0 - 12.0 - 420.0));
    }

    #[test]
    fn an_arabic_taskbar_with_its_icons_on_the_left_gets_the_flyout_on_the_left() {
        let monitor = a(0.0, 0.0, 1920.0, 1080.0);
        let work = a(0.0, 0.0, 1920.0, 1032.0);
        let icon = a(90.0, 1040.0, 32.0, 32.0);
        let (x, _) = place(icon, (360.0, 420.0), monitor, work, 12.0, 12.0);
        assert_eq!(x, 12.0);
    }

    #[test]
    fn a_taskbar_on_the_left_puts_the_flyout_beside_it() {
        let monitor = a(0.0, 0.0, 1920.0, 1080.0);
        let work = a(48.0, 0.0, 1872.0, 1080.0);
        let icon = a(8.0, 1000.0, 32.0, 32.0);
        assert_eq!(edge_of(icon, monitor, work), Edge::Left);
        let (x, y) = place(icon, (360.0, 420.0), monitor, work, 12.0, 12.0);
        assert_eq!((x, y), (60.0, 1080.0 - 12.0 - 420.0));
    }

    #[test]
    fn a_taskbar_that_hides_itself_still_gets_the_flyout_above_the_icon() {
        let monitor = a(0.0, 0.0, 1920.0, 1080.0);
        let icon = a(1700.0, 1040.0, 32.0, 32.0);
        let (_, y) = place(icon, (360.0, 420.0), monitor, monitor, 12.0, 12.0);
        assert_eq!(y, 1040.0 - 12.0 - 420.0);
    }

    #[test]
    fn a_second_display_left_of_the_first_keeps_its_own_coordinates() {
        let monitor = a(-1920.0, 0.0, 1920.0, 1080.0);
        let work = a(-1920.0, 0.0, 1920.0, 1032.0);
        let icon = a(-250.0, 1040.0, 32.0, 32.0);
        let (x, y) = place(icon, (360.0, 420.0), monitor, work, 12.0, 12.0);
        assert_eq!((x, y), (-234.0 - 180.0, 600.0));
    }

    #[test]
    fn a_popover_taller_than_the_screen_starts_at_the_work_area_edge() {
        let monitor = a(0.0, 0.0, 800.0, 600.0);
        let work = a(0.0, 0.0, 800.0, 560.0);
        let icon = a(10.0, 300.0, 32.0, 32.0);
        let (_, y) = place(icon, (360.0, 900.0), monitor, work, 12.0, 12.0);
        assert_eq!(y, 12.0);
    }

    #[test]
    fn an_icon_on_a_1x_display_beside_a_2x_display_is_placed_on_its_own_display() {
        // macOS, in points: the 2x built-in display, and a 1x display to its right.
        let screens = [a(0.0, 0.0, 1512.0, 982.0), a(1512.0, 0.0, 1920.0, 1080.0)];
        // The tray measures the icon in its own display's pixels: here points times 1.
        let icon = a(2000.0, 0.0, 30.0, 24.0);
        // Divided by the built-in display's scale, it would look like an icon on the built-in display.
        assert_eq!(monitor_at(icon.scaled(2.0), &screens), Some(0));
        // Divided by its own display's scale (tray_macos::icon_scale), it is on the 1x display.
        assert_eq!(monitor_at(icon.scaled(1.0), &screens), Some(1));
    }

    #[test]
    fn an_icon_off_every_screen_has_no_screen() {
        let screens = [a(0.0, 0.0, 1512.0, 982.0)];
        assert_eq!(monitor_at(a(3000.0, 0.0, 30.0, 24.0), &screens), None);
    }

    #[test]
    fn a_tray_with_no_rectangle_places_by_the_pointer() {
        let rect = tauri::Rect {
            position: PhysicalPosition::new(0.0, 0.0).into(),
            size: tauri::PhysicalSize::new(0.0, 0.0).into(),
        };
        let area = icon_area(&rect, PhysicalPosition::new(500.0, 20.0));
        assert_eq!(area, a(500.0, 20.0, 1.0, 1.0));
    }
}
