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
use wasl_store::{Pair, SharedStore, StoreError};

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
