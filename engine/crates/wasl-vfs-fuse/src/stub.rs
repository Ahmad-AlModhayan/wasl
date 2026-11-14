//! Not Linux: the same API, but nothing mounts. Windows has `wasl-vfs-windows` (Cloud Files);
//! macOS will have a File Provider extension (docs/ON_DEMAND.md §2.2).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::runtime::Handle;
use wasl_vfs::{Hydrator, Tree};

use crate::{MountOptions, View};

/// A pair's on-demand view mounted at a folder: FUSE, Linux only. [`OnDemandMount::mount`]
/// returns [`io::ErrorKind::Unsupported`] here.
pub struct OnDemandMount {
    view: Arc<View>,
    at: PathBuf,
}

impl OnDemandMount {
    pub fn mount(
        _at: impl AsRef<Path>,
        _tree: Tree,
        _hydrator: Arc<Hydrator>,
        _runtime: Handle,
        _options: &MountOptions,
    ) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "FUSE mounts are Linux only",
        ))
    }

    pub fn mountpoint(&self) -> &Path {
        &self.at
    }

    pub fn view(&self) -> &Arc<View> {
        &self.view
    }

    pub fn refresh(&self, tree: Tree) {
        self.view.refresh(tree);
    }

    pub async fn pin(&self, path: &str) -> wasl_vfs::Result<()> {
        self.view.pin(path).await
    }

    pub fn unpin(&self, path: &str) -> wasl_vfs::Result<()> {
        self.view.unpin(path)
    }

    pub fn is_pinned(&self, path: &str) -> bool {
        self.view.is_pinned(path)
    }

    pub fn unmount(self) -> io::Result<()> {
        Ok(())
    }
}
