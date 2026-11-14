//! Vaults known on this device (docs/ENCRYPTION.md §6, §11.1). A row holds no secret: the vault's
//! `vault.json` (salt, cost, key check) and where it lives on Drive. Whether the vault is unlocked
//! here is decided by the platform's key store, not by this table.

use crate::{Result, Store};
use rusqlite::{OptionalExtension, Row, params};

/// One vault.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VaultRecord {
    pub id: String,
    pub name: String,
    /// 1 (`drivesync-vault`, PBKDF2) or 2 (`wasl-vault`, Argon2id).
    pub format_version: u32,
    /// The key derivation and its cost, e.g. `{"kdf":"argon2id","m":65536,"t":3,"p":1}`.
    pub kdf_params_json: String,
    pub key_check: Vec<u8>,
    /// The vault's `vault.json` as found on Drive.
    pub manifest_json: String,
    /// The Drive folder the vault was made in or unlocked from.
    pub account_id: Option<String>,
    pub drive_folder_id: Option<String>,
    pub drive_id: Option<String>,
    /// Display path of that folder, e.g. `My Drive/Private`.
    pub remote_path: String,
    pub created_at: i64,
}

const COLUMNS: &str = "id, name, format_version, kdf_params_json, key_check, manifest_json, account_id,
    drive_folder_id, drive_id, remote_path, created_at";

fn vault_from_row(r: &Row<'_>) -> rusqlite::Result<VaultRecord> {
    Ok(VaultRecord {
        id: r.get(0)?,
        name: r.get(1)?,
        format_version: u32::try_from(r.get::<_, i64>(2)?).unwrap_or(0),
        kdf_params_json: r.get(3)?,
        key_check: r.get(4)?,
        manifest_json: r.get(5)?,
        account_id: r.get(6)?,
        drive_folder_id: r.get(7)?,
        drive_id: r.get(8)?,
        remote_path: r.get(9)?,
        created_at: r.get(10)?,
    })
}

impl Store {
    /// Inserts the vault, or replaces the row with the same id.
    pub fn put_vault(&self, v: &VaultRecord) -> Result<()> {
        self.conn.execute(
            &format!(
                "INSERT OR REPLACE INTO vaults ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"
            ),
            params![
                v.id,
                v.name,
                i64::from(v.format_version),
                v.kdf_params_json,
                v.key_check,
                v.manifest_json,
                v.account_id,
                v.drive_folder_id,
                v.drive_id,
                v.remote_path,
                v.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn vaults(&self) -> Result<Vec<VaultRecord>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!("SELECT {COLUMNS} FROM vaults ORDER BY name, id"))?;
        let rows = stmt.query_map([], vault_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn vault(&self, id: &str) -> Result<Option<VaultRecord>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM vaults WHERE id = ?1"),
                [id],
                vault_from_row,
            )
            .optional()?)
    }

    pub fn delete_vault(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM vaults WHERE id = ?1", [id])?;
        Ok(())
    }
}
