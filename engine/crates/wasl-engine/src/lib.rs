//! The Wasl sync engine: runs pairs end to end on top of the planner, file access, the store and
//! the remote (SRS FR-E4). Platform apps call [`Engine`]; they never decide what to sync (FR-E3).

#[cfg(feature = "ffi")]
uniffi::setup_scaffolding!();

mod actions;
mod agent;
mod devices;
mod drive_remote;
mod engine;
mod error;
mod executor;
mod journal;
mod memory_remote;
mod on_demand;
mod oplog;
pub mod plain_guard;
mod remote;
mod support;
mod undo;
mod vault_remote;
mod vault_v2_remote;

pub use actions::{
    ConflictChoice, ConflictDetails, ConflictSide, ConflictUndo, DeletionReview, FileVersions,
    PendingDeletion, RestoreFailure, RestoreResult, VersionEntry, VersionRef, VersionSource,
};
pub use agent::{
    AGENT_HISTORY_MS, AGENT_REVERT, ChangesetDetails, MAX_BEGIN_COPY, MAX_SAVED_FILE, RevertReport, actor_id,
};
pub use devices::{
    DeviceService, DeviceSync, MAX_PULL_BYTES, MAX_PULL_ENTRIES, PullReport, ScanReport,
    TOMBSTONE_RETENTION_MS, shares_by_id,
};
pub use drive_remote::DriveRemote;
pub use engine::{
    ALREADY_RUNNING, Engine, EngineConfig, Environment, GOOGLE_ANY_SIZE, HOLD_MESSAGE, INTERNAL_SOURCE,
    IgnoreVerdict, Preview, RunOutcome, SIZE_LIMIT_LOCAL_ONLY, STALLED, STOPPED, Trigger,
    excluded_by_pair_settings, export_beside_note, export_here_note, google_skipped_note, local_only_note,
};
pub use error::{EngineError, Result};
pub use executor::{BIN_RETENTION_MS, MAX_LOCAL_VERSIONS};
pub use journal::{JournalReport, MAX_CAPTURE, Version};
pub use memory_remote::{MemoryRemote, Op, ROOT_ID};
pub use on_demand::{DriveSource, RemoteSource, VaultV1Source, view_tree};
pub use oplog::{OPLOG_FORMAT_VERSION, OplogEntry, OplogOpEntry, OplogPage, rfc3339};
pub use plain_guard::PlainGuard;
pub use remote::{ItemUpdate, Listing, ProgressFn, Remote, RemoteInfo, Revision, UploadSpec};
pub use support::{CancelToken, Clock, Phase, Progress, ProgressListener, SystemClock, stamp_utc};
pub use undo::{
    KeepBy, SnapshotStats, UndoStore, WatchSnapshots, check_watch_root, hash_clones, snapshot_folder,
};
pub use vault_remote::{
    DIFFERENT_VAULT, DOCS_UNSUPPORTED, ManifestStatus, NEEDS_EMPTY_FOLDER, V1_NO_NEW_FOLDERS, VaultRemote,
    VaultSkips, ensure_manifest,
};
pub use vault_v2_remote::{
    CopyReport, SWEEP_GRACE, StateCheck, SweepReport, V2Skips, VAULT_IS_V1, VAULT_IS_V2, VAULT_ROLLED_BACK,
    VaultV2Remote, copy_vault, ensure_manifest_v2,
};
