//! Device sync through the phone's binding: two phones (in process) pair with a code and sync a
//! shared pair; the key goes through the platform's key store.

use std::sync::{Arc, Mutex};
use std::time::Duration;
use wasl_core::conditions::DeviceState;
use wasl_core::{Route, RunStatus};
use wasl_mobile::{
    DeviceEvents, DeviceKeyStore, DevicesInit, EngineInit, Platform, SafBridge, TokenSource, WaslDevices,
    WaslEngine, WaslError,
};
use wasl_store::Pair;

struct Tokens;
impl TokenSource for Tokens {
    fn access_token(&self, _account: String) -> Result<String, WaslError> {
        Ok("tok".into())
    }
    fn invalidate(&self, _account: String) {}
}

struct Device;
impl Platform for Device {
    fn device_state(&self) -> DeviceState {
        DeviceState::unrestricted()
    }
    fn saf(&self) -> Option<Arc<dyn SafBridge>> {
        None
    }
    fn vault_access(&self, _vault_id: String) -> Option<wasl_mobile::VaultAccess> {
        None
    }
}

#[derive(Default)]
struct Keys(Mutex<Option<Vec<u8>>>);
impl DeviceKeyStore for Keys {
    fn load(&self) -> Result<Option<Vec<u8>>, WaslError> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, seed: Vec<u8>) -> Result<(), WaslError> {
        *self.0.lock().unwrap() = Some(seed);
        Ok(())
    }
}

#[derive(Default)]
struct Events(Mutex<Vec<String>>);
impl DeviceEvents for Events {
    fn changed(&self) {}
    fn pair_changed(&self, pair_id: String) {
        self.0.lock().unwrap().push(pair_id);
    }
}

struct Phone {
    devices: Arc<WaslDevices>,
    engine: Arc<WaslEngine>,
    keys: Arc<Keys>,
    root: std::path::PathBuf,
    events: Arc<Events>,
    _dir: tempfile::TempDir,
}

async fn phone(name: &str) -> Phone {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Shared");
    std::fs::create_dir_all(&root).unwrap();
    let engine = WaslEngine::new(
        EngineInit {
            db_path: dir.path().join("wasl.db").to_string_lossy().into_owned(),
            bin_dir: dir.path().join("bin").to_string_lossy().into_owned(),
            staging_dir: dir.path().join("staging").to_string_lossy().into_owned(),
            device_name: name.into(),
            drive_base_url: Some("http://127.0.0.1:9/drive/v3".into()),
            drive_upload_url: Some("http://127.0.0.1:9/upload/drive/v3".into()),
        },
        Arc::new(Tokens),
        Arc::new(Device),
        None,
    )
    .unwrap();
    let mut pair = Pair::new("p", "Shared", root.to_str().unwrap());
    pair.route = Route::Devices;
    engine.create_pair(pair).unwrap();
    // A new pair waits for its preview (ADR-0001); these phones have started theirs.
    engine
        .set_pair_flags("p".into(), None, None, Some(false))
        .unwrap();
    let keys = Arc::new(Keys::default());
    let events = Arc::new(Events::default());
    let devices = WaslDevices::open(
        engine.clone(),
        DevicesInit {
            data_dir: dir.path().to_string_lossy().into_owned(),
            device_name: name.into(),
            port: 0,
            bind_address: Some("127.0.0.1".into()),
            discover: false,
        },
        keys.clone(),
        events.clone(),
    )
    .await
    .unwrap();
    Phone {
        devices,
        engine,
        keys,
        root,
        events,
        _dir: dir,
    }
}

async fn until(p: &Phone, stage: wasl_devices::PairingStage) -> wasl_devices::PairingStatus {
    for _ in 0..200 {
        let s = p.devices.pairing_status();
        if s.stage == stage || s.stage == wasl_devices::PairingStage::Failed {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("never reached {stage:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn two_phones_pair_share_and_sync_through_the_binding() {
    use wasl_devices::PairingStage::*;
    let (a, b) = (phone("Pixel").await, phone("Galaxy").await);
    assert_eq!(
        32,
        a.keys.0.lock().unwrap().as_ref().unwrap().len(),
        "the seed went to the key store"
    );

    // The app calls from its own threads, outside any runtime.
    let inviter = a.devices.clone();
    let code = std::thread::spawn(move || inviter.invite().unwrap().code.unwrap())
        .join()
        .unwrap();
    b.devices.join(code).unwrap();
    assert_eq!(Confirm, until(&a, Confirm).await.stage);
    assert_eq!(Confirm, until(&b, Confirm).await.stage);
    a.devices.confirm_pairing(true).unwrap();
    b.devices.confirm_pairing(true).unwrap();
    assert_eq!(Paired, until(&a, Paired).await.stage);
    assert_eq!(Paired, until(&b, Paired).await.stage);
    assert_eq!(2, a.devices.info().devices.len());

    let share = a.devices.share_pair("p".into(), None).unwrap();
    assert_eq!(
        share,
        b.devices.share_pair("p".into(), Some(share.clone())).unwrap()
    );
    let addr = a.devices.info().listen.unwrap();
    b.devices.connect(addr).await.unwrap();
    let b_addr = b.devices.info().listen.unwrap();
    a.devices.connect(b_addr).await.unwrap();
    assert!(
        a.devices
            .info()
            .devices
            .iter()
            .any(|d| d.connected && d.path.is_some())
    );

    std::fs::write(a.root.join("photo.jpg"), b"pixels").unwrap();
    let r = a.devices.sync_pair("p".into()).await.unwrap();
    assert_eq!(RunStatus::Success, r.status, "{:?}", r.message);
    for _ in 0..100 {
        if !b.events.0.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(vec!["p".to_string()], *b.events.0.lock().unwrap(), "B was told");
    b.devices.sync_pair("p".into()).await.unwrap();
    assert_eq!(
        b"pixels".to_vec(),
        std::fs::read(b.root.join("photo.jpg")).unwrap()
    );

    a.devices.shutdown();
    b.devices.shutdown();
}

/// Two phones paired with a code, connected both ways.
async fn paired() -> (Phone, Phone) {
    use wasl_devices::PairingStage::*;
    let (a, b) = (phone("Pixel").await, phone("Galaxy").await);
    let code = a.devices.invite().unwrap().code.unwrap();
    b.devices.join(code).unwrap();
    assert_eq!(Confirm, until(&a, Confirm).await.stage);
    assert_eq!(Confirm, until(&b, Confirm).await.stage);
    a.devices.confirm_pairing(true).unwrap();
    b.devices.confirm_pairing(true).unwrap();
    assert_eq!(Paired, until(&a, Paired).await.stage);
    assert_eq!(Paired, until(&b, Paired).await.stage);
    b.devices.connect(a.devices.info().listen.unwrap()).await.unwrap();
    a.devices.connect(b.devices.info().listen.unwrap()).await.unwrap();
    (a, b)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_folder_is_offered_and_accepted_through_the_binding() {
    // docs/INTRODUCTIONS.md §3.3, the way the app does it.
    let (a, b) = paired().await;
    assert!(
        a.devices.info().devices.iter().all(|d| d.known) && b.devices.info().devices.iter().all(|d| d.known),
        "paired phones know each other"
    );
    let b_id = b
        .devices
        .info()
        .devices
        .into_iter()
        .find(|d| d.this_device)
        .unwrap()
        .id;
    std::fs::write(a.root.join("photo.jpg"), b"pixels").unwrap();
    let share = a.devices.share_pair("p".into(), None).unwrap();
    a.devices.sync_pair("p".into()).await.unwrap(); // indexes it
    a.devices
        .share_with("p".into(), false, vec![b_id.clone()])
        .unwrap();
    let sharing = a.devices.sharing("p".into()).unwrap().unwrap();
    assert_eq!((false, vec![b_id]), (sharing.all_devices, sharing.devices));

    let mut offers = Vec::new();
    for _ in 0..200 {
        offers = b.devices.offers();
        if !offers.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(1, offers.len(), "B is offered the folder");
    let o = offers.remove(0);
    assert_eq!(
        (share.as_str(), "Shared", "Pixel", 1, 6),
        (
            o.share.as_str(),
            o.name.as_str(),
            o.from_name.as_str(),
            o.files,
            o.bytes
        )
    );

    let here = b.root.parent().unwrap().join("From Pixel");
    std::fs::create_dir_all(&here).unwrap();
    // The phone's way: its pairs always have a Drive folder (no devices-only pairs there yet).
    let mut pair = Pair::new("accepted", &o.name, here.to_str().unwrap());
    pair.route = Route::Drive;
    pair.drive_folder_id = Some("drive-folder".into());
    pair.account_id = Some("me@example.com".into());
    b.engine.create_pair(pair).unwrap();
    assert!(
        b.devices
            .accept_offer("accepted".into(), "00".repeat(16), o.from_device.clone())
            .is_err(),
        "only a share that device offers"
    );
    b.devices
        .accept_offer("accepted".into(), o.share.clone(), o.from_device.clone())
        .unwrap();
    // Shared with the offering phone only, never widened to all devices.
    let sharing = b.devices.sharing("accepted".into()).unwrap().unwrap();
    assert_eq!(
        (o.share.clone(), false, vec![o.from_device.clone()]),
        (sharing.share, sharing.all_devices, sharing.devices)
    );
    assert_eq!(wasl_devices::OfferStatus::Accepted, b.devices.offers()[0].status);
    let route = b.engine.pair("accepted".into()).unwrap().unwrap().route;
    assert_eq!(
        Route::DriveAndDevices,
        route,
        "a Drive pair now syncs with devices too"
    );
    // Held for its first preview, as the app saves every new pair: nothing comes from the other
    // phone before the person starts it (review of #28: the files arrived before the preview).
    b.engine
        .set_pair_flags("accepted".into(), None, None, Some(true))
        .unwrap();
    let held = b.devices.sync_pair("accepted".into()).await.unwrap();
    assert_eq!(RunStatus::Skipped, held.status, "{:?}", held.message);
    assert!(!here.join("photo.jpg").exists(), "nothing before the preview");
    // Started from the preview (the app approves once, then syncs).
    b.engine
        .set_pair_flags("accepted".into(), Some(true), None, None)
        .unwrap();
    let r = b.devices.sync_pair("accepted".into()).await.unwrap();
    assert_eq!(RunStatus::Success, r.status, "{:?}", r.message);
    assert_eq!(b"pixels".to_vec(), std::fs::read(here.join("photo.jpg")).unwrap());
    assert!(
        b.engine
            .pair("accepted".into())
            .unwrap()
            .unwrap()
            .status
            .approve_once,
        "a Drive-and-devices pair's approval is left for its Drive run"
    );

    a.devices.shutdown();
    b.devices.shutdown();
}
