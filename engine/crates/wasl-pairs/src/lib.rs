//! The rules for pairs and the operations on them, one copy for every app (ADR-0001,
//! `docs/adr/0001-pair-rules-in-one-module.md`; the words are defined in `CONTEXT.md`).
//!
//! The desktop service and the phone used to keep their own copies, and they drifted: the phone
//! saved a pair without checking it, the desktop checked it and held it for its preview. Apps now
//! call [`Pairs`] and only translate its answers into their transport and language. What differs
//! between apps (where it keeps its own data, how it finds a vault) comes in through [`PairsEnv`].

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use wasl_core::Route;
use wasl_engine::{Engine, EngineError};
use wasl_store::{AGENT_CHANGES_PENDING, Pair, SharedStore, StoreError};

/// Why a pair breaks the pair rules. A typed reason, never a sentence: each app words it in its
/// own language (Wasl is English first; [`Problem`]'s `Display` is the English).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Problem {
    /// The pair's ID isn't one plain name.
    #[error("A pair id is one plain name")]
    InvalidId,
    /// No folder on this device was chosen (neither a full path nor a folder picked with the
    /// system's folder picker).
    #[error("Choose the folder on this device by its full path")]
    NoFolder,
    /// A pair that syncs with Drive needs its account and Drive folder.
    #[error("Choose a Google account and a Drive folder")]
    NoDriveFolder,
    /// The folder is in or around one where Wasl keeps its own data (its database, key files,
    /// staged files): syncing it would upload them, and downloads could overwrite them.
    #[error(
        "{folder} is in or around {own}, where Wasl keeps its own data (its database and private key files). Choose another folder."
    )]
    OwnData { folder: String, own: String },
    /// The folder is in or around a view's folder, which is read-only.
    #[error("{folder} is in or around the view {view}, which is read-only. Choose another folder.")]
    AroundView { folder: String, view: String },
    /// The folder is another pair's, or in or around it: two pairs on the same files undo each
    /// other's work.
    #[error(
        "{folder} is already synced by the pair {pair}, or is in or around its folder. Two pairs on the same files can undo each other. Choose another folder."
    )]
    Overlaps { folder: String, pair: String },
    /// The change points the pair at other files while it syncs: the run would write the old
    /// files' state back after the reset. (The word "folders" is the person's: Drive folder or
    /// local folder.)
    #[error("{}", wasl_engine::syncing_refusal(pair))]
    Syncing { pair: String },
    /// An agent's changes on the pair wait for review: the pair can't be removed or pointed at
    /// other files until they are accepted or reverted.
    #[error("{}", AGENT_CHANGES_PENDING)]
    AgentChanges,
}

/// Why an operation on a pair didn't happen.
#[derive(Debug, thiserror::Error)]
pub enum PairError {
    /// The pair breaks a rule.
    #[error(transparent)]
    Rule(#[from] Problem),
    /// A pair on a vault's folder couldn't be linked to its vault: the app's own error, kept
    /// whole so the app answers as it always has (its kind, its words).
    #[error("{0}")]
    Vault(Box<dyn std::error::Error + Send + Sync>),
    /// The engine or the store refused in a way no rule above names (a change that raced with a
    /// run starting): their words, kept whole.
    #[error("{0}")]
    Refused(String),
    /// Any other failure of the engine's save, kept whole so the app answers as it always has.
    #[error(transparent)]
    Engine(EngineError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// What differs between the apps in front of the module.
#[async_trait::async_trait]
pub trait PairsEnv: Send + Sync {
    /// Folders and files where this app keeps its own data, which no pair may be in or around.
    fn own_dirs(&self) -> Vec<PathBuf>;

    /// Links a pair on a vault's folder to its vault before it is recorded, or refuses it when
    /// that can't be told. The default leaves the pair as it is.
    async fn link_vault(&self, _pair: &mut Pair) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
}

/// The pairs on this device, through their rules.
pub struct Pairs {
    store: SharedStore,
    env: Arc<dyn PairsEnv>,
}

impl Pairs {
    pub fn new(store: SharedStore, env: Arc<dyn PairsEnv>) -> Pairs {
        Pairs { store, env }
    }

    /// Whether `pair` keeps the pair rules on this device, as it would be saved: the first rule
    /// it breaks. The same answer a save gives, so an editor can warn before saving.
    pub fn check(&self, pair: &Pair) -> Result<(), PairError> {
        if pair.id.is_empty() || pair.id.contains(['/', '\\', ':']) || pair.id == "." || pair.id == ".." {
            return Err(Problem::InvalidId.into());
        }
        let Some(root) = comparable(&pair.local_root) else {
            return Err(Problem::NoFolder.into());
        };
        let has_drive =
            !pair.drive_folder_id.as_deref().unwrap_or("").is_empty() && pair.account_id.is_some();
        if pair.route != Route::Devices && !has_drive {
            return Err(Problem::NoDriveFolder.into());
        }
        let near = |a: &Path, b: &Path| contains(a, b) || contains(b, a);
        for own in self.env.own_dirs() {
            for own_form in forms(&own) {
                for root_form in forms(&root) {
                    if near(&own_form, &root_form) {
                        return Err(Problem::OwnData {
                            folder: pair.local_root.clone(),
                            own: own.display().to_string(),
                        }
                        .into());
                    }
                }
            }
        }
        let s = self.store.lock();
        for v in s.views()? {
            if comparable(&v.local_path).is_some_and(|view| near(&view, &root)) {
                return Err(Problem::AroundView {
                    folder: pair.local_root.clone(),
                    view: v.name,
                }
                .into());
            }
        }
        for other in s.list_pairs()? {
            if other.id != pair.id && comparable(&other.local_root).is_some_and(|theirs| near(&theirs, &root))
            {
                return Err(Problem::Overlaps {
                    folder: pair.local_root.clone(),
                    pair: other.name,
                }
                .into());
            }
        }
        Ok(())
    }

    /// Records a new pair: checked against the pair rules, linked to its vault when its folder is
    /// one, and held for its first preview (nothing moves until the person has seen the first plan
    /// and started it), whichever app made it. Returns the pair as recorded.
    pub async fn create(&self, mut pair: Pair) -> Result<Pair, PairError> {
        self.check(&pair)?;
        self.env.link_vault(&mut pair).await.map_err(PairError::Vault)?;
        pair.status.hold_for_preview = true;
        self.store.lock().create_pair(&pair)?;
        Ok(pair)
    }

    /// Saves a pair's options, whichever app made the change: checked against the same rules as
    /// a new pair, and the pair's status stays the engine's. Another folder, Drive folder or vault
    /// starts the pair over and holds it for its preview; that is refused while the pair syncs
    /// and while an agent's changes on it wait (`Engine::update_pair`). A pair on a new vault's
    /// folder is linked first, as when it is created. Returns whether the pair moved.
    pub async fn update(&self, engine: &Engine, mut pair: Pair) -> Result<bool, PairError> {
        self.check(&pair)?;
        let before = self.store.lock().get_pair(&pair.id)?;
        let moved = before.as_ref().is_none_or(|b| {
            (&b.account_id, &b.drive_folder_id, &b.drive_id, &b.vault_id)
                != (
                    &pair.account_id,
                    &pair.drive_folder_id,
                    &pair.drive_id,
                    &pair.vault_id,
                )
        });
        // Advisory, ahead of the vault's remote call: a move refused afterwards would leave a vault
        // linked on Drive. `Engine::update_pair` decides under the lock a run takes (#134).
        if before.as_ref().is_some_and(|b| repointed(b, &pair))
            && self.store.lock().pair_has_agent_changes(&pair.id)?
        {
            return Err(Problem::AgentChanges.into());
        }
        if moved {
            if engine.is_running(&pair.id) {
                return Err(Problem::Syncing { pair: pair.name }.into());
            }
            self.env.link_vault(&mut pair).await.map_err(PairError::Vault)?;
        }
        let name = pair.name.clone();
        engine.update_pair(&pair).map_err(|e| refused(e, &name))
    }

    /// Removes a pair and everything that belongs to it, whichever app asks. Refused while an
    /// agent's changes on it wait; a running sync is stopped only once the pair is gone.
    pub fn delete(&self, engine: &Engine, id: &str) -> Result<(), PairError> {
        match self.store.lock().delete_pair(id) {
            Err(StoreError::Refused(m)) if m == AGENT_CHANGES_PENDING => {
                return Err(Problem::AgentChanges.into());
            }
            other => other?,
        }
        engine.cancel(id);
        Ok(())
    }
}

/// What the engine's own refusal of a save is, in this module's terms. The engine words the two
/// it can give (a run in the way, agent changes waiting); they are recognised by those words,
/// which [`Problem`] takes from the same places.
fn refused(e: EngineError, pair_name: &str) -> PairError {
    match e {
        EngineError::Other(m) if m == wasl_engine::syncing_refusal(pair_name) => Problem::Syncing {
            pair: pair_name.into(),
        }
        .into(),
        EngineError::Store(StoreError::Refused(m)) if m == AGENT_CHANGES_PENDING => {
            Problem::AgentChanges.into()
        }
        EngineError::Store(StoreError::Refused(m)) | EngineError::Other(m) => PairError::Refused(m),
        other => PairError::Engine(other),
    }
}

/// Re-exported from the store, which applies it on every save (`Store::update_pair`).
pub use wasl_store::repointed;

pub use wasl_core::paths::comparable;

/// A path as given and, where it exists, with links and short names resolved.
fn forms(p: &Path) -> Vec<PathBuf> {
    let mut v = vec![p.to_path_buf()];
    if let Ok(c) = std::fs::canonicalize(p) {
        v.push(c);
    }
    v
}

/// `inner` is `outer` or inside it, compared as the OS compares names (case-insensitively on
/// Windows).
fn contains(outer: &Path, inner: &Path) -> bool {
    let (o, i) = (parts(outer), parts(inner));
    i.len() >= o.len() && i[..o.len()] == o[..]
}

fn parts(p: &Path) -> Vec<String> {
    p.components()
        .filter(|c| !matches!(c, Component::CurDir))
        .map(|c| {
            let s = c.as_os_str().to_string_lossy().into_owned();
            if cfg!(windows) { s.to_lowercase() } else { s }
        })
        .collect()
}
