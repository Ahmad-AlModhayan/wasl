//! Files on demand (docs/ON_DEMAND.md §2): a pair's files look whole on the device but are read
//! from Drive (or a vault) only when something opens them, a block at a time, kept in a bounded
//! cache, and pinned files stay complete on the device (SRS FR-O1–O5).
//!
//! This crate is the portable core: [`Hydrator`] serves byte ranges of a file from the
//! [`BlockCache`] or its [`Source`], with read-ahead for sequential readers. The platform back
//! ends (Cloud Files on Windows, FUSE on Linux, File Provider on macOS, Android's
//! DocumentsProvider) only translate the OS's requests into [`Hydrator::read`].

mod cache;
mod hydrator;
mod kept;
mod tree;

pub use cache::{BLOCK, BlockCache};
pub use hydrator::{Hydrator, ReadAhead};
pub use kept::{Kept, KeptKind};
pub use tree::{Node, Tree, safe_path};

use async_trait::async_trait;

/// One version of one remote file, as the listing knows it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileRef {
    /// Stable for the file's life (the Drive file id; a vault v2 file id).
    pub id: String,
    /// Changes whenever the content does (Drive's version or md5; a vault manifest's hash).
    pub version: String,
    pub size: u64,
}

/// Why a read failed.
#[derive(Debug, thiserror::Error)]
pub enum VfsError {
    #[error("the file is not available: {0}")]
    Unavailable(String),
    #[error("the content did not check out: {0}")]
    Integrity(String),
    #[error("the cache could not be used: {0}")]
    Cache(#[from] std::io::Error),
    #[error("read past the end of the file")]
    OutOfRange,
}

pub type Result<T, E = VfsError> = std::result::Result<T, E>;

/// Where file content comes from: Drive range requests, a vault (decrypting), a peer device.
#[async_trait]
pub trait Source: Send + Sync {
    /// Exactly `len` bytes of `file` from `offset` (fewer only at the end of the file).
    async fn read(&self, file: &FileRef, offset: u64, len: u64) -> Result<Vec<u8>>;

    /// The range a read of `offset..offset+len` should fetch: a vault v2 file widens it to its
    /// chunk boundaries, so a chunk is decrypted once. Default: the cache's blocks.
    fn natural_range(&self, _file: &FileRef, offset: u64, len: u64) -> (u64, u64) {
        (offset, len)
    }
}
