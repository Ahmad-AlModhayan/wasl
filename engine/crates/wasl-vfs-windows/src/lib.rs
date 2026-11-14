//! Files on demand on Windows (docs/ON_DEMAND.md §2.2, SRS FR-O2): a pair's local folder becomes a
//! Cloud Files sync root (the mechanism OneDrive uses). Every file of the pair's [`Tree`] is a
//! placeholder at full size with its time; reading one makes Windows ask for the bytes it needs,
//! which are served through [`wasl_vfs::Hydrator::read`], so a seek in a 20 GB file fetches one
//! block. Explorer's "Always keep on this device" and "Free up space" map to
//! [`wasl_vfs::Hydrator::pin`] / [`wasl_vfs::Hydrator::unpin`] and to hydrating / dehydrating
//! the placeholder.
//!
//! The Cloud Filter API is called through the `windows` crate. This is the only Wasl crate where
//! `unsafe` is allowed; it is kept to thin wrappers in `win::cf` and the two callbacks in
//! `win::fetch`, each block with its safety reasoning.
//! On other platforms the crate builds, and every entry point returns [`Error::Unsupported`].
//!
//! [`Tree`]: wasl_vfs::Tree

mod identity;

pub use identity::{
    Identity, decode as decode_identity, encode_dir as encode_dir_identity,
    encode_file as encode_file_identity,
};

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::{OnDemandRoot, is_sync_root, registered_sync_roots, unregister_sync_root};

#[cfg(windows)]
pub mod volume;

#[cfg(not(windows))]
mod stub;
#[cfg(not(windows))]
pub use stub::{OnDemandRoot, is_sync_root, registered_sync_roots, unregister_sync_root};

use std::path::PathBuf;

/// The provider name Explorer and the sync root id show.
pub const PROVIDER: &str = "Wasl";

/// How one pair's folder is registered.
#[derive(Debug, Clone)]
pub struct RootConfig {
    /// The pair's local folder (created if missing). Must be on NTFS.
    pub root: PathBuf,
    /// Stable for the pair's life; part of the sync root id, so it must not contain `!`.
    pub pair_id: String,
    /// What Explorer calls the folder in its navigation pane.
    pub display_name: String,
    /// The provider version Windows records with the registration.
    pub version: String,
    /// Explorer's icon for the folder (`path,-id` resource); a cloud icon when `None`.
    pub icon: Option<String>,
    /// Refuse reads of a dehydrated placeholder by this process itself (true for wasld).
    /// Windows serves a provider's own reads by hydrating the whole file, whatever the range, so
    /// a scan or hash of the folder by Wasl would download everything; with this, such a read
    /// fails instead. Pinning and [`OnDemandRoot::hydrate_all_and_unregister`] still work (they
    /// hydrate explicitly). Other programs' reads are not affected.
    pub block_self_hydration: bool,
    /// File placeholders carry the read-only attribute, so programs open them for reading only
    /// (a read-only view, docs/ON_DEMAND.md §2.3). A file made writable by hand and changed gets
    /// Drive's version back at the next [`OnDemandRoot::sync`] when [`RootConfig::set_aside`] is
    /// set; otherwise it is left alone and reported, as any local change.
    pub read_only: bool,
    /// Only for a read-only root: a placeholder changed here anyway (its read-only attribute
    /// cleared by hand) gets Drive's version back at the next [`OnDemandRoot::sync`], and the
    /// changed copy is moved here first (`<folder>/<time>/<path>`), so nothing the person wrote
    /// is lost (security review S3-2). `None`: a changed file is left alone and reported.
    pub set_aside: Option<PathBuf>,
}

impl RootConfig {
    /// The sync root id: `Wasl!<user SID>!<pair id>` (the form Windows documents).
    pub fn sync_root_id(&self, user_sid: &str) -> String {
        format!("{PROVIDER}!{user_sid}!{}", self.pair_id)
    }
}

/// What [`OnDemandRoot::sync`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    /// Placeholders created.
    pub created: usize,
    /// Placeholders whose content, size or time changed (dehydrated; read again on demand).
    pub updated: usize,
    /// Placeholders removed because the tree no longer has them.
    pub removed: usize,
    /// Regular files that matched the tree (size and time) and became full placeholders.
    pub converted: usize,
    /// Paths left alone, with why: local changes, local-only files, names Windows refuses.
    pub kept: Vec<wasl_vfs::Kept>,
    /// Files of a read-only root that were changed here and got Drive's version back: each
    /// path, and where its changed copy was set aside ([`RootConfig::set_aside`]).
    pub restored: Vec<(String, PathBuf)>,
}

/// One range Windows asked Wasl for, and which program's read caused it (diagnostics: shows
/// whether an app, the shell or a scanner made a file download).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchRecord {
    /// The requesting program's file name (e.g. `notepad.exe`), or `pid <n>` when Windows gives
    /// no image path.
    pub process: String,
    pub process_id: u32,
    /// Full path of the placeholder.
    pub path: String,
    pub offset: u64,
    pub len: u64,
}

/// A placeholder as Windows sees it (for `wasl status` and the tray app).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceholderState {
    /// "Always keep on this device".
    pub pinned: bool,
    /// False once it was changed here and not yet uploaded.
    pub in_sync: bool,
    /// Bytes of its content on the disk (0: dehydrated; the size: fully hydrated).
    pub on_disk: u64,
    /// The file it stands for (`None` for a folder).
    pub file: Option<wasl_vfs::FileRef>,
}

/// Why an on-demand operation failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("files on demand through Cloud Files need Windows 10 1809 or later")]
    Unsupported,
    #[error("{op} failed for {path}: {source}")]
    Os {
        op: &'static str,
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("not a placeholder of this on-demand folder: {0}")]
    NotOurs(String),
    #[error("outside the on-demand folder: {0}")]
    Outside(String),
    #[error("the pair id must not be empty or contain '!': {0:?}")]
    BadPairId(String),
    #[error(transparent)]
    Vfs(#[from] wasl_vfs::VfsError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
