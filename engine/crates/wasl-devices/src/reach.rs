//! Reaching devices over the internet (docs/DEVICE_SYNC.md §7): the rendezvous files in the Drive
//! app-data folder, and the loop that publishes this device's addresses, answers offers and dials
//! members that are not on the LAN.
//!
//! Only when a pair synced with devices has reach "LAN and internet" (FR-P3); otherwise nothing
//! here contacts STUN, Drive or any public address (FR-N11). The devices of a group meet through
//! one Google account's app-data folder, so this needs a signed-in account shared by them.
//!
//! The same folder holds each device's group file ([`crate::board`]), on a policy of its own
//! ([`ReachPolicy::board`]).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use wasl_core::Reach;
use wasl_drive::{DriveClient, UploadRequest};
use wasl_net::holepunch::{self, PunchConfig};
use wasl_net::rendezvous::{self, PeerRecord, Rendezvous, RendezvousError, ReplayGuard};
use wasl_net::stun::{self, StunConfig};

use crate::devices::Devices;

/// The rendezvous files, as flat names in one account's Drive app-data folder.
pub struct DriveRendezvous {
    client: Arc<DriveClient>,
    account: String,
    tmp: PathBuf,
    /// Name → file id, refreshed by every listing.
    ids: Mutex<HashMap<String, String>>,
}

impl DriveRendezvous {
    pub fn new(client: Arc<DriveClient>, account: String, tmp: PathBuf) -> Self {
        Self {
            client,
            account,
            tmp,
            ids: Mutex::new(HashMap::new()),
        }
    }

    async fn refresh(&self) -> Result<HashMap<String, (String, u64)>, RendezvousError> {
        let files = self.client.list_app_data(&self.account).await.map_err(storage)?;
        let found: HashMap<String, (String, u64)> = files
            .into_iter()
            .filter(|f| !f.trashed && is_rendezvous_name(&f.name))
            .map(|f| {
                let size = f.size.as_deref().and_then(|s| s.parse().ok()).unwrap_or(u64::MAX);
                (f.name, (f.id, size))
            })
            .collect();
        *self.ids.lock().unwrap_or_else(|p| p.into_inner()) =
            found.iter().map(|(n, (id, _))| (n.clone(), id.clone())).collect();
        Ok(found)
    }

    fn temp(&self, name: &str) -> PathBuf {
        let safe: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.tmp
            .join(format!("{safe}.{}{}", std::process::id(), wasl_fs::PART_SUFFIX))
    }
}

fn storage(e: impl std::fmt::Display) -> RendezvousError {
    RendezvousError::Storage(e.to_string())
}

/// Only the rendezvous files and group files are ever read or written here; a group file only
/// under the exact name a device writes.
fn is_rendezvous_name(name: &str) -> bool {
    if name.starts_with(rendezvous::GROUP_PREFIX) {
        return rendezvous::parse_group_file_name(name).is_some();
    }
    (name.starts_with(rendezvous::PEERS_PREFIX) || name.starts_with(rendezvous::OFFERS_PREFIX))
        && name.len() < 200
        && name.ends_with(".json")
}

#[async_trait]
impl Rendezvous for DriveRendezvous {
    async fn put(&self, name: &str, bytes: Vec<u8>) -> Result<(), RendezvousError> {
        if !is_rendezvous_name(name) || bytes.len() > rendezvous::max_bytes(name) {
            return Err(RendezvousError::TooLarge);
        }
        let existing = self.refresh().await?.get(name).map(|(id, _)| id.clone());
        let tmp = self.temp(name);
        tokio::fs::create_dir_all(&self.tmp).await.map_err(storage)?;
        tokio::fs::write(&tmp, &bytes).await.map_err(storage)?;
        let mut req = UploadRequest::new(&tmp, name);
        req.mime_type = "application/json";
        match &existing {
            Some(id) => req.existing_file_id = Some(id),
            None => req.parent_id = Some("appDataFolder"),
        }
        let result = self.client.upload(&self.account, req, |_, _| {}).await;
        let _ = tokio::fs::remove_file(&tmp).await;
        result.map(|_| ()).map_err(storage)
    }

    async fn get(&self, name: &str) -> Result<Option<Vec<u8>>, RendezvousError> {
        if !is_rendezvous_name(name) {
            return Ok(None);
        }
        let Some((id, size)) = self.refresh().await?.get(name).cloned() else {
            return Ok(None);
        };
        // Never fetch more than a file of its kind can be.
        let limit = rendezvous::max_bytes(name);
        if size > limit as u64 {
            return Err(RendezvousError::TooLarge);
        }
        tokio::fs::create_dir_all(&self.tmp).await.map_err(storage)?;
        let tmp = self.temp(name);
        let _ = tokio::fs::remove_file(&tmp).await;
        let got = self
            .client
            .download(&self.account, &id, &tmp, Some(size), |_, _| {})
            .await;
        let bytes = match got {
            Ok(()) => tokio::fs::read(&tmp).await.map_err(storage),
            Err(e) => Err(storage(e)),
        };
        let _ = tokio::fs::remove_file(&tmp).await;
        let bytes = bytes?;
        if bytes.len() > limit {
            return Err(RendezvousError::TooLarge);
        }
        Ok(Some(bytes))
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>, RendezvousError> {
        let mut names: Vec<String> = self
            .refresh()
            .await?
            .into_keys()
            .filter(|n| n.starts_with(prefix))
            .collect();
        names.sort();
        Ok(names)
    }

    async fn delete(&self, name: &str) -> Result<(), RendezvousError> {
        let id = self
            .ids
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(name)
            .cloned();
        let id = match id {
            Some(id) => Some(id),
            None => self.refresh().await?.get(name).map(|(id, _)| id.clone()),
        };
        if let Some(id) = id {
            self.client
                .delete_permanently(&self.account, &id)
                .await
                .map_err(storage)?;
        }
        Ok(())
    }
}

/// How often the loop looks for offers, republishes and dials.
pub struct ReachTiming {
    pub answer_every: Duration,
    pub publish_every: Duration,
    pub dial_every: Duration,
}

impl Default for ReachTiming {
    fn default() -> Self {
        Self {
            answer_every: Duration::from_secs(5),
            publish_every: Duration::from_secs(300),
            dial_every: Duration::from_secs(60),
        }
    }
}

/// Runs while the daemon runs. `rendezvous` gives the store to use now (`None`: no account, or
/// no pair wants the internet), checked every round so turning reach on or off takes effect.
pub async fn run(
    devices: Arc<Devices>,
    rendezvous: impl Fn() -> Option<Arc<dyn Rendezvous>> + Send + Sync + 'static,
    stun_servers: Vec<String>,
    timing: ReachTiming,
) {
    let mut guard = ReplayGuard::default();
    let mut limits = holepunch::AnswerLimits::default();
    let mut ours = None;
    let mut published = Instant::now() - timing.publish_every;
    let mut dialed = Instant::now() - timing.dial_every;
    let cfg = PunchConfig::default();
    let reach = Reach::LanAndInternet;
    loop {
        tokio::time::sleep(timing.answer_every).await;
        let Some(rv) = rendezvous() else {
            ours = None;
            continue;
        };
        let trust = {
            let d = devices.clone();
            move |pk: &[u8; 32]| *pk != d.public_key() && d.trusts_key(pk)
        };
        // Offers are looked at only from other members (security review 2026-10).
        let is_member = {
            let d = devices.clone();
            move |id: &wasl_net::DeviceId| *id != d.id() && d.members().iter().any(|m| m.id == *id)
        };
        if ours.is_none() || published.elapsed() >= timing.publish_every {
            let servers: Vec<SocketAddr> = stun::resolve(&stun_servers).await;
            let addrs =
                holepunch::gather(devices.endpoint(), reach, &servers, None, &StunConfig::default()).await;
            let now = rendezvous::unix_now();
            if let Ok(record) = PeerRecord::new(devices.key(), addrs.clone(), now, Duration::from_secs(600))
                && rendezvous::publish(rv.as_ref(), &record).await.is_ok()
            {
                published = Instant::now();
            }
            ours = Some(addrs);
        }
        let Some(addrs) = &ours else { continue };
        if let Ok(answered) = holepunch::answer(
            devices.endpoint(),
            reach,
            rv.as_ref(),
            devices.key(),
            addrs,
            &trust,
            &is_member,
            &mut guard,
            &mut limits,
            &cfg,
        )
        .await
        {
            for (_, link) in answered {
                if let Ok((link, candidate)) = link {
                    devices.adopt_link(link, path_of(candidate.via));
                }
            }
        }
        if dialed.elapsed() >= timing.dial_every {
            dialed = Instant::now();
            let connected = devices.connected();
            for m in devices.members() {
                if m.id == devices.id() || connected.contains(&m.id) {
                    continue;
                }
                if let Ok((link, candidate)) = holepunch::dial(
                    devices.endpoint(),
                    reach,
                    rv.as_ref(),
                    devices.key(),
                    addrs,
                    m.id,
                    &trust,
                    &cfg,
                )
                .await
                {
                    devices.adopt_link(link, path_of(candidate.via));
                }
            }
        }
    }
}

fn path_of(via: holepunch::Via) -> crate::status::LinkPath {
    use crate::status::LinkPath;
    match via {
        holepunch::Via::Lan => LinkPath::Lan,
        holepunch::Via::Ipv6 | holepunch::Via::Mapped => LinkPath::Direct,
        holepunch::Via::Punch => LinkPath::Punched,
    }
}

/// Decides each round whether this device should use the internet, and through which account.
pub struct ReachPolicy {
    store: wasl_store::SharedStore,
    client: Arc<DriveClient>,
    tmp: PathBuf,
    cached: Mutex<Option<(String, Arc<DriveRendezvous>)>>,
}

impl ReachPolicy {
    pub fn new(store: wasl_store::SharedStore, client: Arc<DriveClient>, tmp: PathBuf) -> Self {
        Self {
            store,
            client,
            tmp,
            cached: Mutex::new(None),
        }
    }

    /// The rendezvous to use now: only when an enabled pair shared with devices asks for reach
    /// "LAN and internet", through the first signed-in account without a sign-in problem.
    pub fn current(&self) -> Option<Arc<dyn Rendezvous>> {
        let (wants, account) = {
            let s = self.store.lock();
            let wants = s.list_pairs().ok()?.iter().any(|p| {
                p.enabled
                    && !p.paused
                    && p.route != wasl_core::Route::Drive
                    && p.reach == Reach::LanAndInternet
                    && s.share_id(&p.id).ok().flatten().is_some()
            });
            (wants, Self::account(&s))
        };
        self.store_for(account.filter(|_| wants)?)
    }

    /// Where the group files live (docs/SECURITY.md S2-1): only when an enabled pair synced with
    /// devices already uses Drive (route "Drive and devices") or the internet (reach "LAN and
    /// internet"), through the same account as [`ReachPolicy::current`]. A person who keeps
    /// every device pair to devices on the LAN gets the peer-to-peer exchange only, and nothing
    /// about the group (device names, keys) goes to Google. Paused pairs count: news of a removal
    /// matters most while nothing syncs.
    pub fn board(&self) -> Option<Arc<dyn Rendezvous>> {
        let (wants, account) = {
            let s = self.store.lock();
            let wants = s.list_pairs().ok()?.iter().any(|p| {
                p.enabled
                    && (p.route == wasl_core::Route::DriveAndDevices
                        || (p.route == wasl_core::Route::Devices && p.reach == Reach::LanAndInternet))
            });
            (wants, Self::account(&s))
        };
        self.store_for(account.filter(|_| wants)?)
    }

    /// The first signed-in account without a sign-in problem.
    fn account(s: &wasl_store::Store) -> Option<String> {
        s.accounts()
            .ok()?
            .into_iter()
            .find(|(_, _, problem)| problem.is_none())
            .map(|(id, _, _)| id)
    }

    fn store_for(&self, account: String) -> Option<Arc<dyn Rendezvous>> {
        let mut cached = self.cached.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((a, rv)) = cached.as_ref()
            && *a == account
        {
            return Some(rv.clone());
        }
        let rv = Arc::new(DriveRendezvous::new(
            self.client.clone(),
            account.clone(),
            self.tmp.clone(),
        ));
        *cached = Some((account, rv.clone()));
        Some(rv)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasl_store::{Pair, SharedStore, Store};

    struct NoTokens;

    #[async_trait]
    impl wasl_drive::TokenProvider for NoTokens {
        async fn access_token(&self, _: &str) -> Result<String, wasl_drive::DriveError> {
            Err(wasl_drive::DriveError::Auth {
                message: "none in tests".into(),
                user_action_required: false,
            })
        }
        async fn invalidate(&self, _: &str) {}
    }

    fn policy() -> (ReachPolicy, SharedStore) {
        let store = SharedStore::new(Store::open_in_memory().unwrap());
        let client = Arc::new(DriveClient::new(Arc::new(NoTokens)).unwrap());
        (
            ReachPolicy::new(store.clone(), client, std::env::temp_dir()),
            store,
        )
    }

    #[test]
    fn the_internet_is_used_only_when_a_shared_pair_asks_and_an_account_exists() {
        let (p, store) = policy();
        let mut pair = Pair::new("p", "P", "/x");
        pair.route = wasl_core::Route::Devices;
        pair.reach = Reach::Lan;
        store.lock().create_pair(&pair).unwrap();
        store.lock().set_share_id("p", "00").unwrap();
        assert!(p.current().is_none(), "reach lan never touches the internet");

        pair.reach = Reach::LanAndInternet;
        store.lock().update_pair(&pair).unwrap();
        assert!(p.current().is_none(), "no account to meet through");

        store
            .lock()
            .connection()
            .execute(
                "INSERT INTO accounts (id, email) VALUES ('a', 'a@example.com')",
                [],
            )
            .unwrap();
        assert!(p.current().is_some());
        store.lock().remove_share_id("p").unwrap();
        assert!(p.current().is_none(), "unshared pairs do not count");
    }

    #[test]
    fn group_files_go_to_drive_only_where_a_device_pair_already_uses_it() {
        let (p, store) = policy();
        store
            .lock()
            .connection()
            .execute(
                "INSERT INTO accounts (id, email) VALUES ('a', 'a@example.com')",
                [],
            )
            .unwrap();
        let mut pair = Pair::new("p", "P", "/x");
        pair.route = wasl_core::Route::Devices;
        pair.reach = Reach::Lan;
        store.lock().create_pair(&pair).unwrap();
        assert!(p.board().is_none(), "devices on the LAN only: peer to peer only");
        pair.route = wasl_core::Route::Drive;
        store.lock().update_pair(&pair).unwrap();
        assert!(p.board().is_none(), "no pair synced with devices");
        pair.route = wasl_core::Route::DriveAndDevices;
        store.lock().update_pair(&pair).unwrap();
        assert!(p.board().is_some(), "Drive is in use for this pair anyway");
        pair.route = wasl_core::Route::Devices;
        pair.reach = Reach::LanAndInternet;
        pair.paused = true;
        store.lock().update_pair(&pair).unwrap();
        assert!(
            p.board().is_some(),
            "the internet is in use; paused still hears of removals"
        );
        pair.enabled = false;
        store.lock().update_pair(&pair).unwrap();
        assert!(p.board().is_none());
    }

    #[test]
    fn only_rendezvous_names_are_touched() {
        assert!(is_rendezvous_name("peers/WASL-AAAA.json"));
        assert!(is_rendezvous_name("offers/WASL-A-WASL-B.json"));
        assert!(!is_rendezvous_name("group.json"));
        assert!(!is_rendezvous_name("peers/x.txt"));
        assert!(!is_rendezvous_name(&format!("peers/{}.json", "x".repeat(300))));
        let id = wasl_net::DeviceKey::from_seed(&[1; 32]).id();
        let group = rendezvous::group_file_name(&wasl_net::OpId([2; 32]), &id);
        assert!(is_rendezvous_name(&group));
        assert!(!is_rendezvous_name("group/anything.json"));
        assert_eq!(rendezvous::max_bytes(&group), rendezvous::MAX_GROUP_FILE_BYTES);
    }
}
