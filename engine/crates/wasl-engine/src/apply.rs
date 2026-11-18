//! The one place a local file is changed on behalf of a plan. The Drive executor calls it today;
//! device pull follows (wasl#14). The install keeps one rule: the new content is on disk, flushed,
//! before it takes the name, and the old copy goes to the bin from beside the swap, never before
//! it, so no moment exists where the path is empty while the baseline says the file is there.

use crate::error::{EngineError, Result};
use crate::support::{Clock, new_id};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use wasl_core::LocalEntry;
use wasl_core::identity::name_key;
use wasl_core::paths::name_of;
use wasl_fs::{FsEntry, LocalFs};
use wasl_store::{BinSide, NewBinEntry, SharedStore};

/// Local bin retention (FR-F5).
pub const BIN_RETENTION_MS: i64 = 30 * 24 * 3600 * 1000;
/// Overwritten versions kept per file in the local bin (FR-F5).
pub const MAX_LOCAL_VERSIONS: u32 = 5;

/// Stops a test run at a named point (see `fault`); expands to nothing in a production build.
#[cfg(any(test, feature = "test-support"))]
macro_rules! kill_point {
    ($apply:expr, $point:ident) => {
        if let Some(plan) = &$apply.fault {
            plan.hit($crate::fault::Point::$point)?;
        }
    };
}
#[cfg(not(any(test, feature = "test-support")))]
macro_rules! kill_point {
    ($apply:expr, $point:ident) => {};
}

/// Run-report code for a file the apply step left alone because it no longer matched the plan.
pub const CHANGED_SINCE_PLANNED: &str = "CHANGED_SINCE_PLANNED";
/// The words that go with [`CHANGED_SINCE_PLANNED`] until an app words it from the catalogue.
pub const CHANGED_SINCE_PLANNED_MESSAGE: &str =
    "Changed on the device after the plan was made, so it was left alone. The next sync plans it again";

#[derive(Clone, Copy)]
struct Scanned {
    is_dir: bool,
    size: u64,
    mtime: i64,
}

pub(crate) struct Apply {
    /// The local entries the plan was made from, by name key. `None` turns the re-check off.
    expected: Option<HashMap<String, Scanned>>,
    pub store: SharedStore,
    pub local: Arc<dyn LocalFs>,
    pub clock: Arc<dyn Clock>,
    pub pair_id: String,
    pub pair_name: String,
    #[cfg(any(test, feature = "test-support"))]
    pub fault: Option<Arc<crate::fault::FaultPlan>>,
}

impl Apply {
    pub(crate) fn new(
        store: SharedStore,
        local: Arc<dyn LocalFs>,
        clock: Arc<dyn Clock>,
        pair: &wasl_store::Pair,
    ) -> Apply {
        Apply {
            expected: None,
            store,
            local,
            clock,
            pair_id: pair.id.clone(),
            pair_name: pair.name.clone(),
            #[cfg(any(test, feature = "test-support"))]
            fault: None,
        }
    }

    /// What the plan was made from: before a file is replaced or removed it is looked at again and
    /// compared with this.
    pub(crate) fn expecting<'a>(self, scanned: impl Iterator<Item = &'a LocalEntry>) -> Apply {
        self.expecting_entries(scanned.map(|e| (e.rel.as_str(), e.is_dir, e.size, e.mtime)))
    }

    /// [`Apply::expecting`] for a plan that holds its own picture of the folder (device pull's
    /// index): `(path, is_dir, size, mtime)` of each item the plan saw. An item not listed is one
    /// the plan saw nothing at.
    pub(crate) fn expecting_entries<'a>(
        mut self,
        seen: impl Iterator<Item = (&'a str, bool, u64, i64)>,
    ) -> Apply {
        self.expected = Some(
            seen.map(|(rel, is_dir, size, mtime)| (name_key(rel), Scanned { is_dir, size, mtime }))
                .collect(),
        );
        self
    }

    /// True when what is at `rel` now is what the plan saw: the same file (size and time), the same
    /// folder, or nothing where it saw nothing. A file that has vanished is fine either way (there
    /// is nothing to lose); a file where the plan saw none, a folder where it saw a file, or a file
    /// edited since is not.
    pub(crate) async fn as_planned(&self, rel: &str) -> Result<bool> {
        let Some(expected) = &self.expected else {
            return Ok(true);
        };
        let r = rel.to_owned();
        let now = self.fs(move |fs| fs.stat(&r)).await?;
        Ok(match (expected.get(&name_key(rel)), now) {
            (_, None) => true,
            (None, Some(_)) => false,
            (Some(w), Some(n)) => {
                w.is_dir == n.is_dir && (w.is_dir || (w.size == n.size && w.mtime == n.mtime_ms))
            }
        })
    }

    /// Creates folder `rel` (and missing parents).
    pub(crate) async fn mkdir(&self, rel: &str) -> Result<()> {
        self.check_name(rel)?;
        let r = rel.to_owned();
        self.fs(move |fs| fs.mkdir(&r)).await
    }

    /// Removes folder `rel` when it is empty, unless it changed since the plan. `false`: still
    /// holds something, or was left alone.
    pub(crate) async fn remove_empty_dir(&self, rel: &str) -> Result<bool> {
        if !self.as_planned(rel).await? {
            return Ok(false);
        }
        let r = rel.to_owned();
        let gone = self.fs(move |fs| fs.remove_dir_if_empty(&r)).await?;
        kill_point!(self, AfterRename);
        Ok(gone)
    }

    /// Renames `from` to `to` in one folder (the same item spelled another way, or a conflict
    /// copy's new name), unless `from` changed since the plan. `false`: left alone.
    pub(crate) async fn rename(&self, from: &str, to: &str) -> Result<bool> {
        self.check_name(to)?;
        if !self.as_planned(from).await? {
            return Ok(false);
        }
        kill_point!(self, BeforeRename);
        let (f, t) = (from.to_owned(), to.to_owned());
        self.fs(move |fs| fs.rename(&f, &t)).await?;
        kill_point!(self, AfterRename);
        Ok(true)
    }

    /// Moves a file the plan wants gone to the bin, unless it changed since the plan. `false`: left
    /// alone.
    pub(crate) async fn delete(&self, key: &str, rel: &str, reason: &str) -> Result<bool> {
        if !self.as_planned(rel).await? {
            return Ok(false);
        }
        self.bin(key, rel, reason).await?;
        kill_point!(self, AfterBin);
        Ok(true)
    }

    /// Test seam: stops where `fault` says.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn with_fault(mut self, fault: Option<Arc<crate::fault::FaultPlan>>) -> Apply {
        self.fault = fault;
        self
    }

    /// Every path given to the apply layer is a local name made by the one mapping function, so a
    /// path the disk still refuses is [`EngineError::NameRefused`], not a plain I/O failure.
    async fn fs<T: Send + 'static>(
        &self,
        f: impl FnOnce(&dyn LocalFs) -> std::io::Result<T> + Send + 'static,
    ) -> Result<T> {
        self.fs_raw(f).await?.map_err(EngineError::from_name_io)
    }

    /// Whether the disk takes `rel` (already mapped): every part passes [`wasl_core::names::refusal`]
    /// under the disk's rules. Checked before a file is fetched, so a name that can never be written
    /// costs no download. The disk's own refusal of a path comes back as the same error.
    pub(crate) fn check_name(&self, rel: &str) -> Result<()> {
        let rules = self.local.name_rules();
        for part in rel.split('/') {
            if let Some(why) = wasl_core::names::refusal(part, rules) {
                let why = match why {
                    wasl_core::names::NameRefusal::TooLong => "it is longer than the disk allows",
                    wasl_core::names::NameRefusal::Reserved => "the disk reserves it",
                };
                return Err(EngineError::NameRefused(format!(
                    "The disk refuses the name {part:?} even after Wasl mapped it: {why}. Rename it where it came from"
                )));
            }
        }
        Ok(())
    }

    /// Like [`Apply::fs`], but hands back the I/O error itself.
    async fn fs_raw<T: Send + 'static>(
        &self,
        f: impl FnOnce(&dyn LocalFs) -> std::io::Result<T> + Send + 'static,
    ) -> Result<std::io::Result<T>> {
        let fs = self.local.clone();
        tokio::task::spawn_blocking(move || f(fs.as_ref()))
            .await
            .map_err(|e| EngineError::Other(e.to_string()))
    }

    /// Whether a test's fault plan has stopped the run.
    pub(crate) fn killed(&self) -> bool {
        #[cfg(any(test, feature = "test-support"))]
        if let Some(p) = &self.fault {
            return p.is_dead();
        }
        false
    }

    /// Moves a local file to the bin and indexes it; trims old versions beyond the cap.
    pub(crate) async fn bin(&self, key: &str, rel: &str, reason: &str) -> Result<()> {
        let rel_owned = rel.to_owned();
        let Some(st) = self.fs(move |fs| fs.stat(&rel_owned)).await? else {
            return Ok(());
        };
        let rel_owned = rel.to_owned();
        let location = self.fs(move |fs| fs.move_to_bin(&rel_owned)).await?;
        self.index_binned(key, rel, reason, location, st.size)
            .await
            .map(|_| ())
    }

    /// Adds a bin entry for `location` and trims old versions; returns the entry's id.
    async fn index_binned(
        &self,
        key: &str,
        rel: &str,
        reason: &str,
        location: String,
        size: u64,
    ) -> Result<String> {
        let now = self.clock.now_ms();
        let id = new_id(self.clock.as_ref());
        let entry = NewBinEntry {
            id: id.clone(),
            pair_id: self.pair_id.clone(),
            side: BinSide::Local,
            key: key.to_owned(),
            reason: reason.to_owned(),
            location,
            deleted_at: now,
            expires_at: now + BIN_RETENTION_MS,
            size,
            account_id: None,
            remote_file_id: None,
            display_name: Some(name_of(rel).to_owned()),
            pair_name: Some(self.pair_name.clone()),
        };
        let excess = {
            let s = self.store.lock();
            s.add_to_bin(&entry)?;
            s.excess_local_versions(&self.pair_id, key, MAX_LOCAL_VERSIONS)?
        };
        for old in excess {
            let loc = old.entry.location.clone();
            if self.fs(move |fs| fs.purge_from_bin(&loc)).await.is_ok() {
                self.store.lock().remove_from_bin(&old.entry.id)?;
            }
        }
        Ok(id)
    }

    /// Puts the staged file at `rel`: flushes it, copies the file it replaces to the bin (when
    /// there is one), swaps the new file in with one atomic rename, then indexes the bin copy.
    pub(crate) async fn install(
        &self,
        staged: PathBuf,
        key: &str,
        rel: &str,
        mtime: Option<i64>,
        reason: &str,
    ) -> Result<Option<FsEntry>> {
        let r = self.install_staged(&staged, key, rel, mtime, reason).await;
        // A failed install doesn't leave a full-size staged file behind (a stopped test run does,
        // as a killed process would).
        if r.is_err() && !self.killed() {
            let _ = std::fs::remove_file(&staged);
        }
        r
    }

    async fn install_staged(
        &self,
        staged: &std::path::Path,
        key: &str,
        rel: &str,
        mtime: Option<i64>,
        reason: &str,
    ) -> Result<Option<FsEntry>> {
        let staged = staged.to_path_buf();
        self.check_name(rel)?;
        kill_point!(self, AfterStage);
        let to_flush = staged.clone();
        tokio::task::spawn_blocking(move || {
            std::fs::OpenOptions::new().write(true).open(to_flush)?.sync_all()
        })
        .await
        .map_err(|e| EngineError::Other(e.to_string()))??;
        kill_point!(self, AfterFlush);

        if !self.as_planned(rel).await? {
            let _ = std::fs::remove_file(&staged);
            return Ok(None);
        }
        let old_rel = rel.to_owned();
        let old = self.fs(move |fs| fs.stat(&old_rel)).await?;
        let old_rel = rel.to_owned();
        // The copy is indexed before the swap, so a stop anywhere leaves no bin file the index
        // does not know (it would never expire). A stop before the swap leaves an indexed copy of
        // a file that is still in place: the next run bins the same content again, and the cap on
        // versions and the retention time clear the extra.
        let copy = match self.fs_raw(move |fs| fs.bin_copy(&old_rel)).await? {
            Ok(Some(location)) => {
                let size = old.map_or(0, |o| o.size);
                let id = self
                    .index_binned(key, rel, reason, location.clone(), size)
                    .await?;
                Some((location, id))
            }
            Ok(None) => None,
            Err(e) if e.kind() == std::io::ErrorKind::Unsupported => {
                // This file system (or this kind of item) can't copy to the bin: the old way, the
                // bin first, then the install.
                self.bin(key, rel, reason).await?;
                None
            }
            Err(e) => return Err(EngineError::from_name_io(e)),
        };
        kill_point!(self, AfterBin);
        let rel_owned = rel.to_owned();
        let installed = match self.fs(move |fs| fs.install(&staged, &rel_owned, mtime)).await {
            Ok(i) => i,
            Err(e) => {
                // Nothing was replaced: the copy is not a version anyone lost.
                if let Some((loc, id)) = copy
                    && self.fs(move |fs| fs.purge_from_bin(&loc)).await.is_ok()
                {
                    let _ = self.store.lock().remove_from_bin(&id);
                }
                return Err(e);
            }
        };
        kill_point!(self, AfterRename);
        Ok(Some(installed))
    }

    /// Removes the file that was just uploaded in "upload then delete" mode, only while it is still
    /// the version that went up. `false`: left alone.
    pub(crate) async fn remove_uploaded(&self, rel: &str, size: u64, mtime: i64) -> Result<bool> {
        let r = rel.to_owned();
        let now = self.fs(move |fs| fs.stat(&r)).await?;
        if now
            .as_ref()
            .is_none_or(|n| n.is_dir || n.size != size || n.mtime_ms != mtime)
        {
            return Ok(false);
        }
        let r = rel.to_owned();
        self.fs(move |fs| fs.remove_file(&r)).await?;
        kill_point!(self, AfterBin);
        Ok(true)
    }

    /// The last point before a result goes into the baseline.
    pub(crate) fn before_record(&self) -> Result<()> {
        kill_point!(self, BeforeRecord);
        Ok(())
    }

    /// The point between removing the baseline row and moving the remote to Drive's bin.
    pub(crate) fn after_row_removed(&self) -> Result<()> {
        kill_point!(self, AfterRowRemoved);
        Ok(())
    }

    /// The point after a remote was moved to Drive's bin (move mode).
    pub(crate) fn after_remote_trash(&self) -> Result<()> {
        kill_point!(self, AfterRemoteTrash);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::SystemClock;
    use wasl_fs::StdFs;
    use wasl_store::{Pair, Store};

    fn apply() -> (Apply, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let (root, bin) = (dir.path().join("folder"), dir.path().join("bin"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let pair = Pair::new("p", "Shared", root.to_str().unwrap());
        let store = SharedStore::new(Store::open_in_memory().unwrap());
        let fs: Arc<dyn LocalFs> = Arc::new(StdFs::new(&root, &bin));
        (Apply::new(store, fs, Arc::new(SystemClock), &pair), dir)
    }

    #[test]
    fn a_name_over_the_byte_limit_is_refused_before_anything_is_fetched() {
        let (apply, _dir) = apply();
        assert!(apply.check_name("Docs/plain.txt").is_ok());
        // 150 two-byte letters: 150 characters, 300 bytes.
        let long = format!("Docs/{}.txt", "\u{e9}".repeat(150));
        let err = apply.check_name(&long).unwrap_err();
        assert!(matches!(err, EngineError::NameRefused(_)), "{err}");
        let fits = format!("Docs/{}.txt", "\u{e9}".repeat(100));
        assert!(apply.check_name(&fits).is_ok());
    }
}
