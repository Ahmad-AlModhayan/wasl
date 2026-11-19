//! The messages. One request gets one response with the same `id`; events arrive on connections
//! that subscribed, between responses, with no id.

use serde::{Deserialize, Serialize};
use wasl_proto::{
    BinEntry, ConflictChoice, ConflictUndo, DeviceStatus, FolderStatus, Lang, OplogStatus, OverallState,
    OverallStatus, Pair, Progress, RunOutcome, SignInCause, Trigger, VersionRef,
};
pub use wasl_proto::{
    DeviceInfo, DevicesInfo, LinkPath, OfferInfo, OfferStatus, PairSharing, PairingStage, PairingStatus, View,
};

/// The most changesets one [`Request::OplogList`] answer holds, and its `limit` when a request
/// leaves it out.
pub const OPLOG_LIST_MAX: u32 = 1000;

fn oplog_list_max() -> u32 {
    OPLOG_LIST_MAX
}

/// The newest protocol version this build speaks. Bumped when a message changes meaning; a client
/// and a daemon talk at the highest version both support (see [`VersionRange`]).
pub const PROTOCOL_VERSION: u32 = 1;
/// The oldest protocol version this build still speaks.
pub const MIN_PROTOCOL_VERSION: u32 = 1;

/// The lowest and highest protocol version one side supports (both inclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionRange {
    pub min: u32,
    pub max: u32,
}

/// Which side of a refused connection needs an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Behind {
    /// This side speaks only versions older than the other side's.
    This,
    /// The other side speaks only versions older than this side's.
    Peer,
}

impl VersionRange {
    /// The versions this build speaks.
    pub const CURRENT: VersionRange = VersionRange {
        min: MIN_PROTOCOL_VERSION,
        max: PROTOCOL_VERSION,
    };

    /// The highest version both ranges contain, if any.
    pub fn highest_common(self, other: VersionRange) -> Option<u32> {
        let top = self.max.min(other.max);
        (top >= self.min.max(other.min)).then_some(top)
    }

    /// For ranges with nothing in common: who is behind.
    pub fn behind(self, other: VersionRange) -> Behind {
        if self.max < other.min {
            Behind::This
        } else {
            Behind::Peer
        }
    }
}

/// What a client asks. Results are typed per request (see `Client`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// The daemon at a glance: version, accounts, pairs with their state and live progress. With a
    /// `lang` the folder statuses and the overall status carry their words in that language
    /// (English without one); the keys and values are the same in every language.
    Status {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lang: Option<Lang>,
    },
    Pairs,
    Pair {
        id: String,
    },
    /// With a `lang`, a refusal by a pair rule carries its words in that language
    /// ([`RemoteError::pair_rule`]); English without one. The key and values are the same in
    /// every language.
    CreatePair {
        pair: Pair,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lang: Option<Lang>,
    },
    /// As [`Request::CreatePair`] for the `lang`.
    UpdatePair {
        pair: Pair,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lang: Option<Lang>,
    },
    /// As [`Request::CreatePair`] for the `lang`.
    DeletePair {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lang: Option<Lang>,
    },
    SetPaused {
        id: String,
        paused: bool,
        reason: Option<String>,
    },
    Preview {
        id: String,
    },
    Sync {
        id: String,
        trigger: Trigger,
    },
    SyncAll {
        trigger: Trigger,
    },
    Cancel {
        id: String,
    },
    ApproveOnce {
        id: String,
    },
    BlockedPlan {
        id: String,
    },
    ClearBlockedPlan {
        id: String,
    },
    Runs {
        pair_id: Option<String>,
        limit: u32,
    },
    RunEvents {
        run_id: String,
    },
    /// The files a run left out or could not sync, worded in `lang` (English without one): each
    /// with its key, values and fix, and what to do about it. The reply is a list of
    /// [`wasl_proto::RunItem`], empty for a run that left nothing out.
    RunItems {
        run_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lang: Option<Lang>,
    },
    OpenConflicts {
        pair_id: Option<String>,
    },
    ConflictDetails {
        id: String,
    },
    ResolveConflict {
        id: String,
        choice: ConflictChoice,
    },
    UndoConflict {
        undo: ConflictUndo,
    },
    DeletionReview {
        pair_id: String,
    },
    RestoreDeletions {
        pair_id: String,
    },
    FileVersions {
        pair_id: String,
        key: String,
    },
    RestoreVersion {
        pair_id: String,
        key: String,
        version: VersionRef,
    },
    BinEntries {
        pair_id: Option<String>,
    },
    RestoreBin {
        ids: Vec<String>,
    },
    UndoBinRestore {
        restored: Vec<BinEntry>,
    },
    DeleteBinForever {
        ids: Vec<String>,
    },
    EmptyBin,
    /// Why a path in a pair is ignored or not (FR-A3 `wasl ignore --explain`).
    IgnoreExplain {
        pair_id: String,
        path: String,
    },
    Accounts,
    /// Starts Google sign-in in the browser; the result arrives as `Event::AccountAdded` or
    /// `Event::AccountFailed`. Returns the URL in case the browser could not be opened.
    AddAccount,
    RemoveAccount {
        id: String,
    },
    /// Drive folders under `parent_id` ("root" for My Drive), for choosing a pair's folder.
    DriveFolders {
        account: String,
        parent_id: String,
    },
    /// A Drive folder from a path like "My Drive/Photos/2026", created when `create` is set.
    ResolveDrivePath {
        account: String,
        path: String,
        create: bool,
    },
    /// This computer as a device and the devices in its group.
    Devices,
    /// Removes a device from the group: its key is never trusted again.
    RemoveDevice {
        id: String,
    },
    /// Connects to a group member at a typed address (`host:port`), where discovery cannot.
    ConnectDevice {
        address: String,
    },
    /// Shares a pair with the group's devices under a new share code, or under `share`, the code
    /// another device made for the same folder. Returns the code.
    SharePair {
        id: String,
        share: Option<String>,
    },
    /// The pair's share code, if it is shared with devices.
    ShareCode {
        id: String,
    },
    /// Stops syncing a pair with devices.
    UnsharePair {
        id: String,
    },
    /// Who a shared pair is served to: a [`PairSharing`], or null when it isn't shared.
    Sharing {
        id: String,
    },
    /// Sets who a shared pair is served to (docs/INTRODUCTIONS.md §3.2): every device known here
    /// (`all`), and the `devices` named (`WASL-…`), each of which is offered the pair. Replaces the
    /// list; a device left off it is refused at its next request.
    ShareWith {
        id: String,
        #[serde(default)]
        all: bool,
        #[serde(default)]
        devices: Vec<String>,
    },
    /// The folders the group's devices offer this computer ([`OfferInfo`], each sender apart).
    Offers,
    /// Hides `from`'s offer of `share` on this computer.
    DeclineOffer {
        share: String,
        from: String,
    },
    /// Accepts `from`'s offer of `share`: makes a pair synced with devices at `local_root` (a full
    /// path), named `name` or the offered name, under the offered share. Like any new pair it
    /// waits for its first-sync preview. Returns the new [`Pair`]. Refused when a pair here
    /// already syncs that share.
    AcceptOffer {
        share: String,
        from: String,
        local_root: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// The language a refusal by a pair rule is worded in ([`RemoteError::pair_rule`]);
        /// English without one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lang: Option<Lang>,
    },
    /// Keeps a device another device added (docs/INTRODUCTIONS.md §3.1): it is known on this
    /// computer and gets the folders shared with all devices.
    KeepDevice {
        id: String,
    },
    /// Opens a pairing code for a new device to join this computer's group.
    InviteDevice,
    /// Joins the device whose code was scanned or pasted.
    JoinDevice {
        code: String,
        /// Where to look for a typed code (`ip:port`) when discovery is blocked on this network.
        #[serde(default)]
        addresses: Vec<String>,
    },
    /// Where the open pairing stands (also after `Event::Changed`).
    PairingStatus,
    /// The person here says the four words match (`accept`), or not.
    ConfirmPairing {
        accept: bool,
    },
    CancelPairing,
    /// Read-only on-demand views (docs/ON_DEMAND.md §2.3) with their state.
    Views,
    /// Shows a Drive folder at a folder on this computer, read-only. Returns the new [`View`].
    AddView {
        view: NewView,
    },
    /// Stops showing a view. Its placeholders go, except pinned files, which stay as ordinary
    /// files; with `keep_files` every file is fetched and kept. Local changes are never touched.
    RemoveView {
        id: String,
        keep_files: bool,
    },
    SetViewEnabled {
        id: String,
        enabled: bool,
    },
    /// Bytes of unpinned content the view may keep on this computer.
    SetViewCacheLimit {
        id: String,
        limit: u64,
    },
    /// Reads the view's Drive folder again now.
    RefreshView {
        id: String,
    },
    /// Keeps a file (or every file of a folder) of a view on this computer: `path` is a full
    /// path inside a view's folder.
    PinPath {
        path: String,
    },
    /// Lets a pinned file's content go again when space is needed.
    UnpinPath {
        path: String,
    },
    /// The macOS File Provider extension asks (docs/ON_DEMAND.md §2.6), on its own socket in the
    /// app group's container: one item of a view (`id` is its path in the view, `""` the root).
    /// Returns a [`ProviderItem`].
    ProviderItem {
        view: String,
        id: String,
    },
    /// The children of a folder of a view (`""` the root), or with no `parent` every item of it
    /// (the system's working set). Returns a [`ProviderListing`].
    ProviderList {
        view: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
    },
    /// What changed in a view since `anchor` (from a listing or an earlier change set). Returns
    /// [`ProviderChanges`].
    ProviderChanges {
        view: String,
        anchor: String,
    },
    /// Writes a file's content (all of it, or `len` bytes from `offset`) to a new file the
    /// extension can hand to the system. Returns [`ProviderContent`].
    ProviderFetch {
        view: String,
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        len: Option<u64>,
    },
    /// Encrypted vaults known on this computer, each with whether it is unlocked here.
    Vaults,
    /// Makes a new vault (format v2) in a new or empty Drive folder and unlocks it on this
    /// computer. Returns its [`VaultInfo`]. Slow on purpose (Argon2id).
    CreateVault {
        vault: NewVault,
    },
    /// Unlocks the vault (v1 or v2) a Drive folder holds on this computer, and records the vault
    /// when it is new here. Returns its [`VaultInfo`]; a wrong passphrase is `Invalid`.
    UnlockVaultFolder {
        folder: VaultFolder,
        passphrase: Passphrase,
    },
    /// Unlocks a vault this computer knows. Returns its [`VaultInfo`]; a wrong passphrase is
    /// `Invalid`.
    UnlockVault {
        id: String,
        passphrase: Passphrase,
    },
    /// Copies an unlocked vault (format v1 or 2.0) into a new vault of the current format in
    /// `folder`, a new or empty Drive folder, with `passphrase` (security review S4-2). The old
    /// vault, its pairs and views are not touched. Returns a [`VaultUpgrade`]. Slow: it moves
    /// every file through this computer.
    UpgradeVault {
        id: String,
        folder: VaultFolder,
        passphrase: Passphrase,
    },
    /// Forgets the vault's keys on this computer: its pairs and views wait until it is unlocked
    /// again. Nothing on Drive changes.
    LockVault {
        id: String,
    },
    /// An agent starts working in `path` (a folder in a pair, or the pair's folder). Returns the
    /// open [`wasl_proto::Changeset`]; with a `session` already open, that one.
    /// docs/AGENT_CHANGES.md.
    AgentBegin {
        path: String,
        actor: String,
        label: Option<String>,
        session: Option<String>,
    },
    /// An agent is about to write the file at `path`: Wasl keeps its current content. Returns
    /// whether a copy was made (false outside any agent session).
    AgentSave {
        path: String,
        session: Option<String>,
    },
    /// Ends a session, found by changeset id, by agent session id, or by the folder it began in.
    /// Returns the held changeset, or null when the agent changed nothing.
    AgentEnd {
        id: Option<String>,
        session: Option<String>,
        path: Option<String>,
    },
    /// Open and held changesets (every one with `all`), newest first.
    AgentChangesets {
        pair_id: Option<String>,
        all: bool,
    },
    /// One changeset with its files ([`wasl_proto::ChangesetDetails`]).
    AgentChangeset {
        id: String,
    },
    /// What a session changed in watched folders and what it missed
    /// ([`wasl_proto::JournalReport`], docs/AGENT_UNDO.md UND-3, UND-4).
    AgentJournal {
        id: String,
    },
    /// A unified diff of one file of a changeset (`rel` relative to the pair).
    AgentDiff {
        id: String,
        rel: String,
    },
    /// Asks to keep the agent's work. Nothing is accepted yet: the daemon has the Wasl app show a
    /// confirmation to the person, and accepts (the work then syncs at once) only when they
    /// confirm there; the answer is an [`AcceptConfirmation`] with the code the dialog shows.
    /// Nothing a client sends over this protocol can stand in for that confirmation
    /// (docs/AGENT_CHANGES.md §5, security review S3-1). Refused at once when `agent` is set: the
    /// client found an agent marker in its environment. `client` is recorded with the decision.
    AgentAccept {
        id: String,
        /// Which client asks: "cli", "desktop". Absent from older clients.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client: Option<String>,
        /// The agent marker the client found (for example `CLAUDECODE`), if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
    },
    /// Puts back what the session changed. Returns a [`wasl_proto::RevertReport`].
    AgentRevert {
        id: String,
    },
    /// Adds a folder to agent undo's watch list (docs/AGENT_UNDO.md, UND-1). Returns the
    /// [`wasl_proto::Watch`]. A disk's top folder, Wasl's data folder (or one around it) and a
    /// folder in or around a watched one are refused.
    AgentWatch {
        path: String,
    },
    /// Takes a folder off the watch list, by watch id or path. Returns the removed
    /// [`wasl_proto::Watch`].
    AgentUnwatch {
        watch: String,
    },
    /// The watch list, by path ([`wasl_proto::Watch`]s).
    AgentWatches,
    /// Sets the default ignores a watch (id or path) still applies; one left out stops applying.
    AgentWatchIgnores {
        watch: String,
        ignores: Vec<String>,
    },
    /// The op-log's changesets that match, newest first, each with its `op_count`
    /// ([`wasl_proto::OplogPage`], ADR-0002 Read format). Read-only, like `OplogShow`.
    OplogList {
        actor: Option<String>,
        status: Option<OplogStatus>,
        /// Opened at or after this, milliseconds.
        since: Option<i64>,
        /// A pair id: changesets in that pair.
        pair: Option<String>,
        /// At most this many, the newest; the daemon caps it at [`OPLOG_LIST_MAX`], which is
        /// also what a request without `limit` gets.
        #[serde(default = "oplog_list_max")]
        limit: u32,
    },
    /// One changeset with its operations ([`wasl_proto::OplogPage`] with one changeset). No
    /// `origin_device`: this device. One whose answer would pass [`crate::transport::MAX_LINE`]
    /// is refused with an error that says so.
    OplogShow {
        origin_device: Option<String>,
        id: String,
    },
    Setting {
        key: String,
    },
    SetSetting {
        key: String,
        value: String,
    },
    /// From now on this connection also receives events. With `status_lang` it also receives
    /// [`Event::FolderStatus`], worded in that language; without it that event is never sent (a
    /// client that doesn't know it could not read it).
    Subscribe {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status_lang: Option<Lang>,
    },
    /// Stops the daemon (after the running syncs are cancelled cleanly).
    Shutdown,
}

/// Why a request failed, in a form a client can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    NotFound,
    Invalid,
    Auth,
    Network,
    Permission,
    Locked,
    Busy,
    Unsupported,
    Other,
}

#[cfg(test)]
mod error_key_tests {
    use super::ErrorKind;
    use wasl_proto::words;

    #[test]
    fn every_daemon_error_kind_has_its_entry() {
        let want = [
            (ErrorKind::NotFound, "request.not_found"),
            (ErrorKind::Invalid, "error.invalid"),
            (ErrorKind::Auth, "request.auth"),
            (ErrorKind::Network, "request.network"),
            (ErrorKind::Permission, "request.permission"),
            (ErrorKind::Locked, "request.locked"),
            (ErrorKind::Busy, "error.busy"),
            (ErrorKind::Unsupported, "error.unsupported"),
            (ErrorKind::Other, words::REQUEST_FALLBACK),
        ];
        for (kind, key) in want {
            assert_eq!(kind.key(), key);
            assert!(words::entry(key).is_some(), "{key} is not in the catalogue");
        }
    }
}

impl ErrorKind {
    /// The catalogue entry (`wasl_proto::words`) that words this kind of error.
    pub fn key(self) -> &'static str {
        match self {
            ErrorKind::NotFound => "request.not_found",
            ErrorKind::Invalid => "error.invalid",
            ErrorKind::Auth => "request.auth",
            ErrorKind::Network => "request.network",
            ErrorKind::Permission => "request.permission",
            ErrorKind::Locked => "request.locked",
            ErrorKind::Busy => "error.busy",
            ErrorKind::Unsupported => "error.unsupported",
            ErrorKind::Other => wasl_proto::words::REQUEST_FALLBACK,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct RemoteError {
    pub kind: ErrorKind,
    pub message: String,
    /// Set when the request failed because one of two devices needs an update (a device
    /// connection refused for no shared protocol version, wasl#36). The `kind` and `message`
    /// stay the generic network failure they have always been, so a client from before this
    /// field still reads the answer; a newer one words the typed reason. Boxed: it is large and
    /// every `ClientError` carries a `RemoteError`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_update: Option<Box<wasl_proto::DeviceNeedsUpdate>>,
    /// Set when the request was refused by a pair rule (a folder that overlaps another, a view, a
    /// sync running, an agent's changes waiting; wasl#61): the catalogue's entry for the refusal,
    /// with its stable `key`, the folders in its `values` and the sentence in the language the
    /// request asked for (English without one). The `kind` and `message` stay what they have
    /// always been (`Invalid` and the English words), so a client from before this field still
    /// reads the answer; a newer one words it again in its own language with
    /// [`wasl_proto::Rendered::in_lang`]. Boxed so every `ClientError` stays small.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pair_rule: Option<Box<wasl_proto::Rendered>>,
}

/// A pair as the status line shows it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairState {
    pub pair: Pair,
    pub running: bool,
    pub progress: Option<Progress>,
    pub open_conflicts: u32,
    pub blocked: bool,
    /// Bytes in the pair's folder at its last sync (its baseline's sizes for this device), not a
    /// fresh walk. None before the first sync, and from an older daemon.
    #[serde(default)]
    pub folder_bytes: Option<u64>,
    /// Files the last finished run left alone on purpose (the planner's skips: names that can't
    /// be held, files over the size limit). Files the ignore rules leave out aren't counted.
    /// None before the first finished run, and from an older daemon.
    #[serde(default)]
    pub skipped: Option<u32>,
    /// The other devices the pair syncs with, and whether each is connected now. Empty for a pair
    /// that only syncs with Drive, when device sync is off, and from an older daemon.
    #[serde(default)]
    pub peers: Vec<PeerState>,
    /// Free bytes for this user on the drive holding the pair's folder (its nearest existing
    /// parent while the folder isn't there). None when it can't be told, and from an older daemon.
    #[serde(default)]
    pub free_bytes: Option<u64>,
    /// The folder's status, decided once by the engine ([`wasl_proto::status`]): what a screen
    /// draws and an agent reads. None from an older daemon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<FolderStatus>,
}

/// Another device a pair syncs with, as the status line names it ("Waiting for …").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerState {
    /// `WASL-XXXX-…`.
    pub id: String,
    pub name: String,
    /// Connected to this device now.
    pub online: bool,
}

/// The tray's whole state: one icon state plus per-pair detail (FR-B5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub version: String,
    pub protocol: u32,
    pub accounts: Vec<Account>,
    pub pairs: Vec<PairState>,
    pub overall: Overall,
    /// Agent changesets waiting for a decision (docs/AGENT_CHANGES.md).
    #[serde(default)]
    pub agent_changes: u32,
    /// The whole app's status, worked out from the folder statuses by the same rules. `overall`
    /// above is its older form: the same decision, in the states older clients know. None from an
    /// older daemon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overall_status: Option<OverallStatus>,
    /// Where each device in the group stands (online, syncing, last seen, can't reach), in the
    /// caller's language, worked out once in the engine's catalogue. Empty from an older daemon and
    /// when device sync is off.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<DeviceStatus>,
}

/// The tray icon's states (FR-B5). The older form of [`OverallState`], kept so older clients still
/// read the reply: a state they don't know ("partly synced") reads as the nearest they do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Overall {
    UpToDate,
    Syncing,
    Paused,
    Waiting,
    NeedsYou,
    /// No pairs yet.
    Idle,
}

impl From<OverallState> for Overall {
    fn from(s: OverallState) -> Overall {
        match s {
            OverallState::Idle => Overall::Idle,
            OverallState::Synced => Overall::UpToDate,
            OverallState::Syncing => Overall::Syncing,
            OverallState::Paused => Overall::Paused,
            // The files that didn't move are tried again at the next sync: waiting, as far as an
            // older client can say it.
            OverallState::Waiting | OverallState::PartlySynced => Overall::Waiting,
            OverallState::NeedsYou => Overall::NeedsYou,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub email: String,
    /// Set when the account must sign in again. The raw cause, for Details; the words and the fix
    /// are the catalogue's, chosen by `sign_in`.
    pub auth_error: Option<String>,
    /// Why the account must sign in again, typed by the engine: Google no longer accepts the
    /// sign-in, or the one saved on this device can't be read. None when it needn't, and from an
    /// older daemon (a problem with no cause reads as the first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign_in: Option<SignInCause>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriveFolder {
    pub id: String,
    pub name: String,
    pub drive_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IgnoreExplanation {
    pub ignored: bool,
    /// The rule that decided, as written, with where it came from ("pair settings", ".waslignore:3").
    pub rule: Option<String>,
    pub source: Option<String>,
    /// What explain could not check, e.g. the size of a file that is only on Drive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// What [`Request::AddView`] takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewView {
    pub name: String,
    pub account_id: String,
    pub drive_folder_id: String,
    pub drive_id: Option<String>,
    /// Display path of the Drive folder, e.g. `My Drive/Photos`.
    pub remote_path: String,
    /// Where the view appears: a full path to a new or empty folder.
    pub local_path: String,
    /// Bytes of unpinned content to keep; `None` is the default (20 GiB).
    pub cache_limit: Option<u64>,
}

/// A vault passphrase on its way to the daemon. Never printed (`Debug` hides it), wiped from
/// memory when dropped, and never stored: the daemon derives the keys and lets it go.
#[derive(Clone, Default)]
pub struct Passphrase(zeroize::Zeroizing<String>);

impl Passphrase {
    pub fn new(passphrase: impl Into<String>) -> Passphrase {
        Passphrase(zeroize::Zeroizing::new(passphrase.into()))
    }

    /// The passphrase itself, for deriving keys.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for Passphrase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Passphrase(..)")
    }
}

impl Serialize for Passphrase {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Passphrase {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Passphrase, D::Error> {
        String::deserialize(d).map(Passphrase::new)
    }
}

/// What [`Request::CreateVault`] takes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewVault {
    pub account_id: String,
    pub drive_id: Option<String>,
    /// Where the vault goes: a folder named `name` in this one (`root` for My Drive), made when
    /// missing; one that exists must be empty.
    pub parent_id: String,
    /// The vault's own folder instead, when the client already chose a new or empty one
    /// (`parent_id` is then not used).
    #[serde(default)]
    pub folder_id: Option<String>,
    /// Display path of the vault's folder, e.g. `My Drive/Private`.
    pub remote_path: String,
    /// The vault's name (in its `vault.json`, and its folder's name when one is made).
    pub name: String,
    pub passphrase: Passphrase,
}

/// What [`Request::UpgradeVault`] did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultUpgrade {
    /// The new vault.
    pub vault: VaultInfo,
    pub folders: usize,
    pub files: usize,
    pub bytes: u64,
    /// Files an earlier, interrupted upgrade already copied.
    pub already_there: usize,
    /// Items the old vault's listing could not place (same names in one folder, …).
    pub left_out: Vec<String>,
    /// Files of the old vault that did not decrypt.
    pub failed: Vec<String>,
}

/// A Drive folder that holds a vault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultFolder {
    pub account_id: String,
    pub folder_id: String,
    pub drive_id: Option<String>,
    /// Display path, e.g. `My Drive/Private`.
    pub remote_path: String,
}

/// A vault known on this computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultInfo {
    pub id: String,
    pub name: String,
    /// 1 or 2 (docs/ENCRYPTION.md §11).
    pub version: u32,
    /// "1", "2.0" or "2.1", for people (empty from a wasld before 0.3.0).
    #[serde(default)]
    pub format: String,
    /// Its keys are on this computer: its pairs sync and its views show files.
    pub unlocked: bool,
    /// The Drive folder it was made in or unlocked from.
    pub account_id: Option<String>,
    pub drive_folder_id: Option<String>,
    pub drive_id: Option<String>,
    pub remote_path: String,
    pub created_at: i64,
}

/// Read-only views and whether this computer can show them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewsInfo {
    /// False where files on demand are not available (macOS without the File Provider extension).
    pub supported: bool,
    pub views: Vec<ViewState>,
}

/// A view with what it shows now. Its last refresh and error are in [`View`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewState {
    pub view: View,
    /// Its folder is set up and serving files.
    pub mounted: bool,
    /// Files and their total size in the last listing.
    pub files: u64,
    pub bytes: u64,
    /// Content on this computer: all of it, and the pinned part.
    pub cache_used: u64,
    pub pinned_bytes: u64,
    /// Things in the folder Wasl left alone: never uploaded, never deleted.
    pub local_changes: Vec<LocalChange>,
    /// What kind of failure [`View::last_error`] is, when this run of the service saw it: lets a
    /// client say it in its own words and language.
    #[serde(default)]
    pub last_error_kind: Option<ErrorKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalChange {
    /// Relative to the view's folder, `/`-separated.
    pub path: String,
    /// Why, in English (for the CLI and logs); clients that translate use [`LocalChange::kind`].
    pub reason: String,
    #[serde(default)]
    pub kind: Option<LocalChangeKind>,
}

/// An accept waits for the person (answer to [`Request::AgentAccept`]): the Wasl app shows a
/// dialog with `code`, and the changeset is accepted only if they confirm it there within
/// `expires_in_secs`. A client says so and may watch the changeset's status.
///
/// When `retry_in_secs` is set, no dialog was opened (and `code` is empty): this changeset's last
/// confirmation was declined or timed out, and another may be asked for after that many seconds.
/// The wait grows with each refusal, so an agent asking in a loop cannot keep a dialog on screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptConfirmation {
    /// Shown in the dialog and by the client that asked, so the person can tell the dialog
    /// belongs to what they asked for. Not a secret.
    pub code: String,
    pub expires_in_secs: u64,
    /// Absent from older daemons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_in_secs: Option<u64>,
}

/// One file or folder of a view, as the File Provider extension shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderItem {
    /// Its path in the view, `/`-separated; `""` is the view's root.
    pub id: String,
    /// The folder it is in (`""` for the root's children and the root itself).
    pub parent: String,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// Milliseconds since the Unix epoch.
    pub mtime_ms: i64,
    /// Changes whenever the content does (empty for a folder).
    pub version: String,
    /// Kept on this computer: the system downloads it and keeps it downloaded.
    pub pinned: bool,
}

/// The children of a folder, with the anchor to ask for changes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderListing {
    pub anchor: String,
    pub items: Vec<ProviderItem>,
}

/// What changed since an anchor. With `expired`, the daemon can't tell (it restarted, or more
/// than one refresh happened since): the extension lists everything again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderChanges {
    pub anchor: String,
    pub updated: Vec<ProviderItem>,
    pub deleted: Vec<String>,
    pub expired: bool,
}

/// A file's content, written to `path` (in the app group's container): `len` bytes from `offset`
/// of `item`. The extension hands the file to the system, which takes it over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderContent {
    pub path: String,
    pub item: ProviderItem,
    pub offset: u64,
    pub len: u64,
}

/// Why something in a view was left alone (wasl-vfs `KeptKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalChangeKind {
    OnlyHere,
    ChangedHere,
    Differs,
    Unsupported,
    Failed,
    /// A file of a read-only view that was changed here: Drive's version was put back, and the
    /// changed copy set aside in Wasl's data folder; [`LocalChange::reason`] is where (security
    /// review S3-2).
    Restored,
}

/// Something happened that a subscribed client may want to show.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Progress {
        progress: Progress,
    },
    RunFinished {
        pair_id: String,
        outcome: RunOutcome,
    },
    /// Pairs, settings, history or the bin changed: re-read what is on screen.
    Changed,
    AccountAdded {
        account: Account,
    },
    AccountFailed {
        message: String,
    },
    ShuttingDown,
    /// A folder's status changed. Sent only to connections that asked for it
    /// ([`Request::Subscribe::status_lang`]), with the new status in their language.
    FolderStatus {
        pair_id: String,
        status: Box<FolderStatus>,
    },
}

/// One line on the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Frame {
    Request { id: u64, request: Box<Request> },
    Ok { id: u64, ok: serde_json::Value },
    Err { id: u64, err: RemoteError },
    Event { event: Event },
}

#[cfg(test)]
mod tests {
    use super::{DaemonStatus, ErrorKind, OPLOG_LIST_MAX, Overall, RemoteError, Request};
    use wasl_proto::OverallState;

    #[test]
    fn an_oplog_list_request_without_limit_gets_the_daemon_cap() {
        let r: Request = serde_json::from_value(serde_json::json!({"op": "oplog_list"})).unwrap();
        assert!(
            matches!(
                r,
                Request::OplogList {
                    actor: None,
                    status: None,
                    since: None,
                    pair: None,
                    limit: OPLOG_LIST_MAX,
                }
            ),
            "{r:?}"
        );
        let r: Request = serde_json::from_value(serde_json::json!({"op": "oplog_list", "limit": 5})).unwrap();
        assert!(matches!(r, Request::OplogList { limit: 5, .. }), "{r:?}");
    }
    // ---- additive compatibility of the status layer (issue #29)

    /// What an older client's reader looked like: only the fields it knew.
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct OlderStatus {
        version: String,
        protocol: u32,
        pairs: Vec<OlderPairState>,
        overall: Overall,
    }
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct OlderPairState {
        pair: wasl_proto::Pair,
        running: bool,
        open_conflicts: u32,
        blocked: bool,
    }

    /// wasl#36: what a client from before `needs_update` reads of an error answer (a copy of the
    /// previous release's shape, so it stays that when `RemoteError` grows again).
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct OlderRemoteError {
        kind: super::ErrorKind,
        message: String,
    }

    #[test]
    fn a_needs_update_answer_reads_as_a_network_failure_to_an_older_client() {
        use super::{ErrorKind, Frame, RemoteError};
        use wasl_proto::{DeviceNeedsUpdate, Lang, UpdateSide};
        let n = DeviceNeedsUpdate::new(UpdateSide::Peer, "abc", Some("Phone".into()), Lang::En);
        let err = RemoteError {
            kind: ErrorKind::Network,
            message: "Couldn't reach a device of your group at x: Phone needs an update".into(),
            needs_update: Some(Box::new(n.clone())),
            pair_rule: None,
        };
        let wire = serde_json::to_value(Frame::Err {
            id: 7,
            err: err.clone(),
        })
        .unwrap();
        // An older client's reader ignores the new field and sees a network failure.
        let old: OlderRemoteError = serde_json::from_value(wire["err"].clone()).unwrap();
        assert_eq!(ErrorKind::Network, old.kind);
        assert_eq!(err.message, old.message);
        // A new client reads the typed reason back.
        let back: RemoteError = serde_json::from_value(wire["err"].clone()).unwrap();
        assert_eq!(Some(Box::new(n)), back.needs_update);
        // An answer without the reason is the old shape on the wire, and reads back without one.
        let plain = RemoteError {
            needs_update: None,
            pair_rule: None,
            ..err
        };
        let wire = serde_json::to_value(&plain).unwrap();
        assert!(wire.get("needs_update").is_none(), "{wire}");
        let back: RemoteError =
            serde_json::from_value(serde_json::json!({"kind": "network", "message": "m"})).unwrap();
        assert!(back.needs_update.is_none());
    }

    /// wasl#61: a pair rule's refusal rides beside the answer an older client reads, and the pair
    /// requests older clients send (no `lang`) still parse and say nothing more than before.
    #[test]
    fn a_pair_rule_refusal_is_typed_beside_the_answer_an_older_client_reads() {
        use wasl_proto::words::{Lang, Values, render};
        let words = render(
            "pair.overlaps",
            &Values::default().folder("/photos/2026").other("Camera"),
            Lang::En,
            None,
            &[],
            &mut |_| {},
        );
        let err = RemoteError {
            kind: ErrorKind::Invalid,
            message: words.text.clone(),
            needs_update: None,
            pair_rule: Some(Box::new(words.clone())),
        };
        let wire = serde_json::to_value(&err).unwrap();
        assert_eq!("invalid", wire["kind"]);
        assert_eq!("pair.overlaps", wire["pair_rule"]["key"]);
        assert_eq!("Camera", wire["pair_rule"]["values"]["other"]);
        let back: RemoteError = serde_json::from_value(wire).unwrap();
        assert_eq!(Some(Box::new(words)), back.pair_rule);
        // From an older daemon: no field, no words.
        let old: RemoteError =
            serde_json::from_value(serde_json::json!({"kind": "invalid", "message": "m"})).unwrap();
        assert!(old.pair_rule.is_none());
        assert!(serde_json::to_value(&old).unwrap().get("pair_rule").is_none());

        let r: Request = serde_json::from_value(serde_json::json!({"op": "delete_pair", "id": "p"})).unwrap();
        assert!(matches!(r, Request::DeletePair { lang: None, .. }), "{r:?}");
        assert_eq!(
            serde_json::json!({"op": "delete_pair", "id": "p"}),
            serde_json::to_value(Request::DeletePair {
                id: "p".into(),
                lang: None
            })
            .unwrap()
        );
        let r: Request =
            serde_json::from_value(serde_json::json!({"op": "delete_pair", "id": "p", "lang": "ar"}))
                .unwrap();
        assert!(
            matches!(
                r,
                Request::DeletePair {
                    lang: Some(Lang::Ar),
                    ..
                }
            ),
            "{r:?}"
        );
    }

    #[test]
    fn the_status_requests_older_clients_send_still_parse() {
        let r: Request = serde_json::from_value(serde_json::json!({"op": "status"})).unwrap();
        assert!(matches!(r, Request::Status { lang: None }), "{r:?}");
        let r: Request = serde_json::from_value(serde_json::json!({"op": "subscribe"})).unwrap();
        assert!(matches!(r, Request::Subscribe { status_lang: None }), "{r:?}");
        // And the new ones, with no language asked, say nothing more than the old ones did.
        assert_eq!(
            serde_json::json!({"op": "status"}),
            serde_json::to_value(Request::Status { lang: None }).unwrap()
        );
        assert_eq!(
            serde_json::json!({"op": "subscribe"}),
            serde_json::to_value(Request::Subscribe { status_lang: None }).unwrap()
        );
        let r: Request = serde_json::from_value(serde_json::json!({"op": "status", "lang": "ar"})).unwrap();
        assert!(
            matches!(
                r,
                Request::Status {
                    lang: Some(wasl_proto::Lang::Ar)
                }
            ),
            "{r:?}"
        );
    }

    #[test]
    fn the_run_items_request_names_a_run_and_may_name_a_language() {
        let r: Request =
            serde_json::from_value(serde_json::json!({"op": "run_items", "run_id": "r1"})).unwrap();
        assert!(
            matches!(&r, Request::RunItems { run_id, lang: None } if run_id == "r1"),
            "{r:?}"
        );
        assert_eq!(
            serde_json::json!({"op": "run_items", "run_id": "r1", "lang": "ar"}),
            serde_json::to_value(Request::RunItems {
                run_id: "r1".into(),
                lang: Some(wasl_proto::Lang::Ar)
            })
            .unwrap()
        );
    }

    #[test]
    fn a_reply_from_an_older_daemon_reads_without_the_new_fields() {
        let reply = serde_json::json!({
            "version": "0.9", "protocol": 3, "accounts": [], "pairs": [], "overall": "up_to_date"
        });
        let st: DaemonStatus = serde_json::from_value(reply).unwrap();
        assert!(st.overall_status.is_none());
        assert_eq!(Overall::UpToDate, st.overall);
    }

    #[test]
    fn the_devices_in_a_status_are_optional_both_ways() {
        let reply = serde_json::json!({
            "version": "0.9", "protocol": 3, "accounts": [], "pairs": [], "overall": "up_to_date"
        });
        let st: DaemonStatus = serde_json::from_value(reply).unwrap();
        assert!(st.devices.is_empty());
        // No devices: the field is left out, so an older client sees the reply it knew.
        let json = serde_json::to_value(&st).unwrap();
        assert!(json.get("devices").is_none(), "{json}");
        let with = serde_json::json!({
            "version": "1.0", "protocol": 3, "accounts": [], "pairs": [], "overall": "up_to_date",
            "devices": [{
                "id": "WASL-AAAA", "name": "Laptop", "this_device": false, "state": "last_seen",
                "words": {"key": "device.last_seen_days", "text": "Last seen 3 days ago",
                          "values": {"device": "Laptop", "count": 3}},
                "last_seen_at": 1_700_000_000_000_i64
            }]
        });
        let st: DaemonStatus = serde_json::from_value(with).unwrap();
        assert_eq!(1, st.devices.len());
        assert_eq!("device.last_seen_days", st.devices[0].words.key);
        assert_eq!(Some(3), st.devices[0].words.values.count);
    }

    #[test]
    fn the_new_reply_reads_as_an_older_client_would() {
        let mut out = serde_json::json!({
            "version": "1.0", "protocol": 3, "accounts": [], "pairs": [], "overall": "waiting",
            "overall_status": {
                "state": "partly_synced",
                "words": {"key": "overall.partly_synced", "text": "Partly synced"},
                "needs_you": 0
            }
        });
        let st: DaemonStatus = serde_json::from_value(out.clone()).unwrap();
        assert_eq!(OverallState::PartlySynced, st.overall_status.unwrap().state);
        let old: OlderStatus = serde_json::from_value(out.take()).unwrap();
        assert_eq!(
            Overall::Waiting,
            old.overall,
            "a state an older client lacks reads as the nearest it knows"
        );
    }

    #[test]
    fn the_older_overall_never_names_a_state_older_clients_lack() {
        for (state, older) in [
            (OverallState::Idle, Overall::Idle),
            (OverallState::Synced, Overall::UpToDate),
            (OverallState::Syncing, Overall::Syncing),
            (OverallState::Paused, Overall::Paused),
            (OverallState::Waiting, Overall::Waiting),
            (OverallState::PartlySynced, Overall::Waiting),
            (OverallState::NeedsYou, Overall::NeedsYou),
        ] {
            assert_eq!(older, Overall::from(state), "{state:?}");
            let wire = serde_json::to_value(Overall::from(state)).unwrap();
            assert!(
                ["idle", "up_to_date", "syncing", "paused", "waiting", "needs_you"]
                    .contains(&wire.as_str().unwrap()),
                "{wire}"
            );
        }
    }
}
