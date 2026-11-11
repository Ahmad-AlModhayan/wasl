//! Read queries for the apps' screens (pair cards, activity, problems, bin, version history).

use crate::{BinEntry, ConflictRecord, EventLevel, Result, RunEvent, RunRecord, Store, to_u64};
use rusqlite::{OptionalExtension, params};
use wasl_core::BaselineEntry;
use wasl_core::identity::name_key;

/// Baseline totals of one pair (files only).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct PairTotals {
    pub pair_id: String,
    pub files: u64,
    pub local_bytes: u64,
    pub remote_bytes: u64,
}

/// A run event with the run and pair it belongs to, as lists across runs show it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct PairEvent {
    pub run_id: String,
    pub pair_id: String,
    /// Position within its run.
    pub seq: u64,
    pub event: RunEvent,
}

const EVENT_COLUMNS: &str =
    "e.run_id, r.pair_id, e.seq, e.at, e.level, e.action, e.key, e.message, e.detail, e.bytes";

fn pair_event(r: &rusqlite::Row<'_>) -> rusqlite::Result<PairEvent> {
    let level: String = r.get(4)?;
    Ok(PairEvent {
        run_id: r.get(0)?,
        pair_id: r.get(1)?,
        seq: to_u64(r.get(2)?),
        event: RunEvent {
            at: r.get(3)?,
            level: match level.as_str() {
                "WARN" => EventLevel::Warn,
                "ERROR" => EventLevel::Error,
                _ => EventLevel::Info,
            },
            action: r.get(5)?,
            key: r.get(6)?,
            message: r.get(7)?,
            detail: r.get(8)?,
            bytes: to_u64(r.get(9)?),
        },
    })
}

impl Store {
    /// Files and bytes per pair from the baseline.
    pub fn pair_totals(&self) -> Result<Vec<PairTotals>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT pair_id,
                    SUM(CASE WHEN side = 'local' THEN 1 ELSE 0 END),
                    COALESCE(SUM(CASE WHEN side = 'local' THEN size END), 0),
                    COALESCE(SUM(CASE WHEN side = 'drive' THEN size END), 0)
             FROM file_state WHERE is_dir = 0 GROUP BY pair_id ORDER BY pair_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(PairTotals {
                pair_id: r.get(0)?,
                files: to_u64(r.get(1)?),
                local_bytes: to_u64(r.get(2)?),
                remote_bytes: to_u64(r.get(3)?),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Files (not folders) in a pair's baseline.
    pub fn baseline_file_count(&self, pair_id: &str) -> Result<u64> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM file_state WHERE pair_id = ?1 AND side = 'local' AND is_dir = 0",
            [pair_id],
            |r| r.get(0),
        )?;
        Ok(to_u64(n))
    }

    /// Whether a baseline row other than `key`'s has the local name `local_rel`, compared by
    /// [`name_key`] (ADR-0004), as on a case-insensitive device folder. SQLite can't fold Unicode
    /// case, so the names are compared here; an export only asks when it moves to another name.
    pub fn local_rel_held_by_other(&self, pair_id: &str, local_rel: &str, key: &str) -> Result<bool> {
        let wanted = name_key(local_rel);
        let mut stmt = self.conn.prepare_cached(
            "SELECT local_name FROM file_state WHERE pair_id = ?1 AND side = 'local' AND key <> ?2",
        )?;
        let mut rows = stmt.query(params![pair_id, key])?;
        while let Some(row) = rows.next()? {
            let name: String = row.get(0)?;
            if name == local_rel || name_key(&name) == wanted {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The baseline row whose local name is `local_rel` (local names can differ from keys).
    pub fn baseline_by_local_rel(&self, pair_id: &str, local_rel: &str) -> Result<Option<BaselineEntry>> {
        let key: Option<String> = self
            .conn
            .query_row(
                "SELECT key FROM file_state WHERE pair_id = ?1 AND side = 'local' AND local_name = ?2",
                params![pair_id, local_rel],
                |r| r.get(0),
            )
            .optional()?;
        match key {
            Some(k) => self.baseline_entry(pair_id, &k),
            None => Ok(None),
        }
    }

    /// Events newest first: for one pair or all, problems only (warnings and errors) or all,
    /// since a time or ever.
    pub fn events(
        &self,
        pair_id: Option<&str>,
        problems_only: bool,
        since: Option<i64>,
        limit: u32,
    ) -> Result<Vec<PairEvent>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {EVENT_COLUMNS} FROM run_events e JOIN runs r ON r.id = e.run_id
             WHERE (?1 IS NULL OR r.pair_id = ?1) AND (?2 = 0 OR e.level != 'INFO') AND (?3 IS NULL OR e.at >= ?3)
             ORDER BY e.at DESC, r.rowid DESC, e.seq DESC LIMIT ?4"
        ))?;
        let rows = stmt.query_map(params![pair_id, problems_only, since, limit], pair_event)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Every upload of one file, newest first (the version history's "uploaded from this device").
    pub fn uploads_of(&self, pair_id: &str, key: &str, limit: u32) -> Result<Vec<PairEvent>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {EVENT_COLUMNS} FROM run_events e JOIN runs r ON r.id = e.run_id
             WHERE r.pair_id = ?1 AND e.key = ?2 AND e.action = 'UPLOAD' ORDER BY e.at DESC LIMIT ?3"
        ))?;
        let rows = stmt.query_map(params![pair_id, key, limit], pair_event)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Runs that started at or after `since`, newest first.
    pub fn runs_since(&self, since: i64) -> Result<Vec<RunRecord>> {
        let ids: Vec<String> = {
            let mut stmt = self.conn.prepare_cached(
                "SELECT id FROM runs WHERE started_at >= ?1 ORDER BY started_at DESC, rowid DESC",
            )?;
            let rows = stmt.query_map([since], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(r) = self.run(&id)? {
                out.push(r);
            }
        }
        Ok(out)
    }

    /// Total bytes held by bin entries.
    pub fn bin_total_size(&self) -> Result<u64> {
        let n: i64 = self
            .conn
            .query_row("SELECT COALESCE(SUM(size), 0) FROM trash", [], |r| r.get(0))?;
        Ok(to_u64(n))
    }

    /// Bin entries deleted before `before`, oldest first.
    pub fn bin_entries_before(&self, before: i64) -> Result<Vec<BinEntry>> {
        Ok(self
            .bin_entries(None)?
            .into_iter()
            .rev()
            .filter(|b| b.entry.deleted_at < before)
            .collect())
    }

    pub fn conflict(&self, id: &str) -> Result<Option<ConflictRecord>> {
        Ok(self
            .open_or_resolved_conflicts()?
            .into_iter()
            .find(|c| c.id == id))
    }

    /// Undoes a resolution (the "Undo" after Keep this / Keep other).
    pub fn unresolve_conflict(&self, id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE conflicts SET resolved = 0, resolved_at = NULL WHERE id = ?1",
            [id],
        )?;
        Ok(())
    }

    fn open_or_resolved_conflicts(&self) -> Result<Vec<ConflictRecord>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, pair_id, key, detected_at, policy, local_copy_rel, remote_file_id, message, resolved, resolved_at
             FROM conflicts ORDER BY detected_at DESC, id",
        )?;
        let rows = stmt.query_map([], |r| {
            let policy: String = r.get(4)?;
            Ok(ConflictRecord {
                id: r.get(0)?,
                pair_id: r.get(1)?,
                key: r.get(2)?,
                detected_at: r.get(3)?,
                policy: crate::parse_policy(&policy).unwrap_or(wasl_core::ConflictPolicy::KeepBoth),
                local_copy_rel: r.get(5)?,
                remote_file_id: r.get(6)?,
                message: r.get(7)?,
                resolved: r.get(8)?,
                resolved_at: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}
