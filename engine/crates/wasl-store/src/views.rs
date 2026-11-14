//! On-demand views (docs/ON_DEMAND.md §2.3): a Drive folder shown read-only at a local path.

use crate::{Result, Store, to_i64, to_u64};
use rusqlite::{OptionalExtension, Row, params};

pub use wasl_proto::store::View;
const COLUMNS: &str = "id, name, account_id, drive_folder_id, drive_id, remote_path, local_path, vault_id,
    cache_limit, enabled, created_at, last_refresh_at, last_error";

fn view_from_row(r: &Row<'_>) -> rusqlite::Result<View> {
    Ok(View {
        id: r.get(0)?,
        name: r.get(1)?,
        account_id: r.get(2)?,
        drive_folder_id: r.get(3)?,
        drive_id: r.get(4)?,
        remote_path: r.get(5)?,
        local_path: r.get(6)?,
        vault_id: r.get(7)?,
        cache_limit: to_u64(r.get(8)?),
        enabled: r.get(9)?,
        created_at: r.get(10)?,
        last_refresh_at: r.get(11)?,
        last_error: r.get(12)?,
    })
}

impl Store {
    pub fn create_view(&self, v: &View) -> Result<()> {
        self.conn.execute(
            &format!("INSERT INTO views ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"),
            params![
                v.id,
                v.name,
                v.account_id,
                v.drive_folder_id,
                v.drive_id,
                v.remote_path,
                v.local_path,
                v.vault_id,
                to_i64(v.cache_limit),
                v.enabled,
                v.created_at,
                v.last_refresh_at,
                v.last_error,
            ],
        )?;
        Ok(())
    }

    pub fn views(&self) -> Result<Vec<View>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!("SELECT {COLUMNS} FROM views ORDER BY name, id"))?;
        let rows = stmt.query_map([], view_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn view(&self, id: &str) -> Result<Option<View>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM views WHERE id = ?1"),
                [id],
                view_from_row,
            )
            .optional()?)
    }

    pub fn delete_view(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM views WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn set_view_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE views SET enabled = ?2 WHERE id = ?1",
            params![id, enabled],
        )?;
        Ok(())
    }

    pub fn set_view_cache_limit(&self, id: &str, limit: u64) -> Result<()> {
        self.conn.execute(
            "UPDATE views SET cache_limit = ?2 WHERE id = ?1",
            params![id, to_i64(limit)],
        )?;
        Ok(())
    }

    /// Links the view to the vault its folder holds (or unlinks it with `None`).
    pub fn set_view_vault(&self, id: &str, vault_id: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE views SET vault_id = ?2 WHERE id = ?1",
            params![id, vault_id],
        )?;
        Ok(())
    }

    /// Records a refresh: its time, and the error if it failed (cleared when it worked).
    pub fn set_view_refreshed(&self, id: &str, at: i64, error: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE views SET last_refresh_at = ?2, last_error = ?3 WHERE id = ?1",
            params![id, at, error],
        )?;
        Ok(())
    }
}
