//! Files on demand on Linux (docs/ON_DEMAND.md §2.2, SRS FR-O4): a pair's on-demand view mounted
//! at a folder with FUSE 3. Every file shows at full size with its time; reading one goes through
//! the portable [`wasl_vfs::Hydrator`], so only the blocks a program reads are fetched.
//!
//! The crate has two layers:
//!
//! - [`View`] (every OS): what the mount shows. Inode numbers from a [`wasl_vfs::Tree`] (stable
//!   per path across refreshes), attributes, directory listings, open files, the
//!   `user.wasl.pinned` attribute, pins, and the atomic swap of the tree. Plain functions, tested
//!   on Windows, macOS and Linux alike.
//! - [`OnDemandMount`] (Linux): the FUSE file system over a [`View`], with the `fuser` crate's safe
//!   API in its pure-Rust mode (no libfuse to build against; at run time the `fusermount3` helper
//!   of the distribution's `fuse3` package mounts and unmounts). On other systems
//!   [`OnDemandMount::mount`] returns [`std::io::ErrorKind::Unsupported`], so the workspace builds
//!   everywhere.
//!
//! **Read-only for now.** Writes, creates, renames, deletes and attribute changes fail with
//! `EROFS` (and the mount itself is `ro`): local edits on on-demand pairs come in a later step,
//! where an edited file becomes an ordinary local file and syncs as today (§2.1 "Writes").

mod view;

pub use view::{Attr, Change, DirEntry, FsError, Kind, PINNED_XATTR, ROOT, View, XattrReply};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::OnDemandMount;

#[cfg(not(target_os = "linux"))]
mod stub;
#[cfg(not(target_os = "linux"))]
pub use stub::OnDemandMount;

use std::time::Duration;

/// How a mount behaves.
#[derive(Debug, Clone)]
pub struct MountOptions {
    /// The name `mount` and `df` show for the file system (`fsname`).
    pub name: String,
    /// How long the kernel may keep attributes and names before asking again. Refreshes also
    /// invalidate what changed, so this only bounds staleness if a notification is lost.
    pub ttl: Duration,
    /// How long [`OnDemandMount::unmount`] (and drop) waits for the FUSE thread to finish after
    /// unmounting. A folder still in use is detached lazily and ends when its last file closes.
    pub unmount_wait: Duration,
}

impl Default for MountOptions {
    fn default() -> Self {
        Self {
            name: "wasl".into(),
            ttl: Duration::from_secs(1),
            unmount_wait: Duration::from_secs(5),
        }
    }
}
