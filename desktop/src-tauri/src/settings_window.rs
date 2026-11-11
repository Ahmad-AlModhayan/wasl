//! Settings (#68, design D5). On macOS, Settings is its own window on ⌘,: one instance, opened on demand from the
//! app menu's Settings… item (the page invokes `open_settings`), with minimise and zoom disabled. It loads the same
//! page as the main window with `?window=settings` (`src/main.ts`), which draws the toolbar tabs and the panes and
//! sizes the window to the pane. Windows shows Settings as a page inside the main window instead, so it never opens
//! this window; it asks for `system_accent` because WebView2 has no CSS `AccentColor`.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

/// The window's label; `capabilities/settings.json` grants it what it calls.
pub const LABEL: &str = "settings";

/// Opens the Settings window, or brings it forward if it is already open.
#[tauri::command]
pub fn open_settings(app: AppHandle) -> Result<(), String> {
    // The main window may be closed to the menu bar: Settings needs the Dock icon and the menu bar back.
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    if let Some(window) = app.get_webview_window(LABEL) {
        window.show().map_err(|e| e.to_string())?;
        return window.set_focus().map_err(|e| e.to_string());
    }
    let builder = WebviewWindowBuilder::new(&app, LABEL, WebviewUrl::App("index.html?window=settings".into()))
        .title("Settings")
        // The page resizes the window to its pane once it has drawn it.
        .inner_size(620.0, 440.0)
        .resizable(false)
        .minimizable(false)
        .maximizable(false)
        .center()
        .focused(true);
    // The tabs sit in the title bar's place, like the main window's toolbar (#61).
    #[cfg(target_os = "macos")]
    let builder = builder.title_bar_style(tauri::TitleBarStyle::Overlay).hidden_title(true);
    builder.build().map_err(|e| e.to_string())?;
    Ok(())
}

/// The person's accent colour as `#rrggbb` where the page can't read it itself (Windows); `None` elsewhere.
#[tauri::command]
pub fn system_accent() -> Option<String> {
    #[cfg(windows)]
    return crate::chrome_windows::accent_color();
    #[cfg(not(windows))]
    None
}
