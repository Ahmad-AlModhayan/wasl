//! Public wire DTOs, protocol constants, and schema rules for Wasl.

#![forbid(unsafe_code)]

#[cfg(feature = "ffi")]
uniffi::setup_scaffolding!();

pub mod conditions;
pub mod devices;
pub mod engine;
pub mod model;
pub mod oplog;
pub mod presets;
pub mod run;
pub mod store;

pub use conditions::{Blocker, Conditions, DeviceState, blocker};
pub use devices::{
    DeviceInfo, DevicesInfo, LinkPath, OfferInfo, OfferStatus, PairSharing, PairingStage, PairingStatus,
};
pub use engine::{
    AGENT_REVERT, ALREADY_RUNNING, ChangesetDetails, ConflictChoice, ConflictDetails, ConflictSide,
    ConflictUndo, DeletionReview, FileVersions, HOLD_MESSAGE, INTERRUPTED, JournalReport, PendingDeletion,
    Phase, Preview, Progress, RestoreFailure, RestoreResult, RevertReport, RunOutcome, SAVING_MESSAGE,
    STALLED, STOPPED, Trigger, Version, VersionEntry, VersionRef, VersionSource,
};
pub use model::{
    Action, BaselineEntry, BlockKind, BlockSide, ConflictPolicy, DocsFormat, LocalEntry, PlanInput,
    PlanSummary, Reach, RemoteEntry, Route, SyncMode, SyncPlan,
};
pub use oplog::{
    Decision, ImportOp, NewChangeset, NewOp, OPLOG_FORMAT_VERSION, OPLOG_HISTORY_MS, OpOutcome,
    OplogChangeset, OplogEntry, OplogError, OplogFilter, OplogKey, OplogListed, OplogOp, OplogOpEntry,
    OplogPage, OplogStatus, check_actor, check_device, check_entity_uri, check_move, check_op, check_outcome,
    file_uri, file_uri_parts, folder_uri, format_rfc3339_utc, inverse_op, move_allowed, outcome_allowed,
    revert_result,
};
pub use presets::{PRESET_IDS, is_valid_preset_id};
pub use run::{ErrorKind, RunStatus};
pub use store::{
    AgentEntry, BinEntry, BinSide, Changeset, ChangesetStatus, ConflictRecord, DEFAULT_WATCH_IGNORES,
    EarlierVersion, EntryKind, EventLevel, JournalEntry, ManifestRow, NewBinEntry, NextMeta, Pair,
    PairStatus, RunEvent, RunRecord, RunStats, SavedCopy, SnapshotOrigin, SnapshotRow, StatKey, StorageMode,
    View, Watch, WatchGap,
};
