//! Agent undo's working detail (docs/AGENT_UNDO.md §6, schema v12): the watch list and what each
//! session found in the watched folders at its start. The journal, markers and restores
//! (`agent_journal`, `agent_markers`, `agent_restores`) record what a session changed there.

use crate::agent::oplog_record;
use crate::oplog::{self, NewOp, file_uri};
use crate::{Result, Store, StoreError, to_i64};
use rusqlite::{OptionalExtension, params};
use serde_json::json;
use std::collections::HashMap;

pub use wasl_proto::store::{
    DEFAULT_WATCH_IGNORES, JournalEntry, NextMeta, SnapshotOrigin, SnapshotRow, StatKey, Watch, WatchGap,
};

fn parse_snapshot_origin(s: &str) -> Result<SnapshotOrigin> {
    SnapshotOrigin::parse(s).ok_or_else(|| StoreError::Corrupt(format!("snapshot origin {s:?}")))
}

const JOURNAL_COLUMNS: &str =
    "seq, root, op, rel, old_rel, prev_blake3, next_blake3, origin, location, captured, reason, at";
fn watch_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<Watch> {
    let ignores: String = r.get(2)?;
    Ok(Watch {
        id: r.get(0)?,
        root: r.get(1)?,
        ignores: ignores
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect(),
        added_at: r.get(3)?,
    })
}

fn size_of(n: u64) -> Result<i64> {
    i64::try_from(n).map_err(|_| StoreError::Corrupt(format!("size {n} is too large")))
}

impl Store {
    /// Adds `root` to the watch list with the default ignores. The caller checks the folder
    /// (not a file-system root, not Wasl's own data). Refused when it is watched already.
    pub fn add_watch(&mut self, id: &str, root: &str, at: i64) -> Result<Watch> {
        if self.watch_at(root)?.is_some() {
            return Err(StoreError::Refused(format!("{root} is watched already.")));
        }
        let ignores = DEFAULT_WATCH_IGNORES.join("\n");
        self.conn.execute(
            "INSERT INTO agent_watches (id, root, ignores, added_at) VALUES (?1, ?2, ?3, ?4)",
            params![id, root, ignores, at],
        )?;
        Ok(Watch {
            id: id.to_owned(),
            root: root.to_owned(),
            ignores: DEFAULT_WATCH_IGNORES.map(str::to_owned).to_vec(),
            added_at: at,
        })
    }

    /// The folders on the watch list, by path.
    pub fn watches(&self) -> Result<Vec<Watch>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, root, ignores, added_at FROM agent_watches WHERE removed_at IS NULL ORDER BY root",
        )?;
        let rows = stmt.query_map([], watch_from)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The watch of `root` (exactly that path), if it is on the list.
    pub fn watch_at(&self, root: &str) -> Result<Option<Watch>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, root, ignores, added_at FROM agent_watches WHERE root = ?1 AND removed_at IS NULL",
                [root],
                watch_from,
            )
            .optional()?)
    }

    /// Takes a watch off the list. Its sessions keep their rows. Returns whether it was on it.
    pub fn remove_watch(&mut self, id: &str, at: i64) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE agent_watches SET removed_at = ?2 WHERE id = ?1 AND removed_at IS NULL",
            params![id, at],
        )? > 0)
    }

    /// Replaces the default ignores a watch still applies.
    pub fn set_watch_ignores(&mut self, id: &str, ignores: &[String]) -> Result<()> {
        if let Some(bad) = ignores.iter().find(|p| p.is_empty() || p.contains('\n')) {
            return Err(StoreError::Refused(format!("{bad:?} is not an ignore pattern.")));
        }
        let n = self.conn.execute(
            "UPDATE agent_watches SET ignores = ?2 WHERE id = ?1 AND removed_at IS NULL",
            params![id, ignores.join("\n")],
        )?;
        if n == 0 {
            return Err(StoreError::Refused(format!("no watch {id}")));
        }
        Ok(())
    }

    /// Records what the session `changeset_id` found in the watched folder `watch_id`, replacing
    /// what was recorded before for the pair of them.
    pub fn put_snapshot(&mut self, changeset_id: &str, watch_id: &str, rows: &[SnapshotRow]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for table in ["agent_snapshots", "agent_snapshot_failures"] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE changeset_id = ?1 AND watch_id = ?2"),
                params![changeset_id, watch_id],
            )?;
        }
        tx.execute(
            "INSERT OR IGNORE INTO agent_session_watches (changeset_id, watch_id) VALUES (?1, ?2)",
            params![changeset_id, watch_id],
        )?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO agent_snapshots
                     (changeset_id, watch_id, rel, size, mtime, ctime, file_id, blake3, origin, location)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            for r in rows {
                stmt.execute(params![
                    changeset_id,
                    watch_id,
                    r.rel,
                    size_of(r.size)?,
                    r.mtime,
                    r.ctime,
                    r.file_id,
                    r.blake3,
                    r.origin.as_str(),
                    r.location
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Records that the session `changeset_id` couldn't snapshot `watch_id`, and why, in place of
    /// what was recorded before for the pair of them.
    pub fn put_snapshot_failure(
        &mut self,
        changeset_id: &str,
        watch_id: &str,
        reason: &str,
        at: i64,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        for table in ["agent_snapshots", "agent_session_watches"] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE changeset_id = ?1 AND watch_id = ?2"),
                params![changeset_id, watch_id],
            )?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO agent_snapshot_failures (changeset_id, watch_id, reason, at)
             VALUES (?1, ?2, ?3, ?4)",
            params![changeset_id, watch_id, reason, at],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The watched folders the session `changeset_id` couldn't snapshot: (watch id, why).
    pub fn snapshot_failures(&self, changeset_id: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT watch_id, reason FROM agent_snapshot_failures WHERE changeset_id = ?1 ORDER BY watch_id",
        )?;
        let rows = stmt.query_map(params![changeset_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Records that the session `changeset_id` missed `rel` in `watch_id` (`""` for the whole
    /// folder), and why, in place of what was recorded for that path before.
    pub fn put_watch_gap(
        &mut self,
        changeset_id: &str,
        watch_id: &str,
        rel: &str,
        reason: &str,
        at: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO agent_watch_gaps (changeset_id, watch_id, rel, reason, at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![changeset_id, watch_id, rel, reason, at],
        )?;
        Ok(())
    }

    /// Forgets the files listed as missed in `watch_id` once a rescan of it went through (it
    /// journaled each difference); a gap for the whole folder stays.
    pub fn clear_watch_gaps(&mut self, changeset_id: &str, watch_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM agent_watch_gaps WHERE changeset_id = ?1 AND watch_id = ?2 AND rel <> ''",
            params![changeset_id, watch_id],
        )?;
        Ok(())
    }

    /// What the session `changeset_id` missed while Wasl couldn't watch or rescan a folder.
    pub fn watch_gaps(&self, changeset_id: &str) -> Result<Vec<WatchGap>> {
        let mut stmt = self.conn.prepare(
            "SELECT watch_id, rel, reason FROM agent_watch_gaps WHERE changeset_id = ?1
             ORDER BY watch_id, rel",
        )?;
        let rows = stmt.query_map(params![changeset_id], |r| {
            let rel: String = r.get(1)?;
            Ok(WatchGap {
                watch_id: r.get(0)?,
                rel: (!rel.is_empty()).then_some(rel),
                reason: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Notes `rel`'s stat when it was last captured; `None` forgets it (the file is gone).
    pub fn put_journal_stat(
        &mut self,
        changeset_id: &str,
        watch_id: &str,
        rel: &str,
        stat: Option<StatKey>,
    ) -> Result<()> {
        match stat {
            Some((size, mtime, ctime, file_id)) => self.conn.execute(
                "INSERT OR REPLACE INTO agent_journal_stats
                     (changeset_id, watch_id, rel, size, mtime, ctime, file_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![changeset_id, watch_id, rel, to_i64(size), mtime, ctime, file_id],
            )?,
            None => self.conn.execute(
                "DELETE FROM agent_journal_stats WHERE changeset_id = ?1 AND watch_id = ?2 AND rel = ?3",
                params![changeset_id, watch_id, rel],
            )?,
        };
        Ok(())
    }

    /// The stat of each file of `watch_id` the session journaled, as last captured, by path.
    pub fn journal_stats(&self, changeset_id: &str, watch_id: &str) -> Result<HashMap<String, StatKey>> {
        let mut stmt = self.conn.prepare(
            "SELECT rel, size, mtime, ctime, file_id FROM agent_journal_stats
             WHERE changeset_id = ?1 AND watch_id = ?2",
        )?;
        let rows = stmt.query_map(params![changeset_id, watch_id], |r| {
            let size: i64 = r.get(1)?;
            Ok((r.get(0)?, (size.max(0) as u64, r.get(2)?, r.get(3)?, r.get(4)?)))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// What the session `changeset_id` found in `watch_id`, by path.
    pub fn snapshot(&self, changeset_id: &str, watch_id: &str) -> Result<Vec<SnapshotRow>> {
        self.snapshot_rows(
            "SELECT rel, size, mtime, ctime, file_id, blake3, origin, location FROM agent_snapshots
             WHERE changeset_id = ?1 AND watch_id = ?2 ORDER BY rel",
            params![changeset_id, watch_id],
        )
    }

    /// The newest snapshot of `watch_id` (of the session that started last), by path; empty when
    /// none was taken. A new snapshot reuses what it can from it.
    pub fn last_snapshot(&self, watch_id: &str) -> Result<Vec<SnapshotRow>> {
        self.snapshot_rows(
            "SELECT s.rel, s.size, s.mtime, s.ctime, s.file_id, s.blake3, s.origin, s.location FROM agent_snapshots s
             WHERE s.watch_id = ?1 AND s.changeset_id = (
                 SELECT c.id FROM agent_changesets c
                 WHERE EXISTS (SELECT 1 FROM agent_session_watches x WHERE x.changeset_id = c.id AND x.watch_id = ?1)
                 ORDER BY c.started_at DESC, c.id DESC LIMIT 1)
             ORDER BY s.rel",
            params![watch_id],
        )
    }

    fn snapshot_rows(&self, sql: &str, args: impl rusqlite::Params) -> Result<Vec<SnapshotRow>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(args, |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (rel, size, mtime, ctime, file_id, blake3, origin, location) = row?;
            out.push(SnapshotRow {
                rel,
                size: u64::try_from(size).map_err(|_| StoreError::Corrupt(format!("size {size}")))?,
                mtime,
                ctime,
                file_id,
                blake3,
                origin: parse_snapshot_origin(&origin)?,
                location,
            });
        }
        Ok(out)
    }

    /// Appends a journal entry to the open session `changeset_id` and, when the op-log is written,
    /// its operation on `file://<watch id>/<rel>` in the same transaction (UND-9). Returns its seq.
    pub fn append_journal(&mut self, changeset_id: &str, e: &JournalEntry, next: NextMeta) -> Result<u32> {
        let tx = self.oplog_tx()?;
        let open: Option<String> = tx
            .query_row(
                "SELECT status FROM agent_changesets WHERE id = ?1",
                [changeset_id],
                |r| r.get(0),
            )
            .optional()?;
        if open.as_deref() != Some("open") {
            return Err(StoreError::Refused(format!("session {changeset_id} is not open")));
        }
        let seq: u32 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM agent_journal WHERE changeset_id = ?1",
            [changeset_id],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO agent_journal (changeset_id, seq, root, op, rel, old_rel, prev_blake3, next_blake3,
                                        origin, location, captured, reason, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                changeset_id,
                seq,
                e.root,
                e.op,
                e.rel,
                e.old_rel,
                e.prev_blake3,
                e.next_blake3,
                e.origin.map(SnapshotOrigin::as_str),
                e.location,
                e.captured,
                e.reason,
                e.at
            ],
        )?;
        if let Some((k, _)) = oplog_record(&tx, self.oplog_device.as_deref(), changeset_id)? {
            let prev = e
                .prev_blake3
                .as_ref()
                .map(|h| json!({"kind": "file", "blake3": h}));
            let next_state = match (e.op.as_str(), next) {
                ("delete", _) => None,
                (_, Some((size, mtime))) => {
                    let mut v = json!({"kind": "file", "size": size, "mtime_ms": mtime});
                    if let Some(h) = &e.next_blake3 {
                        v["blake3"] = json!(h);
                    }
                    Some(v)
                }
                (_, None) => Some(json!({"kind": "file"})),
            };
            // A rename seen as one event: the create names the path it came from.
            let next_state = match (next_state, &e.old_rel) {
                (Some(mut v), Some(old)) if e.op == "create" => {
                    v["renamed_from"] = json!(old);
                    Some(v)
                }
                (v, _) => v,
            };
            // A create has no state before; an update or delete whose earlier content is unknown
            // still has one.
            let prev = match e.op.as_str() {
                "create" => None,
                _ => Some(prev.unwrap_or_else(|| json!({"kind": "file"}))),
            };
            oplog::append_in(
                &tx,
                &k,
                &NewOp {
                    entity_uri: file_uri(&e.root, &e.rel),
                    op: e.op.clone(),
                    prev,
                    next: next_state,
                    at: e.at,
                },
            )?;
        }
        tx.commit()?;
        Ok(seq)
    }

    /// The journal of `changeset_id`, in order.
    pub fn journal(&self, changeset_id: &str) -> Result<Vec<JournalEntry>> {
        self.journal_rows(
            &format!("SELECT {JOURNAL_COLUMNS} FROM agent_journal WHERE changeset_id = ?1 ORDER BY seq"),
            params![changeset_id],
        )
    }

    /// The newest journal entry of `rel` in watch `root` (as a new path; a rename's old path
    /// counts by its `delete`).
    pub fn journal_last(&self, changeset_id: &str, root: &str, rel: &str) -> Result<Option<JournalEntry>> {
        Ok(self
            .journal_rows(
                &format!(
                    "SELECT {JOURNAL_COLUMNS} FROM agent_journal
                     WHERE changeset_id = ?1 AND root = ?2 AND rel = ?3 ORDER BY seq DESC LIMIT 1"
                ),
                params![changeset_id, root, rel],
            )?
            .pop())
    }

    /// The snapshot row of one path, when the session's snapshot of `watch_id` holds it.
    pub fn snapshot_row(&self, changeset_id: &str, watch_id: &str, rel: &str) -> Result<Option<SnapshotRow>> {
        Ok(self
            .snapshot_rows(
                "SELECT rel, size, mtime, ctime, file_id, blake3, origin, location FROM agent_snapshots
                 WHERE changeset_id = ?1 AND watch_id = ?2 AND rel = ?3",
                params![changeset_id, watch_id, rel],
            )?
            .pop())
    }

    /// The watched folders the session `changeset_id` snapshotted at its start: the ones it
    /// journals. A folder unwatched since stays in the list.
    pub fn journal_roots(&self, changeset_id: &str) -> Result<Vec<Watch>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, root, ignores, added_at FROM agent_watches
             WHERE id IN (SELECT watch_id FROM agent_session_watches WHERE changeset_id = ?1)
             ORDER BY root",
        )?;
        let rows = stmt.query_map([changeset_id], watch_from)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn journal_rows(&self, sql: &str, args: impl rusqlite::Params) -> Result<Vec<JournalEntry>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(args, |r| {
            Ok((
                JournalEntry {
                    seq: r.get(0)?,
                    root: r.get(1)?,
                    op: r.get(2)?,
                    rel: r.get(3)?,
                    old_rel: r.get(4)?,
                    prev_blake3: r.get(5)?,
                    next_blake3: r.get(6)?,
                    origin: None,
                    location: r.get(8)?,
                    captured: r.get(9)?,
                    reason: r.get(10)?,
                    at: r.get(11)?,
                },
                r.get::<_, Option<String>>(7)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (mut e, origin) = row?;
            e.origin = origin.as_deref().map(parse_snapshot_origin).transpose()?;
            out.push(e);
        }
        Ok(out)
    }
}
