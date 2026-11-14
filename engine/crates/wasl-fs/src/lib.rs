//! File-system side of the engine: local file access, scanning, hashing, atomic installs and the
//! recycle bin (SRS FR-C1, FR-F5, FR-N10).
//!
//! Everything goes through the [`LocalFs`] trait so the engine never touches a path it built
//! itself: [`StdFs`] serves desktop and Android's All files access, and Android implements the trait
//! over the Storage Access Framework. Every relative path is checked by [`check_rel`] first.
//!
//! Name mapping is pure, so it lives in `wasl-core` (the planner needs it); it is re-exported here.

mod hash;
mod local;
mod scan;
mod std_fs;

pub use hash::{ContentHash, hash_reader};
pub use local::{FsEntry, LocalFs, ReadablePath, check_rel, check_rel_on, is_internal_name};
pub use scan::{ScanOutcome, scan, scan_under};
pub use std_fs::StdFs;
pub use wasl_core::names;

/// Suffix of a download in progress, next to its target. Never synced, never listed.
pub const PART_SUFFIX: &str = ".wasl-part";
