//! `libwasl` for Android (and any UniFFI caller): one [`WaslEngine`] object with the operations of
//! SRS FR-E4, and the interfaces the platform implements (tokens, device state, SAF, progress).
//! No sync decision is made outside the engine (FR-E3).

uniffi::setup_scaffolding!();

mod devices;
mod on_demand;
mod saf;
mod vaults;

pub use devices::{DeviceEvents, DeviceKeyStore, DevicesInit, WaslDevices};
pub use on_demand::{
    CacheUsage, DEFAULT_CACHE_LIMIT, OnDemandFile, OnDemandOrigin, OnDemandReader, OnDemandRemotes,
    VaultItem, VaultLocks,
};
pub use saf::{SafBridge, SafEntry};
pub use vaults::{NewVault, vault_v2_create, vault_v2_unlock, vault_version};

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use wasl_core::BaselineEntry;
use wasl_core::conditions::DeviceState;
use wasl_core::{PlanSummary, RunStatus};
use wasl_drive::{DriveClient, DriveClientConfig, DriveEndpoints, DriveError, DriveNodeCache, TokenProvider};
use wasl_engine::{
    ConflictChoice, ConflictDetails, ConflictUndo, DeletionReview, DriveRemote, Engine, EngineConfig,
    EngineError, Environment, FileVersions, Preview, Progress, Remote, RestoreResult, RunOutcome,
    SystemClock, Trigger, VersionRef,
};
use wasl_fs::{LocalFs, StdFs};
use wasl_store::{
    BinEntry, BinSide, ConflictRecord, NewBinEntry, Pair, PairEvent, PairTotals, RunEvent, RunRecord,
    SharedStore, StorageMode, Store,
};

/// Errors crossing the boundary. Kotlin implementations return them too.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum WaslError {
    #[error("{detail}")]
    Auth { detail: String },
    #[error("{detail}")]
    Network { detail: String },
    #[error("{detail}")]
    Permission { detail: String },
    #[error("{detail}")]
    NotFound { detail: String },
    /// A vault is locked on this device.
    #[error("{detail}")]
    Locked { detail: String },
    #[error("{detail}")]
    Io { detail: String },
    #[error("{detail}")]
    Other { detail: String },
    /// The pair breaks a pair rule (overlaps another pair, in Wasl's own folders, around a view,
    /// no folder or Drive folder chosen); `detail` says which, in English.
    #[error("{detail}")]
    PairRule { detail: String },
}

impl From<EngineError> for WaslError {
    fn from(e: EngineError) -> Self {
        use wasl_core::ErrorKind::*;
        let message = e.to_string();
        match e.kind() {
            Auth => WaslError::Auth { detail: message },
            Network | RateLimit | Server => WaslError::Network { detail: message },
            Permission => WaslError::Permission { detail: message },
            NotFound => WaslError::NotFound { detail: message },
            Locked => WaslError::Locked { detail: message },
            Io | Integrity | Quota => WaslError::Io { detail: message },
            Other => WaslError::Other { detail: message },
        }
    }
}

impl From<wasl_store::StoreError> for WaslError {
    fn from(e: wasl_store::StoreError) -> Self {
        WaslError::Io {
            detail: e.to_string(),
        }
    }
}

impl From<uniffi::UnexpectedUniFFICallbackError> for WaslError {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        WaslError::Other { detail: e.reason }
    }
}

/// OAuth access tokens; the platform owns sign-in (Play services on Android).
#[uniffi::export(with_foreign)]
pub trait TokenSource: Send + Sync {
    /// A valid access token for the account. Called from a background thread; may block.
    fn access_token(&self, account: String) -> Result<String, WaslError>;
    /// The token was rejected; the next call must fetch a fresh one.
    fn invalidate(&self, account: String);
}

/// What only the platform knows.
#[uniffi::export(with_foreign)]
pub trait Platform: Send + Sync {
    fn device_state(&self) -> DeviceState;
    /// The SAF bridge, for pairs in SAF mode. `None` where SAF does not exist.
    fn saf(&self) -> Option<Arc<dyn SafBridge>>;
    /// An unlocked vault: its manifest and keys. `None` when the vault is locked on this device.
    fn vault_access(&self, vault_id: String) -> Option<VaultAccess>;
}

/// What the engine needs to sync an encrypted pair. The keys come from the platform's keystore and
/// are never written by the engine.
#[derive(Clone, uniffi::Record)]
pub struct VaultAccess {
    /// The vault's `vault.json`.
    pub manifest_json: String,
    /// The key material: keyEnc || keyName, 64 bytes, for a v1 vault (docs/ENCRYPTION.md); the
    /// Argon2id output, 32 bytes, for a v2 vault (docs/ON_DEMAND.md §1.1).
    pub keys: Vec<u8>,
}

impl std::fmt::Debug for VaultAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VaultAccess { .. }")
    }
}

/// Receives live progress; called often from background threads.
#[uniffi::export(with_foreign)]
pub trait ProgressSink: Send + Sync {
    fn on_progress(&self, progress: Progress);
}

/// Where the engine keeps its data.
#[derive(Debug, Clone, uniffi::Record)]
pub struct EngineInit {
    /// The SQLite database file.
    pub db_path: String,
    /// Recycle bin for pairs on a plain file system (one folder per pair inside).
    pub bin_dir: String,
    /// Staging for downloads into SAF pairs (the app's cache).
    pub staging_dir: String,
    pub device_name: String,
    /// Drive endpoints; `None` uses Google's. Tests point these at a mock server.
    pub drive_base_url: Option<String>,
    pub drive_upload_url: Option<String>,
}

/// A stopped plan waiting for the user (FR-F3).
#[derive(Debug, Clone, uniffi::Record)]
pub struct BlockedPlan {
    pub reason: String,
    pub summary: Option<PlanSummary>,
    pub created_at: i64,
}

struct TokenBridge(Arc<dyn TokenSource>);

#[async_trait]
impl TokenProvider for TokenBridge {
    async fn access_token(&self, account: &str) -> Result<String, DriveError> {
        let src = self.0.clone();
        let account = account.to_owned();
        let r = tokio::task::spawn_blocking(move || src.access_token(account))
            .await
            .map_err(|e| DriveError::Unknown(e.to_string()))?;
        r.map_err(|e| match e {
            WaslError::Auth { detail: message } => DriveError::Auth {
                message,
                user_action_required: true,
            },
            WaslError::Network { detail: message } => DriveError::Network(message),
            other => DriveError::Unknown(other.to_string()),
        })
    }

    async fn invalidate(&self, account: &str) {
        let src = self.0.clone();
        let account = account.to_owned();
        let _ = tokio::task::spawn_blocking(move || src.invalidate(account)).await;
    }
}

struct Env {
    store: SharedStore,
    platform: Arc<dyn Platform>,
    init: EngineInit,
    client: Arc<DriveClient>,
    cache: Arc<DriveNodeCache>,
    remotes: Mutex<HashMap<String, Arc<DriveRemote>>>,
}

impl Env {
    /// The account's plain Drive remote, one per account.
    fn drive_remote(&self, account: &str) -> Arc<DriveRemote> {
        let mut map = self.remotes.lock().unwrap_or_else(|p| p.into_inner());
        map.entry(account.to_owned())
            .or_insert_with(|| {
                Arc::new(DriveRemote {
                    account: account.to_owned(),
                    client: self.client.clone(),
                    cache: self.cache.clone(),
                    sessions: Arc::new(self.store.clone()),
                    flagged: Default::default(),
                })
            })
            .clone()
    }
}

/// What an on-demand reader of the app reads through: the engine's Drive, the platform's vault
/// keys, and the platform's cheap lock check when it gives one.
struct AppRemotes {
    env: Arc<Env>,
    locks: Option<Arc<dyn VaultLocks>>,
}

impl OnDemandRemotes for AppRemotes {
    fn remote(&self, account: &str) -> Result<Arc<dyn Remote>, WaslError> {
        Ok(self.env.drive_remote(account))
    }

    fn source(&self, account: &str) -> Result<Arc<dyn wasl_vfs::Source>, WaslError> {
        Ok(Arc::new(wasl_engine::DriveSource::new(
            self.env.client.clone(),
            account,
        )))
    }

    fn vault_access(&self, vault_id: &str) -> Option<VaultAccess> {
        self.env.platform.vault_access(vault_id.to_owned())
    }

    fn vault_unlocked(&self, vault_id: &str) -> bool {
        match &self.locks {
            Some(l) => l.is_unlocked(vault_id.to_owned()),
            None => self.vault_access(vault_id).is_some(),
        }
    }
}

impl Environment for Env {
    fn local_fs(&self, pair: &Pair) -> wasl_engine::Result<Arc<dyn LocalFs>> {
        let bin = PathBuf::from(&self.init.bin_dir).join(&pair.id);
        match pair.storage_mode {
            StorageMode::Files => Ok(Arc::new(StdFs::new(&pair.local_root, bin))),
            StorageMode::Saf => {
                let bridge = self.platform.saf().ok_or_else(|| {
                    EngineError::Permission("This device has no Storage Access Framework".into())
                })?;
                Ok(Arc::new(saf::SafFs::new(
                    bridge,
                    pair.local_root.clone(),
                    PathBuf::from(&self.init.staging_dir).join(&pair.id),
                )))
            }
        }
    }

    fn remote(&self, pair: &Pair) -> wasl_engine::Result<Arc<dyn Remote>> {
        let account = pair
            .account_id
            .clone()
            .ok_or_else(|| EngineError::NotFound("This pair has no Google account".into()))?;
        let r = self.drive_remote(&account);
        let Some(vault_id) = &pair.vault_id else {
            // A plain pair never syncs a vault's folder, as on the computer (E2E 2026-10).
            let tmp = PathBuf::from(&self.init.staging_dir).join("plain").join(&pair.id);
            return Ok(Arc::new(wasl_engine::PlainGuard::new(r, tmp, pair.clone())));
        };
        let access = self.platform.vault_access(vault_id.clone()).ok_or_else(|| {
            EngineError::Locked("The vault is locked on this device. Unlock it to sync this pair.".into())
        })?;
        let tmp = PathBuf::from(&self.init.staging_dir).join("vault").join(&pair.id);
        let unusable =
            |e: &dyn std::fmt::Display| EngineError::Locked(format!("The vault's keys are unusable: {e}"));
        let invalid = |e: &dyn std::fmt::Display| {
            EngineError::Integrity(format!("The vault's manifest is invalid: {e}"))
        };
        if vaults::vault_version(access.manifest_json.clone()) == Some(2) {
            let ikm: [u8; 32] = access
                .keys
                .as_slice()
                .try_into()
                .map_err(|_| unusable(&"a v2 vault keeps 32 bytes"))?;
            let keys = wasl_crypto::v2::VaultKeysV2::from_ikm(&ikm);
            let manifest = wasl_crypto::v2::vault_json::VaultManifestV2::parse(&access.manifest_json)
                .map_err(|e| invalid(&e))?;
            return Ok(Arc::new(
                wasl_engine::VaultV2Remote::new(r, Arc::new(keys), tmp)
                    .with_manifest(manifest)
                    .with_store(self.store.clone(), pair.id.clone()),
            ));
        }
        let keys = wasl_crypto::VaultKeys::from_master(&access.keys).map_err(|e| unusable(&e))?;
        let manifest = wasl_crypto::VaultManifest::parse(&access.manifest_json).map_err(|e| invalid(&e))?;
        Ok(Arc::new(
            wasl_engine::VaultRemote::new(r, Arc::new(keys), tmp).with_manifest(manifest),
        ))
    }

    fn device_state(&self) -> DeviceState {
        self.platform.device_state()
    }
}

/// The engine, as Android sees it.
#[derive(uniffi::Object)]
pub struct WaslEngine {
    engine: Engine,
    env: Arc<Env>,
    /// The pair rules, shared with the desktop (ADR-0001).
    pairs: wasl_pairs::Pairs,
    bin_dir: PathBuf,
    /// On-demand readers by cache folder: one per folder, shared by every caller.
    readers: Mutex<HashMap<PathBuf, Arc<OnDemandReader>>>,
}

/// One run of Drive Sync's history with its events, for the import (FR-K2).
#[derive(Debug, Clone, uniffi::Record)]
pub struct ImportedRun {
    pub run: RunRecord,
    pub events: Vec<RunEvent>,
}

/// What an import call wrote; rows already there are not counted.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct ImportCounts {
    pub baseline: u32,
    pub runs: u32,
    pub conflicts: u32,
}

/// An id used as a folder name: one plain path component.
fn safe_id(id: &str) -> Result<(), WaslError> {
    let ok = !id.is_empty()
        && id != "."
        && id != ".."
        && !id
            .chars()
            .any(|c| matches!(c, '/' | '\\' | ':') || c.is_control())
        && id.len() <= 128;
    if ok {
        Ok(())
    } else {
        Err(WaslError::Permission {
            detail: format!("{id:?} is not a valid pair id"),
        })
    }
}

fn io(e: std::io::Error) -> WaslError {
    WaslError::Io {
        detail: e.to_string(),
    }
}

fn other(e: impl std::fmt::Display) -> WaslError {
    WaslError::Other {
        detail: e.to_string(),
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl WaslEngine {
    /// Opens (and migrates) the database and closes runs a killed process left open.
    #[uniffi::constructor]
    pub fn new(
        init: EngineInit,
        tokens: Arc<dyn TokenSource>,
        platform: Arc<dyn Platform>,
        progress: Option<Arc<dyn ProgressSink>>,
    ) -> Result<Arc<WaslEngine>, WaslError> {
        let store = SharedStore::new(Store::open(&init.db_path)?);
        let bin_dir = PathBuf::from(&init.bin_dir);
        let defaults = DriveEndpoints::default();
        let endpoints = DriveEndpoints {
            base: init.drive_base_url.clone().unwrap_or(defaults.base),
            upload: init.drive_upload_url.clone().unwrap_or(defaults.upload),
        };
        let client = Arc::new(if init.drive_base_url.is_some() {
            DriveClient::with_http(
                DriveClient::http_client_builder().build().map_err(other)?,
                Arc::new(TokenBridge(tokens)),
                DriveClientConfig {
                    endpoints,
                    ..DriveClientConfig::default()
                },
            )
        } else {
            DriveClient::new(Arc::new(TokenBridge(tokens))).map_err(other)?
        });
        let cache = Arc::new(DriveNodeCache::new(client.clone(), Arc::new(store.clone())));
        let env = Arc::new(Env {
            store: store.clone(),
            platform,
            init: init.clone(),
            client,
            cache,
            remotes: Mutex::new(HashMap::new()),
        });
        let listener = progress.map(|sink| -> wasl_engine::ProgressListener {
            Arc::new(move |p: &Progress| sink.on_progress(p.clone()))
        });
        let engine = Engine::new(
            store,
            env.clone(),
            Arc::new(SystemClock),
            EngineConfig {
                device_name: init.device_name,
                ..EngineConfig::default()
            },
            listener,
        );
        engine.recover()?;
        // The phone's own data: the database, the bin and the staging folder (the database's
        // folder is the app's private one, which a picked folder can't be).
        let pairs = wasl_pairs::Pairs::new(
            engine.store().clone(),
            Arc::new(PhonePairs {
                own: vec![
                    PathBuf::from(&init.db_path),
                    bin_dir.clone(),
                    PathBuf::from(&init.staging_dir),
                ],
            }),
        );
        Ok(Arc::new(WaslEngine {
            engine,
            env,
            pairs,
            bin_dir,
            readers: Mutex::new(HashMap::new()),
        }))
    }

    // ---------------------------------------------------------------- files on demand

    /// The on-demand reader keeping its cache in `cache_dir` (docs/ON_DEMAND.md §2.2): byte ranges
    /// of Drive and vault files through one bounded block cache. Asking again for the same folder
    /// returns the same reader, with `limit` applied. `locks` answers, before every vault read,
    /// whether the vault is still unlocked without unwrapping its keys; without it the reader asks
    /// the platform for the vault's keys each time.
    pub fn on_demand(
        &self,
        cache_dir: String,
        limit: u64,
        locks: Option<Arc<dyn VaultLocks>>,
    ) -> Result<Arc<OnDemandReader>, WaslError> {
        let dir = PathBuf::from(cache_dir);
        let mut readers = self.readers.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(r) = readers.get(&dir) {
            r.set_limit(limit);
            return Ok(r.clone());
        }
        let remotes = Arc::new(AppRemotes {
            env: self.env.clone(),
            locks,
        });
        let reader = OnDemandReader::open(&dir, limit, remotes)?;
        readers.insert(dir, reader.clone());
        Ok(reader)
    }

    // ---------------------------------------------------------------- import from Drive Sync 1.1

    /// Writes a pair's state from Drive Sync 1.1 (FR-K2, docs/MIGRATION.md): its baseline, history
    /// and open conflicts, so its first run in Wasl is incremental. A pair that already has a
    /// baseline keeps it, and runs and conflicts already there are skipped: importing twice changes
    /// nothing.
    pub fn import_pair_state(
        &self,
        pair_id: String,
        baseline: Vec<BaselineEntry>,
        runs: Vec<ImportedRun>,
        conflicts: Vec<ConflictRecord>,
    ) -> Result<ImportCounts, WaslError> {
        let mut s = self.engine.store().lock();
        if s.get_pair(&pair_id)?.is_none() {
            return Err(WaslError::NotFound {
                detail: format!("No pair {pair_id}"),
            });
        }
        safe_id(&pair_id)?;
        let mut counts = ImportCounts::default();
        if s.baseline_file_count(&pair_id)? == 0 && s.load_baseline(&pair_id)?.is_empty() {
            s.put_baselines(&pair_id, &baseline)?;
            counts.baseline = u32::try_from(baseline.len()).unwrap_or(u32::MAX);
        }
        for r in runs.iter().filter(|r| r.run.pair_id == pair_id) {
            if s.run(&r.run.id)?.is_some() {
                continue;
            }
            let run = &r.run;
            s.start_run(&run.id, &pair_id, &run.trigger, run.started_at)?;
            // A run Drive Sync never finished ended when the app stopped; it cannot resume here.
            let status = if run.status == RunStatus::Running {
                RunStatus::Failed
            } else {
                run.status
            };
            s.finish_run(
                &run.id,
                status,
                &run.stats,
                run.message.as_deref(),
                run.ended_at.unwrap_or(run.started_at),
            )?;
            s.append_events(&run.id, &r.events)?;
            counts.runs += 1;
        }
        for c in conflicts.iter().filter(|c| c.pair_id == pair_id && !c.resolved) {
            if s.conflict(&c.id)?.is_none() {
                s.add_conflict(c)?;
                counts.conflicts += 1;
            }
        }
        Ok(counts)
    }

    /// Puts one of Drive Sync's bin entries into Wasl's bin. A phone-side entry needs `source`, the
    /// file from the export, which moves into the pair's bin folder (the location is made here; the
    /// entry's own is ignored). A Drive-side entry points at Drive's bin and needs no file.
    /// Returns false when the entry was imported before.
    pub fn import_bin_entry(&self, entry: NewBinEntry, source: Option<String>) -> Result<bool, WaslError> {
        let s = self.engine.store().lock();
        if s.bin_entry(&entry.id)?.is_some() {
            return Ok(false);
        }
        // The file comes from outside the app: its ids name folders, so they must be plain names.
        let pair = s.get_pair(&entry.pair_id)?.ok_or_else(|| WaslError::NotFound {
            detail: format!("No pair {}", entry.pair_id),
        })?;
        safe_id(&pair.id)?;
        if entry.side == BinSide::Drive && entry.account_id != pair.account_id {
            return Err(WaslError::Permission {
                detail: "A Drive bin entry belongs to its pair's account".into(),
            });
        }
        let mut entry = entry;
        if entry.side == BinSide::Local {
            if pair.storage_mode == StorageMode::Saf {
                return Err(WaslError::Io {
                    detail: "Drive Sync bins hold phone-folder files only".into(),
                });
            }
            let source = source.ok_or_else(|| WaslError::NotFound {
                detail: format!("The export has no file for {}", entry.key),
            })?;
            let name: String = entry
                .key
                .rsplit('/')
                .next()
                .unwrap_or("file")
                .chars()
                .filter(|c| !matches!(c, '/' | '\\' | '\0'))
                .collect();
            let location = format!(
                "{}_import_{}",
                entry.deleted_at,
                if name.is_empty() { "file" } else { &name }
            );
            let dir = self.bin_dir.join(&pair.id);
            std::fs::create_dir_all(&dir).map_err(io)?;
            let target = dir.join(&location);
            // The export sits in the app's cache, usually another volume: copy, then remove.
            if std::fs::rename(&source, &target).is_err() {
                std::fs::copy(&source, &target).map_err(io)?;
                let _ = std::fs::remove_file(&source);
            }
            entry.location = location;
        } else {
            entry.location = String::new();
        }
        s.add_to_bin(&entry)?;
        Ok(true)
    }

    /// The engine's version, for Diagnostics.
    pub fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_owned()
    }

    // ---------------------------------------------------------------- pairs

    pub fn pairs(&self) -> Result<Vec<Pair>, WaslError> {
        Ok(self.engine.store().lock().list_pairs()?)
    }

    pub fn pair(&self, id: String) -> Result<Option<Pair>, WaslError> {
        Ok(self.engine.store().lock().get_pair(&id)?)
    }

    /// Records a new pair through the pair rules (ADR-0001): checked like on the desktop and held
    /// for its first preview. A broken rule is [`WaslError::PairRule`].
    pub fn create_pair(&self, pair: Pair) -> Result<(), WaslError> {
        right_away(self.pairs.create(pair))?.map(|_| ()).map_err(pair_err)
    }

    /// Saves the pair's options; its status stays the engine's. Another folder, Drive folder or
    /// vault starts the pair over and holds it for its preview, and is refused while the pair
    /// syncs (`Engine::update_pair`, the same path as the desktop's, #94).
    pub fn update_pair(&self, pair: Pair) -> Result<(), WaslError> {
        self.engine.update_pair(&pair)?;
        Ok(())
    }

    pub fn delete_pair(&self, id: String) -> Result<(), WaslError> {
        self.engine.cancel(&id);
        Ok(self.engine.store().lock().delete_pair(&id)?)
    }

    pub fn set_paused(&self, id: String, paused: bool, reason: Option<String>) -> Result<(), WaslError> {
        Ok(self
            .engine
            .store()
            .lock()
            .set_paused(&id, paused, reason.as_deref())?)
    }

    /// Records why a pair did not run (its conditions), in the user's language, for its card. The
    /// rest of its status stays.
    pub fn record_skip(&self, id: String, reason: String) -> Result<(), WaslError> {
        let s = self.engine.store().lock();
        if let Some(p) = s.get_pair(&id)? {
            let mut st = p.status;
            st.last_status = Some(RunStatus::Skipped);
            st.last_error = Some(reason);
            s.set_pair_status(&id, &st)?;
        }
        Ok(())
    }

    pub fn set_account_email(&self, account: String, email: String) -> Result<(), WaslError> {
        Ok(self.engine.store().lock().set_account_email(&account, &email)?)
    }

    // ---------------------------------------------------------------- running

    /// The dry run (FR-F1). Dropping the call (leaving the preview screen) stops its work.
    pub async fn preview(&self, id: String) -> Result<Preview, WaslError> {
        Ok(self.engine.preview(&id).await?)
    }

    /// Live progress of a pair's preview: its step and the files checked so far. Not a run, so
    /// it never appears in [`ProgressSink`].
    pub fn preview_progress(&self, id: String) -> Option<Progress> {
        self.engine.preview_progress(&id)
    }

    /// Stops a pair's preview; the preview call then fails as cancelled.
    pub fn cancel_preview(&self, id: String) {
        self.engine.cancel_preview(&id);
    }

    pub async fn sync(&self, id: String, trigger: Trigger) -> RunOutcome {
        self.engine.sync(&id, trigger).await
    }

    /// Runs every enabled pair in turn; returns each outcome.
    pub async fn sync_all(&self, trigger: Trigger) -> Result<Vec<RunOutcome>, WaslError> {
        let ids: Vec<String> = self
            .engine
            .store()
            .lock()
            .list_pairs()?
            .into_iter()
            .filter(|p| p.enabled)
            .map(|p| p.id)
            .collect();
        let mut out = Vec::new();
        for id in ids {
            out.push(self.engine.sync(&id, trigger).await);
        }
        Ok(out)
    }

    pub fn cancel(&self, id: String) {
        self.engine.cancel(&id);
    }

    pub fn cancel_all(&self) {
        self.engine.cancel_all();
    }

    pub fn is_running(&self, id: String) -> bool {
        self.engine.is_running(&id)
    }

    pub fn progress(&self, id: String) -> Option<Progress> {
        self.engine.progress(&id)
    }

    /// The user approved a stopped plan, or started from the preview.
    pub fn approve_once(&self, id: String) -> Result<(), WaslError> {
        Ok(self.engine.approve_once(&id)?)
    }

    pub fn blocked_plan(&self, id: String) -> Result<Option<BlockedPlan>, WaslError> {
        Ok(self
            .engine
            .store()
            .lock()
            .blocked_plan(&id)?
            .map(|(reason, json, created_at)| BlockedPlan {
                reason,
                summary: serde_json::from_str(&json).ok(),
                created_at,
            }))
    }

    // ---------------------------------------------------------------- history, bin, conflicts

    pub fn runs(&self, pair_id: Option<String>, limit: u32) -> Result<Vec<RunRecord>, WaslError> {
        Ok(self.engine.store().lock().runs(pair_id.as_deref(), limit)?)
    }

    pub fn run_events(&self, run_id: String) -> Result<Vec<RunEvent>, WaslError> {
        Ok(self.engine.store().lock().run_events(&run_id)?)
    }

    /// Deletes history older than `before` (epoch ms; FR-B6 keeps 90 days).
    pub fn prune_runs(&self, before: i64) -> Result<u32, WaslError> {
        Ok(self.engine.store().lock().prune_runs(before)? as u32)
    }

    pub fn bin_entries(&self, pair_id: Option<String>) -> Result<Vec<BinEntry>, WaslError> {
        Ok(self.engine.store().lock().bin_entries(pair_id.as_deref())?)
    }

    pub fn bin_versions(&self, pair_id: String, key: String) -> Result<Vec<BinEntry>, WaslError> {
        Ok(self.engine.store().lock().bin_versions(&pair_id, &key)?)
    }

    // ---------------------------------------------------------------- actions after a run

    /// Everything a stopped plan would delete ("Review deletions").
    pub async fn deletion_review(&self, pair_id: String) -> Result<DeletionReview, WaslError> {
        Ok(self.engine.deletion_review(&pair_id).await?)
    }

    /// Copies everything a stopped plan would delete back to the side that lost it.
    pub async fn restore_deletions(&self, pair_id: String) -> Result<u32, WaslError> {
        Ok(self.engine.restore_deletions(&pair_id).await?)
    }

    pub async fn conflict_details(&self, conflict_id: String) -> Result<Option<ConflictDetails>, WaslError> {
        Ok(self.engine.conflict_details(&conflict_id).await?)
    }

    pub async fn resolve_conflict(
        &self,
        conflict_id: String,
        choice: ConflictChoice,
    ) -> Result<ConflictUndo, WaslError> {
        Ok(self.engine.resolve_conflict(&conflict_id, choice).await?)
    }

    pub async fn undo_conflict(&self, undo: ConflictUndo) -> Result<(), WaslError> {
        Ok(self.engine.undo_conflict(&undo).await?)
    }

    pub async fn file_versions(&self, pair_id: String, key: String) -> Result<FileVersions, WaslError> {
        Ok(self.engine.file_versions(&pair_id, &key).await?)
    }

    pub async fn restore_version(
        &self,
        pair_id: String,
        key: String,
        version: VersionRef,
    ) -> Result<(), WaslError> {
        Ok(self.engine.restore_version(&pair_id, &key, &version).await?)
    }

    /// Returns the copy's local path, relative to the pair.
    pub async fn save_version_copy(
        &self,
        pair_id: String,
        key: String,
        version: VersionRef,
        at: i64,
    ) -> Result<String, WaslError> {
        Ok(self
            .engine
            .save_version_copy(&pair_id, &key, &version, at)
            .await?)
    }

    pub async fn keep_revision(
        &self,
        pair_id: String,
        key: String,
        revision_id: String,
        keep: bool,
    ) -> Result<(), WaslError> {
        Ok(self
            .engine
            .keep_revision(&pair_id, &key, &revision_id, keep)
            .await?)
    }

    pub async fn restore_bin_entries(&self, ids: Vec<String>) -> Result<RestoreResult, WaslError> {
        Ok(self.engine.restore_bin_entries(&ids).await?)
    }

    pub async fn undo_bin_restore(&self, restored: Vec<BinEntry>) -> Result<(), WaslError> {
        Ok(self.engine.undo_bin_restore(&restored).await?)
    }

    /// Irreversible.
    pub async fn delete_bin_forever(&self, ids: Vec<String>) -> Result<u32, WaslError> {
        Ok(self.engine.delete_bin_forever(&ids).await?)
    }

    /// Irreversible.
    pub async fn empty_bin(&self) -> Result<u32, WaslError> {
        Ok(self.engine.empty_bin().await?)
    }

    pub async fn restore_from_bin(&self, entry_id: String) -> Result<(), WaslError> {
        Ok(self.engine.restore_from_bin(&entry_id).await?)
    }

    pub async fn purge_expired_bin(&self) -> Result<u32, WaslError> {
        Ok(self.engine.purge_expired_bin().await? as u32)
    }

    pub fn open_conflicts(&self, pair_id: Option<String>) -> Result<Vec<ConflictRecord>, WaslError> {
        Ok(self.engine.store().lock().open_conflicts(pair_id.as_deref())?)
    }

    // ---------------------------------------------------------------- screens

    /// The user's own decisions about a pair: approve the next run once, hold it for the preview,
    /// or clear a stop. `None` leaves a flag as it is.
    pub fn set_pair_flags(
        &self,
        id: String,
        approve_once: Option<bool>,
        pending_approval: Option<bool>,
        hold_for_preview: Option<bool>,
    ) -> Result<(), WaslError> {
        let s = self.engine.store().lock();
        if let Some(p) = s.get_pair(&id)? {
            let mut st = p.status;
            if let Some(v) = approve_once {
                st.approve_once = v;
            }
            if let Some(v) = pending_approval {
                st.pending_approval = v;
            }
            if let Some(v) = hold_for_preview {
                st.hold_for_preview = v;
            }
            s.set_pair_status(&id, &st)?;
        }
        Ok(())
    }

    /// Forgets a stopped plan (the user dealt with it another way).
    pub fn clear_blocked_plan(&self, id: String) -> Result<(), WaslError> {
        self.set_pair_flags(id.clone(), None, Some(false), None)?;
        Ok(self.engine.store().lock().delete_blocked_plan(&id)?)
    }

    pub fn baseline(&self, pair_id: String) -> Result<Vec<BaselineEntry>, WaslError> {
        Ok(self
            .engine
            .store()
            .lock()
            .load_baseline(&pair_id)?
            .into_values()
            .collect())
    }

    pub fn run(&self, run_id: String) -> Result<Option<RunRecord>, WaslError> {
        Ok(self.engine.store().lock().run(&run_id)?)
    }

    pub fn bin_entry(&self, id: String) -> Result<Option<BinEntry>, WaslError> {
        Ok(self.engine.store().lock().bin_entry(&id)?)
    }

    pub fn pair_totals(&self) -> Result<Vec<PairTotals>, WaslError> {
        Ok(self.engine.store().lock().pair_totals()?)
    }

    pub fn baseline_file_count(&self, pair_id: String) -> Result<u64, WaslError> {
        Ok(self.engine.store().lock().baseline_file_count(&pair_id)?)
    }

    pub fn baseline_entry(&self, pair_id: String, key: String) -> Result<Option<BaselineEntry>, WaslError> {
        Ok(self.engine.store().lock().baseline_entry(&pair_id, &key)?)
    }

    pub fn baseline_by_local_rel(
        &self,
        pair_id: String,
        local_rel: String,
    ) -> Result<Option<BaselineEntry>, WaslError> {
        Ok(self
            .engine
            .store()
            .lock()
            .baseline_by_local_rel(&pair_id, &local_rel)?)
    }

    /// Events newest first; for one pair or all, problems only or all, since a time or ever.
    pub fn events(
        &self,
        pair_id: Option<String>,
        problems_only: bool,
        since: Option<i64>,
        limit: u32,
    ) -> Result<Vec<PairEvent>, WaslError> {
        Ok(self
            .engine
            .store()
            .lock()
            .events(pair_id.as_deref(), problems_only, since, limit)?)
    }

    pub fn uploads_of(&self, pair_id: String, key: String, limit: u32) -> Result<Vec<PairEvent>, WaslError> {
        Ok(self.engine.store().lock().uploads_of(&pair_id, &key, limit)?)
    }

    pub fn runs_since(&self, since: i64) -> Result<Vec<RunRecord>, WaslError> {
        Ok(self.engine.store().lock().runs_since(since)?)
    }

    pub fn bin_total_size(&self) -> Result<u64, WaslError> {
        Ok(self.engine.store().lock().bin_total_size()?)
    }

    pub fn bin_entries_before(&self, before: i64) -> Result<Vec<BinEntry>, WaslError> {
        Ok(self.engine.store().lock().bin_entries_before(before)?)
    }

    pub fn conflict(&self, id: String) -> Result<Option<ConflictRecord>, WaslError> {
        Ok(self.engine.store().lock().conflict(&id)?)
    }

    pub fn unresolve_conflict(&self, id: String) -> Result<(), WaslError> {
        Ok(self.engine.store().lock().unresolve_conflict(&id)?)
    }

    pub fn account_auth_error(&self, account: String) -> Result<Option<String>, WaslError> {
        Ok(self.engine.store().lock().account_auth_error(&account)?)
    }

    // ---------------------------------------------------------------- settings

    pub fn setting(&self, key: String) -> Result<Option<String>, WaslError> {
        Ok(self.engine.store().lock().setting(&key)?)
    }

    pub fn set_setting(&self, key: String, value: String) -> Result<(), WaslError> {
        Ok(self.engine.store().lock().set_setting(&key, &value)?)
    }
}

/// True when the outcome means "nothing to worry about" (a skip or success), for notifications.
#[uniffi::export]
pub fn outcome_is_quiet(outcome: &RunOutcome) -> bool {
    matches!(outcome.status, RunStatus::Success | RunStatus::Skipped)
}

/// True when a device run synced nothing because no device that has the pair was connected
/// (`wasl_devices::NO_PEER`), so the app can word it in the person's language.
#[uniffi::export]
pub fn outcome_has_no_peer(outcome: &RunOutcome) -> bool {
    outcome.status == RunStatus::Skipped && outcome.message.as_deref() == Some(wasl_devices::NO_PEER)
}

/// The phone's side of the pair rules: where it keeps its own data. Vaults are linked by the app
/// before it saves a pair (moving into the module with ticket #38), so linking leaves it as is.
struct PhonePairs {
    own: Vec<PathBuf>,
}

impl wasl_pairs::PairsEnv for PhonePairs {
    fn own_dirs(&self) -> Vec<PathBuf> {
        self.own.clone()
    }
}

fn pair_err(e: wasl_pairs::PairError) -> WaslError {
    match e {
        wasl_pairs::PairError::Rule(p) => WaslError::PairRule {
            detail: p.to_string(),
        },
        wasl_pairs::PairError::Vault(e) => WaslError::Other {
            detail: e.to_string(),
        },
        wasl_pairs::PairError::Store(e) => e.into(),
    }
}

/// The result of a future that finishes without waiting, as the phone's pair operations do (its
/// [`PhonePairs`] links no vault, and the store answers at once). The binding's functions stay
/// plain calls for the app; one that would wait is an error, not a hang.
fn right_away<F: std::future::Future>(f: F) -> Result<F::Output, WaslError> {
    let mut f = std::pin::pin!(f);
    match f
        .as_mut()
        .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
    {
        std::task::Poll::Ready(out) => Ok(out),
        std::task::Poll::Pending => Err(WaslError::Other {
            detail: "The pair couldn't be saved right away. Try again.".into(),
        }),
    }
}
