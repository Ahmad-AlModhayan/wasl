//! Run history (kept 90 days, SRS FR-B6) and conflicts.

use crate::{Result, Store, to_i64, to_u64};
use rusqlite::{OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use wasl_core::{ConflictPolicy, RunStatus};

/// Counters of one run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct RunStats {
    pub uploaded: u32,
    pub downloaded: u32,
    pub exported: u32,
    pub deleted_local: u32,
    pub deleted_remote: u32,
    pub conflicts: u32,
    pub skipped: u32,
    pub errors: u32,
    pub bytes_up: u64,
    pub bytes_down: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct RunRecord {
    pub id: String,
    pub pair_id: String,
    pub trigger: String,
    pub status: RunStatus,
    pub stats: RunStats,
    pub message: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum EventLevel {
    Info,
    Warn,
    Error,
}

impl EventLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            EventLevel::Info => "INFO",
            EventLevel::Warn => "WARN",
            EventLevel::Error => "ERROR",
        }
    }
}

/// One line of a run's log.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct RunEvent {
    pub at: i64,
    pub level: EventLevel,
    /// `UPLOAD`, `DOWNLOAD`, `EXPORT`, `DELETE_LOCAL`, `DELETE_REMOTE`, `CONFLICT`, `MKDIR`, `SKIP`,
    /// `ERROR`, `BLOCKED`, `INFO`, as in 1.1.4.
    pub action: String,
    pub key: Option<String>,
    pub message: String,
    pub detail: Option<String>,
    pub bytes: u64,
}

/// A file changed on both sides (SRS FR-F4).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct ConflictRecord {
    pub id: String,
    pub pair_id: String,
    pub key: String,
    pub detected_at: i64,
    pub policy: ConflictPolicy,
    /// The renamed local copy, for keep-both.
    pub local_copy_rel: Option<String>,
    pub remote_file_id: Option<String>,
    pub message: String,
    pub resolved: bool,
    pub resolved_at: Option<i64>,
}

fn run_from_row(r: &Row<'_>) -> rusqlite::Result<RunRecord> {
    let status: String = r.get(3)?;
    let stats: String = r.get(4)?;
    Ok(RunRecord {
        id: r.get(0)?,
        pair_id: r.get(1)?,
        trigger: r.get(2)?,
        status: RunStatus::parse(&status).unwrap_or(RunStatus::Failed),
        stats: serde_json::from_str(&stats).unwrap_or_default(),
        message: r.get(5)?,
        started_at: r.get(6)?,
        ended_at: r.get(7)?,
    })
}

fn conflict_from_row(r: &Row<'_>) -> rusqlite::Result<ConflictRecord> {
    let policy: String = r.get(4)?;
    Ok(ConflictRecord {
        id: r.get(0)?,
        pair_id: r.get(1)?,
        key: r.get(2)?,
        detected_at: r.get(3)?,
        policy: crate::parse_policy(&policy).unwrap_or(ConflictPolicy::KeepBoth),
        local_copy_rel: r.get(5)?,
        remote_file_id: r.get(6)?,
        message: r.get(7)?,
        resolved: r.get(8)?,
        resolved_at: r.get(9)?,
    })
}

impl Store {
    pub fn start_run(&self, id: &str, pair_id: &str, trigger: &str, started_at: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO runs (id, pair_id, trigger, status, stats_json, started_at) VALUES (?1, ?2, ?3, 'RUNNING', '{}', ?4)",
            params![id, pair_id, trigger, started_at],
        )?;
        Ok(())
    }

    pub fn finish_run(
        &self,
        id: &str,
        status: RunStatus,
        stats: &RunStats,
        message: Option<&str>,
        ended_at: i64,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE runs SET status = ?2, stats_json = ?3, message = ?4, ended_at = ?5 WHERE id = ?1",
            params![
                id,
                status.as_str(),
                serde_json::to_string(stats).unwrap_or_else(|_| "{}".into()),
                message,
                ended_at
            ],
        )?;
        Ok(())
    }

    pub fn run(&self, id: &str) -> Result<Option<RunRecord>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, pair_id, trigger, status, stats_json, message, started_at, ended_at FROM runs WHERE id = ?1",
                [id],
                run_from_row,
            )
            .optional()?)
    }

    /// Runs newest first; for one pair or all.
    pub fn runs(&self, pair_id: Option<&str>, limit: u32) -> Result<Vec<RunRecord>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, pair_id, trigger, status, stats_json, message, started_at, ended_at FROM runs
             WHERE ?1 IS NULL OR pair_id = ?1 ORDER BY started_at DESC, rowid DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![pair_id, limit], run_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Closes runs left `RUNNING` by a process that died (SRS FR-B4).
    pub fn close_stale_runs(&self, message: &str, ended_at: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE runs SET status = 'CANCELLED', message = ?1, ended_at = ?2 WHERE status = 'RUNNING'",
            params![message, ended_at],
        )?)
    }

    /// Appends events to a run's log, in order.
    pub fn append_events(&mut self, run_id: &str, events: &[RunEvent]) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        let mut seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM run_events WHERE run_id = ?1",
            [run_id],
            |r| r.get(0),
        )?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO run_events (run_id, seq, level, key, message, at, action, detail, bytes)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for e in events {
                seq += 1;
                stmt.execute(params![
                    run_id,
                    seq,
                    e.level.as_str(),
                    e.key,
                    e.message,
                    e.at,
                    e.action,
                    e.detail,
                    to_i64(e.bytes)
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn run_events(&self, run_id: &str) -> Result<Vec<RunEvent>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT at, level, action, key, message, detail, bytes FROM run_events WHERE run_id = ?1 ORDER BY seq",
        )?;
        let rows = stmt.query_map([run_id], |r| {
            let level: String = r.get(1)?;
            Ok(RunEvent {
                at: r.get(0)?,
                level: match level.as_str() {
                    "WARN" => EventLevel::Warn,
                    "ERROR" => EventLevel::Error,
                    _ => EventLevel::Info,
                },
                action: r.get(2)?,
                key: r.get(3)?,
                message: r.get(4)?,
                detail: r.get(5)?,
                bytes: to_u64(r.get(6)?),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Deletes runs (and their events) that started before `before` (SRS FR-B6: 90 days).
    pub fn prune_runs(&self, before: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM runs WHERE started_at < ?1 AND status != 'RUNNING'",
            [before],
        )?)
    }

    pub fn add_conflict(&self, c: &ConflictRecord) -> Result<()> {
        self.conn.execute(
            "INSERT INTO conflicts (id, pair_id, key, detected_at, policy, local_copy_rel, remote_file_id, message, resolved, resolved_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                c.id,
                c.pair_id,
                c.key,
                c.detected_at,
                crate::policy_str(c.policy),
                c.local_copy_rel,
                c.remote_file_id,
                c.message,
                c.resolved,
                c.resolved_at
            ],
        )?;
        Ok(())
    }

    /// Open conflicts, newest first; for one pair or all.
    pub fn open_conflicts(&self, pair_id: Option<&str>) -> Result<Vec<ConflictRecord>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, pair_id, key, detected_at, policy, local_copy_rel, remote_file_id, message, resolved, resolved_at
             FROM conflicts WHERE resolved = 0 AND (?1 IS NULL OR pair_id = ?1) ORDER BY detected_at DESC, id",
        )?;
        let rows = stmt.query_map([pair_id], conflict_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn resolve_conflict(&self, id: &str, at: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE conflicts SET resolved = 1, resolved_at = ?2 WHERE id = ?1",
            params![id, at],
        )?;
        Ok(())
    }
}
