//! Agent changes (docs/AGENT_CHANGES.md): changesets, the begin list of each session, the earlier
//! versions saved for it and what it changed.
//!
//! These tables are the working detail; the op-log is the record other products read (ADR-0002,
//! SRS FR-OL7). Once the engine has set this device's id ([`Store::set_oplog_device`]), every
//! write here that changes a changeset's status or an entry's outcome writes the op-log in the
//! same transaction: a session opens an `open` changeset, its end appends one operation per entry
//! on `file://<pair id>/<key>` and stages it (`held`), and accept and revert decide it. Changesets
//! that are not in the op-log (from before v11, or made with no device id) are copied in once by
//! [`Store::copy_changesets_to_oplog`].

use crate::oplog::{
    self, Decision, ImportOp, NewChangeset, NewOp, OpOutcome, OplogChangeset, OplogError, OplogKey,
    OplogStatus, file_uri, folder_uri, inverse_op,
};
use crate::{Result, SIDE_LOCAL, Store, StoreError, to_i64, to_u64};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde_json::{Value, json};
use std::collections::HashMap;

/// Where a changeset is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangesetStatus {
    /// The agent is still working.
    Open,
    /// Ended; its files wait for the user and are not synced.
    Held,
    Accepted,
    Reverted,
    /// Reverted except for files the user changed after the session (or that failed).
    PartlyReverted,
}

impl ChangesetStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ChangesetStatus::Open => "open",
            ChangesetStatus::Held => "held",
            ChangesetStatus::Accepted => "accepted",
            ChangesetStatus::Reverted => "reverted",
            ChangesetStatus::PartlyReverted => "partly_reverted",
        }
    }

    fn parse(s: &str) -> rusqlite::Result<ChangesetStatus> {
        Ok(match s {
            "open" => ChangesetStatus::Open,
            "held" => ChangesetStatus::Held,
            "accepted" => ChangesetStatus::Accepted,
            "reverted" => ChangesetStatus::Reverted,
            "partly_reverted" => ChangesetStatus::PartlyReverted,
            other => return Err(corrupt(format!("changeset status {other:?}"))),
        })
    }

    /// Open and held changesets keep their files out of sync.
    pub fn holds(self) -> bool {
        matches!(self, ChangesetStatus::Open | ChangesetStatus::Held)
    }

    /// The op-log status this is recorded as (ADR-0002): `held` is `staged`, `accepted` is
    /// `committed`, the others keep their names.
    pub fn in_oplog(self) -> OplogStatus {
        match self {
            ChangesetStatus::Open => OplogStatus::Open,
            ChangesetStatus::Held => OplogStatus::Staged,
            ChangesetStatus::Accepted => OplogStatus::Committed,
            ChangesetStatus::Reverted => OplogStatus::Reverted,
            ChangesetStatus::PartlyReverted => OplogStatus::PartlyReverted,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Changeset {
    pub id: String,
    /// The pair the session works in; `None` for a session in a watched folder (#57).
    pub pair_id: Option<String>,
    /// `agent:<name>`.
    pub actor: String,
    pub label: Option<String>,
    /// Pair-relative folder the agent works in; empty for the whole pair.
    pub scope: String,
    /// The agent's own session id (Claude Code's `session_id`), when it has one.
    pub session: Option<String>,
    pub status: ChangesetStatus,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub decided_at: Option<i64>,
}

impl Changeset {
    /// The pair a pair session works in; refused for a session in a watched folder.
    pub fn pair(&self) -> Result<&str> {
        self.pair_id.as_deref().ok_or_else(|| in_watch(&self.id))
    }
}

/// One item of the scope when the session began (local relative path).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ManifestRow {
    pub rel: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: i64,
    /// In this state on Drive or the other devices when the session began.
    pub synced: bool,
}

/// An earlier version copied to the agent folder.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SavedCopy {
    pub rel: String,
    /// File name under the changeset's folder.
    pub location: String,
    pub size: u64,
    pub mtime: i64,
    /// BLAKE3 of the copy (hex), taken while copying; `None` for copies saved before schema v9.
    #[serde(default)]
    pub blake3: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Added,
    Changed,
    Deleted,
}

impl EntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EntryKind::Added => "added",
            EntryKind::Changed => "changed",
            EntryKind::Deleted => "deleted",
        }
    }
}

/// Where an entry's earlier version is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EarlierVersion {
    /// Copied to the agent folder (`location`).
    Saved,
    /// The synced version, which Drive or the other devices still hold.
    Synced,
    /// None: the file is new.
    None,
}

impl EarlierVersion {
    pub fn as_str(self) -> &'static str {
        match self {
            EarlierVersion::Saved => "saved",
            EarlierVersion::Synced => "synced",
            EarlierVersion::None => "none",
        }
    }
}

/// One file or folder a session added, changed or deleted.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentEntry {
    /// Local relative path.
    pub rel: String,
    /// Canonical key (Drive names).
    pub key: String,
    pub kind: EntryKind,
    pub is_dir: bool,
    pub before_size: Option<u64>,
    pub before_mtime: Option<i64>,
    pub after_size: Option<u64>,
    pub after_mtime: Option<i64>,
    pub source: EarlierVersion,
    pub location: Option<String>,
    /// Set by a revert: `reverted`, `skipped_changed` or `failed: <why>`.
    pub outcome: Option<String>,
}

fn corrupt(what: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(StoreError::Corrupt(what)),
    )
}

const CS_COLUMNS: &str =
    "id, pair_id, actor, label, scope, session, status, started_at, ended_at, decided_at";

fn changeset_from_row(r: &Row<'_>) -> rusqlite::Result<Changeset> {
    Ok(Changeset {
        id: r.get(0)?,
        pair_id: r.get(1)?,
        actor: r.get(2)?,
        label: r.get(3)?,
        scope: r.get(4)?,
        session: r.get(5)?,
        status: ChangesetStatus::parse(&r.get::<_, String>(6)?)?,
        started_at: r.get(7)?,
        ended_at: r.get(8)?,
        decided_at: r.get(9)?,
    })
}

fn saved_from_row(r: &Row<'_>) -> rusqlite::Result<SavedCopy> {
    Ok(SavedCopy {
        rel: r.get(0)?,
        location: r.get(1)?,
        size: to_u64(r.get(2)?),
        mtime: r.get(3)?,
        blake3: r.get(4)?,
    })
}

const ENTRY_COLUMNS: &str =
    "rel, key, kind, is_dir, before_size, before_mtime, after_size, after_mtime, source,
    location, outcome";

fn entry_from_row(r: &Row<'_>) -> rusqlite::Result<AgentEntry> {
    let kind = match r.get::<_, String>(2)?.as_str() {
        "added" => EntryKind::Added,
        "changed" => EntryKind::Changed,
        "deleted" => EntryKind::Deleted,
        other => return Err(corrupt(format!("entry kind {other:?}"))),
    };
    let source = match r.get::<_, String>(8)?.as_str() {
        "saved" => EarlierVersion::Saved,
        "synced" => EarlierVersion::Synced,
        "none" => EarlierVersion::None,
        other => return Err(corrupt(format!("earlier version {other:?}"))),
    };
    Ok(AgentEntry {
        rel: r.get(0)?,
        key: r.get(1)?,
        kind,
        is_dir: r.get(3)?,
        before_size: r.get::<_, Option<i64>>(4)?.map(to_u64),
        before_mtime: r.get(5)?,
        after_size: r.get::<_, Option<i64>>(6)?.map(to_u64),
        after_mtime: r.get(7)?,
        source,
        location: r.get(9)?,
        outcome: r.get(10)?,
    })
}

// ---------------------------------------------------------------- the op-log record (#47)

/// The op-log name of changeset `id` and its op-log status, when the op-log is written (a device
/// id is set) and holds it.
pub(crate) fn oplog_record(
    c: &Connection,
    device: Option<&str>,
    id: &str,
) -> Result<Option<(OplogKey, OplogStatus)>> {
    let Some(device) = device else {
        return Ok(None);
    };
    let k = OplogKey::new(device, id);
    Ok(oplog::status_in(c, &k)?.map(|s| (k, s)))
}

/// The key of a pair's local path: the baseline's, else the path itself (as the engine names
/// entries it has no baseline for).
fn key_of(c: &Connection, pair_id: &str, rel: &str) -> Result<String> {
    if rel.is_empty() {
        return Ok(String::new());
    }
    Ok(c.query_row(
        "SELECT key FROM file_state WHERE pair_id = ?1 AND side = ?2 AND COALESCE(local_name, key) = ?3
         LIMIT 1",
        params![pair_id, SIDE_LOCAL, rel],
        |r| r.get(0),
    )
    .optional()?
    .unwrap_or_else(|| rel.to_owned()))
}

fn pair_of(c: &Connection, changeset_id: &str) -> Result<String> {
    let pair: Option<String> = c.query_row(
        "SELECT pair_id FROM agent_changesets WHERE id = ?1",
        [changeset_id],
        |r| r.get(0),
    )?;
    pair.ok_or_else(|| in_watch(changeset_id))
}

/// The refusal for a pair-only step asked of a session in a watched folder.
fn in_watch(changeset_id: &str) -> StoreError {
    StoreError::Refused(format!(
        "session {changeset_id} works in a watched folder, not a pair"
    ))
}

/// The BLAKE3 of each saved earlier version of a changeset, by path.
fn saved_hashes(c: &Connection, changeset_id: &str) -> Result<HashMap<String, String>> {
    let mut stmt = c.prepare_cached(
        "SELECT rel, blake3 FROM agent_saved WHERE changeset_id = ?1 AND blake3 IS NOT NULL",
    )?;
    let rows = stmt.query_map([changeset_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// An entry as an op-log operation (ADR-0002): `create`, `update` or `delete` of
/// `file://<pair id>/<key>`, with the state before and after (`{"kind": "file", "size",
/// "mtime_ms", "blake3"}`, BLAKE3 only where Wasl knows it: the saved earlier version's; a folder
/// `{"kind": "dir"}`).
fn entry_op(pair_id: &str, e: &AgentEntry, saved: &HashMap<String, String>, at: i64) -> NewOp {
    let state = |size: Option<u64>, mtime: Option<i64>, blake3: Option<&String>| match size {
        Some(size) => {
            let mut v = json!({"kind": "file", "size": size});
            if let Some(m) = mtime {
                v["mtime_ms"] = json!(m);
            }
            if let Some(h) = blake3 {
                v["blake3"] = json!(h);
            }
            v
        }
        None => json!({"kind": "dir"}),
    };
    let before_hash = if e.source == EarlierVersion::Saved {
        saved.get(&e.rel)
    } else {
        None
    };
    let prev: Option<Value> =
        (e.kind != EntryKind::Added).then(|| state(e.before_size, e.before_mtime, before_hash));
    let next: Option<Value> =
        (e.kind != EntryKind::Deleted).then(|| state(e.after_size, e.after_mtime, None));
    NewOp {
        entity_uri: file_uri(pair_id, &e.key),
        op: match e.kind {
            EntryKind::Added => "create",
            EntryKind::Changed => "update",
            EntryKind::Deleted => "delete",
        }
        .to_owned(),
        prev,
        next,
        at,
    }
}

/// An entry outcome (`reverted`, `skipped_changed`, `failed: <why>`) as an op-log outcome.
fn op_outcome(outcome: &str) -> Option<OpOutcome> {
    match outcome {
        "reverted" => Some(OpOutcome::Reverted),
        "skipped_changed" => Some(OpOutcome::SkippedChanged),
        o if o == "failed" || o.starts_with("failed:") => Some(OpOutcome::Failed),
        _ => None,
    }
}

/// Writes an entry's outcome to its operation: `reverted` appends the inverse operation (once),
/// `skipped_changed` and `failed` decide it.
fn record_outcome(c: &Connection, k: &OplogKey, rel: &str, outcome: &str, at: i64) -> Result<()> {
    let Some((pair_id, key)) = c
        .query_row(
            "SELECT c.pair_id, e.key FROM agent_entries e JOIN agent_changesets c ON c.id = e.changeset_id
             WHERE e.changeset_id = ?1 AND e.rel = ?2",
            params![k.id, rel],
            |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()?
    else {
        return Ok(());
    };
    let pair_id = pair_id.ok_or_else(|| in_watch(&k.id))?;
    let to = op_outcome(outcome).ok_or_else(|| {
        StoreError::Refused(format!(
            "entry outcome {outcome:?}: expected reverted, skipped_changed or failed: <why>"
        ))
    })?;
    let uri = file_uri(&pair_id, &key);
    let (seq, current) = c
        .query_row(
            "SELECT seq, outcome FROM oplog_ops
             WHERE origin_device = ?1 AND changeset_id = ?2 AND entity_uri = ?3 AND reverts IS NULL
             ORDER BY seq LIMIT 1",
            params![k.origin_device, k.id, uri],
            |r| Ok((r.get::<_, u32>(0)?, r.get::<_, Option<String>>(1)?)),
        )
        .optional()?
        .ok_or_else(|| StoreError::Corrupt(format!("op-log changeset {} has no operation on {uri}", k.id)))?;
    match to {
        OpOutcome::Reverted if current.as_deref() == Some(OpOutcome::Reverted.as_str()) => {}
        OpOutcome::Reverted => {
            oplog::revert_op_in(c, k, seq, at)?;
        }
        other => oplog::set_outcome_in(c, k, seq, other)?,
    }
    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

impl Store {
    /// Records a new open changeset and its begin list.
    pub fn create_changeset(&mut self, cs: &Changeset, manifest: &[ManifestRow]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            &format!(
                "INSERT INTO agent_changesets ({CS_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"
            ),
            params![
                cs.id,
                cs.pair_id,
                cs.actor,
                cs.label,
                cs.scope,
                cs.session,
                cs.status.as_str(),
                cs.started_at,
                cs.ended_at,
                cs.decided_at,
            ],
        )?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO agent_manifest (changeset_id, rel, is_dir, size, mtime, synced) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for m in manifest {
                stmt.execute(params![cs.id, m.rel, m.is_dir, to_i64(m.size), m.mtime, m.synced])?;
            }
        }
        // Readers see an agent at work from the start (ADR-0002).
        // A session in a watched folder has no pair to name its scope by: its journal (#57) is its
        // record.
        if let Some(device) = self.oplog_device.as_deref()
            && cs.status == ChangesetStatus::Open
            && let Some(pair) = cs.pair_id.as_deref()
        {
            let scope = folder_uri(pair, &key_of(&tx, pair, &cs.scope)?);
            oplog::open_in(
                &tx,
                &NewChangeset {
                    key: OplogKey::new(device, &cs.id),
                    actor: cs.actor.clone(),
                    label: cs.label.clone(),
                    scope,
                    opened_at: cs.started_at,
                },
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn changeset(&self, id: &str) -> Result<Option<Changeset>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {CS_COLUMNS} FROM agent_changesets WHERE id = ?1"),
                [id],
                changeset_from_row,
            )
            .optional()?)
    }

    /// Changesets, newest first: for one pair or all; only open and held ones unless `all`.
    pub fn changesets(&self, pair_id: Option<&str>, all: bool) -> Result<Vec<Changeset>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {CS_COLUMNS} FROM agent_changesets
             WHERE (?1 IS NULL OR pair_id = ?1) AND (?2 OR status IN ('open', 'held'))
             ORDER BY started_at DESC, id DESC"
        ))?;
        let rows = stmt.query_map(params![pair_id, all], changeset_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The open changeset of an agent session, if any.
    pub fn open_changeset_for_session(&self, session: &str) -> Result<Option<Changeset>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {CS_COLUMNS} FROM agent_changesets WHERE session = ?1 AND status = 'open'
                     ORDER BY started_at DESC LIMIT 1"
                ),
                [session],
                changeset_from_row,
            )
            .optional()?)
    }

    pub fn manifest(&self, changeset_id: &str) -> Result<Vec<ManifestRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT rel, is_dir, size, mtime, synced FROM agent_manifest WHERE changeset_id = ?1",
        )?;
        let rows = stmt.query_map([changeset_id], |r| {
            Ok(ManifestRow {
                rel: r.get(0)?,
                is_dir: r.get(1)?,
                size: to_u64(r.get(2)?),
                mtime: r.get(3)?,
                synced: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Records a saved earlier version. The first copy of a path wins: it is the one from before
    /// the agent touched the file. Returns whether it was recorded.
    pub fn put_saved(&self, changeset_id: &str, s: &SavedCopy) -> Result<bool> {
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO agent_saved (changeset_id, rel, location, size, mtime, blake3) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![changeset_id, s.rel, s.location, to_i64(s.size), s.mtime, s.blake3],
        )?;
        Ok(n == 1)
    }

    pub fn saved(&self, changeset_id: &str) -> Result<Vec<SavedCopy>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT rel, location, size, mtime, blake3 FROM agent_saved WHERE changeset_id = ?1",
        )?;
        let rows = stmt.query_map([changeset_id], saved_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The saved earlier version of one path, kept until the changeset is decided.
    pub fn saved_copy(&self, changeset_id: &str, rel: &str) -> Result<Option<SavedCopy>> {
        Ok(self
            .conn
            .query_row(
                "SELECT rel, location, size, mtime, blake3 FROM agent_saved WHERE changeset_id = ?1 AND rel = ?2",
                params![changeset_id, rel],
                saved_from_row,
            )
            .optional()?)
    }

    pub fn has_saved(&self, changeset_id: &str, rel: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM agent_saved WHERE changeset_id = ?1 AND rel = ?2",
                params![changeset_id, rel],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Ends a session: records what it changed, drops its begin list and sets `status` (held, or
    /// gone when it changed nothing: then the changeset is deleted). In the op-log: one operation
    /// per entry, then `staged`; or the empty changeset dropped. A session that changed only
    /// watched folders (its journal, #57) holds nothing: `accepted`, and `committed` in the op-log.
    pub fn end_changeset(&mut self, id: &str, entries: &[AgentEntry], ended_at: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        let recorded = oplog_record(&tx, self.oplog_device.as_deref(), id)?;
        let journaled: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM agent_journal WHERE changeset_id = ?1)",
            [id],
            |r| r.get(0),
        )?;
        if entries.is_empty() && journaled {
            // It changed only watched folders, which hold nothing (UND-9): closed, committed.
            if let Some((k, _)) = &recorded {
                oplog::end_in(&tx, k, OplogStatus::Committed, ended_at)?;
            }
            tx.execute("DELETE FROM agent_manifest WHERE changeset_id = ?1", [id])?;
            tx.execute(
                "UPDATE agent_changesets SET status = 'accepted', ended_at = ?2, decided_at = ?2 WHERE id = ?1",
                params![id, ended_at],
            )?;
        } else if entries.is_empty() {
            tx.execute("DELETE FROM agent_changesets WHERE id = ?1", [id])?;
            if let Some((k, _)) = &recorded {
                oplog::drop_empty_in(&tx, k)?;
            }
        } else {
            if let Some((k, _)) = &recorded {
                let pair_id = pair_of(&tx, id)?;
                let saved = saved_hashes(&tx, id)?;
                for e in entries {
                    oplog::append_in(&tx, k, &entry_op(&pair_id, e, &saved, ended_at))?;
                }
                oplog::end_in(&tx, k, OplogStatus::Staged, ended_at)?;
            }
            {
                let mut stmt = tx.prepare_cached(&format!(
                    "INSERT INTO agent_entries (changeset_id, {ENTRY_COLUMNS})
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
                ))?;
                for e in entries {
                    stmt.execute(params![
                        id,
                        e.rel,
                        e.key,
                        e.kind.as_str(),
                        e.is_dir,
                        e.before_size.map(to_i64),
                        e.before_mtime,
                        e.after_size.map(to_i64),
                        e.after_mtime,
                        e.source.as_str(),
                        e.location,
                        e.outcome,
                    ])?;
                }
            }
            tx.execute("DELETE FROM agent_manifest WHERE changeset_id = ?1", [id])?;
            tx.execute(
                "UPDATE agent_changesets SET status = 'held', ended_at = ?2 WHERE id = ?1",
                params![id, ended_at],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Which client decided a changeset, when it was recorded.
    pub fn changeset_decided_by(&self, id: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT decided_by FROM agent_changesets WHERE id = ?1",
                [id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }

    /// Deletes changesets accepted or reverted before `before`. Returns how many went.
    pub fn prune_changesets(&self, before: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM agent_changesets WHERE status NOT IN ('open', 'held') AND decided_at < ?1",
            [before],
        )?)
    }

    /// Entries, deepest paths first (the order a revert works in).
    pub fn agent_entries(&self, changeset_id: &str) -> Result<Vec<AgentEntry>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {ENTRY_COLUMNS} FROM agent_entries WHERE changeset_id = ?1 ORDER BY rel DESC"
        ))?;
        let rows = stmt.query_map([changeset_id], entry_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// [`Store::set_entry_outcome_at`] at the system clock's time.
    pub fn set_entry_outcome(&self, changeset_id: &str, rel: &str, outcome: &str) -> Result<()> {
        self.set_entry_outcome_at(changeset_id, rel, outcome, now_ms())
    }

    /// Records what a revert did to one entry (`reverted`, `skipped_changed`, `failed: <why>`). In
    /// the op-log, in the same transaction: `reverted` appends the inverse operation (written at
    /// `at`) and marks the entry's operation reverted; the others decide its outcome. So a revert
    /// cut short leaves both records at the same entry.
    pub fn set_entry_outcome_at(&self, changeset_id: &str, rel: &str, outcome: &str, at: i64) -> Result<()> {
        let tx = self.oplog_tx()?;
        tx.execute(
            "UPDATE agent_entries SET outcome = ?3 WHERE changeset_id = ?1 AND rel = ?2",
            params![changeset_id, rel, outcome],
        )?;
        if let Some((k, _)) = oplog_record(&tx, self.oplog_device.as_deref(), changeset_id)? {
            record_outcome(&tx, &k, rel, outcome, at)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn decide_changeset(&self, id: &str, status: ChangesetStatus, at: i64) -> Result<()> {
        self.decide_changeset_by(id, status, at, None)
    }

    /// Decides a changeset and records which client decided (see [`Store::changeset_decided_by`]).
    /// In the op-log, in the same transaction: accepted commits it; reverted or partly reverted
    /// ends its revert, which must come to the same status from the operations' outcomes. The
    /// decider is `person:<device id>`: Wasl doesn't know which agent, if any, asked for a revert.
    pub fn decide_changeset_by(
        &self,
        id: &str,
        status: ChangesetStatus,
        at: i64,
        by: Option<&str>,
    ) -> Result<()> {
        let tx = self.oplog_tx()?;
        tx.execute(
            "UPDATE agent_changesets SET status = ?2, decided_at = ?3, decided_by = ?4 WHERE id = ?1",
            params![id, status.as_str(), at, by],
        )?;
        tx.execute("DELETE FROM agent_saved WHERE changeset_id = ?1", [id])?;
        if let Some((k, from)) = oplog_record(&tx, self.oplog_device.as_deref(), id)? {
            let d = Decision {
                by: by.unwrap_or("unknown").to_owned(),
                actor: format!("person:{}", k.origin_device),
                at,
            };
            let to = status.in_oplog();
            match status {
                ChangesetStatus::Accepted => oplog::decide_in(&tx, &k, to, &d)?,
                ChangesetStatus::Reverted | ChangesetStatus::PartlyReverted => {
                    let found = oplog::finish_revert_in(&tx, &k, &d)?;
                    if found != to {
                        return Err(StoreError::Corrupt(format!(
                            "changeset {id} is {} but its op-log operations make it {found}",
                            status.as_str()
                        )));
                    }
                }
                ChangesetStatus::Open | ChangesetStatus::Held => {
                    return Err(OplogError::BadMove { from, to }.into());
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The one-time copy into the op-log (ADR-0002, FR-OL7): every agent changeset not yet in it
    /// (those from before schema v11, and any made while no device id was set) is imported as it
    /// is, with its status, decision and entry outcomes, one operation per entry and an inverse
    /// operation for each reverted entry, marked `imported_at` = `now`. An open one comes in open
    /// and empty; its operations follow when its session ends, as for a new one. Each changeset is
    /// one transaction. Does nothing without a device id, and nothing the second time. Returns
    /// how many were copied.
    pub fn copy_changesets_to_oplog(&mut self, now: i64) -> Result<usize> {
        let Some(device) = self.oplog_device.clone() else {
            return Ok(0);
        };
        let ids: Vec<String> = {
            let mut stmt = self.conn.prepare(
                "SELECT id FROM agent_changesets
                 WHERE id NOT IN (SELECT id FROM oplog_changesets WHERE origin_device = ?1)
                 ORDER BY started_at, id",
            )?;
            let rows = stmt.query_map([device.as_str()], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut copied = 0;
        for id in &ids {
            let tx = self.oplog_tx()?;
            let Some(cs) = self.changeset(id)? else {
                continue;
            };
            // Sessions in watched folders came after the op-log; #57 records them as they run.
            let Some(pair) = cs.pair_id.clone() else {
                continue;
            };
            let mut entries = self.agent_entries(id)?;
            entries.sort_by(|a, b| a.rel.cmp(&b.rel));
            let saved = saved_hashes(&tx, id)?;
            let decided_by = self.changeset_decided_by(id)?;
            let status = cs.status.in_oplog();
            let decided = cs.decided_at.is_some();
            let record = OplogChangeset {
                origin_device: device.clone(),
                id: cs.id.clone(),
                actor: cs.actor.clone(),
                label: cs.label.clone(),
                scope: folder_uri(&pair, &key_of(&tx, &pair, &cs.scope)?),
                status,
                opened_at: cs.started_at,
                closed_at: cs.ended_at,
                decided_at: cs.decided_at,
                decided_by: decided.then(|| decided_by.unwrap_or_else(|| "unknown".to_owned())),
                decided_actor: decided.then(|| format!("person:{device}")),
                imported_at: Some(now),
            };
            let at = cs.ended_at.unwrap_or(cs.started_at);
            let mut ops = Vec::with_capacity(entries.len());
            let mut taken_back = Vec::new();
            for (i, e) in entries.iter().enumerate() {
                let op = entry_op(&pair, e, &saved, at);
                let outcome = match e.outcome.as_deref().and_then(op_outcome) {
                    Some(o) => Some(o),
                    // Still waiting: undecided.
                    None if matches!(status, OplogStatus::Open | OplogStatus::Staged) => None,
                    // Accepted, or a revert that left it: it stands.
                    None => Some(OpOutcome::Applied),
                };
                if outcome == Some(OpOutcome::Reverted) {
                    taken_back.push((i as u32 + 1, op.clone()));
                }
                ops.push(ImportOp {
                    op,
                    reverts: None,
                    outcome,
                });
            }
            let reverted_at = cs.decided_at.unwrap_or(at);
            for (seq, op) in taken_back {
                ops.push(ImportOp {
                    op: NewOp {
                        op: inverse_op(&op.op).to_owned(),
                        entity_uri: op.entity_uri,
                        prev: op.next,
                        next: op.prev,
                        at: reverted_at,
                    },
                    reverts: Some(seq),
                    outcome: Some(OpOutcome::Applied),
                });
            }
            oplog::import_in(&tx, &record, &ops, now)?;
            tx.commit()?;
            copied += 1;
        }
        Ok(copied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pair_only_step_on_a_watched_folder_session_is_refused_not_an_sql_error() {
        let mut s = Store::open_in_memory().unwrap();
        s.add_watch("w1", "/home/me/site", 1).unwrap();
        let c = s.connection();
        c.execute_batch(
            "INSERT INTO agent_changesets (id, watch_id, actor, scope, status, started_at)
                 VALUES ('cs', 'w1', 'agent:x', '', 'open', 1);
             INSERT INTO agent_entries (changeset_id, rel, key, kind, is_dir, source)
                 VALUES ('cs', 'a.txt', 'a.txt', 'changed', 0, 'none');",
        )
        .unwrap();
        assert!(matches!(pair_of(c, "cs"), Err(StoreError::Refused(_))));
        let k = OplogKey::new("dev", "cs");
        assert!(matches!(
            record_outcome(c, &k, "a.txt", "reverted", 2),
            Err(StoreError::Refused(_))
        ));
    }
}
