//! What the window can ask: each command forwards to the daemon and returns its JSON. Errors reach
//! the window as `{kind, message}`.

use crate::daemon::{Conn, Daemon};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, PoisonError};
use tauri::{AppHandle, State};
use wasl_core::SyncMode;
use wasl_engine::{
    ChangesetDetails, ConflictChoice, ConflictDetails, ConflictUndo, DeletionReview, Preview, RestoreResult,
    RevertReport, RunOutcome, Trigger,
};
use wasl_ipc::{
    AcceptConfirmation, Account, Client, ClientError, DaemonStatus, DevicesInfo, DriveFolder, NewVault, NewView,
    OfferInfo, PairSharing, PairingStatus,
    Passphrase, Request, VaultInfo, View, ViewsInfo,
};
use wasl_store::{BinEntry, Changeset, ConflictRecord, Pair, RunEvent, RunRecord};

/// An error as the window sees it. `kind` is the daemon's error kind (`not_found`, `auth`, …) or
/// one of ours: `not_running`, `disconnected`, `untrusted`, `version`, `cli_missing`, `cli_failed`.
#[derive(Debug, Serialize)]
pub struct UiError {
    pub kind: String,
    pub message: String,
}

impl UiError {
    pub fn new(kind: &str, message: impl Into<String>) -> UiError {
        UiError {
            kind: kind.into(),
            message: message.into(),
        }
    }
}

impl From<ClientError> for UiError {
    fn from(e: ClientError) -> UiError {
        let kind = match &e {
            ClientError::NotRunning => "not_running".to_owned(),
            ClientError::Untrusted => "untrusted".to_owned(),
            ClientError::Version(_) => "version".to_owned(),
            ClientError::Io(_) => "disconnected".to_owned(),
            ClientError::Remote(r) => serde_json::to_value(r.kind)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| "other".to_owned()),
        };
        UiError {
            kind,
            message: e.to_string(),
        }
    }
}

type Res<T> = Result<T, UiError>;
type D<'a> = State<'a, Arc<Daemon>>;

async fn client(d: &Daemon) -> Res<Arc<Client>> {
    d.client()
        .await
        .ok_or_else(|| UiError::new("not_running", "Wasl's background service isn't running"))
}

// ---------------------------------------------------------------- the connection and the app

#[derive(Serialize)]
pub struct AppInfo {
    pub version: String,
    /// The `wasl` CLI ships next to the app (needed for "Start at login").
    pub cli: bool,
}

#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        cli: crate::daemon::sibling("wasl").is_some(),
    }
}

#[tauri::command]
pub fn daemon_state(d: D<'_>) -> Conn {
    d.conn()
}

#[tauri::command]
pub fn start_daemon(d: D<'_>) {
    d.request_start();
}

#[tauri::command]
pub fn set_tray_text(app: AppHandle, mut text: crate::tray::TrayText) {
    crate::notify::set_text(&app, text.notify.take());
    crate::tray::set_text(&app, text);
}

/// "Start at login": `wasl service install|uninstall` through the CLI next to the app.
#[tauri::command]
pub async fn set_start_at_login(enabled: bool) -> Res<()> {
    let cli = crate::daemon::sibling("wasl")
        .ok_or_else(|| UiError::new("cli_missing", "The wasl command isn't installed next to the app"))?;
    let action = if enabled { "install" } else { "uninstall" };
    let out = tauri::async_runtime::spawn_blocking(move || {
        let mut cmd = std::process::Command::new(cli);
        cmd.args(["service", action]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        cmd.output()
    })
    .await
    .map_err(|e| UiError::new("cli_failed", e.to_string()))?
    .map_err(|e| UiError::new("cli_failed", e.to_string()))?;
    if out.status.success() {
        Ok(())
    } else {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        Err(UiError::new("cli_failed", msg))
    }
}

// ---------------------------------------------------------------- status and pairs

#[tauri::command]
pub async fn status(d: D<'_>) -> Res<DaemonStatus> {
    Ok(client(&d).await?.status().await?)
}

#[tauri::command]
pub async fn pairs(d: D<'_>) -> Res<Vec<Pair>> {
    Ok(client(&d).await?.pairs().await?)
}

/// What the Add pair screen collects.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewPair {
    pub name: String,
    pub local_root: String,
    pub account_id: String,
    pub drive_folder_id: String,
    pub remote_path: String,
    pub drive_id: Option<String>,
    pub method: SyncMode,
    /// The vault the pair syncs through (its Drive folder is the vault's), when it has one.
    #[serde(default)]
    pub vault_id: Option<String>,
    /// Leave out what the folder's .gitignore files leave out of git.
    #[serde(default)]
    pub gitignore: bool,
    /// Preset ids (node, python, …) whose build and dependency folders are left out.
    #[serde(default)]
    pub presets: Vec<String>,
    /// Sync hidden files and folders too (names starting with a dot).
    #[serde(default)]
    pub hidden: bool,
}

/// Creates a pair with the engine's defaults; it waits for "Start sync" on its preview.
#[tauri::command]
pub async fn create_pair(d: D<'_>, pair: NewPair) -> Res<String> {
    if pair.name.trim().is_empty() || pair.local_root.trim().is_empty() {
        return Err(UiError::new("invalid", "A pair needs a name and a folder"));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let mut p = Pair::new(&id, pair.name.trim(), &pair.local_root);
    p.account_id = Some(pair.account_id);
    p.drive_folder_id = Some(pair.drive_folder_id);
    p.remote_path = pair.remote_path;
    p.drive_id = pair.drive_id;
    p.method = pair.method;
    p.vault_id = pair.vault_id;
    p.honor_gitignore = pair.gitignore;
    p.presets = pair.presets;
    p.sync_hidden = pair.hidden;
    p.status.hold_for_preview = true;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |t| t.as_millis() as i64);
    p.created_at = now;
    p.updated_at = now;
    client(&d).await?.create_pair(p).await?;
    Ok(id)
}

/// What Edit pair changes (#65, #103): the name and the method, and when given, the local folder or the Drive side.
/// The daemon resets a pair whose folders change and holds it for a new preview (#94).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairEdit {
    pub name: String,
    pub method: SyncMode,
    /// Another folder on this computer.
    #[serde(default)]
    pub local_root: Option<String>,
    /// Another Google account, Drive folder or vault.
    #[serde(default)]
    pub drive: Option<PairDrive>,
}

/// A pair's Drive side, as Edit pair sets it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairDrive {
    pub account_id: String,
    pub drive_folder_id: String,
    pub remote_path: String,
    pub drive_id: Option<String>,
    pub vault_id: Option<String>,
}

/// Renames a pair, changes its method or moves its folders. The daemon checks the pair rules again (UpdatePair).
///
/// wasld's UpdatePair replaces every option of the pair, `paused` included, so the edit is read and written as close
/// together as the desktop can: everything is prepared first, the pair is read fresh right before the write, and only
/// the edited fields are copied onto that read. A race remains: a pause, resume or other option change that lands
/// between this read and the write is undone by the write. Closing it needs a request that changes only the given
/// fields (engine issue #94).
#[tauri::command]
pub async fn update_pair(d: D<'_>, id: String, edit: PairEdit) -> Res<()> {
    let name = edit.name.trim().to_owned();
    if name.is_empty() {
        return Err(UiError::new("invalid", "A pair needs a name"));
    }
    let c = client(&d).await?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |t| t.as_millis() as i64);
    // The Drive side comes from the window, so it is checked against what the daemon knows: the account must be
    // signed in, and a vault must be the pair's own or one made in the chosen Drive folder.
    let vaults = match &edit.drive {
        Some(drive) => {
            if !c.accounts().await?.iter().any(|a| a.id == drive.account_id) {
                return Err(UiError::new("invalid", "That Google account isn't signed in to Wasl"));
            }
            if drive.vault_id.is_some() {
                c.vaults().await?
            } else {
                Vec::new()
            }
        }
        None => Vec::new(),
    };
    let mut p = c
        .pair(&id)
        .await?
        .ok_or_else(|| UiError::new("not_found", "That pair is gone"))?;
    let vault_choice = edit
        .drive
        .as_ref()
        .and_then(|d| {
            d.vault_id
                .as_deref()
                .map(|v| (v, d.drive_folder_id.as_str(), d.account_id.as_str()))
        });
    if let Some((vault_id, folder_id, account_id)) = vault_choice {
        // The pair's own vault only while it stays in the same account and Drive folder: its key
        // must never be used on another folder.
        let own = p.vault_id.as_deref() == Some(vault_id)
            && p.drive_folder_id.as_deref() == Some(folder_id)
            && p.account_id.as_deref() == Some(account_id);
        let in_folder = vaults.iter().any(|v| {
            v.id == vault_id
                && v.drive_folder_id.as_deref() == Some(folder_id)
                && v.account_id.as_deref() == Some(account_id)
        });
        if !own && !in_folder {
            return Err(UiError::new("invalid", "That vault isn't in the chosen Drive folder"));
        }
    }
    p.name = name;
    p.method = edit.method;
    if let Some(root) = edit.local_root {
        p.local_root = root;
    }
    if let Some(drive) = edit.drive {
        p.account_id = Some(drive.account_id);
        p.drive_folder_id = Some(drive.drive_folder_id);
        p.remote_path = drive.remote_path;
        p.drive_id = drive.drive_id;
        p.vault_id = drive.vault_id;
    }
    p.updated_at = now;
    Ok(c.update_pair(p).await?)
}

/// Removes the pair from Wasl. Files on both sides are not touched.
#[tauri::command]
pub async fn delete_pair(d: D<'_>, id: String) -> Res<()> {
    Ok(client(&d).await?.delete_pair(&id).await?)
}

/// Shows a pair's folder in Finder or File Explorer (#63). The window names the pair, not a path, so this only
/// opens folders Wasl syncs. macOS selects the folder in its parent (`open -R`), Windows too
/// (`explorer /select,`); Linux opens the folder. A root that isn't absolute, or isn't there (an unplugged
/// drive), is the same not-found error as an unknown pair: nothing is started for it, and an absolute path can't
/// be read as an option by `open` or `xdg-open`.
#[tauri::command]
pub async fn show_pair_folder(d: D<'_>, id: String) -> Res<()> {
    let not_found = || UiError::new("not_found", "That folder pair isn't on this computer any more");
    let pairs = client(&d).await?.pairs().await?;
    let root = pairs
        .into_iter()
        .find(|p| p.id == id)
        .map(|p| p.local_root)
        .ok_or_else(not_found)?;
    tauri::async_runtime::spawn_blocking(move || {
        let path = std::path::Path::new(&root);
        // Checked here, off the async runtime: a stat on a gone network drive can block.
        if !path.is_absolute() || !path.exists() {
            return Err(not_found());
        }
        #[cfg(target_os = "macos")]
        let shown = std::process::Command::new("open").arg("-R").arg(path).status().map(|s| s.success());
        #[cfg(windows)]
        let shown = {
            // explorer takes `/select,` and the path as one argument; `raw_arg` keeps the quotes it needs. Its
            // exit code is 1 even when it worked, so only failing to start it is an error.
            use std::os::windows::process::CommandExt;
            std::process::Command::new("explorer")
                .raw_arg(format!("/select,\"{root}\""))
                .status()
                .map(|_| true)
        };
        #[cfg(not(any(target_os = "macos", windows)))]
        let shown = std::process::Command::new("xdg-open").arg(path).status().map(|s| s.success());
        // Waited for, so no zombie is left. `open` and `xdg-open` exit non-zero when they couldn't show it.
        match shown {
            Ok(true) => Ok(()),
            Ok(false) => Err(UiError::new("io", "The file manager couldn't show that folder")),
            Err(e) => Err(UiError::new("io", e.to_string())),
        }
    })
    .await
    .map_err(|e| UiError::new("io", e.to_string()))?
}

/// The Wasl site's pages the apps link to (SRS FR-R4, #146): the privacy policy and the terms, in the window's
/// language. Only these pages are built here, so the window can't ask for any other address to be opened.
pub fn site_page_url(page: &str, lang: &str) -> Option<String> {
    if !matches!(page, "privacy" | "terms") {
        return None;
    }
    let prefix = if lang == "ar" { "/ar" } else { "" };
    Some(format!("https://wasl.kahflabs.com{prefix}/{page}"))
}

/// Opens a site page (`site_page_url`) in the default browser.
#[tauri::command]
pub async fn open_site_page(page: String, lang: String) -> Res<()> {
    let url = site_page_url(&page, &lang).ok_or_else(|| UiError::new("invalid", "Wasl has no such page"))?;
    open_in_browser(url).await
}

/// Opens a page on the Wasl site in the default browser, with `std::process::Command`. The address must pass
/// `update::site_url` (https, the site's host, a plain path), and the re-built form is what is opened.
pub async fn open_in_browser(url: String) -> Res<()> {
    let url = crate::update::site_url(&url).ok_or_else(|| UiError::new("invalid", "Wasl has no such page"))?;
    tauri::async_runtime::spawn_blocking(move || {
        #[cfg(target_os = "macos")]
        let opened = std::process::Command::new("open").arg(&url).status().map(|s| s.success());
        // explorer hands a URL to the default browser; its exit code is 1 even when it worked.
        #[cfg(windows)]
        let opened = std::process::Command::new("explorer").arg(&url).status().map(|_| true);
        #[cfg(not(any(target_os = "macos", windows)))]
        let opened = std::process::Command::new("xdg-open").arg(&url).status().map(|s| s.success());
        match opened {
            Ok(true) => Ok(()),
            Ok(false) => Err(UiError::new("io", "The browser couldn't open that page")),
            Err(e) => Err(UiError::new("io", e.to_string())),
        }
    })
    .await
    .map_err(|e| UiError::new("io", e.to_string()))?
}

#[tauri::command]
pub async fn set_paused(d: D<'_>, id: String, paused: bool) -> Res<()> {
    Ok(client(&d).await?.set_paused(&id, paused, None).await?)
}

#[tauri::command]
pub async fn preview(d: D<'_>, id: String) -> Res<Preview> {
    Ok(client(&d).await?.preview(&id).await?)
}

#[tauri::command]
pub async fn sync(d: D<'_>, id: String, trigger: Trigger) -> Res<RunOutcome> {
    Ok(client(&d).await?.sync(&id, trigger).await?)
}

#[tauri::command]
pub async fn sync_all(d: D<'_>) -> Res<Vec<RunOutcome>> {
    Ok(client(&d).await?.sync_all(Trigger::Manual).await?)
}

#[tauri::command]
pub async fn cancel(d: D<'_>, id: String) -> Res<()> {
    Ok(client(&d).await?.cancel(&id).await?)
}

#[tauri::command]
pub async fn approve_once(d: D<'_>, id: String) -> Res<()> {
    Ok(client(&d).await?.approve_once(&id).await?)
}

/// The plan a guard stopped, as the daemon describes it.
#[tauri::command]
pub async fn blocked_plan(d: D<'_>, id: String) -> Res<serde_json::Value> {
    Ok(client(&d).await?.call(Request::BlockedPlan { id }).await?)
}

/// Everything a stopped plan would delete, file by file ("Review deletions", #66).
#[tauri::command]
pub async fn deletion_review(d: D<'_>, pair_id: String) -> Res<DeletionReview> {
    Ok(client(&d).await?.deletion_review(&pair_id).await?)
}

/// Puts back every file a stopped plan would delete, on the side that lost it, and clears the stop.
#[tauri::command]
pub async fn restore_deletions(d: D<'_>, pair_id: String) -> Res<u32> {
    Ok(client(&d).await?.restore_deletions(&pair_id).await?)
}

// ---------------------------------------------------------------- history and problems

#[tauri::command]
pub async fn runs(d: D<'_>, pair_id: Option<String>, limit: u32) -> Res<Vec<RunRecord>> {
    Ok(client(&d).await?.runs(pair_id.as_deref(), limit).await?)
}

#[tauri::command]
pub async fn run_events(d: D<'_>, run_id: String) -> Res<Vec<RunEvent>> {
    Ok(client(&d).await?.run_events(&run_id).await?)
}

#[tauri::command]
pub async fn open_conflicts(d: D<'_>) -> Res<Vec<ConflictRecord>> {
    Ok(client(&d).await?.open_conflicts(None).await?)
}

#[tauri::command]
pub async fn conflict_details(d: D<'_>, id: String) -> Res<Option<ConflictDetails>> {
    Ok(client(&d).await?.conflict_details(&id).await?)
}

#[tauri::command]
pub async fn resolve_conflict(d: D<'_>, id: String, choice: ConflictChoice) -> Res<ConflictUndo> {
    Ok(client(&d).await?.resolve_conflict(&id, choice).await?)
}

#[tauri::command]
pub async fn undo_conflict(d: D<'_>, undo: ConflictUndo) -> Res<()> {
    Ok(client(&d).await?.undo_conflict(undo).await?)
}

// ---------------------------------------------------------------- agent changes

#[tauri::command]
pub async fn agent_changesets(d: D<'_>, all: bool) -> Res<Vec<Changeset>> {
    Ok(client(&d).await?.agent_changesets(None, all).await?)
}

#[tauri::command]
pub async fn agent_changeset(d: D<'_>, id: String) -> Res<ChangesetDetails> {
    Ok(client(&d).await?.agent_changeset(&id).await?)
}

#[tauri::command]
pub async fn agent_diff(d: D<'_>, id: String, rel: String) -> Res<String> {
    Ok(client(&d).await?.agent_diff(&id, &rel).await?)
}

#[tauri::command]
pub async fn agent_accept(d: D<'_>, id: String) -> Res<AcceptConfirmation> {
    // Not an accept: wasld opens its own confirmation dialog and accepts only when the person
    // confirms there (this window is a client like any other).
    Ok(client(&d).await?.agent_accept_as(&id, Some("desktop"), None).await?)
}

#[tauri::command]
pub async fn agent_end(d: D<'_>, id: String) -> Res<Option<Changeset>> {
    Ok(client(&d).await?.agent_end(Some(&id), None, None).await?)
}

/// How to connect agents on this computer: the `wasl` CLI installed next to the app (or on PATH)
/// and Claude Code's hooks block with its full path, ready to paste into settings.json.
#[derive(Serialize)]
pub struct AgentSetup {
    pub cli: String,
    pub claude_code_hooks: String,
}

#[tauri::command]
pub fn agent_setup() -> AgentSetup {
    let cli = crate::daemon::sibling("wasl")
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| "wasl".into());
    let command = |event: &str| format!("\"{cli}\" agent hook claude-code {event}");
    let one = |event: &str| serde_json::json!([{ "hooks": [{ "type": "command", "command": command(event) }] }]);
    let hooks = serde_json::json!({
        "hooks": {
            "UserPromptSubmit": one("prompt"),
            "PreToolUse": [{
                "matcher": "Edit|Write|MultiEdit|NotebookEdit",
                "hooks": [{ "type": "command", "command": command("pre-tool") }]
            }],
            "Stop": one("stop"),
            "SessionEnd": one("stop"),
        }
    });
    AgentSetup {
        claude_code_hooks: serde_json::to_string_pretty(&hooks).unwrap_or_default(),
        cli,
    }
}

#[tauri::command]
pub async fn agent_revert(d: D<'_>, id: String) -> Res<RevertReport> {
    Ok(client(&d).await?.agent_revert(&id).await?)
}

// ---------------------------------------------------------------- recycle bin

#[tauri::command]
pub async fn bin_entries(d: D<'_>) -> Res<Vec<BinEntry>> {
    Ok(client(&d).await?.bin_entries(None).await?)
}

/// What the last restore from the bin brought back, kept here for its Undo: the window sends back ids, never
/// entries (keys, Drive file ids), and only entries this app restored go back to wasld.
#[derive(Default)]
pub struct BinUndo(Mutex<Vec<BinEntry>>);

#[tauri::command]
pub async fn restore_bin(d: D<'_>, undo: State<'_, BinUndo>, ids: Vec<String>) -> Res<RestoreResult> {
    let r = client(&d).await?.restore_bin(ids).await?;
    *undo.0.lock().unwrap_or_else(PoisonError::into_inner) = r.restored.clone();
    Ok(r)
}

/// Undo for the last restore from the bin: those of its files named by `ids` go back into the bin (#67).
#[tauri::command]
pub async fn undo_bin_restore(d: D<'_>, undo: State<'_, BinUndo>, ids: Vec<String>) -> Res<()> {
    let restored: Vec<BinEntry> = undo
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|b| ids.contains(&b.entry.id))
        .cloned()
        .collect();
    if restored.is_empty() {
        return Err(UiError::new("not_found", "Nothing to undo: that restore isn't the last one"));
    }
    let c = client(&d).await?;
    c.call::<()>(Request::UndoBinRestore { restored }).await?;
    undo.0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|b| !ids.contains(&b.entry.id));
    Ok(())
}

#[tauri::command]
pub async fn delete_bin_forever(d: D<'_>, ids: Vec<String>) -> Res<u32> {
    Ok(client(&d).await?.delete_bin_forever(ids).await?)
}

#[tauri::command]
pub async fn empty_bin(d: D<'_>) -> Res<u32> {
    Ok(client(&d).await?.empty_bin().await?)
}

// ---------------------------------------------------------------- accounts and Drive

#[tauri::command]
pub async fn accounts(d: D<'_>) -> Res<Vec<Account>> {
    Ok(client(&d).await?.accounts().await?)
}

/// Starts Google sign-in in the browser; the result arrives as an `account_added` or
/// `account_failed` event. Returns the sign-in URL.
#[tauri::command]
pub async fn add_account(d: D<'_>) -> Res<String> {
    Ok(client(&d).await?.add_account().await?)
}

#[tauri::command]
pub async fn remove_account(d: D<'_>, id: String) -> Res<()> {
    Ok(client(&d).await?.remove_account(&id).await?)
}

#[tauri::command]
pub async fn drive_folders(d: D<'_>, account: String, parent_id: String) -> Res<Vec<DriveFolder>> {
    Ok(client(&d).await?.drive_folders(&account, &parent_id).await?)
}

#[tauri::command]
pub async fn resolve_drive_path(
    d: D<'_>,
    account: String,
    path: String,
    create: bool,
) -> Res<DriveFolder> {
    Ok(client(&d)
        .await?
        .resolve_drive_path(&account, &path, create)
        .await?)
}

// ---------------------------------------------------------------- devices

/// This computer and the devices in its group.
#[tauri::command]
pub async fn devices(d: D<'_>) -> Res<DevicesInfo> {
    Ok(client(&d).await?.devices().await?)
}

/// Removes a device from the group: it is never trusted again.
#[tauri::command]
pub async fn remove_device(d: D<'_>, id: String) -> Res<()> {
    Ok(client(&d).await?.remove_device(&id).await?)
}

/// Connects to a device of the group at `host:port`. Returns the ID of the device reached.
#[tauri::command]
pub async fn connect_device(d: D<'_>, address: String) -> Res<String> {
    let address = address.trim();
    if address.is_empty() {
        return Err(UiError::new("invalid", "An address looks like host:port"));
    }
    Ok(client(&d).await?.connect_device(address).await?)
}

/// Shares a pair with the group's devices under its share code (a new one the first time), or
/// under `share`, a code another device made for the same folder. Returns the code.
#[tauri::command]
pub async fn share_pair(d: D<'_>, id: String, share: Option<String>) -> Res<String> {
    let share = share.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    Ok(client(&d).await?.share_pair(&id, share.as_deref()).await?)
}

/// The pair's share code, if it syncs with devices.
#[tauri::command]
pub async fn share_code(d: D<'_>, id: String) -> Res<Option<String>> {
    Ok(client(&d).await?.share_code(&id).await?)
}

#[tauri::command]
pub async fn unshare_pair(d: D<'_>, id: String) -> Res<()> {
    Ok(client(&d).await?.unshare_pair(&id).await?)
}

/// Who a pair is shared with: every device known here, or the devices listed (docs/INTRODUCTIONS.md §3.2).
#[tauri::command]
pub async fn sharing(d: D<'_>, id: String) -> Res<Option<PairSharing>> {
    Ok(client(&d).await?.sharing(&id).await?)
}

/// Shares a pair with every device known here (`all`) and/or the devices listed; each listed
/// device is offered it.
#[tauri::command]
pub async fn share_with(d: D<'_>, id: String, all: bool, devices: Vec<String>) -> Res<()> {
    Ok(client(&d).await?.share_with(&id, all, devices).await?)
}

/// Folders other devices offer this one.
#[tauri::command]
pub async fn offers(d: D<'_>) -> Res<Vec<OfferInfo>> {
    Ok(client(&d).await?.offers().await?)
}

#[tauri::command]
pub async fn decline_offer(d: D<'_>, share: String, from: String) -> Res<()> {
    Ok(client(&d).await?.decline_offer(&share, &from).await?)
}

/// Makes a pair from an offer in `local_root`; it waits for its first-sync preview like any new pair.
#[tauri::command]
pub async fn accept_offer(d: D<'_>, share: String, from: String, local_root: String, name: Option<String>) -> Res<Pair> {
    let name = name.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty());
    Ok(client(&d).await?.accept_offer(&share, &from, &local_root, name.as_deref()).await?)
}

/// Marks a device another one added as known here: it may then sync what is shared with all devices.
#[tauri::command]
pub async fn keep_device(d: D<'_>, id: String) -> Res<()> {
    Ok(client(&d).await?.keep_device(&id).await?)
}

/// Opens a pairing code for a new device; the stages that follow arrive as `changed` events.
#[tauri::command]
pub async fn invite_device(d: D<'_>) -> Res<PairingStatus> {
    Ok(client(&d).await?.invite_device().await?)
}

#[tauri::command]
pub async fn join_device(d: D<'_>, code: String) -> Res<()> {
    let code = code.trim();
    if code.is_empty() {
        return Err(UiError::new("invalid", "Paste the code the other device shows"));
    }
    Ok(client(&d).await?.join_device(code).await?)
}

#[tauri::command]
pub async fn pairing_status(d: D<'_>) -> Res<PairingStatus> {
    Ok(client(&d).await?.pairing_status().await?)
}

/// The person here says the four words match (`accept`), or not.
#[tauri::command]
pub async fn confirm_pairing(d: D<'_>, accept: bool) -> Res<PairingStatus> {
    Ok(client(&d).await?.confirm_pairing(accept).await?)
}

#[tauri::command]
pub async fn cancel_pairing(d: D<'_>) -> Res<()> {
    Ok(client(&d).await?.cancel_pairing().await?)
}

// ---------------------------------------------------------------- read-only views

/// Read-only views of Drive folders, with their state, and whether this computer can show them.
#[tauri::command]
pub async fn views(d: D<'_>) -> Res<ViewsInfo> {
    Ok(client(&d).await?.views().await?)
}

/// What the Add view screen collects.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewViewForm {
    pub name: String,
    pub local_path: String,
    pub account_id: String,
    pub drive_folder_id: String,
    pub remote_path: String,
    pub drive_id: Option<String>,
    pub cache_limit: Option<u64>,
}

/// Shows a Drive folder at a new or empty folder on this computer, read-only.
#[tauri::command]
pub async fn add_view(d: D<'_>, view: NewViewForm) -> Res<View> {
    if view.name.trim().is_empty() || view.local_path.trim().is_empty() {
        return Err(UiError::new("invalid", "A view needs a name and a folder"));
    }
    Ok(client(&d)
        .await?
        .add_view(NewView {
            name: view.name.trim().to_owned(),
            account_id: view.account_id,
            drive_folder_id: view.drive_folder_id,
            drive_id: view.drive_id,
            remote_path: view.remote_path,
            local_path: view.local_path,
            cache_limit: view.cache_limit,
        })
        .await?)
}

/// Removes a view: pinned files stay as ordinary files; with `keep_files` every file is kept.
#[tauri::command]
pub async fn remove_view(d: D<'_>, id: String, keep_files: bool) -> Res<()> {
    Ok(client(&d).await?.remove_view(&id, keep_files).await?)
}

#[tauri::command]
pub async fn set_view_enabled(d: D<'_>, id: String, enabled: bool) -> Res<()> {
    Ok(client(&d).await?.set_view_enabled(&id, enabled).await?)
}

#[tauri::command]
pub async fn set_view_cache_limit(d: D<'_>, id: String, limit: u64) -> Res<()> {
    Ok(client(&d).await?.set_view_cache_limit(&id, limit).await?)
}

#[tauri::command]
pub async fn refresh_view(d: D<'_>, id: String) -> Res<()> {
    Ok(client(&d).await?.refresh_view(&id).await?)
}

/// Keeps a file or folder of a view on this computer (`path`: a full path in a view's folder).
#[tauri::command]
pub async fn pin_path(d: D<'_>, path: String) -> Res<()> {
    Ok(client(&d).await?.pin_path(&path).await?)
}

#[tauri::command]
pub async fn unpin_path(d: D<'_>, path: String) -> Res<()> {
    Ok(client(&d).await?.unpin_path(&path).await?)
}

// ---------------------------------------------------------------- vaults

/// Encrypted vaults known on this computer, with whether each is unlocked here.
#[tauri::command]
pub async fn vaults(d: D<'_>) -> Res<Vec<VaultInfo>> {
    Ok(client(&d).await?.vaults().await?)
}

/// What "Encrypt this folder (vault)" collects. The passphrase goes straight to the daemon (as a
/// `Passphrase`, wiped when dropped) and is never kept or printed.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewVaultForm {
    pub account_id: String,
    pub folder_id: String,
    pub drive_id: Option<String>,
    pub remote_path: String,
    pub name: String,
    pub passphrase: String,
}

/// Makes a new vault (format v2) in a new or empty Drive folder and unlocks it on this computer.
#[tauri::command]
pub async fn create_vault(d: D<'_>, vault: NewVaultForm) -> Res<VaultInfo> {
    let NewVaultForm { account_id, folder_id, drive_id, remote_path, name, passphrase } = vault;
    let passphrase = Passphrase::new(passphrase);
    Ok(client(&d)
        .await?
        .create_vault(NewVault {
            account_id,
            drive_id,
            parent_id: String::new(),
            folder_id: Some(folder_id),
            remote_path,
            name,
            passphrase,
        })
        .await?)
}

/// Unlocks a vault on this computer. A wrong passphrase is an `invalid` error.
#[tauri::command]
pub async fn unlock_vault(d: D<'_>, id: String, passphrase: String) -> Res<VaultInfo> {
    let passphrase = Passphrase::new(passphrase);
    Ok(client(&d).await?.unlock_vault(&id, passphrase).await?)
}

/// Forgets a vault's keys on this computer.
#[tauri::command]
pub async fn lock_vault(d: D<'_>, id: String) -> Res<()> {
    Ok(client(&d).await?.lock_vault(&id).await?)
}

// ---------------------------------------------------------------- settings

#[tauri::command]
pub async fn setting(d: D<'_>, key: String) -> Res<Option<String>> {
    Ok(client(&d).await?.setting(&key).await?)
}

#[tauri::command]
pub async fn set_setting(d: D<'_>, key: String, value: String) -> Res<()> {
    Ok(client(&d).await?.set_setting(&key, &value).await?)
}

// ---------------------------------------------------------------- the tray's popover and flyout (#69)

/// Esc in the popover: hide it and give focus back.
#[tauri::command]
pub fn tray_hide(app: AppHandle) {
    crate::popover::hide(&app, true);
}

/// The popover page's height in logical pixels, whenever it changes.
#[tauri::command]
pub fn tray_fit(app: AppHandle, height: f64) {
    crate::popover::fit(&app, height);
}

/// Open the main window, at `route` when given (`problems`, `agents`, `settings`, `pair:<id>`).
#[tauri::command]
pub fn tray_open(app: AppHandle, route: Option<String>) {
    crate::popover::open_in_main(&app, route);
}

/// The popover's More button: the tray's own menu under the button (`x`, `y`: logical pixels in the window).
#[tauri::command]
pub fn tray_menu(window: tauri::WebviewWindow, x: f64, y: f64) {
    crate::tray::popup_menu(&window, x, y);
}

/// General: "Show Wasl in the menu bar / notification area".
#[tauri::command]
pub fn set_tray_visible(app: AppHandle, visible: bool) {
    crate::tray::set_visible(&app, visible);
}

// ---------------------------------------------------------------- folder drop (#71)

/// Whether a path dropped on the window is a folder: only folders make pairs. Only an absolute
/// local path is looked at: a network path (`\\host\share`) would make Windows connect to that
/// host and offer the person's credentials, and a relative one says nothing about a drop
/// (security review L5).
#[tauri::command]
pub fn is_folder(path: String) -> bool {
    local_absolute(std::path::Path::new(&path)) && std::fs::metadata(path).is_ok_and(|m| m.is_dir())
}

/// An absolute path on a local disk: on Windows a drive letter path (`C:\...`), never a UNC,
/// `\\?\` or device path.
fn local_absolute(path: &std::path::Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        matches!(
            path.components().next(),
            Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_))
        )
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// A folder dropped on the Dock icon (macOS, main.rs), kept until the window takes it.
#[derive(Default)]
pub struct OpenedFolder(pub Mutex<Option<String>>);

/// The folder dropped on the Dock icon, once: the window asks when it starts and on `app://opened-folder`.
#[tauri::command]
pub fn take_opened_folder(opened: State<'_, OpenedFolder>) -> Option<String> {
    opened.0.lock().unwrap_or_else(PoisonError::into_inner).take()
}

// ---------------------------------------------------------------- window chrome and the menu bar (#61)

/// The menu bar's words in the app's language and the actions the window handles. macOS only; elsewhere
/// there is no menu bar and nothing happens.
#[tauri::command]
pub fn set_menu_text(app: AppHandle, text: crate::menu::MenuText) {
    crate::menu::set_text(&app, text);
}

/// Where the traffic lights are and whether the sidebar must be opaque (macOS). Not async, so it runs on the
/// main thread, where AppKit answers.
#[cfg(target_os = "macos")]
#[tauri::command]
pub fn window_chrome(window: tauri::WebviewWindow) -> crate::chrome_macos::Chrome {
    crate::chrome_macos::chrome(&window)
}

/// Windows and Linux draw no traffic lights: nothing to keep clear.
#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub fn window_chrome() -> Option<()> {
    None
}

/// A double-click on the toolbar strip, following the system setting (macOS).
#[tauri::command]
pub fn title_bar_double_click(window: tauri::WebviewWindow) {
    #[cfg(target_os = "macos")]
    crate::chrome_macos::title_bar_double_click(&window);
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

#[cfg(test)]
mod tests {
    use super::local_absolute;
    use super::site_page_url;
    use std::path::Path;

    #[test]
    fn only_the_privacy_and_terms_pages_open_in_the_window_language() {
        assert_eq!(site_page_url("privacy", "en").as_deref(), Some("https://wasl.kahflabs.com/privacy"));
        assert_eq!(site_page_url("terms", "ar").as_deref(), Some("https://wasl.kahflabs.com/ar/terms"));
        assert_eq!(site_page_url("https://example.com", "en"), None);
        assert_eq!(site_page_url("../privacy", "en"), None);
    }

    /// Security review L5 (#136): `is_folder` never looks at a network, device or relative path.
    #[test]
    fn only_absolute_local_paths_are_looked_at() {
        assert!(!local_absolute(Path::new("Projects")));
        assert!(!local_absolute(Path::new(r"\\host\share\Projects")));
        assert!(!local_absolute(Path::new(r"\\?\UNC\host\share")));
        assert!(!local_absolute(Path::new(r"\\.\pipe\x")));
        #[cfg(windows)]
        {
            assert!(local_absolute(Path::new(r"C:\Users\a\Projects")));
            assert!(!local_absolute(Path::new(r"\\?\C:\Users\a")));
        }
        #[cfg(not(windows))]
        assert!(local_absolute(Path::new("/Users/a/Projects")));
    }
}
