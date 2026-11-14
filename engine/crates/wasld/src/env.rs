//! What the engine needs from the desktop: plain folders, Drive (through a vault for a vault's
//! pairs), and the device's state.

use crate::vaults::Vaults;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use wasl_core::conditions::DeviceState;
use wasl_drive::{DriveClient, DriveNodeCache};
use wasl_engine::{DriveRemote, EngineError, Environment, Remote};
use wasl_fs::{LocalFs, StdFs};
use wasl_store::{Pair, SharedStore, StorageMode};

pub struct DesktopEnv {
    pub store: SharedStore,
    pub client: Arc<DriveClient>,
    pub cache: Arc<DriveNodeCache>,
    pub bin_dir: PathBuf,
    pub vaults: Arc<Vaults>,
    remotes: Mutex<HashMap<String, Arc<DriveRemote>>>,
}

impl DesktopEnv {
    pub fn new(
        store: SharedStore,
        client: Arc<DriveClient>,
        bin_dir: PathBuf,
        vaults: Arc<Vaults>,
    ) -> DesktopEnv {
        let cache = Arc::new(DriveNodeCache::new(client.clone(), Arc::new(store.clone())));
        DesktopEnv {
            store,
            client,
            cache,
            bin_dir,
            vaults,
            remotes: Mutex::new(HashMap::new()),
        }
    }

    /// The account's Drive, shared by its pairs and views (one node cache per account).
    pub fn drive_remote(&self, account: &str) -> Arc<DriveRemote> {
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

impl Environment for DesktopEnv {
    fn local_fs(&self, pair: &Pair) -> wasl_engine::Result<Arc<dyn LocalFs>> {
        match pair.storage_mode {
            // Bin next to the data folder: same volume for most pairs, so binning is a rename.
            StorageMode::Files => Ok(Arc::new(StdFs::new(
                &pair.local_root,
                self.bin_dir.join(&pair.id),
            ))),
            StorageMode::Saf => Err(EngineError::Permission(
                "This pair uses Android's folder picker; it syncs on the phone only.".into(),
            )),
        }
    }

    /// The account's Drive; for a vault pair, the vault's remote over it (a locked vault stops
    /// the pair with `Locked`).
    fn remote(&self, pair: &Pair) -> wasl_engine::Result<Arc<dyn Remote>> {
        let account = pair
            .account_id
            .clone()
            .ok_or_else(|| EngineError::NotFound("This pair has no Google account".into()))?;
        self.vaults.pair_remote(self.drive_remote(&account), pair)
    }

    /// A desktop is treated as online, on an unmetered connection and on power. Pair conditions
    /// about Wi-Fi and battery are phone conditions; network failures still stop runs cleanly.
    fn device_state(&self) -> DeviceState {
        DeviceState::unrestricted()
    }
}
