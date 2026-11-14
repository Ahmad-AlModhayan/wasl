//! Applies a plan (port of 1.1.4 `Executor.kt`). Every action that succeeds updates the baseline at
//! once, so an interrupted run resumes without repeating finished work (FR-B3). Folders first,
//! then bookkeeping, then transfers in parallel, then deletions deepest first.

use crate::error::{EngineError, Result};
use crate::remote::{Remote, RemoteInfo, UploadSpec};
use crate::support::{CancelToken, Clock, HashCache, Phase, Tracker, new_id};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;
use wasl_core::identity::name_key;
use wasl_core::names::{conflict_name, numbered_conflict_name};
use wasl_core::paths::{name_of, parent_of};
use wasl_core::{Action, BaselineEntry, LocalEntry, RemoteEntry, SyncMode, SyncPlan};
use wasl_fs::LocalFs;
use wasl_store::{BinSide, ConflictRecord, EventLevel, NewBinEntry, Pair, RunEvent, RunStats, SharedStore};

/// Conflict copies of one file within one minute before the rename is left to fail.
const MAX_CONFLICT_COPIES: u32 = 100;

/// `name` in the folder `parent` (`""` for the pair's top folder).
fn child(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

/// Local bin retention (FR-F5).
pub const BIN_RETENTION_MS: i64 = 30 * 24 * 3600 * 1000;
/// Overwritten versions kept per file in the local bin (FR-F5).
pub const MAX_LOCAL_VERSIONS: u32 = 5;
/// The start of a failed action's message, in words a user reads.
fn failed_to(a: &Action) -> &'static str {
    match a {
        Action::MkdirLocal { .. } => "Couldn't create the folder on the device.",
        Action::MkdirRemote { .. } => "Couldn't create the folder on Drive.",
        Action::Upload { .. } => "Couldn't upload.",
        Action::Download { .. } | Action::Export { .. } => "Couldn't download.",
        Action::DeleteLocal { .. } => "Couldn't move it to the recycle bin on the device.",
        Action::DeleteRemote { .. } => "Couldn't move it to Drive's bin.",
        Action::Conflict { .. } => "Couldn't keep both versions.",
        Action::Adopt { .. } | Action::Forget { .. } | Action::Skip { .. } | Action::RefreshState { .. } => {
            "Couldn't record it."
        }
    }
}

/// A file still being written while it uploaded. Nothing for the user to do.
pub const CHANGED_WHILE_UPLOADING: &str =
    "Changed while it was uploading (still being saved). The next sync uploads the latest version";
/// A file deleted on the device while it uploaded.
pub const DELETED_WHILE_UPLOADING: &str =
    "Deleted on the device while it was uploading. The next sync treats it as deleted";

pub(crate) struct Executor {
    pub pair: Pair,
    pub run_id: String,
    pub store: SharedStore,
    pub local: Arc<dyn LocalFs>,
    pub remote: Arc<dyn Remote>,
    /// Canonical key → Drive id; `""` is the pair's Drive folder.
    pub remote_ids: Mutex<HashMap<String, String>>,
    pub hashes: Arc<HashCache>,
    pub concurrency: usize,
    pub device_name: String,
    pub clock: Arc<dyn Clock>,
    pub tracker: Arc<Tracker>,
    pub cancel: Arc<CancelToken>,
    pub stats: Mutex<RunStats>,
    pub events: Mutex<Vec<RunEvent>>,
    pub fatal: Mutex<Option<EngineError>>,
    /// The first file that failed and why, for the run's one-line result.
    pub first_error: Mutex<Option<String>>,
    /// Local names (under `name_key`) this plan's downloads write. An export moving off one of them
    /// leaves the old file to that download, which bins it as it installs (#139).
    pub incoming: Mutex<HashSet<String>>,
}

impl Executor {
    pub(crate) async fn execute(self: &Arc<Self>, plan: &SyncPlan) {
        let bytes_total = plan.summary.upload_bytes + plan.summary.download_bytes;
        self.tracker.update(|p| {
            p.phase = Phase::Transferring;
            p.total_actions = plan.actions.len() as u64;
            p.bytes_total = bytes_total;
        });

        let sem = Arc::new(Semaphore::new(self.concurrency.clamp(1, 8)));
        // 1. folders, shallow first: one depth at a time, the folders of a depth in parallel (a
        // first sync of 600 folders made 600 calls one after another; perf audit 2026-10-29)
        let mut by_depth: std::collections::BTreeMap<usize, Vec<Action>> = Default::default();
        for a in plan.actions.iter().filter(|a| a.is_mkdir()) {
            by_depth
                .entry(a.key().matches('/').count())
                .or_default()
                .push(a.clone());
        }
        for level in by_depth.into_values() {
            if self.stopped() {
                return self.flush();
            }
            self.run_all(level, &sem).await;
        }
        // 2. bookkeeping
        for a in plan.actions.iter().filter(|a| {
            matches!(
                a,
                Action::Adopt { .. }
                    | Action::Forget { .. }
                    | Action::Skip { .. }
                    | Action::RefreshState { .. }
            )
        }) {
            if self.stopped() {
                return self.flush();
            }
            self.clone().run(a.clone()).await;
        }
        // 3. transfers in parallel
        *self.lock(&self.incoming) = plan
            .actions
            .iter()
            .filter_map(|a| match a {
                Action::Download { local_rel, .. } => Some(name_key(local_rel)),
                _ => None,
            })
            .collect();
        let transfers = plan
            .actions
            .iter()
            .filter(|a| {
                matches!(
                    a,
                    Action::Upload { .. }
                        | Action::Download { .. }
                        | Action::Export { .. }
                        | Action::Conflict { .. }
                )
            })
            .cloned()
            .collect();
        self.run_all(transfers, &sem).await;
        if self.stopped() {
            return self.flush();
        }
        // 4. deletions, deepest first, in order
        self.tracker.update(|p| p.phase = Phase::Deleting);
        for a in plan.actions.iter().filter(|a| a.is_delete()) {
            if self.stopped() {
                break;
            }
            self.clone().run(a.clone()).await;
        }
        self.flush();
    }

    /// Runs `actions` in parallel, at most the pair's concurrency at once, and waits for them all.
    async fn run_all(self: &Arc<Self>, actions: Vec<Action>, sem: &Arc<Semaphore>) {
        let mut tasks = tokio::task::JoinSet::new();
        for a in actions {
            let me = self.clone();
            let sem = sem.clone();
            tasks.spawn(async move {
                let Ok(_permit) = sem.acquire_owned().await else {
                    return;
                };
                if !me.stopped() {
                    me.run(a).await;
                }
            });
        }
        while tasks.join_next().await.is_some() {}
    }

    fn stopped(&self) -> bool {
        self.cancel.is_cancelled() || self.lock(&self.fatal).is_some()
    }

    fn lock<'a, T>(&self, m: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
        m.lock().unwrap_or_else(|p| p.into_inner())
    }

    async fn run(self: Arc<Self>, a: Action) {
        let result = tokio::select! {
            r = self.handle(&a) => r,
            _ = self.cancel.cancelled() => Err(EngineError::Cancelled),
        };
        let result = match result {
            Err(EngineError::Vanished(m)) => match self.fs(|fs| Ok(crate::support::root_gone(fs))).await {
                Ok(Some(e)) => Err(EngineError::from(e)),
                _ => Err(EngineError::Vanished(m)),
            },
            r => r,
        };
        if let Err(EngineError::Vanished(message)) = &result {
            // Moved or deleted on the device since the scan: the next sync sees what is there.
            self.lock(&self.stats).skipped += 1;
            self.event(EventLevel::Warn, "SKIP", Some(a.key()), message, None, 0);
        } else if let Err(e) = &result
            && e.is_flagged_by_drive()
        {
            // Someone else's file Drive will not serve: nothing a retry or the user here can change,
            // so it is skipped with the reason instead of failing every run.
            self.lock(&self.stats).skipped += 1;
            self.event(
                EventLevel::Warn,
                "SKIP",
                Some(a.key()),
                &e.user_message(),
                Some(e.to_string()),
                0,
            );
        } else if let Err(e) = result
            && !matches!(e, EngineError::Cancelled)
        {
            self.lock(&self.stats).errors += 1;
            let message = format!("{} {}", failed_to(&a), e.user_message());
            self.lock(&self.first_error)
                .get_or_insert_with(|| format!("{}: {message}", name_of(a.key())));
            self.event(
                EventLevel::Error,
                "ERROR",
                Some(a.key()),
                &message,
                Some(e.to_string()),
                0,
            );
            if e.is_fatal() {
                *self.lock(&self.fatal) = Some(e);
            }
        }
        let errors = u64::from(self.lock(&self.stats).errors);
        self.tracker.update(|p| {
            p.done_actions += 1;
            p.errors = errors;
        });
        if self.lock(&self.events).len() >= 20 {
            self.flush();
        }
    }

    async fn handle(&self, a: &Action) -> Result<()> {
        match a {
            Action::MkdirLocal { key, local_rel } => self.mkdir_local(key, local_rel).await,
            Action::MkdirRemote {
                key,
                parent_key,
                name,
            } => self.mkdir_remote(key, parent_key, name).await,
            Action::Upload {
                key,
                local,
                existing_remote_id,
                parent_key,
                name,
                reason,
            } => {
                self.upload(
                    key,
                    local,
                    existing_remote_id.as_deref(),
                    parent_key,
                    name,
                    reason,
                )
                .await
            }
            Action::Download {
                key,
                remote,
                local_rel,
                reason,
                ..
            } => self.download(key, remote, local_rel, reason).await,
            Action::Export {
                key,
                remote,
                local_rel,
                mime,
                ..
            } => self.export(key, remote, local_rel, mime).await,
            Action::DeleteLocal {
                key,
                local_rel,
                is_dir,
                reason,
            } => self.delete_local(key, local_rel, *is_dir, reason).await,
            Action::DeleteRemote {
                key,
                remote_id,
                is_dir,
                reason,
            } => self.delete_remote(key, remote_id, *is_dir, reason).await,
            Action::Conflict {
                key,
                local,
                remote,
                local_rel,
                parent_key,
            } => self.conflict(key, local, remote, local_rel, parent_key).await,
            Action::Adopt {
                key,
                local,
                remote,
                local_rel,
            } => self.adopt(key, local, remote, local_rel).await,
            Action::Forget { key } => Ok(self.store.lock().remove_baseline(&self.pair.id, key)?),
            Action::Skip { key, reason, warn } => {
                self.lock(&self.stats).skipped += 1;
                let level = if *warn { EventLevel::Warn } else { EventLevel::Info };
                self.event(level, "SKIP", Some(key), reason, None, 0);
                Ok(())
            }
            Action::RefreshState {
                key,
                local,
                remote,
                local_rel,
                export_mime,
            } => {
                self.refresh(
                    key,
                    local.as_ref(),
                    remote.as_ref(),
                    local_rel,
                    export_mime.as_deref(),
                )
                .await
            }
        }
    }

    // ------------------------------------------------------------------ helpers

    /// Runs a blocking file-system call off the async threads.
    async fn fs<T: Send + 'static>(
        &self,
        f: impl FnOnce(&dyn LocalFs) -> std::io::Result<T> + Send + 'static,
    ) -> Result<T> {
        let fs = self.local.clone();
        Ok(tokio::task::spawn_blocking(move || f(fs.as_ref()))
            .await
            .map_err(|e| EngineError::Other(e.to_string()))??)
    }

    fn remote_id(&self, key: &str) -> Option<String> {
        self.lock(&self.remote_ids).get(key).cloned()
    }

    fn put_baseline(&self, e: &BaselineEntry) -> Result<()> {
        Ok(self.store.lock().put_baseline(&self.pair.id, e)?)
    }

    fn event(
        &self,
        level: EventLevel,
        action: &str,
        key: Option<&str>,
        message: &str,
        detail: Option<String>,
        bytes: u64,
    ) {
        self.lock(&self.events).push(RunEvent {
            at: self.clock.now_ms(),
            level,
            action: action.to_owned(),
            key: key.map(str::to_owned),
            message: message.to_owned(),
            detail,
            bytes,
        });
    }

    pub(crate) fn flush(&self) {
        let events = std::mem::take(&mut *self.lock(&self.events));
        if !events.is_empty() {
            // History is best effort: a full disk must not turn a finished transfer into a failure.
            let _ = self.store.lock().append_events(&self.run_id, &events);
        }
    }

    /// Counts a transfer's bytes toward the run's progress, at most `size` (the file's own size):
    /// a vault remote reports its padded, encrypted bytes, which would push the total past 100%.
    fn progress_fn(&self, size: u64) -> impl Fn(u64) + Send + Sync + '_ {
        let last = std::sync::atomic::AtomicU64::new(0);
        move |done: u64| {
            let done = done.min(size);
            let prev = last.swap(done, std::sync::atomic::Ordering::Relaxed);
            let delta = done.saturating_sub(prev);
            // A transfer that reports no new bytes is not progress: the watchdog must still see it stall.
            if delta > 0 {
                self.tracker.update(|p| p.bytes_done += delta);
            }
        }
    }

    /// Moves a local file to the bin and indexes it; trims old versions beyond the cap.
    async fn bin_local(&self, key: &str, rel: &str, reason: &str) -> Result<()> {
        let rel_owned = rel.to_owned();
        let Some(st) = self.fs(move |fs| fs.stat(&rel_owned)).await? else {
            return Ok(());
        };
        let rel_owned = rel.to_owned();
        let location = self.fs(move |fs| fs.move_to_bin(&rel_owned)).await?;
        let now = self.clock.now_ms();
        let entry = NewBinEntry {
            id: new_id(self.clock.as_ref()),
            pair_id: self.pair.id.clone(),
            side: BinSide::Local,
            key: key.to_owned(),
            reason: reason.to_owned(),
            location,
            deleted_at: now,
            expires_at: now + BIN_RETENTION_MS,
            size: st.size,
            account_id: None,
            remote_file_id: None,
            display_name: Some(name_of(rel).to_owned()),
            pair_name: Some(self.pair.name.clone()),
        };
        let excess = {
            let s = self.store.lock();
            s.add_to_bin(&entry)?;
            s.excess_local_versions(&self.pair.id, key, MAX_LOCAL_VERSIONS)?
        };
        for old in excess {
            let loc = old.entry.location.clone();
            if self.fs(move |fs| fs.purge_from_bin(&loc)).await.is_ok() {
                self.store.lock().remove_from_bin(&old.entry.id)?;
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------ handlers

    async fn mkdir_local(&self, key: &str, local_rel: &str) -> Result<()> {
        let rel = local_rel.to_owned();
        let st = self
            .fs(move |fs| {
                fs.mkdir(&rel)?;
                fs.stat(&rel)
            })
            .await?;
        let mut e = folder_entry(key, local_rel);
        e.local_mtime = st.map_or(0, |s| s.mtime_ms);
        e.remote_id = self.remote_id(key);
        self.put_baseline(&e)?;
        self.event(
            EventLevel::Info,
            "MKDIR",
            Some(key),
            "Created folder on device",
            None,
            0,
        );
        Ok(())
    }

    async fn mkdir_remote(&self, key: &str, parent_key: &str, name: &str) -> Result<()> {
        let parent = self
            .remote_id(parent_key)
            .ok_or_else(|| EngineError::NotFound(format!("parent folder for {key} is missing on Drive")))?;
        let info = self.remote.create_folder(&parent, name).await?;
        self.lock(&self.remote_ids)
            .insert(key.to_owned(), info.id.clone());
        let local_rel = self
            .store
            .lock()
            .baseline_entry(&self.pair.id, key)?
            .map_or_else(|| key.to_owned(), |b| b.local_rel);
        let mut e = folder_entry(key, &local_rel);
        e.remote_id = Some(info.id);
        e.remote_mtime = Some(info.mtime);
        self.put_baseline(&e)?;
        self.event(
            EventLevel::Info,
            "MKDIR",
            Some(key),
            "Created folder on Drive",
            None,
            0,
        );
        Ok(())
    }

    /// Records what an upload left on Drive as Wasl's own copy, with the device side unknown, so
    /// the next sync replaces it with the device file (or treats that as deleted) instead of
    /// keeping both as a conflict.
    fn record_own_upload(&self, key: &str, local_rel: &str, info: &RemoteInfo) -> Result<()> {
        self.lock(&self.remote_ids)
            .insert(key.to_owned(), info.id.clone());
        self.put_baseline(&BaselineEntry {
            key: key.to_owned(),
            local_rel: local_rel.to_owned(),
            is_dir: false,
            local_size: 0,
            local_mtime: 0,
            local_hash: None,
            remote_id: Some(info.id.clone()),
            remote_md5: info.md5.clone(),
            remote_size: info.size,
            remote_mtime: Some(info.mtime),
            remote_version: info.version,
            export_mime: None,
        })
    }

    async fn upload(
        &self,
        key: &str,
        local: &LocalEntry,
        existing: Option<&str>,
        parent_key: &str,
        name: &str,
        reason: &str,
    ) -> Result<()> {
        let parent = self
            .remote_id(parent_key)
            .ok_or_else(|| EngineError::NotFound(format!("parent folder for {key} is missing on Drive")))?;
        let hash = self
            .hashes
            .hash(&self.local, local)
            .await
            .map_err(EngineError::file_read)?;
        if let Some(id) = existing
            && self.replaced_on_drive(key, id, &hash.md5).await?
        {
            self.lock(&self.stats).skipped += 1;
            self.event(
                EventLevel::Warn,
                "SKIP",
                Some(key),
                "Not uploaded: it changed on Drive during this sync. The next sync keeps both versions",
                None,
                0,
            );
            return Ok(());
        }
        let session_key = format!("{}|{}|{}", self.pair.id, key, hash.md5);
        self.tracker.update(|p| p.current_file = Some(local.rel.clone()));
        let mut hash = hash;
        let mut fresh = None;
        let mut replace = existing.map(str::to_owned);
        let mut attempt = 0;
        let info = loop {
            attempt += 1;
            let rel = local.rel.clone();
            let readable = self
                .fs(move |fs| {
                    Ok(fs
                        .readable_path(&rel)
                        .map_err(|e| crate::support::unreadable(&rel, e)))
                })
                .await?
                .map_err(EngineError::file_read)?;
            let progress = self.progress_fn(local.size);
            let info = self
                .remote
                .upload(
                    UploadSpec {
                        src: &readable.path,
                        name,
                        parent_id: &parent,
                        existing_id: replace.as_deref(),
                        mtime_ms: local.mtime,
                        // A second attempt starts over: the first one's session finished.
                        session_key: (attempt == 1).then_some(session_key.as_str()),
                    },
                    &progress,
                )
                .await?;
            drop(readable);
            let Some(md5) = info.md5.clone().filter(|_| self.remote.plaintext_hashes()) else {
                break info;
            };
            if md5.eq_ignore_ascii_case(&hash.md5) {
                break info;
            }
            // Drive has other bytes than the ones hashed. Read the file again to tell why (the
            // owner's phone, 2026-10-29: two .xlsx files stopped the run with nothing the user could do).
            let rel = local.rel.clone();
            let (now, h) = self
                .fs(move |fs| match fs.stat(&rel)? {
                    Some(now) => Ok((Some(now), Some(crate::support::read_hash(fs, &rel)?))),
                    None => Ok((None, None)),
                })
                .await?;
            match (now, h) {
                // Saved again just before the upload read it: Drive has exactly what the device has.
                (Some(now), Some(h)) if md5.eq_ignore_ascii_case(&h.md5) => {
                    fresh = Some(LocalEntry {
                        rel: local.rel.clone(),
                        is_dir: false,
                        size: now.size,
                        mtime: now.mtime_ms,
                    });
                    hash = h;
                    break info;
                }
                // The device file is unchanged, so Drive received it damaged: send it again, over
                // the damaged copy.
                (Some(_), Some(h)) if h.md5.eq_ignore_ascii_case(&hash.md5) => {
                    if attempt < 2 {
                        replace = Some(info.id.clone());
                        continue;
                    }
                    self.record_own_upload(key, &local.rel, &info)?;
                    return Err(EngineError::Integrity(format!(
                        "Drive received it damaged twice (Drive has md5 {md5}, the file is {}). The next sync uploads it again",
                        hash.md5
                    )));
                }
                // Deleted since: the next sync sees it gone and treats it as deleted.
                (None, _) => {
                    self.record_own_upload(key, &local.rel, &info)?;
                    self.lock(&self.stats).skipped += 1;
                    self.event(
                        EventLevel::Warn,
                        "SKIP",
                        Some(key),
                        DELETED_WHILE_UPLOADING,
                        None,
                        0,
                    );
                    return Ok(());
                }
                // Still being written: the next sync uploads the finished file over Wasl's copy.
                _ => {
                    self.record_own_upload(key, &local.rel, &info)?;
                    self.lock(&self.stats).skipped += 1;
                    self.event(
                        EventLevel::Warn,
                        "SKIP",
                        Some(key),
                        CHANGED_WHILE_UPLOADING,
                        None,
                        0,
                    );
                    return Ok(());
                }
            }
        };
        let local = fresh.as_ref().unwrap_or(local);
        self.lock(&self.remote_ids)
            .insert(key.to_owned(), info.id.clone());
        self.put_baseline(&BaselineEntry {
            key: key.to_owned(),
            local_rel: local.rel.clone(),
            is_dir: false,
            local_size: local.size,
            local_mtime: local.mtime,
            local_hash: Some(hash.md5.clone()),
            remote_id: Some(info.id.clone()),
            remote_md5: info.md5.clone(),
            remote_size: info.size,
            remote_mtime: Some(info.mtime),
            remote_version: info.version,
            export_mime: None,
        })?;
        {
            let mut s = self.lock(&self.stats);
            s.uploaded += 1;
            s.bytes_up += local.size;
        }
        self.event(
            EventLevel::Info,
            "UPLOAD",
            Some(key),
            &format!("Uploaded ({reason})"),
            Some(format!("md5:{}", hash.md5)),
            local.size,
        );

        if self.pair.method == SyncMode::UploadThenDelete {
            // Only the version that was uploaded may go. A file edited during the upload stays, and
            // the next run uploads the new version.
            let rel = local.rel.clone();
            let now = self.fs(move |fs| fs.stat(&rel)).await?;
            if now
                .as_ref()
                .is_none_or(|n| n.size != local.size || n.mtime_ms != local.mtime)
            {
                self.event(
                    EventLevel::Warn,
                    "SKIP",
                    Some(key),
                    "Kept on the device: it changed while it was uploading",
                    None,
                    0,
                );
                return Ok(());
            }
            let rel = local.rel.clone();
            self.fs(move |fs| fs.remove_file(&rel)).await?;
            self.store.lock().remove_baseline(&self.pair.id, key)?;
            self.event(
                EventLevel::Info,
                "DELETE_LOCAL",
                Some(key),
                "Removed from the device after upload",
                None,
                0,
            );
        }
        Ok(())
    }

    /// True when the Drive file an upload would replace is no longer the version last synced, and
    /// not this content either: another device uploaded it after the listing this plan was made
    /// from (Drive's change list reports uploads seconds late). Replacing it would lose that
    /// version; the next sync, listing it, keeps both.
    async fn replaced_on_drive(&self, key: &str, id: &str, local_md5: &str) -> Result<bool> {
        if !self.remote.plaintext_hashes() {
            return Ok(false);
        }
        let Some(synced) = self
            .store
            .lock()
            .baseline_entry(&self.pair.id, key)?
            .and_then(|b| b.remote_md5)
        else {
            return Ok(false);
        };
        let Some(now) = self.remote.current(id).await? else {
            return Ok(false);
        };
        Ok(now
            .md5
            .is_some_and(|m| !m.eq_ignore_ascii_case(&synced) && !m.eq_ignore_ascii_case(local_md5)))
    }

    async fn download(&self, key: &str, remote: &RemoteEntry, local_rel: &str, reason: &str) -> Result<()> {
        let rel = local_rel.to_owned();
        let staging = self.fs(move |fs| fs.staging_path(&rel)).await?;
        self.tracker
            .update(|p| p.current_file = Some(local_rel.to_owned()));
        let progress = self.progress_fn(remote.size.unwrap_or(u64::MAX));
        let mut attempt = 0;
        let hash = loop {
            attempt += 1;
            self.remote.download(remote, &staging, &progress).await?;
            let staged = staging.clone();
            let hash = tokio::task::spawn_blocking(move || {
                std::fs::File::open(&staged).and_then(wasl_fs::hash_reader)
            })
            .await
            .map_err(|e| EngineError::Other(e.to_string()))??;
            let Some(md5) = remote.md5.as_ref().filter(|_| self.remote.plaintext_hashes()) else {
                break hash;
            };
            if md5.eq_ignore_ascii_case(&hash.md5) {
                break hash;
            }
            let _ = std::fs::remove_file(&staging);
            // Changed on Drive since it was listed: the next sync sees the new version and gets it.
            let now = self.remote.current(&remote.id).await?;
            if now.is_none_or(|n| n.md5.as_deref().is_none_or(|m| !m.eq_ignore_ascii_case(md5))) {
                self.lock(&self.stats).skipped += 1;
                self.event(
                    EventLevel::Warn,
                    "SKIP",
                    Some(key),
                    "Changed on Drive while it was downloading. The next sync downloads the latest version",
                    None,
                    0,
                );
                return Ok(());
            }
            // Same file on Drive, damaged on the way: fetch it once more.
            if attempt >= 2 {
                return Err(EngineError::Integrity(format!(
                    "The download arrived damaged twice (it hashes to {}, Drive says {md5}). The next sync tries again",
                    hash.md5
                )));
            }
        };
        self.bin_local(key, local_rel, "OVERWRITTEN").await?;
        let rel = local_rel.to_owned();
        let mtime = self.pair.preserve_mtime.then_some(remote.mtime);
        let installed = self.fs(move |fs| fs.install(&staging, &rel, mtime)).await?;
        self.put_baseline(&BaselineEntry {
            key: key.to_owned(),
            local_rel: local_rel.to_owned(),
            is_dir: false,
            local_size: installed.size,
            local_mtime: installed.mtime_ms,
            local_hash: Some(hash.md5),
            remote_id: Some(remote.id.clone()),
            remote_md5: remote.md5.clone(),
            remote_size: remote.size,
            remote_mtime: Some(remote.mtime),
            remote_version: remote.version,
            export_mime: None,
        })?;
        {
            let mut s = self.lock(&self.stats);
            s.downloaded += 1;
            s.bytes_down += installed.size;
        }
        self.event(
            EventLevel::Info,
            "DOWNLOAD",
            Some(key),
            &format!("Downloaded ({reason})"),
            None,
            installed.size,
        );
        if self.remote.flagged_by_remote(&remote.id) {
            self.event(
                EventLevel::Warn,
                "INFO",
                Some(key),
                "Google Drive flagged this file as malware or spam. It is yours, so Wasl \
                 downloaded it anyway; Drive's scan is often wrong about code and config files.",
                None,
                0,
            );
        }

        if self.pair.method == SyncMode::DownloadThenDelete {
            self.remote.trash(&remote.id).await?;
            self.store.lock().remove_baseline(&self.pair.id, key)?;
            self.event(
                EventLevel::Info,
                "DELETE_REMOTE",
                Some(key),
                "Moved to Drive's bin after download (move mode)",
                None,
                0,
            );
        }
        Ok(())
    }

    async fn export(&self, key: &str, remote: &RemoteEntry, local_rel: &str, mime: &str) -> Result<()> {
        let rel = local_rel.to_owned();
        let staging = self.fs(move |fs| fs.staging_path(&rel)).await?;
        self.tracker
            .update(|p| p.current_file = Some(local_rel.to_owned()));
        let progress = self.progress_fn(u64::MAX);
        self.remote.export(remote, mime, &staging, &progress).await?;
        let staged = staging.clone();
        let hash =
            tokio::task::spawn_blocking(move || std::fs::File::open(&staged).and_then(wasl_fs::hash_reader))
                .await
                .map_err(|e| EngineError::Other(e.to_string()))??;
        // A previous export under another name (the format changed, or a real file took the name)
        // goes to the bin only while it is still that export: another item holds none of its name,
        // and the file there has the size and time the export left (#139). Anything else is a real
        // file, maybe the same file in another case, or one downloaded a moment ago.
        let prev = self.store.lock().baseline_entry(&self.pair.id, key)?;
        if let Some(p) = prev
            && p.export_mime.is_some()
            && p.local_rel != local_rel
        {
            let rel = p.local_rel.clone();
            let on_disk = self.fs(move |fs| fs.stat(&rel)).await?;
            let held = self
                .store
                .lock()
                .local_rel_held_by_other(&self.pair.id, &p.local_rel, key)?
                || self.lock(&self.incoming).contains(&name_key(&p.local_rel));
            match on_disk {
                Some(st)
                    if !held && !st.is_dir && st.size == p.local_size && st.mtime_ms == p.local_mtime =>
                {
                    self.bin_local(key, &p.local_rel, "OVERWRITTEN").await?;
                }
                Some(_) => self.event(
                    EventLevel::Info,
                    "EXPORT",
                    Some(key),
                    &format!(
                        "Left '{}' in place: it is no longer this Google file's export",
                        p.local_rel
                    ),
                    None,
                    0,
                ),
                None => {}
            }
        }
        self.bin_local(key, local_rel, "OVERWRITTEN").await?;
        let rel = local_rel.to_owned();
        let mtime = self.pair.preserve_mtime.then_some(remote.mtime);
        let installed = self.fs(move |fs| fs.install(&staging, &rel, mtime)).await?;
        self.put_baseline(&BaselineEntry {
            key: key.to_owned(),
            local_rel: local_rel.to_owned(),
            is_dir: false,
            local_size: installed.size,
            local_mtime: installed.mtime_ms,
            local_hash: Some(hash.md5),
            remote_id: Some(remote.id.clone()),
            remote_md5: None,
            remote_size: remote.size,
            remote_mtime: Some(remote.mtime),
            remote_version: remote.version,
            export_mime: Some(mime.to_owned()),
        })?;
        {
            let mut s = self.lock(&self.stats);
            s.exported += 1;
            s.bytes_down += installed.size;
        }
        let ext = local_rel.rsplit('.').next().unwrap_or_default();
        self.event(
            EventLevel::Info,
            "EXPORT",
            Some(key),
            &format!("Exported Google Doc as {ext}"),
            None,
            installed.size,
        );
        Ok(())
    }

    async fn delete_local(&self, key: &str, local_rel: &str, is_dir: bool, reason: &str) -> Result<()> {
        let mut size = 0;
        if is_dir {
            let rel = local_rel.to_owned();
            let (removed, exists) = self
                .fs(move |fs| Ok((fs.remove_dir_if_empty(&rel)?, fs.stat(&rel)?.is_some())))
                .await?;
            if !removed && exists {
                self.event(
                    EventLevel::Warn,
                    "SKIP",
                    Some(key),
                    "Folder not empty on the device; left in place",
                    None,
                    0,
                );
                return Ok(());
            }
            self.store.lock().remove_baseline_subtree(&self.pair.id, key)?;
        } else {
            let rel = local_rel.to_owned();
            size = self.fs(move |fs| fs.stat(&rel)).await?.map_or(0, |s| s.size);
            self.bin_local(key, local_rel, "DELETED").await?;
            self.store.lock().remove_baseline(&self.pair.id, key)?;
        }
        self.lock(&self.stats).deleted_local += 1;
        let msg = if is_dir {
            format!("Removed folder on device ({reason})")
        } else {
            format!("Moved to the recycle bin ({reason})")
        };
        self.event(EventLevel::Info, "DELETE_LOCAL", Some(key), &msg, None, size);
        Ok(())
    }

    async fn delete_remote(&self, key: &str, remote_id: &str, is_dir: bool, reason: &str) -> Result<()> {
        let prev = self.store.lock().baseline_entry(&self.pair.id, key)?;
        self.remote.trash(remote_id).await?;
        let now = self.clock.now_ms();
        let size = prev.as_ref().and_then(|p| p.remote_size).unwrap_or(0);
        {
            let s = self.store.lock();
            s.add_to_bin(&NewBinEntry {
                id: new_id(self.clock.as_ref()),
                pair_id: self.pair.id.clone(),
                side: BinSide::Drive,
                key: key.to_owned(),
                reason: "DELETED".into(),
                location: String::new(),
                deleted_at: now,
                expires_at: now + BIN_RETENTION_MS,
                size,
                account_id: self.pair.account_id.clone(),
                remote_file_id: Some(remote_id.to_owned()),
                display_name: Some(name_of(key).to_owned()),
                pair_name: Some(self.pair.name.clone()),
            })?;
            if is_dir {
                s.remove_baseline_subtree(&self.pair.id, key)?;
            } else {
                s.remove_baseline(&self.pair.id, key)?;
            }
        }
        self.lock(&self.stats).deleted_remote += 1;
        self.event(
            EventLevel::Info,
            "DELETE_REMOTE",
            Some(key),
            &format!("Moved to Drive's bin ({reason})"),
            None,
            size,
        );
        Ok(())
    }

    /// Keep both: the local copy is renamed to a conflict name and uploaded; the Drive version takes
    /// the original path.
    async fn conflict(
        &self,
        key: &str,
        local: &LocalEntry,
        remote: &RemoteEntry,
        local_rel: &str,
        parent_key: &str,
    ) -> Result<()> {
        let (name, device, stamp) = (
            name_of(local_rel).to_owned(),
            self.device_name.clone(),
            self.clock.stamp(),
        );
        let parent_rel = parent_of(local_rel).to_owned();
        let from = local_rel.to_owned();
        let (copy_name, copy_rel, renamed) = self
            .fs(move |fs| {
                let mut copy = conflict_name(&name, &device, &stamp);
                let mut copy_rel = child(&parent_rel, &copy);
                if fs.stat(&from)?.is_some() {
                    // The stamp is to the minute: a second conflict on this file within it numbers
                    // its copy rather than fail on the first one's name (#145).
                    let mut n = 1;
                    while fs.stat(&copy_rel)?.is_some() && n < MAX_CONFLICT_COPIES {
                        n += 1;
                        copy = numbered_conflict_name(&name, &device, &stamp, n);
                        copy_rel = child(&parent_rel, &copy);
                    }
                    fs.rename(&from, &copy_rel)?;
                }
                let st = fs.stat(&copy_rel)?;
                Ok((copy, copy_rel, st))
            })
            .await?;
        let copy_key = child(parent_key, &copy_name);
        self.lock(&self.stats).conflicts += 1;
        self.store.lock().add_conflict(&ConflictRecord {
            id: new_id(self.clock.as_ref()),
            pair_id: self.pair.id.clone(),
            key: key.to_owned(),
            detected_at: self.clock.now_ms(),
            policy: self.pair.conflict_policy,
            local_copy_rel: Some(copy_rel.clone()),
            remote_file_id: Some(remote.id.clone()),
            message: format!(
                "Modified on both sides. Device version kept as \"{copy_name}\"; Drive version restored to the original name."
            ),
            resolved: false,
            resolved_at: None,
        })?;
        self.event(
            EventLevel::Warn,
            "CONFLICT",
            Some(key),
            &format!("Both sides changed. Kept device copy as \"{copy_name}\""),
            None,
            0,
        );
        if remote.is_dir || local.is_dir {
            // A file on one side and a folder on the other: the rename alone resolves it.
            self.store.lock().remove_baseline(&self.pair.id, key)?;
            return Ok(());
        }
        if let Some(st) = renamed.filter(|s| !s.is_dir) {
            let entry = LocalEntry {
                rel: copy_rel,
                is_dir: false,
                size: st.size,
                mtime: st.mtime_ms,
            };
            Box::pin(self.upload(&copy_key, &entry, None, parent_key, &copy_name, "conflict copy")).await?;
        }
        Box::pin(self.download(key, remote, local_rel, "conflict: Drive version")).await
    }

    async fn adopt(
        &self,
        key: &str,
        local: &LocalEntry,
        remote: &RemoteEntry,
        local_rel: &str,
    ) -> Result<()> {
        let hash = self
            .hashes
            .hash(&self.local, local)
            .await
            .map_err(EngineError::file_read)?;
        self.put_baseline(&BaselineEntry {
            key: key.to_owned(),
            local_rel: local_rel.to_owned(),
            is_dir: false,
            local_size: local.size,
            local_mtime: local.mtime,
            local_hash: Some(hash.md5),
            remote_id: Some(remote.id.clone()),
            remote_md5: remote.md5.clone(),
            remote_size: remote.size,
            remote_mtime: Some(remote.mtime),
            remote_version: remote.version,
            export_mime: None,
        })?;
        self.lock(&self.remote_ids)
            .insert(key.to_owned(), remote.id.clone());
        Ok(())
    }

    async fn refresh(
        &self,
        key: &str,
        l: Option<&LocalEntry>,
        r: Option<&RemoteEntry>,
        local_rel: &str,
        export_mime: Option<&str>,
    ) -> Result<()> {
        let prev = self.store.lock().baseline_entry(&self.pair.id, key)?;
        let is_dir = l
            .map(|l| l.is_dir)
            .or(r.map(|r| r.is_dir))
            .or(prev.as_ref().map(|p| p.is_dir))
            .unwrap_or(false);
        let hash = match l {
            Some(l) if !is_dir => Some(
                self.hashes
                    .hash(&self.local, l)
                    .await
                    .map_err(EngineError::file_read)?
                    .md5,
            ),
            _ => prev.as_ref().and_then(|p| p.local_hash.clone()),
        };
        let p = prev.as_ref();
        self.put_baseline(&BaselineEntry {
            key: key.to_owned(),
            local_rel: local_rel.to_owned(),
            is_dir,
            local_size: l.map(|l| l.size).or(p.map(|p| p.local_size)).unwrap_or(0),
            local_mtime: l.map(|l| l.mtime).or(p.map(|p| p.local_mtime)).unwrap_or(0),
            local_hash: hash,
            remote_id: r.map(|r| r.id.clone()).or(p.and_then(|p| p.remote_id.clone())),
            remote_md5: r
                .and_then(|r| r.md5.clone())
                .or(p.and_then(|p| p.remote_md5.clone())),
            remote_size: r.and_then(|r| r.size).or(p.and_then(|p| p.remote_size)),
            remote_mtime: r.map(|r| r.mtime).or(p.and_then(|p| p.remote_mtime)),
            remote_version: r.and_then(|r| r.version).or(p.and_then(|p| p.remote_version)),
            export_mime: export_mime
                .map(str::to_owned)
                .or(p.and_then(|p| p.export_mime.clone())),
        })?;
        if let Some(r) = r {
            self.lock(&self.remote_ids).insert(key.to_owned(), r.id.clone());
        }
        Ok(())
    }
}

fn folder_entry(key: &str, local_rel: &str) -> BaselineEntry {
    BaselineEntry {
        key: key.to_owned(),
        local_rel: local_rel.to_owned(),
        is_dir: true,
        local_size: 0,
        local_mtime: 0,
        local_hash: None,
        remote_id: None,
        remote_md5: None,
        remote_size: None,
        remote_mtime: None,
        remote_version: None,
        export_mime: None,
    }
}
