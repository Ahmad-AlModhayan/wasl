//! Device sync for the phone (docs/DEVICE_SYNC.md, SRS FR-N12): this phone in the group, pairing,
//! and running pairs with the devices it is connected to.
//!
//! The app decides when the phone takes part: it opens a [`WaslDevices`] while the app is open,
//! while charging on Wi-Fi (when the person allows it) and during a sync session the person
//! started, and closes it otherwise. Outside those windows a pair synced only with devices waits;
//! the app says "Waiting for <device>". While open, the phone listens for its devices, finds them
//! on the LAN (the app holds a multicast lock) and, when a pair asks for it, beyond the LAN through
//! Drive.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use wasl_devices::{
    DeviceConfig, Devices, DevicesInfo, OfferInfo, PairSharing, Pairing, PairingStatus, ReachPolicy,
};
use wasl_engine::RunOutcome;
use wasl_net::{IdentityError, KeyStore};
use zeroize::Zeroizing;

use crate::{WaslEngine, WaslError, other};

/// Where the phone keeps its device key's 32-byte seed: wrapped by an Android Keystore key, so it
/// never leaves the device in the clear.
#[uniffi::export(with_foreign)]
pub trait DeviceKeyStore: Send + Sync {
    /// The stored seed, if any.
    fn load(&self) -> Result<Option<Vec<u8>>, WaslError>;
    /// Stores the seed, replacing any previous one.
    fn save(&self, seed: Vec<u8>) -> Result<(), WaslError>;
}

struct KeyStoreBridge(Arc<dyn DeviceKeyStore>);

impl KeyStore for KeyStoreBridge {
    fn load(&self) -> Result<Option<Zeroizing<[u8; 32]>>, IdentityError> {
        let Some(seed) = self
            .0
            .load()
            .map_err(|e| IdentityError::KeyStore(e.to_string()))?
        else {
            return Ok(None);
        };
        let seed = Zeroizing::new(seed);
        let arr: [u8; 32] = seed
            .as_slice()
            .try_into()
            .map_err(|_| IdentityError::KeyStore("the stored device key is damaged".into()))?;
        Ok(Some(Zeroizing::new(arr)))
    }

    fn save(&self, seed: &[u8; 32]) -> Result<(), IdentityError> {
        self.0
            .save(seed.to_vec())
            .map_err(|e| IdentityError::KeyStore(e.to_string()))
    }
}

/// Is told about changes the screens show (pairing stages, devices, a peer's news).
#[uniffi::export(with_foreign)]
pub trait DeviceEvents: Send + Sync {
    /// Pairing or the device list changed: read [`WaslDevices::pairing_status`] / `info` again.
    fn changed(&self);
    /// A device says `pair_id` changed there: sync it.
    fn pair_changed(&self, pair_id: String);
}

/// Where and how the phone takes part.
#[derive(Debug, Clone, uniffi::Record)]
pub struct DevicesInit {
    /// The app's private files folder (the group file lives here).
    pub data_dir: String,
    pub device_name: String,
    /// The UDP port to listen on; 0 picks one.
    pub port: u16,
    /// The address to listen on; `None` is every interface (tests use 127.0.0.1).
    pub bind_address: Option<String>,
    /// Announce and look for devices on the LAN (needs a multicast lock held by the app).
    pub discover: bool,
}

/// This phone as a device.
#[derive(uniffi::Object)]
pub struct WaslDevices {
    devices: Arc<Devices>,
    pairing: Arc<Pairing>,
    engine: Arc<WaslEngine>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// The runtime the object was opened on: calls from the app's threads enter it, so work
    /// they start (pairing, timers) has a reactor.
    rt: tokio::runtime::Handle,
}

#[uniffi::export(async_runtime = "tokio")]
impl WaslDevices {
    /// Starts taking part: loads (or creates) the key and the group, listens, and looks for the
    /// group's devices. Close it when the phone should stop.
    #[uniffi::constructor]
    pub async fn open(
        engine: Arc<WaslEngine>,
        init: DevicesInit,
        key_store: Arc<dyn DeviceKeyStore>,
        events: Arc<dyn DeviceEvents>,
    ) -> Result<Arc<WaslDevices>, WaslError> {
        let env = engine.env.clone();
        let open: wasl_devices::OpenFolder = Arc::new(move |p| {
            use wasl_engine::Environment as _;
            env.local_fs(p)
        });
        let cfg = DeviceConfig {
            listen: match &init.bind_address {
                Some(ip) => SocketAddr::new(ip.parse().map_err(other)?, init.port),
                None => SocketAddr::from(([0, 0, 0, 0], init.port)),
            },
            discover: init.discover,
        };
        let (devices, mut notified) = Devices::open(
            std::path::Path::new(&init.data_dir),
            &KeyStoreBridge(key_store),
            engine.engine.store().clone(),
            open,
            &init.device_name,
            &cfg,
        )
        .map_err(other)?;
        // Arrivals and offers change on their own (docs/INTRODUCTIONS.md).
        let ev = events.clone();
        devices.on_change(Arc::new(move || ev.changed()));
        let ev = events.clone();
        let pairing = Arc::new(Pairing::new(
            devices.clone(),
            &init.device_name,
            Arc::new(move || ev.changed()),
        ));
        let mut tasks = Vec::new();
        let ev = events.clone();
        tasks.push(tokio::spawn(async move {
            while let Some(pair_id) = notified.recv().await {
                ev.pair_changed(pair_id);
            }
        }));
        let policy = Arc::new(ReachPolicy::new(
            engine.engine.store().clone(),
            engine.env.client.clone(),
            std::path::PathBuf::from(&engine.env.init.staging_dir).join("rendezvous"),
        ));
        // The group files in Drive's app-data folder, for members never online together with
        // this phone (docs/SECURITY.md S2-1).
        let board = policy.clone();
        tasks.push(tokio::spawn(wasl_devices::board::run(
            devices.clone(),
            move || board.board(),
            wasl_devices::BoardTiming::default(),
        )));
        tasks.push(tokio::spawn(wasl_devices::reach::run(
            devices.clone(),
            move || policy.current(),
            wasl_net::stun::DEFAULT_SERVERS
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            wasl_devices::ReachTiming::default(),
        )));
        Ok(Arc::new(WaslDevices {
            devices,
            pairing,
            engine,
            tasks: Mutex::new(tasks),
            rt: tokio::runtime::Handle::current(),
        }))
    }

    /// This phone and the group's devices, with who is connected and how.
    pub fn info(&self) -> DevicesInfo {
        let _rt = self.rt.enter();
        let d = &self.devices;
        DevicesInfo {
            enabled: true,
            listen: d.local_addr().ok().map(|a| a.to_string()),
            devices: d.device_infos(),
        }
    }

    /// Opens a pairing code (shown as a QR) for a new device.
    pub fn invite(&self) -> Result<PairingStatus, WaslError> {
        let _rt = self.rt.enter();
        self.pairing.invite().map_err(other)
    }

    /// Joins the device whose code was scanned. Follow it with `pairing_status`.
    pub fn join(&self, code: String) -> Result<(), WaslError> {
        let _rt = self.rt.enter();
        self.pairing.join(&code).map_err(other)
    }

    pub fn pairing_status(&self) -> PairingStatus {
        let _rt = self.rt.enter();
        self.pairing.status()
    }

    /// The person says the four words match (`accept`), or not.
    pub fn confirm_pairing(&self, accept: bool) -> Result<PairingStatus, WaslError> {
        let _rt = self.rt.enter();
        self.pairing.confirm(accept).map_err(other)
    }

    pub fn cancel_pairing(&self) {
        let _rt = self.rt.enter();
        self.pairing.cancel();
    }

    /// What device sync may spend on compression (docs/COMPRESSION.md §2.3): on a metered network
    /// it asks peers to compress harder; on battery it compresses what it sends only lightly. Call
    /// when the network or the charger changes.
    pub fn set_budget(&self, metered: bool, on_battery: bool) {
        let _rt = self.rt.enter();
        self.devices.set_budget(metered, on_battery);
    }

    /// Removes another device from the group.
    pub fn remove_device(&self, id: String) -> Result<(), WaslError> {
        let _rt = self.rt.enter();
        let id = wasl_net::DeviceId::parse(&id).map_err(other)?;
        if id == self.devices.id() {
            return Err(other("Remove this phone from another of your devices."));
        }
        self.devices.remove_member(id).map_err(other)?;
        Ok(())
    }

    /// Connects to a device of the group at `host:port`.
    pub async fn connect(&self, address: String) -> Result<String, WaslError> {
        let addrs: Vec<SocketAddr> = tokio::net::lookup_host(address.as_str())
            .await
            .map_err(other)?
            .collect();
        let mut last = format!("Can't find {address}");
        for a in addrs {
            match self.devices.connect(a).await {
                Ok(id) => return Ok(id.to_string()),
                Err(e) => last = e,
            }
        }
        Err(WaslError::Network { detail: last })
    }

    /// Shares a pair with the group under a new code, or under `code` from another device;
    /// a pair already shared keeps its code. Returns the code.
    pub fn share_pair(&self, pair_id: String, code: Option<String>) -> Result<String, WaslError> {
        let _rt = self.rt.enter();
        let store = self.engine.engine.store().clone();
        let s = store.lock();
        let pair = s.get_pair(&pair_id)?.ok_or_else(|| WaslError::NotFound {
            detail: format!("No pair {pair_id}"),
        })?;
        let existing = s.share_id(&pair_id)?;
        let share = match (code, existing) {
            (Some(c), _) => wasl_net::ShareId::from_hex(c.trim()).ok_or_else(|| other("Not a share code"))?,
            (None, Some(old)) => {
                wasl_net::ShareId::from_hex(&old).ok_or_else(|| other("This pair's share code is damaged"))?
            }
            (None, None) => wasl_net::ShareId::random(),
        };
        if let Some(other_pair) = s.pair_for_share(&share.to_hex())?
            && other_pair != pair_id
        {
            return Err(other("Another pair on this phone already uses that share code."));
        }
        s.set_share_id(&pair_id, &share.to_hex())?;
        if pair.route == wasl_core::Route::Drive {
            let route = if pair.drive_folder_id.is_some() {
                wasl_core::Route::DriveAndDevices
            } else {
                wasl_core::Route::Devices
            };
            s.set_pair_route(&pair_id, route)?;
        }
        Ok(share.to_hex())
    }

    pub fn share_code(&self, pair_id: String) -> Result<Option<String>, WaslError> {
        let _rt = self.rt.enter();
        Ok(self.engine.engine.store().lock().share_id(&pair_id)?)
    }

    /// Stops syncing a pair with devices; a pair with a Drive folder goes back to Drive only.
    pub fn unshare_pair(&self, pair_id: String) -> Result<(), WaslError> {
        let _rt = self.rt.enter();
        let store = self.engine.engine.store().clone();
        let s = store.lock();
        s.remove_share_id(&pair_id)?;
        if let Some(pair) = s.get_pair(&pair_id)?
            && pair.route != wasl_core::Route::Drive
            && pair.drive_folder_id.is_some()
        {
            s.set_pair_route(&pair_id, wasl_core::Route::Drive)?;
        }
        Ok(())
    }

    /// Keeps a device another device added (docs/INTRODUCTIONS.md §3.1): it is known on this
    /// phone and gets the folders shared with all devices.
    pub fn keep_device(&self, id: String) -> Result<(), WaslError> {
        let _rt = self.rt.enter();
        let id = wasl_net::DeviceId::parse(&id).map_err(other)?;
        self.devices.keep_device(id).map_err(other)
    }

    /// Who a shared pair is served to; `None` when it isn't shared.
    pub fn sharing(&self, pair_id: String) -> Result<Option<PairSharing>, WaslError> {
        let _rt = self.rt.enter();
        self.devices.sharing(&pair_id).map_err(other)
    }

    /// Sets who a pair shared with `share_pair` is served to (§3.2): every device known here
    /// (`all_devices`), and the `devices` named (`WASL-…`), each of which is offered the pair.
    pub fn share_with(
        &self,
        pair_id: String,
        all_devices: bool,
        devices: Vec<String>,
    ) -> Result<(), WaslError> {
        let _rt = self.rt.enter();
        let ids = devices
            .iter()
            .map(|d| wasl_net::DeviceId::parse(d).map_err(other))
            .collect::<Result<Vec<_>, _>>()?;
        self.devices
            .share_with(&pair_id, all_devices, &ids)
            .map_err(other)
    }

    /// The folders the group's devices offer this phone (§3.3), each sender apart.
    pub fn offers(&self) -> Vec<OfferInfo> {
        let _rt = self.rt.enter();
        self.devices.offers()
    }

    /// Hides `from`'s offer of `share` on this phone.
    pub fn decline_offer(&self, share: String, from: String) -> Result<(), WaslError> {
        let _rt = self.rt.enter();
        let from = wasl_net::DeviceId::parse(&from).map_err(other)?;
        self.devices.decline_offer(&share, from).map_err(other)
    }

    /// Accepts `from`'s offer of `share` for the pair `pair_id`, which the app has just made the
    /// way it adds any pair (held for its first-sync preview), in place of `share_pair`: the pair
    /// is shared under the offered share with the offering device only, never with "all my
    /// devices" (docs/INTRODUCTIONS.md §3.3), and the offer is marked accepted. A Drive pair also
    /// syncs with devices from then on, as `share_pair` makes it: the phone has no devices-only
    /// pairs yet, so its pairs always have a Drive folder (E2E 2026-10, G4).
    pub fn accept_offer(&self, pair_id: String, share: String, from: String) -> Result<(), WaslError> {
        let _rt = self.rt.enter();
        let from = wasl_net::DeviceId::parse(&from).map_err(other)?;
        let store = self.engine.engine.store().clone();
        let pair = store
            .lock()
            .get_pair(&pair_id)?
            .ok_or_else(|| WaslError::NotFound {
                detail: format!("No pair {pair_id}"),
            })?;
        // The route changes in the same transaction as the share and the offer: a failure leaves
        // nothing half accepted (review of #28).
        let route = (pair.route == wasl_core::Route::Drive).then(|| {
            if pair.drive_folder_id.is_some() {
                wasl_core::Route::DriveAndDevices
            } else {
                wasl_core::Route::Devices
            }
        });
        self.devices
            .accept_offer(&pair_id, &share, from, route)
            .map_err(other)?;
        Ok(())
    }

    /// Syncs a pair with the connected devices (its Drive part runs through `WaslEngine::sync`).
    pub async fn sync_pair(&self, pair_id: String) -> Result<RunOutcome, WaslError> {
        let pair = self
            .engine
            .engine
            .store()
            .lock()
            .get_pair(&pair_id)?
            .ok_or_else(|| WaslError::NotFound {
                detail: format!("No pair {pair_id}"),
            })?;
        Ok(self.devices.sync_pair(&pair, wasl_engine::Trigger::Manual).await)
    }

    /// Stops taking part (named apart from Kotlin's `close`, which frees the object): closes the endpoint, discovery and the background work.
    pub fn shutdown(&self) {
        let _rt = self.rt.enter();
        self.pairing.cancel();
        for t in self.tasks.lock().unwrap_or_else(|p| p.into_inner()).drain(..) {
            t.abort();
        }
        self.devices.close();
    }
}
