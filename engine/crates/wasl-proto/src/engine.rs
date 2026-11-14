//! Engine-level outcome, progress, preview, conflict, version, and agent report DTOs.

use crate::conditions::Blocker;
use crate::model::{PlanSummary, SyncPlan};
use crate::run::{ErrorKind, RunStatus};
use crate::store::{AgentEntry, BinEntry, Changeset, ConflictRecord, JournalEntry, WatchGap};
use std::collections::HashMap;
use std::path::PathBuf;

/// What started a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum Trigger {
    /// "Sync now": only needs a connection (FR-P4).
    Manual,
    /// "Start sync" on the preview: also lifts the first-sync hold.
    Preview,
    Schedule,
    /// A watcher or remote change.
    Instant,
    /// Intents, CLI, shortcuts: conditions apply (FR-P4).
    Automation,
}

impl Trigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Trigger::Manual => "manual",
            Trigger::Preview => "preview",
            Trigger::Schedule => "schedule",
            Trigger::Instant => "instant",
            Trigger::Automation => "automation",
        }
    }
    pub fn manual(self) -> bool {
        matches!(self, Trigger::Manual | Trigger::Preview)
    }
}

/// How a sync request ended.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct RunOutcome {
    /// Empty when no run was recorded (skipped before starting).
    pub run_id: String,
    pub status: RunStatus,
    pub message: Option<String>,
    pub summary: Option<PlanSummary>,
    pub error_kind: Option<ErrorKind>,
    /// Why a skipped run did not start, when a condition stopped it.
    pub blocker: Option<Blocker>,
}

/// A dry run: the plan and notes the user sees before anything happens (FR-F1).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct Preview {
    pub plan: SyncPlan,
    /// What needs the user's attention (the plan's warnings among them).
    pub notes: Vec<String>,
    /// What the comparison did that is not a problem: where its time went, renames followed,
    /// shortcuts left out.
    #[serde(default)]
    pub infos: Vec<String>,
    /// Sizes of the files the plan deletes or skips (their actions carry none): the device's size
    /// for device-side items, Drive's for Drive-side ones.
    pub sizes: HashMap<String, u64>,
}

pub const HOLD_MESSAGE: &str = "Waiting for your preview";
/// A run asked for while the pair is already running is skipped with this message.
pub const ALREADY_RUNNING: &str = "Already running";
/// Why a run didn't start: its pair's options were being saved at that moment.
pub const SAVING_MESSAGE: &str = "Its options are being saved. Try again in a moment.";
pub const STOPPED: &str = "Stopped. Files synced so far stay synced.";
pub const STALLED: &str = "Stopped after 15 minutes without progress. Trying again shortly.";
/// A run the process or its caller stopped before it finished.
pub const INTERRUPTED: &str = "Stopped before it finished. The next sync carries on.";

/// Where a run is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum Phase {
    Starting,
    ListingRemote,
    ScanningLocal,
    Planning,
    Transferring,
    Deleting,
    Finishing,
}

/// A snapshot of a run's progress, for the status surfaces (FR-B5).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct Progress {
    pub pair_id: String,
    pub run_id: String,
    pub phase: Phase,
    pub total_actions: u64,
    pub done_actions: u64,
    pub bytes_total: u64,
    pub bytes_done: u64,
    /// Files hashed so far (a big first sync spends minutes here).
    pub checked: u64,
    pub errors: u64,
    pub current_file: Option<String>,
}

/// One file or folder a stopped plan would delete. Folders carry what is inside them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct PendingDeletion {
    pub key: String,
    pub name: String,
    /// Parent key, `""` at the root.
    pub folder: String,
    pub is_dir: bool,
    pub files: u32,
    pub bytes: u64,
    /// True when the deletion would happen on Drive (the file is gone from the device).
    pub on_drive: bool,
}

/// Everything a stopped plan would delete ("Review deletions").
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct DeletionReview {
    pub summary: PlanSummary,
    /// Folders first, then files, by path.
    pub items: Vec<PendingDeletion>,
    pub files_on_drive: u32,
    pub files_on_device: u32,
    pub bytes: u64,
}

/// One side of a conflict as the compare screen shows it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct ConflictSide {
    pub mtime_ms: i64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct ConflictDetails {
    pub conflict: ConflictRecord,
    pub name: String,
    pub folder: String,
    /// This device's edit, kept under the conflict name.
    pub device: Option<ConflictSide>,
    /// Drive's edit, now at the original name.
    pub drive: Option<ConflictSide>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum ConflictChoice {
    KeepBoth,
    KeepDevice,
    KeepDrive,
}

/// What a conflict choice changed, so Undo can put it back.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct ConflictUndo {
    pub conflict_id: String,
    pub choice: ConflictChoice,
    pub bin_entry_id: Option<String>,
    /// Local relative paths of a rename to reverse.
    pub renamed_from: Option<String>,
    pub renamed_to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum VersionRef {
    Drive { revision_id: String },
    Local { bin_entry_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum VersionSource {
    /// Uploaded from this device.
    Device,
    /// Made on Drive or another device.
    Drive,
    /// This device's copy that lost a conflict.
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct VersionEntry {
    pub version: VersionRef,
    pub at: i64,
    pub source: VersionSource,
    pub size: u64,
    pub note: Option<String>,
    pub current: bool,
    pub keep_forever: bool,
}

/// A file's timeline, newest first: Drive revisions merged with the versions this device replaced.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct FileVersions {
    pub name: String,
    pub path: String,
    pub pair_name: String,
    pub size: Option<u64>,
    pub entries: Vec<VersionEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct RestoreFailure {
    pub entry_id: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct RestoreResult {
    pub restored: Vec<BinEntry>,
    pub failed: Vec<RestoreFailure>,
}

/// Bin reason of an agent's version put aside by a revert.
pub const AGENT_REVERT: &str = "AGENT_REVERT";

/// A changeset with what it changed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangesetDetails {
    pub changeset: Changeset,
    pub pair_name: String,
    pub entries: Vec<AgentEntry>,
    /// Which client accepted it ("cli", "desktop", "unknown"), when it was recorded.
    #[serde(default)]
    pub decided_by: Option<String>,
}

/// How a revert went.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RevertReport {
    pub reverted: Vec<String>,
    /// Changed after the session ended: left as they are.
    pub skipped: Vec<String>,
    /// Path and why.
    pub failed: Vec<(String, String)>,
}

/// One version of a file in a session, oldest first: the snapshot's (`seq` 0), then each journal
/// entry's.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Version {
    /// 0 for the session-start snapshot.
    pub seq: u32,
    /// `None` once the file is deleted.
    pub blake3: Option<String>,
    /// Where its bytes are, when Wasl kept them here (a synced version is fetched from device
    /// sync instead).
    pub path: Option<PathBuf>,
    pub captured: bool,
    pub reason: Option<String>,
    pub at: i64,
}

/// What a session journaled and what it missed (UND-4), for the session report (UND-10).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JournalReport {
    pub entries: Vec<JournalEntry>,
    /// Entries for versions Wasl could not keep, or found only by a rescan (versions in between
    /// were not seen).
    pub not_captured: Vec<JournalEntry>,
    /// Watched folders the session couldn't snapshot at its start: (watch id, why).
    pub snapshot_failures: Vec<(String, String)>,
    /// What it missed while Wasl couldn't watch or rescan a watched folder.
    #[serde(default)]
    pub gaps: Vec<WatchGap>,
}
