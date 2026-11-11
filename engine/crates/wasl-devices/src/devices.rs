//! Device sync in the daemon (docs/DEVICE_SYNC.md §2–7): this computer's device key, its group,
//! the QUIC endpoint other devices reach it on, LAN discovery, and running pairs whose route
//! includes devices.
//!
//! The key's seed lives where the caller's [`KeyStore`] keeps it (the OS keychain on a computer). The group's operations live
//! in `group.json` in the data folder (they are signed, so the file needs no secrecy; it is
//! checked again on every load). Peers are found by mDNS and connected to with the group as the
//! only trust: a device outside the group cannot complete a handshake, either way.
//!
//! Members keep each other's group up to date (security review 2026-10): on every new connection,
//! at every run, every [`GROUP_EVERY`] and as soon as a member says its group changed, a device
//! asks each connected member for the operations it lacks and takes in those that count. A
//! removal therefore reaches every member that is connected to one that knows it, and the removed
//! device is cut off there at once (the endpoint's trust and the gate read the group live); an
//! addition spreads the same way, so a device paired through one member is trusted by the others.
//! Devices that are never online together meet through their group files in Drive's app-data
//! folder instead ([`crate::board`], docs/SECURITY.md S2-1), taken in the same way. Each signer's
//! new operations are taken in at a bounded rate, whichever way they come ([`SignerThrottle`]).
//!
//! Introductions (docs/INTRODUCTIONS.md, 2026-10): this device keeps a note on each member,
//! known or new. Members present when the notes are first made (an upgrade) are known, and so are
//! this device, a device it adds by pairing and the device it joins through; any other member that
//! arrives through the group is new, and gets no folder from this device until its person keeps
//! it ([`Devices::keep_device`]). Each exchange with a member also asks it for the folders it
//! offers this device, kept in the store under that member's authenticated ID.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tokio::sync::mpsc;
use wasl_chunk::{ChunkList, Hash};
use wasl_core::{PlanSummary, RunStatus};
use wasl_engine::{DeviceService, DeviceSync, EngineError, RunOutcome, SystemClock, Trigger};
use wasl_fs::LocalFs;
use wasl_net::compress;
use wasl_net::discover::{Lan, Token};
use wasl_net::group::{Screened, SignerThrottle};
use wasl_net::rendezvous::is_lan_ip;
use wasl_net::{
    DeviceId, DeviceKey, Endpoint, GroupOp, GroupOps, IndexPage, KeyStore, OpId, PairReply, PairRound,
    PeerError, PeerLink, PeerResult, PeerService, QuicLink, ShareId, Trust,
};
use wasl_store::{NewOffer, OfferState, Pair, RunStats, SharedStore};

use crate::status::{DeviceInfo, LinkPath, OfferInfo, OfferStatus, PairSharing};

/// An offer this device no longer has from that device: withdrawn, or the device left the group.
pub const NO_LONGER_OFFERED: &str =
    "That device no longer offers this folder. Ask on that device to share it with this one again.";
/// A device this device's group no longer has (removed elsewhere, or never added).
pub const NOT_IN_GROUP: &str =
    "That device isn't one of your devices any more. Add it again under Devices to sync with it.";
/// A pair with no device share: nothing to sync, and nothing to preview.
pub const NOT_SHARED: &str = "This pair isn't shared with any device yet.";
/// A run that found no device to sync with: none connected that has this folder (#148).
pub const NO_PEER: &str =
    "No device that syncs this folder is connected. Add a device, or offer this folder to one.";

/// How often connected members compare groups even when nobody says theirs changed.
pub const GROUP_EVERY: Duration = Duration::from_secs(60);
/// How long a device found on the LAN gets to complete the handshake.
pub const DISCOVERY_DIAL_TIMEOUT: Duration = Duration::from_secs(3);
/// Devices found on the LAN dialed at once.
pub const DISCOVERY_DIALS: usize = 4;
/// Most offers kept from one device, and in all (docs/INTRODUCTIONS.md §4).
pub const MAX_OFFERS_PER_DEVICE: usize = 64;
pub const MAX_OFFERS_KEPT: usize = 256;

/// Is told when what the screens show about devices changed on its own (an arrival, an offer).
pub type OnChange = Arc<dyn Fn() + Send + Sync>;

/// Bidi controls: they could make a name read as another (security review 2026-10).
fn is_bidi(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// A device name as shown and kept: no control or bidi characters, at most 64 of them, not blank.
pub fn clean_name(n: &str) -> Option<String> {
    let n: String = n
        .chars()
        .filter(|c| !c.is_control() && !is_bidi(*c))
        .take(64)
        .collect();
    (!n.trim().is_empty()).then_some(n)
}

/// An offered folder's name as shown and kept: cleaned like a device name, at most 128 bytes.
fn clean_offer_name(n: &str) -> Option<String> {
    let n: String = n.chars().filter(|c| !c.is_control() && !is_bidi(*c)).collect();
    let n = wasl_net::peer::offer_name(&n);
    (!n.trim().is_empty()).then_some(n)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Opens a pair's local folder.
pub type OpenFolder = Arc<dyn Fn(&Pair) -> wasl_engine::Result<Arc<dyn LocalFs>> + Send + Sync>;

/// How device sync runs in this daemon.
#[derive(Debug, Clone)]
pub struct DeviceConfig {
    /// Where the endpoint listens; port 0 picks a free one.
    pub listen: SocketAddr,
    /// Announce and look for devices on the LAN (off in tests).
    pub discover: bool,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([0, 0, 0, 0], 0)),
            discover: true,
        }
    }
}

/// This device in its group.
pub struct Devices {
    key: DeviceKey,
    pairing: PairingHost,
    group: Arc<Mutex<GroupOps>>,
    group_path: PathBuf,
    /// How fast each signer's operations are taken in (S2-2).
    throttle: Mutex<SignerThrottle>,
    endpoint: Endpoint,
    links: Mutex<HashMap<DeviceId, QuicLink>>,
    /// How each link was made (kept with the link it describes).
    paths: Mutex<HashMap<DeviceId, LinkPath>>,
    store: SharedStore,
    open_folder: OpenFolder,
    running: Mutex<HashSet<String>>,
    again: Mutex<HashSet<String>>,
    token: Token,
    lan: Mutex<Option<Lan>>,
    /// Background work (group exchange, discovery), stopped by [`Devices::close`].
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// This device is on a metered network (mobile data): ask peers to compress harder.
    metered: AtomicBool,
    /// Told when arrivals or offers change (see [`Devices::on_change`]).
    on_change: Mutex<Option<OnChange>>,
}

/// The zstd level to ask a peer for over `path` (docs/COMPRESSION.md §2.3).
pub fn compression_level(path: LinkPath, metered: bool) -> u8 {
    match (path, metered) {
        (_, true) => compress::LEVEL_METERED,
        (LinkPath::Lan, false) => compress::LEVEL_LAN,
        (LinkPath::Direct | LinkPath::Punched, false) => compress::LEVEL_INTERNET,
    }
}

impl Devices {
    /// Loads (or creates) the key and the group and starts the endpoint. Pairs a peer says have
    /// changed are sent on the returned channel, to be run.
    pub fn open(
        data_dir: &Path,
        key_store: &dyn KeyStore,
        store: SharedStore,
        open_folder: OpenFolder,
        device_name: &str,
        cfg: &DeviceConfig,
    ) -> Result<(Arc<Devices>, mpsc::UnboundedReceiver<String>), String> {
        let key = DeviceKey::load_or_create(key_store).map_err(|e| e.to_string())?;
        let group_path = data_dir.join("group.json");
        let group = match load_group(&group_path)? {
            Some(g) => g,
            None => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                let (g, _) = GroupOps::create(&key, device_name, now).map_err(|e| e.to_string())?;
                save_group(&group_path, &g)?;
                g
            }
        };
        if !group.state().trusts_key(&key.public()) {
            return Err(
                "This computer was removed from its device group. Pair it again to sync with devices.".into(),
            );
        }
        let group = Arc::new(Mutex::new(group));
        let pairing: PairingHost = Arc::new(Mutex::new(None));
        let (g, open) = (group.clone(), pairing.clone());
        let me = key.public();
        // Members only, and never this device's own key from elsewhere. While a pairing code is
        // open any key may connect, but the gate below lets a non-member do nothing but pair,
        // and trust is asked again before every request, so a stranger is cut off as soon as
        // the code closes.
        let trust: Trust = Arc::new(move |pk: &[u8; 32]| {
            *pk != me
                && (g.lock().unwrap_or_else(|p| p.into_inner()).state().trusts_key(pk)
                    || open.lock().unwrap_or_else(|p| p.into_inner()).is_some())
        });
        let endpoint = Endpoint::bind(cfg.listen, &key, trust).map_err(|e| e.to_string())?;
        let (tx, rx) = mpsc::unbounded_channel();
        let (group_tx, group_rx) = mpsc::unbounded_channel();
        let folders = open_folder.clone();
        let service = Arc::new(DeviceService::new(
            store.clone(),
            move |p| folders(p),
            move |_, pair_id, _| {
                let _ = tx.send(pair_id);
            },
        ));
        endpoint.serve(Arc::new(Gate {
            inner: service,
            group: group.clone(),
            pairing: pairing.clone(),
            group_changed: group_tx,
        }));
        let devices = Arc::new(Devices {
            key,
            pairing,
            group,
            group_path,
            throttle: Mutex::new(SignerThrottle::default()),
            endpoint,
            links: Mutex::new(HashMap::new()),
            paths: Mutex::new(HashMap::new()),
            store,
            open_folder,
            running: Mutex::new(HashSet::new()),
            again: Mutex::new(HashSet::new()),
            token: Token::random(),
            lan: Mutex::new(None),
            tasks: Mutex::new(Vec::new()),
            metered: AtomicBool::new(false),
            on_change: Mutex::new(None),
        });
        devices.note_members();
        let exchange = tokio::spawn(exchange_groups(Arc::downgrade(&devices), group_rx));
        devices.tasks().push(exchange);
        if cfg.discover {
            devices.clone().start_discovery();
        }
        Ok((devices, rx))
    }

    fn tasks(&self) -> std::sync::MutexGuard<'_, Vec<tokio::task::JoinHandle<()>>> {
        self.tasks.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Members of the group only, never this device's own key, and never a stranger even while a
    /// pairing code is open: what this device dials with, so a stranger it reaches never sees
    /// this device's certificate (security review 2026-10).
    fn members_only(&self) -> Trust {
        let (group, me) = (self.group.clone(), self.key.public());
        Arc::new(move |pk: &[u8; 32]| {
            *pk != me
                && group
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .state()
                    .trusts_key(pk)
        })
    }

    pub fn id(&self) -> DeviceId {
        self.key.id()
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.key.public()
    }

    pub fn key(&self) -> &DeviceKey {
        &self.key
    }

    /// Lets `host` answer pairing rounds on this device's endpoint until [`Devices::close_pairing`].
    pub fn open_pairing(&self, host: Arc<dyn PeerService>) {
        *self.pairing.lock().unwrap_or_else(|p| p.into_inner()) = Some(host);
    }

    /// Whether `host` is the one answering pairing rounds now.
    pub fn pairing_host_is(&self, host: &Arc<dyn PeerService>) -> bool {
        self.pairing
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_some_and(|h| Arc::ptr_eq(h, host))
    }

    pub fn close_pairing(&self) {
        *self.pairing.lock().unwrap_or_else(|p| p.into_inner()) = None;
        self.drop_untrusted();
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.endpoint.local_addr()
    }

    /// The group's members.
    pub fn members(&self) -> Vec<wasl_net::GroupDevice> {
        self.group().state().devices().to_vec()
    }

    /// Every group operation, to hand to a device joining or to another member.
    pub fn group_ops(&self) -> Vec<GroupOp> {
        self.group().ops().into_iter().cloned().collect()
    }

    /// The create the group grew from: its identity in file names.
    pub fn genesis(&self) -> OpId {
        self.group().genesis()
    }

    /// The group operations nothing builds on yet: they change exactly when the group does.
    pub fn group_heads(&self) -> Vec<OpId> {
        self.group().heads()
    }

    fn group(&self) -> std::sync::MutexGuard<'_, GroupOps> {
        self.group.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Adds a device (after pairing confirmed it) and saves the group. The person here checked
    /// the words, so it is known on this device.
    pub fn add_member(&self, pk: [u8; 32], name: &str, now: u64) -> Result<GroupOp, String> {
        let mut g = self.group();
        let op = g
            .add_device(pk, name, now, &self.key)
            .map_err(|e| e.to_string())?;
        g.insert(op.clone()).map_err(|e| e.to_string())?;
        save_group(&self.group_path, &g)?;
        drop(g);
        self.store
            .lock()
            .set_device_known(&DeviceId::from_public_key(&pk).to_string(), true)
            .map_err(|e| e.to_string())?;
        self.announce_group(None);
        Ok(op)
    }

    /// Calls `f` when arrivals or offers change without a request (the daemon sends its clients
    /// an event; the phone tells the app).
    pub fn on_change(&self, f: OnChange) {
        *self.on_change.lock().unwrap_or_else(|p| p.into_inner()) = Some(f);
    }

    fn changed(&self) {
        let f = self.on_change.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if let Some(f) = f {
            f();
        }
    }

    /// Notes members this device has no note on yet (docs/INTRODUCTIONS.md §3.1): when there are
    /// no notes at all (the first run of introductions, an upgrade included) every member is
    /// known, so nothing changes for devices that already sync; afterwards an arrival is new. This
    /// device is always known. True if a new member was noted.
    fn note_members(&self) -> bool {
        let members = self.members();
        let s = self.store.lock();
        // A store that cannot say counts as noted: arrivals are then new (fail closed).
        let first = !s.has_device_notes().unwrap_or(true);
        let mut arrived = false;
        for m in &members {
            if s.note_device(&m.id.to_string(), first).unwrap_or(false) && !first {
                arrived = true;
            }
        }
        let _ = s.set_device_known(&self.key.id().to_string(), true);
        arrived
    }

    /// The person here keeps a member another device added: it is known on this device and
    /// gets the folders shared with all devices (§3.1). Other devices decide for themselves.
    pub fn keep_device(&self, id: DeviceId) -> Result<(), String> {
        if !self.members().iter().any(|m| m.id == id) {
            return Err(NOT_IN_GROUP.into());
        }
        self.store
            .lock()
            .set_device_known(&id.to_string(), true)
            .map_err(|e| e.to_string())?;
        self.changed();
        Ok(())
    }

    /// The group's members as the screens show them, with who added each and whether it is
    /// known here.
    pub fn device_infos(&self) -> Vec<DeviceInfo> {
        let (me, connected) = (self.key.id(), self.connected());
        let notes: HashMap<String, bool> = self
            .store
            .lock()
            .device_notes()
            .unwrap_or_default()
            .into_iter()
            .collect();
        self.members()
            .into_iter()
            .map(|m| DeviceInfo {
                id: m.id.to_string(),
                this_device: m.id == me,
                connected: connected.contains(&m.id),
                path: self.path_of(&m.id),
                added_at: m.added_at,
                added_by: (m.added_by != m.id).then(|| m.added_by.to_string()),
                known: m.id == me || notes.get(&m.id.to_string()).copied().unwrap_or(false),
                name: m.name,
            })
            .collect()
    }

    /// Asks the member at the other end of `link` for the folders it offers this device and keeps
    /// them (§3.3): under the member's authenticated ID, names cleaned, first seen by this device's
    /// clock, offers it no longer makes dropped unless accepted, within the limits (§4). True if
    /// anything changed.
    pub async fn fetch_offers(&self, link: &QuicLink) -> Result<bool, String> {
        let peer = link.peer();
        if peer == self.key.id() || !self.members().iter().any(|m| m.id == peer) {
            return Ok(false);
        }
        let offers = link.offers().await.map_err(|e| e.to_string())?;
        let offers: Vec<NewOffer> = offers
            .into_iter()
            .take(wasl_net::peer::MAX_OFFERS)
            .filter_map(|o| {
                Some(NewOffer {
                    share: o.share.to_hex(),
                    name: clean_offer_name(&o.name)?,
                    files: o.files,
                    bytes: o.bytes,
                })
            })
            .collect();
        let changed = {
            let mut s = self.store.lock();
            let changed = s
                .put_received_offers(&peer.to_string(), &offers, now_ms())
                .map_err(|e| e.to_string())?;
            s.trim_received_offers(MAX_OFFERS_PER_DEVICE, MAX_OFFERS_KEPT)
                .map_err(|e| e.to_string())?;
            changed
        };
        if changed {
            self.changed();
        }
        Ok(changed)
    }

    /// Folders the group's members offer this device, oldest first, each sender apart (a share
    /// offered by two devices shows twice). Offers from a device no longer in the group are left
    /// out, and so are offers of a folder a pair here already syncs: a device that accepted this
    /// device's folder lists it back, and offering a folder to the device it came from is no
    /// offer (review of #28: the sender saw "*Phone* offers you *Projects*" for its own folder).
    pub fn offers(&self) -> Vec<OfferInfo> {
        let names: HashMap<DeviceId, String> = self
            .members()
            .into_iter()
            .filter(|m| m.id != self.key.id())
            .map(|m| (m.id, m.name))
            .collect();
        let held = {
            let s = self.store.lock();
            s.received_offers()
                .unwrap_or_default()
                .into_iter()
                .filter(|o| {
                    o.state == OfferState::Accepted || s.pair_for_share(&o.share).ok().flatten().is_none()
                })
                .collect::<Vec<_>>()
        };
        held.into_iter()
            .filter_map(|o| {
                let from = DeviceId::parse(&o.from_device).ok()?;
                Some(OfferInfo {
                    from_name: names.get(&from)?.clone(),
                    share: o.share,
                    from_device: o.from_device,
                    name: o.name,
                    files: o.files,
                    bytes: o.bytes,
                    first_seen: o.first_seen,
                    status: match o.state {
                        OfferState::New => OfferStatus::New,
                        OfferState::Declined => OfferStatus::Declined,
                        OfferState::Accepted => OfferStatus::Accepted,
                    },
                })
            })
            .collect()
    }

    /// `from`'s offer of `share`, if a member makes it.
    pub fn offer(&self, share: &str, from: DeviceId) -> Option<OfferInfo> {
        let from = from.to_string();
        self.offers()
            .into_iter()
            .find(|o| o.share == share && o.from_device == from)
    }

    /// Hides `from`'s offer of `share` on this device. Its sender sees it not accepted yet.
    pub fn decline_offer(&self, share: &str, from: DeviceId) -> Result<(), String> {
        match self.offer(share, from) {
            Some(o) if o.status == OfferStatus::Accepted => {
                Err("That folder already syncs here. Remove its pair to stop it.".into())
            }
            Some(_) => {
                self.store
                    .lock()
                    .set_offer_state(share, &from.to_string(), OfferState::Declined)
                    .map_err(|e| e.to_string())?;
                self.changed();
                Ok(())
            }
            None => Err(NO_LONGER_OFFERED.into()),
        }
    }

    /// Makes the pair `pair_id` sync `from`'s offer of `share` (the caller made the pair, held for
    /// its first preview): shared under that share with `from` only, never with "all my devices"
    /// (§3.3: accepting a folder must not widen who it reaches; the person can widen it later),
    /// its route set to `route` when given, and the offer marked accepted, all in one transaction.
    /// Refused when another pair here already syncs the share or the pair is shared already.
    pub fn accept_offer(
        &self,
        pair_id: &str,
        share: &str,
        from: DeviceId,
        route: Option<wasl_core::Route>,
    ) -> Result<(), String> {
        if self.offer(share, from).is_none() {
            return Err(NO_LONGER_OFFERED.into());
        }
        self.store
            .lock()
            .accept_received_share(pair_id, share, &from.to_string(), route)
            .map_err(|e| e.to_string())?;
        self.changed();
        Ok(())
    }

    /// Who the shared pair `pair_id` is served to (§3.2): all devices known here, and the devices
    /// it is shared with by name, which are offered it. Replaces the list; a device left off it is
    /// refused at its next request. Members tell each other their groups changed, so the devices
    /// listed ask for their offers now.
    pub fn share_with(&self, pair_id: &str, all_devices: bool, devices: &[DeviceId]) -> Result<(), String> {
        let members: HashSet<DeviceId> = self.members().iter().map(|m| m.id).collect();
        for d in devices {
            if *d == self.key.id() {
                return Err("A folder is always on this device; pick other devices.".into());
            }
            if !members.contains(d) {
                return Err(format!(
                    "{d} isn't one of your devices. Add it under Devices first, then share the folder with it."
                ));
            }
        }
        {
            let mut s = self.store.lock();
            if s.share_id(pair_id).map_err(|e| e.to_string())?.is_none() {
                return Err("Share this pair with your devices first.".into());
            }
            let mut ids: Vec<String> = devices.iter().map(ToString::to_string).collect();
            ids.sort();
            ids.dedup();
            s.set_share_scope(pair_id, all_devices, &ids)
                .map_err(|e| e.to_string())?;
        }
        self.announce_group(None);
        Ok(())
    }

    /// Who the pair `pair_id` is shared with, if it is shared; devices no longer in the group are
    /// left out.
    pub fn sharing(&self, pair_id: &str) -> Result<Option<PairSharing>, String> {
        let members: HashSet<String> = self.members().iter().map(|m| m.id.to_string()).collect();
        let s = self.store.lock();
        let Some(share) = s.share_id(pair_id).map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let all_devices = s
            .share_all_devices(pair_id)
            .map_err(|e| e.to_string())?
            .unwrap_or(true);
        let devices = s
            .share_devices(pair_id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|d| members.contains(d))
            .collect();
        Ok(Some(PairSharing {
            share,
            all_devices,
            devices,
        }))
    }

    /// Removes a device and closes any connection to it.
    pub fn remove_member(&self, id: DeviceId) -> Result<GroupOp, String> {
        let op = {
            let mut g = self.group();
            let op = g.remove_device(id, &self.key).map_err(|e| e.to_string())?;
            g.insert(op.clone()).map_err(|e| e.to_string())?;
            // Closed while the group is locked, as in `receive_ops`, and before a save that may fail.
            if let Some(link) = self.links().remove(&id) {
                link.close();
            }
            save_group(&self.group_path, &g)?;
            op
        };
        self.announce_group(None);
        Ok(op)
    }

    /// Takes in operations another member sent or a member's group file held (each screened by
    /// [`GroupOps::screen`], then checked by [`GroupOps::insert`] within its signer's rate); true
    /// if the members changed. A removal takes effect at once: the endpoint's trust and the gate read the group live, and
    /// links to removed devices close.
    pub fn receive_ops(&self, ops: &[GroupOp]) -> Result<bool, String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut g = self.group();
        let before = g.state().clone();
        let heads = g.heads();
        let mut throttle = self.throttle.lock().unwrap_or_else(|p| p.into_inner());
        for op in ops.iter().take(wasl_net::group::MAX_OPS) {
            // Screened before anyone is charged: operations already kept or waiting cost nothing,
            // forged ones are dropped (whoever writes a group file in Drive could otherwise use up
            // a member's share in its name, its removal of the forger included), and signers not
            // known yet share one share.
            let take = match g.screen(op) {
                Screened::Held | Screened::Refused => false,
                Screened::Signed(signer) => throttle.allow(signer, now),
                Screened::Unknown => throttle.allow_unknown(now),
            };
            if take {
                let _ = g.insert(op.clone());
            }
        }
        drop(throttle);
        let changed = *g.state() != before;
        if changed {
            // Trust reads the group live, so links to devices it no longer lists close before the
            // group is unlocked: no one sees a device untrusted but still connected. And before
            // the save, so a failed save leaves no such link open either.
            self.close_untrusted(&g);
        }
        if g.heads() != heads {
            // New operations, counting or waiting for their parents: kept across restarts.
            save_group(&self.group_path, &g)?;
        }
        drop(g);
        if changed {
            // A member that arrived through another device is new here (§3.1).
            if self.note_members() {
                self.changed();
            }
            self.announce_group(None);
        }
        Ok(changed)
    }

    /// Asks the member at the other end of `link` for the group operations this device lacks and
    /// takes them in, then for the folders it offers (docs/INTRODUCTIONS.md §3.3: on every link,
    /// every minute and whenever it says something changed). True if the members changed.
    pub async fn exchange_group(&self, link: &QuicLink) -> Result<bool, String> {
        let heads = self.group().heads();
        let ops = link.group_ops(heads).await.map_err(|e| e.to_string())?;
        let changed = if ops.is_empty() {
            false
        } else {
            self.receive_ops(&ops)?
        };
        let _ = self.fetch_offers(link).await;
        Ok(changed)
    }

    /// [`Devices::exchange_group`] with every connected member.
    pub async fn exchange_groups(&self) {
        let links: Vec<QuicLink> = self.links().values().cloned().collect();
        for link in links {
            let _ = self.exchange_group(&link).await;
        }
    }

    /// Tells connected members (but `except`) that this device's group changed, so they ask for
    /// the news now rather than at their next exchange.
    fn announce_group(&self, except: Option<DeviceId>) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        for link in self.links().values().filter(|l| Some(l.peer()) != except) {
            let link = link.clone();
            rt.spawn(async move {
                let _ = link.group_changed().await;
            });
        }
    }

    /// Joins the group a paired peer belongs to, replacing this device's own group. The device
    /// joined through is known here (its person checked the words); the group's other members are
    /// new until kept (docs/INTRODUCTIONS.md §3.1).
    pub fn join(&self, ops: &[GroupOp], peer_pk: &[u8; 32]) -> Result<(), String> {
        let joined = GroupOps::bootstrap(ops, peer_pk).map_err(|e| e.to_string())?;
        if !joined.state().trusts_key(&self.key.public()) {
            return Err("That group does not list this computer yet.".into());
        }
        save_group(&self.group_path, &joined)?;
        {
            let mut g = self.group();
            *g = joined;
            self.close_untrusted(&g);
        }
        self.store
            .lock()
            .set_device_known(&DeviceId::from_public_key(peer_pk).to_string(), true)
            .map_err(|e| e.to_string())?;
        if self.note_members() {
            self.changed();
        }
        Ok(())
    }

    fn links(&self) -> std::sync::MutexGuard<'_, HashMap<DeviceId, QuicLink>> {
        self.links.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Closes connections to devices no longer in the group (to every device, once this one is
    /// no longer in it).
    fn drop_untrusted(&self) {
        self.close_untrusted(&self.group());
    }

    /// [`Self::drop_untrusted`] with the group already locked. Locks are taken group first, then
    /// links.
    fn close_untrusted(&self, g: &GroupOps) {
        let mut members: HashSet<DeviceId> = g.state().devices().iter().map(|d| d.id).collect();
        if !members.contains(&self.key.id()) {
            members.clear();
        }
        self.links().retain(|id, link| {
            let keep = members.contains(id);
            if !keep {
                link.close();
            }
            keep
        });
    }

    /// Whether `pk` belongs to a member of the group.
    pub fn trusts_key(&self, pk: &[u8; 32]) -> bool {
        self.group().state().trusts_key(pk)
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Keeps a connection made elsewhere (a hole punch), replacing an older one to that device,
    /// and compares groups with it.
    pub fn adopt_link(self: &Arc<Self>, link: QuicLink, path: LinkPath) {
        if !self.keep_link(&link, path) {
            return;
        }
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            let me = self.clone();
            rt.spawn(async move {
                let _ = me.exchange_group(&link).await;
            });
        }
    }

    fn set_path(&self, id: DeviceId, path: LinkPath) {
        self.paths
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, path);
    }

    /// What this device can spend on compression (docs/COMPRESSION.md §2.3): on a metered network
    /// it asks peers to compress harder; on battery it compresses what it sends only lightly.
    /// Applies to open links at once.
    pub fn set_budget(&self, metered: bool, on_battery: bool) {
        self.metered.store(metered, Ordering::Relaxed);
        self.endpoint.set_compression_cap(if on_battery {
            compress::LEVEL_LAN
        } else {
            compress::MAX_LEVEL
        });
        let paths = self.paths.lock().unwrap_or_else(|p| p.into_inner()).clone();
        for (id, link) in self.links().iter() {
            if let Some(path) = paths.get(id) {
                link.set_compression(compression_level(*path, metered));
            }
        }
    }

    fn tune(&self, link: &QuicLink, path: LinkPath) {
        link.set_compression(compression_level(path, self.metered.load(Ordering::Relaxed)));
    }

    /// How the connection to `id` was made, while it is connected.
    pub fn path_of(&self, id: &DeviceId) -> Option<LinkPath> {
        if !self.links().contains_key(id) {
            return None;
        }
        self.paths
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .copied()
    }

    /// Connects to the device at `addr` (it must be in the group), then compares groups with it.
    /// Returns who it is.
    pub async fn connect(&self, addr: SocketAddr) -> Result<DeviceId, String> {
        // Members only, whatever pairing is open: a stranger at `addr` is refused before it sees
        // this device's certificate.
        let link = self
            .endpoint
            .connect_trusting(addr, self.members_only())
            .await
            .map_err(|e| e.to_string())?;
        let path = if is_lan_ip(addr.ip()) {
            LinkPath::Lan
        } else {
            LinkPath::Direct
        };
        if !self.keep_link(&link, path) {
            return Err(NOT_IN_GROUP.into());
        }
        let _ = self.exchange_group(&link).await;
        Ok(link.peer())
    }

    /// Keeps `link`, replacing an older one to that device, if the device is a member; closes it
    /// otherwise. Checked and kept with the group locked, so a removal taking effect meanwhile
    /// cannot leave the link open.
    fn keep_link(&self, link: &QuicLink, path: LinkPath) -> bool {
        let g = self.group();
        if g.state().device(&link.peer()).is_none() {
            link.close();
            return false;
        }
        self.set_path(link.peer(), path);
        self.tune(link, path);
        if let Some(old) = self.links().insert(link.peer(), link.clone()) {
            old.close();
        }
        true
    }

    /// Tries the addresses of a device found on the LAN, each with a short handshake timeout,
    /// until one is a member. Only LAN addresses are dialed. Returns who it is.
    pub async fn connect_found(&self, addrs: &[SocketAddr]) -> Option<DeviceId> {
        for addr in wasl_net::discover::lan_addrs(addrs.iter().copied()) {
            if let Ok(Ok(id)) = tokio::time::timeout(DISCOVERY_DIAL_TIMEOUT, self.connect(addr)).await {
                return Some(id);
            }
        }
        None
    }

    /// Whether `link` stays on the local network: made to a LAN address, and kept as a LAN link.
    fn on_lan(&self, link: &QuicLink) -> bool {
        is_lan_ip(link.remote_addr().ip()) && self.path_of(&link.peer()) == Some(LinkPath::Lan)
    }

    /// Devices connected now.
    pub fn connected(&self) -> Vec<DeviceId> {
        self.links().keys().copied().collect()
    }

    fn start_discovery(self: Arc<Self>) {
        let Ok(port) = self.endpoint.local_addr().map(|a| a.port()) else {
            return;
        };
        let Ok(lan) = Lan::new() else { return };
        if lan.announce(self.token, port).is_err() {
            return;
        }
        let Ok(mut browser) = lan.browse() else { return };
        *self.lan.lock().unwrap_or_else(|p| p.into_inner()) = Some(lan);
        let me = self.token;
        let weak = Arc::downgrade(&self);
        let task = tokio::spawn(async move {
            // Anyone on the network can announce: dials run a few at a time, each bounded.
            let dials = Arc::new(tokio::sync::Semaphore::new(DISCOVERY_DIALS));
            while let Some(found) = browser.next().await {
                if found.token == me {
                    continue;
                }
                let Ok(permit) = dials.clone().acquire_owned().await else {
                    return;
                };
                let Some(devices) = weak.upgrade() else { return };
                tokio::spawn(async move {
                    // A stranger fails the handshake; a member is kept.
                    let _ = devices.connect_found(&found.addrs).await;
                    drop(permit);
                });
            }
        });
        self.tasks().push(task);
    }

    /// Runs one pair with the connected devices: records local changes, pulls from every device
    /// that shares the pair, and tells them when something changed here.
    /// `trigger` is what started the run, as its history row says.
    pub async fn sync_pair(&self, pair: &Pair, trigger: Trigger) -> RunOutcome {
        if let Some(held) = self.held_for_preview(pair) {
            return held;
        }
        {
            let mut running = self.running.lock().unwrap_or_else(|p| p.into_inner());
            if !running.insert(pair.id.clone()) {
                // A run is on: it runs once more when it finishes.
                self.again
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(pair.id.clone());
                return outcome(RunStatus::Skipped, Some("Already syncing with devices".into()));
            }
        }
        // News of the group first, so a device removed elsewhere gets nothing from this run.
        self.exchange_groups().await;
        let mut result;
        loop {
            result = self.sync_once(pair).await;
            let more = self
                .again
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&pair.id);
            if !more {
                break;
            }
        }
        self.running
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&pair.id);
        if pair.route == wasl_core::Route::Devices {
            self.record(pair, &result, trigger);
        }
        result
    }

    /// A new pair waits for its preview with devices too: nothing moves until the person starts it
    /// (docs/INTRODUCTIONS.md §2, "Nothing syncs before that"). Read from the store, not `pair`:
    /// starting from the preview clears the hold after the caller loaded the pair. Starting a
    /// devices-only pair (`approve_once`) lifts its hold here; a Drive-and-devices pair's approval
    /// is the Drive run's to use, so it is only checked (review of #28: an offer accepted on the
    /// phone synced files before its preview).
    fn held_for_preview(&self, pair: &Pair) -> Option<RunOutcome> {
        let s = self.store.lock();
        let mut st = s.get_pair(&pair.id).ok().flatten()?.status;
        if !st.hold_for_preview {
            return None;
        }
        if !st.approve_once {
            return Some(outcome(
                RunStatus::Skipped,
                Some(wasl_engine::HOLD_MESSAGE.into()),
            ));
        }
        if pair.route == wasl_core::Route::Devices {
            // An unshared pair has nothing to preview: the hold stays, and the approval is
            // used up, so the first run after sharing waits for a fresh preview (#94).
            let shared = match s.share_id(&pair.id) {
                Ok(id) => id.is_some(),
                Err(e) => return Some(outcome(RunStatus::Skipped, Some(e.to_string()))),
            };
            if shared {
                st.hold_for_preview = false;
            }
            st.approve_once = false;
            if let Err(e) = s.set_pair_status(&pair.id, &st) {
                return Some(outcome(RunStatus::Skipped, Some(e.to_string())));
            }
            if !shared {
                return Some(outcome(RunStatus::Skipped, Some(NOT_SHARED.into())));
            }
        }
        None
    }

    /// A pair synced only with devices records its runs like a Drive pair (a Drive-and-devices
    /// pair's status comes from its Drive run).
    fn record(&self, pair: &Pair, outcome: &RunOutcome, trigger: Trigger) {
        if outcome.status == RunStatus::Skipped {
            return;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        let s = self.store.lock();
        let Ok(Some(current)) = s.get_pair(&pair.id) else {
            return;
        };
        // A row in the history, like a Drive run's (#148).
        static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let run_id = format!("dev-{now:x}-{:x}", RUNS.fetch_add(1, Ordering::Relaxed));
        let stats = outcome
            .summary
            .as_ref()
            .map_or_else(RunStats::default, |m| RunStats {
                uploaded: m.uploads,
                downloaded: m.downloads,
                deleted_local: m.delete_local,
                conflicts: m.conflicts,
                bytes_down: m.download_bytes,
                ..RunStats::default()
            });
        if s.start_run(&run_id, &pair.id, trigger.as_str(), now).is_ok() {
            let _ = s.finish_run(&run_id, outcome.status, &stats, outcome.message.as_deref(), now);
        }
        let mut st = current.status;
        st.last_run_at = Some(now);
        st.last_status = Some(outcome.status);
        if outcome.status == RunStatus::Success {
            st.last_success_at = Some(now);
            st.last_error = None;
            st.last_error_kind = None;
            st.consecutive_failures = 0;
        } else {
            st.last_error = outcome.message.clone();
            st.consecutive_failures += 1;
        }
        let _ = s.set_pair_status(&pair.id, &st);
    }

    async fn sync_once(&self, pair: &Pair) -> RunOutcome {
        let stored = self.store.lock().share_id(&pair.id);
        let share = match stored {
            Ok(Some(s)) => match ShareId::from_hex(&s) {
                Some(s) => s,
                None => {
                    return outcome(
                        RunStatus::Failed,
                        Some("This pair's device share is damaged.".into()),
                    );
                }
            },
            Ok(None) => {
                return outcome(RunStatus::Skipped, Some(NOT_SHARED.into()));
            }
            Err(e) => return outcome(RunStatus::Failed, Some(e.to_string())),
        };
        let fs = match (self.open_folder)(pair) {
            Ok(fs) => fs,
            Err(e) => return outcome(RunStatus::Failed, Some(e.to_string())),
        };
        let sync = DeviceSync::new(
            self.store.clone(),
            fs,
            pair.clone(),
            share,
            self.key.id(),
            Arc::new(SystemClock),
        );
        let scan = match sync.scan().await {
            Ok(s) => s,
            Err(e) => return failed(e),
        };
        let members: HashSet<DeviceId> = self.members().iter().map(|m| m.id).collect();
        // A pair kept to the LAN is never synced over a link beyond it (security review 2026-10,
        // FR-N11).
        let links: Vec<QuicLink> = self
            .links()
            .values()
            .filter(|l| members.contains(&l.peer()))
            .cloned()
            .collect();
        // (A statement of its own: `on_lan` takes the links lock again.)
        let links: Vec<QuicLink> = links
            .into_iter()
            .filter(|l| pair.reach != wasl_core::Reach::Lan || self.on_lan(l))
            .collect();
        // Only the devices this pair is shared with here, by the rule that decides whom it is
        // served to (docs/INTRODUCTIONS.md §3.2): nothing is taken from, or announced to, a
        // device off its list or still new here (security review 2026-10).
        let links: Vec<QuicLink> = {
            let s = self.store.lock();
            links
                .into_iter()
                .filter(|l| s.share_allows(&pair.id, &l.peer().to_string()).unwrap_or(false))
                .collect()
        };
        let (mut taken, mut deferred, mut failures) = (0, 0, Vec::new());
        let (mut synced_with, mut summary) = (0usize, PlanSummary::default());
        let mut clashes: Vec<String> = Vec::new();
        for link in &links {
            match link.shared().await {
                Ok(shared) if shared.contains(&share) => synced_with += 1,
                Ok(_) => continue,
                Err(_) => {
                    self.links().remove(&link.peer());
                    continue;
                }
            }
            match sync.pull(link).await {
                Ok(r) => {
                    taken += r.taken + r.conflicts;
                    deferred += r.deferred + usize::from(r.truncated);
                    summary.downloads += r.received as u32;
                    summary.download_bytes += r.bytes_fetched;
                    summary.delete_local += r.deleted as u32;
                    summary.conflicts += r.conflicts as u32;
                    clashes.extend(r.clashes);
                }
                Err(e) => failures.push(format!("{}: {e}", link.peer())),
            }
        }
        if scan.changed > 0 || taken > 0 {
            // Tell the others to pull; a pull here is enough to learn our new sequence.
            let seq = self
                .store
                .lock()
                .load_index(&pair.id)
                .ok()
                .flatten()
                .map(|i| i.seq);
            if let Some(seq) = seq {
                for link in &links {
                    let _ = link.notify(share, seq).await;
                }
            }
        }
        if synced_with == 0 && failures.is_empty() {
            // Nothing was synced: no device that has this folder is connected (#148).
            return outcome(RunStatus::Skipped, Some(NO_PEER.into()));
        }
        // Nothing is uploaded: what changed here reaches the others when they pull from it.
        let mut message = format!(
            "{} change(s) here, {} received from {} device(s){}",
            scan.changed,
            taken,
            synced_with,
            if deferred > 0 {
                format!(", {deferred} waiting for the next run")
            } else {
                String::new()
            }
        );
        clashes.sort();
        clashes.dedup();
        if !clashes.is_empty() {
            // ADR-0004 rule 2: named until one of them is renamed.
            let note = format!(
                "These names are the same name, so they don't sync until one is renamed: {}",
                clashes.join(", ")
            );
            message = format!("{message}. {note}");
            summary.warnings.push(note);
        }
        let mut done = if failures.is_empty() && clashes.is_empty() {
            outcome(RunStatus::Success, Some(message))
        } else if failures.is_empty() {
            outcome(RunStatus::Partial, Some(message))
        } else {
            outcome(
                RunStatus::Partial,
                Some(format!("{message}; couldn't sync with {}", failures.join("; "))),
            )
        };
        done.summary = Some(summary);
        done
    }

    /// Stops discovery, the group exchange and the endpoint.
    pub fn close(&self) {
        for t in self.tasks().drain(..) {
            t.abort();
        }
        if let Some(lan) = self.lan.lock().unwrap_or_else(|p| p.into_inner()).take() {
            lan.shutdown();
        }
        self.endpoint.close();
    }
}

/// Who answers pairing rounds while a code is open.
type PairingHost = Arc<Mutex<Option<Arc<dyn PeerService>>>>;

/// What the endpoint serves: sync requests for members only, pairing rounds while a code is open.
struct Gate {
    inner: Arc<DeviceService>,
    group: Arc<Mutex<GroupOps>>,
    pairing: PairingHost,
    /// Members that said their group changed, to ask for the news.
    group_changed: mpsc::UnboundedSender<DeviceId>,
}

impl Gate {
    fn member(&self, peer: DeviceId) -> bool {
        self.group
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .state()
            .device(&peer)
            .is_some()
    }
}

#[async_trait::async_trait]
impl PeerService for Gate {
    async fn shared(&self, peer: DeviceId) -> Vec<ShareId> {
        if !self.member(peer) {
            return Vec::new();
        }
        self.inner.shared(peer).await
    }

    async fn changes_since(
        &self,
        peer: DeviceId,
        pair: ShareId,
        since: u64,
        limit: usize,
    ) -> PeerResult<IndexPage> {
        if !self.member(peer) {
            return Err(PeerError::NotShared);
        }
        self.inner.changes_since(peer, pair, since, limit).await
    }

    async fn chunk_list(
        &self,
        peer: DeviceId,
        pair: ShareId,
        key: &str,
        blake3: Hash,
    ) -> PeerResult<ChunkList> {
        if !self.member(peer) {
            return Err(PeerError::NotShared);
        }
        self.inner.chunk_list(peer, pair, key, blake3).await
    }

    async fn chunk(&self, peer: DeviceId, pair: ShareId, hash: Hash) -> PeerResult<Vec<u8>> {
        if !self.member(peer) {
            return Err(PeerError::NotShared);
        }
        self.inner.chunk(peer, pair, hash).await
    }

    async fn notify(&self, peer: DeviceId, pair: ShareId, seq: u64) {
        if self.member(peer) {
            self.inner.notify(peer, pair, seq).await;
        }
    }

    async fn pair(&self, peer: DeviceId, round: PairRound) -> PeerResult<PairReply> {
        let host = self.pairing.lock().unwrap_or_else(|p| p.into_inner()).clone();
        match host {
            Some(h) => h.pair(peer, round).await,
            None => Err(PeerError::NotShared),
        }
    }

    async fn group_ops(&self, peer: DeviceId, heads: Vec<OpId>) -> PeerResult<Vec<GroupOp>> {
        if !self.member(peer) {
            return Err(PeerError::NotShared);
        }
        let g = self.group.lock().unwrap_or_else(|p| p.into_inner());
        let mut ops = g.missing_for(&heads);
        ops.truncate(wasl_net::group::MAX_OPS);
        Ok(ops)
    }

    async fn group_changed(&self, peer: DeviceId) {
        if self.member(peer) {
            let _ = self.group_changed.send(peer);
        }
    }

    async fn offers(&self, peer: DeviceId) -> Vec<wasl_net::Offer> {
        if !self.member(peer) {
            return Vec::new();
        }
        self.inner.offers(peer).await
    }

    fn for_remote(&self, remote: SocketAddr) -> Option<Arc<dyn PeerService>> {
        Some(Arc::new(Gate {
            inner: Arc::new(self.inner.from_remote(remote)),
            group: self.group.clone(),
            pairing: self.pairing.clone(),
            group_changed: self.group_changed.clone(),
        }))
    }
}

/// Compares groups with members: with the one that says its group changed, at once, and with
/// every connected member every [`GROUP_EVERY`]. Ends with the devices.
async fn exchange_groups(devices: Weak<Devices>, mut changed: mpsc::UnboundedReceiver<DeviceId>) {
    let mut every = tokio::time::interval(GROUP_EVERY);
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            peer = changed.recv() => {
                let Some(peer) = peer else { return };
                let Some(d) = devices.upgrade() else { return };
                let link = d.links().get(&peer).cloned();
                if let Some(link) = link {
                    let _ = d.exchange_group(&link).await;
                }
            }
            _ = every.tick() => {
                let Some(d) = devices.upgrade() else { return };
                d.exchange_groups().await;
            }
        }
    }
}

fn outcome(status: RunStatus, message: Option<String>) -> RunOutcome {
    RunOutcome {
        run_id: String::new(),
        status,
        message,
        summary: None,
        error_kind: None,
        blocker: None,
    }
}

fn failed(e: EngineError) -> RunOutcome {
    outcome(RunStatus::Failed, Some(e.to_string()))
}

/// `group.json`: `{"genesis": <op id hex>, "ops": [...]}`.
fn load_group(path: &Path) -> Result<Option<GroupOps>, String> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let damaged =
        |why: String| format!("The device group file is damaged ({why}). Pair this computer again.");
    let doc: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| damaged(e.to_string()))?;
    let genesis: OpId = serde_json::from_value(doc["genesis"].clone()).map_err(|e| damaged(e.to_string()))?;
    let ops = serde_json::to_vec(&doc["ops"]).map_err(|e| damaged(e.to_string()))?;
    GroupOps::from_json(&ops, genesis)
        .map(Some)
        .map_err(|e| damaged(e.to_string()))
}

fn save_group(path: &Path, g: &GroupOps) -> Result<(), String> {
    let ops: serde_json::Value = serde_json::from_slice(&g.to_json()).map_err(|e| e.to_string())?;
    let doc = serde_json::json!({ "genesis": g.genesis(), "ops": ops });
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&doc).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_lose_control_and_bidi_characters() {
        assert_eq!(Some("Work laptop".into()), clean_name("Work\u{7} laptop"));
        assert_eq!(Some("evil.exe".into()), clean_name("\u{202E}evil.exe\u{2066}"));
        assert_eq!(None, clean_name("\u{200F} \n"));
        assert_eq!(64, clean_name(&"x".repeat(100)).unwrap().chars().count());
    }

    #[test]
    fn offer_names_are_cleaned_and_cut_to_128_bytes() {
        assert_eq!(
            Some("Projects".into()),
            clean_offer_name("\u{202B}Projects\u{1b}")
        );
        assert_eq!(None, clean_offer_name("\u{2067}\t"));
        let cut = clean_offer_name(&"م".repeat(100)).unwrap();
        assert_eq!(128, cut.len());
    }
}
