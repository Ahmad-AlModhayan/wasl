//! Folder pairs: their options (what the user set) and status (what the engine last saw).

use crate::{
    Result, SIDE_LOCAL, Store, StoreError, mode_str, parse_mode, parse_policy, policy_str, to_i64, to_u64,
};
use rusqlite::{OptionalExtension, Row, params};
use wasl_core::{DocsFormat, ErrorKind, Reach, Route, RunStatus};

pub use wasl_proto::store::{Pair, PairStatus, StorageMode};
const COLUMNS: &str =
    "id, name, account_id, local_root, storage_mode, drive_folder_id, remote_path, drive_id,
    route, reach, method, conflict_policy, enabled, paused, pause_reason, conditions_json,
    include_patterns, exclude_patterns, excluded_subfolders, sync_hidden, size_limit,
    propagate_deletions, mass_delete_guard, docs_export, docs_format, preserve_mtime, vault_id,
    gitignore, presets_json, interval_minutes, instant_sync, created_at, updated_at,
    last_run_at, last_success_at, last_status, last_error, last_error_kind, consecutive_failures,
    pending_approval, approve_once, hold_for_preview";

fn lines(v: &[String]) -> String {
    v.join("\n")
}

fn unlines(s: String) -> Vec<String> {
    s.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

fn corrupt(what: &str, value: &str) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(StoreError::Corrupt(format!("{what} {value:?}"))))
}

fn from_row(r: &Row<'_>) -> rusqlite::Result<Pair> {
    let storage: String = r.get(4)?;
    let route: String = r.get(8)?;
    let reach: String = r.get(9)?;
    let method: String = r.get(10)?;
    let policy: String = r.get(11)?;
    let conditions: String = r.get(15)?;
    let docs: String = r.get(24)?;
    let presets: String = r.get(28)?;
    let last_status: Option<String> = r.get(35)?;
    let last_kind: Option<String> = r.get(37)?;
    Ok(Pair {
        id: r.get(0)?,
        name: r.get(1)?,
        account_id: r.get(2)?,
        local_root: r.get(3)?,
        storage_mode: if storage == "saf" {
            StorageMode::Saf
        } else {
            StorageMode::Files
        },
        drive_folder_id: r.get(5)?,
        remote_path: r.get(6)?,
        drive_id: r.get(7)?,
        route: match route.as_str() {
            "drive" => Route::Drive,
            "drive_and_devices" => Route::DriveAndDevices,
            "devices" => Route::Devices,
            other => return Err(corrupt("route", other)),
        },
        reach: if reach == "lan_and_internet" {
            Reach::LanAndInternet
        } else {
            Reach::Lan
        },
        method: parse_mode(&method).ok_or_else(|| corrupt("method", &method))?,
        conflict_policy: parse_policy(&policy).ok_or_else(|| corrupt("policy", &policy))?,
        enabled: r.get(12)?,
        paused: r.get(13)?,
        pause_reason: r.get(14)?,
        conditions: serde_json::from_str(&conditions).unwrap_or_default(),
        include_patterns: unlines(r.get(16)?),
        exclude_patterns: unlines(r.get(17)?),
        excluded_subfolders: unlines(r.get(18)?),
        sync_hidden: r.get(19)?,
        size_limit: to_u64(r.get(20)?),
        propagate_deletions: r.get(21)?,
        mass_delete_guard: r.get(22)?,
        docs_export: r.get(23)?,
        docs_format: if docs == "PDF" {
            DocsFormat::Pdf
        } else {
            DocsFormat::Office
        },
        preserve_mtime: r.get(25)?,
        vault_id: r.get(26)?,
        honor_gitignore: r.get(27)?,
        presets: serde_json::from_str(&presets).unwrap_or_default(),
        interval_minutes: r.get::<_, i64>(29)?.clamp(0, i64::from(u32::MAX)) as u32,
        instant_sync: r.get(30)?,
        created_at: r.get(31)?,
        updated_at: r.get(32)?,
        status: PairStatus {
            last_run_at: r.get(33)?,
            last_success_at: r.get(34)?,
            last_status: last_status.as_deref().and_then(RunStatus::parse),
            last_error: r.get(36)?,
            last_error_kind: last_kind.as_deref().and_then(ErrorKind::parse),
            consecutive_failures: r.get::<_, i64>(38)?.clamp(0, i64::from(u32::MAX)) as u32,
            pending_approval: r.get(39)?,
            approve_once: r.get(40)?,
            hold_for_preview: r.get(41)?,
        },
    })
}

/// Why a pair can't be removed or pointed elsewhere while an agent's changes on it wait.
pub const AGENT_CHANGES_PENDING: &str =
    "This pair has agent changes waiting for review. Accept them in the Wasl app or revert them first.";

/// Whether saving `after` over `before` points the pair at other files: another folder on this
/// device, another Google account, Drive folder or shared drive, or another vault. Such a pair
/// starts over like a new one: its sync state is dropped and it waits for its preview
/// (`Store::update_pair`), since comparing the new folder with the old one's baseline would read
/// every old file as deleted. A new name, method or schedule keeps the state.
pub fn repointed(before: &Pair, after: &Pair) -> bool {
    wasl_core::paths::comparable(&before.local_root) != wasl_core::paths::comparable(&after.local_root)
        || (
            &before.account_id,
            &before.drive_folder_id,
            &before.drive_id,
            &before.vault_id,
        ) != (
            &after.account_id,
            &after.drive_folder_id,
            &after.drive_id,
            &after.vault_id,
        )
}

impl Store {
    /// Creates a pair with its options and status.
    pub fn create_pair(&self, p: &Pair) -> Result<()> {
        self.ensure_account(p)?;
        self.conn.execute(
            &format!("INSERT INTO pairs ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32,
                ?33, ?34, ?35, ?36, ?37, ?38, ?39, ?40, ?41, ?42)"),
            rusqlite::params_from_iter(values(p)),
        )?;
        Ok(())
    }

    /// Saves a pair's options (everything but its status). Returns whether the save pointed the
    /// pair at other files ([`repointed`]): another folder on this device, another Drive folder or
    /// vault. Such a pair starts its sync state over in the same transaction: the baseline, this
    /// device's index for other devices and what it read of theirs, chunk records, the vault's
    /// version and orphan records, resumable uploads, open conflicts and a stopped plan all
    /// described the old files. Without that, the next run would compare the new folder with the
    /// old one's baseline and read every old file as deleted (#94). Its share with other devices
    /// goes too: they still hold the old folder, and a share kept under the same pair would merge
    /// their old files into the new folder once the preview is approved. Sharing again offers the
    /// new folder. The pair is held for its preview, like a new pair. History, the bin and the
    /// op-log are kept. The devices index keeps its sequence number, so other devices still read
    /// what this device indexes next; its old entries go without tombstones, so nothing is
    /// deleted on them.
    ///
    /// Every save goes through here, so no app can move a pair without the reset. A run in flight
    /// would write the old files' state back after it: the engine's `Engine::update_pair` refuses
    /// the move while the pair runs, and apps save through it.
    pub fn update_pair(&self, p: &Pair) -> Result<bool> {
        self.ensure_account(p)?;
        let tx = self.conn.unchecked_transaction()?;
        let before = tx
            .query_row(
                &format!("SELECT {COLUMNS} FROM pairs WHERE id = ?1"),
                [&p.id],
                from_row,
            )
            .optional()?;
        let moved = before.as_ref().is_some_and(|b| repointed(b, p));
        if !moved {
            save_options(&tx, p)?;
            tx.commit()?;
            return Ok(false);
        }
        let shared: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM pair_shares WHERE pair_id = ?1)",
            [&p.id],
            |r| r.get(0),
        )?;
        let mut p = p.clone();
        if shared && p.route != Route::Drive && p.drive_folder_id.is_some() {
            // Like stopping a share by hand: a pair that also syncs with Drive keeps doing that. A
            // device-only pair stays one (a pair with no Drive folder can't have the Drive route,
            // wasl-pairs): its preview lifts the hold and it waits to be shared again.
            p.route = Route::Drive;
        }
        save_options(&tx, &p)?;
        for table in [
            "file_state",
            "device_index",
            "peer_progress",
            "chunk_lists",
            "chunks",
            "vault_versions",
            "vault_orphans",
            "conflicts",
            "blocked_plans",
            "ignore_hits",
            "pair_shares",
            "pair_share_devices",
        ] {
            tx.execute(&format!("DELETE FROM {table} WHERE pair_id = ?1"), [&p.id])?;
        }
        // Session keys are `pair|key|md5` (wasl-engine's executor).
        tx.execute(
            "DELETE FROM upload_sessions WHERE substr(session_key, 1, length(?1) + 1) = ?1 || '|'",
            [&p.id],
        )?;
        tx.execute(
            "UPDATE pairs SET hold_for_preview = 1, pending_approval = 0, approve_once = 0 WHERE id = ?1",
            [&p.id],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Bytes in the pair's folder at its last sync: the sizes its baseline records for this
    /// device's side, or the devices index for a pair that only syncs with devices. Read from
    /// what the last run saw, not from a new walk, so every status answer can carry it. None
    /// before the first sync.
    pub fn folder_bytes(&self, pair_id: &str) -> Result<Option<u64>> {
        let (rows, bytes): (i64, i64) = self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(CASE WHEN is_dir = 0 THEN size END), 0)
             FROM file_state WHERE pair_id = ?1 AND side = ?2",
            params![pair_id, SIDE_LOCAL],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if rows > 0 {
            return Ok(Some(to_u64(bytes)));
        }
        let (rows, bytes): (i64, i64) = self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(CASE WHEN is_dir = 0 THEN local_size END), 0)
             FROM device_index WHERE pair_id = ?1 AND deleted = 0",
            [pair_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok((rows > 0).then(|| to_u64(bytes)))
    }

    /// Files the last run that carried out its plan (a success or a partial run) left alone on
    /// purpose: the planner's skips (names Drive or this device can't hold, files over the size
    /// limit, and the like). Runs that stopped, failed or were blocked never finished a plan and
    /// don't count. Files the ignore rules leave out never reach the plan and aren't counted.
    /// None before such a run, or when its stats can't be read.
    pub fn last_skipped(&self, pair_id: &str) -> Result<Option<u32>> {
        let stats: Option<String> = self
            .conn
            .query_row(
                "SELECT stats_json FROM runs WHERE pair_id = ?1 AND status IN ('SUCCESS', 'PARTIAL')
                 ORDER BY started_at DESC, rowid DESC LIMIT 1",
                [pair_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(stats.and_then(|s| {
            serde_json::from_str::<crate::RunStats>(&s)
                .ok()
                .map(|st| st.skipped)
        }))
    }

    /// A pair names its Google account; make sure the account row exists (its email may come later).
    fn ensure_account(&self, p: &Pair) -> Result<()> {
        if let Some(a) = &p.account_id {
            self.conn
                .execute("INSERT OR IGNORE INTO accounts (id, email) VALUES (?1, ?1)", [a])?;
        }
        Ok(())
    }

    pub fn get_pair(&self, id: &str) -> Result<Option<Pair>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM pairs WHERE id = ?1"),
                [id],
                from_row,
            )
            .optional()?)
    }

    pub fn list_pairs(&self) -> Result<Vec<Pair>> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {COLUMNS} FROM pairs ORDER BY created_at, name"))?;
        let rows = stmt.query_map([], from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Removes a pair and everything that belongs to it (baseline, runs, bin entries…), unless an
    /// agent's changeset on it is open or held: its rows would go with the pair, and a pair made
    /// again on the same folders would sync the agent's files with no hold (security review M1,
    /// #134). The check is part of the delete statement, so nothing can open a changeset between
    /// the two.
    pub fn delete_pair(&self, pair_id: &str) -> Result<()> {
        let deleted = self.conn.execute(
            "DELETE FROM pairs WHERE id = ?1 AND NOT EXISTS (
                 SELECT 1 FROM agent_changesets WHERE pair_id = ?1 AND status IN ('open', 'held'))",
            [pair_id],
        )?;
        if deleted == 0 && self.pair_has_agent_changes(pair_id)? {
            return Err(StoreError::Refused(AGENT_CHANGES_PENDING.into()));
        }
        Ok(())
    }

    /// Whether an agent's changeset on the pair is open or held.
    pub fn pair_has_agent_changes(&self, pair_id: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM agent_changesets
                             WHERE pair_id = ?1 AND status IN ('open', 'held'))",
            [pair_id],
            |r| r.get(0),
        )?)
    }

    pub fn set_paused(&self, pair_id: &str, paused: bool, reason: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE pairs SET paused = ?2, pause_reason = ?3 WHERE id = ?1",
            params![pair_id, paused, if paused { reason } else { None }],
        )?;
        Ok(())
    }

    /// Marks the pair's next run as approved, leaving the rest of its status as it is.
    pub fn set_approve_once(&self, pair_id: &str) -> Result<()> {
        self.conn
            .execute("UPDATE pairs SET approve_once = 1 WHERE id = ?1", [pair_id])?;
        Ok(())
    }

    /// Writes a pair's status (the engine's view after a run or a guard decision).
    /// Sets only a pair's route, for sharing and unsharing: no folder changes, so no reset.
    pub fn set_pair_route(&self, pair_id: &str, route: Route) -> Result<()> {
        self.conn.execute(
            "UPDATE pairs SET route = ?2 WHERE id = ?1",
            params![pair_id, route.as_str()],
        )?;
        Ok(())
    }

    pub fn set_pair_status(&self, pair_id: &str, s: &PairStatus) -> Result<()> {
        self.conn.execute(
            "UPDATE pairs SET last_run_at = ?2, last_success_at = ?3, last_status = ?4, last_error = ?5,
                last_error_kind = ?6, consecutive_failures = ?7, pending_approval = ?8, approve_once = ?9,
                hold_for_preview = ?10
             WHERE id = ?1",
            params![
                pair_id,
                s.last_run_at,
                s.last_success_at,
                s.last_status.map(RunStatus::as_str),
                s.last_error,
                s.last_error_kind.map(ErrorKind::as_str),
                s.consecutive_failures,
                s.pending_approval,
                s.approve_once,
                s.hold_for_preview,
            ],
        )?;
        Ok(())
    }

    /// Saves the plan a guard stopped, for the "Needs you" screen.
    pub fn put_blocked_plan(&self, pair_id: &str, reason: &str, summary_json: &str, at: i64) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO blocked_plans (pair_id, reason, summary_json, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![pair_id, reason, summary_json, at],
        )?;
        Ok(())
    }

    /// (reason, summary JSON, created at) of the stopped plan.
    pub fn blocked_plan(&self, pair_id: &str) -> Result<Option<(String, String, i64)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT reason, summary_json, created_at FROM blocked_plans WHERE pair_id = ?1",
                [pair_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?)
    }

    pub fn delete_blocked_plan(&self, pair_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM blocked_plans WHERE pair_id = ?1", [pair_id])?;
        Ok(())
    }
}

/// Writes the options columns of `pairs` (everything but the status) from `p`.
fn save_options(conn: &rusqlite::Connection, p: &Pair) -> Result<()> {
    let v = values(p);
    conn.execute(
        "UPDATE pairs SET name = ?2, account_id = ?3, local_root = ?4, storage_mode = ?5, drive_folder_id = ?6,
            remote_path = ?7, drive_id = ?8, route = ?9, reach = ?10, method = ?11, conflict_policy = ?12,
            enabled = ?13, paused = ?14, pause_reason = ?15, conditions_json = ?16, include_patterns = ?17,
            exclude_patterns = ?18, excluded_subfolders = ?19, sync_hidden = ?20, size_limit = ?21,
            propagate_deletions = ?22, mass_delete_guard = ?23, docs_export = ?24, docs_format = ?25,
            preserve_mtime = ?26, vault_id = ?27, gitignore = ?28, presets_json = ?29, interval_minutes = ?30,
            instant_sync = ?31, updated_at = ?33
         WHERE id = ?1",
        rusqlite::params_from_iter(v.into_iter().take(33)),
    )?;
    Ok(())
}

fn values(p: &Pair) -> Vec<rusqlite::types::Value> {
    use rusqlite::types::Value::{self, Integer, Null, Text};
    let text = |s: &str| Text(s.to_owned());
    let opt = |s: &Option<String>| s.clone().map_or(Null, Text);
    let int = |n: i64| Integer(n);
    let flag = |b: bool| Integer(i64::from(b));
    let opt_int = |n: Option<i64>| n.map_or(Null, Integer);
    let s = &p.status;
    let v: Vec<Value> = vec![
        text(&p.id),
        text(&p.name),
        opt(&p.account_id),
        text(&p.local_root),
        text(p.storage_mode.as_str()),
        opt(&p.drive_folder_id),
        text(&p.remote_path),
        opt(&p.drive_id),
        text(p.route.as_str()),
        text(p.reach.as_str()),
        text(mode_str(p.method)),
        text(policy_str(p.conflict_policy)),
        flag(p.enabled),
        flag(p.paused),
        opt(&p.pause_reason),
        Text(serde_json::to_string(&p.conditions).unwrap_or_else(|_| "{}".into())),
        Text(lines(&p.include_patterns)),
        Text(lines(&p.exclude_patterns)),
        Text(lines(&p.excluded_subfolders)),
        flag(p.sync_hidden),
        int(to_i64(p.size_limit)),
        flag(p.propagate_deletions),
        flag(p.mass_delete_guard),
        flag(p.docs_export),
        text(if p.docs_format == DocsFormat::Pdf {
            "PDF"
        } else {
            "OFFICE"
        }),
        flag(p.preserve_mtime),
        opt(&p.vault_id),
        flag(p.honor_gitignore),
        Text(serde_json::to_string(&p.presets).unwrap_or_else(|_| "[]".into())),
        int(i64::from(p.interval_minutes)),
        flag(p.instant_sync),
        int(p.created_at),
        int(p.updated_at),
        opt_int(s.last_run_at),
        opt_int(s.last_success_at),
        s.last_status.map_or(Null, |x| text(x.as_str())),
        opt(&s.last_error),
        s.last_error_kind.map_or(Null, |x| text(x.as_str())),
        int(i64::from(s.consecutive_failures)),
        flag(s.pending_approval),
        flag(s.approve_once),
        flag(s.hold_for_preview),
    ];
    v
}
