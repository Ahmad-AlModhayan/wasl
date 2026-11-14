//! The op-log (docs/adr/0002-one-op-log.md): one record of every actor's changes, which other
//! products read. This module is the only writer of `oplog_changesets` and `oplog_ops`. Each write
//! runs in one transaction that first checks the move against [`rules`] and refuses it with an
//! [`OplogError`] when the ADR doesn't allow it, so a refused write changes nothing.
//!
//! A changeset's life: [`Store::oplog_open`], [`Store::oplog_append`] while the session runs, then
//! [`Store::oplog_end`] (`staged` or `committed`) or [`Store::oplog_drop_empty`]. A staged changeset
//! is accepted or discarded with [`Store::oplog_decide`]. A revert takes back one operation at a
//! time with [`Store::oplog_revert_op`] (or records it skipped or failed with
//! [`Store::oplog_set_outcome`]), so one cut short carries on where it stopped, and ends with
//! [`Store::oplog_finish_revert`].
//!
//! The device id (`origin_device`, `person:<device id>`) comes from the caller: the store doesn't
//! hold the device key.

mod rules;

pub use rules::{
    OPLOG_HISTORY_MS, OpOutcome, OplogError, OplogStatus, check_actor, check_device, check_entity_uri,
    check_op, file_uri, file_uri_parts, folder_uri, inverse_op, revert_result,
};

use crate::{Result, Store, StoreError};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use serde_json::Value;

pub use wasl_proto::oplog::{
    Decision, ImportOp, NewChangeset, NewOp, OplogChangeset, OplogFilter, OplogKey, OplogListed, OplogOp,
};
fn corrupt(what: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(StoreError::Corrupt(what)),
    )
}

fn json(text: Option<String>) -> rusqlite::Result<Option<Value>> {
    text.map(|t| serde_json::from_str(&t).map_err(|e| corrupt(format!("op-log state {t:?}: {e}"))))
        .transpose()
}

fn status_col(r: &Row<'_>, i: usize) -> rusqlite::Result<OplogStatus> {
    let s: String = r.get(i)?;
    OplogStatus::parse(&s).ok_or_else(|| corrupt(format!("op-log status {s:?}")))
}

fn outcome_col(r: &Row<'_>, i: usize) -> rusqlite::Result<Option<OpOutcome>> {
    r.get::<_, Option<String>>(i)?
        .map(|s| OpOutcome::parse(&s).ok_or_else(|| corrupt(format!("op-log outcome {s:?}"))))
        .transpose()
}

const CS_COLUMNS: &str = "origin_device, id, actor, label, scope, status, opened_at, closed_at, \
     decided_at, decided_by, decided_actor, imported_at";

fn changeset_from_row(r: &Row<'_>) -> rusqlite::Result<OplogChangeset> {
    Ok(OplogChangeset {
        origin_device: r.get(0)?,
        id: r.get(1)?,
        actor: r.get(2)?,
        label: r.get(3)?,
        scope: r.get(4)?,
        status: status_col(r, 5)?,
        opened_at: r.get(6)?,
        closed_at: r.get(7)?,
        decided_at: r.get(8)?,
        decided_by: r.get(9)?,
        decided_actor: r.get(10)?,
        imported_at: r.get(11)?,
    })
}

const OP_COLUMNS: &str = "seq, entity_uri, op, prev, next, reverts, outcome, at";

fn op_from_row(r: &Row<'_>) -> rusqlite::Result<OplogOp> {
    Ok(OplogOp {
        seq: r.get(0)?,
        entity_uri: r.get(1)?,
        op: r.get(2)?,
        prev: json(r.get(3)?)?,
        next: json(r.get(4)?)?,
        reverts: r.get(5)?,
        outcome: outcome_col(r, 6)?,
        at: r.get(7)?,
    })
}

fn status_of(c: &Connection, k: &OplogKey) -> Result<OplogStatus> {
    c.query_row(
        "SELECT status FROM oplog_changesets WHERE origin_device = ?1 AND id = ?2",
        params![k.origin_device, k.id],
        |r| status_col(r, 0),
    )
    .optional()?
    .ok_or_else(|| OplogError::NoChangeset(k.id.clone()).into())
}

fn op_count(c: &Connection, k: &OplogKey) -> Result<u32> {
    Ok(c.query_row(
        "SELECT COUNT(*) FROM oplog_ops WHERE origin_device = ?1 AND changeset_id = ?2",
        params![k.origin_device, k.id],
        |r| r.get(0),
    )?)
}

fn op_at(c: &Connection, k: &OplogKey, seq: u32) -> Result<OplogOp> {
    c.query_row(
        &format!(
            "SELECT {OP_COLUMNS} FROM oplog_ops WHERE origin_device = ?1 AND changeset_id = ?2 AND seq = ?3"
        ),
        params![k.origin_device, k.id, seq],
        op_from_row,
    )
    .optional()?
    .ok_or_else(|| {
        OplogError::NoOperation {
            id: k.id.clone(),
            seq,
        }
        .into()
    })
}

fn insert_op(
    c: &Connection,
    k: &OplogKey,
    seq: u32,
    op: &NewOp,
    reverts: Option<u32>,
    outcome: Option<OpOutcome>,
) -> Result<()> {
    c.prepare_cached(
        "INSERT INTO oplog_ops (origin_device, changeset_id, seq, entity_uri, op, prev, next, reverts, outcome, at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )?
    .execute(params![
        k.origin_device,
        k.id,
        seq,
        op.entity_uri,
        op.op,
        op.prev.as_ref().map(Value::to_string),
        op.next.as_ref().map(Value::to_string),
        reverts,
        outcome.map(OpOutcome::as_str),
        op.at,
    ])?;
    Ok(())
}

fn check_new_op(op: &NewOp) -> std::result::Result<(), OplogError> {
    check_entity_uri(&op.entity_uri)?;
    check_op(&op.entity_uri, &op.op, op.prev.is_some(), op.next.is_some())
}

fn check_scope(scope: &str) -> std::result::Result<(), OplogError> {
    check_entity_uri(scope)?;
    if scope.starts_with("file://") && !scope.ends_with('/') {
        return Err(OplogError::BadEntityUri(scope.to_owned()));
    }
    Ok(())
}

/// Sets every undecided operation's outcome, as an accept (`applied`) or discard does.
fn decide_undecided(c: &Connection, k: &OplogKey, outcome: OpOutcome) -> Result<()> {
    c.execute(
        "UPDATE oplog_ops SET outcome = ?3 WHERE origin_device = ?1 AND changeset_id = ?2 AND outcome IS NULL",
        params![k.origin_device, k.id, outcome.as_str()],
    )?;
    Ok(())
}

fn record_decision(c: &Connection, k: &OplogKey, to: OplogStatus, d: &Decision) -> Result<()> {
    c.execute(
        "UPDATE oplog_changesets SET status = ?3, decided_at = ?4, decided_by = ?5, decided_actor = ?6
         WHERE origin_device = ?1 AND id = ?2",
        params![k.origin_device, k.id, to.as_str(), d.at, d.by, d.actor],
    )?;
    Ok(())
}

// The writes below each take a connection inside a transaction the caller holds, so the agent
// tables' writes (`agent.rs`, #47) and the op-log's commit or roll back together. The `Store`
// methods further down wrap each in a transaction of its own.

/// A changeset's status, or `None` when it is not in the op-log.
pub(crate) fn status_in(c: &Connection, k: &OplogKey) -> Result<Option<OplogStatus>> {
    Ok(c.query_row(
        "SELECT status FROM oplog_changesets WHERE origin_device = ?1 AND id = ?2",
        params![k.origin_device, k.id],
        |r| status_col(r, 0),
    )
    .optional()?)
}

pub(crate) fn open_in(c: &Connection, cs: &NewChangeset) -> Result<()> {
    check_device(&cs.key.origin_device)?;
    check_actor(&cs.actor)?;
    check_scope(&cs.scope)?;
    if status_of(c, &cs.key).is_ok() {
        return Err(OplogError::Exists(cs.key.id.clone()).into());
    }
    c.execute(
        "INSERT INTO oplog_changesets (origin_device, id, actor, label, scope, status, opened_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'open', ?6)",
        params![
            cs.key.origin_device,
            cs.key.id,
            cs.actor,
            cs.label,
            cs.scope,
            cs.opened_at
        ],
    )?;
    Ok(())
}

pub(crate) fn append_in(c: &Connection, k: &OplogKey, op: &NewOp) -> Result<u32> {
    check_new_op(op)?;
    let status = status_of(c, k)?;
    if status != OplogStatus::Open {
        return Err(OplogError::NotOpen {
            id: k.id.clone(),
            status,
        }
        .into());
    }
    let seq = op_count(c, k)? + 1;
    insert_op(c, k, seq, op, None, None)?;
    Ok(seq)
}

pub(crate) fn end_in(c: &Connection, k: &OplogKey, to: OplogStatus, at: i64) -> Result<()> {
    let status = status_of(c, k)?;
    if status != OplogStatus::Open {
        return Err(OplogError::NotOpen {
            id: k.id.clone(),
            status,
        }
        .into());
    }
    rules::check_move(status, to)?;
    if op_count(c, k)? == 0 {
        return Err(OplogError::Empty(k.id.clone()).into());
    }
    c.execute(
        "UPDATE oplog_changesets SET status = ?3, closed_at = ?4 WHERE origin_device = ?1 AND id = ?2",
        params![k.origin_device, k.id, to.as_str(), at],
    )?;
    if to == OplogStatus::Committed {
        c.execute(
            "UPDATE oplog_changesets SET decided_at = ?3 WHERE origin_device = ?1 AND id = ?2",
            params![k.origin_device, k.id, at],
        )?;
        decide_undecided(c, k, OpOutcome::Applied)?;
    }
    Ok(())
}

pub(crate) fn drop_empty_in(c: &Connection, k: &OplogKey) -> Result<()> {
    let status = status_of(c, k)?;
    if status != OplogStatus::Open {
        return Err(OplogError::NotOpen {
            id: k.id.clone(),
            status,
        }
        .into());
    }
    if op_count(c, k)? > 0 {
        return Err(OplogError::NotEmpty(k.id.clone()).into());
    }
    c.execute(
        "DELETE FROM oplog_changesets WHERE origin_device = ?1 AND id = ?2",
        params![k.origin_device, k.id],
    )?;
    Ok(())
}

pub(crate) fn decide_in(c: &Connection, k: &OplogKey, to: OplogStatus, d: &Decision) -> Result<()> {
    check_actor(&d.actor)?;
    let status = status_of(c, k)?;
    if status != OplogStatus::Staged {
        return Err(OplogError::NotStaged {
            id: k.id.clone(),
            status,
        }
        .into());
    }
    let outcome = match to {
        OplogStatus::Committed => OpOutcome::Applied,
        OplogStatus::Discarded => OpOutcome::Discarded,
        _ => return Err(OplogError::BadMove { from: status, to }.into()),
    };
    decide_undecided(c, k, outcome)?;
    record_decision(c, k, to, d)?;
    Ok(())
}

pub(crate) fn revert_op_in(c: &Connection, k: &OplogKey, seq: u32, at: i64) -> Result<u32> {
    let status = status_of(c, k)?;
    let op = op_at(c, k, seq)?;
    if op.reverts.is_some() {
        return Err(OplogError::InverseOp(seq).into());
    }
    rules::check_outcome(seq, status, op.outcome, OpOutcome::Reverted)?;
    let inverse = NewOp {
        entity_uri: op.entity_uri,
        op: inverse_op(&op.op).to_owned(),
        prev: op.next,
        next: op.prev,
        at,
    };
    let inverse_seq = op_count(c, k)? + 1;
    insert_op(c, k, inverse_seq, &inverse, Some(seq), Some(OpOutcome::Applied))?;
    c.execute(
        "UPDATE oplog_ops SET outcome = 'reverted' WHERE origin_device = ?1 AND changeset_id = ?2 AND seq = ?3",
        params![k.origin_device, k.id, seq],
    )?;
    Ok(inverse_seq)
}

pub(crate) fn set_outcome_in(c: &Connection, k: &OplogKey, seq: u32, outcome: OpOutcome) -> Result<()> {
    if outcome == OpOutcome::Reverted {
        return Err(OplogError::NeedsInverse(seq).into());
    }
    let status = status_of(c, k)?;
    let op = op_at(c, k, seq)?;
    if op.reverts.is_some() {
        return Err(OplogError::InverseOp(seq).into());
    }
    rules::check_outcome(seq, status, op.outcome, outcome)?;
    c.execute(
        "UPDATE oplog_ops SET outcome = ?4 WHERE origin_device = ?1 AND changeset_id = ?2 AND seq = ?3",
        params![k.origin_device, k.id, seq, outcome.as_str()],
    )?;
    Ok(())
}

pub(crate) fn finish_revert_in(c: &Connection, k: &OplogKey, d: &Decision) -> Result<OplogStatus> {
    check_actor(&d.actor)?;
    let status = status_of(c, k)?;
    let outcomes: Vec<Option<OpOutcome>> = {
        let mut stmt = c.prepare_cached(
            "SELECT outcome FROM oplog_ops WHERE origin_device = ?1 AND changeset_id = ?2 AND reverts IS NULL",
        )?;
        let rows = stmt.query_map(params![k.origin_device, k.id], |r| outcome_col(r, 0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let to = revert_result(outcomes);
    rules::check_move(status, to)?;
    record_decision(c, k, to, d)?;
    Ok(to)
}

pub(crate) fn import_in(
    c: &Connection,
    cs: &OplogChangeset,
    ops: &[ImportOp],
    imported_at: i64,
) -> Result<()> {
    check_device(&cs.origin_device)?;
    check_actor(&cs.actor)?;
    check_scope(&cs.scope)?;
    if let Some(a) = &cs.decided_actor {
        check_actor(a)?;
    }
    for (i, o) in ops.iter().enumerate() {
        check_new_op(&o.op)?;
        let seq = i as u32 + 1;
        if let Some(r) = o.reverts.filter(|&r| r == 0 || r >= seq) {
            return Err(OplogError::BadReverts { seq, reverts: r }.into());
        }
    }
    let k = cs.key();
    if status_of(c, &k).is_ok() {
        return Err(OplogError::Exists(cs.id.clone()).into());
    }
    c.execute(
        &format!("INSERT INTO oplog_changesets ({CS_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"),
        params![
            cs.origin_device,
            cs.id,
            cs.actor,
            cs.label,
            cs.scope,
            cs.status.as_str(),
            cs.opened_at,
            cs.closed_at,
            cs.decided_at,
            cs.decided_by,
            cs.decided_actor,
            imported_at,
        ],
    )?;
    for (i, o) in ops.iter().enumerate() {
        insert_op(c, &k, i as u32 + 1, &o.op, o.reverts, o.outcome)?;
    }
    Ok(())
}

impl Store {
    /// A write transaction that takes the write lock at once, so the check and the write see the
    /// same rows.
    pub(crate) fn oplog_tx(&self) -> Result<Transaction<'_>> {
        Ok(Transaction::new_unchecked(
            &self.conn,
            TransactionBehavior::Immediate,
        )?)
    }

    /// Opens a changeset when a session begins (new → `open`).
    pub fn oplog_open(&mut self, cs: &NewChangeset) -> Result<()> {
        let tx = self.oplog_tx()?;
        open_in(&tx, cs)?;
        tx.commit()?;
        Ok(())
    }

    /// Appends an operation to an open changeset. Returns its `seq`.
    pub fn oplog_append(&mut self, k: &OplogKey, op: &NewOp) -> Result<u32> {
        let tx = self.oplog_tx()?;
        let seq = append_in(&tx, k, op)?;
        tx.commit()?;
        Ok(seq)
    }

    /// Ends a session that changed something: `open` → `staged` (changes to hold) or `committed`
    /// (nothing to hold; its operations become `applied`, and it counts as decided at `at` for
    /// pruning). A session that changed nothing is dropped instead ([`Store::oplog_drop_empty`]).
    pub fn oplog_end(&mut self, k: &OplogKey, to: OplogStatus, at: i64) -> Result<()> {
        let tx = self.oplog_tx()?;
        end_in(&tx, k, to, at)?;
        tx.commit()?;
        Ok(())
    }

    /// Drops an open changeset with no operations (`open` → deleted): an empty session leaves no
    /// record.
    pub fn oplog_drop_empty(&mut self, k: &OplogKey) -> Result<()> {
        let tx = self.oplog_tx()?;
        drop_empty_in(&tx, k)?;
        tx.commit()?;
        Ok(())
    }

    /// Accepts (`committed`) or discards (`discarded`) a staged changeset. Undecided operations
    /// become `applied` or `discarded`; an operation already `discarded` (left out of a partial
    /// accept) stays so.
    pub fn oplog_decide(&mut self, k: &OplogKey, to: OplogStatus, d: &Decision) -> Result<()> {
        let tx = self.oplog_tx()?;
        decide_in(&tx, k, to, d)?;
        tx.commit()?;
        Ok(())
    }

    /// Takes back operation `seq`: appends its inverse (`reverts` = `seq`, `prev` and `next`
    /// swapped, `create` ↔ `delete`, outcome `applied`) and marks it `reverted`, in one
    /// transaction. Returns the inverse operation's `seq`.
    pub fn oplog_revert_op(&mut self, k: &OplogKey, seq: u32, at: i64) -> Result<u32> {
        let tx = self.oplog_tx()?;
        let inverse_seq = revert_op_in(&tx, k, seq, at)?;
        tx.commit()?;
        Ok(inverse_seq)
    }

    /// Decides one operation's outcome (`skipped_changed` or `failed` in a revert, `discarded` in a
    /// partial accept). `reverted` is set only by [`Store::oplog_revert_op`].
    pub fn oplog_set_outcome(&mut self, k: &OplogKey, seq: u32, outcome: OpOutcome) -> Result<()> {
        let tx = self.oplog_tx()?;
        set_outcome_in(&tx, k, seq, outcome)?;
        tx.commit()?;
        Ok(())
    }

    /// Ends a revert: `reverted` when every operation that took effect is taken back, else
    /// `partly_reverted` (also from `partly_reverted`: a later revert that still leaves some).
    /// Records the decision and returns the status it set.
    pub fn oplog_finish_revert(&mut self, k: &OplogKey, d: &Decision) -> Result<OplogStatus> {
        let tx = self.oplog_tx()?;
        let to = finish_revert_in(&tx, k, d)?;
        tx.commit()?;
        Ok(to)
    }

    /// The one-time copy of a v10 changeset (ADR-0002): writes it with its status, decision and
    /// outcomes as they are, numbering `ops` from 1, and sets `imported_at` (`cs.imported_at` is not
    /// read). The only write that skips the move and outcome checks; actors, URIs and operation
    /// names are still checked.
    pub fn oplog_import(&mut self, cs: &OplogChangeset, ops: &[ImportOp], imported_at: i64) -> Result<()> {
        let tx = self.oplog_tx()?;
        import_in(&tx, cs, ops, imported_at)?;
        tx.commit()?;
        Ok(())
    }
    /// Deletes changesets decided more than [`OPLOG_HISTORY_MS`] before `now`, with their
    /// operations; `open` and `staged` ones stay. Returns how many went.
    pub fn oplog_prune(&mut self, now: i64) -> Result<usize> {
        let tx = self.oplog_tx()?;
        let n = tx.execute(
            "DELETE FROM oplog_changesets
             WHERE status IN ('committed', 'discarded', 'reverted', 'partly_reverted') AND decided_at < ?1",
            [now.saturating_sub(OPLOG_HISTORY_MS)],
        )?;
        tx.commit()?;
        Ok(n)
    }

    pub fn oplog_changeset(&self, k: &OplogKey) -> Result<Option<OplogChangeset>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {CS_COLUMNS} FROM oplog_changesets WHERE origin_device = ?1 AND id = ?2"),
                params![k.origin_device, k.id],
                changeset_from_row,
            )
            .optional()?)
    }

    /// A changeset's operations in `seq` order.
    pub fn oplog_ops(&self, k: &OplogKey) -> Result<Vec<OplogOp>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {OP_COLUMNS} FROM oplog_ops WHERE origin_device = ?1 AND changeset_id = ?2 ORDER BY seq"
        ))?;
        let rows = stmt.query_map(params![k.origin_device, k.id], op_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The changesets `f` matches, newest first and at most `f.limit`, each with its operation count.
    pub fn oplog_list(&self, f: &OplogFilter) -> Result<Vec<OplogListed>> {
        let pair = f.pair.as_deref().map(|p| folder_uri(p, ""));
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {CS_COLUMNS}, (SELECT COUNT(*) FROM oplog_ops o
                 WHERE o.origin_device = c.origin_device AND o.changeset_id = c.id)
             FROM oplog_changesets c
             WHERE (?1 IS NULL OR actor = ?1) AND (?2 IS NULL OR status = ?2)
               AND (?3 IS NULL OR opened_at >= ?3)
               AND (?4 IS NULL OR substr(scope, 1, length(?4)) = ?4)
             ORDER BY opened_at DESC, origin_device, id DESC
             LIMIT ?5"
        ))?;
        // SQLite reads a negative LIMIT as none.
        let limit = f.limit.map_or(-1, i64::from);
        let rows = stmt.query_map(
            params![f.actor, f.status.map(OplogStatus::as_str), f.since, pair, limit],
            |r| {
                Ok(OplogListed {
                    changeset: changeset_from_row(r)?,
                    op_count: r.get(12)?,
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}
