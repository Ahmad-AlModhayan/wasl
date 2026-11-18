//! What the phone's screens read about folder status (issue #31, ADR 0003): the same states, keys
//! and words as the daemon's, worked out by the same engine rules, and a callback told once for
//! each folder whose status changed.
//!
//! The app asks for the statuses in its language ([`WaslEngine::folder_statuses`],
//! [`WaslEngine::overall_status`]) or watches them ([`WaslEngine::watch_status`]). A watch starts from
//! the statuses it returns and then calls [`StatusSink::on_folder_status`] for each folder whose
//! status is not what the app was last told. Nothing is decided here: the engine decides every
//! status, and `Devices::with_peers` names the device a folder waits for.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use wasl_devices::Devices;
use wasl_proto::{FolderStatus, Lang, OverallStatus, overall_status};
use wasl_store::{Pair, StorageMode};

use crate::{WaslEngine, WaslError};

/// Progress and device news come in bursts; statuses are decided at most this often for them.
const SETTLE: Duration = Duration::from_millis(120);

/// Is told when a folder's status changes. Called from a background thread, once per folder whose
/// status is not what the last call (or [`WaslEngine::watch_status`]) gave.
#[uniffi::export(with_foreign)]
pub trait StatusSink: Send + Sync {
    fn on_folder_status(&self, pair_id: String, status: FolderStatus);
}

/// The watch, the last statuses sent, and the thread that decides again after progress and device
/// news.
#[derive(Default)]
pub(crate) struct StatusWatch {
    watcher: Mutex<Option<(Lang, Arc<dyn StatusSink>)>>,
    sent: Mutex<HashMap<String, FolderStatus>>,
    /// Held while statuses are decided and sent; a call that finds it held only asks for another round.
    busy: Mutex<()>,
    again: AtomicBool,
    /// Wakes the thread (made with the first watch).
    poke: Mutex<Option<mpsc::Sender<()>>>,
    engine: OnceLock<Weak<WaslEngine>>,
}

impl StatusWatch {
    pub(crate) fn bind(&self, engine: &Arc<WaslEngine>) {
        let _ = self.engine.set(Arc::downgrade(engine));
    }

    fn watching(&self) -> bool {
        self.watcher.lock().unwrap_or_else(|p| p.into_inner()).is_some()
    }

    /// Asks for statuses to be decided again soon, off the caller's thread (it may hold the
    /// engine's locks: progress, device news).
    pub(crate) fn poke(&self) {
        if let Some(tx) = self.poke.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            let _ = tx.send(());
        }
    }

    fn start_thread(&self) {
        let mut poke = self.poke.lock().unwrap_or_else(|p| p.into_inner());
        if poke.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel::<()>();
        *poke = Some(tx);
        let engine = self.engine.get().cloned();
        std::thread::spawn(move || {
            while rx.recv().is_ok() {
                // One decision for a burst.
                std::thread::sleep(SETTLE);
                while rx.try_recv().is_ok() {}
                let Some(engine) = engine.as_ref().and_then(Weak::upgrade) else {
                    return;
                };
                engine.touched();
            }
        });
    }

    /// Decides again and tells the watcher what changed. A call made from inside the callback, or
    /// while another round runs, only asks for one more round.
    pub(crate) fn refresh(&self, engine: &WaslEngine) {
        if !self.watching() {
            return;
        }
        self.again.store(true, Ordering::SeqCst);
        let Ok(_round) = self.busy.try_lock() else {
            return;
        };
        while self.again.swap(false, Ordering::SeqCst) {
            let Some((lang, sink)) = self.watcher.lock().unwrap_or_else(|p| p.into_inner()).clone() else {
                return;
            };
            let Ok(now) = engine.statuses(lang) else {
                continue;
            };
            let changed: Vec<FolderStatus> = {
                let mut sent = self.sent.lock().unwrap_or_else(|p| p.into_inner());
                sent.retain(|id, _| now.iter().any(|s| &s.pair_id == id));
                now.into_iter()
                    .filter(|s| {
                        let same = sent.get(&s.pair_id) == Some(s);
                        if !same {
                            sent.insert(s.pair_id.clone(), s.clone());
                        }
                        !same
                    })
                    .collect()
            };
            for status in changed {
                sink.on_folder_status(status.pair_id.clone(), status);
            }
        }
    }
}

/// Free bytes the app may write on the drive holding `pair`'s folder: for a pair on a plain file
/// system; None for a picked folder (the grant has no path) and where it can't be told.
fn free_bytes(pair: &Pair) -> Option<u64> {
    if pair.storage_mode != StorageMode::Files {
        return None;
    }
    let path = Path::new(&pair.local_root);
    if !path.is_absolute() {
        return None;
    }
    available(path.ancestors().find(|p| p.exists())?)
}

#[cfg(unix)]
#[allow(clippy::unnecessary_cast)] // the field types differ by OS
fn available(path: &Path) -> Option<u64> {
    let st = nix::sys::statvfs::statvfs(path).ok()?;
    (st.blocks_available() as u64).checked_mul(st.fragment_size() as u64)
}

#[cfg(not(unix))]
fn available(_path: &Path) -> Option<u64> {
    None
}

impl WaslEngine {
    /// Every folder's status in `lang`, decided by the engine, with the devices each is shared with
    /// when this phone takes part in its device group.
    pub(crate) fn statuses(&self, lang: Lang) -> Result<Vec<FolderStatus>, WaslError> {
        let list = self.engine.folder_statuses(lang, &free_bytes, &mut |_| {})?;
        let devices = self.devices.lock().unwrap_or_else(|p| p.into_inner()).clone();
        Ok(match devices {
            Some(d) => d.with_peers(list, lang),
            None => list,
        })
    }

    pub(crate) fn set_devices(&self, devices: Option<Arc<Devices>>) {
        *self.devices.lock().unwrap_or_else(|p| p.into_inner()) = devices;
    }

    /// A call that can change a status has run: tell the watcher, if there is one.
    pub(crate) fn touched(&self) {
        self.watch.refresh(self);
    }

    /// Asks for the statuses to be decided again soon, from another thread.
    pub(crate) fn poke_status(&self) {
        self.watch.poke();
    }
}

#[uniffi::export]
impl WaslEngine {
    /// Every folder's status, in `lang`: the state, why it waits, what needs the person, and the
    /// words (each with its stable key and values). The same statuses the desktop's `wasl status`
    /// gives, decided by the same rules.
    pub fn folder_statuses(&self, lang: Lang) -> Result<Vec<FolderStatus>, WaslError> {
        self.statuses(lang)
    }

    /// The whole app's status in `lang`, worked out from the folder statuses by the same rules as
    /// the desktop's.
    pub fn overall_status(&self, lang: Lang) -> Result<OverallStatus, WaslError> {
        let folders = self.statuses(lang)?;
        let (pairs, accounts) = {
            let s = self.engine.store().lock();
            (s.list_pairs()?, s.accounts()?)
        };
        // A Google account to sign in again with no folder showing it still needs the person.
        let orphans = accounts
            .iter()
            .filter(|(_, _, auth_error)| auth_error.is_some())
            .filter(|(id, _, _)| !pairs.iter().any(|p| p.account_id.as_deref() == Some(id.as_str())))
            .count() as u32;
        Ok(overall_status(&folders, orphans, lang))
    }

    /// Starts telling `sink` when a folder's status changes, in `lang`, and returns the statuses as
    /// they are now: the sink hears only what changes after this. A second watch replaces the first.
    pub fn watch_status(
        &self,
        lang: Lang,
        sink: Arc<dyn StatusSink>,
    ) -> Result<Vec<FolderStatus>, WaslError> {
        let now = self.statuses(lang)?;
        {
            let mut sent = self.watch.sent.lock().unwrap_or_else(|p| p.into_inner());
            *sent = now.iter().map(|s| (s.pair_id.clone(), s.clone())).collect();
            *self.watch.watcher.lock().unwrap_or_else(|p| p.into_inner()) = Some((lang, sink));
        }
        self.watch.start_thread();
        Ok(now)
    }

    /// Stops telling the watcher.
    pub fn unwatch_status(&self) {
        *self.watch.watcher.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    /// Decides the statuses again now and tells the watcher what changed. For a change the engine
    /// can't see (the app changed a setting a status reads).
    pub fn refresh_status(&self) {
        self.touched();
    }
}
