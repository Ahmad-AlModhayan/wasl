//! The highest vault v2 manifest version seen per file (rollback detection, docs/ENCRYPTION.md
//! §11.6).

use crate::{Result, Store, to_i64, to_u64};
use rusqlite::OptionalExtension;

impl Store {
    /// The highest manifest version recorded for `file_id` (hex) in `pair_id`'s vault.
    pub fn vault_version(&self, pair_id: &str, file_id: &str) -> Result<Option<u64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT version FROM vault_versions WHERE pair_id = ?1 AND file_id = ?2",
                (pair_id, file_id),
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .map(to_u64))
    }

    /// Records that `version` of `file_id` was seen; the stored value only ever grows.
    pub fn note_vault_version(&self, pair_id: &str, file_id: &str, version: u64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO vault_versions (pair_id, file_id, version) VALUES (?1, ?2, ?3)
             ON CONFLICT (pair_id, file_id) DO UPDATE SET version = max(version, excluded.version)",
            (pair_id, file_id, to_i64(version)),
        )?;
        Ok(())
    }
}
