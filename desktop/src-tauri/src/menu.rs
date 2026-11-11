//! The macOS menu bar (#61, design D7): Wasl, File, Edit, View, Pair, Window and Help with the standard
//! shortcuts. Items Wasl handles itself send `menu://action` with their id to the window, which routes it; the
//! window says which ids it handles (`enabled`) and sends the words in the app's language (`set_menu_text`).
//! English is used until it does. The menu is built once; later words and states are set on the items.
//! Windows and Linux have no menu bar: `install` is only called on macOS.

// Only macOS calls `install`; elsewhere `set_text` finds no state and returns.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::menu::{
    AboutMetadata, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu,
};
use tauri::{AppHandle, Emitter, Manager, Wry};

/// The event the window listens to; the payload is an action id (`ACTIONS`).
pub const EVENT: &str = "menu://action";

/// Ids of the menu items the window handles, with the English word and the shortcut.
/// Menu ids carry a `menu:` prefix so they never meet the tray menu's ids in the app-wide menu events.
const ACTIONS: &[(&str, &str, Option<&str>)] = &[
    ("settings", "Settings…", Some("Cmd+Comma")),
    ("new_pair", "New Pair…", Some("Cmd+N")),
    ("toggle_sidebar", "Hide Sidebar", Some("Ctrl+Cmd+S")),
    ("toggle_inspector", "Show Inspector", Some("Alt+Cmd+I")),
    ("go_pairs", "All Pairs", Some("Cmd+1")),
    ("go_views", "Views", Some("Cmd+2")),
    ("go_devices", "Devices", Some("Cmd+3")),
    ("go_problems", "Problems", Some("Cmd+4")),
    ("go_agents", "Agent Changes", Some("Cmd+5")),
    ("go_history", "History", Some("Cmd+6")),
    ("go_bin", "Recycle Bin", Some("Cmd+7")),
    ("sync_now", "Sync Now", Some("Cmd+R")),
    ("toggle_pause", "Pause", Some("Alt+Cmd+P")),
    ("show_in_finder", "Show in Finder", Some("Shift+Cmd+R")),
];

/// English words for the menus' titles and the system items.
const WORDS: &[(&str, &str)] = &[
    ("file", "File"),
    ("edit", "Edit"),
    ("view", "View"),
    ("pair", "Pair"),
    ("window", "Window"),
    ("help", "Help"),
    ("about", "About Wasl"),
    ("services", "Services"),
    ("hide", "Hide Wasl"),
    ("hide_others", "Hide Others"),
    ("show_all", "Show All"),
    ("quit", "Quit Wasl"),
    ("close_window", "Close Window"),
    ("undo", "Undo"),
    ("redo", "Redo"),
    ("cut", "Cut"),
    ("copy", "Copy"),
    ("paste", "Paste"),
    ("select_all", "Select All"),
    ("full_screen", "Enter Full Screen"),
    ("minimize", "Minimize"),
    ("zoom", "Zoom"),
    ("bring_all_to_front", "Bring All to Front"),
];

/// The menu's words in the user's language, keyed like `ACTIONS` and `WORDS`, and the action ids the window
/// handles today (the rest stay disabled until a screen handles them).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuText {
    pub words: HashMap<String, String>,
    pub enabled: Vec<String>,
}

impl MenuText {
    fn word<'a>(&'a self, key: &str, english: &'a str) -> &'a str {
        self.words
            .get(key)
            .map(String::as_str)
            .filter(|w| !w.is_empty())
            .unwrap_or(english)
    }

    fn english_word(key: &str) -> &'static str {
        WORDS
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, w)| *w)
            .or_else(|| {
                ACTIONS
                    .iter()
                    .find(|(k, _, _)| *k == key)
                    .map(|(_, w, _)| *w)
            })
            .unwrap_or("")
    }

    fn get(&self, key: &str) -> &str {
        self.word(key, Self::english_word(key))
    }

    /// What the window handles before it says: the screens that existed before the menu bar. Not Sync Now,
    /// which needs the daemon and a pair.
    fn initial() -> Self {
        let enabled = [
            "settings",
            "new_pair",
            "go_pairs",
            "go_views",
            "go_devices",
            "go_problems",
            "go_agents",
            "go_history",
            "go_bin",
        ];
        MenuText {
            words: HashMap::new(),
            enabled: enabled.map(String::from).to_vec(),
        }
    }
}

/// The menu, built once, and handles to everything whose words or state change: new words and enabled
/// items are set in place, so NSApp's main menu is never swapped (not even while a menu is open).
struct MenuState {
    last: Mutex<MenuText>,
    submenus: Vec<(&'static str, Submenu<Wry>)>,
    system: Vec<(&'static str, PredefinedMenuItem<Wry>)>,
    actions: Vec<(&'static str, MenuItem<Wry>)>,
}

/// The action id for a menu event, if it is one of ours.
pub fn action_of(menu_id: &str) -> Option<&'static str> {
    let id = menu_id.strip_prefix("menu:")?;
    ACTIONS
        .iter()
        .find(|(k, _, _)| *k == id)
        .map(|(k, _, _)| *k)
}

/// Builds the menu's parts and keeps a handle to each.
struct Parts<'a> {
    app: &'a AppHandle,
    text: &'a MenuText,
    submenus: Vec<(&'static str, Submenu<Wry>)>,
    system: Vec<(&'static str, PredefinedMenuItem<Wry>)>,
    actions: Vec<(&'static str, MenuItem<Wry>)>,
}

impl Parts<'_> {
    fn action(&mut self, id: &'static str) -> tauri::Result<MenuItem<Wry>> {
        let shortcut = ACTIONS
            .iter()
            .find(|(k, _, _)| *k == id)
            .and_then(|(_, _, s)| *s);
        let enabled = self.text.enabled.iter().any(|e| e == id);
        let item = MenuItem::with_id(
            self.app,
            format!("menu:{id}"),
            self.text.get(id),
            enabled,
            shortcut,
        )?;
        self.actions.push((id, item.clone()));
        Ok(item)
    }

    /// A system item macOS draws and handles, with Wasl's word for it.
    fn system(
        &mut self,
        key: &'static str,
        make: fn(&AppHandle, Option<&str>) -> tauri::Result<PredefinedMenuItem<Wry>>,
    ) -> tauri::Result<PredefinedMenuItem<Wry>> {
        let item = make(self.app, Some(self.text.get(key)))?;
        self.system.push((key, item.clone()));
        Ok(item)
    }

    fn sep(&self) -> tauri::Result<PredefinedMenuItem<Wry>> {
        PredefinedMenuItem::separator(self.app)
    }

    fn submenu(
        &mut self,
        key: &'static str,
        items: &[&dyn IsMenuItem<Wry>],
    ) -> tauri::Result<Submenu<Wry>> {
        let submenu = Submenu::with_items(self.app, self.text.get(key), true, items)?;
        self.submenus.push((key, submenu.clone()));
        Ok(submenu)
    }
}

fn build(app: &AppHandle, text: &MenuText) -> tauri::Result<(Menu<Wry>, MenuState)> {
    let mut p = Parts {
        app,
        text,
        submenus: Vec::new(),
        system: Vec::new(),
        actions: Vec::new(),
    };
    let about_metadata = AboutMetadata {
        name: Some("Wasl".into()),
        version: Some(app.package_info().version.to_string()),
        ..Default::default()
    };
    let about = PredefinedMenuItem::about(app, Some(text.get("about")), Some(about_metadata))?;
    p.system.push(("about", about.clone()));

    let wasl = Submenu::with_items(
        app,
        "Wasl",
        true,
        &[
            &about,
            &p.sep()?,
            &p.action("settings")?,
            &p.sep()?,
            &p.system("services", PredefinedMenuItem::services)?,
            &p.sep()?,
            &p.system("hide", PredefinedMenuItem::hide)?,
            &p.system("hide_others", PredefinedMenuItem::hide_others)?,
            &p.system("show_all", PredefinedMenuItem::show_all)?,
            &p.sep()?,
            // Quits the window app only; wasld keeps syncing.
            &p.system("quit", PredefinedMenuItem::quit)?,
        ],
    )?;
    let items = [
        &p.action("new_pair")? as &dyn IsMenuItem<Wry>,
        &p.sep()?,
        // Closing hides the window (main.rs); syncing goes on from the menu bar.
        &p.system("close_window", PredefinedMenuItem::close_window)?,
    ];
    let file = p.submenu("file", &items)?;
    let items = [
        &p.system("undo", PredefinedMenuItem::undo)? as &dyn IsMenuItem<Wry>,
        &p.system("redo", PredefinedMenuItem::redo)?,
        &p.sep()?,
        &p.system("cut", PredefinedMenuItem::cut)?,
        &p.system("copy", PredefinedMenuItem::copy)?,
        &p.system("paste", PredefinedMenuItem::paste)?,
        &p.system("select_all", PredefinedMenuItem::select_all)?,
    ];
    let edit = p.submenu("edit", &items)?;
    let items = [
        &p.action("toggle_sidebar")? as &dyn IsMenuItem<Wry>,
        &p.action("toggle_inspector")?,
        &p.sep()?,
        &p.action("go_pairs")?,
        &p.action("go_views")?,
        &p.action("go_devices")?,
        &p.action("go_problems")?,
        &p.action("go_agents")?,
        &p.action("go_history")?,
        &p.action("go_bin")?,
        &p.sep()?,
        &p.system("full_screen", PredefinedMenuItem::fullscreen)?,
    ];
    let view = p.submenu("view", &items)?;
    let items = [
        &p.action("sync_now")? as &dyn IsMenuItem<Wry>,
        &p.action("toggle_pause")?,
        &p.sep()?,
        &p.action("show_in_finder")?,
    ];
    let pair = p.submenu("pair", &items)?;
    let items = [
        &p.system("minimize", PredefinedMenuItem::minimize)? as &dyn IsMenuItem<Wry>,
        &p.system("zoom", PredefinedMenuItem::maximize)?,
        &p.sep()?,
        &p.system("bring_all_to_front", PredefinedMenuItem::bring_all_to_front)?,
    ];
    let window = p.submenu("window", &items)?;
    // macOS adds its search field to the Help menu (set as NSApp's help menu in `install`).
    let help = p.submenu("help", &[])?;

    let menu = Menu::with_items(app, &[&wasl, &file, &edit, &view, &pair, &window, &help])?;
    let state = MenuState {
        last: Mutex::new(text.clone()),
        submenus: p.submenus,
        system: p.system,
        actions: p.actions,
    };
    Ok((menu, state))
}

/// Puts the menu bar up in English and sends our items' clicks to the window.
pub fn install(app: &AppHandle) -> tauri::Result<()> {
    let (menu, state) = build(app, &MenuText::initial())?;
    app.set_menu(menu)?;
    // Only now: muda finds these submenus under NSApp's main menu, which they joined in `set_menu`.
    #[cfg(target_os = "macos")]
    for (key, submenu) in &state.submenus {
        match *key {
            "window" => submenu.set_as_windows_menu_for_nsapp()?,
            "help" => submenu.set_as_help_menu_for_nsapp()?,
            _ => {}
        }
    }
    app.manage(state);
    app.on_menu_event(on_menu);
    Ok(())
}

fn on_menu(app: &AppHandle, event: MenuEvent) {
    let Some(action) = action_of(event.id.as_ref()) else {
        return;
    };
    // A menu shortcut can arrive while the main window is hidden (closed to the menu bar, with Settings key):
    // commands that act in it bring it forward first, so a sheet never opens out of sight. Settings opens its own
    // window, and Sync Now and Pause need no window.
    if !matches!(action, "sync_now" | "toggle_pause" | "settings") {
        crate::show_main(app);
    }
    let _ = app.emit_to("main", EVENT, action);
}

/// New words or a new set of handled ids from the window: changes only the items that differ.
pub fn set_text(app: &AppHandle, text: MenuText) {
    let Some(state) = app.try_state::<MenuState>() else {
        return;
    };
    let mut last = state.last.lock().unwrap_or_else(|e| e.into_inner());
    if *last == text {
        return;
    }
    for (key, submenu) in &state.submenus {
        if last.get(key) != text.get(key) {
            let _ = submenu.set_text(text.get(key));
        }
    }
    for (key, item) in &state.system {
        if last.get(key) != text.get(key) {
            let _ = item.set_text(text.get(key));
        }
    }
    for (id, item) in &state.actions {
        if last.get(id) != text.get(id) {
            let _ = item.set_text(text.get(id));
        }
        let (was, now) = (
            last.enabled.iter().any(|e| e == id),
            text.enabled.iter().any(|e| e == id),
        );
        if was != now {
            let _ = item.set_enabled(now);
        }
    }
    *last = text;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_our_prefixed_ids_are_actions() {
        assert_eq!(action_of("menu:new_pair"), Some("new_pair"));
        assert_eq!(action_of("menu:go_bin"), Some("go_bin"));
        assert_eq!(action_of("sync"), None, "the tray's ids are not ours");
        assert_eq!(action_of("menu:nope"), None);
    }

    #[test]
    fn words_fall_back_to_english() {
        let mut text = MenuText::default();
        assert_eq!(text.get("quit"), "Quit Wasl");
        assert_eq!(text.get("go_pairs"), "All Pairs");
        text.words.insert("quit".into(), "إنهاء وصل".into());
        text.words.insert("file".into(), String::new());
        assert_eq!(text.get("quit"), "إنهاء وصل");
        assert_eq!(text.get("file"), "File", "an empty word falls back");
    }

    #[test]
    fn every_action_has_a_unique_id_and_shortcut() {
        let mut ids: Vec<_> = ACTIONS.iter().map(|(k, _, _)| *k).collect();
        let mut keys: Vec<_> = ACTIONS.iter().filter_map(|(_, _, s)| *s).collect();
        ids.sort_unstable();
        keys.sort_unstable();
        assert!(ids.windows(2).all(|p| p[0] != p[1]));
        assert!(keys.windows(2).all(|p| p[0] != p[1]));
        assert_eq!(keys.len(), ACTIONS.len());
    }
}
