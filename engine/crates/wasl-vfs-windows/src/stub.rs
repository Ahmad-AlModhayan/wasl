//! Not Windows: the same API, every call [`Error::Unsupported`], so the workspace builds everywhere.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use wasl_vfs::{FileRef, Hydrator, Tree};

use crate::{Error, PlaceholderState, Result, RootConfig, SyncReport};

/// A pair's folder as a Cloud Files sync root (Windows only).
pub struct OnDemandRoot {
    _never: std::convert::Infallible,
}

impl OnDemandRoot {
    pub fn register(_config: RootConfig, _hydrator: Arc<Hydrator>) -> Result<Self> {
        Err(Error::Unsupported)
    }

    pub fn root(&self) -> &Path {
        match self._never {}
    }

    pub fn id(&self) -> &str {
        match self._never {}
    }

    pub fn sync(&self, _tree: &Tree) -> Result<SyncReport> {
        match self._never {}
    }

    pub fn pin(&self, _path: impl AsRef<Path>) -> Result<()> {
        match self._never {}
    }

    pub fn unpin(&self, _path: impl AsRef<Path>) -> Result<()> {
        match self._never {}
    }

    pub fn dehydrate(&self, _path: impl AsRef<Path>) -> Result<()> {
        match self._never {}
    }

    pub fn dehydrate_unpinned(&self) -> Result<()> {
        match self._never {}
    }

    pub fn mark_in_sync(&self, _path: impl AsRef<Path>, _file: &FileRef) -> Result<()> {
        match self._never {}
    }

    pub fn state(&self, _path: impl AsRef<Path>) -> Result<Option<PlaceholderState>> {
        match self._never {}
    }

    pub fn problems(&self) -> Vec<String> {
        match self._never {}
    }

    pub fn recent_fetches(&self) -> Vec<crate::FetchRecord> {
        match self._never {}
    }

    pub fn hydrate_all_and_unregister(self) -> Result<()> {
        match self._never {}
    }

    pub fn unregister(self) -> Result<()> {
        match self._never {}
    }
}

/// Whether `path` is inside a registered sync root (never, here).
pub fn is_sync_root(_path: &Path) -> bool {
    false
}

/// The registered sync roots of this user: (id, folder).
pub fn registered_sync_roots() -> Result<Vec<(String, PathBuf)>> {
    Err(Error::Unsupported)
}

/// Removes a registration by id (a leftover from a crash).
pub fn unregister_sync_root(_id: &str) -> Result<()> {
    Err(Error::Unsupported)
}
