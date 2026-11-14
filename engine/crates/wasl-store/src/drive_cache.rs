//! The Drive metadata cache and resumable upload sessions, persisted (SRS FR-C3, FR-D1).

use crate::{Result, SharedStore, Store, to_i64, to_u64};
use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use wasl_drive::{AccountState, DriveError, DriveNode, NodeStore, SessionStore};

fn db_err(e: crate::StoreError) -> DriveError {
    DriveError::Io(format!("local database: {e}"))
}

const NODE_COLUMNS: &str =
    "account_id, id, name, mime_type, parent_id, drive_id, size, md5, modified_time, version,
    trashed, is_folder, is_shortcut, shortcut_target_id, shortcut_target_mime, shared, permission_count,
    app_properties_json";

fn node_from_row(r: &Row<'_>) -> rusqlite::Result<DriveNode> {
    Ok(DriveNode {
        account_id: r.get(0)?,
        id: r.get(1)?,
        name: r.get(2)?,
        mime_type: r.get(3)?,
        parent_id: r.get(4)?,
        drive_id: r.get(5)?,
        size: r.get::<_, Option<i64>>(6)?.map(to_u64),
        md5: r.get(7)?,
        modified_time: r.get(8)?,
        version: r.get::<_, Option<i64>>(9)?.map(to_u64),
        trashed: r.get(10)?,
        is_folder: r.get(11)?,
        is_shortcut: r.get(12)?,
        shortcut_target_id: r.get(13)?,
        shortcut_target_mime: r.get(14)?,
        shared: r.get(15)?,
        permission_count: r
            .get::<_, Option<i64>>(16)?
            .map(|n| n.clamp(0, i64::from(u32::MAX)) as u32),
        // A damaged value is read as none: it is a cache Drive refills.
        app_properties: r
            .get::<_, Option<String>>(17)?
            .and_then(|j| serde_json::from_str(&j).ok()),
    })
}

impl Store {
    pub fn drive_account(&self, account_id: &str) -> Result<Option<AccountState>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, root_folder_id, change_page_token, last_crawl_at, last_delta_at FROM accounts WHERE id = ?1",
                [account_id],
                |r| {
                    Ok(AccountState {
                        id: r.get(0)?,
                        root_folder_id: r.get(1)?,
                        changes_token: r.get(2)?,
                        last_crawl_at: r.get(3)?,
                        last_delta_at: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    /// Creates or updates an account's cache bookkeeping, keeping its email.
    pub fn upsert_drive_account(&self, a: &AccountState) -> Result<()> {
        self.conn.execute(
            "INSERT INTO accounts (id, email, root_folder_id, change_page_token, last_crawl_at, last_delta_at)
             VALUES (?1, ?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET root_folder_id = ?2, change_page_token = ?3, last_crawl_at = ?4, last_delta_at = ?5",
            params![a.id, a.root_folder_id, a.changes_token, a.last_crawl_at, a.last_delta_at],
        )?;
        Ok(())
    }

    pub fn set_account_email(&self, account_id: &str, email: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO accounts (id, email) VALUES (?1, ?2) ON CONFLICT(id) DO UPDATE SET email = ?2",
            params![account_id, email],
        )?;
        Ok(())
    }

    pub fn set_account_auth_error(&self, account_id: &str, error: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE accounts SET auth_error = ?2 WHERE id = ?1",
            params![account_id, error],
        )?;
        Ok(())
    }

    /// Every account: (id, email, last sign-in problem).
    pub fn accounts(&self) -> Result<Vec<(String, String, Option<String>)>> {
        let mut st = self
            .conn
            .prepare("SELECT id, email, auth_error FROM accounts ORDER BY email")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Forgets an account and its Drive cache. Refused while a pair uses it.
    pub fn remove_account(&self, account_id: &str) -> Result<bool> {
        let used: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM pairs WHERE account_id = ?1",
            [account_id],
            |r| r.get(0),
        )?;
        if used > 0 {
            return Ok(false);
        }
        self.conn
            .execute("DELETE FROM drive_nodes WHERE account_id = ?1", [account_id])?;
        self.conn
            .execute("DELETE FROM accounts WHERE id = ?1", [account_id])?;
        Ok(true)
    }

    /// The last sign-in problem recorded for an account, if it still stands.
    pub fn account_auth_error(&self, account_id: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT auth_error FROM accounts WHERE id = ?1",
                [account_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten())
    }

    pub fn drive_nodes(&self, account_id: &str) -> Result<Vec<DriveNode>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {NODE_COLUMNS} FROM drive_nodes WHERE account_id = ?1 AND trashed = 0"
        ))?;
        let rows = stmt.query_map([account_id], node_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn upsert_drive_nodes(&mut self, nodes: &[DriveNode]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(&format!(
                "INSERT OR REPLACE INTO drive_nodes ({NODE_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)"
            ))?;
            for n in nodes {
                stmt.execute(params![
                    n.account_id,
                    n.id,
                    n.name,
                    n.mime_type,
                    n.parent_id,
                    n.drive_id,
                    n.size.map(to_i64),
                    n.md5,
                    n.modified_time,
                    n.version.map(to_i64),
                    n.trashed,
                    n.is_folder,
                    n.is_shortcut,
                    n.shortcut_target_id,
                    n.shortcut_target_mime,
                    n.shared,
                    n.permission_count,
                    n.app_properties
                        .as_ref()
                        .map(|p| serde_json::to_string(p).expect("strings are JSON")),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

#[async_trait]
impl NodeStore for SharedStore {
    async fn account(&self, account_id: &str) -> wasl_drive::Result<Option<AccountState>> {
        self.lock().drive_account(account_id).map_err(db_err)
    }

    async fn upsert_account(&self, account: AccountState) -> wasl_drive::Result<()> {
        self.lock().upsert_drive_account(&account).map_err(db_err)
    }

    async fn set_changes_token(
        &self,
        account_id: &str,
        token: Option<&str>,
        at: i64,
    ) -> wasl_drive::Result<()> {
        self.lock()
            .conn
            .execute(
                "UPDATE accounts SET change_page_token = ?2, last_delta_at = ?3 WHERE id = ?1",
                params![account_id, token, at],
            )
            .map(|_| ())
            .map_err(|e| db_err(e.into()))
    }

    async fn all_for_account(&self, account_id: &str) -> wasl_drive::Result<Vec<DriveNode>> {
        self.lock().drive_nodes(account_id).map_err(db_err)
    }

    async fn count(&self, account_id: &str) -> wasl_drive::Result<usize> {
        self.lock()
            .conn
            .query_row(
                "SELECT COUNT(*) FROM drive_nodes WHERE account_id = ?1",
                [account_id],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n as usize)
            .map_err(|e| db_err(e.into()))
    }

    async fn upsert_all(&self, nodes: &[DriveNode]) -> wasl_drive::Result<()> {
        self.lock().upsert_drive_nodes(nodes).map_err(db_err)
    }

    async fn delete_ids(&self, account_id: &str, ids: &[String]) -> wasl_drive::Result<()> {
        let mut store = self.lock();
        let tx = store.conn.transaction().map_err(|e| db_err(e.into()))?;
        for id in ids {
            tx.execute(
                "DELETE FROM drive_nodes WHERE account_id = ?1 AND id = ?2",
                params![account_id, id],
            )
            .map_err(|e| db_err(e.into()))?;
        }
        tx.commit().map_err(|e| db_err(e.into()))
    }

    async fn clear_account(&self, account_id: &str) -> wasl_drive::Result<()> {
        self.lock()
            .conn
            .execute("DELETE FROM drive_nodes WHERE account_id = ?1", [account_id])
            .map(|_| ())
            .map_err(|e| db_err(e.into()))
    }

    async fn clear_drive(&self, account_id: &str, drive_id: &str) -> wasl_drive::Result<()> {
        self.lock()
            .conn
            .execute(
                "DELETE FROM drive_nodes WHERE account_id = ?1 AND drive_id = ?2",
                params![account_id, drive_id],
            )
            .map(|_| ())
            .map_err(|e| db_err(e.into()))
    }
}

#[async_trait]
impl SessionStore for SharedStore {
    async fn get(&self, key: &str) -> wasl_drive::Result<Option<String>> {
        self.lock()
            .conn
            .query_row(
                "SELECT uri FROM upload_sessions WHERE session_key = ?1",
                [key],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| db_err(e.into()))
    }

    async fn put(&self, key: &str, uri: &str) -> wasl_drive::Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        self.lock()
            .conn
            .execute(
                "INSERT OR REPLACE INTO upload_sessions (session_key, uri, updated_at) VALUES (?1, ?2, ?3)",
                params![key, uri, now],
            )
            .map(|_| ())
            .map_err(|e| db_err(e.into()))
    }

    async fn remove(&self, key: &str) -> wasl_drive::Result<()> {
        self.lock()
            .conn
            .execute("DELETE FROM upload_sessions WHERE session_key = ?1", [key])
            .map(|_| ())
            .map_err(|e| db_err(e.into()))
    }
}
