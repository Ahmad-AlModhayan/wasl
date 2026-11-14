//! Persistence DTOs shared across the store, daemon IPC, FFI, and CLI.

use crate::conditions::Conditions;
use crate::model::{ConflictPolicy, DocsFormat, Reach, Route, SyncMode};
use crate::oplog::rules::OplogStatus;
use crate::run::{ErrorKind, RunStatus};
use serde::{Deserialize, Serialize};

/// How the engine reaches the local folder on Android (SRS FR-R3); `Files` everywhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum StorageMode {
    /// A plain path (desktop, Android All files access).
    Files,
    /// An Android Storage Access Framework tree URI.
    Saf,
}

impl StorageMode {
    pub fn as_str(self) -> &'static str {
        match self {
            StorageMode::Files => "files",
            StorageMode::Saf => "saf",
        }
    }
}

/// What the engine last saw of a pair. Written by the engine only.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct PairStatus {
    pub last_run_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub last_status: Option<RunStatus>,
    pub last_error: Option<String>,
    pub last_error_kind: Option<ErrorKind>,
    pub consecutive_failures: u32,
    /// A guard stopped the last run; the summary is in `blocked_plans`.
    pub pending_approval: bool,
    /// The user approved the stopped plan; the next run goes ahead once.
    pub approve_once: bool,
    /// A new or re-pointed pair waits for "Start sync" on the preview.
    pub hold_for_preview: bool,
}

/// A folder pair. Fields mirror 1.1.4's `FolderPairEntity` plus the Wasl 2 route and reach.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct Pair {
    pub id: String,
    pub name: String,
    pub account_id: Option<String>,
    /// A path (`Files`) or a tree URI (`Saf`).
    pub local_root: String,
    pub storage_mode: StorageMode,
    pub drive_folder_id: Option<String>,
    /// Display path of the Drive folder, e.g. `My Drive/Photos`.
    pub remote_path: String,
    /// Shared drive id, when the folder is in one.
    pub drive_id: Option<String>,
    pub route: Route,
    pub reach: Reach,
    pub method: SyncMode,
    pub conflict_policy: ConflictPolicy,
    pub enabled: bool,
    pub paused: bool,
    pub pause_reason: Option<String>,
    pub conditions: Conditions,
    pub include_patterns: Vec<String>,
    pub exclude_patterns: Vec<String>,
    pub excluded_subfolders: Vec<String>,
    pub sync_hidden: bool,
    /// 0 means no limit.
    pub size_limit: u64,
    pub propagate_deletions: bool,
    pub mass_delete_guard: bool,
    pub docs_export: bool,
    pub docs_format: DocsFormat,
    pub preserve_mtime: bool,
    pub vault_id: Option<String>,
    pub honor_gitignore: bool,
    /// Preset ids (`node`, `python`…).
    pub presets: Vec<String>,
    /// 0 follows the global setting.
    pub interval_minutes: u32,
    pub instant_sync: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub status: PairStatus,
}

impl Pair {
    /// A Drive pair with 1.1.4's defaults.
    pub fn new(id: &str, name: &str, local_root: &str) -> Pair {
        Pair {
            id: id.to_owned(),
            name: name.to_owned(),
            account_id: None,
            local_root: local_root.to_owned(),
            storage_mode: StorageMode::Files,
            drive_folder_id: None,
            remote_path: String::new(),
            drive_id: None,
            route: Route::Drive,
            reach: Reach::Lan,
            method: SyncMode::TwoWay,
            conflict_policy: ConflictPolicy::KeepBoth,
            enabled: true,
            paused: false,
            pause_reason: None,
            conditions: Conditions::default(),
            include_patterns: Vec::new(),
            exclude_patterns: Vec::new(),
            excluded_subfolders: Vec::new(),
            sync_hidden: false,
            size_limit: 0,
            propagate_deletions: true,
            mass_delete_guard: true,
            docs_export: true,
            docs_format: DocsFormat::Office,
            preserve_mtime: true,
            vault_id: None,
            honor_gitignore: false,
            presets: Vec::new(),
            interval_minutes: 0,
            instant_sync: true,
            created_at: 0,
            updated_at: 0,
            status: PairStatus::default(),
        }
    }
}

/// One view.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct View {
    pub id: String,
    pub name: String,
    pub account_id: String,
    pub drive_folder_id: String,
    pub drive_id: Option<String>,
    /// Display path of the Drive folder, e.g. `My Drive/Photos`.
    pub remote_path: String,
    /// Where the view appears on this device.
    pub local_path: String,
    pub vault_id: Option<String>,
    /// Bytes of unpinned content the view may keep.
    pub cache_limit: u64,
    pub enabled: bool,
    pub created_at: i64,
    pub last_refresh_at: Option<i64>,
    pub last_error: Option<String>,
}

/// Where the removed file was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum BinSide {
    /// This device's folder; the file is in the local bin.
    Local,
    /// Drive; the file is in Drive's bin.
    Drive,
}

impl BinSide {
    pub fn as_str(self) -> &'static str {
        match self {
            BinSide::Local => "local",
            BinSide::Drive => "drive",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct NewBinEntry {
    pub id: String,
    pub pair_id: String,
    pub side: BinSide,
    pub key: String,
    /// `DELETED`, `OVERWRITTEN`, `CONFLICT` or `RESTORED_VERSION`, as in 1.1.4.
    pub reason: String,
    /// Local side: the location token from `LocalFs::move_to_bin`. Drive side: empty.
    pub location: String,
    pub deleted_at: i64,
    pub expires_at: i64,
    pub size: u64,
    pub account_id: Option<String>,
    pub remote_file_id: Option<String>,
    pub display_name: Option<String>,
    pub pair_name: Option<String>,
}

/// A bin entry with its version number among entries for the same key and side.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct BinEntry {
    pub entry: NewBinEntry,
    pub version_no: u32,
}

/// Counters of one run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct RunStats {
    pub uploaded: u32,
    pub downloaded: u32,
    pub exported: u32,
    pub deleted_local: u32,
    pub deleted_remote: u32,
    pub conflicts: u32,
    pub skipped: u32,
    pub errors: u32,
    pub bytes_up: u64,
    pub bytes_down: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct RunRecord {
    pub id: String,
    pub pair_id: String,
    pub trigger: String,
    pub status: RunStatus,
    pub stats: RunStats,
    pub message: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum EventLevel {
    Info,
    Warn,
    Error,
}

impl EventLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            EventLevel::Info => "INFO",
            EventLevel::Warn => "WARN",
            EventLevel::Error => "ERROR",
        }
    }
}

/// One line of a run's log.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct RunEvent {
    pub at: i64,
    pub level: EventLevel,
    /// `UPLOAD`, `DOWNLOAD`, `EXPORT`, `DELETE_LOCAL`, `DELETE_REMOTE`, `CONFLICT`, `MKDIR`, `SKIP`,
    /// `ERROR`, `BLOCKED`, `INFO`, as in 1.1.4.
    pub action: String,
    pub key: Option<String>,
    pub message: String,
    pub detail: Option<String>,
    pub bytes: u64,
}

/// A file changed on both sides (SRS FR-F4).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct ConflictRecord {
    pub id: String,
    pub pair_id: String,
    pub key: String,
    pub detected_at: i64,
    pub policy: ConflictPolicy,
    /// The renamed local copy, for keep-both.
    pub local_copy_rel: Option<String>,
    pub remote_file_id: Option<String>,
    pub message: String,
    pub resolved: bool,
    pub resolved_at: Option<i64>,
}

/// Where a changeset is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangesetStatus {
    /// The agent is still working.
    Open,
    /// Ended; its files wait for the user and are not synced.
    Held,
    Accepted,
    Reverted,
    /// Reverted except for files the user changed after the session (or that failed).
    PartlyReverted,
}

impl ChangesetStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ChangesetStatus::Open => "open",
            ChangesetStatus::Held => "held",
            ChangesetStatus::Accepted => "accepted",
            ChangesetStatus::Reverted => "reverted",
            ChangesetStatus::PartlyReverted => "partly_reverted",
        }
    }

    pub fn parse(s: &str) -> Option<ChangesetStatus> {
        Some(match s {
            "open" => ChangesetStatus::Open,
            "held" => ChangesetStatus::Held,
            "accepted" => ChangesetStatus::Accepted,
            "reverted" => ChangesetStatus::Reverted,
            "partly_reverted" => ChangesetStatus::PartlyReverted,
            _ => return None,
        })
    }

    /// Open and held changesets keep their files out of sync.
    pub fn holds(self) -> bool {
        matches!(self, ChangesetStatus::Open | ChangesetStatus::Held)
    }

    /// The op-log status this is recorded as (ADR-0002): `held` is `staged`, `accepted` is
    /// `committed`, the others keep their names.
    pub fn in_oplog(self) -> OplogStatus {
        match self {
            ChangesetStatus::Open => OplogStatus::Open,
            ChangesetStatus::Held => OplogStatus::Staged,
            ChangesetStatus::Accepted => OplogStatus::Committed,
            ChangesetStatus::Reverted => OplogStatus::Reverted,
            ChangesetStatus::PartlyReverted => OplogStatus::PartlyReverted,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Changeset {
    pub id: String,
    /// The pair the session works in; `None` for a session in a watched folder (#57).
    pub pair_id: Option<String>,
    /// `agent:<name>`.
    pub actor: String,
    pub label: Option<String>,
    /// Pair-relative folder the agent works in; empty for the whole pair.
    pub scope: String,
    /// The agent's own session id (Claude Code's `session_id`), when it has one.
    pub session: Option<String>,
    pub status: ChangesetStatus,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub decided_at: Option<i64>,
}

/// One item of the scope when the session began (local relative path).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ManifestRow {
    pub rel: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: i64,
    /// In this state on Drive or the other devices when the session began.
    pub synced: bool,
}

/// An earlier version copied to the agent folder.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SavedCopy {
    pub rel: String,
    /// File name under the changeset's folder.
    pub location: String,
    pub size: u64,
    pub mtime: i64,
    /// BLAKE3 of the copy (hex), taken while copying; `None` for copies saved before schema v9.
    #[serde(default)]
    pub blake3: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Added,
    Changed,
    Deleted,
}

impl EntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EntryKind::Added => "added",
            EntryKind::Changed => "changed",
            EntryKind::Deleted => "deleted",
        }
    }
}

/// Where an entry's earlier version is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EarlierVersion {
    /// Copied to the agent folder (`location`).
    Saved,
    /// The synced version, which Drive or the other devices still hold.
    Synced,
    /// None: the file is new.
    None,
}

impl EarlierVersion {
    pub fn as_str(self) -> &'static str {
        match self {
            EarlierVersion::Saved => "saved",
            EarlierVersion::Synced => "synced",
            EarlierVersion::None => "none",
        }
    }
}

/// One file or folder a session added, changed or deleted.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentEntry {
    /// Local relative path.
    pub rel: String,
    /// Canonical key (Drive names).
    pub key: String,
    pub kind: EntryKind,
    pub is_dir: bool,
    pub before_size: Option<u64>,
    pub before_mtime: Option<i64>,
    pub after_size: Option<u64>,
    pub after_mtime: Option<i64>,
    pub source: EarlierVersion,
    pub location: Option<String>,
    /// Set by a revert: `reverted`, `skipped_changed` or `failed: <why>`.
    pub outcome: Option<String>,
}

/// Ignored in every new watch until removed from its list (UND-1): folders of installed packages
/// and build output an agent rewrites wholesale.
pub const DEFAULT_WATCH_IGNORES: [&str; 4] = ["node_modules/", ".venv/", "target/", "build/"];

/// A folder on the watch list.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Watch {
    /// From the same id space as pair ids (ADR-0002).
    pub id: String,
    /// The folder's full path.
    pub root: String,
    /// The default ignores still applied (`.waslignore` files apply as well).
    pub ignores: Vec<String>,
    /// Unix ms.
    pub added_at: i64,
}

/// Where a snapshot's bytes are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotOrigin {
    /// A copy-on-write clone in the undo store, hashed later.
    Cloned,
    /// The undo store's object, named by its BLAKE3.
    Stored,
    /// The synced version of a pair file (its BLAKE3 from the device index); nothing kept here.
    Synced,
}

impl SnapshotOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            SnapshotOrigin::Cloned => "cloned",
            SnapshotOrigin::Stored => "stored",
            SnapshotOrigin::Synced => "synced",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "cloned" => SnapshotOrigin::Cloned,
            "stored" => SnapshotOrigin::Stored,
            "synced" => SnapshotOrigin::Synced,
            _ => return None,
        })
    }
}

/// One change a session made in a watched folder (UND-3), or a version Wasl found it had missed
/// (UND-4).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JournalEntry {
    /// 1, 2, 3 … per session; ignored when appending.
    pub seq: u32,
    /// The watch id `rel` is relative to.
    pub root: String,
    /// `create`, `update` or `delete`. A rename is a `delete` plus a `create` naming `old_rel`.
    pub op: String,
    pub rel: String,
    pub old_rel: Option<String>,
    /// The content before and after, BLAKE3 hex; `None` for a missing side (or unknown).
    pub prev_blake3: Option<String>,
    pub next_blake3: Option<String>,
    /// Where the new content's bytes are, when kept.
    pub origin: Option<SnapshotOrigin>,
    pub location: Option<String>,
    /// `false`: a version Wasl could not keep; `reason` says why.
    pub captured: bool,
    pub reason: Option<String>,
    /// Unix ms.
    pub at: i64,
}

/// The new content's size and mtime (Unix ms), for the op-log state; `None` for a delete.
pub type NextMeta = Option<(u64, i64)>;

/// A file's size, mtime (Unix ms), ctime and file id: what tells it unchanged (the snapshot's
/// reuse key).
pub type StatKey = (u64, i64, Option<i64>, Option<i64>);

/// Something a session missed while Wasl couldn't watch or rescan a watched folder (UND-4).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WatchGap {
    pub watch_id: String,
    /// A file Wasl last knew there; `None` for the whole folder.
    pub rel: Option<String>,
    pub reason: String,
}

/// One file of a watched folder as a session found it at its start.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SnapshotRow {
    /// Relative to the watched folder, `/`-separated.
    pub rel: String,
    pub size: u64,
    /// Unix ms.
    pub mtime: i64,
    /// Status change time in ns (creation time on Windows); with `file_id`, tells a replaced file
    /// from an unchanged one of the same size and mtime.
    pub ctime: Option<i64>,
    /// The inode (`None` on Windows for now).
    pub file_id: Option<i64>,
    /// BLAKE3 hex; `None` for a clone not hashed yet.
    pub blake3: Option<String>,
    pub origin: SnapshotOrigin,
    /// Relative to the undo store; `None` for [`SnapshotOrigin::Synced`].
    pub location: Option<String>,
}
