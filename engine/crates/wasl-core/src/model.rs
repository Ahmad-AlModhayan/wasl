//! Pure data the planner reads and produces.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The seven sync methods (SRS FR-P2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum SyncMode {
    /// Changes and deletions on either side are mirrored to the other.
    TwoWay,
    /// New and changed device files go to Drive. Nothing is deleted on Drive.
    UploadOnly,
    /// Move files from the device to Drive.
    UploadThenDelete,
    /// The Drive folder becomes an exact copy of the device folder.
    UploadMirror,
    /// New and changed Drive files come to the device. Nothing is deleted locally.
    DownloadOnly,
    /// Move files from Drive to the device.
    DownloadThenDelete,
    /// The device folder becomes an exact copy of the Drive folder.
    DownloadMirror,
}

impl SyncMode {
    /// The mode's name as people see it, in English and Arabic: the CLI's labels and the engine's
    /// notes both read it, so the two never drift.
    pub const fn names(self) -> (&'static str, &'static str) {
        match self {
            SyncMode::TwoWay => ("Two-way", "مزامنة باتجاهين"),
            SyncMode::UploadOnly => ("Upload only", "رفع فقط"),
            SyncMode::UploadThenDelete => ("Upload, then delete here", "رفع ثم الحذف من هنا"),
            SyncMode::UploadMirror => ("Mirror to Drive", "نسخة مطابقة في Drive"),
            SyncMode::DownloadOnly => ("Download only", "تنزيل فقط"),
            SyncMode::DownloadThenDelete => ("Download, then delete on Drive", "تنزيل ثم الحذف من Drive"),
            SyncMode::DownloadMirror => ("Mirror from Drive", "نسخة مطابقة من Drive"),
        }
    }

    pub const ALL: [SyncMode; 7] = [
        SyncMode::TwoWay,
        SyncMode::UploadOnly,
        SyncMode::UploadThenDelete,
        SyncMode::UploadMirror,
        SyncMode::DownloadOnly,
        SyncMode::DownloadThenDelete,
        SyncMode::DownloadMirror,
    ];

    pub fn uploads(self) -> bool {
        matches!(
            self,
            SyncMode::TwoWay | SyncMode::UploadOnly | SyncMode::UploadThenDelete | SyncMode::UploadMirror
        )
    }

    pub fn downloads(self) -> bool {
        matches!(
            self,
            SyncMode::TwoWay
                | SyncMode::DownloadOnly
                | SyncMode::DownloadThenDelete
                | SyncMode::DownloadMirror
        )
    }
}

/// What to do when a file changed on both sides (SRS FR-P5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum ConflictPolicy {
    KeepBoth,
    NewerWins,
    LocalWins,
    RemoteWins,
    Skip,
}

impl ConflictPolicy {
    pub const ALL: [ConflictPolicy; 5] = [
        ConflictPolicy::KeepBoth,
        ConflictPolicy::NewerWins,
        ConflictPolicy::LocalWins,
        ConflictPolicy::RemoteWins,
        ConflictPolicy::Skip,
    ];
}

/// Export format for Google Docs, Sheets and Slides mirrors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum DocsFormat {
    Office,
    Pdf,
}

/// The last synced state of one key (1.1.4 `FileStateEntity` without the pair id and timestamp).
///
/// Times are milliseconds since the Unix epoch. `local_hash` and `remote_md5` are MD5 hex strings on
/// Drive pairs, so a local file and its Drive copy compare directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct BaselineEntry {
    /// Canonical relative path using Drive names.
    pub key: String,
    /// Local relative path; differs from `key` only when names were mapped.
    pub local_rel: String,
    pub is_dir: bool,
    pub local_size: u64,
    pub local_mtime: i64,
    pub local_hash: Option<String>,
    pub remote_id: Option<String>,
    pub remote_md5: Option<String>,
    pub remote_size: Option<u64>,
    pub remote_mtime: Option<i64>,
    pub remote_version: Option<i64>,
    /// Set for Google Docs export mirrors, which are never uploaded back.
    pub export_mime: Option<String>,
}

/// A local file or folder as the scanner saw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct LocalEntry {
    /// Relative path with local names.
    pub rel: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: i64,
}

/// A Drive file or folder inside the pair's folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct RemoteEntry {
    /// Relative path with Drive names, `/`-separated.
    pub key: String,
    pub id: String,
    pub name: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub md5: Option<String>,
    pub mtime: i64,
    pub version: Option<i64>,
    /// Set for Google-native documents, which are exported and never uploaded.
    pub google_mime: Option<String>,
    pub parent_id: Option<String>,
}

/// Everything the planner reads. Keys are canonical (Drive-name) relative paths.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlanInput {
    pub baseline: BTreeMap<String, BaselineEntry>,
    /// Local entries by canonical key (after un-mapping local names).
    pub local: BTreeMap<String, LocalEntry>,
    /// Canonical key → local relative path, for keys whose local name is known.
    pub local_rel_by_key: BTreeMap<String, String>,
    pub remote: BTreeMap<String, RemoteEntry>,
}

impl PlanInput {
    /// Drops every key `ignored(key, is_dir)` rejects from all three trees, so the planner never
    /// sees it: an ignored file is not uploaded, downloaded or deleted on any side, and adding a rule
    /// never deletes copies that already exist elsewhere (SRS FR-G4, FR-G5). Baseline rows are only
    /// hidden from this plan, not removed from the store.
    pub fn without_ignored(mut self, mut ignored: impl FnMut(&str, bool) -> bool) -> PlanInput {
        let mut is_dir: BTreeMap<String, bool> = BTreeMap::new();
        for (k, b) in &self.baseline {
            is_dir.insert(k.clone(), b.is_dir);
        }
        for (k, r) in &self.remote {
            is_dir.insert(k.clone(), r.is_dir);
        }
        for (k, l) in &self.local {
            is_dir.insert(k.clone(), l.is_dir);
        }
        let dropped: std::collections::BTreeSet<String> = is_dir
            .into_iter()
            .filter(|(k, d)| ignored(k, *d))
            .map(|(k, _)| k)
            .collect();
        if !dropped.is_empty() {
            self.baseline.retain(|k, _| !dropped.contains(k));
            self.local.retain(|k, _| !dropped.contains(k));
            self.remote.retain(|k, _| !dropped.contains(k));
            self.local_rel_by_key.retain(|k, _| !dropped.contains(k));
        }
        self
    }
}

/// One unit of work decided by the planner.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum Action {
    MkdirLocal {
        key: String,
        local_rel: String,
    },
    MkdirRemote {
        key: String,
        parent_key: String,
        name: String,
    },
    Upload {
        key: String,
        local: LocalEntry,
        existing_remote_id: Option<String>,
        parent_key: String,
        name: String,
        reason: String,
    },
    Download {
        key: String,
        remote: RemoteEntry,
        local_rel: String,
        overwrites: bool,
        reason: String,
    },
    Export {
        key: String,
        remote: RemoteEntry,
        local_rel: String,
        mime: String,
        reason: String,
    },
    DeleteLocal {
        key: String,
        local_rel: String,
        is_dir: bool,
        reason: String,
    },
    DeleteRemote {
        key: String,
        remote_id: String,
        is_dir: bool,
        reason: String,
    },
    Conflict {
        key: String,
        local: LocalEntry,
        remote: RemoteEntry,
        local_rel: String,
        parent_key: String,
    },
    /// Both sides are identical: just record the baseline.
    Adopt {
        key: String,
        local: LocalEntry,
        remote: RemoteEntry,
        local_rel: String,
    },
    /// Gone on both sides: drop the baseline row.
    Forget {
        key: String,
    },
    Skip {
        key: String,
        reason: String,
        warn: bool,
    },
    /// Metadata drift only: update the baseline row.
    RefreshState {
        key: String,
        local: Option<LocalEntry>,
        remote: Option<RemoteEntry>,
        local_rel: String,
        export_mime: Option<String>,
    },
}

impl Action {
    pub fn key(&self) -> &str {
        match self {
            Action::MkdirLocal { key, .. }
            | Action::MkdirRemote { key, .. }
            | Action::Upload { key, .. }
            | Action::Download { key, .. }
            | Action::Export { key, .. }
            | Action::DeleteLocal { key, .. }
            | Action::DeleteRemote { key, .. }
            | Action::Conflict { key, .. }
            | Action::Adopt { key, .. }
            | Action::Forget { key }
            | Action::Skip { key, .. }
            | Action::RefreshState { key, .. } => key,
        }
    }

    /// The variant name, e.g. `"Upload"`.
    pub fn kind(&self) -> &'static str {
        match self {
            Action::MkdirLocal { .. } => "MkdirLocal",
            Action::MkdirRemote { .. } => "MkdirRemote",
            Action::Upload { .. } => "Upload",
            Action::Download { .. } => "Download",
            Action::Export { .. } => "Export",
            Action::DeleteLocal { .. } => "DeleteLocal",
            Action::DeleteRemote { .. } => "DeleteRemote",
            Action::Conflict { .. } => "Conflict",
            Action::Adopt { .. } => "Adopt",
            Action::Forget { .. } => "Forget",
            Action::Skip { .. } => "Skip",
            Action::RefreshState { .. } => "RefreshState",
        }
    }

    pub fn is_mkdir(&self) -> bool {
        matches!(self, Action::MkdirLocal { .. } | Action::MkdirRemote { .. })
    }

    pub fn is_delete(&self) -> bool {
        matches!(self, Action::DeleteLocal { .. } | Action::DeleteRemote { .. })
    }
}

/// Why a plan stopped to ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum BlockKind {
    #[serde(rename = "deletions")]
    Deletions,
    #[serde(rename = "wipe")]
    Wipe,
    #[serde(rename = "first-merge")]
    FirstMerge,
}

/// Where a blocked plan would delete files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum BlockSide {
    #[serde(rename = "drive")]
    Drive,
    /// The local folder (1.1.4 stored it as "phone").
    #[serde(rename = "phone")]
    Device,
}

/// Counts and samples for the preview and the run history. Serialised like 1.1.4's `PlanSummary`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct PlanSummary {
    pub uploads: u32,
    pub upload_bytes: u64,
    pub downloads: u32,
    pub download_bytes: u64,
    pub exports: u32,
    pub delete_local: u32,
    pub delete_remote: u32,
    pub conflicts: u32,
    pub mkdirs: u32,
    pub skips: u32,
    pub local_files: u32,
    pub remote_files: u32,
    pub warnings: Vec<String>,
    pub blocked_reason: Option<String>,
    pub sample_deletes: Vec<String>,
    /// Files that already match and will not be touched.
    pub unchanged: u32,
    pub blocked_kind: Option<BlockKind>,
    pub blocked_side: Option<BlockSide>,
    pub blocked_percent: u32,
    pub blocked_count: u32,
    /// Files the method removes from the sending side once they arrived: every upload of "upload
    /// then delete" (to this device's bin) and every download of "download then delete" (to
    /// Drive's bin). Not counted in `delete_*`, so the mass-delete guard does not stop them.
    #[serde(default)]
    pub moved_after_transfer: u32,
}

impl PlanSummary {
    /// True when the plan moves, deletes or creates nothing (bookkeeping only).
    pub fn is_empty(&self) -> bool {
        self.uploads
            + self.downloads
            + self.exports
            + self.delete_local
            + self.delete_remote
            + self.conflicts
            + self.mkdirs
            == 0
    }
}

/// An ordered list of actions: folders first (shallow first), then transfers, then deletions
/// (deepest first), so parents are never removed before their children.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SyncPlan {
    pub actions: Vec<Action>,
    pub summary: PlanSummary,
    /// A safety guard stopped this plan; the actions are still listed for the preview.
    pub blocked: bool,
}

/// Where a pair syncs (SRS FR-P3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum Route {
    Drive,
    DriveAndDevices,
    Devices,
}

impl Route {
    pub fn as_str(self) -> &'static str {
        match self {
            Route::Drive => "drive",
            Route::DriveAndDevices => "drive_and_devices",
            Route::Devices => "devices",
        }
    }
}

/// Where device sync may connect (SRS FR-P3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum Reach {
    Lan,
    LanAndInternet,
}

impl Reach {
    pub fn as_str(self) -> &'static str {
        match self {
            Reach::Lan => "lan",
            Reach::LanAndInternet => "lan_and_internet",
        }
    }
}
