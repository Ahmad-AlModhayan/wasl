//! Agent changes (docs/AGENT_CHANGES.md): an agent's session in a folder of a pair becomes a
//! changeset whose files stay out of sync until the user accepts it or reverts it.

use crate::engine::{Engine, rules_builder};
use crate::error::{EngineError, Result};
use crate::executor::BIN_RETENTION_MS;
use crate::support::new_id;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use wasl_core::{BaselineEntry, LocalEntry, PlanInput};
use wasl_fs::LocalFs;
use wasl_store::{
    AgentEntry, BinSide, Changeset, ChangesetStatus, EarlierVersion, EntryKind, ManifestRow, NewBinEntry,
    Pair, SavedCopy, Store,
};

/// A file larger than this is not copied as an earlier version; its revert relies on the synced
/// version.
pub const MAX_SAVED_FILE: u64 = 100 * 1024 * 1024;
/// At most this much is copied when a session begins.
pub const MAX_BEGIN_COPY: u64 = 512 * 1024 * 1024;

/// Decided changesets are kept this long, like sync history (FR-B6).
pub const AGENT_HISTORY_MS: i64 = 90 * 24 * 3600 * 1000;

/// Bin reason of an agent's version put aside by a revert.
pub const AGENT_REVERT: &str = "AGENT_REVERT";

/// A writer that hashes what goes through it, so a saved copy is hashed as it is written.
struct Hashing {
    inner: std::fs::File,
    hasher: blake3::Hasher,
}

impl io::Write for Hashing {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// A changeset with what it changed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangesetDetails {
    pub changeset: Changeset,
    pub pair_name: String,
    pub entries: Vec<AgentEntry>,
    /// Which client accepted it ("cli", "desktop", "unknown"), when it was recorded.
    #[serde(default)]
    pub decided_by: Option<String>,
}

/// How a revert went.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RevertReport {
    pub reverted: Vec<String>,
    /// Changed after the session ended: left as they are.
    pub skipped: Vec<String>,
    /// Path and why.
    pub failed: Vec<(String, String)>,
}

/// `agent:<name>` from a name with or without the prefix: lowercase letters, digits, `.`, `_`, `-`.
pub fn actor_id(name: &str) -> Result<String> {
    let name = name.trim();
    let bare = name.strip_prefix("agent:").unwrap_or(name).to_ascii_lowercase();
    if bare.is_empty()
        || bare.len() > 64
        || !bare
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
    {
        return Err(EngineError::Other(format!(
            "agent name {name:?}: use letters, digits, '.', '_' or '-' (for example claude-code)"
        )));
    }
    Ok(format!("agent:{bare}"))
}

pub(crate) fn in_scope(rel: &str, scope: &str) -> bool {
    scope.is_empty() || rel == scope || rel.strip_prefix(scope).is_some_and(|r| r.starts_with('/'))
}

fn overlaps(a: &str, b: &str) -> bool {
    in_scope(a, b) || in_scope(b, a)
}

/// Size and modification time of a file, or "a folder", for comparing two moments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    Dir,
    File(u64, i64),
}

impl State {
    pub(crate) fn of_local(e: &LocalEntry) -> State {
        if e.is_dir {
            State::Dir
        } else {
            State::File(e.size, e.mtime)
        }
    }
    fn of_manifest(m: &ManifestRow) -> State {
        if m.is_dir {
            State::Dir
        } else {
            State::File(m.size, m.mtime)
        }
    }
    fn of_baseline(b: &BaselineEntry) -> State {
        if b.is_dir {
            State::Dir
        } else {
            State::File(b.local_size, b.local_mtime)
        }
    }
}

/// What a session changed, from the begin list (each path's state and whether it was synced then),
/// the scope now and the synced states now. Wasl's own work is left out: a file whose state now is
/// its synced state was written by Wasl, and a synced file that is gone and no longer synced was
/// removed by Wasl because another device deleted it.
fn changed_rels(
    manifest: &HashMap<&str, (State, bool)>,
    now: &HashMap<&str, State>,
    synced: &HashMap<String, State>,
) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for rel in manifest.keys().chain(now.keys()) {
        let (m, c) = (manifest.get(rel), now.get(rel));
        if m.map(|m| &m.0) == c {
            continue;
        }
        match c {
            Some(c @ State::File(..)) if synced.get(*rel) == Some(c) => continue,
            None if m.is_some_and(|m| m.1) && !synced.contains_key(*rel) => continue,
            _ => {}
        }
        out.insert((*rel).to_owned());
    }
    out
}

fn manifest_states(manifest: &[ManifestRow]) -> HashMap<&str, (State, bool)> {
    manifest
        .iter()
        .map(|r| (r.rel.as_str(), (State::of_manifest(r), r.synced)))
        .collect()
}

/// Each local path as it was when last synced, on any route: the Drive baseline and the device
/// index. A file in this state has a copy elsewhere; one Wasl just downloaded is in it too.
pub(crate) fn synced_states(s: &Store, pair_id: &str) -> Result<HashMap<String, State>> {
    let mut out: HashMap<String, State> = HashMap::new();
    if let Some(index) = s.load_index(pair_id)? {
        for (rel, e) in index.entries {
            if !e.version.deleted {
                let st = if e.version.is_dir {
                    State::Dir
                } else {
                    State::File(e.local_size, e.local_mtime)
                };
                out.insert(rel, st);
            }
        }
    }
    for b in s.load_baseline(pair_id)?.into_values() {
        out.insert(b.local_rel.clone(), State::of_baseline(&b));
    }
    Ok(out)
}

/// Local paths sync must leave alone now: those of held changesets, and, for each open session,
/// the paths in its scope that changed since it began (unless Wasl itself synced them). `local` is
/// the pair's folder as just scanned.
pub(crate) fn held_rels(s: &Store, pair_id: &str, local: &[&LocalEntry]) -> Result<BTreeSet<String>> {
    let mut held = BTreeSet::new();
    let sets = s.changesets(Some(pair_id), false)?;
    if sets.is_empty() {
        return Ok(held);
    }
    for cs in sets.iter().filter(|c| c.status == ChangesetStatus::Held) {
        held.extend(s.agent_entries(&cs.id)?.into_iter().map(|e| e.rel));
    }
    let open: Vec<&Changeset> = sets
        .iter()
        .filter(|c| c.status == ChangesetStatus::Open)
        .collect();
    if open.is_empty() {
        return Ok(held);
    }
    let synced = synced_states(s, pair_id)?;
    for cs in open {
        let manifest = s.manifest(&cs.id)?;
        let m = manifest_states(&manifest);
        let now: HashMap<&str, State> = local
            .iter()
            .filter(|e| in_scope(&e.rel, &cs.scope) && e.rel != cs.scope)
            .map(|e| (e.rel.as_str(), State::of_local(e)))
            .collect();
        held.extend(changed_rels(&m, &now, &synced));
    }
    Ok(held)
}

impl Engine {
    fn agent_dir(&self, changeset_id: &str) -> Result<PathBuf> {
        let base = self.config.agent_dir.clone().ok_or_else(|| {
            EngineError::Other("agent changes are not set up on this device (no agent folder)".into())
        })?;
        Ok(base.join(changeset_id))
    }

    /// The scope's files now (ignored items left out), by local relative path. Only the scope is
    /// walked: a prompt hook waits for this, so it must not cost a scan of the whole pair.
    async fn scan_scope(&self, pair: &Pair, fs: &Arc<dyn LocalFs>, scope: &str) -> Result<Vec<LocalEntry>> {
        let fs = fs.clone();
        let builder = rules_builder(pair);
        let scope = scope.to_owned();
        tokio::task::spawn_blocking(move || {
            let scan =
                wasl_fs::scan_under(fs.as_ref(), builder, &|rel: &str| rel.to_owned(), &scope, || {
                    false
                })?;
            if !scan.unreadable.is_empty() {
                return Err(EngineError::Io(format!(
                    "couldn't read {} folder(s) in the pair: {}",
                    scan.unreadable.len(),
                    scan.unreadable.join(", ")
                )));
            }
            Ok(scan
                .entries
                .into_iter()
                .filter(|e| in_scope(&e.rel, &scope) && e.rel != scope)
                .collect())
        })
        .await
        .map_err(|e| EngineError::Other(e.to_string()))?
    }

    /// Copies the file at `rel` as an earlier version of `changeset_id`. Returns the copy.
    async fn save_copy(
        &self,
        fs: &Arc<dyn LocalFs>,
        changeset_id: &str,
        rel: &str,
    ) -> Result<Option<SavedCopy>> {
        let dir = self.agent_dir(changeset_id)?;
        let location = new_id(self.clock.as_ref());
        let (fs, rel_owned, dest) = (fs.clone(), rel.to_owned(), dir.join(&location));
        let copied = tokio::task::spawn_blocking(move || -> io::Result<Option<(u64, i64, String)>> {
            let Some(st) = fs.stat(&rel_owned)? else {
                return Ok(None);
            };
            if st.is_dir || st.size > MAX_SAVED_FILE {
                return Ok(None);
            }
            std::fs::create_dir_all(dest.parent().unwrap_or(&dest))?;
            let mut src = fs.open_read(&rel_owned)?;
            let mut out = Hashing {
                inner: std::fs::File::create(&dest)?,
                hasher: blake3::Hasher::new(),
            };
            io::copy(&mut src, &mut out)?;
            out.inner.sync_all()?;
            Ok(Some((
                st.size,
                st.mtime_ms,
                out.hasher.finalize().to_hex().to_string(),
            )))
        })
        .await
        .map_err(|e| EngineError::Other(e.to_string()))??;
        Ok(copied.map(|(size, mtime, hash)| SavedCopy {
            rel: rel.to_owned(),
            location,
            size,
            mtime,
            blake3: Some(hash),
        }))
    }

    /// Checks that the earlier version of `rel` read back from the agent folder is what was saved.
    /// The folder is this user's, so any program of the user (the agent too) can write it; a copy
    /// changed there must never be installed into the pair or shown as the earlier version
    /// (security review 2026-10). Copies saved before schema v9 have no hash and are taken as
    /// they are.
    fn check_saved(&self, changeset_id: &str, rel: &str, content: &[u8]) -> Result<()> {
        let expected = self
            .store
            .lock()
            .saved_copy(changeset_id, rel)?
            .and_then(|c| c.blake3);
        match expected {
            Some(h) if blake3::hash(content).to_hex().as_str() != h => Err(EngineError::Integrity(
                "the earlier copy was changed after it was saved; it was not put back".into(),
            )),
            _ => Ok(()),
        }
    }

    /// Starts an agent session in `scope` (pair-relative; empty for the whole pair). With a
    /// `session` that already has an open changeset, returns that one.
    pub async fn agent_begin(
        &self,
        pair_id: &str,
        scope: &str,
        actor: &str,
        label: Option<&str>,
        session: Option<&str>,
    ) -> Result<Changeset> {
        let actor = actor_id(actor)?;
        let pair = self.pair(pair_id)?;
        let scope = scope.trim_matches('/');
        if !scope.is_empty() {
            wasl_fs::check_rel(scope)?;
        }
        if let Some(s) = session
            && let Some(open) = self.store.lock().open_changeset_for_session(s)?
        {
            return Ok(open);
        }
        let others = self.store.lock().changesets(Some(pair_id), false)?;
        if let Some(o) = others
            .iter()
            .find(|c| c.status == ChangesetStatus::Open && overlaps(&c.scope, scope))
        {
            return Err(EngineError::Other(format!(
                "{} is already working in {:?} of this pair (changeset {}). End that session first.",
                o.actor,
                if o.scope.is_empty() { "/" } else { o.scope.as_str() },
                o.id
            )));
        }

        let fs = self.env.local_fs(&pair)?;
        let entries = self.scan_scope(&pair, &fs, scope).await?;
        let cs = Changeset {
            id: new_id(self.clock.as_ref()),
            pair_id: Some(pair_id.to_owned()),
            actor,
            label: label.map(str::to_owned),
            scope: scope.to_owned(),
            session: session.map(str::to_owned),
            status: ChangesetStatus::Open,
            started_at: self.clock.now_ms(),
            ended_at: None,
            decided_at: None,
        };
        let synced = synced_states(&self.store.lock(), pair_id)?;
        let manifest: Vec<ManifestRow> = entries
            .iter()
            .map(|e| ManifestRow {
                rel: e.rel.clone(),
                is_dir: e.is_dir,
                size: e.size,
                mtime: e.mtime,
                synced: synced.get(&e.rel) == Some(&State::of_local(e)),
            })
            .collect();
        self.store.lock().create_changeset(&cs, &manifest)?;

        // Files not yet synced have no copy anywhere else: keep one.
        let mut budget = MAX_BEGIN_COPY;
        for e in entries.iter().filter(|e| !e.is_dir) {
            if synced.get(&e.rel) == Some(&State::of_local(e)) || e.size > budget {
                continue;
            }
            if let Some(copy) = self.save_copy(&fs, &cs.id, &e.rel).await? {
                budget = budget.saturating_sub(copy.size);
                self.store.lock().put_saved(&cs.id, &copy)?;
            }
        }
        // The session also covers every watched folder (docs/AGENT_UNDO.md, UND-2). A folder that
        // can't be snapshotted doesn't stop the session: why is kept with it, and the session
        // report lists it (#57).
        if self.config.undo_dir.is_some() {
            let _ = self.snapshot_watches(&cs.id).await;
        }
        Ok(cs)
    }

    /// The open changeset of `pair_id` whose scope holds `rel` (and of `session`, when given).
    fn open_changeset_for(
        &self,
        pair_id: &str,
        rel: &str,
        session: Option<&str>,
    ) -> Result<Option<Changeset>> {
        Ok(self
            .store
            .lock()
            .changesets(Some(pair_id), false)?
            .into_iter()
            .find(|c| {
                c.status == ChangesetStatus::Open
                    && in_scope(rel, &c.scope)
                    && session.is_none_or(|s| c.session.as_deref() == Some(s))
            }))
    }

    /// An agent is about to write `rel`: keeps its current content as the earlier version, once
    /// per session. Returns whether a copy was made.
    pub async fn agent_save(&self, pair_id: &str, rel: &str, session: Option<&str>) -> Result<bool> {
        wasl_fs::check_rel(rel)?;
        let Some(cs) = self.open_changeset_for(pair_id, rel, session)? else {
            return Ok(false);
        };
        if self.store.lock().has_saved(&cs.id, rel)? {
            return Ok(false);
        }
        let pair = self.pair(pair_id)?;
        let fs = self.env.local_fs(&pair)?;
        match self.save_copy(&fs, &cs.id, rel).await? {
            Some(copy) => Ok(self.store.lock().put_saved(&cs.id, &copy)?),
            None => Ok(false),
        }
    }

    /// Ends a session: records what it changed and holds it for the user. Returns `None` when
    /// it changed nothing (the changeset is dropped).
    pub async fn agent_end(&self, changeset_id: &str) -> Result<Option<Changeset>> {
        let cs = self
            .store
            .lock()
            .changeset(changeset_id)?
            .ok_or_else(|| EngineError::NotFound(format!("changeset {changeset_id}")))?;
        if cs.status != ChangesetStatus::Open {
            return Ok(Some(cs));
        }
        // What the watcher hadn't journaled yet (events still queued would be refused once the
        // session closes): found by a last rescan. A folder it can't rescan is listed as missed.
        // Its error is already in the report.
        if !self.store.lock().journal_roots(&cs.id)?.is_empty() {
            let _ = self.agent_rescan(&cs.id, "the session was asked to end").await;
        }
        let pair = self.pair(cs.pair()?)?;
        let fs = self.env.local_fs(&pair)?;
        let now_entries = self.scan_scope(&pair, &fs, &cs.scope).await?;
        let (manifest, saved, baseline, synced) = {
            let s = self.store.lock();
            (
                s.manifest(&cs.id)?,
                s.saved(&cs.id)?,
                s.load_baseline(cs.pair()?)?,
                synced_states(&s, cs.pair()?)?,
            )
        };
        let manifest_by_rel: HashMap<&str, &ManifestRow> =
            manifest.iter().map(|m| (m.rel.as_str(), m)).collect();
        let now_by_rel: HashMap<&str, &LocalEntry> =
            now_entries.iter().map(|e| (e.rel.as_str(), e)).collect();
        let baseline_by_rel: HashMap<&str, &BaselineEntry> =
            baseline.values().map(|b| (b.local_rel.as_str(), b)).collect();
        let saved_by_rel: HashMap<&str, &SavedCopy> = saved.iter().map(|c| (c.rel.as_str(), c)).collect();

        let changed = changed_rels(
            &manifest_states(&manifest),
            &now_by_rel.iter().map(|(r, e)| (*r, State::of_local(e))).collect(),
            &synced,
        );
        let mut entries = Vec::new();
        for rel in changed {
            let (m, c) = (manifest_by_rel.get(rel.as_str()), now_by_rel.get(rel.as_str()));
            let kind = match (m, c) {
                (None, _) => EntryKind::Added,
                (_, None) => EntryKind::Deleted,
                _ => EntryKind::Changed,
            };
            let is_dir = c.map(|e| e.is_dir).or(m.map(|m| m.is_dir)).unwrap_or(false);
            let b = baseline_by_rel.get(rel.as_str());
            let (source, location) = match (kind, saved_by_rel.get(rel.as_str())) {
                (EntryKind::Added, _) => (EarlierVersion::None, None),
                _ if is_dir => (EarlierVersion::None, None),
                (_, Some(copy)) => (EarlierVersion::Saved, Some(copy.location.clone())),
                _ if m.zip(b).is_some_and(|(m, b)| {
                    State::of_manifest(m) == State::of_baseline(b) && b.remote_id.is_some()
                }) =>
                {
                    (EarlierVersion::Synced, None)
                }
                _ => (EarlierVersion::None, None),
            };
            entries.push(AgentEntry {
                key: b.map(|b| b.key.clone()).unwrap_or_else(|| rel.clone()),
                rel,
                kind,
                is_dir,
                before_size: m.filter(|m| !m.is_dir).map(|m| m.size),
                before_mtime: m.filter(|m| !m.is_dir).map(|m| m.mtime),
                after_size: c.filter(|e| !e.is_dir).map(|e| e.size),
                after_mtime: c.filter(|e| !e.is_dir).map(|e| e.mtime),
                source,
                location,
                outcome: None,
            });
        }
        // Copies of files the agent did not change are not needed.
        let kept: BTreeSet<&str> = entries.iter().filter_map(|e| e.location.as_deref()).collect();
        let dir = self.agent_dir(&cs.id)?;
        for copy in &saved {
            if !kept.contains(copy.location.as_str()) {
                let _ = std::fs::remove_file(dir.join(&copy.location));
            }
        }
        if entries.is_empty() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        self.store
            .lock()
            .end_changeset(&cs.id, &entries, self.clock.now_ms())?;
        self.store.lock().changeset(&cs.id).map_err(Into::into)
    }

    /// Changesets, newest first: of one pair or all; only open and held ones unless `all`.
    pub fn agent_changesets(&self, pair_id: Option<&str>, all: bool) -> Result<Vec<Changeset>> {
        Ok(self.store.lock().changesets(pair_id, all)?)
    }

    pub fn agent_changeset(&self, changeset_id: &str) -> Result<ChangesetDetails> {
        let s = self.store.lock();
        let changeset = s
            .changeset(changeset_id)?
            .ok_or_else(|| EngineError::NotFound(format!("changeset {changeset_id}")))?;
        let pair_name = s.get_pair(changeset.pair()?)?.map(|p| p.name).unwrap_or_default();
        let mut entries = s.agent_entries(changeset_id)?;
        entries.sort_by(|a, b| a.rel.cmp(&b.rel));
        let decided_by = s.changeset_decided_by(changeset_id)?;
        Ok(ChangesetDetails {
            changeset,
            pair_name,
            entries,
            decided_by,
        })
    }

    fn held(&self, changeset_id: &str) -> Result<Changeset> {
        let cs = self
            .store
            .lock()
            .changeset(changeset_id)?
            .ok_or_else(|| EngineError::NotFound(format!("changeset {changeset_id}")))?;
        match cs.status {
            ChangesetStatus::Held => Ok(cs),
            ChangesetStatus::Open => Err(EngineError::Other(format!(
                "{} is still working; end the session first",
                cs.actor
            ))),
            s => Err(EngineError::Other(format!(
                "this changeset was already decided ({})",
                s.as_str()
            ))),
        }
    }

    /// Keeps the agent's work: its files sync from the next run on.
    pub fn agent_accept(&self, changeset_id: &str) -> Result<()> {
        self.agent_accept_by(changeset_id, None)
    }

    /// [`Engine::agent_accept`], recording which client accepted (security review 2026-10: the
    /// history says who let an agent's work out).
    pub fn agent_accept_by(&self, changeset_id: &str, by: Option<&str>) -> Result<()> {
        let cs = self.held(changeset_id)?;
        self.store
            .lock()
            .decide_changeset_by(&cs.id, ChangesetStatus::Accepted, self.clock.now_ms(), by)?;
        let _ = std::fs::remove_dir_all(self.agent_dir(&cs.id)?);
        Ok(())
    }

    /// Puts back every file the session changed, except files changed after it ended.
    pub async fn agent_revert(&self, changeset_id: &str) -> Result<RevertReport> {
        let cs = self.held(changeset_id)?;
        let pair = self.pair(cs.pair()?)?;
        let fs = self.env.local_fs(&pair)?;
        let entries = self.store.lock().agent_entries(&cs.id)?;
        let mut report = RevertReport::default();
        for e in &entries {
            // A revert cut short (crash, reboot) carries on where it stopped.
            if e.outcome.as_deref() == Some("reverted") {
                report.reverted.push(e.rel.clone());
                continue;
            }
            let outcome = match self.revert_entry(&pair, &fs, &cs, e).await {
                Ok(true) => {
                    report.reverted.push(e.rel.clone());
                    "reverted".to_owned()
                }
                Ok(false) => {
                    report.skipped.push(e.rel.clone());
                    "skipped_changed".to_owned()
                }
                Err(err) => {
                    report.failed.push((e.rel.clone(), err.to_string()));
                    format!("failed: {err}")
                }
            };
            self.store
                .lock()
                .set_entry_outcome_at(&cs.id, &e.rel, &outcome, self.clock.now_ms())?;
        }
        let status = if report.skipped.is_empty() && report.failed.is_empty() {
            ChangesetStatus::Reverted
        } else {
            ChangesetStatus::PartlyReverted
        };
        self.store
            .lock()
            .decide_changeset(&cs.id, status, self.clock.now_ms())?;
        let _ = std::fs::remove_dir_all(self.agent_dir(&cs.id)?);
        Ok(report)
    }

    /// Reverts one entry. `Ok(false)`: the file changed after the session, left alone.
    async fn revert_entry(
        &self,
        pair: &Pair,
        fs: &Arc<dyn LocalFs>,
        cs: &Changeset,
        e: &AgentEntry,
    ) -> Result<bool> {
        let rel = e.rel.clone();
        let current = {
            let (fs, rel) = (fs.clone(), rel.clone());
            tokio::task::spawn_blocking(move || fs.stat(&rel))
                .await
                .map_err(|e| EngineError::Other(e.to_string()))??
        };
        if e.is_dir {
            let (fs, rel) = (fs.clone(), rel.clone());
            return match e.kind {
                EntryKind::Added => {
                    tokio::task::spawn_blocking(move || fs.remove_dir_if_empty(&rel).map(|_| true))
                        .await
                        .map_err(|e| EngineError::Other(e.to_string()))?
                        .map_err(Into::into)
                }
                _ => tokio::task::spawn_blocking(move || fs.mkdir(&rel).map(|_| true))
                    .await
                    .map_err(|e| EngineError::Other(e.to_string()))?
                    .map_err(Into::into),
            };
        }
        let now = current.as_ref().map(|c| (c.is_dir, c.size, c.mtime_ms));
        let after = e.after_size.zip(e.after_mtime).map(|(s, m)| (false, s, m));
        if now != after {
            return Ok(false);
        }
        if e.kind != EntryKind::Added && e.source == EarlierVersion::None {
            return Err(EngineError::NotFound(
                "no earlier version was kept (the file was too large to copy)".into(),
            ));
        }

        // Fetch the earlier version first, so a failure leaves the agent's version in place.
        let staging = {
            let (fs, rel) = (fs.clone(), rel.clone());
            tokio::task::spawn_blocking(move || fs.staging_path(&rel))
                .await
                .map_err(|e| EngineError::Other(e.to_string()))??
        };
        match e.source {
            EarlierVersion::Saved => {
                let src = self
                    .agent_dir(&cs.id)?
                    .join(e.location.as_deref().unwrap_or_default());
                // Checked in staging, the bytes that would be installed, so the copy cannot change
                // between the check and the install.
                std::fs::copy(&src, &staging)?;
                if let Err(err) = std::fs::read(&staging)
                    .map_err(EngineError::from)
                    .and_then(|bytes| self.check_saved(&cs.id, &e.rel, &bytes))
                {
                    let _ = std::fs::remove_file(&staging);
                    return Err(err);
                }
            }
            EarlierVersion::Synced => self.fetch_synced(pair, &e.key, &staging).await?,
            EarlierVersion::None => {}
        }

        if current.is_some() {
            self.bin_agent_version(pair, fs, e).await?;
        }
        if e.kind != EntryKind::Added {
            let (fs, rel, mtime) = (fs.clone(), rel.clone(), e.before_mtime);
            tokio::task::spawn_blocking(move || {
                if let Some((parent, _)) = rel.rsplit_once('/') {
                    fs.mkdir(parent)?;
                }
                fs.install(&staging, &rel, mtime)
            })
            .await
            .map_err(|e| EngineError::Other(e.to_string()))??;
        }
        Ok(true)
    }

    /// Downloads the synced version of `key` (what the baseline recorded) to `dest`.
    async fn fetch_synced(&self, pair: &Pair, key: &str, dest: &std::path::Path) -> Result<()> {
        let b = self
            .store
            .lock()
            .baseline_entry(&pair.id, key)?
            .ok_or_else(|| EngineError::NotFound(format!("{key} has no synced version")))?;
        let remote = self.env.remote(pair)?;
        let root = pair
            .drive_folder_id
            .clone()
            .ok_or_else(|| EngineError::NotFound("this pair has no Drive folder".into()))?;
        let listing = remote.list_subtree(&root, pair.drive_id.as_deref()).await?;
        let entry = listing
            .entries
            .into_iter()
            .find(|r| r.key == key)
            .ok_or_else(|| EngineError::NotFound(format!("{key} is no longer on Drive")))?;
        let same = match (&entry.md5, &b.remote_md5) {
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
            _ => entry.version == b.remote_version && entry.version.is_some(),
        };
        if !same {
            return Err(EngineError::Integrity(format!(
                "{key} changed on another device since; restore it from its versions instead"
            )));
        }
        remote.download(&entry, dest, &|_| {}).await?;
        Ok(())
    }

    /// Moves the agent's version of an entry to the recycle bin, so a revert can be undone.
    async fn bin_agent_version(&self, pair: &Pair, fs: &Arc<dyn LocalFs>, e: &AgentEntry) -> Result<()> {
        let (fs, rel) = (fs.clone(), e.rel.clone());
        let location = tokio::task::spawn_blocking(move || fs.move_to_bin(&rel))
            .await
            .map_err(|e| EngineError::Other(e.to_string()))??;
        let now = self.clock.now_ms();
        self.store.lock().add_to_bin(&NewBinEntry {
            id: new_id(self.clock.as_ref()),
            pair_id: pair.id.clone(),
            side: BinSide::Local,
            key: e.key.clone(),
            reason: AGENT_REVERT.into(),
            location,
            deleted_at: now,
            expires_at: now + BIN_RETENTION_MS,
            size: e.after_size.unwrap_or(0),
            account_id: None,
            remote_file_id: None,
            display_name: Some(e.rel.rsplit('/').next().unwrap_or(&e.rel).to_owned()),
            pair_name: Some(pair.name.clone()),
        })?;
        Ok(())
    }

    /// The one-time copy of agent changesets into the op-log (ADR-0002, FR-OL7): those from before
    /// schema v11, and any made while this device had no id. Call once at start-up, after
    /// [`Engine::recover`]; a second call copies nothing. Returns how many were copied (none
    /// without a device id in the engine's config).
    pub fn copy_agent_changesets_to_oplog(&self) -> Result<usize> {
        Ok(self.store.lock().copy_changesets_to_oplog(self.clock.now_ms())?)
    }

    /// Drops changesets decided more than [`AGENT_HISTORY_MS`] ago and every copy folder that no open
    /// or held changeset owns (left by a crash, or by a pair that was removed). Returns how many
    /// changesets went. Runs with the recycle-bin purge.
    pub fn agent_housekeeping(&self) -> Result<usize> {
        let n = self
            .store
            .lock()
            .prune_changesets(self.clock.now_ms() - AGENT_HISTORY_MS)?;
        let Some(base) = self.config.agent_dir.clone() else {
            return Ok(n);
        };
        let live: BTreeSet<String> = self
            .store
            .lock()
            .changesets(None, false)?
            .into_iter()
            .map(|c| c.id)
            .collect();
        if let Ok(dirs) = std::fs::read_dir(&base) {
            for d in dirs.flatten() {
                let name = d.file_name().to_string_lossy().into_owned();
                if !live.contains(&name) {
                    let _ = std::fs::remove_dir_all(d.path());
                }
            }
        }
        Ok(n)
    }

    /// A unified diff of one entry: earlier version against the agent's. Text files up to 1 MB.
    pub async fn agent_diff(&self, changeset_id: &str, rel: &str) -> Result<String> {
        const LIMIT: u64 = 1024 * 1024;
        let d = self.agent_changeset(changeset_id)?;
        let e = d
            .entries
            .iter()
            .find(|e| e.rel == rel)
            .ok_or_else(|| EngineError::NotFound(format!("{rel} is not in this changeset")))?;
        if e.is_dir {
            return Ok(String::new());
        }
        if e.before_size.unwrap_or(0) > LIMIT || e.after_size.unwrap_or(0) > LIMIT {
            return Ok("(larger than 1 MB, not shown)\n".into());
        }
        let pair = self.pair(d.changeset.pair()?)?;
        let before: Vec<u8> = match e.source {
            EarlierVersion::Saved if d.changeset.status.holds() => {
                let bytes = std::fs::read(
                    self.agent_dir(changeset_id)?
                        .join(e.location.as_deref().unwrap_or_default()),
                )?;
                self.check_saved(changeset_id, &e.rel, &bytes)?;
                bytes
            }
            EarlierVersion::Synced if d.changeset.status.holds() => {
                let tmp = self
                    .agent_dir(changeset_id)?
                    .join(format!("diff-{}", new_id(self.clock.as_ref())));
                std::fs::create_dir_all(tmp.parent().unwrap_or(&tmp))?;
                self.fetch_synced(&pair, &e.key, &tmp).await?;
                let bytes = std::fs::read(&tmp)?;
                let _ = std::fs::remove_file(&tmp);
                bytes
            }
            _ => Vec::new(),
        };
        let after: Vec<u8> = if e.kind == EntryKind::Deleted {
            Vec::new()
        } else {
            let fs = self.env.local_fs(&pair)?;
            let rel = e.rel.clone();
            tokio::task::spawn_blocking(move || -> io::Result<Vec<u8>> {
                let mut buf = Vec::new();
                io::Read::read_to_end(&mut fs.open_read(&rel)?, &mut buf)?;
                Ok(buf)
            })
            .await
            .map_err(|e| EngineError::Other(e.to_string()))??
        };
        let (Ok(before), Ok(after)) = (std::str::from_utf8(&before), std::str::from_utf8(&after)) else {
            return Ok("(binary file, not shown)\n".into());
        };
        Ok(similar::TextDiff::from_lines(before, after)
            .unified_diff()
            .context_radius(3)
            .header(&format!("a/{rel}"), &format!("b/{rel}"))
            .to_string())
    }

    /// Keys sync must leave alone now (see [`held_rels`]).
    pub(crate) fn agent_held_keys(&self, pair_id: &str, input: &PlanInput) -> Result<BTreeSet<String>> {
        let local: Vec<&LocalEntry> = input.local.values().collect();
        let rels = held_rels(&self.store.lock(), pair_id, &local)?;
        if rels.is_empty() {
            return Ok(rels);
        }
        let key_by_rel: BTreeMap<&str, &str> = input
            .local_rel_by_key
            .iter()
            .map(|(k, r)| (r.as_str(), k.as_str()))
            .collect();
        Ok(rels
            .into_iter()
            .map(|rel| {
                key_by_rel
                    .get(rel.as_str())
                    .map(|k| (*k).to_owned())
                    .unwrap_or(rel)
            })
            .collect())
    }
}
