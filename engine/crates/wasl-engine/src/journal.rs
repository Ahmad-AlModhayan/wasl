//! Agent undo's journal (docs/AGENT_UNDO.md §5): every change a session makes in a watched folder,
//! with its new content kept (UND-3), and what a gap in watching made Wasl miss (UND-4). Each entry
//! is also an op-log operation, written in the same transaction (UND-9, ADR-0002).
//!
//! The daemon's watcher calls [`Engine::agent_capture`] for each changed path and
//! [`Engine::agent_rescan`] after an overflow, a watcher error or a restart.

use crate::engine::Engine;
use crate::error::{EngineError, Result};
use crate::undo::{UndoStore, gone, same_file, stat_key, sync_objects, watch_rules};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use wasl_store::{JournalEntry, SnapshotOrigin, StatKey, Watch};

/// Larger files are journaled but not captured (UND-11).
pub const MAX_CAPTURE: u64 = 2 * 1024 * 1024 * 1024;

pub use wasl_proto::engine::{JournalReport, Version};

/// What Wasl last knew of one path in a session.
enum Known {
    Missing,
    File {
        blake3: Option<String>,
        /// Whether its bytes can be had back (kept here, or synced).
        kept: bool,
    },
}

/// Whether the default ignores still applied to `w` cover `rel` or a folder above it.
/// `.waslignore` files apply through the rescan's folder scan.
fn ignored(w: &Watch, rel: &str) -> bool {
    let Ok(rules) = watch_rules(&w.ignores).build() else {
        return false;
    };
    let mut prefix = String::new();
    let parts: Vec<&str> = rel.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(part);
        if rules.is_ignored(&prefix, i + 1 < parts.len()) {
            return true;
        }
    }
    false
}

fn kept(store: &UndoStore, origin: Option<SnapshotOrigin>, location: Option<&str>) -> bool {
    match (origin, location) {
        (Some(SnapshotOrigin::Synced), _) => true,
        (_, Some(l)) => store.path(l).exists(),
        _ => false,
    }
}

impl Engine {
    /// The watched folder `watch_id` as the session `changeset_id` journals it.
    fn journal_root(&self, changeset_id: &str, watch_id: &str) -> Result<Watch> {
        self.store
            .lock()
            .journal_roots(changeset_id)?
            .into_iter()
            .find(|w| w.id == watch_id)
            .ok_or_else(|| {
                EngineError::NotFound(format!("watched folder {watch_id} in session {changeset_id}"))
            })
    }

    fn known(&self, store: &UndoStore, changeset_id: &str, watch_id: &str, rel: &str) -> Result<Known> {
        let s = self.store.lock();
        if let Some(e) = s.journal_last(changeset_id, watch_id, rel)? {
            return Ok(match e.op.as_str() {
                "delete" => Known::Missing,
                _ => Known::File {
                    kept: e.captured && kept(store, e.origin, e.location.as_deref()),
                    blake3: e.next_blake3,
                },
            });
        }
        Ok(match s.snapshot_row(changeset_id, watch_id, rel)? {
            Some(r) => Known::File {
                kept: kept(store, Some(r.origin), r.location.as_deref()),
                blake3: r.blake3,
            },
            None => Known::Missing,
        })
    }

    /// Journals what `rel` in watched folder `watch_id` is now, against what the session last
    /// knew of it (UND-3): a new or changed file is captured into the undo store, a removed one
    /// journals the version it had. `seen_late` is why the change was found by a rescan rather
    /// than as it happened. Returns the entry, or `None` when nothing changed (or the path is
    /// ignored, a folder or a link).
    pub async fn agent_capture(
        &self,
        changeset_id: &str,
        watch_id: &str,
        rel: &str,
        seen_late: Option<&str>,
    ) -> Result<Option<JournalEntry>> {
        let late =
            seen_late.map(|why| format!("found by a rescan after {why}; versions in between were not seen"));
        self.capture(changeset_id, watch_id, rel, late, None).await
    }

    /// As [`Engine::agent_capture`], for a change the watcher saw together with others on the
    /// same path: `note` says the versions in between were not seen, and the entry is listed.
    pub async fn agent_capture_noted(
        &self,
        changeset_id: &str,
        watch_id: &str,
        rel: &str,
        note: &str,
    ) -> Result<Option<JournalEntry>> {
        self.capture(changeset_id, watch_id, rel, Some(note.to_owned()), None)
            .await
    }

    /// `late` is why versions before this one may not have been seen; `renamed_from` names the
    /// path a `create` came from (written with it, in one transaction).
    async fn capture(
        &self,
        changeset_id: &str,
        watch_id: &str,
        rel: &str,
        late: Option<String>,
        renamed_from: Option<&str>,
    ) -> Result<Option<JournalEntry>> {
        wasl_fs::check_rel(rel)?;
        let w = self.journal_root(changeset_id, watch_id)?;
        if ignored(&w, rel) {
            return Ok(None);
        }
        let store = self.undo_store()?;
        let known = self.known(&store, changeset_id, watch_id, rel)?;
        let path = Path::new(&w.root).join(rel);
        let store_root = store.root.clone();
        // The stat before the read: a write after it changes the stat, so a rescan reads again.
        let (key, taken) = tokio::task::spawn_blocking(move || {
            (stat_key(&path), capture_file(&UndoStore::new(store_root), &path))
        })
        .await
        .map_err(|e| EngineError::Other(e.to_string()))?;
        let mut e = JournalEntry {
            seq: 0,
            root: watch_id.to_owned(),
            op: String::new(),
            rel: rel.to_owned(),
            old_rel: None,
            prev_blake3: None,
            next_blake3: None,
            origin: None,
            location: None,
            captured: true,
            reason: late.clone(),
            at: self.clock.now_ms(),
        };
        let next = match (taken, known) {
            (Captured::NotAFile, _) | (Captured::Missing, Known::Missing) => return Ok(None),
            (Captured::Missing, Known::File { blake3, kept }) => {
                e.op = "delete".into();
                e.prev_blake3 = blake3;
                if !kept {
                    e.captured = false;
                    e.reason = Some(join(late, "the version it had was not kept"));
                }
                None
            }
            (
                Captured::File { blake3, .. },
                Known::File {
                    blake3: Some(was), ..
                },
            ) if blake3 == was => {
                self.store
                    .lock()
                    .put_journal_stat(changeset_id, watch_id, rel, key)?;
                return Ok(None);
            }
            (taken, known) => {
                if let Known::File { blake3, .. } = known {
                    e.op = "update".into();
                    e.prev_blake3 = blake3;
                } else {
                    e.op = "create".into();
                    e.old_rel = renamed_from.map(str::to_owned);
                }
                match taken {
                    Captured::File {
                        blake3,
                        size,
                        mtime,
                        location,
                    } => {
                        e.next_blake3 = Some(blake3);
                        e.origin = Some(SnapshotOrigin::Stored);
                        e.location = Some(location);
                        Some((size, mtime))
                    }
                    Captured::NotKept { size, mtime, why } => {
                        e.captured = false;
                        e.reason = Some(join(late, &why));
                        Some((size, mtime))
                    }
                    Captured::Missing | Captured::NotAFile => unreachable!("handled above"),
                }
            }
        };
        let mut s = self.store.lock();
        e.seq = s.append_journal(changeset_id, &e, next)?;
        let key = if e.op == "delete" { None } else { key };
        s.put_journal_stat(changeset_id, watch_id, rel, key)?;
        Ok(Some(e))
    }

    /// A rename seen as one event: journals a `delete` of `from` and a `create` of `to` naming it
    /// (UND-9).
    pub async fn agent_capture_rename(
        &self,
        changeset_id: &str,
        watch_id: &str,
        from: &str,
        to: &str,
    ) -> Result<Vec<JournalEntry>> {
        let mut out = Vec::new();
        out.extend(self.agent_capture(changeset_id, watch_id, from, None).await?);
        out.extend(self.capture(changeset_id, watch_id, to, None, Some(from)).await?);
        Ok(out)
    }

    /// After a watcher overflow, a watcher error or a daemon restart (`why`): compares every
    /// watched folder of the session with what it last knew and journals each difference (UND-4).
    /// A file whose last known version wasn't kept is listed as not captured, with the reason.
    /// Returns the entries added.
    ///
    /// A folder that can't be rescanned (removed, unreadable) doesn't stop the others: each file
    /// the session last knew there is listed as missed, with the error, and the call fails once
    /// the rest are done so the caller tries again.
    pub async fn agent_rescan(&self, changeset_id: &str, why: &str) -> Result<Vec<JournalEntry>> {
        let roots = self.store.lock().journal_roots(changeset_id)?;
        let mut out = Vec::new();
        let mut failed = Vec::new();
        for w in roots {
            match self.rescan_root(changeset_id, &w, why, &mut out).await {
                Ok(()) => self.store.lock().clear_watch_gaps(changeset_id, &w.id)?,
                Err(e) => {
                    let reason = format!("can't rescan {}: {e}", w.root);
                    self.note_missed(changeset_id, &w.id, &reason)?;
                    failed.push(reason);
                }
            }
        }
        match failed.is_empty() {
            true => Ok(out),
            false => Err(EngineError::Other(failed.join("; "))),
        }
    }

    /// Lists each file the session last knew in `watch_id` as missed, for `reason`.
    fn note_missed(&self, changeset_id: &str, watch_id: &str, reason: &str) -> Result<()> {
        let at = self.clock.now_ms();
        let mut s = self.store.lock();
        let mut rels: BTreeSet<String> = s
            .snapshot(changeset_id, watch_id)?
            .into_iter()
            .map(|r| r.rel)
            .collect();
        rels.extend(
            s.journal(changeset_id)?
                .into_iter()
                .filter(|e| e.root == watch_id)
                .map(|e| e.rel),
        );
        for rel in rels {
            let gone = s
                .journal_last(changeset_id, watch_id, &rel)?
                .is_some_and(|e| e.op == "delete");
            if !gone {
                s.put_watch_gap(changeset_id, watch_id, &rel, reason, at)?;
            }
        }
        Ok(())
    }

    /// Records that the session `changeset_id` couldn't watch `watch_id` (UND-4): the session
    /// report lists it until the session ends.
    pub fn agent_watch_gap(&self, changeset_id: &str, watch_id: &str, reason: &str) -> Result<()> {
        let at = self.clock.now_ms();
        Ok(self
            .store
            .lock()
            .put_watch_gap(changeset_id, watch_id, "", reason, at)?)
    }

    async fn rescan_root(
        &self,
        changeset_id: &str,
        w: &Watch,
        why: &str,
        out: &mut Vec<JournalEntry>,
    ) -> Result<()> {
        let store = self.undo_store()?;
        {
            let (snapshot, journaled, stats) = {
                let s = self.store.lock();
                let journaled: BTreeSet<String> = s
                    .journal(changeset_id)?
                    .into_iter()
                    .filter(|e| e.root == w.id)
                    .flat_map(|e| [Some(e.rel), e.old_rel])
                    .flatten()
                    .collect();
                let stats = s.journal_stats(changeset_id, &w.id)?;
                (s.snapshot(changeset_id, &w.id)?, journaled, stats)
            };
            let (root, ignores, tmp) = (PathBuf::from(&w.root), w.ignores.clone(), store.root.join("tmp"));
            // Each file found, with its reuse key when it can be read (`None`: read it).
            let now = tokio::task::spawn_blocking(move || {
                let fs_ = wasl_fs::StdFs::new(root.clone(), tmp);
                let scan = wasl_fs::scan(&fs_, watch_rules(&ignores), &|rel: &str| rel.to_owned(), || false)?;
                Ok::<_, EngineError>(
                    scan.entries
                        .into_iter()
                        .filter(|e| !e.is_dir)
                        .map(|e| {
                            let key = stat_key(&root.join(&e.rel));
                            (e.rel, key)
                        })
                        .collect::<HashMap<String, Option<StatKey>>>(),
                )
            })
            .await
            .map_err(|e| EngineError::Other(e.to_string()))??;
            let at_start: HashMap<&str, StatKey> = snapshot
                .iter()
                .map(|r| (r.rel.as_str(), (r.size, r.mtime, r.ctime, r.file_id)))
                .collect();
            let mut rels: BTreeSet<&str> = now.keys().map(String::as_str).collect();
            rels.extend(at_start.keys().copied());
            rels.extend(journaled.iter().map(String::as_str));
            for rel in rels {
                // Untouched since the snapshot or the last capture, by the snapshot's reuse key:
                // nothing to read (and no second entry for a file too large to keep).
                let was = match journaled.contains(rel) {
                    true => stats.get(rel),
                    false => at_start.get(rel),
                };
                if let (Some(was), Some(Some(now))) = (was, now.get(rel))
                    && same_file(*was, *now)
                {
                    continue;
                }
                if !now.contains_key(rel) && Path::new(&w.root).join(rel).exists() {
                    // Still there but left out of the scan: ignored.
                    continue;
                }
                out.extend(self.agent_capture(changeset_id, &w.id, rel, Some(why)).await?);
            }
        }
        Ok(())
    }

    /// Every version of `rel` in watched folder `watch_id` that the session saw, oldest first
    /// (UND-3): what a restore can go back to.
    pub fn agent_versions(&self, changeset_id: &str, watch_id: &str, rel: &str) -> Result<Vec<Version>> {
        let store = self.undo_store()?;
        let path = |location: &Option<String>| location.as_deref().map(|l| store.path(l));
        let s = self.store.lock();
        let mut out = Vec::new();
        if let Some(r) = s.snapshot_row(changeset_id, watch_id, rel)? {
            let started = s.changeset(changeset_id)?.map(|c| c.started_at).unwrap_or(0);
            out.push(Version {
                seq: 0,
                blake3: r.blake3.clone(),
                path: path(&r.location),
                captured: true,
                reason: None,
                at: started,
            });
        }
        for e in s
            .journal(changeset_id)?
            .into_iter()
            .filter(|e| e.root == watch_id && e.rel == rel)
        {
            out.push(Version {
                seq: e.seq,
                blake3: e.next_blake3.clone(),
                path: path(&e.location),
                captured: e.captured,
                reason: e.reason.clone(),
                at: e.at,
            });
        }
        Ok(out)
    }

    /// The session's journal and what it missed: entries not captured or found late, and the
    /// watched folders it couldn't snapshot (UND-4, for UND-10's report).
    pub fn agent_journal(&self, changeset_id: &str) -> Result<JournalReport> {
        let s = self.store.lock();
        if s.changeset(changeset_id)?.is_none() {
            return Err(EngineError::NotFound(format!("changeset {changeset_id}")));
        }
        let entries = s.journal(changeset_id)?;
        let not_captured = entries
            .iter()
            .filter(|e| !e.captured || e.reason.is_some())
            .cloned()
            .collect();
        Ok(JournalReport {
            entries,
            not_captured,
            snapshot_failures: s.snapshot_failures(changeset_id)?,
            gaps: s.watch_gaps(changeset_id)?,
        })
    }
}

fn join(late: Option<String>, why: &str) -> String {
    match late {
        Some(l) => format!("{why}; {l}"),
        None => why.to_owned(),
    }
}

/// A file as the capture found it.
enum Captured {
    File {
        blake3: String,
        size: u64,
        mtime: i64,
        location: String,
    },
    /// There, but its bytes were not kept.
    NotKept {
        size: u64,
        mtime: i64,
        why: String,
    },
    Missing,
    /// A folder or a link: not journaled.
    NotAFile,
}

fn mtime_ms(m: &fs::Metadata) -> i64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Keeps `path`'s bytes in the store, reading them once.
fn capture_file(store: &UndoStore, path: &Path) -> Captured {
    let m = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if gone(&e) => return Captured::Missing,
        Err(e) => {
            return Captured::NotKept {
                size: 0,
                mtime: 0,
                why: format!("can't be read: {e}"),
            };
        }
    };
    if !m.is_file() {
        return Captured::NotAFile;
    }
    let (size, mtime) = (m.len(), mtime_ms(&m));
    if size > MAX_CAPTURE {
        return Captured::NotKept {
            size,
            mtime,
            why: "larger than 2 GB: journaled, not captured".into(),
        };
    }
    match store.store(path) {
        Ok((blake3, size, location, fresh)) => {
            if fresh && let Err(e) = sync_objects(&store.root, &[store.path(&location)]) {
                return Captured::NotKept {
                    size,
                    mtime,
                    why: format!("can't be kept: {e}"),
                };
            }
            Captured::File {
                blake3,
                size,
                mtime,
                location,
            }
        }
        Err(e) if gone(&e) => Captured::Missing,
        Err(e) => Captured::NotKept {
            size,
            mtime,
            why: format!("can't be read: {e}"),
        },
    }
}
