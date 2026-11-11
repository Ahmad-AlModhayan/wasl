//! The Windows 11 title bar's native half (#62). The window has no frame (`tauri.windows.conf.json`), so the page
//! draws the 48px bar and its caption buttons (`src/chrome/windows.ts`). Two things only native code can do:
//!
//! - Snap Layouts. Windows opens the flyout when the window under the pointer answers `WM_NCHITTEST` with
//!   `HTMAXBUTTON`. The page sits in WebView2's own child window, which belongs to another process, so its hit test
//!   can't be answered from here (tauri-apps/tauri#4531). Instead a small transparent child window of ours lies over
//!   the page's maximize button and answers `HTMAXBUTTON`. It swallows that button's mouse input, so it tells the
//!   page about hover and press (for the button's look) and maximizes or restores on click itself.
//! - The system menu at the pointer when the bar is right-clicked.
//!
//! The page and this module talk through events: the page says where its maximize button is (`chrome://snap-area`,
//! physical pixels in the window's client area, again whenever it moves) and where to open the system menu
//! (`chrome://system-menu`: a right-click on the bar, or Alt+Space, which WebView2 doesn't pass on); this module sends
//! `chrome://snap` with "enter", "leave", "press" or "release".

// Win32 calls throughout; each unsafe block says why it is sound. This is the only module that allows it.
#![allow(unsafe_code)]

use serde::Deserialize;
use std::ptr::{null, null_mut};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use tauri::{AppHandle, Emitter, Listener, Manager};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{ClientToScreen, GetStockObject, NULL_BRUSH};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, EnableMenuItem, GetClientRect, GetSystemMenu, HTMAXBUTTON, HWND_TOP, IDC_ARROW,
    IsZoomed, LoadCursorW, MF_BYCOMMAND, MF_ENABLED, MF_GRAYED, PostMessageW, RegisterClassW, SC_CLOSE, SC_MAXIMIZE,
    SC_MINIMIZE, SC_MOVE, SC_RESTORE, SC_SIZE, SM_CYFRAME, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    SetMenuDefaultItem, SetWindowPos, SetWindowTextW, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTALIGN, TPM_RIGHTBUTTON,
    TrackPopupMenu, WM_APP, WM_CAPTURECHANGED, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCHITTEST, WM_NCLBUTTONDBLCLK,
    WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE, WM_SYSCOMMAND, WNDCLASSW, WS_CHILD,
    WS_CLIPSIBLINGS, WS_EX_LAYERED, WS_EX_NOREDIRECTIONBITMAP,
};

const SNAP_AREA: &str = "chrome://snap-area";
const SYSTEM_MENU: &str = "chrome://system-menu";
const SNAP: &str = "chrome://snap";

/// Posted to the overlay to open the system menu from the window's own message loop, not from inside the WebView2
/// callback that delivered the page's event: the menu runs a modal loop. wParam and lParam carry x and y.
const WM_SYSTEM_MENU: u32 = WM_APP + 1;

/// Where the page's maximize button is, in physical pixels from the client area's top-left corner; its name
/// ("Maximize" or "Restore" in the window's language) for Narrator, which reaches the overlay before the page; and
/// whether the window is maximized, which moves the button (and which the overlay needs, below).
#[derive(Deserialize)]
struct SnapArea {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    label: String,
    maximized: bool,
}

/// Where to open the system menu, in physical pixels from the client area's top-left corner. In Arabic (`rtl`) the
/// menu opens to the left of that point.
#[derive(Deserialize)]
struct MenuAt {
    x: i32,
    y: i32,
    rtl: bool,
}

static APP: OnceLock<AppHandle> = OnceLock::new();
// Window handles are raw pointers, which statics can't hold; kept as integers. 0 until setup.
static WINDOW: AtomicIsize = AtomicIsize::new(0);
static OVERLAY: AtomicIsize = AtomicIsize::new(0);
static TRACKING: AtomicBool = AtomicBool::new(false);
/// Pressed on the button and not yet released; the pointer is captured meanwhile.
static PRESSED: AtomicBool = AtomicBool::new(false);
/// While pressed: whether the pointer is still over the button.
static INSIDE: AtomicBool = AtomicBool::new(false);
static MENU_RTL: AtomicBool = AtomicBool::new(false);

/// Lays the overlay over the main window and starts answering the page. If anything fails, the window keeps its
/// caption buttons without the Snap Layouts flyout.
pub fn setup(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else { return };
    let Ok(hwnd) = window.hwnd() else { return };
    let parent: HWND = hwnd.0;
    let overlay = create_overlay(parent);
    if overlay.is_null() {
        return;
    }
    let _ = APP.set(app.clone());
    WINDOW.store(parent as isize, Ordering::Relaxed);
    OVERLAY.store(overlay as isize, Ordering::Relaxed);

    // Win32 windows answer only on the thread that made them: the main one. (The page's events usually arrive there
    // already, and then run_on_main_thread runs the closure at once.)
    let handle = app.clone();
    app.listen_any(SNAP_AREA, move |e| {
        if let Ok(area) = serde_json::from_str::<SnapArea>(e.payload()) {
            let _ = handle.run_on_main_thread(move || place(area));
        }
    });
    app.listen_any(SYSTEM_MENU, move |e| {
        if let Ok(at) = serde_json::from_str::<MenuAt>(e.payload()) {
            MENU_RTL.store(at.rtl, Ordering::Relaxed);
            // SAFETY: posting to our own overlay window; any thread may post. A failed post only loses this menu.
            unsafe { PostMessageW(overlay_hwnd(), WM_SYSTEM_MENU, at.x as WPARAM, at.y as LPARAM) };
        }
    });
}

fn overlay_hwnd() -> HWND {
    OVERLAY.load(Ordering::Relaxed) as HWND
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn create_overlay(parent: HWND) -> HWND {
    let class = wide("WaslSnapOverlay");
    // SAFETY: plain Win32 calls with valid arguments; `class` outlives them (the system copies the class name).
    // If the class is already registered, RegisterClassW fails harmlessly and CreateWindowExW uses the old one.
    // Windows Terminal's styles for its caption overlay: layered with no redirection bitmap, so the window has no
    // surface at all (nothing to paint black or punch through the page) yet still answers hit tests. A child window
    // may be layered only when the app declares Windows 8 or later in its manifest (windows-app-manifest.xml, set in
    // build.rs). Should that ever fail, a plain child that never paints (null brush) is the fallback.
    unsafe {
        let instance = GetModuleHandleW(null());
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(overlay_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: null_mut(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: GetStockObject(NULL_BRUSH),
            lpszMenuName: null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
        // Hidden and zero-sized until the page says where its button is.
        let style = WS_CHILD | WS_CLIPSIBLINGS;
        let layered = WS_EX_LAYERED | WS_EX_NOREDIRECTIONBITMAP;
        let hwnd = CreateWindowExW(layered, class.as_ptr(), null(), style, 0, 0, 0, 0, parent, null_mut(), instance, null());
        if !hwnd.is_null() {
            return hwnd;
        }
        CreateWindowExW(0, class.as_ptr(), null(), style, 0, 0, 0, 0, parent, null_mut(), instance, null())
    }
}

fn place(area: SnapArea) {
    let overlay = overlay_hwnd();
    let window = WINDOW.load(Ordering::Relaxed) as HWND;
    // SAFETY: `window` is the app's live main window.
    let dpi = unsafe { GetDpiForWindow(window) };
    // A window that isn't maximized resizes from its top edge (tao answers HTTOP for SM_CYFRAME pixels there); the
    // overlay starts below that edge so it doesn't take it over the maximize button.
    // SAFETY: a plain query with a valid metric and DPI.
    let edge = if area.maximized { 0 } else { unsafe { GetSystemMetricsForDpi(SM_CYFRAME, dpi) } };
    let (y, height) = (area.y.max(edge), area.height - (edge - area.y).max(0));
    let shown = area.width > 0 && height > 0;
    let flags = SWP_NOACTIVATE | if shown { SWP_SHOWWINDOW } else { SWP_HIDEWINDOW };
    let label = wide(&area.label);
    // SAFETY: `overlay` is our own live child window (set once in setup, never destroyed before the app exits);
    // `label` is a NUL-terminated buffer that outlives the call. HWND_TOP keeps it above WebView2's window.
    unsafe {
        SetWindowPos(overlay, HWND_TOP, area.x, y, area.width, height, flags);
        SetWindowTextW(overlay, label.as_ptr());
    }
}

/// Tells the page. Called from the window procedure on the main thread: emit_to only queues a script for WebView2
/// to run later, so nothing re-enters this procedure.
fn tell_page(what: &'static str) {
    if let Some(app) = APP.get() {
        let _ = app.emit_to("main", SNAP, what);
    }
}

/// Asks for one WM_NCMOUSELEAVE when the pointer leaves the button.
fn track_leave(hwnd: HWND) {
    TRACKING.store(true, Ordering::Relaxed);
    let mut track = TRACKMOUSEEVENT {
        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE | TME_NONCLIENT,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    // SAFETY: `track` is a valid, initialised struct for our own window.
    unsafe { TrackMouseEvent(&mut track) };
}

/// Whether a point from a captured mouse message (client coordinates, which may be negative) is over the button.
fn over_button(hwnd: HWND, lparam: LPARAM) -> bool {
    let (x, y) = ((lparam & 0xffff) as i16 as i32, ((lparam >> 16) & 0xffff) as i16 as i32);
    let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: our own window and a valid out-parameter.
    unsafe { GetClientRect(hwnd, &mut rect) };
    x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => return HTMAXBUTTON as LRESULT,
        WM_NCMOUSEMOVE => {
            if !PRESSED.load(Ordering::Relaxed) && !TRACKING.load(Ordering::Relaxed) {
                track_leave(hwnd);
                tell_page("enter");
            }
            return 0;
        }
        WM_NCMOUSELEAVE => {
            TRACKING.store(false, Ordering::Relaxed);
            // While pressed the pointer is captured and the press decides what the page shows.
            if !PRESSED.load(Ordering::Relaxed) {
                tell_page("leave");
            }
            return 0;
        }
        // Pressing captures the pointer, as a native button does: dragging off shows the button at rest, dragging
        // back on shows it pressed again, and only a release over it clicks. A quick second click arrives as a
        // double-click and presses the button again, as on a native frame.
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK => {
            PRESSED.store(true, Ordering::Relaxed);
            INSIDE.store(true, Ordering::Relaxed);
            // SAFETY: capturing the mouse for our own window, on its thread.
            unsafe { SetCapture(hwnd) };
            tell_page("press");
            return 0;
        }
        WM_MOUSEMOVE if PRESSED.load(Ordering::Relaxed) => {
            let inside = over_button(hwnd, lparam);
            if INSIDE.swap(inside, Ordering::Relaxed) != inside {
                tell_page(if inside { "press" } else { "leave" });
            }
            return 0;
        }
        // Captured, the release arrives as a client message; uncaptured (capture refused), as a non-client one.
        WM_LBUTTONUP | WM_NCLBUTTONUP => {
            if PRESSED.swap(false, Ordering::Relaxed) {
                let inside = msg == WM_NCLBUTTONUP || over_button(hwnd, lparam);
                // SAFETY: releasing our own capture; WM_CAPTURECHANGED then finds PRESSED already cleared.
                unsafe { ReleaseCapture() };
                if inside {
                    tell_page("release");
                    track_leave(hwnd); // a leave arrives at once if the pointer is no longer over the button
                    toggle_maximize();
                } else {
                    TRACKING.store(false, Ordering::Relaxed);
                    tell_page("leave");
                }
            }
            return 0;
        }
        // Capture taken away mid-press (another window, Alt+Tab): no click.
        WM_CAPTURECHANGED => {
            if PRESSED.swap(false, Ordering::Relaxed) {
                TRACKING.store(false, Ordering::Relaxed);
                tell_page("leave");
            }
            return 0;
        }
        WM_SYSTEM_MENU => {
            system_menu(wparam as i32, lparam as i32, MENU_RTL.load(Ordering::Relaxed));
            return 0;
        }
        _ => {}
    }
    // Not passing the button messages above on matters: DefWindowProcW would track and paint a classic button.
    // SAFETY: forwarding a message to the default procedure with the arguments we were given.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// What a native maximize button does: a system command, so Windows animates it and remembers the restored size.
fn toggle_maximize() {
    let window = WINDOW.load(Ordering::Relaxed) as HWND;
    // SAFETY: `window` is the app's main window, alive while the app runs; posting is asynchronous.
    unsafe {
        let command = if IsZoomed(window) != 0 { SC_RESTORE } else { SC_MAXIMIZE };
        PostMessageW(window, WM_SYSCOMMAND, command as WPARAM, 0);
    }
}

/// The window's system menu at a point in the client area, with the items a native frame would grey out.
fn system_menu(x: i32, y: i32, rtl: bool) {
    let window = WINDOW.load(Ordering::Relaxed) as HWND;
    // SAFETY: `window` is the app's live main window and `point` a valid out-parameter. GetSystemMenu(.., FALSE)
    // returns the window's own menu, which Windows owns and we only adjust. TrackPopupMenu runs from the overlay's
    // window procedure on the main thread, which owns the window, and returns the chosen command, which we post back
    // as a native frame does.
    unsafe {
        let mut point = POINT { x, y };
        ClientToScreen(window, &mut point);
        let menu = GetSystemMenu(window, 0);
        if menu.is_null() {
            return;
        }
        let maximized = IsZoomed(window) != 0;
        let state = |on: bool| MF_BYCOMMAND | if on { MF_ENABLED } else { MF_GRAYED };
        EnableMenuItem(menu, SC_RESTORE, state(maximized));
        EnableMenuItem(menu, SC_MOVE, state(!maximized));
        EnableMenuItem(menu, SC_SIZE, state(!maximized));
        EnableMenuItem(menu, SC_MINIMIZE, state(true));
        EnableMenuItem(menu, SC_MAXIMIZE, state(!maximized));
        EnableMenuItem(menu, SC_CLOSE, state(true));
        SetMenuDefaultItem(menu, SC_CLOSE, 0);
        let align = if rtl { TPM_RIGHTALIGN } else { TPM_LEFTALIGN };
        let command = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | align, point.x, point.y, 0, window, null());
        if command != 0 {
            PostMessageW(window, WM_SYSCOMMAND, command as WPARAM, 0);
        }
    }
}

/// Makes a popup menu's first item its default, drawn bold, as Windows draws a tray menu's "Open" (#69).
pub fn bold_first_item(hmenu: isize) {
    // SAFETY: `hmenu` is a live menu handle from the tray's menu, valid while that menu is; SetMenuDefaultItem only
    // sets a flag on one of its items (by position, the third argument).
    unsafe { SetMenuDefaultItem(hmenu as _, 0, 1) };
}

/// The person's Windows accent colour as `#rrggbb`, for Settings' "Use my Windows accent" (#68). WebView2 has no CSS
/// `AccentColor`, so the page gets it from here, on each call: the page asks again whenever the window regains focus.
/// Windows keeps it as `AccentColorMenu`, a DWORD in 0xAABBGGRR order, the same colour UISettings reports as the
/// accent. That value isn't documented; the documented source, `UISettings::GetColorValue(UIColorType::Accent)`,
/// needs WinRT (the `windows` crate), which this app doesn't otherwise pull in. If the read fails the page keeps Wasl
/// navy and Settings says so, so a missing value never leaves the accent unresolved.
pub fn accent_color() -> Option<String> {
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent");
    let name = wide("AccentColorMenu");
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: both strings are NUL-terminated and outlive the call; `value` is a u32 and `size` says so, and
    // RRF_RT_REG_DWORD makes the call fail rather than write anything that isn't a DWORD.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&raw mut value).cast(),
            &raw mut size,
        )
    };
    (status == 0).then(|| format!("#{:02x}{:02x}{:02x}", value & 0xff, (value >> 8) & 0xff, (value >> 16) & 0xff))
}
