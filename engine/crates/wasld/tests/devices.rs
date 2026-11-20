//! Two daemons on this machine syncing a pair over QUIC on the loopback interface: the device key
//! in the keychain, the group, connections, notify, and removal from the group.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use wasl_core::{Route, RunStatus};
use wasl_devices::DeviceConfig;
use wasl_drive::DriveEndpoints;
use wasl_engine::Trigger;
use wasl_net::ShareId;
use wasl_proto::Lang;
use wasl_store::Pair;
use wasld::daemon::{Daemon, DaemonConfig};
use wasld::oauth::OAuthConfig;
use wasld::tokens::{MemorySecrets, Secrets};

const SHARE: ShareId = ShareId([3; 16]);

struct Node {
    daemon: Arc<Daemon>,
    root: PathBuf,
    secrets: Arc<MemorySecrets>,
    data: PathBuf,
    _dir: tempfile::TempDir,
}

fn config(data: PathBuf, secrets: Arc<MemorySecrets>, name: &str) -> DaemonConfig {
    // Drive is never reached: these pairs sync with devices only.
    let uri = "http://127.0.0.1:9";
    DaemonConfig {
        data_dir: data,
        oauth: OAuthConfig {
            client_id: "test".into(),
            client_secret: "test".into(),
            auth_url: format!("{uri}/auth"),
            token_url: format!("{uri}/token"),
            userinfo_url: format!("{uri}/userinfo"),
            scopes: vec![],
        },
        secrets,
        vault_keys: Arc::new(MemorySecrets::default()),
        drive_endpoints: Some(DriveEndpoints {
            base: format!("{uri}/drive/v3"),
            upload: format!("{uri}/upload/drive/v3"),
        }),
        open_browser: false,
        device_name: name.into(),
        sign_in_timeout: Duration::from_secs(1),
        devices: Some(DeviceConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            discover: false,
        }),
        view_backend: None,
        view_refresh: Duration::from_secs(300),
    }
}

fn node(name: &str) -> Node {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Shared");
    std::fs::create_dir_all(&root).unwrap();
    let data = dir.path().join("data");
    let secrets = Arc::new(MemorySecrets::default());
    let daemon = Daemon::open(config(data.clone(), secrets.clone(), name)).unwrap();
    let mut pair = Pair::new("p", "Shared", root.to_str().unwrap());
    pair.route = Route::Devices;
    daemon.store.lock().create_pair(&pair).unwrap();
    daemon.store.lock().set_share_id("p", &SHARE.to_hex()).unwrap();
    Node {
        daemon,
        root,
        secrets,
        data,
        _dir: dir,
    }
}

/// What a restart sees: the node's data folder as a copy. The running daemon keeps its lock on the
/// original (one engine per data folder, wasl#21) and a stopped process isn't possible here, so
/// the "restarted" daemon opens the copy.
///
/// The store is copied with SQLite (`VACUUM INTO`), under the store's own lock, so a task that is
/// still writing (a closing link notes the device, wasl#49) is either in the copy or after it.
/// Copying the files would not do: on Windows SQLite holds byte-range locks on the file while it
/// writes, and a copy that meets one fails with "locked a portion of the file".
fn restart_copy(n: &Node, name: &str) -> PathBuf {
    let to = n.data.with_file_name(name);
    std::fs::create_dir_all(&to).unwrap();
    n.daemon
        .store
        .lock()
        .connection()
        .execute("VACUUM INTO ?1", [to.join("wasl.db").to_str().unwrap()])
        .unwrap();
    fn copy(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let (src, dst) = (e.path(), to.join(e.file_name()));
            let name = e.file_name().to_string_lossy().into_owned();
            if e.file_type().unwrap().is_dir() {
                copy(&src, &dst);
            } else if !name.starts_with("wasl.db") && src.extension().is_none_or(|x| x != "lock") {
                std::fs::copy(&src, &dst).unwrap();
            }
        }
    }
    copy(&n.data, &to);
    to
}

/// A and B in one group, connected both ways.
async fn pair_up() -> (Node, Node) {
    let (a, b) = (node("Laptop"), node("Desktop"));
    let (da, db) = (
        a.daemon.devices.clone().unwrap(),
        b.daemon.devices.clone().unwrap(),
    );
    da.add_member(db.public_key(), "Desktop", 1).unwrap();
    db.join(&da.group_ops(), &da.public_key()).unwrap();
    assert_eq!(da.members(), db.members());
    assert_eq!(db.id(), da.connect(db.local_addr().unwrap()).await.unwrap());
    assert_eq!(da.id(), db.connect(da.local_addr().unwrap()).await.unwrap());
    (a, b)
}

#[tokio::test(flavor = "multi_thread")]
async fn two_daemons_sync_a_pair_and_notify_each_other() {
    let (a, b) = pair_up().await;
    let mut b_notified = b.daemon.take_notifications().unwrap();
    std::fs::write(a.root.join("from-a.txt"), b"hello B").unwrap();
    std::fs::create_dir_all(a.root.join("نصوص")).unwrap();
    std::fs::write(a.root.join("نصوص/ملف.txt"), "مرحبا".as_bytes()).unwrap();

    let ra = a.daemon.run("p", Trigger::Manual).await;
    assert_eq!(RunStatus::Success, ra.status, "{:?}", ra.message);
    // A told B; B runs on that, as the daemon's server would.
    let notified = tokio::time::timeout(Duration::from_secs(5), b_notified.recv())
        .await
        .unwrap();
    assert_eq!(Some("p".to_string()), notified);
    let rb = b.daemon.run("p", Trigger::Instant).await;
    assert_eq!(RunStatus::Success, rb.status, "{:?}", rb.message);
    let status = b.daemon.store.lock().get_pair("p").unwrap().unwrap().status;
    assert_eq!(
        Some(RunStatus::Success),
        status.last_status,
        "device runs are recorded"
    );
    assert!(status.last_success_at.is_some());
    assert_eq!(
        b"hello B".to_vec(),
        std::fs::read(b.root.join("from-a.txt")).unwrap()
    );
    assert_eq!(
        "مرحبا".as_bytes(),
        std::fs::read(b.root.join("نصوص/ملف.txt")).unwrap()
    );

    // And back.
    std::fs::write(b.root.join("from-b.txt"), b"hello A").unwrap();
    b.daemon.run("p", Trigger::Manual).await;
    a.daemon.run("p", Trigger::Instant).await;
    assert_eq!(
        b"hello A".to_vec(),
        std::fs::read(a.root.join("from-b.txt")).unwrap()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_key_and_the_group_survive_a_restart() {
    let (a, _b) = pair_up().await;
    let da = a.daemon.devices.clone().unwrap();
    let (id, members) = (da.id(), da.members());
    assert!(
        a.secrets.get("device-key").is_some(),
        "the seed is in the keychain"
    );
    da.close();
    drop(da);
    let data = restart_copy(&a, "data-restarted");
    let damaged = restart_copy(&a, "data-damaged");
    let again = Daemon::open(config(data.clone(), a.secrets.clone(), "Laptop")).unwrap();
    let d2 = again.devices.clone().unwrap();
    assert_eq!(id, d2.id());
    assert_eq!(members, d2.members());

    // A damaged group file is refused, not silently replaced.
    d2.close();
    let path = damaged.join("group.json");
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("Desktop", "Desk top");
    std::fs::write(&path, text).unwrap();
    assert!(Daemon::open(config(damaged.clone(), a.secrets.clone(), "Laptop")).is_err());
}

/// A task still writes to the store when a restart copy is taken, as the note a closing link
/// leaves does (wasl#49). The copy must not trip over the writer's lock, and must open as a store.
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_copy_is_taken_while_the_store_is_being_written() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let n = node("Laptop");
    let store = n.daemon.store.clone();
    // Stops the writer however the test ends, so a failed assertion leaves no thread spinning.
    struct Stop(Arc<AtomicBool>);
    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }
    let stop = Stop(Arc::new(AtomicBool::new(false)));
    let writer = {
        let stop = stop.0.clone();
        std::thread::spawn(move || {
            let mut at = 0;
            while !stop.load(Ordering::Relaxed) {
                at += 1;
                store.lock().note_device_seen("writer", at).unwrap();
                std::thread::yield_now();
            }
        })
    };
    for i in 0..20 {
        let copy = restart_copy(&n, &format!("data-copy-{i}"));
        let opened = wasl_store::Store::open(copy.join("wasl.db")).unwrap();
        assert!(opened.get_pair("p").unwrap().is_some(), "copy {i} has the pair");
    }
    drop(stop);
    writer.join().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_removed_device_is_cut_off() {
    let (a, b) = pair_up().await;
    let (da, db) = (
        a.daemon.devices.clone().unwrap(),
        b.daemon.devices.clone().unwrap(),
    );
    std::fs::write(a.root.join("before.txt"), b"x").unwrap();
    a.daemon.run("p", Trigger::Manual).await;

    da.remove_member(db.id()).unwrap();
    assert!(da.connected().is_empty());
    std::fs::write(a.root.join("after.txt"), b"secret").unwrap();
    a.daemon.run("p", Trigger::Manual).await;

    // B still holds an old link to A: A refuses it now, and B gets nothing new.
    let rb = b.daemon.run("p", Trigger::Manual).await;
    assert!(!b.root.join("after.txt").exists(), "{:?}", rb.message);
    assert!(db.connect(da.local_addr().unwrap()).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pair_not_shared_yet_is_skipped_with_a_reason() {
    let a = node("Solo");
    a.daemon
        .store
        .lock()
        .connection()
        .execute("DELETE FROM pair_shares", [])
        .unwrap();
    let r = a.daemon.run("p", Trigger::Manual).await;
    assert_eq!(RunStatus::Skipped, r.status);
    assert!(r.message.unwrap().contains("isn't shared"));
}

/// #101 review: a shared device-only pair moved to another folder loses its share. It stays a
/// device-only pair, held; its preview lifts the hold, and it waits to be shared again.
#[tokio::test(flavor = "multi_thread")]
async fn a_shared_device_only_pair_moved_to_another_folder_stays_held_until_shared_again() {
    let a = node("Solo");
    let other = a.root.parent().unwrap().join("Other");
    std::fs::create_dir_all(&other).unwrap();
    let mut moved = a.daemon.store.lock().get_pair("p").unwrap().unwrap();
    moved.local_root = other.to_str().unwrap().into();
    assert!(a.daemon.store.lock().update_pair(&moved).unwrap());
    let got = a.daemon.store.lock().get_pair("p").unwrap().unwrap();
    assert_eq!(Route::Devices, got.route);
    assert!(got.status.hold_for_preview);

    let manual = a.daemon.run("p", Trigger::Manual).await;
    assert_eq!(Some(wasl_engine::HOLD_MESSAGE.to_string()), manual.message);
    let preview = a.daemon.run("p", Trigger::Preview).await;
    assert_eq!(RunStatus::Skipped, preview.status);
    assert!(preview.message.unwrap().contains("isn't shared"));
    let got = a.daemon.store.lock().get_pair("p").unwrap().unwrap();
    assert!(
        got.status.hold_for_preview,
        "an unshared pair's preview keeps the hold"
    );

    // Shared again, the next run still waits for a preview instead of syncing.
    a.daemon.store.lock().set_share_id("p", &SHARE.to_hex()).unwrap();
    let next = a.daemon.run("p", Trigger::Manual).await;
    assert_eq!(Some(wasl_engine::HOLD_MESSAGE.to_string()), next.message);
    let got = a.daemon.store.lock().get_pair("p").unwrap().unwrap();
    assert!(got.status.hold_for_preview);
    // Now a preview lifts it.
    a.daemon.run("p", Trigger::Preview).await;
    let got = a.daemon.store.lock().get_pair("p").unwrap().unwrap();
    assert!(
        !got.status.hold_for_preview,
        "the shared pair's preview lifted the hold"
    );
}

/// #94: the same rule without the daemon. The FFI starts a preview with `engine.sync(Preview)`,
/// which writes `approve_once`, then calls `Devices::sync_pair`: an unshared held pair keeps its
/// hold there too, and the approval is used up.
#[tokio::test(flavor = "multi_thread")]
async fn an_unshared_held_pair_keeps_its_hold_when_devices_sync_it_directly() {
    let a = node("Solo");
    let other = a.root.parent().unwrap().join("Other");
    std::fs::create_dir_all(&other).unwrap();
    let mut moved = a.daemon.store.lock().get_pair("p").unwrap().unwrap();
    moved.local_root = other.to_str().unwrap().into();
    assert!(a.daemon.store.lock().update_pair(&moved).unwrap());
    let devices = a.daemon.devices.clone().unwrap();

    // What `engine.sync(Preview)` writes before the devices run.
    a.daemon.store.lock().set_approve_once("p").unwrap();
    let pair = a.daemon.store.lock().get_pair("p").unwrap().unwrap();
    assert!(pair.status.hold_for_preview && pair.status.approve_once);
    let r = devices.sync_pair(&pair, Trigger::Manual).await;
    assert_eq!(RunStatus::Skipped, r.status);
    assert_eq!(Some(wasl_devices::NOT_SHARED.to_string()), r.message);
    let got = a.daemon.store.lock().get_pair("p").unwrap().unwrap();
    assert!(got.status.hold_for_preview, "an unshared pair keeps its hold");
    assert!(!got.status.approve_once, "the approval is used up");

    // Shared again, a run without a fresh approval still waits for the preview.
    a.daemon.store.lock().set_share_id("p", &SHARE.to_hex()).unwrap();
    let r = devices.sync_pair(&got, Trigger::Manual).await;
    assert_eq!(Some(wasl_engine::HOLD_MESSAGE.to_string()), r.message);
    assert!(
        a.daemon
            .store
            .lock()
            .get_pair("p")
            .unwrap()
            .unwrap()
            .status
            .hold_for_preview
    );

    // A fresh approval on the shared pair lifts the hold.
    a.daemon.store.lock().set_approve_once("p").unwrap();
    devices.sync_pair(&got, Trigger::Manual).await;
    let got = a.daemon.store.lock().get_pair("p").unwrap().unwrap();
    assert!(
        !got.status.hold_for_preview,
        "the shared pair's approval lifted the hold"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn devices_are_listed_shared_and_removed_through_the_protocol() {
    use wasl_ipc::{DevicesInfo, Request};
    let (a, b) = pair_up().await;
    let info: DevicesInfo = serde_json::from_value(a.daemon.handle(Request::Devices).await.unwrap()).unwrap();
    assert!(info.enabled);
    assert_eq!(2, info.devices.len());
    let me = info.devices.iter().find(|d| d.this_device).unwrap();
    assert_eq!("Laptop", me.name);
    let other = info.devices.iter().find(|d| !d.this_device).unwrap();
    assert!(other.connected);

    // This computer cannot remove itself here; an unknown ID is refused.
    assert!(
        a.daemon
            .handle(Request::RemoveDevice { id: me.id.clone() })
            .await
            .is_err()
    );
    assert!(
        a.daemon
            .handle(Request::RemoveDevice {
                id: "WASL-NOPE".into()
            })
            .await
            .is_err()
    );

    // Sharing a Drive-only pair adds devices to its route; a code joins the same share.
    let mut drive_only = Pair::new("q", "Docs", a.root.to_str().unwrap());
    drive_only.drive_folder_id = Some("F".into());
    a.daemon.store.lock().create_pair(&drive_only).unwrap();
    let code: String = serde_json::from_value(
        a.daemon
            .handle(Request::SharePair {
                id: "q".into(),
                share: None,
            })
            .await
            .unwrap(),
    )
    .unwrap();
    let q = a.daemon.store.lock().get_pair("q").unwrap().unwrap();
    assert_eq!(Route::DriveAndDevices, q.route);
    // Sharing again keeps the code; it can be read back.
    let again: String = serde_json::from_value(
        a.daemon
            .handle(Request::SharePair {
                id: "q".into(),
                share: None,
            })
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(code, again);
    let read: Option<String> = serde_json::from_value(
        a.daemon
            .handle(Request::ShareCode { id: "q".into() })
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(Some(code.clone()), read);
    assert!(
        a.daemon
            .handle(Request::SharePair {
                id: "p".into(),
                share: Some(code.clone())
            })
            .await
            .is_err(),
        "one code names one pair here"
    );
    let mut on_b = Pair::new("q2", "Docs", b.root.to_str().unwrap());
    on_b.route = Route::Devices;
    b.daemon.store.lock().create_pair(&on_b).unwrap();
    b.daemon
        .handle(Request::SharePair {
            id: "q2".into(),
            share: Some(code.clone()),
        })
        .await
        .unwrap();
    assert_eq!(Some(code), b.daemon.store.lock().share_id("q2").unwrap());
    a.daemon
        .handle(Request::UnsharePair { id: "q".into() })
        .await
        .unwrap();
    assert_eq!(
        Route::Drive,
        a.daemon.store.lock().get_pair("q").unwrap().unwrap().route
    );

    a.daemon
        .handle(Request::RemoveDevice { id: other.id.clone() })
        .await
        .unwrap();
    let info: DevicesInfo = serde_json::from_value(a.daemon.handle(Request::Devices).await.unwrap()).unwrap();
    assert_eq!(1, info.devices.len());
}

/// Waits until `node`'s pairing reaches `stage` (or fails).
async fn until(node: &Node, stage: wasl_ipc::PairingStage) -> wasl_ipc::PairingStatus {
    let p = node.daemon.pairing.clone().unwrap();
    for _ in 0..200 {
        let s = p.status();
        if s.stage == stage || s.stage == wasl_ipc::PairingStage::Failed {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("pairing never reached {stage:?}: {:?}", p.status());
}

#[tokio::test(flavor = "multi_thread")]
async fn two_computers_pair_with_a_code_and_then_sync() {
    use wasl_ipc::PairingStage::*;
    let (a, b) = (node("Laptop"), node("Desktop"));
    let (pa, pb) = (
        a.daemon.pairing.clone().unwrap(),
        b.daemon.pairing.clone().unwrap(),
    );
    let invite = pa.invite().unwrap();
    assert_eq!(Waiting, invite.stage);
    pb.join(invite.code.as_deref().unwrap()).unwrap();

    let (sa, sb) = (until(&a, Confirm).await, until(&b, Confirm).await);
    assert_eq!(Confirm, sa.stage, "{sa:?}");
    assert_eq!(Confirm, sb.stage, "{sb:?}");
    assert_eq!(4, sa.words.len());
    assert_eq!(sa.words, sb.words, "both screens show the same words");
    // Beside the words, each screen can show which device is asking.
    let (ida, idb) = (
        a.daemon.devices.as_ref().unwrap().id().to_string(),
        b.daemon.devices.as_ref().unwrap().id().to_string(),
    );
    assert_eq!(Some(idb.clone()), sa.peer_id);
    assert_eq!(Some("Desktop".into()), sa.peer_name, "known from the first round");
    assert_eq!(Some(ida), sb.peer_id);

    pa.confirm(true).unwrap();
    pb.confirm(true).unwrap();
    let (sa, sb) = (until(&a, Paired).await, until(&b, Paired).await);
    assert_eq!(Paired, sa.stage, "{sa:?}");
    assert_eq!(Paired, sb.stage, "{sb:?}");
    assert_eq!(Some("Desktop".into()), sa.peer_name);
    assert_eq!(Some("Laptop".into()), sb.peer_name);

    let (da, db) = (
        a.daemon.devices.clone().unwrap(),
        b.daemon.devices.clone().unwrap(),
    );
    assert_eq!(da.members(), db.members());
    assert_eq!(2, da.members().len());

    // Now they sync like any two members.
    da.connect(db.local_addr().unwrap()).await.unwrap();
    std::fs::write(a.root.join("after-pairing.txt"), b"hi").unwrap();
    a.daemon.run("p", Trigger::Manual).await;
    b.daemon.run("p", Trigger::Manual).await;
    assert_eq!(
        b"hi".to_vec(),
        std::fs::read(b.root.join("after-pairing.txt")).unwrap()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn different_words_add_no_one_and_a_stranger_reads_nothing_while_a_code_is_open() {
    use wasl_ipc::PairingStage::*;
    let (a, b) = (node("Laptop"), node("Stranger"));
    std::fs::write(a.root.join("private.txt"), b"secret").unwrap();
    a.daemon.run("p", Trigger::Manual).await;
    let pa = a.daemon.pairing.clone().unwrap();
    pa.invite().unwrap();

    // While the code is open the stranger can connect, but asks for the pair in vain.
    let db = b.daemon.devices.clone().unwrap();
    let da = a.daemon.devices.clone().unwrap();
    let stranger = wasl_net::Endpoint::bind(
        "127.0.0.1:0".parse().unwrap(),
        db.key(),
        Arc::new(|_: &[u8; 32]| true),
    )
    .unwrap();
    let link = stranger
        .connect(da.local_addr().unwrap(), Some(da.id()))
        .await
        .unwrap();
    use wasl_net::PeerLink as _;
    assert!(link.shared().await.unwrap().is_empty());
    assert!(link.changes_since(SHARE, 0, 100).await.is_err());

    // The words do not match for the person on A: nothing is added, and the code is closed.
    let pb = b.daemon.pairing.clone().unwrap();
    let code = pa.status().code.unwrap();
    pb.join(&code).unwrap();
    until(&a, Confirm).await;
    pa.confirm(false).unwrap();
    assert_eq!(Failed, until(&b, Paired).await.stage);
    assert_eq!(1, da.members().len());
    assert!(
        link.shared().await.is_err(),
        "the stranger's connection is refused once the code closes"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_open_code_never_makes_a_stranger_a_sync_source_and_junk_does_not_use_it_up() {
    use wasl_ipc::PairingStage::*;
    use wasl_net::PeerLink as _;
    let (a, b) = (node("Laptop"), node("Desktop"));
    let (da, db) = (
        a.daemon.devices.clone().unwrap(),
        b.daemon.devices.clone().unwrap(),
    );
    let pa = a.daemon.pairing.clone().unwrap();
    let code = pa.invite().unwrap().code.unwrap();

    // A's endpoint lets any key in while the code is open, but A never keeps a stranger as a
    // device to sync with, in either direction.
    assert!(da.connect(db.local_addr().unwrap()).await.is_err());
    assert!(da.connected().is_empty());

    // A round that opens without a Hello is refused and leaves the code usable.
    let stranger = wasl_net::Endpoint::bind(
        "127.0.0.1:0".parse().unwrap(),
        &wasl_net::DeviceKey::from_seed(&[77; 32]),
        Arc::new(|_: &[u8; 32]| true),
    )
    .unwrap();
    let link = stranger
        .connect(da.local_addr().unwrap(), Some(da.id()))
        .await
        .unwrap();
    assert!(
        link.pair(wasl_net::PairRound {
            msgs: Vec::new(),
            name: None
        })
        .await
        .is_err()
    );
    let pb = b.daemon.pairing.clone().unwrap();
    pb.join(&code).unwrap();
    assert_eq!(Confirm, until(&b, Confirm).await.stage);

    // The joiner says no: the inviter's code closes at once.
    pb.confirm(false).unwrap();
    assert_eq!(Failed, until(&a, Paired).await.stage);
    assert!(link.shared().await.is_err(), "the code is closed for everyone");
}

/// Waits (up to 10 s) until `cond` holds.
async fn eventually(what: &str, cond: impl Fn() -> bool) {
    for _ in 0..200 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("never: {what}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_removal_and_an_addition_reach_the_other_members() {
    let (a, b, c) = (node("Laptop"), node("Desktop"), node("Tablet"));
    let (da, db, dc) = (
        a.daemon.devices.clone().unwrap(),
        b.daemon.devices.clone().unwrap(),
        c.daemon.devices.clone().unwrap(),
    );
    da.add_member(db.public_key(), "Desktop", 1).unwrap();
    da.add_member(dc.public_key(), "Tablet", 2).unwrap();
    db.join(&da.group_ops(), &da.public_key()).unwrap();
    dc.join(&da.group_ops(), &da.public_key()).unwrap();
    for (x, y) in [
        (&da, &db),
        (&db, &da),
        (&db, &dc),
        (&dc, &db),
        (&da, &dc),
        (&dc, &da),
    ] {
        assert_eq!(y.id(), x.connect(y.local_addr().unwrap()).await.unwrap());
    }

    // C's person keeps B (A added both, so B is new to C), so C's runs ask B.
    dc.keep_device(db.id()).unwrap();

    // A removes C. A tells B, B asks A for the news and cuts C off at once.
    da.remove_member(dc.id()).unwrap();
    eventually("B learns of the removal", || !db.trusts_key(&dc.public_key())).await;
    // B closes its link to C in the same step that drops trust.
    assert!(!db.connected().contains(&dc.id()));
    std::fs::write(b.root.join("after.txt"), b"not for C").unwrap();
    b.daemon.run("p", Trigger::Manual).await;
    // C's own link to B is still open on C's side. B's endpoint reads trust live, so C's next
    // request over it closes it and C gets nothing; a new connection is refused at the handshake.
    assert!(dc.connected().contains(&db.id()));
    c.daemon.run("p", Trigger::Manual).await;
    assert!(!c.root.join("after.txt").exists());
    assert!(
        !dc.connected().contains(&db.id()),
        "B closed C's link at its request"
    );
    assert!(dc.connect(db.local_addr().unwrap()).await.is_err());

    // D is paired through A; B, connected to A, trusts it too, and they sync directly.
    let d = node("Phone");
    let dd = d.daemon.devices.clone().unwrap();
    da.add_member(dd.public_key(), "Phone", 3).unwrap();
    dd.join(&da.group_ops(), &da.public_key()).unwrap();
    eventually("B learns of the addition", || db.trusts_key(&dd.public_key())).await;
    // A added both, so B and D are new to each other (docs/INTRODUCTIONS.md §3.1): each side's
    // person keeps the other before they sync.
    db.keep_device(dd.id()).unwrap();
    dd.keep_device(db.id()).unwrap();
    assert_eq!(db.id(), dd.connect(db.local_addr().unwrap()).await.unwrap());
    d.daemon.run("p", Trigger::Manual).await;
    assert_eq!(
        b"not for C".to_vec(),
        std::fs::read(d.root.join("after.txt")).unwrap()
    );
    assert_eq!(da.members(), db.members());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pair_kept_to_the_lan_never_syncs_over_a_link_beyond_it() {
    let (a, b) = pair_up().await;
    let (da, db) = (
        a.daemon.devices.clone().unwrap(),
        b.daemon.devices.clone().unwrap(),
    );
    // A's link to B is one made over the internet (as a hole punch would be).
    let link = da
        .endpoint()
        .connect(db.local_addr().unwrap(), Some(db.id()))
        .await
        .unwrap();
    da.adopt_link(link, wasl_devices::LinkPath::Punched);
    std::fs::write(b.root.join("far.txt"), b"x").unwrap();
    b.daemon.run("p", Trigger::Manual).await;
    // Reach "LAN" (the default): nothing comes over it.
    a.daemon.run("p", Trigger::Manual).await;
    assert!(!a.root.join("far.txt").exists());
    // Reach "LAN and internet": it does.
    let mut pair = a.daemon.store.lock().get_pair("p").unwrap().unwrap();
    pair.reach = wasl_core::Reach::LanAndInternet;
    a.daemon.store.lock().update_pair(&pair).unwrap();
    a.daemon.run("p", Trigger::Manual).await;
    assert!(a.root.join("far.txt").exists());
}

/// Answers nothing.
struct Nothing;

#[async_trait::async_trait]
impl wasl_net::PeerService for Nothing {
    async fn shared(&self, _: wasl_net::DeviceId) -> Vec<ShareId> {
        Vec::new()
    }
    async fn changes_since(
        &self,
        _: wasl_net::DeviceId,
        _: ShareId,
        _: u64,
        _: usize,
    ) -> wasl_net::PeerResult<wasl_net::IndexPage> {
        Err(wasl_net::PeerError::NotShared)
    }
    async fn chunk_list(
        &self,
        _: wasl_net::DeviceId,
        _: ShareId,
        _: &str,
        _: wasl_chunk::Hash,
    ) -> wasl_net::PeerResult<wasl_chunk::ChunkList> {
        Err(wasl_net::PeerError::NotShared)
    }
    async fn chunk(
        &self,
        _: wasl_net::DeviceId,
        _: ShareId,
        _: wasl_chunk::Hash,
    ) -> wasl_net::PeerResult<Vec<u8>> {
        Err(wasl_net::PeerError::NotShared)
    }
    async fn notify(&self, _: wasl_net::DeviceId, _: ShareId, _: u64) {}
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stranger_found_on_the_lan_never_sees_this_device_even_while_a_code_is_open() {
    let a = node("Laptop");
    let da = a.daemon.devices.clone().unwrap();
    a.daemon.pairing.clone().unwrap().invite().unwrap();
    let asked: Arc<std::sync::Mutex<Vec<[u8; 32]>>> = Arc::default();
    let seen = asked.clone();
    let stranger = wasl_net::Endpoint::bind(
        "127.0.0.1:0".parse().unwrap(),
        &wasl_net::DeviceKey::from_seed(&[66; 32]),
        Arc::new(move |pk: &[u8; 32]| {
            seen.lock().unwrap().push(*pk);
            true
        }),
    )
    .unwrap();
    stranger.serve(Arc::new(Nothing));
    assert_eq!(None, da.connect_found(&[stranger.local_addr().unwrap()]).await);
    assert!(
        asked.lock().unwrap().is_empty(),
        "A refused the stranger before showing its own certificate"
    );
    assert!(da.connected().is_empty());
}

/// A, B and C in one group (A added both); B and C connected to each other, A to nobody.
async fn three_with_a_apart() -> (Node, Node, Node) {
    let (a, b, c) = (node("Laptop"), node("Desktop"), node("Tablet"));
    let (da, db, dc) = (
        a.daemon.devices.clone().unwrap(),
        b.daemon.devices.clone().unwrap(),
        c.daemon.devices.clone().unwrap(),
    );
    da.add_member(db.public_key(), "Desktop", 1).unwrap();
    da.add_member(dc.public_key(), "Tablet", 2).unwrap();
    db.join(&da.group_ops(), &da.public_key()).unwrap();
    dc.join(&da.group_ops(), &da.public_key()).unwrap();
    assert_eq!(dc.id(), db.connect(dc.local_addr().unwrap()).await.unwrap());
    assert_eq!(db.id(), dc.connect(db.local_addr().unwrap()).await.unwrap());
    (a, b, c)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_removal_reaches_a_member_that_never_meets_the_remover() {
    // S2-1: A removes C while B is away (B and A are never online together). B later reads the
    // group files in the app-data folder and cuts C off.
    let (a, b, c) = three_with_a_apart().await;
    let (da, db, dc) = (
        a.daemon.devices.clone().unwrap(),
        b.daemon.devices.clone().unwrap(),
        c.daemon.devices.clone().unwrap(),
    );
    let app_data = wasl_net::rendezvous::MemoryRendezvous::new();
    da.remove_member(dc.id()).unwrap();
    wasl_devices::board::publish(&da, &app_data).await.unwrap();
    // C publishes too (it does not know yet); nobody reads a removed device's file.
    wasl_devices::board::publish(&dc, &app_data).await.unwrap();
    assert!(db.trusts_key(&dc.public_key()), "B has not heard yet");

    assert!(wasl_devices::board::read(&db, &app_data).await.unwrap());
    assert!(!db.trusts_key(&dc.public_key()));
    assert!(!db.connected().contains(&dc.id()), "C's link closed at once");
    std::fs::write(b.root.join("after.txt"), b"not for C").unwrap();
    b.daemon.run("p", Trigger::Manual).await;
    c.daemon.run("p", Trigger::Manual).await;
    assert!(!c.root.join("after.txt").exists());
    assert!(dc.connect(db.local_addr().unwrap()).await.is_err());
    assert_eq!(da.members(), db.members());
    // Read again: nothing new.
    assert!(!wasl_devices::board::read(&db, &app_data).await.unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_group_files_are_written_on_change_and_read_at_start() {
    let (a, b, c) = three_with_a_apart().await;
    let (da, db, dc) = (
        a.daemon.devices.clone().unwrap(),
        b.daemon.devices.clone().unwrap(),
        c.daemon.devices.clone().unwrap(),
    );
    let app_data: Arc<dyn wasl_net::rendezvous::Rendezvous> =
        Arc::new(wasl_net::rendezvous::MemoryRendezvous::new());
    let fast = wasl_devices::BoardTiming {
        check_every: Duration::from_millis(20),
        read_every: Duration::from_millis(50),
    };
    let store = app_data.clone();
    let on_a = tokio::spawn(wasl_devices::board::run(
        da.clone(),
        move || Some(store.clone()),
        fast,
    ));
    da.remove_member(dc.id()).unwrap();
    let name = wasl_net::rendezvous::group_file_name(&da.genesis(), &da.id());
    let mut written = false;
    for _ in 0..200 {
        if let Some(bytes) = app_data.get(&name).await.unwrap() {
            let ops = wasl_net::rendezvous::parse_group_file(&bytes).unwrap();
            if ops.len() == da.group_ops().len() {
                written = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(written, "A wrote its removal of C");
    // B starts its loop later and reads at once.
    let store = app_data.clone();
    let on_b = tokio::spawn(wasl_devices::board::run(
        db.clone(),
        move || Some(store.clone()),
        fast,
    ));
    eventually("B learns of the removal from the app-data folder", || {
        !db.trusts_key(&dc.public_key())
    })
    .await;
    // No store (devices on the LAN only): the loop leaves the folder alone.
    use wasl_net::rendezvous::Rendezvous as _;
    let quiet = wasl_net::rendezvous::MemoryRendezvous::new();
    let on_c = tokio::spawn(wasl_devices::board::run(dc.clone(), || None, fast));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(quiet.list("").await.unwrap().is_empty());
    for t in [on_a, on_b, on_c] {
        t.abort();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn one_signers_operations_are_taken_in_at_a_bounded_rate() {
    // S2-2: however many operations a member sends at once, each signer's are taken in at most
    // OPS_PER_SIGNER_PER_MINUTE a minute; the rest come again at a later exchange.
    let (a, b) = (node("Laptop"), node("Desktop"));
    let (da, db) = (
        a.daemon.devices.clone().unwrap(),
        b.daemon.devices.clone().unwrap(),
    );
    da.add_member(db.public_key(), "Desktop", 1).unwrap();
    db.join(&da.group_ops(), &da.public_key()).unwrap();
    let extra = wasl_net::group::OPS_PER_SIGNER_PER_MINUTE + 6;
    for n in 0..extra {
        let k = wasl_net::DeviceKey::from_seed(&[n as u8 + 10; 32]);
        da.add_member(k.public(), "Extra", 2).unwrap();
    }
    assert!(db.receive_ops(&da.group_ops()).unwrap());
    assert_eq!(db.members().len(), 2 + wasl_net::group::OPS_PER_SIGNER_PER_MINUTE);
    // Taken in already: sending them again costs nothing and changes nothing.
    assert!(!db.receive_ops(&da.group_ops()).unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn forged_operations_in_a_members_name_do_not_use_up_its_share() {
    // Anyone who can write a group file (a removed device that still has the Google account) can
    // claim any signer. Operations whose signature does not check against that member's key cost
    // the member nothing, so its real removal still gets in at once.
    let (a, b, c) = (node("Laptop"), node("Desktop"), node("Tablet"));
    let (da, db, dc) = (
        a.daemon.devices.clone().unwrap(),
        b.daemon.devices.clone().unwrap(),
        c.daemon.devices.clone().unwrap(),
    );
    da.add_member(db.public_key(), "Desktop", 1).unwrap();
    da.add_member(dc.public_key(), "Tablet", 2).unwrap();
    db.join(&da.group_ops(), &da.public_key()).unwrap();
    assert!(db.trusts_key(&dc.public_key()));
    let heads = da.group_heads();
    let mut flood = Vec::new();
    for n in 0..(2 * wasl_net::group::OPS_PER_SIGNER_PER_MINUTE) {
        let mut seed = [9u8; 32];
        seed[0] = n as u8;
        let k = wasl_net::DeviceKey::from_seed(&seed);
        flood.push(wasl_net::GroupOp {
            change: wasl_net::group::Change::Add(wasl_net::GroupDevice {
                id: k.id(),
                name: "Junk".into(),
                pk: k.public(),
                added_at: 3,
                added_by: da.id(),
            }),
            parents: heads.clone(),
            signed_by: da.id(),
            signature: [7; 64],
        });
    }
    da.remove_member(dc.id()).unwrap();
    flood.extend(da.group_ops());
    db.receive_ops(&flood).unwrap();
    assert!(!db.trusts_key(&dc.public_key()), "A's removal of C got in");
    assert!(db.members().iter().all(|m| m.name != "Junk"));
}

#[tokio::test(flavor = "multi_thread")]
async fn this_devices_group_file_is_put_back_when_someone_else_changes_it() {
    // A removed device that still has the Google account can overwrite or delete the remover's
    // file in the app-data folder, to keep the news of its removal from devices that rely on
    // Drive. The remover reads its own file back every read round and writes it again.
    let (a, _b, c) = three_with_a_apart().await;
    let (da, dc) = (
        a.daemon.devices.clone().unwrap(),
        c.daemon.devices.clone().unwrap(),
    );
    let app_data: Arc<dyn wasl_net::rendezvous::Rendezvous> =
        Arc::new(wasl_net::rendezvous::MemoryRendezvous::new());
    let fast = wasl_devices::BoardTiming {
        check_every: Duration::from_millis(20),
        read_every: Duration::from_millis(100),
    };
    let store = app_data.clone();
    let on_a = tokio::spawn(wasl_devices::board::run(
        da.clone(),
        move || Some(store.clone()),
        fast,
    ));
    da.remove_member(dc.id()).unwrap();
    let name = wasl_net::rendezvous::group_file_name(&da.genesis(), &da.id());
    let whole = || {
        let (app_data, name, da) = (app_data.clone(), name.clone(), da.clone());
        async move {
            app_data
                .get(&name)
                .await
                .unwrap()
                .and_then(|b| wasl_net::rendezvous::parse_group_file(&b).ok())
                .is_some_and(|ops| ops.len() == da.group_ops().len())
        }
    };
    let mut ok = false;
    for _ in 0..200 {
        if whole().await {
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(ok, "A wrote its removal of C");
    // C overwrites A's file with an older group, then deletes it: each time it comes back.
    let old = wasl_net::rendezvous::group_file(
        &da.group_ops()[..da.group_ops().len() - 1],
        wasl_net::rendezvous::MAX_GROUP_FILE_BYTES,
    );
    app_data.put(&name, old).await.unwrap();
    let mut back = false;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        if whole().await {
            back = true;
            break;
        }
    }
    assert!(back, "an overwritten file is written again");
    app_data.delete(&name).await.unwrap();
    let mut back = false;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        if whole().await {
            back = true;
            break;
        }
    }
    assert!(back, "a deleted file is written again");
    on_a.abort();
}

// ------------------------------------------------------------- introductions (2026-10)

async fn devices_info(node: &Node) -> wasl_ipc::DevicesInfo {
    serde_json::from_value(node.daemon.handle(wasl_ipc::Request::Devices).await.unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_added_by_another_is_new_and_gets_nothing_until_kept() {
    use wasl_ipc::Request;
    // A adds B, then C; both join through A. On B, A is known (joined through it) and C is new.
    let (a, b, c) = three_with_a_apart().await;
    let (da, db, dc) = (
        a.daemon.devices.clone().unwrap(),
        b.daemon.devices.clone().unwrap(),
        c.daemon.devices.clone().unwrap(),
    );
    let info = devices_info(&b).await;
    let on_b = |id: wasl_net::DeviceId| info.devices.iter().find(|d| d.id == id.to_string()).unwrap();
    assert!(on_b(db.id()).known && on_b(da.id()).known);
    let new = on_b(dc.id());
    assert!(!new.known);
    assert_eq!(Some(da.id().to_string()), new.added_by, "who brought it in");
    // On A, which added both, nobody is new.
    assert!(devices_info(&a).await.devices.iter().all(|d| d.known));

    // B's pair is shared with all its devices: C still gets nothing from B.
    std::fs::write(b.root.join("on-b.txt"), b"for my devices").unwrap();
    b.daemon.run("p", Trigger::Manual).await;
    c.daemon.run("p", Trigger::Manual).await;
    assert!(!c.root.join("on-b.txt").exists());

    // Kept on B: C syncs.
    b.daemon
        .handle(Request::KeepDevice {
            id: dc.id().to_string(),
        })
        .await
        .unwrap();
    assert!(devices_info(&b).await.devices.iter().all(|d| d.known));
    // B is new on C too, and each side decides for itself: C takes nothing from B until kept.
    c.daemon.run("p", Trigger::Manual).await;
    assert!(!c.root.join("on-b.txt").exists());
    dc.keep_device(db.id()).unwrap();
    c.daemon.run("p", Trigger::Manual).await;
    assert_eq!(
        b"for my devices".to_vec(),
        std::fs::read(c.root.join("on-b.txt")).unwrap()
    );
    // Keeping a stranger is refused.
    assert!(
        b.daemon
            .handle(Request::KeepDevice {
                id: wasl_net::DeviceKey::from_seed(&[99; 32]).id().to_string()
            })
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pair_shared_with_chosen_devices_is_served_only_to_them() {
    use wasl_ipc::{PairSharing, Request};
    // A added B and C itself, so both are known on A; A shares its pair with B only.
    let (a, b) = pair_up().await;
    let c = node("Tablet");
    let (da, dc) = (
        a.daemon.devices.clone().unwrap(),
        c.daemon.devices.clone().unwrap(),
    );
    da.add_member(dc.public_key(), "Tablet", 2).unwrap();
    dc.join(&da.group_ops(), &da.public_key()).unwrap();
    assert_eq!(da.id(), dc.connect(da.local_addr().unwrap()).await.unwrap());
    let b_id = b.daemon.devices.as_ref().unwrap().id().to_string();
    a.daemon
        .handle(Request::ShareWith {
            id: "p".into(),
            all: false,
            devices: vec![b_id.clone()],
        })
        .await
        .unwrap();
    let sharing: Option<PairSharing> = serde_json::from_value(
        a.daemon
            .handle(Request::Sharing { id: "p".into() })
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        Some(PairSharing {
            share: SHARE.to_hex(),
            all_devices: false,
            devices: vec![b_id.clone()]
        }),
        sharing
    );

    std::fs::write(a.root.join("plan.txt"), b"for B").unwrap();
    a.daemon.run("p", Trigger::Manual).await;
    c.daemon.run("p", Trigger::Manual).await;
    assert!(!c.root.join("plan.txt").exists(), "C is not on the list");
    b.daemon.run("p", Trigger::Manual).await;
    assert_eq!(b"for B".to_vec(), std::fs::read(b.root.join("plan.txt")).unwrap());

    // Off the list: B is stopped at its next request.
    a.daemon
        .handle(Request::ShareWith {
            id: "p".into(),
            all: false,
            devices: vec![],
        })
        .await
        .unwrap();
    std::fs::write(a.root.join("later.txt"), b"not for B").unwrap();
    a.daemon.run("p", Trigger::Manual).await;
    b.daemon.run("p", Trigger::Manual).await;
    assert!(!b.root.join("later.txt").exists());

    // Only members, never this device, and only a shared pair.
    for (id, devices) in [
        ("p", vec![da.id().to_string()]),
        (
            "p",
            vec![wasl_net::DeviceKey::from_seed(&[99; 32]).id().to_string()],
        ),
        ("p", vec!["junk".to_string()]),
        ("nope", vec![b_id.clone()]),
    ] {
        assert!(
            a.daemon
                .handle(Request::ShareWith {
                    id: id.into(),
                    all: false,
                    devices
                })
                .await
                .is_err()
        );
    }
}

async fn offers(node: &Node) -> Vec<wasl_ipc::OfferInfo> {
    serde_json::from_value(node.daemon.handle(wasl_ipc::Request::Offers).await.unwrap()).unwrap()
}

/// Waits (up to 10 s) until `node` holds offers matching `cond`.
async fn offers_until(
    node: &Node,
    what: &str,
    cond: impl Fn(&[wasl_ipc::OfferInfo]) -> bool,
) -> Vec<wasl_ipc::OfferInfo> {
    for _ in 0..200 {
        let o = offers(node).await;
        if cond(&o) {
            return o;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("never: {what}: {:?}", offers(node).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_folder_is_offered_declined_withdrawn_and_accepted() {
    use wasl_ipc::{OfferStatus, Request};
    let (a, b) = pair_up().await;
    let (da, db) = (
        a.daemon.devices.clone().unwrap(),
        b.daemon.devices.clone().unwrap(),
    );
    // A has a second folder, Projects, synced with devices.
    let projects = a.root.parent().unwrap().join("Projects");
    std::fs::create_dir_all(&projects).unwrap();
    std::fs::write(projects.join("plan.md"), b"twelve bytes").unwrap();
    std::fs::write(projects.join("notes.txt"), b"abc").unwrap();
    let mut pair = Pair::new("projects", "Projects", projects.to_str().unwrap());
    pair.route = Route::Devices;
    a.daemon.store.lock().create_pair(&pair).unwrap();
    a.daemon
        .handle(Request::SharePair {
            id: "projects".into(),
            share: None,
        })
        .await
        .unwrap();
    a.daemon.run("projects", Trigger::Manual).await; // indexes it
    assert!(
        offers(&b).await.is_empty(),
        "shared with all devices: offered to nobody"
    );

    // Offering it to B: B hears at once (A says its group changed) and sees name and counts.
    let share_with = |devices: Vec<String>| Request::ShareWith {
        id: "projects".into(),
        all: false,
        devices,
    };
    a.daemon
        .handle(share_with(vec![db.id().to_string()]))
        .await
        .unwrap();
    let got = offers_until(&b, "B sees the offer", |o| !o.is_empty()).await;
    assert_eq!(1, got.len());
    let o = &got[0];
    assert_eq!(
        ("Projects", "Laptop", 2, 15, OfferStatus::New),
        (o.name.as_str(), o.from_name.as_str(), o.files, o.bytes, o.status)
    );
    assert_eq!(da.id().to_string(), o.from_device);
    let share = o.share.clone();

    // Declined: hidden, kept as declined while A still offers it.
    b.daemon
        .handle(Request::DeclineOffer {
            share: share.clone(),
            from: da.id().to_string(),
        })
        .await
        .unwrap();
    assert_eq!(OfferStatus::Declined, offers(&b).await[0].status);
    db.exchange_groups().await;
    assert_eq!(OfferStatus::Declined, offers(&b).await[0].status);

    // Withdrawn: gone.
    a.daemon.handle(share_with(vec![])).await.unwrap();
    offers_until(&b, "the offer is withdrawn", <[_]>::is_empty).await;

    // Offered again and accepted: a pair on B, held for its first preview.
    a.daemon
        .handle(share_with(vec![db.id().to_string()]))
        .await
        .unwrap();
    offers_until(&b, "B sees it again", |o| {
        o.len() == 1 && o[0].status == OfferStatus::New
    })
    .await;
    let here = b.root.parent().unwrap().join("Projects on B");
    let accept = |name: Option<String>| Request::AcceptOffer {
        share: share.clone(),
        from: da.id().to_string(),
        local_root: here.to_string_lossy().into_owned(),
        name,
        lang: None,
    };
    // The pair rules are wasl-pairs' here too (wasl#60): a folder on Wasl's own data or on
    // another pair's files is refused with the same typed words as a new pair, in
    // the caller's language, and nothing is saved.
    let before = b.daemon.store.lock().list_pairs().unwrap().len();
    for (key, root) in [
        ("pair.own_data", b.data.join("bin")),
        ("pair.overlaps", b.root.join("inside")),
    ] {
        let refused = b
            .daemon
            .handle(Request::AcceptOffer {
                share: share.clone(),
                from: da.id().to_string(),
                local_root: root.to_string_lossy().into_owned(),
                name: None,
                lang: Some(Lang::Ar),
            })
            .await
            .unwrap_err();
        let words = *refused.pair_rule.expect("typed words");
        assert_eq!(key, words.key);
        assert!(
            words.text.chars().any(|c| ('\u{0600}'..='\u{06FF}').contains(&c)),
            "{}",
            words.text
        );
        assert!(!root.exists(), "a refused folder isn't made: {}", root.display());
    }
    assert_eq!(before, b.daemon.store.lock().list_pairs().unwrap().len());
    let made: Pair = serde_json::from_value(b.daemon.handle(accept(None)).await.unwrap()).unwrap();
    assert_eq!(("Projects", Route::Devices), (made.name.as_str(), made.route));
    assert!(made.status.hold_for_preview);
    assert_eq!(
        Some(share.clone()),
        b.daemon.store.lock().share_id(&made.id).unwrap()
    );
    assert_eq!(OfferStatus::Accepted, offers(&b).await[0].status);
    // Shared with A only: accepting never widens who the folder reaches.
    let sharing: Option<wasl_ipc::PairSharing> = serde_json::from_value(
        b.daemon
            .handle(Request::Sharing { id: made.id.clone() })
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        Some((false, vec![da.id().to_string()])),
        sharing.map(|s| (s.all_devices, s.devices))
    );
    assert!(
        b.daemon.handle(accept(Some("Again".into()))).await.is_err(),
        "a share a pair here already syncs is refused"
    );
    // B now lists A on the pair it accepted, but A's own folder is no offer to A (review of #28).
    db.exchange_groups().await;
    da.exchange_groups().await;
    assert!(
        offers(&a).await.is_empty(),
        "A isn't offered its own folder back: {:?}",
        offers(&a).await
    );

    // Nothing moves before the preview; starting from it syncs.
    let held = b.daemon.run(&made.id, Trigger::Manual).await;
    assert_eq!(RunStatus::Skipped, held.status);
    assert!(!here.join("plan.md").exists());
    let r = b.daemon.run(&made.id, Trigger::Preview).await;
    assert_eq!(RunStatus::Success, r.status, "{:?}", r.message);
    assert_eq!(
        b"twelve bytes".to_vec(),
        std::fs::read(here.join("plan.md")).unwrap()
    );

    // An accepted offer stays when the sender stops offering it.
    a.daemon.handle(share_with(vec![])).await.unwrap();
    db.exchange_groups().await;
    assert_eq!(OfferStatus::Accepted, offers(&b).await[0].status);
}

#[tokio::test(flavor = "multi_thread")]
async fn pairing_makes_each_side_know_the_other() {
    use wasl_ipc::PairingStage::*;
    let (a, b) = (node("Laptop"), node("Desktop"));
    let (pa, pb) = (
        a.daemon.pairing.clone().unwrap(),
        b.daemon.pairing.clone().unwrap(),
    );
    pb.join(pa.invite().unwrap().code.as_deref().unwrap()).unwrap();
    assert_eq!(Confirm, until(&a, Confirm).await.stage);
    assert_eq!(Confirm, until(&b, Confirm).await.stage);
    pa.confirm(true).unwrap();
    pb.confirm(true).unwrap();
    assert_eq!(Paired, until(&a, Paired).await.stage);
    assert_eq!(Paired, until(&b, Paired).await.stage);
    let (ia, ib) = (devices_info(&a).await, devices_info(&b).await);
    assert_eq!(2, ia.devices.len());
    assert!(
        ia.devices.iter().all(|d| d.known),
        "the inviter knows its new device"
    );
    assert!(ib.devices.iter().all(|d| d.known), "the joiner knows the inviter");
    let joiner = ia.devices.iter().find(|d| !d.this_device).unwrap();
    assert_eq!(
        Some(a.daemon.devices.as_ref().unwrap().id().to_string()),
        joiner.added_by
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn members_present_when_notes_are_first_made_are_known() {
    // An upgrade: B synced before introductions, so it has no note on anyone yet.
    let (_a, b, c) = three_with_a_apart().await;
    let secrets = b.secrets.clone();
    let c_id = c.daemon.devices.as_ref().unwrap().id().to_string();
    assert_eq!(Some(false), b.daemon.store.lock().device_known(&c_id).unwrap());
    b.daemon
        .store
        .lock()
        .connection()
        .execute("DELETE FROM device_notes", [])
        .unwrap();
    b.daemon.devices.as_ref().unwrap().close();
    let data = restart_copy(&b, "data-restarted");
    let again = Daemon::open(config(data, secrets, "Desktop")).unwrap();
    let info: wasl_ipc::DevicesInfo =
        serde_json::from_value(again.handle(wasl_ipc::Request::Devices).await.unwrap()).unwrap();
    assert_eq!(3, info.devices.len());
    assert!(info.devices.iter().all(|d| d.known), "{info:?}");
    assert_eq!(Some(true), again.store.lock().device_known(&c_id).unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pair_never_pulls_from_a_device_it_does_not_share_with() {
    // C joined through A, so C is new on B. C keeps B and offers its files to it: B still takes
    // nothing from C until B's person keeps C (security review 2026-10).
    let (_a, b, c) = three_with_a_apart().await;
    let (db, dc) = (
        b.daemon.devices.clone().unwrap(),
        c.daemon.devices.clone().unwrap(),
    );
    dc.keep_device(db.id()).unwrap();
    std::fs::write(c.root.join("from-c.txt"), b"unasked").unwrap();
    c.daemon.run("p", Trigger::Manual).await;
    b.daemon.run("p", Trigger::Manual).await;
    assert!(
        !b.root.join("from-c.txt").exists(),
        "B pulled from a device new here"
    );
    db.keep_device(dc.id()).unwrap();
    b.daemon.run("p", Trigger::Manual).await;
    assert_eq!(
        b"unasked".to_vec(),
        std::fs::read(b.root.join("from-c.txt")).unwrap()
    );
}

/// ADR-0002 Read format (#48): a changeset from another listed device carries that device's name;
/// one from a device the list doesn't know, or no longer lists, carries `null`.
#[tokio::test(flavor = "multi_thread")]
async fn the_op_log_names_a_listed_device_and_leaves_an_unknown_or_removed_one_unnamed() {
    use wasl_ipc::Request;
    const UNKNOWN: &str = "WASL-0123-4567-89AB-CDEF-GHJK-MNPQ-RSTV";
    let (a, phone) = (node("Laptop"), node("Phone"));
    let da = a.daemon.devices.clone().unwrap();
    let dp = phone.daemon.devices.clone().unwrap();
    da.add_member(dp.public_key(), "Phone", 1).unwrap();
    let phone_id = dp.id().to_string();
    for (device, id) in [
        (phone_id.as_str(), "1790000000000-1"),
        (UNKNOWN, "1790000000000-2"),
    ] {
        a.daemon
            .store
            .lock()
            .oplog_open(&wasl_store::NewChangeset {
                key: wasl_store::OplogKey::new(device, id),
                actor: "agent:claude-code".into(),
                label: None,
                scope: wasl_store::folder_uri("p", ""),
                opened_at: 1_790_000_000_000,
            })
            .unwrap();
    }
    let names = || async {
        let list = Request::OplogList {
            actor: None,
            status: None,
            since: None,
            pair: None,
            limit: 10,
        };
        let page: wasl_engine::OplogPage =
            serde_json::from_value(a.daemon.handle(list).await.unwrap()).unwrap();
        let mut names: Vec<(String, Option<String>)> = page
            .changesets
            .into_iter()
            .map(|c| (c.origin_device, c.origin_device_name))
            .collect();
        names.sort();
        names
    };
    let mut want = vec![
        (phone_id.clone(), Some("Phone".to_owned())),
        (UNKNOWN.to_owned(), None),
    ];
    want.sort();
    assert_eq!(want, names().await);
    let show = Request::OplogShow {
        origin_device: Some(phone_id.clone()),
        id: "1790000000000-1".into(),
    };
    let shown: wasl_engine::OplogPage = serde_json::from_value(a.daemon.handle(show).await.unwrap()).unwrap();
    assert_eq!(Some("Phone"), shown.changesets[0].origin_device_name.as_deref());

    da.remove_member(dp.id()).unwrap();
    assert!(
        names().await.iter().all(|(_, name)| name.is_none()),
        "a removed device is no longer named"
    );
}

/// #148 item 7: a run with devices only said "0 up, 0 down, 0 deleted" whatever it moved, and
/// `wasl history` had no row for it.
#[tokio::test(flavor = "multi_thread")]
async fn a_device_run_counts_what_it_moved_and_is_in_the_history() {
    let (a, b) = pair_up().await;
    std::fs::write(a.root.join("one.txt"), b"1").unwrap();
    std::fs::write(a.root.join("two.txt"), b"2").unwrap();
    a.daemon.run("p", Trigger::Manual).await;
    let r = b.daemon.run("p", Trigger::Manual).await;
    assert_eq!(RunStatus::Success, r.status, "{:?}", r.message);
    let s = r.summary.expect("a summary");
    assert_eq!((2, 0, 0), (s.downloads, s.delete_local, s.uploads), "{s:?}");
    assert!(
        r.message
            .as_deref()
            .unwrap_or_default()
            .contains("from 1 device(s)"),
        "{:?}",
        r.message
    );
    std::fs::remove_file(a.root.join("one.txt")).unwrap();
    a.daemon.run("p", Trigger::Manual).await;
    let r = b.daemon.run("p", Trigger::Manual).await;
    assert_eq!(1, r.summary.expect("a summary").delete_local);
    let runs = b.daemon.store.lock().runs(Some("p"), 10).unwrap();
    assert_eq!(2, runs.len(), "{runs:?}");
    assert_eq!(1, runs[0].stats.deleted_local, "{runs:?}");
    assert_eq!("manual", runs[0].trigger);
    assert!(runs.iter().all(|r| r.status == RunStatus::Success));
}

/// #148 items 2 and 9: with no device to sync with (none yet, or none that has accepted the
/// folder), the run said "Synced with devices".
#[tokio::test(flavor = "multi_thread")]
async fn a_run_with_no_device_to_sync_with_is_not_a_sync() {
    let a = node("Solo");
    std::fs::write(a.root.join("here.txt"), b"x").unwrap();
    let r = a.daemon.run("p", Trigger::Manual).await;
    assert_eq!(RunStatus::Skipped, r.status, "{:?}", r.message);
    assert_eq!(Some(wasl_devices::NO_PEER), r.message.as_deref());
    let status = a.daemon.store.lock().get_pair("p").unwrap().unwrap().status;
    assert_eq!(None, status.last_success_at);
}

/// wasl#20: one lock per pair covers Drive and device runs. A device run started while another
/// run holds the pair waits its turn, and a Drive-side start while a run holds the pair reports
/// "already syncing"; once the holder ends, both go through. Other pairs are not held.
#[tokio::test(flavor = "multi_thread")]
async fn a_device_run_and_a_drive_run_of_one_pair_never_overlap() {
    let a = node("Laptop");
    let engine = &a.daemon.engine;
    let devices = a.daemon.devices.clone().unwrap();
    let pair = a.daemon.store.lock().get_pair("p").unwrap().unwrap();

    // A Drive-side run holds the pair.
    let held = engine.pair_locks().try_acquire("p").unwrap();

    // A device run waits behind it (nothing about it finishes while the lock is held).
    let (d2, p2) = (devices.clone(), pair.clone());
    let run = tokio::spawn(async move { d2.sync_pair(&p2, Trigger::Manual).await });
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!run.is_finished(), "the device run waits for the pair's lock");

    // A Drive-side start meanwhile reports instead of running alongside.
    let r = engine.sync("p", Trigger::Manual).await;
    assert_eq!(RunStatus::Skipped, r.status);
    assert_eq!(Some(wasl_engine::ALREADY_RUNNING.to_string()), r.message);

    // Another pair is free.
    assert!(engine.pair_locks().try_acquire("other").is_some());

    drop(held);
    let r = tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(Some("Already syncing".to_string()), r.message);
    // The device run released the lock when it ended.
    assert!(!engine.pair_locks().is_held("p"));
    let r = engine.sync("p", Trigger::Manual).await;
    assert_ne!(Some(wasl_engine::ALREADY_RUNNING.to_string()), r.message, "{r:?}");
}

/// wasl#20: a device run dropped while it waits for the pair's lock frees its place, so the next
/// run isn't turned away as "already syncing with devices" for good.
#[tokio::test(flavor = "multi_thread")]
async fn a_device_run_dropped_while_it_waits_leaves_nothing_behind() {
    let a = node("Laptop");
    let devices = a.daemon.devices.clone().unwrap();
    let pair = a.daemon.store.lock().get_pair("p").unwrap().unwrap();
    let held = a.daemon.engine.pair_locks().try_acquire("p").unwrap();
    let (d2, p2) = (devices.clone(), pair.clone());
    let waiting = tokio::spawn(async move { d2.sync_pair(&p2, Trigger::Manual).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    waiting.abort();
    let _ = waiting.await;
    drop(held);
    let r = tokio::time::timeout(Duration::from_secs(10), devices.sync_pair(&pair, Trigger::Manual))
        .await
        .unwrap();
    assert_ne!(Some("Already syncing with devices".to_string()), r.message);
}

// ---- where each device stands, and the folder that waits for one (issue #32)

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// Sets when `device` was last seen on `node`, whatever it was (a test's way to age a device).
fn seen_ago(node: &Node, device: &str, ago_ms: i64) {
    let s = node.daemon.store.lock();
    s.connection()
        .execute(
            &format!(
                "INSERT INTO device_seen (device_id, last_seen_at) VALUES ('{device}', {at})
                 ON CONFLICT(device_id) DO UPDATE SET last_seen_at = {at}",
                at = now_ms() - ago_ms
            ),
            [],
        )
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_folder_shared_with_an_offline_device_says_which_device_it_waits_for() {
    use wasl_ipc::Request;
    use wasl_proto::{DevicePresence, FolderState, Lang, WaitReason};
    const MIN: i64 = 60_000;
    const DAY: i64 = 24 * 60 * MIN;
    let (a, _b) = pair_up().await;
    let r = a.daemon.run("p", Trigger::Manual).await;
    assert_eq!(RunStatus::Success, r.status, "{:?}", r.message);

    // B is connected: online, and the folder is up to date.
    let st = a.daemon.status(Lang::En).unwrap();
    let b = st.devices.iter().find(|d| !d.this_device).unwrap();
    assert_eq!(
        (DevicePresence::Online, "device.online", "Online"),
        (b.state, b.words.key.as_str(), b.words.text.as_str())
    );
    let folder = st.pairs[0].status.clone().unwrap();
    assert_eq!(FolderState::Synced, folder.state);
    assert_eq!(1, folder.peers.len());
    assert_eq!(DevicePresence::Online, folder.peers[0].state);

    // A third device joined the group and was never connected.
    let tablet = node("Tablet");
    let dt = tablet.daemon.devices.clone().unwrap();
    let da = a.daemon.devices.clone().unwrap();
    da.add_member(dt.public_key(), "Tablet", 2).unwrap();
    let tablet_id = dt.id().to_string();
    let status_of = |lang| a.daemon.status(lang).unwrap();
    let tablet_in =
        |st: &wasl_ipc::DaemonStatus| st.devices.iter().find(|d| d.id == tablet_id).cloned().unwrap();

    // Never seen: can't reach, and the folder says so by name.
    let st = status_of(Lang::En);
    assert_eq!(DevicePresence::CantReach, tablet_in(&st).state);
    let folder = st.pairs[0].status.clone().unwrap();
    assert_eq!(
        (
            FolderState::Waiting,
            Some(WaitReason::Device),
            "folder.cant_reach_device"
        ),
        (folder.state, folder.wait, folder.words.key.as_str())
    );
    assert_eq!(
        "Wasl can't reach Tablet, so Shared isn't synced there.",
        folder.words.text
    );

    // Offline for 5 minutes: last seen, with the time, and the folder waits for it.
    seen_ago(&a, &tablet_id, 5 * MIN + 1_000);
    let st = status_of(Lang::En);
    let t = tablet_in(&st);
    assert_eq!(
        (DevicePresence::LastSeen, "device.last_seen_minutes", Some(5)),
        (t.state, t.words.key.as_str(), t.words.values.count)
    );
    assert_eq!("Last seen 5 minutes ago", t.words.text);
    assert!(t.last_seen_at.is_some());
    let folder = st.pairs[0].status.clone().unwrap();
    assert_eq!("folder.waiting_device", folder.words.key);
    assert_eq!("Shared will sync when Tablet is back online.", folder.words.text);
    assert_eq!(Some("Tablet"), folder.words.values.device.as_deref());
    assert_eq!(
        wasl_proto::OverallState::Waiting,
        st.overall_status.unwrap().state
    );

    // The same state, key and values in Arabic.
    let ar = status_of(Lang::Ar);
    let t_ar = tablet_in(&ar);
    assert_eq!(
        (t.state, &t.words.key, &t.words.values),
        (t_ar.state, &t_ar.words.key, &t_ar.words.values)
    );
    assert_eq!("آخر ظهور قبل 5 دقائق", t_ar.words.text);
    let folder_ar = ar.pairs[0].status.clone().unwrap();
    assert_eq!("folder.waiting_device", folder_ar.words.key);
    assert_eq!(
        "ستتم مزامنة Shared عندما يعود Tablet إلى الاتصال.",
        folder_ar.words.text
    );

    // Offline for 3 days.
    seen_ago(&a, &tablet_id, 3 * DAY + 1_000);
    let t = tablet_in(&status_of(Lang::En));
    assert_eq!(
        ("device.last_seen_days", Some(3)),
        (t.words.key.as_str(), t.words.values.count)
    );
    assert_eq!("Last seen 3 days ago", t.words.text);
    assert_eq!("آخر ظهور قبل 3 أيام", tablet_in(&status_of(Lang::Ar)).words.text);

    // Long gone: can't reach again.
    seen_ago(&a, &tablet_id, 31 * DAY);
    assert_eq!(DevicePresence::CantReach, tablet_in(&status_of(Lang::En)).state);

    // A folder that also syncs with Drive stays up to date: its Drive copy is current.
    seen_ago(&a, &tablet_id, 5 * MIN);
    a.daemon
        .store
        .lock()
        .set_pair_route("p", Route::DriveAndDevices)
        .unwrap();
    let folder = status_of(Lang::En).pairs[0].status.clone().unwrap();
    assert_eq!(FolderState::Synced, folder.state);
    assert!(
        folder
            .peers
            .iter()
            .any(|p| p.name == "Tablet" && p.state == DevicePresence::LastSeen)
    );

    // `--json` and the protocol carry the same: stable keys and the time as a value.
    let v = serde_json::to_value(status_of(Lang::En)).unwrap();
    let keys: Vec<&str> = v["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["words"]["key"].as_str().unwrap())
        .collect();
    assert!(keys.contains(&"device.last_seen_minutes"), "{keys:?}");
    let _ = Request::Status { lang: None };
}

/// A device that goes away is shown as offline at once, not at the next check.
#[tokio::test(flavor = "multi_thread")]
async fn a_device_that_goes_away_is_last_seen_just_now() {
    use wasl_proto::{DevicePresence, FolderState, Lang};
    let (a, b) = pair_up().await;
    let r = a.daemon.run("p", Trigger::Manual).await;
    assert_eq!(RunStatus::Success, r.status, "{:?}", r.message);
    b.daemon.devices.as_ref().unwrap().close();
    eventually("A sees B go", || {
        a.daemon
            .status(Lang::En)
            .map(|st| {
                st.devices
                    .iter()
                    .any(|d| !d.this_device && d.state == DevicePresence::LastSeen)
            })
            .unwrap_or(false)
    })
    .await;
    let st = a.daemon.status(Lang::En).unwrap();
    let b_status = st.devices.iter().find(|d| !d.this_device).unwrap();
    assert_eq!("Last seen a moment ago", b_status.words.text);
    let folder = st.pairs[0].status.clone().unwrap();
    assert_eq!(FolderState::Waiting, folder.state);
    assert_eq!("Shared will sync when Desktop is back online.", folder.words.text);
}

/// Rows in every table the store keeps per device that name `id` (wasl#13).
fn rows_about(n: &Node, id: &str) -> Vec<(&'static str, i64)> {
    let store = n.daemon.store.lock();
    let c = store.connection();
    [
        ("device_notes", "device_id"),
        ("device_seen", "device_id"),
        ("pair_share_devices", "device_id"),
        ("received_offers", "from_device"),
        ("peer_progress", "peer"),
    ]
    .into_iter()
    .map(|(table, col)| {
        let n: i64 = c
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE {col} = ?1"),
                [id],
                |r| r.get(0),
            )
            .unwrap();
        (table, n)
    })
    .filter(|(_, n)| *n > 0)
    .collect()
}

/// Pairs `b` into `a`'s group with a code, in process, the way the apps do.
async fn pair_with_code(a: &Node, b: &Node) {
    use wasl_ipc::PairingStage::*;
    let (pa, pb) = (
        a.daemon.pairing.clone().unwrap(),
        b.daemon.pairing.clone().unwrap(),
    );
    let code = pa.invite().unwrap().code.unwrap();
    pb.join(&code).unwrap();
    let (sa, sb) = (until(a, Confirm).await, until(b, Confirm).await);
    assert_eq!(sa.words, sb.words, "{sa:?} {sb:?}");
    pa.confirm(true).unwrap();
    pb.confirm(true).unwrap();
    assert_eq!(Paired, until(a, Paired).await.stage);
    assert_eq!(Paired, until(b, Paired).await.stage);
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_a_device_clears_every_row_kept_about_it() {
    let (a, b) = pair_up().await;
    let (da, db) = (
        a.daemon.devices.clone().unwrap(),
        b.daemon.devices.clone().unwrap(),
    );
    let idb = db.id().to_string();
    // What A holds about B: a share by name, an offer from it, when it was seen, how far its
    // index was read.
    da.share_with("p", false, &[db.id()]).unwrap();
    {
        let mut s = a.daemon.store.lock();
        s.put_received_offers(
            &idb,
            &[wasl_store::NewOffer {
                share: "ab".repeat(16),
                name: "Photos".into(),
                files: 1,
                bytes: 1,
            }],
            1,
        )
        .unwrap();
        s.set_peer_since("p", &idb, 7).unwrap();
        s.note_device_seen(&idb, 5).unwrap();
    }
    let before: Vec<_> = rows_about(&a, &idb).into_iter().map(|(t, _)| t).collect();
    for t in [
        "device_notes",
        "device_seen",
        "pair_share_devices",
        "received_offers",
        "peer_progress",
    ] {
        assert!(before.contains(&t), "A holds a {t} row about B: {before:?}");
    }

    da.remove_member(db.id()).unwrap();
    assert!(
        rows_about(&a, &idb).is_empty(),
        "rows about the removed device remain: {:?}",
        rows_about(&a, &idb)
    );
    assert!(
        a.daemon
            .store
            .lock()
            .device_notes()
            .unwrap()
            .iter()
            .all(|(d, _)| *d != idb)
    );
    assert!(da.offers().is_empty());
    assert_eq!(1, da.device_infos().len());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_removed_device_opens_with_a_group_of_its_own_and_pairs_again() {
    use wasl_ipc::PairingStage::*;
    let (a, b) = pair_up().await;
    let (da, db) = (
        a.daemon.devices.clone().unwrap(),
        b.daemon.devices.clone().unwrap(),
    );
    let old_b = db.id();
    da.remove_member(old_b).unwrap();
    // B was away: what it restarts with is its data folder, and A's group file now lists the
    // removal (it reaches B through the group file in Drive).
    db.close();
    drop(db);
    let data = restart_copy(&b, "data-b-removed");
    std::fs::copy(a.data.join("group.json"), data.join("group.json")).unwrap();
    let b2 = Daemon::open(config(data, b.secrets.clone(), "Desktop")).unwrap();
    let db2 = b2.devices.clone().unwrap();
    assert_ne!(
        old_b,
        db2.id(),
        "a removed key is never a member again: B takes a new one"
    );
    assert_eq!(1, db2.members().len(), "a group of its own");
    assert_eq!(db2.id(), db2.members()[0].id);
    let leftovers = {
        let s = b2.store.lock();
        s.device_notes().unwrap()
    };
    assert_eq!(
        vec![db2.id().to_string()],
        leftovers.into_iter().map(|(d, _)| d).collect::<Vec<_>>(),
        "only itself is noted"
    );

    // A new invitation from A, and B pairs as a new device.
    let b = Node {
        daemon: b2,
        root: b.root.clone(),
        secrets: b.secrets.clone(),
        data: b.data.clone(),
        _dir: tempfile::tempdir().unwrap(),
    };
    pair_with_code(&a, &b).await;
    let members = da.members();
    assert_eq!(2, members.len(), "{members:?}");
    assert!(members.iter().any(|m| m.id == db2.id()));
    assert!(members.iter().all(|m| m.id != old_b), "the old B stays gone");
    assert_eq!(da.members(), db2.members());
    // And they sync.
    da.connect(db2.local_addr().unwrap()).await.unwrap();
    std::fs::write(a.root.join("again.txt"), b"back").unwrap();
    a.daemon.run("p", Trigger::Manual).await;
    assert_eq!(Paired, until(&a, Paired).await.stage);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reset_device_leaves_the_old_entry_marked_not_seen_and_pairs_as_new() {
    use wasl_proto::{DevicePresence, Lang};
    let (a, b) = pair_up().await;
    let (da, db) = (
        a.daemon.devices.clone().unwrap(),
        b.daemon.devices.clone().unwrap(),
    );
    let old_b = db.id();
    assert!(da.connected().contains(&old_b));
    db.close();
    drop(db);
    // B's identity is reset: its data stays, its key is gone from the keychain.
    let data = restart_copy(&b, "data-b-reset");
    let fresh_secrets = Arc::new(MemorySecrets::default());
    let b2 = Daemon::open(config(data, fresh_secrets.clone(), "Desktop")).unwrap();
    let db2 = b2.devices.clone().unwrap();
    assert_ne!(old_b, db2.id());
    let b = Node {
        daemon: b2,
        root: b.root.clone(),
        secrets: fresh_secrets,
        data: b.data.clone(),
        _dir: tempfile::tempdir().unwrap(),
    };
    pair_with_code(&a, &b).await;

    let statuses = da.device_statuses(Lang::En);
    assert_eq!(3, statuses.len(), "{statuses:?}");
    let old = statuses.iter().find(|s| s.id == old_b.to_string()).unwrap();
    assert_ne!(
        DevicePresence::Online,
        old.state,
        "the old B is not online: {old:?}"
    );
    assert!(
        old.last_seen_at.is_some(),
        "it says when it was last seen: {old:?}"
    );
    let new = statuses.iter().find(|s| s.id == db2.id().to_string()).unwrap();
    assert!(!new.this_device);
    assert_eq!("Desktop", new.name);
}

/// A member of A's group that speaks `range`, as a device from another release would.
fn peer_speaking(range: wasl_net::proto::VersionRange, seed: u8) -> (wasl_net::Endpoint, [u8; 32]) {
    let key = wasl_net::DeviceKey::from_seed(&[seed; 32]);
    let ep = wasl_net::Endpoint::bind(
        "127.0.0.1:0".parse().unwrap(),
        &key,
        Arc::new(|_: &[u8; 32]| true),
    )
    .unwrap()
    .with_protocol_range(range);
    ep.serve(Arc::new(Nothing));
    (ep, key.public())
}

/// wasl#36: a device on another protocol version is refused with a typed reason that says which
/// side is behind and names the device, through the library and through the daemon's answer.
#[tokio::test(flavor = "multi_thread")]
async fn a_device_on_another_protocol_version_is_refused_with_a_typed_reason() {
    use wasl_devices::ConnectError;
    use wasl_ipc::{ErrorKind, Request};
    use wasl_net::proto::VersionRange;
    use wasl_proto::UpdateSide;

    let a = node("Laptop");
    let da = a.daemon.devices.clone().unwrap();

    // A device that speaks only newer versions: this one is behind.
    let (newer, newer_pk) = peer_speaking(VersionRange { min: 2, max: 3 }, 71);
    da.add_member(newer_pk, "Tablet", 1).unwrap();
    let newer_addr = newer.local_addr().unwrap();
    let Err(ConnectError::NeedsUpdate(n)) = da.connect(newer_addr).await else {
        panic!("a peer that needs a newer version is refused with a reason");
    };
    assert_eq!(UpdateSide::This, n.behind);
    assert_eq!(Some("Tablet"), n.name.as_deref());
    assert_eq!(
        wasl_net::DeviceKey::from_seed(&[71; 32]).id().to_string(),
        n.device
    );
    assert_eq!("device.this_needs_update", n.words.key);

    // The daemon's answer keeps the old shape (a network failure) and adds the typed reason.
    let err = a
        .daemon
        .handle(Request::ConnectDevice {
            address: newer_addr.to_string(),
        })
        .await
        .unwrap_err();
    assert_eq!(ErrorKind::Network, err.kind);
    let n = err.needs_update.expect("the typed reason rides along");
    assert_eq!(UpdateSide::This, n.behind);
    assert_eq!(Some("Tablet"), n.name.as_deref());
    assert!(err.message.contains(&n.words.text), "{}", err.message);

    // A device that speaks only older versions: it is the one behind.
    let (older, older_pk) = peer_speaking(VersionRange { min: 0, max: 0 }, 72);
    da.add_member(older_pk, "Phone", 2).unwrap();
    let err = a
        .daemon
        .handle(Request::ConnectDevice {
            address: older.local_addr().unwrap().to_string(),
        })
        .await
        .unwrap_err();
    let n = err.needs_update.expect("the typed reason rides along");
    assert_eq!(UpdateSide::Peer, n.behind);
    assert_eq!(Some("Phone"), n.name.as_deref());
    assert_eq!("device.peer_needs_update", n.words.key);

    // Anything else stays a plain network failure, with no typed reason: here a device that is
    // not in the group (a closed port would wait out a handshake timeout instead).
    let (stranger, _) = peer_speaking(VersionRange { min: 2, max: 3 }, 73);
    let err = a
        .daemon
        .handle(Request::ConnectDevice {
            address: stranger.local_addr().unwrap().to_string(),
        })
        .await
        .unwrap_err();
    assert_eq!(ErrorKind::Network, err.kind);
    assert!(err.needs_update.is_none());
}
