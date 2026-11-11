//! The per-file sync baseline, one row per side.

use crate::{Result, SIDE_DRIVE, SIDE_LOCAL, Store, StoreError, to_i64, to_u64};
use rusqlite::{Transaction, params};
use std::collections::BTreeMap;
use wasl_core::BaselineEntry;

const NS_PER_MS: i64 = 1_000_000;

impl Store {
    /// The baseline of a pair, by key.
    pub fn load_baseline(&self, pair_id: &str) -> Result<BTreeMap<String, BaselineEntry>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT key, side, is_dir, size, mtime_ns, md5, drive_id, drive_version, local_name, export_mime
             FROM file_state WHERE pair_id = ?1 AND side IN (?2, ?3) ORDER BY key, side",
        )?;
        let mut rows = stmt.query(params![pair_id, SIDE_LOCAL, SIDE_DRIVE])?;
        let mut out: BTreeMap<String, BaselineEntry> = BTreeMap::new();
        while let Some(row) = rows.next()? {
            let key: String = row.get(0)?;
            let side: String = row.get(1)?;
            let is_dir: bool = row.get(2)?;
            let size: Option<i64> = row.get(3)?;
            let mtime_ns: Option<i64> = row.get(4)?;
            let e = out
                .entry(key.clone())
                .or_insert_with(|| empty_entry(&key, is_dir));
            if side == SIDE_LOCAL {
                e.is_dir = is_dir;
                e.local_size = size.map_or(0, to_u64);
                e.local_mtime = mtime_ns.map_or(0, |ns| ns / NS_PER_MS);
                e.local_hash = row.get(5)?;
                e.local_rel = row.get::<_, Option<String>>(8)?.unwrap_or_else(|| key.clone());
                e.export_mime = row.get(9)?;
            } else {
                e.remote_size = size.map(to_u64);
                e.remote_mtime = mtime_ns.map(|ns| ns / NS_PER_MS);
                e.remote_md5 = row.get(5)?;
                e.remote_id = row.get(6)?;
                e.remote_version = row.get(7)?;
            }
        }
        Ok(out)
    }

    /// The baseline row for one key.
    pub fn baseline_entry(&self, pair_id: &str, key: &str) -> Result<Option<BaselineEntry>> {
        // Small and rare: reuse the full loader's mapping on a filtered query.
        let mut stmt = self.conn.prepare_cached(
            "SELECT side, is_dir, size, mtime_ns, md5, drive_id, drive_version, local_name, export_mime
             FROM file_state WHERE pair_id = ?1 AND key = ?2 AND side IN (?3, ?4)",
        )?;
        let mut rows = stmt.query(params![pair_id, key, SIDE_LOCAL, SIDE_DRIVE])?;
        let mut out: Option<BaselineEntry> = None;
        while let Some(row) = rows.next()? {
            let side: String = row.get(0)?;
            let is_dir: bool = row.get(1)?;
            let size: Option<i64> = row.get(2)?;
            let mtime_ns: Option<i64> = row.get(3)?;
            let e = out.get_or_insert_with(|| empty_entry(key, is_dir));
            if side == SIDE_LOCAL {
                e.is_dir = is_dir;
                e.local_size = size.map_or(0, to_u64);
                e.local_mtime = mtime_ns.map_or(0, |ns| ns / NS_PER_MS);
                e.local_hash = row.get(4)?;
                e.local_rel = row.get::<_, Option<String>>(7)?.unwrap_or_else(|| key.to_owned());
                e.export_mime = row.get(8)?;
            } else {
                e.remote_size = size.map(to_u64);
                e.remote_mtime = mtime_ns.map(|ns| ns / NS_PER_MS);
                e.remote_md5 = row.get(4)?;
                e.remote_id = row.get(5)?;
                e.remote_version = row.get(6)?;
            }
        }
        Ok(out)
    }

    /// Records the synced state of one key on every side, atomically.
    pub fn put_baseline(&mut self, pair_id: &str, entry: &BaselineEntry) -> Result<()> {
        let tx = self.conn.transaction()?;
        put(&tx, pair_id, entry)?;
        tx.commit()?;
        Ok(())
    }

    /// Records several keys in one transaction (for bookkeeping-only plans).
    pub fn put_baselines<'a>(
        &mut self,
        pair_id: &str,
        entries: impl IntoIterator<Item = &'a BaselineEntry>,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        for e in entries {
            put(&tx, pair_id, e)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Forgets a key on every side.
    pub fn remove_baseline(&self, pair_id: &str, key: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM file_state WHERE pair_id = ?1 AND key = ?2",
            params![pair_id, key],
        )?;
        Ok(())
    }

    /// Forgets a folder and everything under it.
    pub fn remove_baseline_subtree(&self, pair_id: &str, key: &str) -> Result<()> {
        // Prefix match without LIKE, whose wildcards (`%`, `_`) occur in file names.
        self.conn.execute(
            "DELETE FROM file_state WHERE pair_id = ?1 AND (key = ?2 OR substr(key, 1, length(?2) + 1) = ?2 || '/')",
            params![pair_id, key],
        )?;
        Ok(())
    }
}

fn put(tx: &Transaction<'_>, pair_id: &str, e: &BaselineEntry) -> Result<()> {
    tx.execute(
        "INSERT OR REPLACE INTO file_state
            (pair_id, key, side, is_dir, size, mtime_ns, md5, local_name, export_mime)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            pair_id,
            e.key,
            SIDE_LOCAL,
            e.is_dir,
            to_i64(e.local_size),
            to_ns(e.local_mtime)?,
            e.local_hash,
            e.local_rel,
            e.export_mime,
        ],
    )?;
    let has_drive = e.remote_id.is_some()
        || e.remote_md5.is_some()
        || e.remote_size.is_some()
        || e.remote_mtime.is_some()
        || e.remote_version.is_some();
    if has_drive {
        tx.execute(
            "INSERT OR REPLACE INTO file_state
                (pair_id, key, side, is_dir, size, mtime_ns, md5, drive_id, drive_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                pair_id,
                e.key,
                SIDE_DRIVE,
                e.is_dir,
                e.remote_size.map(to_i64),
                e.remote_mtime.map(to_ns).transpose()?,
                e.remote_md5,
                e.remote_id,
                e.remote_version,
            ],
        )?;
    } else {
        tx.execute(
            "DELETE FROM file_state WHERE pair_id = ?1 AND key = ?2 AND side = ?3",
            params![pair_id, e.key, SIDE_DRIVE],
        )?;
    }
    Ok(())
}

fn empty_entry(key: &str, is_dir: bool) -> BaselineEntry {
    BaselineEntry {
        key: key.to_owned(),
        local_rel: key.to_owned(),
        is_dir,
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

fn to_ns(ms: i64) -> Result<i64> {
    ms.checked_mul(NS_PER_MS).ok_or(StoreError::TimeOutOfRange(ms))
}
