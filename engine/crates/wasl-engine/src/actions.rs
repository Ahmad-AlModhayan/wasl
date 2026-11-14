//! What the user does after a run (port of 1.1.4 `actions/`): review and undo a stopped deletion,
//! settle a conflict, restore an old version, work the recycle bin. These touch files and the
//! baseline, so they live in the engine, not in the apps (FR-E3).

use crate::engine::Engine;
use crate::error::{EngineError, Result};
use crate::executor::{BIN_RETENTION_MS, Executor, MAX_LOCAL_VERSIONS};
use crate::support::{CancelToken, Tracker, new_id};
use std::sync::{Arc, Mutex};
use wasl_core::names::conflict_name;
use wasl_core::paths::{name_of, parent_of};
use wasl_core::planner::mapped_local_rel;
use wasl_core::{Action, PlanInput, PlanSummary, RunStatus, SyncPlan};
use wasl_fs::LocalFs;
use wasl_store::{BinEntry, BinSide, NewBinEntry, Pair, RunStats};

pub use wasl_proto::engine::{
    ConflictChoice, ConflictDetails, ConflictSide, ConflictUndo, DeletionReview, FileVersions,
    PendingDeletion, RestoreFailure, RestoreResult, VersionEntry, VersionRef, VersionSource,
};

async fn blocking<T: Send + 'static>(
    fs: &Arc<dyn LocalFs>,
    f: impl FnOnce(&dyn LocalFs) -> std::io::Result<T> + Send + 'static,
) -> Result<T> {
    let fs = fs.clone();
    Ok(tokio::task::spawn_blocking(move || f(fs.as_ref()))
        .await
        .map_err(|e| EngineError::Other(e.to_string()))??)
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

impl Engine {
    /// The local relative path of a key: the baseline's, else the mapped Drive name.
    fn local_rel_of(&self, pair: &Pair, key: &str) -> Result<String> {
        let base = self.store.lock().load_baseline(&pair.id)?;
        Ok(match base.get(key) {
            Some(b) => b.local_rel.clone(),
            None => mapped_local_rel(
                key,
                &PlanInput {
                    baseline: base,
                    ..PlanInput::default()
                },
            ),
        })
    }

    /// Moves a local file to the bin and indexes it (reason as in 1.1.4).
    async fn bin_local_file(
        &self,
        pair: &Pair,
        fs: &Arc<dyn LocalFs>,
        key: &str,
        rel: &str,
        reason: &str,
    ) -> Result<Option<String>> {
        let r = rel.to_owned();
        let Some(st) = blocking(fs, move |fs| fs.stat(&r)).await? else {
            return Ok(None);
        };
        let r = rel.to_owned();
        let location = blocking(fs, move |fs| fs.move_to_bin(&r)).await?;
        let now = self.clock.now_ms();
        let id = new_id(self.clock.as_ref());
        self.store.lock().add_to_bin(&NewBinEntry {
            id: id.clone(),
            pair_id: pair.id.clone(),
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
            pair_name: Some(pair.name.clone()),
        })?;
        Ok(Some(id))
    }

    /// Runs a plan the engine made itself (a restore), recorded as a run with `trigger`.
    async fn run_internal_plan(
        &self,
        pair: &Pair,
        prepared: crate::engine::Prepared,
        plan: SyncPlan,
        trigger: &str,
    ) -> Result<RunStats> {
        let run_id = new_id(self.clock.as_ref());
        self.store
            .lock()
            .start_run(&run_id, &pair.id, trigger, self.clock.now_ms())?;
        let exec = Arc::new(Executor {
            pair: pair.clone(),
            run_id: run_id.clone(),
            store: self.store.clone(),
            local: prepared.local,
            remote: prepared.remote,
            remote_ids: Mutex::new(prepared.remote_ids),
            hashes: prepared.hashes,
            concurrency: self.config.concurrency,
            device_name: self.config.device_name.clone(),
            clock: self.clock.clone(),
            tracker: Arc::new(Tracker::new(&pair.id, &run_id, None)),
            cancel: Arc::new(CancelToken::default()),
            stats: Mutex::new(RunStats::default()),
            events: Mutex::new(Vec::new()),
            fatal: Mutex::new(None),
            first_error: Mutex::new(None),
            incoming: Mutex::new(Default::default()),
        });
        exec.execute(&plan).await;
        let stats = exec.stats.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let status = if stats.errors > 0 {
            RunStatus::Partial
        } else {
            RunStatus::Success
        };
        self.store
            .lock()
            .finish_run(&run_id, status, &stats, None, self.clock.now_ms())?;
        if let Some(f) = exec.fatal.lock().unwrap_or_else(|p| p.into_inner()).take() {
            return Err(f);
        }
        Ok(stats)
    }

    // ------------------------------------------------------------------ stopped deletions

    /// Everything the pair's next plan would delete, for "Review deletions" (FR-F3).
    pub async fn deletion_review(&self, pair_id: &str) -> Result<DeletionReview> {
        let pair = self.pair(pair_id)?;
        let tracker = Arc::new(Tracker::new(pair_id, "", None));
        let p = self
            .prepare(&pair, false, &tracker, &Arc::new(CancelToken::default()), false)
            .await?;
        let deletions: Vec<&Action> = p.plan.actions.iter().filter(|a| a.is_delete()).collect();
        let is_dir = |a: &Action| {
            matches!(
                a,
                Action::DeleteLocal { is_dir: true, .. } | Action::DeleteRemote { is_dir: true, .. }
            )
        };
        let on_drive = |a: &Action| matches!(a, Action::DeleteRemote { .. });
        let dirs: Vec<&str> = deletions.iter().filter(|a| is_dir(a)).map(|a| a.key()).collect();
        let under_deleted_dir = |k: &str| dirs.iter().any(|d| k != *d && k.starts_with(&format!("{d}/")));
        let size_of = |a: &Action| -> u64 {
            let k = a.key();
            let base = p.input.baseline.get(k);
            if on_drive(a) {
                p.input
                    .remote
                    .get(k)
                    .and_then(|r| r.size)
                    .or(base.and_then(|b| b.remote_size))
                    .unwrap_or(0)
            } else {
                p.input
                    .local
                    .get(k)
                    .map(|l| l.size)
                    .or(base.map(|b| b.local_size))
                    .unwrap_or(0)
            }
        };
        let files: Vec<&&Action> = deletions.iter().filter(|a| !is_dir(a)).collect();
        let mut items = Vec::new();
        for d in deletions
            .iter()
            .filter(|a| is_dir(a) && !under_deleted_dir(a.key()))
        {
            let inside: Vec<&&&Action> = files
                .iter()
                .filter(|f| f.key().starts_with(&format!("{}/", d.key())))
                .collect();
            items.push(PendingDeletion {
                key: d.key().to_owned(),
                name: name_of(d.key()).to_owned(),
                folder: parent_of(d.key()).to_owned(),
                is_dir: true,
                files: inside.len() as u32,
                bytes: inside.iter().map(|f| size_of(f)).sum(),
                on_drive: on_drive(d),
            });
        }
        for f in files.iter().filter(|f| !under_deleted_dir(f.key())) {
            items.push(PendingDeletion {
                key: f.key().to_owned(),
                name: name_of(f.key()).to_owned(),
                folder: parent_of(f.key()).to_owned(),
                is_dir: false,
                files: 1,
                bytes: size_of(f),
                on_drive: on_drive(f),
            });
        }
        items.sort_by_key(|i| (!i.is_dir, i.key.to_lowercase()));
        Ok(DeletionReview {
            summary: p.plan.summary.clone(),
            files_on_drive: files.iter().filter(|f| on_drive(f)).count() as u32,
            files_on_device: files.iter().filter(|f| !on_drive(f)).count() as u32,
            bytes: files.iter().map(|f| size_of(f)).sum(),
            items,
        })
    }

    /// "Restore": copies every file the stopped plan would delete back to the side that lost it,
    /// then clears the stop. Returns how many files came back.
    pub async fn restore_deletions(&self, pair_id: &str) -> Result<u32> {
        let pair = self.pair(pair_id)?;
        let tracker = Arc::new(Tracker::new(pair_id, "", None));
        let p = self
            .prepare(&pair, false, &tracker, &Arc::new(CancelToken::default()), false)
            .await?;
        let mut actions = Vec::new();
        for a in &p.plan.actions {
            match a {
                // Gone from the device, still on Drive: bring it back down.
                Action::DeleteRemote {
                    key, is_dir: true, ..
                } => actions.push(Action::MkdirLocal {
                    key: key.clone(),
                    local_rel: p
                        .input
                        .local_rel_by_key
                        .get(key)
                        .cloned()
                        .unwrap_or_else(|| key.clone()),
                }),
                Action::DeleteRemote { key, .. } => {
                    if let Some(r) = p.input.remote.get(key) {
                        actions.push(Action::Download {
                            key: key.clone(),
                            remote: r.clone(),
                            local_rel: p
                                .input
                                .local_rel_by_key
                                .get(key)
                                .cloned()
                                .unwrap_or_else(|| key.clone()),
                            overwrites: false,
                            reason: "restored after a stopped deletion".into(),
                        });
                    }
                }
                // Gone from Drive, still on the device: put it back up.
                Action::DeleteLocal {
                    key, is_dir: true, ..
                } => actions.push(Action::MkdirRemote {
                    key: key.clone(),
                    parent_key: parent_of(key).to_owned(),
                    name: name_of(key).to_owned(),
                }),
                Action::DeleteLocal { key, .. } => {
                    if let Some(l) = p.input.local.get(key) {
                        actions.push(Action::Upload {
                            key: key.clone(),
                            local: l.clone(),
                            existing_remote_id: None,
                            parent_key: parent_of(key).to_owned(),
                            name: name_of(key).to_owned(),
                            reason: "restored after a stopped deletion".into(),
                        });
                    }
                }
                _ => {}
            }
        }
        // Folders shallow first, as the executor expects.
        actions.sort_by_key(|a| (!a.is_mkdir(), wasl_core::paths::depth(a.key())));
        let plan = SyncPlan {
            actions,
            summary: PlanSummary::default(),
            blocked: false,
        };
        let stats = self.run_internal_plan(&pair, p, plan, "restore").await?;
        {
            let s = self.store.lock();
            if let Some(mut st) = s.get_pair(pair_id)?.map(|p| p.status) {
                st.pending_approval = false;
                s.set_pair_status(pair_id, &st)?;
            }
            s.delete_blocked_plan(pair_id)?;
        }
        Ok(stats.uploaded + stats.downloaded)
    }

    // ------------------------------------------------------------------ conflicts (FR-F4)

    pub async fn conflict_details(&self, conflict_id: &str) -> Result<Option<ConflictDetails>> {
        let Some(c) = self.store.lock().conflict(conflict_id)? else {
            return Ok(None);
        };
        let name = name_of(&c.key).to_owned();
        let folder = parent_of(&c.key).to_owned();
        let Ok(pair) = self.pair(&c.pair_id) else {
            return Ok(Some(ConflictDetails {
                conflict: c,
                name,
                folder,
                device: None,
                drive: None,
            }));
        };
        let fs = self.env.local_fs(&pair)?;
        let original = self.local_rel_of(&pair, &c.key)?;
        let copy = c.local_copy_rel.clone();
        let (o, cp) = (original.clone(), copy.clone());
        let (drive, device) = blocking(&fs, move |fs| {
            let side = |rel: &str| -> std::io::Result<Option<ConflictSide>> {
                Ok(fs.stat(rel)?.filter(|e| !e.is_dir).map(|e| ConflictSide {
                    mtime_ms: e.mtime_ms,
                    size: e.size,
                }))
            };
            Ok((
                side(&o)?,
                match &cp {
                    Some(c) => side(c)?,
                    None => None,
                },
            ))
        })
        .await?;
        Ok(Some(ConflictDetails {
            conflict: c,
            name,
            folder,
            device,
            drive,
        }))
    }

    /// Keep both closes the conflict. Keep device / Keep Drive moves the other copy to the bin; the
    /// next sync carries the choice to Drive.
    pub async fn resolve_conflict(&self, conflict_id: &str, choice: ConflictChoice) -> Result<ConflictUndo> {
        let c = self
            .store
            .lock()
            .conflict(conflict_id)?
            .ok_or_else(|| EngineError::NotFound("This conflict was already resolved.".into()))?;
        let now = self.clock.now_ms();
        let pair = self.pair(&c.pair_id).ok();
        let undo = |bin_entry_id, renamed_from, renamed_to| ConflictUndo {
            conflict_id: conflict_id.to_owned(),
            choice,
            bin_entry_id,
            renamed_from,
            renamed_to,
        };
        let (Some(pair), Some(copy)) = (pair, c.local_copy_rel.clone()) else {
            self.store.lock().resolve_conflict(conflict_id, now)?;
            return Ok(undo(None, None, None));
        };
        if choice == ConflictChoice::KeepBoth {
            self.store.lock().resolve_conflict(conflict_id, now)?;
            return Ok(undo(None, None, None));
        }
        let fs = self.env.local_fs(&pair)?;
        let original = self.local_rel_of(&pair, &c.key)?;
        let cp = copy.clone();
        if blocking(&fs, move |fs| fs.stat(&cp)).await?.is_none() {
            return Err(EngineError::Io(format!(
                "The device copy is no longer at {copy}."
            )));
        }
        let result = match choice {
            ConflictChoice::KeepDrive => {
                // The copy has its own key (it was uploaded under its conflict name); file it there
                // so restoring it puts it back under that name.
                let copy_key = self
                    .store
                    .lock()
                    .baseline_by_local_rel(&pair.id, &copy)?
                    .map_or_else(|| copy.clone(), |b| b.key);
                let id = self
                    .bin_local_file(&pair, &fs, &copy_key, &copy, "CONFLICT")
                    .await?;
                undo(id, None, None)
            }
            ConflictChoice::KeepDevice => {
                let id = self
                    .bin_local_file(&pair, &fs, &c.key, &original, "CONFLICT")
                    .await?;
                let (from, to) = (copy.clone(), original.clone());
                blocking(&fs, move |fs| fs.rename(&from, &to)).await?;
                undo(id, Some(copy), Some(original))
            }
            ConflictChoice::KeepBoth => unreachable!("handled above"),
        };
        self.store.lock().resolve_conflict(conflict_id, now)?;
        Ok(result)
    }

    pub async fn undo_conflict(&self, u: &ConflictUndo) -> Result<()> {
        let c = self.store.lock().conflict(&u.conflict_id)?;
        if let Some(c) = &c
            && let Ok(pair) = self.pair(&c.pair_id)
        {
            let fs = self.env.local_fs(&pair)?;
            if let (Some(from), Some(to)) = (u.renamed_from.clone(), u.renamed_to.clone()) {
                blocking(&fs, move |fs| fs.rename(&to, &from)).await?;
            }
            if let Some(id) = &u.bin_entry_id {
                self.restore_one(id).await?;
            }
        }
        self.store.lock().unresolve_conflict(&u.conflict_id)?;
        Ok(())
    }

    // ------------------------------------------------------------------ versions (FR-F6)

    pub async fn file_versions(&self, pair_id: &str, key: &str) -> Result<FileVersions> {
        let pair = self.pair(pair_id)?;
        let base = self.store.lock().baseline_entry(pair_id, key)?;
        let remote = self.env.remote(&pair)?;
        let revisions = match base.as_ref().and_then(|b| b.remote_id.clone()) {
            Some(id) => match remote.list_revisions(&id).await {
                Err(wasl_drive::DriveError::NotFound(_)) => Vec::new(),
                other => other?,
            },
            None => Vec::new(),
        };
        let uploaded: Vec<String> = self
            .store
            .lock()
            .uploads_of(pair_id, key, 200)?
            .into_iter()
            .filter_map(|e| {
                e.event
                    .detail
                    .and_then(|d| d.strip_prefix("md5:").map(str::to_lowercase))
            })
            .collect();
        let mut entries: Vec<VersionEntry> = revisions
            .iter()
            .map(|r| {
                let from_device = r
                    .md5
                    .as_ref()
                    .is_some_and(|m| uploaded.contains(&m.to_lowercase()));
                VersionEntry {
                    version: VersionRef::Drive {
                        revision_id: r.id.clone(),
                    },
                    at: r.modified_ms,
                    source: if from_device {
                        VersionSource::Device
                    } else {
                        VersionSource::Drive
                    },
                    size: r.size.unwrap_or(0),
                    note: if from_device { None } else { r.modified_by.clone() },
                    current: false,
                    keep_forever: r.keep_forever,
                }
            })
            .collect();
        let local: Vec<VersionEntry> = self
            .store
            .lock()
            .bin_versions(pair_id, key)?
            .into_iter()
            .filter(|b| b.entry.side == BinSide::Local && b.entry.reason != "DELETED")
            .take(MAX_LOCAL_VERSIONS as usize)
            .map(|b| VersionEntry {
                version: VersionRef::Local {
                    bin_entry_id: b.entry.id.clone(),
                },
                at: b.entry.deleted_at,
                source: if b.entry.reason == "CONFLICT" {
                    VersionSource::Conflict
                } else {
                    VersionSource::Device
                },
                size: b.entry.size,
                note: Some("replaced on this device".into()),
                current: false,
                keep_forever: false,
            })
            .collect();
        entries.extend(local);
        entries.sort_by_key(|e| std::cmp::Reverse(e.at));
        if let Some(first) = entries
            .iter_mut()
            .find(|e| matches!(e.version, VersionRef::Drive { .. }))
        {
            first.current = true;
        }
        Ok(FileVersions {
            name: name_of(key).to_owned(),
            path: join(pair.remote_path.trim_end_matches('/'), key),
            pair_name: pair.name.clone(),
            size: base
                .and_then(|b| b.remote_size)
                .or(revisions.last().and_then(|r| r.size)),
            entries,
        })
    }

    /// Makes a version current on this device now, and on Drive at the next sync (the baseline is
    /// left alone, so the next run sees a device change). The replaced file goes to the bin.
    pub async fn restore_version(&self, pair_id: &str, key: &str, version: &VersionRef) -> Result<()> {
        let pair = self.pair(pair_id)?;
        let fs = self.env.local_fs(&pair)?;
        let rel = self.local_rel_of(&pair, key)?;
        match version {
            VersionRef::Drive { revision_id } => {
                let id = self
                    .store
                    .lock()
                    .baseline_entry(pair_id, key)?
                    .and_then(|b| b.remote_id)
                    .ok_or_else(|| EngineError::NotFound("This file is not on Drive.".into()))?;
                let r = rel.clone();
                let staging = blocking(&fs, move |fs| fs.staging_path(&r)).await?;
                self.env
                    .remote(&pair)?
                    .download_revision(&id, revision_id, &staging)
                    .await?;
                self.bin_local_file(&pair, &fs, key, &rel, "RESTORED_VERSION")
                    .await?;
                let r = rel.clone();
                blocking(&fs, move |fs| fs.install(&staging, &r, None)).await?;
            }
            VersionRef::Local { bin_entry_id } => {
                let b = self.store.lock().bin_entry(bin_entry_id)?.ok_or_else(|| {
                    EngineError::NotFound("That version is no longer in the recycle bin.".into())
                })?;
                self.bin_local_file(&pair, &fs, key, &rel, "RESTORED_VERSION")
                    .await?;
                let (loc, r) = (b.entry.location.clone(), rel.clone());
                blocking(&fs, move |fs| fs.restore_from_bin(&loc, &r)).await?;
                self.store.lock().remove_from_bin(bin_entry_id)?;
            }
        }
        Ok(())
    }

    /// Saves a version beside the file as `name (version STAMP).ext`; returns its local path. A
    /// version from the local bin moves out of the bin into that copy.
    pub async fn save_version_copy(
        &self,
        pair_id: &str,
        key: &str,
        version: &VersionRef,
        at: i64,
    ) -> Result<String> {
        let pair = self.pair(pair_id)?;
        let fs = self.env.local_fs(&pair)?;
        let rel = self.local_rel_of(&pair, key)?;
        let name = name_of(&rel);
        let (base, ext) = match name.rfind('.') {
            Some(dot) if dot > 0 => name.split_at(dot),
            _ => (name, ""),
        };
        let stamp = crate::support::stamp_utc(at);
        let mut copy = join(parent_of(&rel), &format!("{base} (version {stamp}){ext}"));
        let mut n = 2;
        loop {
            let c = copy.clone();
            if blocking(&fs, move |fs| fs.stat(&c)).await?.is_none() {
                break;
            }
            copy = join(parent_of(&rel), &format!("{base} (version {stamp}) ({n}){ext}"));
            n += 1;
        }
        match version {
            VersionRef::Drive { revision_id } => {
                let id = self
                    .store
                    .lock()
                    .baseline_entry(pair_id, key)?
                    .and_then(|b| b.remote_id)
                    .ok_or_else(|| EngineError::NotFound("This file is not on Drive.".into()))?;
                let c = copy.clone();
                let staging = blocking(&fs, move |fs| fs.staging_path(&c)).await?;
                self.env
                    .remote(&pair)?
                    .download_revision(&id, revision_id, &staging)
                    .await?;
                let c = copy.clone();
                blocking(&fs, move |fs| fs.install(&staging, &c, Some(at))).await?;
            }
            VersionRef::Local { bin_entry_id } => {
                let b = self.store.lock().bin_entry(bin_entry_id)?.ok_or_else(|| {
                    EngineError::NotFound("That version is no longer in the recycle bin.".into())
                })?;
                let (loc, c) = (b.entry.location.clone(), copy.clone());
                blocking(&fs, move |fs| fs.restore_from_bin(&loc, &c)).await?;
                self.store.lock().remove_from_bin(bin_entry_id)?;
            }
        }
        Ok(copy)
    }

    /// Keeps a Drive revision past Drive's retention, or lets it expire again.
    pub async fn keep_revision(&self, pair_id: &str, key: &str, revision_id: &str, keep: bool) -> Result<()> {
        let pair = self.pair(pair_id)?;
        let id = self
            .store
            .lock()
            .baseline_entry(pair_id, key)?
            .and_then(|b| b.remote_id)
            .ok_or_else(|| EngineError::NotFound("This file is not on Drive.".into()))?;
        Ok(self
            .env
            .remote(&pair)?
            .keep_revision(&id, revision_id, keep)
            .await?)
    }

    // ------------------------------------------------------------------ recycle bin (FR-F5)

    /// Restores one entry. If a newer file sits where it goes, that file goes into the bin first,
    /// so nothing is ever lost. Returns the entry that was restored.
    pub(crate) async fn restore_one(&self, entry_id: &str) -> Result<BinEntry> {
        let b = self
            .store
            .lock()
            .bin_entry(entry_id)?
            .ok_or_else(|| EngineError::NotFound(format!("bin entry {entry_id}")))?;
        let pair = self.pair(&b.entry.pair_id)?;
        match b.entry.side {
            BinSide::Local => {
                let fs = self.env.local_fs(&pair)?;
                let rel = self.local_rel_of(&pair, &b.entry.key)?;
                self.bin_local_file(&pair, &fs, &b.entry.key, &rel, "OVERWRITTEN")
                    .await?;
                let (loc, r) = (b.entry.location.clone(), rel);
                blocking(&fs, move |fs| fs.restore_from_bin(&loc, &r)).await?;
            }
            BinSide::Drive => {
                let id = b
                    .entry
                    .remote_file_id
                    .clone()
                    .ok_or_else(|| EngineError::NotFound("No Drive file recorded".into()))?;
                self.env.remote(&pair)?.untrash(&id).await?;
            }
        }
        self.store.lock().remove_from_bin(entry_id)?;
        Ok(b)
    }

    /// Restores several entries; each one succeeds or fails on its own.
    pub async fn restore_bin_entries(&self, ids: &[String]) -> Result<RestoreResult> {
        let ids = self.with_folder_contents(ids)?;
        let mut restored = Vec::new();
        let mut failed = Vec::new();
        for id in &ids {
            match self.restore_one(id).await {
                Ok(b) => restored.push(b),
                Err(e) => failed.push(RestoreFailure {
                    entry_id: id.clone(),
                    message: e.to_string(),
                }),
            }
        }
        Ok(RestoreResult { restored, failed })
    }

    /// The entries to restore, with what was deleted along with each folder among them: the
    /// deletions under it, on the same side, binned within ten minutes of it (one sync's deletion;
    /// older versions of a file stay in the bin).
    /// Folders come before what is inside them.
    fn with_folder_contents(&self, ids: &[String]) -> Result<Vec<String>> {
        const SAME_DELETION_MS: i64 = 10 * 60 * 1000;
        let store = self.store.lock();
        let mut out: Vec<(usize, String)> = Vec::new();
        for id in ids {
            let Some(b) = store.bin_entry(id)? else {
                out.push((0, id.clone()));
                continue;
            };
            let depth = b.entry.key.matches('/').count();
            out.push((depth, id.clone()));
            let prefix = format!("{}/", b.entry.key);
            for other in store.bin_entries(Some(&b.entry.pair_id))? {
                let e = &other.entry;
                if e.side == b.entry.side
                    && e.reason == "DELETED"
                    && e.key.starts_with(&prefix)
                    && (e.deleted_at - b.entry.deleted_at).abs() <= SAME_DELETION_MS
                {
                    out.push((e.key.matches('/').count(), e.id.clone()));
                }
            }
        }
        out.sort_by_key(|(depth, _)| *depth);
        let mut seen = std::collections::HashSet::new();
        Ok(out
            .into_iter()
            .filter_map(|(_, id)| seen.insert(id.clone()).then_some(id))
            .collect())
    }

    /// Undo for the restore snackbar: puts the restored files back into the bin.
    pub async fn undo_bin_restore(&self, restored: &[BinEntry]) -> Result<()> {
        for b in restored {
            let Ok(pair) = self.pair(&b.entry.pair_id) else {
                continue;
            };
            match b.entry.side {
                BinSide::Local => {
                    let fs = self.env.local_fs(&pair)?;
                    let rel = self.local_rel_of(&pair, &b.entry.key)?;
                    self.bin_local_file(&pair, &fs, &b.entry.key, &rel, &b.entry.reason)
                        .await?;
                }
                BinSide::Drive => {
                    if let Some(id) = &b.entry.remote_file_id {
                        self.env.remote(&pair)?.trash(id).await?;
                        self.store.lock().add_to_bin(&NewBinEntry {
                            id: new_id(self.clock.as_ref()),
                            ..b.entry.clone()
                        })?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Irreversible: removes the files for good (Drive files leave Drive's bin too).
    pub async fn delete_bin_forever(&self, ids: &[String]) -> Result<u32> {
        let mut n = 0;
        for id in ids {
            let Some(b) = self.store.lock().bin_entry(id)? else {
                continue;
            };
            if let Ok(pair) = self.pair(&b.entry.pair_id) {
                match b.entry.side {
                    BinSide::Local => {
                        let fs = self.env.local_fs(&pair)?;
                        let loc = b.entry.location.clone();
                        blocking(&fs, move |fs| fs.purge_from_bin(&loc)).await?;
                    }
                    BinSide::Drive => {
                        if let Some(rid) = &b.entry.remote_file_id {
                            self.env.remote(&pair)?.delete_permanently(rid).await?;
                        }
                    }
                }
            }
            self.store.lock().remove_from_bin(id)?;
            n += 1;
        }
        Ok(n)
    }

    pub async fn empty_bin(&self) -> Result<u32> {
        let ids: Vec<String> = self
            .store
            .lock()
            .bin_entries(None)?
            .into_iter()
            .map(|b| b.entry.id)
            .collect();
        self.delete_bin_forever(&ids).await
    }

    /// The device copy's conflict name for a file, as the executor would make it now.
    pub fn conflict_copy_name(&self, name: &str) -> String {
        conflict_name(name, &self.config.device_name, &self.clock.stamp())
    }
}
