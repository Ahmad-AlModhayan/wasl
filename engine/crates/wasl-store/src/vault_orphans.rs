//! When each vault v2 chunk object of a pair stopped being referenced (the chunk sweep's clock,
//! docs/ENCRYPTION.md §11).

use std::collections::HashMap;

use crate::{Result, Store};

impl Store {
    /// The pair's unreferenced chunks (object name) with when they were first seen unreferenced.
    pub fn vault_orphans(&self, pair_id: &str) -> Result<HashMap<String, i64>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT chunk, since_ms FROM vault_orphans WHERE pair_id = ?1")?;
        let rows = stmt.query_map([pair_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Replaces the pair's record with `orphans` (after a sweep: chunks referenced again or
    /// deleted drop out), atomically.
    pub fn set_vault_orphans(&mut self, pair_id: &str, orphans: &HashMap<String, i64>) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM vault_orphans WHERE pair_id = ?1", [pair_id])?;
        {
            let mut insert = tx
                .prepare_cached("INSERT INTO vault_orphans (pair_id, chunk, since_ms) VALUES (?1, ?2, ?3)")?;
            for (chunk, since) in orphans {
                insert.execute((pair_id, chunk, since))?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}
