//! The recycle bin index: every file Wasl deleted or overwrote, on either side, for 30 days
//! (SRS FR-F5). The files themselves sit in the local bin folder or in Drive's own bin.

use crate::{Result, Store, to_i64, to_u64};
use rusqlite::{OptionalExtension, Row, params};

pub use wasl_proto::store::{BinEntry, BinSide, NewBinEntry};
const COLUMNS: &str = "id, pair_id, side, key, reason, location, deleted_at, expires_at, size, account_id,
    remote_file_id, display_name, pair_name, version_no";

fn from_row(r: &Row<'_>) -> rusqlite::Result<BinEntry> {
    let side: String = r.get(2)?;
    Ok(BinEntry {
        entry: NewBinEntry {
            id: r.get(0)?,
            pair_id: r.get(1)?,
            side: if side == "drive" {
                BinSide::Drive
            } else {
                BinSide::Local
            },
            key: r.get(3)?,
            reason: r.get(4)?,
            location: r.get(5)?,
            deleted_at: r.get(6)?,
            expires_at: r.get(7)?,
            size: to_u64(r.get(8)?),
            account_id: r.get(9)?,
            remote_file_id: r.get(10)?,
            display_name: r.get(11)?,
            pair_name: r.get(12)?,
        },
        version_no: r.get::<_, i64>(13)?.clamp(0, i64::from(u32::MAX)) as u32,
    })
}

impl Store {
    /// Records a binned file. Its version number is one more than the newest for the same key.
    pub fn add_to_bin(&self, e: &NewBinEntry) -> Result<u32> {
        let version: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(version_no), 0) + 1 FROM trash WHERE pair_id = ?1 AND key = ?2 AND side = ?3",
            params![e.pair_id, e.key, e.side.as_str()],
            |r| r.get(0),
        )?;
        self.conn.execute(
            &format!("INSERT INTO trash ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)"),
            params![
                e.id,
                e.pair_id,
                e.side.as_str(),
                e.key,
                e.reason,
                e.location,
                e.deleted_at,
                e.expires_at,
                to_i64(e.size),
                e.account_id,
                e.remote_file_id,
                e.display_name,
                e.pair_name,
                version,
            ],
        )?;
        Ok(version as u32)
    }

    /// Bin entries, newest first; for one pair or all.
    pub fn bin_entries(&self, pair_id: Option<&str>) -> Result<Vec<BinEntry>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM trash WHERE ?1 IS NULL OR pair_id = ?1 ORDER BY deleted_at DESC, id"
        ))?;
        let rows = stmt.query_map([pair_id], from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Versions of one file in the local bin, newest first (SRS FR-F6).
    pub fn bin_versions(&self, pair_id: &str, key: &str) -> Result<Vec<BinEntry>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM trash WHERE pair_id = ?1 AND key = ?2 ORDER BY version_no DESC"
        ))?;
        let rows = stmt.query_map(params![pair_id, key], from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn bin_entry(&self, id: &str) -> Result<Option<BinEntry>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM trash WHERE id = ?1"),
                [id],
                from_row,
            )
            .optional()?)
    }

    pub fn remove_from_bin(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM trash WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Entries whose retention ended before `now`.
    pub fn expired_bin_entries(&self, now: i64) -> Result<Vec<BinEntry>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM trash WHERE expires_at <= ?1 ORDER BY expires_at"
        ))?;
        let rows = stmt.query_map([now], from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Local bin entries for one key beyond the newest `keep` (SRS FR-F5: 5 versions per file).
    pub fn excess_local_versions(&self, pair_id: &str, key: &str, keep: u32) -> Result<Vec<BinEntry>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM trash WHERE pair_id = ?1 AND key = ?2 AND side = 'local'
             ORDER BY version_no DESC LIMIT -1 OFFSET ?3"
        ))?;
        let rows = stmt.query_map(params![pair_id, key, keep], from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}
