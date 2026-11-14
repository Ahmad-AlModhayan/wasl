//! The daemon: one engine for this user, answering the CLI and the tray app (SRS FR-E4).

use crate::env::DesktopEnv;
use crate::oauth::{self, OAuthConfig};
use crate::tokens::{DesktopTokens, Secrets};
use crate::vaults::Vaults;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, broadcast};
use wasl_devices::{DeviceConfig, Devices};
use wasl_drive::{DriveClient, DriveClientConfig, DriveEndpoints, DriveError};
use wasl_engine::{Engine, EngineConfig, EngineError, Progress, ProgressListener, SystemClock, Trigger};
use wasl_ipc::{
    AcceptConfirmation, Account, DaemonStatus, DriveFolder, ErrorKind, Event, IgnoreExplanation, Overall,
    PROTOCOL_VERSION, PairState, PeerState, RemoteError, Request, ok,
};
use wasl_store::{Pair, SharedStore, Store};

pub struct DaemonConfig {
    pub data_dir: PathBuf,
    pub oauth: OAuthConfig,
    pub secrets: Arc<dyn Secrets>,
    /// Where unlocked vaults keep their keys: the OS keychain under its own service
    /// ([`crate::vaults::KEYCHAIN_SERVICE`]); a map in tests.
    pub vault_keys: Arc<dyn Secrets>,
    /// Drive endpoints for tests; `None` is Google.
    pub drive_endpoints: Option<DriveEndpoints>,
    pub open_browser: bool,
    pub device_name: String,
    pub sign_in_timeout: Duration,
    /// Device sync; `None` turns it off.
    pub devices: Option<DeviceConfig>,
    /// Where read-only views appear; `None` is this computer's own (Cloud Files, FUSE).
    pub view_backend: Option<Arc<dyn crate::views::Backend>>,
    /// How often a view reads its Drive folder again.
    pub view_refresh: Duration,
}

pub struct Daemon {
    pub engine: Engine,
    pub store: SharedStore,
    pub tokens: Arc<DesktopTokens>,
    pub client: Arc<DriveClient>,
    events: Arc<OnceLock<broadcast::Sender<Event>>>,
    pub shutdown: Notify,
    pub devices: Option<Arc<Devices>>,
    pub pairing: Option<Arc<wasl_devices::Pairing>>,
    /// Read-only on-demand views (docs/ON_DEMAND.md §2.3).
    pub views: Arc<crate::views::Views>,
    /// Encrypted vaults on this computer.
    pub vaults: Arc<Vaults>,
    env: Arc<DesktopEnv>,
    notifications: Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<String>>>,
    open_browser: bool,
    sign_in_timeout: Duration,
    /// The address of the Google sign-in that waits for its browser redirect, if one does. One sign-in at a
    /// time (#147 M4): another `add_account` while it waits gets the same address.
    signing_in: Arc<tokio::sync::Mutex<Option<String>>>,
    /// Folders no pair may be in or around: the data folder, and the runtime folder once
    /// [`Daemon::keep_out_of_pairs`] names it.
    own_dirs: Arc<Mutex<Vec<PathBuf>>>,
    /// The pair rules, shared with the phone (ADR-0001).
    pairs: wasl_pairs::Pairs,
    /// The agent accept waiting for the person's confirmation, if any (one at a time).
    confirming: Arc<Mutex<Option<Pending>>>,
    /// How long each changeset waits before its next confirmation (after a refusal).
    cooldowns: Mutex<Cooldowns>,
}

/// At most this many progress events per pair and second reach the clients.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
/// The most bytes an op-log answer may take as one line: a client's
/// [`wasl_ipc::transport::MAX_LINE`] less room for the frame around it.
const OPLOG_ANSWER_MAX: usize = wasl_ipc::transport::MAX_LINE - 1024;

impl Daemon {
    pub fn open(cfg: DaemonConfig) -> Result<Arc<Daemon>, String> {
        std::fs::create_dir_all(&cfg.data_dir).map_err(|e| e.to_string())?;
        let store = SharedStore::new(Store::open(cfg.data_dir.join("wasl.db")).map_err(|e| e.to_string())?);
        let tokens = Arc::new(DesktopTokens::new(cfg.oauth, cfg.secrets.clone(), store.clone()));
        let client = Arc::new(match cfg.drive_endpoints {
            Some(endpoints) => DriveClient::with_http(
                DriveClient::http_client_builder()
                    .build()
                    .map_err(|e| e.to_string())?,
                tokens.clone(),
                DriveClientConfig {
                    endpoints,
                    ..DriveClientConfig::default()
                },
            ),
            None => DriveClient::new(tokens.clone()).map_err(|e| e.to_string())?,
        });
        let vaults = Arc::new(Vaults::new(
            store.clone(),
            cfg.vault_keys.clone(),
            crate::vaults::tmp_dir(&cfg.data_dir),
        ));
        let env = Arc::new(DesktopEnv::new(
            store.clone(),
            client.clone(),
            cfg.data_dir.join("bin"),
            vaults.clone(),
        ));
        let events: Arc<OnceLock<broadcast::Sender<Event>>> = Arc::new(OnceLock::new());
        let (devices, notifications) = match &cfg.devices {
            Some(dc) => {
                let folders = env.clone();
                let (d, rx) = Devices::open(
                    &cfg.data_dir,
                    &crate::tokens::KeychainKeyStore(cfg.secrets.clone()),
                    store.clone(),
                    Arc::new(move |p: &Pair| {
                        use wasl_engine::Environment as _;
                        folders.local_fs(p)
                    }),
                    &cfg.device_name,
                    dc,
                )?;
                (Some(d), Some(rx))
            }
            None => (None, None),
        };

        if let Some(d) = &devices {
            // Arrivals and offers change on their own (docs/INTRODUCTIONS.md).
            let ev = events.clone();
            d.on_change(Arc::new(move || {
                if let Some(tx) = ev.get() {
                    let _ = tx.send(Event::Changed);
                }
            }));
        }
        let pairing = devices.as_ref().map(|d| {
            let ev = events.clone();
            Arc::new(wasl_devices::Pairing::new(
                d.clone(),
                &cfg.device_name,
                Arc::new(move || {
                    if let Some(tx) = ev.get() {
                        let _ = tx.send(Event::Changed);
                    }
                }),
            ))
        });
        let ev = events.clone();
        let drives = env.clone();
        let view_remotes = Arc::new(crate::vaults::VaultViews {
            vaults: vaults.clone(),
            drive: Arc::new(move |account: &str| -> Arc<dyn wasl_engine::Remote> {
                drives.drive_remote(account)
            }),
        });
        let views = Arc::new(crate::views::Views::new(
            store.clone(),
            &cfg.data_dir,
            view_remotes,
            cfg.view_backend
                .clone()
                .unwrap_or_else(crate::views::platform_backend),
            cfg.view_refresh,
            Arc::new(move || {
                if let Some(tx) = ev.get() {
                    let _ = tx.send(Event::Changed);
                }
            }),
        ));
        let last: Mutex<HashMap<String, Instant>> = Mutex::new(HashMap::new());
        let ev = events.clone();
        let listener: ProgressListener = Arc::new(move |p: &Progress| {
            let Some(tx) = ev.get() else { return };
            let now = Instant::now();
            let mut last = last.lock().unwrap_or_else(|p| p.into_inner());
            let due = last
                .get(&p.pair_id)
                .is_none_or(|t| now.duration_since(*t) >= PROGRESS_EVERY);
            if due {
                last.insert(p.pair_id.clone(), now);
                let _ = tx.send(Event::Progress { progress: p.clone() });
            }
        });
        let engine = Engine::new(
            store.clone(),
            env.clone(),
            Arc::new(SystemClock),
            EngineConfig {
                device_name: cfg.device_name,
                agent_dir: Some(cfg.data_dir.join("agent")),
                undo_dir: Some(cfg.data_dir.join("agent-undo")),
                // Agent changes are recorded in the op-log under this device's id (ADR-0002).
                device_id: devices.as_ref().map(|d| d.id().to_string()),
                ..EngineConfig::default()
            },
            Some(listener),
        );
        engine.recover().map_err(|e| e.to_string())?;
        // Not fatal: the copy is tried again at the next start, and the agent tables, which sync
        // and revert read, are untouched by it.
        if let Err(e) = engine.copy_agent_changesets_to_oplog() {
            eprintln!("wasld: copying agent changes into the op-log: {e}");
        }
        let own_dirs = Arc::new(Mutex::new(vec![cfg.data_dir.clone()]));
        let pairs = wasl_pairs::Pairs::new(
            store.clone(),
            Arc::new(DesktopPairs {
                store: store.clone(),
                env: env.clone(),
                vaults: vaults.clone(),
                own_dirs: own_dirs.clone(),
            }),
        );
        Ok(Arc::new(Daemon {
            engine,
            store,
            tokens,
            client,
            events,
            shutdown: Notify::new(),
            devices,
            pairing,
            views,
            vaults,
            env,
            notifications: Mutex::new(notifications),
            open_browser: cfg.open_browser,
            sign_in_timeout: cfg.sign_in_timeout,
            signing_in: Arc::new(tokio::sync::Mutex::new(None)),
            own_dirs,
            pairs,
            confirming: Arc::new(Mutex::new(None)),
            cooldowns: Mutex::new(Cooldowns::default()),
        }))
    }

    /// One more folder of the daemon's own (the socket's) that no pair may be in or around.
    pub fn keep_out_of_pairs(&self, dir: PathBuf) {
        let mut dirs = self.own_dirs.lock().unwrap_or_else(|p| p.into_inner());
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }

    /// Connects the event channel of the IPC server.
    pub fn attach_events(&self, tx: broadcast::Sender<Event>) {
        let _ = self.events.set(tx);
    }

    pub fn emit(&self, event: Event) {
        if let Some(tx) = self.events.get() {
            let _ = tx.send(event);
        }
    }

    /// Pairs devices said have changed (once: the daemon's server takes it).
    pub fn take_notifications(&self) -> Option<tokio::sync::mpsc::UnboundedReceiver<String>> {
        self.notifications
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
    }

    /// Runs one pair and tells the clients: with Drive, with devices, or both, by its route.
    pub async fn run(&self, pair_id: &str, trigger: Trigger) -> wasl_engine::RunOutcome {
        let pair = self.store.lock().get_pair(pair_id).ok().flatten();
        let route = pair.as_ref().map_or(wasl_core::Route::Drive, |p| p.route);
        let mut outcome = match route {
            wasl_core::Route::Devices => wasl_engine::RunOutcome {
                run_id: String::new(),
                status: wasl_core::RunStatus::Skipped,
                message: Some("Device sync is off on this computer.".into()),
                summary: None,
                error_kind: None,
                blocker: None,
            },
            _ => self.engine.sync(pair_id, trigger).await,
        };
        // A new pair synced with devices only (an accepted offer, docs/INTRODUCTIONS.md §3.3)
        // waits like any new pair: nothing moves until the person starts it from the preview.
        if route == wasl_core::Route::Devices
            && let Some(p) = &pair
            && p.status.hold_for_preview
        {
            if trigger != Trigger::Preview {
                outcome.message = Some(wasl_engine::HOLD_MESSAGE.into());
                self.emit(Event::Changed);
                return outcome;
            }
            // The preview lifts the hold only once the pair is shared: an unshared pair has
            // nothing to preview, and lifting it then would let the first run after sharing
            // move files with no preview (#94). `Devices::sync_pair` keeps the same rule for
            // callers that reach it directly; this check also covers device sync turned off.
            let s = self.store.lock();
            if s.share_id(&p.id).ok().flatten().is_none() {
                drop(s);
                outcome.message = Some(wasl_devices::NOT_SHARED.into());
                self.emit(Event::RunFinished {
                    pair_id: pair_id.to_owned(),
                    outcome: outcome.clone(),
                });
                self.emit(Event::Changed);
                return outcome;
            }
            let mut st = p.status.clone();
            st.hold_for_preview = false;
            let _ = s.set_pair_status(&p.id, &st);
        }
        if route != wasl_core::Route::Drive
            && let (Some(devices), Some(pair)) = (&self.devices, &pair)
            && pair.enabled
            && !pair.paused
        {
            let with_devices = devices.sync_pair(pair, trigger).await;
            outcome = if route == wasl_core::Route::Devices {
                with_devices
            } else {
                merge_outcomes(outcome, with_devices)
            };
        }
        self.emit(Event::RunFinished {
            pair_id: pair_id.to_owned(),
            outcome: outcome.clone(),
        });
        self.emit(Event::Changed);
        outcome
    }

    /// The pair whose folder holds `path` (the innermost, if pairs nest) and `path` relative to it.
    fn pair_at(&self, path: &str) -> Result<(Pair, String), RemoteError> {
        let p = std::path::Path::new(path);
        if !p.is_absolute() {
            return Err(invalid(format!("{path} is not a full path")));
        }
        self.store
            .lock()
            .list_pairs()
            .map_err(store_err)?
            .into_iter()
            .filter_map(|pair| {
                crate::views::relative(std::path::Path::new(&pair.local_root), p).map(|rel| (pair, rel))
            })
            .max_by_key(|(pair, _)| pair.local_root.len())
            .ok_or_else(|| RemoteError {
                kind: ErrorKind::NotFound,
                message: format!("{path} is not in a folder Wasl syncs"),
            })
    }

    /// A pair's folder cannot be in or around a view's folder (a view is read-only).
    fn check_not_a_view(&self, pair: &Pair) -> Result<(), RemoteError> {
        let root = std::path::Path::new(&pair.local_root);
        for v in self.store.lock().views().map_err(store_err)? {
            let view = std::path::Path::new(&v.local_path);
            if crate::views::relative(view, root).is_some() || crate::views::relative(root, view).is_some() {
                return Err(invalid(format!(
                    "{} is in or around the view {}, which is read-only. Choose another folder.",
                    pair.local_root, v.name
                )));
            }
        }
        Ok(())
    }

    /// A pair in or around the daemon's own folders would upload the database, the IPC token and
    /// staged files, and its downloads could overwrite them (security review 2026-10). Compared
    /// as given and, where the folders exist, with links and short names resolved.
    fn check_not_own_dirs(&self, pair: &Pair) -> Result<(), RemoteError> {
        let forms = |p: &std::path::Path| {
            let mut v = vec![p.to_path_buf()];
            if let Ok(c) = std::fs::canonicalize(p) {
                v.push(c);
            }
            v
        };
        let roots = forms(std::path::Path::new(&pair.local_root));
        let dirs = self.own_dirs.lock().unwrap_or_else(|p| p.into_inner()).clone();
        for dir in dirs {
            for own in forms(&dir) {
                for root in &roots {
                    if crate::views::relative(&own, root).is_some()
                        || crate::views::relative(root, &own).is_some()
                    {
                        return Err(invalid(format!(
                            "{} is in or around {}, where Wasl keeps its own data (its database and private key files). Choose another folder.",
                            pair.local_root,
                            dir.display()
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// Two pairs on the same files undo each other's work (one deletes what the other brings), so
    /// a pair's folder may be neither another pair's folder nor inside or around it.
    fn check_no_overlap(&self, pair: &Pair) -> Result<(), RemoteError> {
        let root = std::path::Path::new(&pair.local_root);
        for other in self.store.lock().list_pairs().map_err(store_err)? {
            if other.id == pair.id {
                continue;
            }
            let theirs = std::path::Path::new(&other.local_root);
            if crate::views::relative(theirs, root).is_some()
                || crate::views::relative(root, theirs).is_some()
            {
                return Err(invalid(format!(
                    "{} is already synced by the pair {}, or is in or around its folder. Two pairs on the same files can undo each other. Choose another folder.",
                    pair.local_root, other.name
                )));
            }
        }
        Ok(())
    }

    /// A pair on a vault's folder is linked to the vault before it is recorded; when that can't
    /// be checked the pair is refused (see `Vaults::link_pair`).
    async fn link_vault(&self, pair: &mut Pair) -> Result<(), RemoteError> {
        link_vault(&self.store, &self.env, &self.vaults, pair).await
    }

    /// A vault was unlocked: its views show their files and its pairs sync, in the background.
    fn vault_unlocked(self: &Arc<Self>, vault_id: &str) -> Result<(), RemoteError> {
        let waiting: Vec<String> = self
            .pairs()?
            .into_iter()
            .filter(|p| p.vault_id.as_deref() == Some(vault_id))
            .filter(|p| p.enabled && !p.paused && !p.status.hold_for_preview)
            .map(|p| p.id)
            .collect();
        let (me, id) = (self.clone(), vault_id.to_owned());
        tokio::spawn(async move {
            me.views.vault_unlocked(&id).await;
            for pair in waiting {
                me.run(&pair, Trigger::Instant).await;
            }
        });
        self.emit(Event::Changed);
        Ok(())
    }

    /// The Drive folder a new vault goes in: the one the client chose, or a folder named after
    /// the vault in `parent_id`, made when missing.
    async fn new_vault_folder(&self, new: &wasl_ipc::NewVault) -> Result<wasl_ipc::VaultFolder, RemoteError> {
        Vaults::check_new(&new.name, &new.passphrase)?;
        let name = new.name.trim();
        let folder_id = match new.folder_id.as_deref().map(str::trim) {
            Some(id) if !id.is_empty() => id.to_owned(),
            _ => {
                let existing = self
                    .drive_folders(&new.account_id, &new.parent_id)
                    .await?
                    .into_iter()
                    .find(|f| f.name == name);
                match existing {
                    Some(f) => f.id,
                    None => {
                        self.client
                            .create_folder(&new.account_id, &new.parent_id, name)
                            .await
                            .map_err(drive_err)?
                            .id
                    }
                }
            }
        };
        Ok(wasl_ipc::VaultFolder {
            account_id: new.account_id.clone(),
            folder_id,
            drive_id: new.drive_id.clone(),
            remote_path: new.remote_path.clone(),
        })
    }

    fn pairing_or_off(&self) -> Result<&Arc<wasl_devices::Pairing>, RemoteError> {
        self.pairing.as_ref().ok_or_else(|| RemoteError {
            kind: ErrorKind::Unsupported,
            message: "Device sync is off on this computer.".into(),
        })
    }

    fn devices_or_off(&self) -> Result<&Arc<Devices>, RemoteError> {
        self.devices.as_ref().ok_or_else(|| RemoteError {
            kind: ErrorKind::Unsupported,
            message: "Device sync is off on this computer.".into(),
        })
    }

    fn devices_info(&self) -> wasl_ipc::DevicesInfo {
        let Some(d) = &self.devices else {
            return wasl_ipc::DevicesInfo {
                enabled: false,
                listen: None,
                devices: Vec::new(),
            };
        };
        wasl_ipc::DevicesInfo {
            enabled: true,
            listen: d.local_addr().ok().map(|a| a.to_string()),
            devices: d.device_infos(),
        }
    }

    /// `page` with each origin device the device list knows named by it (ADR-0002, Read format).
    fn name_devices(&self, mut page: wasl_engine::OplogPage) -> wasl_engine::OplogPage {
        if let Some(d) = &self.devices {
            let infos = d.device_infos();
            page.name_devices(|id| infos.iter().find(|i| i.id == id).map(|i| i.name.clone()));
        }
        page
    }

    /// Makes a pair for `from`'s offer of `share` (docs/INTRODUCTIONS.md §3.3): synced with
    /// devices only, reach as any new device pair (LAN), under the offered share, and held for its
    /// first-sync preview like every new pair. A share some pair here already syncs is refused.
    fn accept_offer(
        &self,
        share: &str,
        from: &str,
        local_root: &str,
        name: Option<&str>,
    ) -> Result<Pair, RemoteError> {
        let d = self.devices_or_off()?;
        let from = device_id(from)?;
        let share = wasl_net::ShareId::from_hex(share.trim())
            .ok_or_else(|| invalid(format!("Not a share code: {share}")))?
            .to_hex();
        let offer = d
            .offer(&share, from)
            .ok_or_else(|| not_found(wasl_devices::NO_LONGER_OFFERED.into()))?;
        if self
            .store
            .lock()
            .pair_for_share(&share)
            .map_err(store_err)?
            .is_some()
        {
            return Err(invalid(
                "A pair on this computer already syncs that folder. Open that pair to change how it syncs."
                    .into(),
            ));
        }
        if !std::path::Path::new(local_root).is_absolute() {
            return Err(invalid(
                "Choose the folder on this computer by its full path".into(),
            ));
        }
        let name = name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or(&offer.name);
        let mut pair = Pair::new(&crate::views::new_id(), name, local_root);
        pair.route = wasl_core::Route::Devices;
        self.check_not_own_dirs(&pair)?;
        self.check_not_a_view(&pair)?;
        self.check_no_overlap(&pair)?;
        pair.status.hold_for_preview = true;
        // The folder the person picked; made when it isn't there yet.
        std::fs::create_dir_all(local_root).map_err(|e| invalid(format!("Can't use {local_root}: {e}")))?;
        self.store.lock().create_pair(&pair).map_err(store_err)?;
        // Shared with the offering device only (docs/INTRODUCTIONS.md §3.3).
        if let Err(e) = d.accept_offer(&pair.id, &share, from, None) {
            let _ = self.store.lock().delete_pair(&pair.id);
            return Err(invalid(e));
        }
        self.emit(Event::Changed);
        Ok(pair)
    }

    pub fn pairs(&self) -> Result<Vec<Pair>, RemoteError> {
        self.store.lock().list_pairs().map_err(store_err)
    }

    fn accounts(&self) -> Result<Vec<Account>, RemoteError> {
        Ok(self
            .store
            .lock()
            .accounts()
            .map_err(store_err)?
            .into_iter()
            .map(|(id, email, auth_error)| Account {
                id,
                email,
                auth_error,
            })
            .collect())
    }

    pub fn status(&self) -> Result<DaemonStatus, RemoteError> {
        let pairs = self.pairs()?;
        let devices = self
            .devices
            .as_ref()
            .map(|d| d.device_infos())
            .unwrap_or_default();
        let mut states = Vec::new();
        for pair in pairs {
            let (open_conflicts, folder_bytes, skipped) = {
                let s = self.store.lock();
                (
                    s.open_conflicts(Some(&pair.id)).map_err(store_err)?.len() as u32,
                    s.folder_bytes(&pair.id).map_err(store_err)?,
                    s.last_skipped(&pair.id).map_err(store_err)?,
                )
            };
            states.push(PairState {
                running: self.engine.is_running(&pair.id),
                progress: self.engine.progress(&pair.id),
                blocked: pair.status.pending_approval,
                open_conflicts,
                folder_bytes,
                skipped,
                peers: self.peers(&pair.id, &devices),
                free_bytes: crate::disk::free_bytes(&pair.local_root),
                pair,
            });
        }
        let accounts = self.accounts()?;
        let agent_changes = self
            .engine
            .agent_changesets(None, false)
            .map_err(engine_err)?
            .iter()
            .filter(|c| c.status == wasl_store::ChangesetStatus::Held)
            .count() as u32;
        Ok(DaemonStatus {
            version: env!("CARGO_PKG_VERSION").into(),
            protocol: PROTOCOL_VERSION,
            overall: overall(&states, &accounts, agent_changes),
            accounts,
            pairs: states,
            agent_changes,
        })
    }

    /// The other devices a pair is shared with (every known member for "All my devices"), by
    /// name, and whether each is connected now. Empty when the pair isn't shared or device sync
    /// is off.
    fn peers(&self, pair_id: &str, devices: &[wasl_devices::DeviceInfo]) -> Vec<PeerState> {
        let Some(sharing) = self
            .devices
            .as_ref()
            .and_then(|d| d.sharing(pair_id).ok().flatten())
        else {
            return Vec::new();
        };
        devices
            .iter()
            .filter(|i| !i.this_device && i.known)
            .filter(|i| sharing.all_devices || sharing.devices.contains(&i.id))
            .map(|i| PeerState {
                id: i.id.clone(),
                name: i.name.clone(),
                online: i.connected,
            })
            .collect()
    }

    async fn add_account(self: &Arc<Self>) -> Result<String, RemoteError> {
        // Held until the sign-in is recorded, so two calls can't both begin one.
        let mut signing_in = self.signing_in.lock().await;
        if let Some(url) = signing_in.as_ref() {
            return Ok(url.clone());
        }
        let pending = oauth::begin(&self.tokens.config).await.map_err(|e| RemoteError {
            kind: ErrorKind::Auth,
            message: e.to_string(),
        })?;
        let url = pending.url.clone();
        *signing_in = Some(url.clone());
        drop(signing_in);
        if self.open_browser {
            let _ = webbrowser::open(&url);
        }
        let me = self.clone();
        tokio::spawn(async move {
            let finished = pending.finish(me.tokens.http(), me.sign_in_timeout).await;
            // Over before the answer goes out: a window that hears it may start the next sign-in.
            *me.signing_in.lock().await = None;
            let event = match finished {
                Ok(signed) => match me.tokens.remember(&signed).await {
                    Ok(()) => Event::AccountAdded {
                        account: Account {
                            id: signed.email.clone(),
                            email: signed.email,
                            auth_error: None,
                        },
                    },
                    Err(message) => Event::AccountFailed { message },
                },
                Err(e) => Event::AccountFailed {
                    message: e.to_string(),
                },
            };
            me.emit(event);
            me.emit(Event::Changed);
        });
        Ok(url)
    }

    async fn drive_folders(&self, account: &str, parent: &str) -> Result<Vec<DriveFolder>, RemoteError> {
        let files = self
            .client
            .list_children(account, parent, None)
            .await
            .map_err(drive_err)?;
        let mut out: Vec<DriveFolder> = files
            .into_iter()
            .filter(|f| f.is_folder())
            .map(|f| DriveFolder {
                id: f.id,
                name: f.name,
                drive_id: f.drive_id,
            })
            .collect();
        out.sort_by_key(|f| f.name.to_lowercase());
        Ok(out)
    }

    /// "My Drive/Photos/2026" (or "/Photos/2026") to the folder, made on the way when `create`.
    async fn resolve_drive_path(
        &self,
        account: &str,
        path: &str,
        create: bool,
    ) -> Result<DriveFolder, RemoteError> {
        let trimmed = path.trim().trim_matches('/');
        let rest = trimmed
            .strip_prefix("My Drive")
            .unwrap_or(trimmed)
            .trim_matches('/');
        let mut current = DriveFolder {
            id: "root".into(),
            name: "My Drive".into(),
            drive_id: None,
        };
        for part in rest.split('/').filter(|p| !p.is_empty()) {
            let found = self
                .drive_folders(account, &current.id)
                .await?
                .into_iter()
                .find(|f| f.name == part);
            current = match found {
                Some(f) => f,
                None if create => {
                    let f = self
                        .client
                        .create_folder(account, &current.id, part)
                        .await
                        .map_err(drive_err)?;
                    DriveFolder {
                        id: f.id,
                        name: f.name,
                        drive_id: f.drive_id,
                    }
                }
                None => {
                    return Err(RemoteError {
                        kind: ErrorKind::NotFound,
                        message: format!("There is no folder \"{part}\" in {} on Drive", current.name),
                    });
                }
            };
        }
        Ok(current)
    }

    pub async fn handle(self: &Arc<Self>, request: Request) -> Result<serde_json::Value, RemoteError> {
        let e = &self.engine;
        match request {
            Request::Status => ok(self.status()?),
            Request::Pairs => ok(self.pairs()?),
            Request::Pair { id } => ok(self.store.lock().get_pair(&id).map_err(store_err)?),
            Request::CreatePair { pair } => {
                // The pair rules and the preview hold live in wasl-pairs (ADR-0001).
                self.pairs.create(pair).await.map_err(pair_err)?;
                self.emit(Event::Changed);
                ok(())
            }
            Request::UpdatePair { mut pair } => {
                check_pair(&pair)?;
                self.check_not_own_dirs(&pair)?;
                self.check_no_overlap(&pair)?;
                // A new Drive folder or vault is checked like a new pair's.
                let before = self.store.lock().get_pair(&pair.id).map_err(store_err)?;
                // Advisory, before `link_vault`'s remote call; `Engine::update_pair` checks again
                // under the lock it saves with (#134).
                if let Some(b) = &before
                    && wasl_store::repointed(b, &pair)
                    && self
                        .store
                        .lock()
                        .pair_has_agent_changes(&pair.id)
                        .map_err(store_err)?
                {
                    return Err(invalid(wasl_store::AGENT_CHANGES_PENDING.into()));
                }
                let moved = before.as_ref().is_none_or(|b| {
                    (&b.account_id, &b.drive_folder_id, &b.drive_id, &b.vault_id)
                        != (
                            &pair.account_id,
                            &pair.drive_folder_id,
                            &pair.drive_id,
                            &pair.vault_id,
                        )
                });
                if moved {
                    // Advisory, before the remote call: a move refused below would otherwise
                    // leave a vault linked on Drive. The real check is in `Engine::update_pair`.
                    if e.is_running(&pair.id) {
                        return Err(invalid(format!(
                            "{} is syncing. Stop it or let it finish, then change its folders.",
                            pair.name
                        )));
                    }
                    self.link_vault(&mut pair).await?;
                }
                // Other files reset the pair's sync state and hold it for its preview (#94). The
                // engine checks that the pair isn't running and saves under the lock a run takes
                // before it reads its pair, so a run can't start between the check and the save.
                e.update_pair(&pair).map_err(|err| match err {
                    EngineError::Other(message)
                    | EngineError::Store(wasl_store::StoreError::Refused(message)) => RemoteError {
                        kind: ErrorKind::Invalid,
                        message,
                    },
                    other => engine_err(other),
                })?;
                self.emit(Event::Changed);
                ok(())
            }
            Request::DeletePair { id } => {
                // The store refuses, in the delete itself, while an agent's changeset on the pair
                // is open or held (#134).
                let deleted = self.store.lock().delete_pair(&id);
                match deleted {
                    Err(wasl_store::StoreError::Refused(m)) => return Err(invalid(m)),
                    other => other.map_err(store_err)?,
                }
                // Only once it's gone: a refused delete leaves a running sync alone.
                e.cancel(&id);
                self.emit(Event::Changed);
                ok(())
            }
            Request::SetPaused { id, paused, reason } => {
                if paused {
                    e.cancel(&id);
                }
                self.store
                    .lock()
                    .set_paused(&id, paused, reason.as_deref())
                    .map_err(store_err)?;
                self.emit(Event::Changed);
                ok(())
            }
            Request::Preview { id } => ok(e.preview(&id).await.map_err(engine_err)?),
            Request::Sync { id, trigger } => ok(self.run(&id, trigger).await),
            Request::SyncAll { trigger } => {
                let ids: Vec<String> = self
                    .pairs()?
                    .into_iter()
                    .filter(|p| p.enabled && !p.paused)
                    .map(|p| p.id)
                    .collect();
                let mut out = Vec::new();
                for id in ids {
                    out.push(self.run(&id, trigger).await);
                }
                ok(out)
            }
            Request::Cancel { id } => {
                e.cancel(&id);
                ok(())
            }
            Request::ApproveOnce { id } => {
                e.approve_once(&id).map_err(engine_err)?;
                self.emit(Event::Changed);
                ok(())
            }
            Request::BlockedPlan { id } => {
                let plan = self.store.lock().blocked_plan(&id).map_err(store_err)?;
                ok(plan.map(|(reason, json, at)| {
                    serde_json::json!({
                        "reason": reason,
                        "summary": serde_json::from_str::<serde_json::Value>(&json).ok(),
                        "created_at": at,
                    })
                }))
            }
            Request::ClearBlockedPlan { id } => {
                let s = self.store.lock();
                if let Some(p) = s.get_pair(&id).map_err(store_err)? {
                    let mut st = p.status;
                    st.pending_approval = false;
                    s.set_pair_status(&id, &st).map_err(store_err)?;
                }
                s.delete_blocked_plan(&id).map_err(store_err)?;
                drop(s);
                self.emit(Event::Changed);
                ok(())
            }
            Request::Runs { pair_id, limit } => ok(self
                .store
                .lock()
                .runs(pair_id.as_deref(), limit.min(1000))
                .map_err(store_err)?),
            Request::RunEvents { run_id } => ok(self.store.lock().run_events(&run_id).map_err(store_err)?),
            Request::OpenConflicts { pair_id } => ok(self
                .store
                .lock()
                .open_conflicts(pair_id.as_deref())
                .map_err(store_err)?),
            Request::ConflictDetails { id } => ok(e.conflict_details(&id).await.map_err(engine_err)?),
            Request::ResolveConflict { id, choice } => {
                let undo = e.resolve_conflict(&id, choice).await.map_err(engine_err)?;
                self.emit(Event::Changed);
                ok(undo)
            }
            Request::UndoConflict { undo } => {
                e.undo_conflict(&undo).await.map_err(engine_err)?;
                self.emit(Event::Changed);
                ok(())
            }
            Request::DeletionReview { pair_id } => {
                ok(e.deletion_review(&pair_id).await.map_err(engine_err)?)
            }
            Request::RestoreDeletions { pair_id } => {
                let n = e.restore_deletions(&pair_id).await.map_err(engine_err)?;
                self.emit(Event::Changed);
                ok(n)
            }
            Request::FileVersions { pair_id, key } => {
                ok(e.file_versions(&pair_id, &key).await.map_err(engine_err)?)
            }
            Request::RestoreVersion {
                pair_id,
                key,
                version,
            } => {
                e.restore_version(&pair_id, &key, &version)
                    .await
                    .map_err(engine_err)?;
                self.emit(Event::Changed);
                ok(())
            }
            Request::BinEntries { pair_id } => ok(self
                .store
                .lock()
                .bin_entries(pair_id.as_deref())
                .map_err(store_err)?),
            Request::RestoreBin { ids } => {
                let r = e.restore_bin_entries(&ids).await.map_err(engine_err)?;
                self.emit(Event::Changed);
                ok(r)
            }
            Request::UndoBinRestore { restored } => {
                e.undo_bin_restore(&restored).await.map_err(engine_err)?;
                self.emit(Event::Changed);
                ok(())
            }
            Request::DeleteBinForever { ids } => {
                let n = e.delete_bin_forever(&ids).await.map_err(engine_err)?;
                self.emit(Event::Changed);
                ok(n)
            }
            Request::EmptyBin => {
                let n = e.empty_bin().await.map_err(engine_err)?;
                self.emit(Event::Changed);
                ok(n)
            }
            Request::AgentBegin {
                path,
                actor,
                label,
                session,
            } => {
                let (pair, mut rel) = self.pair_at(&path)?;
                if std::path::Path::new(&path).is_file() {
                    // A session works in a folder: the file's own.
                    rel = rel
                        .rsplit_once('/')
                        .map(|(d, _)| d.to_owned())
                        .unwrap_or_default();
                }
                let cs = e
                    .agent_begin(&pair.id, &rel, &actor, label.as_deref(), session.as_deref())
                    .await
                    .map_err(engine_err)?;
                self.emit(Event::Changed);
                ok(cs)
            }
            Request::AgentSave { path, session } => {
                // Hooks call this for every write; a file outside every pair is simply not kept.
                let Ok((pair, rel)) = self.pair_at(&path) else {
                    return ok(false);
                };
                ok(e.agent_save(&pair.id, &rel, session.as_deref())
                    .await
                    .map_err(engine_err)?)
            }
            Request::AgentEnd { id, session, path } => {
                let id = match (id, session, path) {
                    (Some(id), ..) => Some(id),
                    (None, Some(s), _) => self
                        .store
                        .lock()
                        .open_changeset_for_session(&s)
                        .map_err(store_err)?
                        .map(|c| c.id),
                    (None, None, Some(path)) => {
                        let (pair, rel) = self.pair_at(&path)?;
                        e.agent_changesets(Some(&pair.id), false)
                            .map_err(engine_err)?
                            .into_iter()
                            .find(|c| c.status == wasl_store::ChangesetStatus::Open && c.scope == rel)
                            .map(|c| c.id)
                    }
                    (None, None, None) => return Err(invalid("say which session to end".into())),
                };
                let Some(id) = id else {
                    // No open session: nothing to end (a hook may call twice).
                    return ok(Option::<wasl_store::Changeset>::None);
                };
                let cs = e.agent_end(&id).await.map_err(engine_err)?;
                self.emit(Event::Changed);
                ok(cs)
            }
            Request::AgentChangesets { pair_id, all } => {
                ok(e.agent_changesets(pair_id.as_deref(), all).map_err(engine_err)?)
            }
            Request::AgentChangeset { id } => ok(e.agent_changeset(&id).map_err(engine_err)?),
            Request::AgentJournal { id } => ok(e.agent_journal(&id).map_err(engine_err)?),
            Request::OplogList {
                actor,
                status,
                since,
                pair,
                limit,
            } => {
                let page = self.name_devices(
                    e.oplog_list(&wasl_store::OplogFilter {
                        actor,
                        status,
                        since,
                        pair,
                        limit: Some(limit.min(wasl_ipc::OPLOG_LIST_MAX)),
                    })
                    .map_err(engine_err)?,
                );
                let n = page.changesets.len();
                wasl_ipc::ok_within(page, OPLOG_ANSWER_MAX, |bytes| {
                    format!(
                        "{n} changesets are {bytes} bytes as one answer: more than the {OPLOG_ANSWER_MAX} bytes a client reads. Ask for fewer with a smaller limit."
                    )
                })
            }
            Request::OplogShow { origin_device, id } => {
                let page =
                    self.name_devices(e.oplog_show(origin_device.as_deref(), &id).map_err(engine_err)?);
                let ops = page
                    .changesets
                    .iter()
                    .flat_map(|c| c.operations.iter().flatten())
                    .count();
                wasl_ipc::ok_within(page, OPLOG_ANSWER_MAX, |bytes| {
                    format!(
                        "changeset {id} has {ops} operations, {bytes} bytes as one answer: more than the {OPLOG_ANSWER_MAX} bytes a client reads"
                    )
                })
            }
            Request::AgentDiff { id, rel } => ok(e.agent_diff(&id, &rel).await.map_err(engine_err)?),
            Request::AgentAccept { id, client, agent } => {
                // Only the person decides. A client that found an agent marker in its environment
                // is refused at once, with a clear message.
                if let Some(marker) = agent {
                    return Err(RemoteError {
                        kind: ErrorKind::Permission,
                        message: format!(
                            "Agents can't accept changes ({marker} is set). Accept in the Wasl app or your own terminal."
                        ),
                    });
                }
                // Everyone else gets a confirmation shown by the Wasl app, never an accept: any
                // program running as this user can speak this protocol and hide its markers, so
                // nothing in a request can prove a person asked (security review S3-1).
                ok(self
                    .ask_to_accept(&id, client.as_deref().unwrap_or("unknown"))
                    .await?)
            }
            Request::AgentRevert { id } => {
                let r = e.agent_revert(&id).await.map_err(engine_err)?;
                self.emit(Event::Changed);
                ok(r)
            }
            Request::AgentWatch { path } => {
                let own = self.own_dirs.lock().unwrap_or_else(|p| p.into_inner()).clone();
                let root =
                    wasl_engine::check_watch_root(std::path::Path::new(&path), &own).map_err(invalid)?;
                let w = e.agent_watch(&root).map_err(watch_err)?;
                self.emit(Event::Changed);
                ok(w)
            }
            Request::AgentUnwatch { watch } => {
                let w = e.agent_unwatch(&watch).map_err(watch_err)?;
                self.emit(Event::Changed);
                ok(w)
            }
            Request::AgentWatches => ok(e.agent_watches().map_err(engine_err)?),
            Request::AgentWatchIgnores { watch, ignores } => {
                let id = match e
                    .agent_watches()
                    .map_err(engine_err)?
                    .into_iter()
                    .find(|w| w.id == watch || w.root == watch)
                {
                    Some(w) => w.id,
                    None => watch,
                };
                e.agent_watch_ignores(&id, &ignores).map_err(watch_err)?;
                self.emit(Event::Changed);
                ok(())
            }
            Request::IgnoreExplain { pair_id, path } => {
                let is_dir = self
                    .store
                    .lock()
                    .get_pair(&pair_id)
                    .ok()
                    .flatten()
                    .map(|p| std::path::Path::new(&p.local_root).join(&path).is_dir())
                    .unwrap_or(false);
                let v = e
                    .explain_ignore(&pair_id, &path, is_dir)
                    .await
                    .map_err(engine_err)?;
                ok(IgnoreExplanation {
                    ignored: v.ignored,
                    rule: v.pattern,
                    source: v.source,
                    note: v.note,
                })
            }
            Request::Accounts => ok(self.accounts()?),
            Request::AddAccount => ok(self.add_account().await?),
            Request::RemoveAccount { id } => {
                let views = self.store.lock().views().map_err(store_err)?;
                if views.iter().any(|v| v.account_id == id) {
                    return Err(RemoteError {
                        kind: ErrorKind::Busy,
                        message: format!("{id} still has views. Remove them first."),
                    });
                }
                let removed = self.store.lock().remove_account(&id).map_err(store_err)?;
                if !removed {
                    return Err(RemoteError {
                        kind: ErrorKind::Busy,
                        message: format!("{id} still has pairs. Remove them first."),
                    });
                }
                self.tokens.forget(&id).await;
                self.emit(Event::Changed);
                ok(())
            }
            Request::DriveFolders { account, parent_id } => {
                ok(self.drive_folders(&account, &parent_id).await?)
            }
            Request::ResolveDrivePath {
                account,
                path,
                create,
            } => ok(self.resolve_drive_path(&account, &path, create).await?),
            Request::Devices => ok(self.devices_info()),
            Request::RemoveDevice { id } => {
                let d = self.devices_or_off()?;
                let id =
                    wasl_net::DeviceId::parse(&id).map_err(|_| invalid(format!("Not a device ID: {id}")))?;
                if id == d.id() {
                    return Err(invalid(
                        "To remove this computer, remove it from another of your devices.".into(),
                    ));
                }
                d.remove_member(id).map_err(invalid)?;
                self.emit(Event::Changed);
                ok(())
            }
            Request::ConnectDevice { address } => {
                let d = self.devices_or_off()?;
                let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host(address.as_str())
                    .await
                    .map_err(|e| invalid(format!("Can't find {address}: {e}")))?
                    .collect();
                let mut last = format!("Can't find {address}");
                for a in addrs {
                    match d.connect(a).await {
                        Ok(id) => {
                            self.emit(Event::Changed);
                            return ok(id.to_string());
                        }
                        Err(e) => last = e,
                    }
                }
                Err(RemoteError {
                    kind: ErrorKind::Network,
                    message: format!("Couldn't reach a device of your group at {address}: {last}"),
                })
            }
            Request::SharePair { id, share } => {
                self.devices_or_off()?;
                // Without a code, a pair already shared keeps its code: a new one would cut off
                // the devices using it.
                let existing = self.store.lock().share_id(&id).map_err(store_err)?;
                let share = match (share, existing) {
                    (Some(code), _) => wasl_net::ShareId::from_hex(code.trim())
                        .ok_or_else(|| invalid(format!("Not a share code: {code}")))?,
                    (None, Some(old)) => wasl_net::ShareId::from_hex(&old)
                        .ok_or_else(|| invalid("This pair's share code is damaged.".into()))?,
                    (None, None) => wasl_net::ShareId::random(),
                };
                let s = self.store.lock();
                let pair = s
                    .get_pair(&id)
                    .map_err(store_err)?
                    .ok_or_else(|| not_found(format!("No pair {id}")))?;
                if let Some(other) = s.pair_for_share(&share.to_hex()).map_err(store_err)?
                    && other != id
                {
                    return Err(invalid(
                        "Another pair on this computer already uses that share code.".into(),
                    ));
                }
                s.set_share_id(&id, &share.to_hex()).map_err(store_err)?;
                if pair.route == wasl_core::Route::Drive {
                    let route = if pair.drive_folder_id.is_some() {
                        wasl_core::Route::DriveAndDevices
                    } else {
                        wasl_core::Route::Devices
                    };
                    s.set_pair_route(&id, route).map_err(store_err)?;
                }
                drop(s);
                self.emit(Event::Changed);
                ok(share.to_hex())
            }
            Request::ShareCode { id } => ok(self.store.lock().share_id(&id).map_err(store_err)?),
            Request::UnsharePair { id } => {
                let s = self.store.lock();
                let pair = s
                    .get_pair(&id)
                    .map_err(store_err)?
                    .ok_or_else(|| not_found(format!("No pair {id}")))?;
                s.remove_share_id(&id).map_err(store_err)?;
                if pair.route != wasl_core::Route::Drive && pair.drive_folder_id.is_some() {
                    s.set_pair_route(&id, wasl_core::Route::Drive)
                        .map_err(store_err)?;
                }
                drop(s);
                self.emit(Event::Changed);
                ok(())
            }
            Request::Sharing { id } => match &self.devices {
                Some(d) => ok(d.sharing(&id).map_err(invalid)?),
                None => ok(Option::<wasl_ipc::PairSharing>::None),
            },
            Request::ShareWith { id, all, devices } => {
                let d = self.devices_or_off()?;
                let ids = devices
                    .iter()
                    .map(|s| device_id(s))
                    .collect::<Result<Vec<_>, _>>()?;
                d.share_with(&id, all, &ids).map_err(invalid)?;
                self.emit(Event::Changed);
                ok(())
            }
            Request::Offers => ok(self.devices.as_ref().map(|d| d.offers()).unwrap_or_default()),
            Request::DeclineOffer { share, from } => {
                let d = self.devices_or_off()?;
                d.decline_offer(&share, device_id(&from)?).map_err(invalid)?;
                self.emit(Event::Changed);
                ok(())
            }
            Request::AcceptOffer {
                share,
                from,
                local_root,
                name,
            } => ok(self.accept_offer(&share, &from, &local_root, name.as_deref())?),
            Request::KeepDevice { id } => {
                let d = self.devices_or_off()?;
                d.keep_device(device_id(&id)?).map_err(invalid)?;
                self.emit(Event::Changed);
                ok(())
            }
            Request::InviteDevice => ok(self.pairing_or_off()?.invite().map_err(invalid)?),
            Request::JoinDevice { code } => {
                self.pairing_or_off()?.join(&code).map_err(invalid)?;
                ok(())
            }
            Request::PairingStatus => ok(self.pairing_or_off()?.status()),
            Request::ConfirmPairing { accept } => {
                ok(self.pairing_or_off()?.confirm(accept).map_err(invalid)?)
            }
            Request::CancelPairing => {
                self.pairing_or_off()?.cancel();
                ok(())
            }
            Request::Views => ok(self.views.info()?),
            Request::AddView { view } => {
                let pairs = self.pairs()?;
                ok(self.views.add(view, &pairs).await?)
            }
            Request::RemoveView { id, keep_files } => ok(self.views.remove(&id, keep_files).await?),
            Request::SetViewEnabled { id, enabled } => ok(self.views.set_enabled(&id, enabled).await?),
            Request::SetViewCacheLimit { id, limit } => ok(self.views.set_cache_limit(&id, limit).await?),
            Request::RefreshView { id } => ok(self.views.refresh_now(&id).await?),
            Request::PinPath { path } => ok(self.views.set_pinned(&path, true).await?),
            Request::UnpinPath { path } => ok(self.views.set_pinned(&path, false).await?),
            // The File Provider extension asks on its own socket; answered here too, for tests
            // and for looking at what the extension sees.
            r @ (Request::ProviderItem { .. }
            | Request::ProviderList { .. }
            | Request::ProviderChanges { .. }
            | Request::ProviderFetch { .. }) => match self.views.provider() {
                Some(host) => crate::provider::answer(&host, &r)
                    .await
                    .expect("a File Provider request"),
                None => Err(RemoteError {
                    kind: ErrorKind::Unsupported,
                    message: "Views aren't File Provider domains on this computer.".into(),
                }),
            },
            Request::Vaults => ok(self.vaults.list()?),
            Request::CreateVault { vault } => {
                let folder = self.new_vault_folder(&vault).await?;
                let drive = self.env.drive_remote(&vault.account_id);
                let made = self
                    .vaults
                    .create(drive.as_ref(), folder, &vault.name, vault.passphrase)
                    .await?;
                self.emit(Event::Changed);
                ok(made)
            }
            Request::UnlockVaultFolder { folder, passphrase } => {
                let drive = self.env.drive_remote(&folder.account_id);
                let vault = self
                    .vaults
                    .unlock_folder(drive.as_ref(), folder, passphrase)
                    .await?;
                self.vault_unlocked(&vault.id)?;
                ok(vault)
            }
            Request::UnlockVault { id, passphrase } => {
                let vault = self.vaults.unlock(&id, passphrase).await?;
                self.vault_unlocked(&id)?;
                ok(vault)
            }
            Request::UpgradeVault {
                id,
                folder,
                passphrase,
            } => {
                let drive = self.env.drive_remote(&folder.account_id);
                let (vault, r) = self.vaults.upgrade(drive, &id, folder, passphrase).await?;
                self.emit(Event::Changed);
                ok(wasl_ipc::VaultUpgrade {
                    vault,
                    folders: r.folders,
                    files: r.files,
                    bytes: r.bytes,
                    already_there: r.already_there,
                    left_out: r.left_out,
                    failed: r.failed,
                })
            }
            Request::LockVault { id } => {
                self.vaults.lock(&id)?;
                // Runs in flight hold the keys until they end: stop them now.
                for p in self.pairs()? {
                    if p.vault_id.as_deref() == Some(id.as_str()) {
                        e.cancel(&p.id);
                    }
                }
                self.views.vault_locked(&id).await;
                self.emit(Event::Changed);
                ok(())
            }
            Request::Setting { key } => ok(self.store.lock().setting(&key).map_err(store_err)?),
            Request::SetSetting { key, value } => {
                self.store.lock().set_setting(&key, &value).map_err(store_err)?;
                self.emit(Event::Changed);
                ok(())
            }
            // Handled by the IPC server itself.
            Request::Subscribe => ok(()),
            Request::Shutdown => {
                e.cancel_all();
                self.emit(Event::ShuttingDown);
                self.shutdown.notify_one();
                ok(())
            }
        }
    }
}

/// The tray's one state (FR-B5), most urgent first.
/// Drive's outcome, with the device part's message added and a device failure counted as partial.
fn merge_outcomes(
    drive: wasl_engine::RunOutcome,
    devices: wasl_engine::RunOutcome,
) -> wasl_engine::RunOutcome {
    use wasl_core::RunStatus::*;
    let mut out = drive;
    if matches!(devices.status, Failed | Partial) && matches!(out.status, Success) {
        out.status = Partial;
    }
    if let Some(m) = devices.message {
        out.message = Some(match out.message.take() {
            Some(d) => format!("{d} Devices: {m}"),
            None => format!("Devices: {m}"),
        });
    }
    out
}

pub fn overall(pairs: &[PairState], accounts: &[Account], agent_changes: u32) -> Overall {
    if pairs.is_empty() {
        return Overall::Idle;
    }
    let needs_you = agent_changes > 0
        || accounts.iter().any(|a| a.auth_error.is_some())
        || pairs.iter().any(|p| {
            p.blocked
                || p.open_conflicts > 0
                || p.pair.status.last_status == Some(wasl_core::RunStatus::Failed)
        });
    if needs_you {
        return Overall::NeedsYou;
    }
    if pairs.iter().any(|p| p.running) {
        return Overall::Syncing;
    }
    let active: Vec<&PairState> = pairs.iter().filter(|p| p.pair.enabled).collect();
    if active.iter().all(|p| p.pair.paused) {
        return Overall::Paused;
    }
    let waiting = active.iter().any(|p| {
        !p.pair.paused
            && (p.pair.status.hold_for_preview
                || p.pair.status.last_status == Some(wasl_core::RunStatus::Skipped))
    });
    if waiting {
        return Overall::Waiting;
    }
    Overall::UpToDate
}

/// The daemon syncs folders by path; a pair it cannot run is refused up front.
/// A pair on a vault's folder is linked to the vault before it is recorded; when that can't be
/// checked the pair is refused (see `Vaults::link_pair`).
async fn link_vault(
    store: &SharedStore,
    env: &DesktopEnv,
    vaults: &Vaults,
    pair: &mut Pair,
) -> Result<(), RemoteError> {
    let Some(account) = pair.account_id.clone() else {
        return Ok(());
    };
    {
        // As the pair's own record does: the Drive cache needs the account's row.
        let s = store.lock();
        let known = s
            .accounts()
            .map_err(store_err)?
            .iter()
            .any(|(id, ..)| *id == account);
        if !known {
            s.set_account_email(&account, &account).map_err(store_err)?;
        }
    }
    let drive = env.drive_remote(&account);
    vaults.link_pair(drive.as_ref(), pair).await
}

/// The desktop's side of the pair rules (ADR-0001): its own data folders (the data folder, the
/// socket's), and linking a vault through the keychain-backed vaults.
struct DesktopPairs {
    store: SharedStore,
    env: Arc<DesktopEnv>,
    vaults: Arc<Vaults>,
    own_dirs: Arc<Mutex<Vec<PathBuf>>>,
}

#[async_trait::async_trait]
impl wasl_pairs::PairsEnv for DesktopPairs {
    fn own_dirs(&self) -> Vec<PathBuf> {
        self.own_dirs.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
    async fn link_vault(&self, pair: &mut Pair) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(link_vault(&self.store, &self.env, &self.vaults, pair).await?)
    }
}

/// A pair operation's error as the desktop has always answered it: a broken rule is `Invalid`
/// with the rule's English, a vault that couldn't be linked keeps its own kind.
fn pair_err(e: wasl_pairs::PairError) -> RemoteError {
    match e {
        wasl_pairs::PairError::Rule(wasl_pairs::Problem::NoFolder) => {
            invalid("Choose the folder on this computer by its full path".into())
        }
        wasl_pairs::PairError::Rule(p) => invalid(p.to_string()),
        wasl_pairs::PairError::Vault(e) => e
            .downcast_ref::<RemoteError>()
            .cloned()
            .unwrap_or_else(|| invalid(e.to_string())),
        wasl_pairs::PairError::Store(e) => store_err(e),
    }
}

fn check_pair(pair: &Pair) -> Result<(), RemoteError> {
    let invalid = |m: &str| {
        Err(RemoteError {
            kind: ErrorKind::Invalid,
            message: m.into(),
        })
    };
    if pair.id.is_empty() || pair.id.contains(['/', '\\', ':']) || pair.id == "." || pair.id == ".." {
        return invalid("A pair id is one plain name");
    }
    if !std::path::Path::new(&pair.local_root).is_absolute() {
        return invalid("Choose the folder on this computer by its full path");
    }
    if pair.drive_folder_id.as_deref().unwrap_or("").is_empty() || pair.account_id.is_none() {
        return invalid("Choose a Google account and a Drive folder");
    }
    Ok(())
}

fn invalid(message: String) -> RemoteError {
    RemoteError {
        kind: ErrorKind::Invalid,
        message,
    }
}

fn device_id(id: &str) -> Result<wasl_net::DeviceId, RemoteError> {
    wasl_net::DeviceId::parse(id).map_err(|_| invalid(format!("Not a device ID: {id}")))
}

fn not_found(message: String) -> RemoteError {
    RemoteError {
        kind: ErrorKind::NotFound,
        message,
    }
}

pub fn store_err(e: wasl_store::StoreError) -> RemoteError {
    RemoteError {
        kind: ErrorKind::Other,
        message: e.to_string(),
    }
}

/// A watch-list change the store turned down is the caller's to fix (`Invalid`), not a failure.
fn watch_err(e: EngineError) -> RemoteError {
    match e {
        EngineError::Store(wasl_store::StoreError::Refused(m)) => invalid(m),
        e => engine_err(e),
    }
}

pub fn engine_err(e: EngineError) -> RemoteError {
    let kind = match &e {
        EngineError::NotFound(_) => ErrorKind::NotFound,
        EngineError::Permission(_) => ErrorKind::Permission,
        EngineError::Locked(_) => ErrorKind::Locked,
        EngineError::Drive(d) => return drive_err_ref(d, e.to_string()),
        _ => ErrorKind::Other,
    };
    RemoteError {
        kind,
        message: e.to_string(),
    }
}

fn drive_err_ref(d: &DriveError, message: String) -> RemoteError {
    let kind = match d {
        DriveError::Auth { .. } => ErrorKind::Auth,
        DriveError::Network(_) => ErrorKind::Network,
        DriveError::NotFound(_) => ErrorKind::NotFound,
        _ => ErrorKind::Other,
    };
    RemoteError { kind, message }
}

pub fn drive_err(d: DriveError) -> RemoteError {
    let m = d.to_string();
    drive_err_ref(&d, m)
}

// ------------------------------------------------------------ accepting an agent's changes
//
// Security review S3-1. An agent's changeset is accepted only after the person confirms it in a
// dialog of the Wasl app. The daemon starts that dialog itself (the app's binary next to wasld,
// `--confirm-agent`), hands it a token minted for this one changeset through the child's stdin,
// and reads the answer from the child's stdout. The token is never written to disk, a command
// line, an environment variable or the IPC socket, so a program that speaks the protocol (the
// IPC token file is readable by every process of this user) can ask for an accept but cannot
// answer the confirmation: that takes the person's click in the dialog.
//
// What this does not stop (docs/AGENT_CHANGES.md §5): a program running as this user that
// clicks the dialog's button through the OS's accessibility or input APIs, reads the app's or
// the daemon's memory, replaces the app's binary, edits the database directly, or stops wasld
// and starts its own copy. Each is deliberate tampering that no check inside a user's own
// session can rule out; what is closed is the ordinary path, an agent calling
// `wasl agent accept` or speaking the IPC.

/// How long a confirmation waits for the person.
pub const CONFIRM_WINDOW: Duration = Duration::from_secs(120);

/// What the confirmation dialog shows, sent to it on its stdin (one JSON line). Its `Debug`
/// leaves the token out, so no log line or panic message can carry it.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ConfirmAsk {
    pub changeset_id: String,
    /// Proves the answer comes from the dialog the daemon started; echoed back when accepted.
    pub token: String,
    /// Also returned to the client that asked, so the person can match the two. Not a secret.
    pub code: String,
    /// The agent's name, without `agent:`.
    pub agent: String,
    pub label: Option<String>,
    pub pair_name: String,
    /// The scope inside the pair (empty: the whole pair).
    pub scope: String,
    pub items: usize,
    /// Which client asked: `cli`, `desktop`, `unknown`.
    pub asked_by: String,
    /// The language the person chose in Wasl (`en`, `ar`, `system`), if any.
    pub language: Option<String>,
    pub expires_in_secs: u64,
}

impl std::fmt::Debug for ConfirmAsk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfirmAsk")
            .field("changeset_id", &self.changeset_id)
            .field("token", &"<redacted>")
            .field("code", &self.code)
            .field("agent", &self.agent)
            .field("label", &self.label)
            .field("pair_name", &self.pair_name)
            .field("scope", &self.scope)
            .field("items", &self.items)
            .field("asked_by", &self.asked_by)
            .field("language", &self.language)
            .field("expires_in_secs", &self.expires_in_secs)
            .finish()
    }
}

/// The dialog's answer, one JSON line on its stdout. `Debug` leaves the token out.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConfirmAnswer {
    pub changeset_id: String,
    pub token: String,
    pub accept: bool,
}

impl std::fmt::Debug for ConfirmAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfirmAnswer")
            .field("changeset_id", &self.changeset_id)
            .field("token", &"<redacted>")
            .field("accept", &self.accept)
            .finish()
    }
}

/// Shows the confirmation to the person. The default starts the Wasl app's dialog; tests and
/// embedders may replace it ([`use_confirmer`]).
#[async_trait::async_trait]
pub trait Confirmer: Send + Sync {
    /// The person's answer; `None` when none came within `ask.expires_in_secs`.
    async fn ask(&self, ask: ConfirmAsk) -> Result<Option<ConfirmAnswer>, String>;
}

static CONFIRMER: OnceLock<Arc<dyn Confirmer>> = OnceLock::new();

/// Replaces how confirmations are shown, for this whole process (tests, embedders). Only the
/// first call counts; returns whether it did. Code inside the daemon's process can do anything
/// the daemon does, so this widens nothing.
pub fn use_confirmer(c: Arc<dyn Confirmer>) -> bool {
    CONFIRMER.set(c).is_ok()
}

/// The accept waiting for the person. `Debug` leaves the token out.
#[derive(Clone)]
pub struct Pending {
    pub changeset_id: String,
    token: String,
    pub code: String,
    pub expires: Instant,
}

impl std::fmt::Debug for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pending")
            .field("changeset_id", &self.changeset_id)
            .field("token", &"<redacted>")
            .field("code", &self.code)
            .field("expires", &self.expires)
            .finish()
    }
}

impl Pending {
    /// A fresh confirmation for one changeset: a 256-bit token and a 6-digit code.
    pub fn mint(changeset_id: &str, now: Instant) -> Result<Pending, String> {
        use base64::Engine as _;
        let mut t = [0u8; 32];
        getrandom::fill(&mut t).map_err(|e| e.to_string())?;
        let mut c = [0u8; 4];
        getrandom::fill(&mut c).map_err(|e| e.to_string())?;
        Ok(Pending {
            changeset_id: changeset_id.to_owned(),
            token: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(t),
            code: format!("{:06}", u32::from_le_bytes(c) % 1_000_000),
            expires: now + CONFIRM_WINDOW,
        })
    }

    /// The token the dialog must echo (tests of [`Pending::confirms`]).
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Whether `answer` confirms this changeset: an accept, for this changeset, with this token
    /// (compared in constant time), before the window closed.
    pub fn confirms(&self, answer: &ConfirmAnswer, now: Instant) -> bool {
        let same_token = answer.token.len() == self.token.len()
            && answer
                .token
                .bytes()
                .zip(self.token.bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0;
        same_token && answer.accept && answer.changeset_id == self.changeset_id && now < self.expires
    }
}

/// What to do with a new accept request while `current` may be waiting.
#[derive(Debug, PartialEq, Eq)]
pub enum Slot {
    /// Nothing waits (or it expired): mint a new confirmation.
    Free,
    /// This changeset already waits: answer with the same code, open no second dialog.
    Same { code: String, left: Duration },
    /// Another changeset waits. One dialog at a time, so a flood of requests cannot bury the
    /// person in dialogs (or wear them down into clicking one).
    Busy,
}

pub fn slot(current: Option<&Pending>, changeset_id: &str, now: Instant) -> Slot {
    match current {
        Some(p) if now < p.expires && p.changeset_id == changeset_id => Slot::Same {
            code: p.code.clone(),
            left: p.expires - now,
        },
        Some(p) if now < p.expires => Slot::Busy,
        _ => Slot::Free,
    }
}

/// The first wait after a changeset's confirmation is declined or times out (or answered with
/// anything but its token); each further refusal doubles it, up to [`COOLDOWN_MAX`].
pub const COOLDOWN_FIRST: Duration = Duration::from_secs(60);
/// The longest wait between two confirmations of one changeset.
pub const COOLDOWN_MAX: Duration = Duration::from_secs(30 * 60);
/// Most changesets whose waits are remembered (the oldest wait is forgotten first).
pub const COOLDOWNS_KEPT: usize = 256;

/// How long each changeset must wait before another confirmation is shown for it. Without it an
/// agent could ask again as soon as the person said no, and keep a dialog on screen until it is
/// clicked by mistake or out of weariness. This is a deterrent, not a boundary: a program that
/// can do that can also click the dialog itself (docs/AGENT_CHANGES.md §5).
#[derive(Debug, Default)]
pub struct Cooldowns {
    waits: HashMap<String, Cooldown>,
}

#[derive(Debug, Clone, Copy)]
struct Cooldown {
    until: Instant,
    last: Duration,
}

impl Cooldowns {
    /// How long `changeset_id` must still wait; `None` when a confirmation may be shown now.
    pub fn left(&self, changeset_id: &str, now: Instant) -> Option<Duration> {
        self.waits
            .get(changeset_id)
            .filter(|c| now < c.until)
            .map(|c| c.until - now)
    }

    /// The person declined, or did not answer: the next confirmation waits [`COOLDOWN_FIRST`],
    /// or twice as long as the last wait, up to [`COOLDOWN_MAX`]. Returns the wait.
    pub fn refused(&mut self, changeset_id: &str, now: Instant) -> Duration {
        // A changeset left alone for a long time after its wait ended starts over.
        self.waits
            .retain(|_, c| now.saturating_duration_since(c.until) < COOLDOWN_MAX);
        let wait = match self.waits.get(changeset_id) {
            Some(c) => (c.last * 2).min(COOLDOWN_MAX),
            None => COOLDOWN_FIRST,
        };
        if !self.waits.contains_key(changeset_id) && self.waits.len() >= COOLDOWNS_KEPT {
            let oldest = self
                .waits
                .iter()
                .min_by_key(|(_, c)| c.until)
                .map(|(id, _)| id.clone());
            if let Some(id) = oldest {
                self.waits.remove(&id);
            }
        }
        self.waits.insert(
            changeset_id.to_owned(),
            Cooldown {
                until: now + wait,
                last: wait,
            },
        );
        wait
    }

    /// Accepted: nothing to wait for any more.
    pub fn accepted(&mut self, changeset_id: &str) {
        self.waits.remove(changeset_id);
    }
}

/// The Wasl app's binary, which shows the dialog: next to wasld (as installed), else
/// `WASL_DESKTOP_APP` from wasld's own environment (development builds). Whoever can set
/// wasld's environment can start wasld themselves, so the variable widens nothing.
fn desktop_app() -> Option<PathBuf> {
    let name = format!("wasl-desktop{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join(&name)))
        .filter(|p| p.is_file())
        .or_else(|| {
            std::env::var_os("WASL_DESKTOP_APP")
                .map(PathBuf::from)
                .filter(|p| p.is_file())
        })
}

/// Starts the app's dialog and waits for its answer (blocking: run off the async threads).
fn ask_app(program: &std::path::Path, ask: &ConfirmAsk) -> Result<Option<ConfirmAnswer>, String> {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::{Command, Stdio};
    let mut child = Command::new(program)
        .arg("--confirm-agent")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("The Wasl app could not show the confirmation: {e}"))?;
    let mut line = serde_json::to_vec(ask).map_err(|e| e.to_string())?;
    line.push(b'\n');
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&line);
    }
    let out = child.stdout.take();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut answer = String::new();
        if let Some(out) = out {
            let _ = BufReader::new(out.take(4096)).read_line(&mut answer);
        }
        let _ = tx.send(answer);
    });
    let answer = rx.recv_timeout(Duration::from_secs(ask.expires_in_secs)).ok();
    // The dialog exits after answering; one still open when the window closed is closed too.
    let deadline = Instant::now() + Duration::from_secs(2);
    while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    Ok(answer.and_then(|a| serde_json::from_str(a.trim()).ok()))
}

/// The default [`Confirmer`]: the Wasl app's dialog.
struct AppDialog;

#[async_trait::async_trait]
impl Confirmer for AppDialog {
    async fn ask(&self, ask: ConfirmAsk) -> Result<Option<ConfirmAnswer>, String> {
        let program = desktop_app().ok_or_else(|| {
            "Accepting an agent's changes needs the Wasl app, which shows the confirmation, and it \
             isn't installed next to wasld."
                .to_owned()
        })?;
        tokio::task::spawn_blocking(move || ask_app(&program, &ask))
            .await
            .map_err(|e| e.to_string())?
    }
}

impl Daemon {
    /// Starts the confirmation of an accept (see above) and answers at once with its code. The
    /// changeset is accepted later, if and when the person confirms.
    async fn ask_to_accept(
        self: &Arc<Self>,
        id: &str,
        asked_by: &str,
    ) -> Result<AcceptConfirmation, RemoteError> {
        let details = self.engine.agent_changeset(id).map_err(engine_err)?;
        let cs = &details.changeset;
        if cs.status != wasl_store::ChangesetStatus::Held {
            return Err(invalid(
                "Only changes that wait for you can be accepted (a working session must end first).".into(),
            ));
        }
        if CONFIRMER.get().is_none() && desktop_app().is_none() {
            return Err(RemoteError {
                kind: ErrorKind::Unsupported,
                message: "Accepting an agent's changes needs the Wasl app, which shows the confirmation, \
                          and it isn't installed next to wasld. Install the Wasl app, then try again."
                    .into(),
            });
        }
        let now = Instant::now();
        let pending = {
            let mut current = self.confirming.lock().unwrap_or_else(|p| p.into_inner());
            match slot(current.as_ref(), &cs.id, now) {
                Slot::Same { code, left } => {
                    return Ok(AcceptConfirmation {
                        code,
                        expires_in_secs: left.as_secs().max(1),
                        retry_in_secs: None,
                    });
                }
                Slot::Busy => {
                    return Err(RemoteError {
                        kind: ErrorKind::Busy,
                        message: "Another agent's changes are waiting for you to confirm in the Wasl app. \
                                  Answer that first."
                            .into(),
                    });
                }
                Slot::Free => {}
            }
            // Declined or unanswered a moment ago: no new dialog yet, and the client is told how
            // long to wait.
            let left = self
                .cooldowns
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .left(&cs.id, now);
            if let Some(left) = left {
                return Ok(AcceptConfirmation {
                    code: String::new(),
                    expires_in_secs: 0,
                    retry_in_secs: Some((left.as_millis() as u64).div_ceil(1000).max(1)),
                });
            }
            let p = Pending::mint(&cs.id, now).map_err(|m| RemoteError {
                kind: ErrorKind::Other,
                message: m,
            })?;
            *current = Some(p.clone());
            p
        };
        let ask = ConfirmAsk {
            changeset_id: cs.id.clone(),
            token: pending.token.clone(),
            code: pending.code.clone(),
            agent: cs.actor.strip_prefix("agent:").unwrap_or(&cs.actor).to_owned(),
            label: cs.label.clone(),
            pair_name: details.pair_name.clone(),
            scope: cs.scope.clone(),
            items: details.entries.len(),
            asked_by: asked_by.to_owned(),
            language: self.store.lock().setting("language").ok().flatten(),
            expires_in_secs: CONFIRM_WINDOW.as_secs(),
        };
        let answer_to = AcceptConfirmation {
            code: pending.code.clone(),
            expires_in_secs: CONFIRM_WINDOW.as_secs(),
            retry_in_secs: None,
        };
        let me = self.clone();
        let by = asked_by.to_owned();
        let pair_id = cs.pair_id.clone();
        tokio::spawn(async move {
            let confirmer = CONFIRMER.get().cloned().unwrap_or_else(|| Arc::new(AppDialog));
            let answer =
                tokio::time::timeout(CONFIRM_WINDOW + Duration::from_secs(5), confirmer.ask(ask)).await;
            // The confirmation is used up whatever the answer: a second one finds nothing.
            let mine = {
                let mut current = me.confirming.lock().unwrap_or_else(|p| p.into_inner());
                if current.as_ref().is_some_and(|p| p.token == pending.token) {
                    current.take()
                } else {
                    None
                }
            };
            let confirmed = match (&mine, &answer) {
                (Some(p), Ok(Ok(Some(a)))) => p.confirms(a, Instant::now()),
                _ => false,
            };
            let accepted = confirmed
                && me
                    .engine
                    .agent_accept_by(&pending.changeset_id, Some(&by))
                    .is_ok();
            {
                // Declined, unanswered, or answered without the token: the next dialog for this
                // changeset waits. A dialog that could not be shown at all was nobody's answer.
                let shown = !matches!(answer, Ok(Err(_)));
                let mut cooldowns = me.cooldowns.lock().unwrap_or_else(|p| p.into_inner());
                if accepted {
                    cooldowns.accepted(&pending.changeset_id);
                } else if shown && !confirmed {
                    cooldowns.refused(&pending.changeset_id, Instant::now());
                }
            }
            me.emit(Event::Changed);
            if accepted && let Some(pair_id) = pair_id {
                // The agent's work goes out now.
                me.run(&pair_id, Trigger::Instant).await;
            }
        });
        Ok(answer_to)
    }
}

/// Lets the IPC server call the daemon.
pub struct Ipc(pub Arc<Daemon>);

#[async_trait::async_trait]
impl wasl_ipc::Handler for Ipc {
    async fn handle(&self, request: Request) -> Result<serde_json::Value, RemoteError> {
        self.0.handle(request).await
    }
}

#[cfg(test)]
mod confirm_tests {
    use super::*;

    fn answer(p: &Pending) -> ConfirmAnswer {
        ConfirmAnswer {
            changeset_id: p.changeset_id.clone(),
            token: p.token().to_owned(),
            accept: true,
        }
    }

    #[test]
    fn only_the_dialogs_own_answer_confirms_and_only_in_time() {
        let now = Instant::now();
        let p = Pending::mint("cs1", now).unwrap();
        assert_eq!(6, p.code.len());
        assert!(p.confirms(&answer(&p), now));
        assert!(p.confirms(&answer(&p), now + CONFIRM_WINDOW - Duration::from_secs(1)));
        assert!(!p.confirms(&answer(&p), now + CONFIRM_WINDOW), "expired");
        let mut a = answer(&p);
        a.accept = false;
        assert!(!p.confirms(&a, now), "declined");
        let mut a = answer(&p);
        a.changeset_id = "cs2".into();
        assert!(!p.confirms(&a, now), "bound to its changeset");
        let mut a = answer(&p);
        a.token.pop();
        a.token.push('x');
        assert!(!p.confirms(&a, now), "another token");
        let mut a = answer(&p);
        a.token = String::new();
        assert!(!p.confirms(&a, now), "no token");
        let other = Pending::mint("cs1", now).unwrap();
        assert!(
            !other.confirms(&answer(&p), now),
            "a token is minted per confirmation"
        );
    }

    #[test]
    fn one_confirmation_waits_at_a_time() {
        let now = Instant::now();
        assert_eq!(Slot::Free, slot(None, "cs1", now));
        let p = Pending::mint("cs1", now).unwrap();
        assert_eq!(
            Slot::Same {
                code: p.code.clone(),
                left: CONFIRM_WINDOW
            },
            slot(Some(&p), "cs1", now)
        );
        assert_eq!(Slot::Busy, slot(Some(&p), "cs2", now));
        assert_eq!(Slot::Free, slot(Some(&p), "cs2", now + CONFIRM_WINDOW));
        assert_eq!(Slot::Free, slot(Some(&p), "cs1", now + CONFIRM_WINDOW));
    }

    #[test]
    fn a_refused_confirmation_waits_longer_each_time_until_one_is_accepted() {
        let now = Instant::now();
        let mut c = Cooldowns::default();
        assert_eq!(None, c.left("cs1", now));
        assert_eq!(COOLDOWN_FIRST, c.refused("cs1", now));
        assert_eq!(Some(COOLDOWN_FIRST), c.left("cs1", now));
        assert_eq!(None, c.left("cs2", now), "per changeset");
        assert_eq!(None, c.left("cs1", now + COOLDOWN_FIRST));
        // Asked again as soon as it may, and refused again: twice as long, up to the cap.
        let mut t = now + COOLDOWN_FIRST;
        let mut waits = Vec::new();
        for _ in 0..8 {
            let w = c.refused("cs1", t);
            waits.push(w.as_secs());
            t += w;
        }
        assert_eq!(vec![120, 240, 480, 960, 1800, 1800, 1800, 1800], waits);
        // Accepted at last: the next changeset of that ID would start over.
        c.accepted("cs1");
        assert_eq!(None, c.left("cs1", t));
        assert_eq!(COOLDOWN_FIRST, c.refused("cs1", t));
        // A long quiet spell starts over too.
        let later = t + COOLDOWN_FIRST + COOLDOWN_MAX;
        assert_eq!(COOLDOWN_FIRST, c.refused("cs1", later));
    }

    #[test]
    fn the_cooldowns_stay_bounded() {
        let now = Instant::now();
        let mut c = Cooldowns::default();
        for n in 0..(COOLDOWNS_KEPT + 20) {
            c.refused(&format!("cs{n}"), now + Duration::from_millis(n as u64));
        }
        assert!(c.waits.len() <= COOLDOWNS_KEPT);
        assert!(
            c.left(&format!("cs{}", COOLDOWNS_KEPT + 19), now).is_some(),
            "the newest kept"
        );
    }

    #[test]
    fn the_token_never_shows_in_debug_output() {
        let now = Instant::now();
        let p = Pending::mint("cs1", now).unwrap();
        let token = p.token().to_owned();
        assert!(!format!("{p:?}").contains(&token));
        let a = answer(&p);
        assert!(!format!("{a:?}").contains(&token));
        let ask = ConfirmAsk {
            changeset_id: "cs1".into(),
            token: token.clone(),
            code: p.code.clone(),
            agent: "claude-code".into(),
            label: None,
            pair_name: "Work".into(),
            scope: String::new(),
            items: 1,
            asked_by: "cli".into(),
            language: None,
            expires_in_secs: 120,
        };
        let shown = format!("{ask:?}");
        assert!(
            !shown.contains(&token) && shown.contains("claude-code"),
            "{shown}"
        );
    }
}
