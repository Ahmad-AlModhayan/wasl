//! The client side, for the CLI and the tray app: typed calls plus an event stream.

use crate::auth::{self, ClientProof, DaemonProof, Hello, Opening, Token, Welcome};
use crate::paths::Paths;
use crate::proto::{
    AcceptConfirmation, Account, DaemonStatus, DevicesInfo, DriveFolder, ErrorKind, Event, Frame,
    IgnoreExplanation, NewVault, NewView, OfferInfo, PROTOCOL_VERSION, PairSharing, PairingStatus,
    Passphrase, RemoteError, Request, VaultFolder, VaultInfo, VaultUpgrade, View, ViewsInfo,
};
use crate::transport::{self, LineWriter};
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Mutex, mpsc, oneshot};
use wasl_engine::{
    ChangesetDetails, ConflictChoice, ConflictDetails, ConflictUndo, DeletionReview, FileVersions, Preview,
    RestoreResult, RevertReport, RunOutcome, Trigger, VersionRef,
};
use wasl_store::{BinEntry, Changeset, ConflictRecord, Pair, RunEvent, RunRecord};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The daemon is not running (or not reachable for this user).
    #[error("wasld is not running")]
    NotRunning,
    #[error("the daemon did not prove it is this user's wasld")]
    Untrusted,
    #[error("wasld speaks protocol {0}; this client speaks {PROTOCOL_VERSION}")]
    Version(u32),
    #[error(transparent)]
    Remote(#[from] RemoteError),
    #[error("connection to wasld lost: {0}")]
    Io(#[from] io::Error),
}

impl ClientError {
    pub fn kind(&self) -> Option<ErrorKind> {
        match self {
            ClientError::Remote(e) => Some(e.kind),
            _ => None,
        }
    }
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<serde_json::Value, RemoteError>>>>>;

/// One authenticated connection. Calls may run concurrently; events go to the receiver from
/// [`Client::subscribe`].
pub struct Client {
    writer: Mutex<LineWriter>,
    pending: Pending,
    next_id: AtomicU64,
    events: Mutex<Option<mpsc::UnboundedReceiver<Event>>>,
    pub daemon_version: String,
}

impl Client {
    pub async fn connect(paths: &Paths) -> Result<Client, ClientError> {
        let token = Token::read(&paths.token()).map_err(|_| ClientError::NotRunning)?;
        let stream = transport::connect(paths)
            .await
            .map_err(|_| ClientError::NotRunning)?;
        Client::handshake(stream, &token).await
    }

    async fn handshake(stream: transport::BoxStream, token: &Token) -> Result<Client, ClientError> {
        let (mut reader, mut writer) = transport::split(stream);
        let mine = auth::nonce()?;
        writer
            .write(&Opening::Hello(Hello {
                version: PROTOCOL_VERSION,
                nonce: mine.clone(),
            }))
            .await?;
        let theirs: DaemonProof = reader.read().await?.ok_or(ClientError::Untrusted)?;
        if !token.verify(auth::DAEMON_ROLE, &mine, &theirs.proof) {
            return Err(ClientError::Untrusted);
        }
        writer
            .write(&ClientProof {
                proof: token.proof(auth::CLIENT_ROLE, &theirs.nonce),
            })
            .await?;
        let welcome: Welcome = reader.read().await?.ok_or(ClientError::Untrusted)?;
        if welcome.protocol != PROTOCOL_VERSION {
            return Err(ClientError::Version(welcome.protocol));
        }

        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = mpsc::unbounded_channel();
        let p = pending.clone();
        tokio::spawn(async move {
            while let Ok(Some(frame)) = reader.read::<Frame>().await {
                match frame {
                    Frame::Ok { id, ok } => {
                        if let Some(s) = p.lock().await.remove(&id) {
                            let _ = s.send(Ok(ok));
                        }
                    }
                    Frame::Err { id, err } => {
                        if let Some(s) = p.lock().await.remove(&id) {
                            let _ = s.send(Err(err));
                        }
                    }
                    Frame::Event { event } => {
                        let _ = tx.send(event);
                    }
                    Frame::Request { .. } => {}
                }
            }
            // Connection gone: every waiting call fails.
            p.lock().await.clear();
        });
        Ok(Client {
            writer: Mutex::new(writer),
            pending,
            next_id: AtomicU64::new(1),
            events: Mutex::new(Some(rx)),
            daemon_version: welcome.version,
        })
    }

    /// Any request, with its result decoded as `T`.
    pub async fn call<T: DeserializeOwned>(&self, request: Request) -> Result<T, ClientError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        self.writer
            .lock()
            .await
            .write(&Frame::Request {
                id,
                request: Box::new(request),
            })
            .await?;
        let value = rx
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "wasld closed the connection"))??;
        serde_json::from_value(value)
            .map_err(|e| ClientError::Io(io::Error::new(io::ErrorKind::InvalidData, e)))
    }

    /// Starts the event stream; call once per connection.
    pub async fn subscribe(&self) -> Result<mpsc::UnboundedReceiver<Event>, ClientError> {
        self.call::<()>(Request::Subscribe).await?;
        self.events
            .lock()
            .await
            .take()
            .ok_or_else(|| ClientError::Io(io::Error::other("already subscribed")))
    }

    // ---------------------------------------------------------------- typed calls

    pub async fn status(&self) -> Result<DaemonStatus, ClientError> {
        self.call(Request::Status).await
    }
    pub async fn pairs(&self) -> Result<Vec<Pair>, ClientError> {
        self.call(Request::Pairs).await
    }
    pub async fn pair(&self, id: &str) -> Result<Option<Pair>, ClientError> {
        self.call(Request::Pair { id: id.into() }).await
    }
    pub async fn create_pair(&self, pair: Pair) -> Result<(), ClientError> {
        self.call(Request::CreatePair { pair }).await
    }
    pub async fn update_pair(&self, pair: Pair) -> Result<(), ClientError> {
        self.call(Request::UpdatePair { pair }).await
    }
    pub async fn delete_pair(&self, id: &str) -> Result<(), ClientError> {
        self.call(Request::DeletePair { id: id.into() }).await
    }
    pub async fn set_paused(
        &self,
        id: &str,
        paused: bool,
        reason: Option<String>,
    ) -> Result<(), ClientError> {
        self.call(Request::SetPaused {
            id: id.into(),
            paused,
            reason,
        })
        .await
    }
    pub async fn preview(&self, id: &str) -> Result<Preview, ClientError> {
        self.call(Request::Preview { id: id.into() }).await
    }
    pub async fn sync(&self, id: &str, trigger: Trigger) -> Result<RunOutcome, ClientError> {
        self.call(Request::Sync {
            id: id.into(),
            trigger,
        })
        .await
    }
    pub async fn sync_all(&self, trigger: Trigger) -> Result<Vec<RunOutcome>, ClientError> {
        self.call(Request::SyncAll { trigger }).await
    }
    pub async fn cancel(&self, id: &str) -> Result<(), ClientError> {
        self.call(Request::Cancel { id: id.into() }).await
    }
    pub async fn approve_once(&self, id: &str) -> Result<(), ClientError> {
        self.call(Request::ApproveOnce { id: id.into() }).await
    }
    pub async fn runs(&self, pair_id: Option<&str>, limit: u32) -> Result<Vec<RunRecord>, ClientError> {
        self.call(Request::Runs {
            pair_id: pair_id.map(Into::into),
            limit,
        })
        .await
    }
    pub async fn run_events(&self, run_id: &str) -> Result<Vec<RunEvent>, ClientError> {
        self.call(Request::RunEvents {
            run_id: run_id.into(),
        })
        .await
    }
    pub async fn open_conflicts(&self, pair_id: Option<&str>) -> Result<Vec<ConflictRecord>, ClientError> {
        self.call(Request::OpenConflicts {
            pair_id: pair_id.map(Into::into),
        })
        .await
    }
    pub async fn conflict_details(&self, id: &str) -> Result<Option<ConflictDetails>, ClientError> {
        self.call(Request::ConflictDetails { id: id.into() }).await
    }
    pub async fn resolve_conflict(
        &self,
        id: &str,
        choice: ConflictChoice,
    ) -> Result<ConflictUndo, ClientError> {
        self.call(Request::ResolveConflict {
            id: id.into(),
            choice,
        })
        .await
    }
    pub async fn undo_conflict(&self, undo: ConflictUndo) -> Result<(), ClientError> {
        self.call(Request::UndoConflict { undo }).await
    }
    pub async fn deletion_review(&self, pair_id: &str) -> Result<DeletionReview, ClientError> {
        self.call(Request::DeletionReview {
            pair_id: pair_id.into(),
        })
        .await
    }
    pub async fn restore_deletions(&self, pair_id: &str) -> Result<u32, ClientError> {
        self.call(Request::RestoreDeletions {
            pair_id: pair_id.into(),
        })
        .await
    }
    pub async fn file_versions(&self, pair_id: &str, key: &str) -> Result<FileVersions, ClientError> {
        self.call(Request::FileVersions {
            pair_id: pair_id.into(),
            key: key.into(),
        })
        .await
    }
    pub async fn restore_version(
        &self,
        pair_id: &str,
        key: &str,
        version: VersionRef,
    ) -> Result<(), ClientError> {
        self.call(Request::RestoreVersion {
            pair_id: pair_id.into(),
            key: key.into(),
            version,
        })
        .await
    }
    pub async fn agent_begin(
        &self,
        path: &str,
        actor: &str,
        label: Option<&str>,
        session: Option<&str>,
    ) -> Result<Changeset, ClientError> {
        self.call(Request::AgentBegin {
            path: path.into(),
            actor: actor.into(),
            label: label.map(Into::into),
            session: session.map(Into::into),
        })
        .await
    }
    pub async fn agent_save(&self, path: &str, session: Option<&str>) -> Result<bool, ClientError> {
        self.call(Request::AgentSave {
            path: path.into(),
            session: session.map(Into::into),
        })
        .await
    }
    pub async fn agent_end(
        &self,
        id: Option<&str>,
        session: Option<&str>,
        path: Option<&str>,
    ) -> Result<Option<Changeset>, ClientError> {
        self.call(Request::AgentEnd {
            id: id.map(Into::into),
            session: session.map(Into::into),
            path: path.map(Into::into),
        })
        .await
    }
    pub async fn agent_changesets(
        &self,
        pair_id: Option<&str>,
        all: bool,
    ) -> Result<Vec<Changeset>, ClientError> {
        self.call(Request::AgentChangesets {
            pair_id: pair_id.map(Into::into),
            all,
        })
        .await
    }
    pub async fn agent_changeset(&self, id: &str) -> Result<ChangesetDetails, ClientError> {
        self.call(Request::AgentChangeset { id: id.into() }).await
    }
    pub async fn agent_journal(&self, id: &str) -> Result<wasl_engine::JournalReport, ClientError> {
        self.call(Request::AgentJournal { id: id.into() }).await
    }
    /// The changesets `f` matches, newest first; `f.limit` `None` is as many as the daemon gives.
    pub async fn oplog_list(
        &self,
        f: &wasl_store::OplogFilter,
    ) -> Result<wasl_engine::OplogPage, ClientError> {
        self.call(Request::OplogList {
            actor: f.actor.clone(),
            status: f.status,
            since: f.since,
            pair: f.pair.clone(),
            limit: f.limit.unwrap_or(crate::proto::OPLOG_LIST_MAX),
        })
        .await
    }
    /// One changeset with its operations; `origin_device` `None` is the daemon's own device.
    pub async fn oplog_show(
        &self,
        origin_device: Option<&str>,
        id: &str,
    ) -> Result<wasl_engine::OplogPage, ClientError> {
        self.call(Request::OplogShow {
            origin_device: origin_device.map(Into::into),
            id: id.into(),
        })
        .await
    }
    pub async fn agent_diff(&self, id: &str, rel: &str) -> Result<String, ClientError> {
        self.call(Request::AgentDiff {
            id: id.into(),
            rel: rel.into(),
        })
        .await
    }
    pub async fn agent_accept(&self, id: &str) -> Result<AcceptConfirmation, ClientError> {
        self.agent_accept_as(id, None, None).await
    }
    /// Asks to accept, saying which client asks and the agent marker found in its environment,
    /// if any (the daemon refuses when there is one). Accepted only once the person confirms in
    /// the Wasl app's dialog, which shows the returned code.
    pub async fn agent_accept_as(
        &self,
        id: &str,
        client: Option<&str>,
        agent: Option<&str>,
    ) -> Result<AcceptConfirmation, ClientError> {
        self.call(Request::AgentAccept {
            id: id.into(),
            client: client.map(Into::into),
            agent: agent.map(Into::into),
        })
        .await
    }
    pub async fn agent_revert(&self, id: &str) -> Result<RevertReport, ClientError> {
        self.call(Request::AgentRevert { id: id.into() }).await
    }
    pub async fn agent_watch(&self, path: &str) -> Result<wasl_store::Watch, ClientError> {
        self.call(Request::AgentWatch { path: path.into() }).await
    }
    /// `watch` is a watch id or the folder's path.
    pub async fn agent_unwatch(&self, watch: &str) -> Result<wasl_store::Watch, ClientError> {
        self.call(Request::AgentUnwatch { watch: watch.into() }).await
    }
    pub async fn agent_watches(&self) -> Result<Vec<wasl_store::Watch>, ClientError> {
        self.call(Request::AgentWatches).await
    }
    pub async fn agent_watch_ignores(&self, watch: &str, ignores: Vec<String>) -> Result<(), ClientError> {
        self.call(Request::AgentWatchIgnores {
            watch: watch.into(),
            ignores,
        })
        .await
    }
    pub async fn bin_entries(&self, pair_id: Option<&str>) -> Result<Vec<BinEntry>, ClientError> {
        self.call(Request::BinEntries {
            pair_id: pair_id.map(Into::into),
        })
        .await
    }
    pub async fn restore_bin(&self, ids: Vec<String>) -> Result<RestoreResult, ClientError> {
        self.call(Request::RestoreBin { ids }).await
    }
    pub async fn delete_bin_forever(&self, ids: Vec<String>) -> Result<u32, ClientError> {
        self.call(Request::DeleteBinForever { ids }).await
    }
    pub async fn empty_bin(&self) -> Result<u32, ClientError> {
        self.call(Request::EmptyBin).await
    }
    pub async fn ignore_explain(&self, pair_id: &str, path: &str) -> Result<IgnoreExplanation, ClientError> {
        self.call(Request::IgnoreExplain {
            pair_id: pair_id.into(),
            path: path.into(),
        })
        .await
    }
    pub async fn devices(&self) -> Result<DevicesInfo, ClientError> {
        self.call(Request::Devices).await
    }
    pub async fn remove_device(&self, id: &str) -> Result<(), ClientError> {
        self.call(Request::RemoveDevice { id: id.into() }).await
    }
    /// Returns the ID of the device reached.
    pub async fn connect_device(&self, address: &str) -> Result<String, ClientError> {
        self.call(Request::ConnectDevice {
            address: address.into(),
        })
        .await
    }
    /// Returns the share code.
    pub async fn share_pair(&self, id: &str, share: Option<&str>) -> Result<String, ClientError> {
        self.call(Request::SharePair {
            id: id.into(),
            share: share.map(Into::into),
        })
        .await
    }
    pub async fn share_code(&self, id: &str) -> Result<Option<String>, ClientError> {
        self.call(Request::ShareCode { id: id.into() }).await
    }
    pub async fn unshare_pair(&self, id: &str) -> Result<(), ClientError> {
        self.call(Request::UnsharePair { id: id.into() }).await
    }
    pub async fn sharing(&self, id: &str) -> Result<Option<PairSharing>, ClientError> {
        self.call(Request::Sharing { id: id.into() }).await
    }
    pub async fn share_with(&self, id: &str, all: bool, devices: Vec<String>) -> Result<(), ClientError> {
        self.call(Request::ShareWith {
            id: id.into(),
            all,
            devices,
        })
        .await
    }
    pub async fn offers(&self) -> Result<Vec<OfferInfo>, ClientError> {
        self.call(Request::Offers).await
    }
    pub async fn decline_offer(&self, share: &str, from: &str) -> Result<(), ClientError> {
        self.call(Request::DeclineOffer {
            share: share.into(),
            from: from.into(),
        })
        .await
    }
    pub async fn accept_offer(
        &self,
        share: &str,
        from: &str,
        local_root: &str,
        name: Option<&str>,
    ) -> Result<Pair, ClientError> {
        self.call(Request::AcceptOffer {
            share: share.into(),
            from: from.into(),
            local_root: local_root.into(),
            name: name.map(Into::into),
        })
        .await
    }
    pub async fn keep_device(&self, id: &str) -> Result<(), ClientError> {
        self.call(Request::KeepDevice { id: id.into() }).await
    }
    pub async fn invite_device(&self) -> Result<PairingStatus, ClientError> {
        self.call(Request::InviteDevice).await
    }
    pub async fn join_device(&self, code: &str) -> Result<(), ClientError> {
        self.call(Request::JoinDevice { code: code.into() }).await
    }
    pub async fn pairing_status(&self) -> Result<PairingStatus, ClientError> {
        self.call(Request::PairingStatus).await
    }
    pub async fn confirm_pairing(&self, accept: bool) -> Result<PairingStatus, ClientError> {
        self.call(Request::ConfirmPairing { accept }).await
    }
    pub async fn cancel_pairing(&self) -> Result<(), ClientError> {
        self.call(Request::CancelPairing).await
    }
    pub async fn accounts(&self) -> Result<Vec<Account>, ClientError> {
        self.call(Request::Accounts).await
    }
    /// Returns the sign-in URL (also opened in the browser by the daemon).
    pub async fn add_account(&self) -> Result<String, ClientError> {
        self.call(Request::AddAccount).await
    }
    pub async fn remove_account(&self, id: &str) -> Result<(), ClientError> {
        self.call(Request::RemoveAccount { id: id.into() }).await
    }
    pub async fn drive_folders(
        &self,
        account: &str,
        parent_id: &str,
    ) -> Result<Vec<DriveFolder>, ClientError> {
        self.call(Request::DriveFolders {
            account: account.into(),
            parent_id: parent_id.into(),
        })
        .await
    }
    pub async fn resolve_drive_path(
        &self,
        account: &str,
        path: &str,
        create: bool,
    ) -> Result<DriveFolder, ClientError> {
        self.call(Request::ResolveDrivePath {
            account: account.into(),
            path: path.into(),
            create,
        })
        .await
    }
    pub async fn views(&self) -> Result<ViewsInfo, ClientError> {
        self.call(Request::Views).await
    }
    pub async fn add_view(&self, view: NewView) -> Result<View, ClientError> {
        self.call(Request::AddView { view }).await
    }
    pub async fn remove_view(&self, id: &str, keep_files: bool) -> Result<(), ClientError> {
        self.call(Request::RemoveView {
            id: id.into(),
            keep_files,
        })
        .await
    }
    pub async fn set_view_enabled(&self, id: &str, enabled: bool) -> Result<(), ClientError> {
        self.call(Request::SetViewEnabled {
            id: id.into(),
            enabled,
        })
        .await
    }
    pub async fn set_view_cache_limit(&self, id: &str, limit: u64) -> Result<(), ClientError> {
        self.call(Request::SetViewCacheLimit { id: id.into(), limit })
            .await
    }
    pub async fn refresh_view(&self, id: &str) -> Result<(), ClientError> {
        self.call(Request::RefreshView { id: id.into() }).await
    }
    pub async fn pin_path(&self, path: &str) -> Result<(), ClientError> {
        self.call(Request::PinPath { path: path.into() }).await
    }
    pub async fn unpin_path(&self, path: &str) -> Result<(), ClientError> {
        self.call(Request::UnpinPath { path: path.into() }).await
    }
    pub async fn vaults(&self) -> Result<Vec<VaultInfo>, ClientError> {
        self.call(Request::Vaults).await
    }
    pub async fn create_vault(&self, vault: NewVault) -> Result<VaultInfo, ClientError> {
        self.call(Request::CreateVault { vault }).await
    }
    pub async fn unlock_vault(&self, id: &str, passphrase: Passphrase) -> Result<VaultInfo, ClientError> {
        self.call(Request::UnlockVault {
            id: id.into(),
            passphrase,
        })
        .await
    }
    pub async fn unlock_vault_folder(
        &self,
        folder: VaultFolder,
        passphrase: Passphrase,
    ) -> Result<VaultInfo, ClientError> {
        self.call(Request::UnlockVaultFolder { folder, passphrase }).await
    }
    pub async fn upgrade_vault(
        &self,
        id: &str,
        folder: VaultFolder,
        passphrase: Passphrase,
    ) -> Result<VaultUpgrade, ClientError> {
        self.call(Request::UpgradeVault {
            id: id.into(),
            folder,
            passphrase,
        })
        .await
    }
    pub async fn lock_vault(&self, id: &str) -> Result<(), ClientError> {
        self.call(Request::LockVault { id: id.into() }).await
    }
    pub async fn setting(&self, key: &str) -> Result<Option<String>, ClientError> {
        self.call(Request::Setting { key: key.into() }).await
    }
    pub async fn set_setting(&self, key: &str, value: &str) -> Result<(), ClientError> {
        self.call(Request::SetSetting {
            key: key.into(),
            value: value.into(),
        })
        .await
    }
    pub async fn shutdown(&self) -> Result<(), ClientError> {
        self.call(Request::Shutdown).await
    }
}
