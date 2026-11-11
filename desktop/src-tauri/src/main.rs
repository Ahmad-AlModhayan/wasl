//! Wasl's tray app: a window and a tray icon in front of `wasld`, which does the syncing (FR-B5,
//! FR-E4). Quitting the tray leaves the daemon running.

// No console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(target_os = "macos")]
mod chrome_macos;
#[cfg(windows)]
mod chrome_windows;
mod commands;
mod confirm;
mod daemon;
mod menu;
mod notify;
mod popover;
mod settings_window;
mod tray;
mod update;
#[cfg(target_os = "macos")]
mod tray_macos;
#[cfg(not(target_os = "macos"))]
mod tray_windows;

use daemon::Daemon;
use std::sync::Arc;
use tauri::{Manager, WindowEvent};

/// Brings the window forward (tray click, "Open Wasl", a second launch).
pub fn show_main(app: &tauri::AppHandle) {
    // macOS: a Dock icon only while the window is open, like other menu bar apps.
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn main() {
    // Started by wasld to confirm an agent's changes: a dialog, then exit (before the window, the
    // tray and the single-instance check, which would hand the launch to the running app).
    if std::env::args().nth(1).as_deref() == Some("--confirm-agent") {
        std::process::exit(confirm::run());
    }
    let daemon = Arc::new(Daemon::new());
    tauri::Builder::default()
        // First, so a second launch only focuses the running one.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_dialog::init())
        // Notifications (#71), sent from Rust only: no window is granted the plugin's commands.
        .plugin(tauri_plugin_notification::init())
        .manage(daemon.clone())
        .manage(popover::Popover::default())
        .manage(commands::BinUndo::default())
        .manage(notify::Notifier::default())
        .manage(commands::OpenedFolder::default())
        .setup(move |app| {
            let handle = app.handle().clone();
            // macOS: the window with its overlay title bar, then the menu bar (#61).
            #[cfg(target_os = "macos")]
            {
                chrome_macos::create_main_window(&handle)?;
                menu::install(&handle)?;
            }
            notify::listen(&handle);
            let t = tray::build(&handle)?;
            app.manage(t);
            tray::refresh(&handle);
            // Windows: Snap Layouts on the title bar Wasl draws itself.
            #[cfg(windows)]
            chrome_windows::setup(&handle);
            // Started at login with --background: stay in the tray (on macOS, the menu bar).
            if std::env::args().any(|a| a == "--background") {
                #[cfg(target_os = "macos")]
                tray_macos::window_hidden(&handle, "main");
            } else {
                show_main(&handle);
            }
            tauri::async_runtime::spawn(daemon::run(handle, daemon));
            Ok(())
        })
        .on_window_event(|window, event| {
            // The tray's popover hides on blur and Esc (#69).
            if window.label() == popover::LABEL {
                popover::on_window_event(window, event);
                return;
            }
            // The Settings window (#68) just closes. If the main window was closed to the menu bar meanwhile, macOS
            // goes back to the menu bar only once Settings is gone.
            if window.label() == settings_window::LABEL {
                #[cfg(target_os = "macos")]
                if matches!(event, WindowEvent::Destroyed) {
                    tray_macos::window_hidden(window.app_handle(), settings_window::LABEL);
                }
                return;
            }
            // Closing the main window hides it; the tray stays.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                // macOS: no Dock icon or menu bar once no window shows (tray_macos::window_hidden).
                #[cfg(target_os = "macos")]
                tray_macos::window_hidden(window.app_handle(), "main");
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::open_site_page,
            update::check_update,
            update::open_download,
            commands::daemon_state,
            commands::start_daemon,
            commands::set_tray_text,
            commands::set_start_at_login,
            commands::status,
            commands::pairs,
            commands::create_pair,
            commands::update_pair,
            commands::delete_pair,
            commands::set_paused,
            commands::show_pair_folder,
            commands::preview,
            commands::sync,
            commands::sync_all,
            commands::cancel,
            commands::approve_once,
            commands::blocked_plan,
            commands::deletion_review,
            commands::restore_deletions,
            commands::runs,
            commands::run_events,
            commands::open_conflicts,
            commands::conflict_details,
            commands::resolve_conflict,
            commands::undo_conflict,
            commands::agent_changesets,
            commands::agent_changeset,
            commands::agent_diff,
            commands::agent_accept,
            commands::agent_revert,
            commands::agent_end,
            commands::agent_setup,
            commands::bin_entries,
            commands::restore_bin,
            commands::undo_bin_restore,
            commands::delete_bin_forever,
            commands::empty_bin,
            commands::accounts,
            commands::add_account,
            commands::remove_account,
            commands::drive_folders,
            commands::resolve_drive_path,
            commands::devices,
            commands::remove_device,
            commands::connect_device,
            commands::share_pair,
            commands::share_code,
            commands::unshare_pair,
            commands::sharing,
            commands::share_with,
            commands::offers,
            commands::decline_offer,
            commands::accept_offer,
            commands::keep_device,
            commands::invite_device,
            commands::join_device,
            commands::pairing_status,
            commands::confirm_pairing,
            commands::cancel_pairing,
            commands::views,
            commands::add_view,
            commands::remove_view,
            commands::set_view_enabled,
            commands::set_view_cache_limit,
            commands::refresh_view,
            commands::pin_path,
            commands::unpin_path,
            commands::vaults,
            commands::create_vault,
            commands::unlock_vault,
            commands::lock_vault,
            commands::setting,
            commands::set_setting,
            // Window chrome and the menu bar (#61).
            commands::set_menu_text,
            commands::window_chrome,
            commands::title_bar_double_click,
            // The tray's popover and flyout (#69).
            commands::tray_hide,
            commands::tray_fit,
            commands::tray_open,
            commands::tray_menu,
            commands::set_tray_visible,
            // Folder drop (#71).
            commands::is_folder,
            commands::take_opened_folder,
            // The Settings window on macOS and the system accent (#68).
            settings_window::open_settings,
            settings_window::system_accent,
        ])
        .build(tauri::generate_context!())
        .expect("Wasl could not start its window")
        .run(|_app, _event| {
            #[cfg(target_os = "macos")]
            match _event {
                // Clicking the Dock icon (or opening the app again from Finder) shows the window.
                tauri::RunEvent::Reopen { .. } => show_main(_app),
                // A folder dropped on the Dock icon (Info.plist declares folders): New pair with it (#71).
                tauri::RunEvent::Opened { urls } => opened(_app, &urls),
                _ => {}
            }
        });
}

/// macOS: a folder dropped on the Dock icon, or opened with Wasl from Finder. The window shows and opens New pair
/// with it; the page takes it with `take_opened_folder`, on `app://opened-folder` or, if it wasn't listening yet,
/// when it starts. Files are ignored: only folders make pairs.
#[cfg(target_os = "macos")]
fn opened(app: &tauri::AppHandle, urls: &[tauri::Url]) {
    let Some(folder) = urls.iter().filter_map(|u| u.to_file_path().ok()).find(|p| p.is_dir()) else {
        return;
    };
    if let Some(state) = app.try_state::<commands::OpenedFolder>() {
        *state.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(folder.to_string_lossy().into_owned());
    }
    show_main(app);
    let _ = tauri::Emitter::emit_to(app, "main", "app://opened-folder", ());
}
