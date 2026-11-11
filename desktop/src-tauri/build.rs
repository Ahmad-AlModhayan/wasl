fn main() {
    println!("cargo:rerun-if-changed=windows-app-manifest.xml");
    // Windows: Tauri's manifest plus a supportedOS list, so the Snap Layouts overlay (src/chrome_windows.rs) can be a
    // layered child window. Other platforms ignore it.
    let windows = tauri_build::WindowsAttributes::new().app_manifest(include_str!("windows-app-manifest.xml"));
    // The app's commands, each with an `allow-…` permission, so a window can call only those its capability grants
    // (capabilities/): the main window all of them, the tray's popover and the macOS Settings window the few their
    // pages use. Every command in main.rs's `generate_handler!` must be here and in capabilities/default.json;
    // src/commands.test.ts checks them all.
    let acl = tauri_build::AppManifest::new().commands(&[
        "app_info", "open_site_page", "check_update", "open_download", "daemon_state", "start_daemon", "set_tray_text", "set_start_at_login", "status", "pairs",
        "create_pair", "delete_pair", "set_paused", "show_pair_folder", "preview", "sync", "sync_all", "cancel",
        "approve_once", "blocked_plan", "runs", "run_events", "open_conflicts", "conflict_details", "resolve_conflict",
        "undo_conflict", "agent_changesets", "agent_changeset", "agent_diff", "agent_accept", "agent_revert",
        "agent_end", "agent_setup", "bin_entries", "restore_bin", "delete_bin_forever", "empty_bin", "accounts",
        "add_account", "remove_account", "drive_folders", "resolve_drive_path", "devices", "remove_device",
        "connect_device", "share_pair", "share_code", "unshare_pair", "sharing", "share_with", "offers",
        "decline_offer", "accept_offer", "keep_device", "invite_device", "join_device", "pairing_status",
        "confirm_pairing", "cancel_pairing", "views", "add_view", "remove_view", "set_view_enabled",
        "set_view_cache_limit", "refresh_view", "pin_path", "unpin_path", "vaults", "create_vault", "unlock_vault",
        "lock_vault", "setting", "set_setting", "set_menu_text", "window_chrome", "title_bar_double_click", "tray_hide",
        "tray_fit", "tray_open", "tray_menu", "set_tray_visible", "update_pair", "deletion_review",
        "restore_deletions", "undo_bin_restore", "open_settings", "system_accent", "is_folder", "take_opened_folder",
    ]);
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows).app_manifest(acl))
        .expect("tauri-build failed");
}
