//! SQLite store: schema, migrations and every table the engine keeps (SRS §6, FR-B3).
//!
//! The baseline is stored one row per side (`local`, `drive`; device ids from M4), so a key's
//! state on each side can be committed on its own. [`Store::put_baseline`] writes all sides of one
//! key in one transaction: an interrupted run keeps every file it finished.
//!
//! [`Store`] is a single connection. [`SharedStore`] wraps it for the engine's threads and also
//! implements the Drive crate's [`wasl_drive::NodeStore`] and [`wasl_drive::SessionStore`], so the
//! Drive metadata cache and resumable uploads survive restarts.

#[cfg(feature = "ffi")]
uniffi::setup_scaffolding!();

mod agent;
mod agent_undo;
mod baseline;
mod bin;
mod devices;
mod drive_cache;
mod history;
mod introductions;
mod oplog;
mod pairs;
mod queries;
mod settings;
mod vault_orphans;
mod vault_versions;
mod vaults;
mod views;

use rusqlite::Connection;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

pub use agent::{
    AgentEntry, Changeset, ChangesetExt, ChangesetStatus, EarlierVersion, EntryKind, ManifestRow, SavedCopy,
};
pub use agent_undo::{
    DEFAULT_WATCH_IGNORES, JournalEntry, NextMeta, SnapshotOrigin, SnapshotRow, StatKey, Watch, WatchGap,
};
pub use bin::{BinEntry, BinSide, NewBinEntry};
pub use devices::{ChunkRow, IndexRow};
pub use history::{ConflictRecord, EventLevel, RunEvent, RunRecord, RunStats};
pub use introductions::{NewOffer, OfferState, ReceivedOffer};
pub use oplog::{
    Decision, ImportOp, NewChangeset, NewOp, OPLOG_HISTORY_MS, OpOutcome, OplogChangeset, OplogError,
    OplogFilter, OplogKey, OplogListed, OplogOp, OplogStatus, check_actor, check_entity_uri, file_uri,
    file_uri_parts, folder_uri,
};
pub use pairs::{AGENT_CHANGES_PENDING, Pair, PairStatus, StorageMode, repointed};
pub use queries::{PairEvent, PairTotals};
pub use vaults::VaultRecord;
pub use views::View;

/// Schema migrations, applied in order. Index + 1 is the schema version (`PRAGMA user_version`).
const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_initial.sql"),
    include_str!("../migrations/0002_engine.sql"),
    include_str!("../migrations/0003_devices.sql"),
    include_str!("../migrations/0004_vault_versions.sql"),
    include_str!("../migrations/0005_views.sql"),
    include_str!("../migrations/0006_vaults_desktop.sql"),
    include_str!("../migrations/0007_vault_orphans.sql"),
    include_str!("../migrations/0008_agent_changesets.sql"),
    include_str!("../migrations/0009_agent_integrity.sql"),
    include_str!("../migrations/0010_introductions.sql"),
    include_str!("../migrations/0011_oplog.sql"),
    include_str!("../migrations/0012_agent_undo.sql"),
];

/// Schema versions whose migration rebuilds a table other tables point at. SQLite's way: foreign
/// keys off while it runs (it can't be changed inside a transaction), checked before it commits
/// (https://sqlite.org/lang_altertable.html#otheralter).
const REBUILDS: [i64; 1] = [12];

pub const SIDE_LOCAL: &str = "local";
pub const SIDE_DRIVE: &str = "drive";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("the database was written by a newer Wasl (schema {found}, this build knows {known})")]
    TooNew { found: i64, known: i64 },
    #[error("time {0} ms is outside the range the store can hold")]
    TimeOutOfRange(i64),
    #[error("stored value is invalid: {0}")]
    Corrupt(String),
    /// A change the store turns down because of what is stored (the caller's request, not
    /// damaged data). The message says what to do.
    #[error("{0}")]
    Refused(String),
    /// An op-log write ADR-0002 doesn't allow.
    #[error(transparent)]
    Oplog(#[from] OplogError),
}

pub type Result<T> = std::result::Result<T, StoreError>;

pub struct Store {
    conn: Connection,
    /// This device's id, which the engine hands over at start ([`Store::set_oplog_device`]):
    /// agent changes are written to the op-log under it. `None` (no device key yet): only the
    /// agent tables are written, and the one-time copy brings those changesets in later.
    oplog_device: Option<String>,
}

impl Store {
    /// Opens (creating if needed) and migrates the database at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Store> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Store> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Store> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate(&mut conn)?;
        Ok(Store {
            conn,
            oplog_device: None,
        })
    }

    /// Sets the device id (`DeviceKey::id()`, as Wasl writes it) that agent changes are recorded
    /// under in the op-log (ADR-0002: the store doesn't hold the device key, so the engine passes
    /// the id in). `None` turns the op-log writes of agent changes off.
    pub fn set_oplog_device(&mut self, device_id: Option<String>) {
        self.oplog_device = device_id;
    }

    /// The device id agent changes are recorded under, when one was set.
    pub fn oplog_device(&self) -> Option<&str> {
        self.oplog_device.as_deref()
    }

    /// The schema version of the open database.
    pub fn schema_version(&self) -> Result<i64> {
        Ok(self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    /// Direct access, for tests and tools.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let known = MIGRATIONS.len() as i64;
    let found: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if found > known {
        return Err(StoreError::TooNew { found, known });
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(found as usize) {
        let version = i as i64 + 1;
        let rebuild = REBUILDS.contains(&version);
        if rebuild {
            conn.pragma_update(None, "foreign_keys", "OFF")?;
        }
        let applied = apply(conn, sql, version, rebuild);
        if rebuild {
            conn.pragma_update(None, "foreign_keys", "ON")?;
        }
        applied?;
    }
    Ok(())
}

fn apply(conn: &mut Connection, sql: &str, version: i64, check_keys: bool) -> Result<()> {
    let tx = conn.transaction()?;
    // Rows that pointed at missing rows before the migration are not its doing: only new ones fail it.
    let before = if check_keys {
        key_violations(&tx)?
    } else {
        HashMap::new()
    };
    tx.execute_batch(sql)?;
    if check_keys
        && let Some(((table, parent, _), _)) = key_violations(&tx)?
            .into_iter()
            .find(|(k, n)| *n > before.get(k).copied().unwrap_or(0))
    {
        return Err(StoreError::Corrupt(format!(
            "schema v{version} would leave rows of {table} pointing at missing rows of {parent}"
        )));
    }
    tx.pragma_update(None, "user_version", version)?;
    tx.commit()?;
    Ok(())
}

/// Rows pointing at missing rows, counted by (table, the table it points at, the values that
/// point). By values, not rowid: a rebuilt table's rows get new rowids, and a count per table alone
/// misses a new orphan next to an old one.
fn key_violations(c: &Connection) -> Result<HashMap<(String, String, String), u64>> {
    let mut stmt = c.prepare("PRAGMA foreign_key_check")?;
    let found = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = HashMap::new();
    for (table, rowid, parent, fk) in found {
        let values = match rowid {
            Some(rowid) => {
                let mut cols = c.prepare(&format!(
                    "SELECT \"from\" FROM pragma_foreign_key_list('{}') WHERE id = ?1 ORDER BY seq",
                    table.replace('\'', "''")
                ))?;
                let cols = cols
                    .query_map([fk], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let quoted: Vec<String> = cols
                    .iter()
                    .map(|col| format!("quote(\"{}\")", col.replace('"', "\"\"")))
                    .collect();
                c.query_row(
                    &format!(
                        "SELECT {} FROM \"{}\" WHERE rowid = ?1",
                        quoted.join(" || ',' || "),
                        table.replace('"', "\"\"")
                    ),
                    [rowid],
                    |r| r.get::<_, String>(0),
                )?
            }
            // A table without rowids: counted with the others of its table.
            None => String::new(),
        };
        *out.entry((table, parent, values)).or_insert(0) += 1;
    }
    Ok(out)
}

/// The store shared by the engine's tasks. Each call takes the lock for one short statement or
/// transaction.
#[derive(Clone)]
pub struct SharedStore(Arc<Mutex<Store>>);

impl SharedStore {
    pub fn new(store: Store) -> SharedStore {
        SharedStore(Arc::new(Mutex::new(store)))
    }

    /// Locks the store. A panic in another holder does not make the data unusable.
    pub fn lock(&self) -> MutexGuard<'_, Store> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// SQLite integers are signed 64-bit; sizes above i64::MAX (8 EiB) do not occur.
pub(crate) fn to_i64(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

pub(crate) fn to_u64(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(0)
}

/// Stored like 1.1.4's Room enums, so a migrated database reads the same.
pub fn mode_str(m: wasl_core::SyncMode) -> &'static str {
    use wasl_core::SyncMode::*;
    match m {
        TwoWay => "TWO_WAY",
        UploadOnly => "UPLOAD_ONLY",
        UploadThenDelete => "UPLOAD_THEN_DELETE",
        UploadMirror => "UPLOAD_MIRROR",
        DownloadOnly => "DOWNLOAD_ONLY",
        DownloadThenDelete => "DOWNLOAD_THEN_DELETE",
        DownloadMirror => "DOWNLOAD_MIRROR",
    }
}

pub fn parse_mode(s: &str) -> Option<wasl_core::SyncMode> {
    wasl_core::SyncMode::ALL.into_iter().find(|m| mode_str(*m) == s)
}

pub fn policy_str(p: wasl_core::ConflictPolicy) -> &'static str {
    use wasl_core::ConflictPolicy::*;
    match p {
        KeepBoth => "KEEP_BOTH",
        NewerWins => "NEWER_WINS",
        LocalWins => "LOCAL_WINS",
        RemoteWins => "REMOTE_WINS",
        Skip => "SKIP",
    }
}

pub fn parse_policy(s: &str) -> Option<wasl_core::ConflictPolicy> {
    wasl_core::ConflictPolicy::ALL
        .into_iter()
        .find(|p| policy_str(*p) == s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn orphans() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        // As `migrate` runs a rebuild: keys unchecked until `apply` compares them.
        c.execute_batch(
            "PRAGMA foreign_keys = OFF;
             CREATE TABLE parent (id TEXT PRIMARY KEY);
             CREATE TABLE child (n INTEGER, parent_id TEXT REFERENCES parent(id));
             INSERT INTO child VALUES (1, 'gone');",
        )
        .unwrap();
        c
    }

    #[test]
    fn a_migration_that_adds_an_orphan_next_to_an_old_one_fails() {
        let mut c = orphans();
        let sql = "INSERT INTO child VALUES (2, 'also gone');";
        assert!(matches!(
            apply(&mut c, sql, 99, true),
            Err(StoreError::Corrupt(_))
        ));
    }

    #[test]
    fn a_migration_that_rebuilds_a_table_with_its_old_orphan_succeeds() {
        let mut c = orphans();
        let sql = "CREATE TABLE child2 (n INTEGER, parent_id TEXT REFERENCES parent(id));
                   INSERT INTO child2 SELECT n + 10, parent_id FROM child;
                   DROP TABLE child;
                   ALTER TABLE child2 RENAME TO child;";
        apply(&mut c, sql, 99, true).unwrap();
    }
}
