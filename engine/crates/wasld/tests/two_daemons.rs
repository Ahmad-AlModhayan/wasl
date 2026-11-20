//! Two real `wasld` processes, each with its own home, on loopback, driven only through the IPC
//! API the way an app does (feature `miqyas`: the test build serves a test folder only).
//! Run with: `cargo test -p wasld --features miqyas --test two_daemons`.

mod support;
use std::time::Duration;
use support::{CURRENT, Device, at_stage, expect_file, expect_file_with, id_of, pair, share_folder};
use wasl_ipc::PairingStage;

async fn two() -> (tempfile::TempDir, Device, Device) {
    let root = tempfile::tempdir().unwrap();
    let a = Device::start(CURRENT, root.path(), "A").await;
    let b = Device::start(CURRENT, root.path(), "B").await;
    (root, a, b)
}

#[tokio::test(flavor = "multi_thread")]
async fn two_processes_pair_with_an_invitation_and_show_the_same_four_words() {
    let (_root, a, b) = two().await;
    let (wa, wb) = pair(&a, &b).await;
    assert_eq!(4, wa.len(), "{wa:?}");
    assert_eq!(wa, wb, "both screens show the same words");
    for (d, other) in [(&a, "B"), (&b, "A")] {
        let devices = d.client().await.devices().await.unwrap().devices;
        let names: Vec<_> = devices.iter().map(|x| x.name.as_str()).collect();
        assert!(names.contains(&other), "{} lists {other}: {names:?}", d.name);
        assert_eq!(2, devices.len(), "{names:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shared_folder_syncs_creates_edits_and_deletes_both_ways() {
    let (_root, a, b) = two().await;
    pair(&a, &b).await;
    share_folder(&a, &b).await;
    let both = [&a, &b];

    std::fs::write(a.path("from-a.txt"), "one").unwrap();
    expect_file(&b, &both, "from-a.txt", Some("one")).await;
    std::fs::write(b.path("from-b.txt"), "two").unwrap();
    expect_file(&a, &both, "from-b.txt", Some("two")).await;

    std::fs::write(a.path("from-a.txt"), "one, edited on A").unwrap();
    expect_file(&b, &both, "from-a.txt", Some("one, edited on A")).await;
    std::fs::write(b.path("from-b.txt"), "two, edited on B").unwrap();
    expect_file(&a, &both, "from-b.txt", Some("two, edited on B")).await;

    std::fs::remove_file(a.path("from-a.txt")).unwrap();
    expect_file(&b, &both, "from-a.txt", None).await;
    std::fs::remove_file(b.path("from-b.txt")).unwrap();
    expect_file(&a, &both, "from-b.txt", None).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_a_device_drops_it_from_the_list_and_stops_its_changes() {
    let (_root, a, b) = two().await;
    pair(&a, &b).await;
    share_folder(&a, &b).await;
    let both = [&a, &b];
    std::fs::write(a.path("before.txt"), "x").unwrap();
    expect_file(&b, &both, "before.txt", Some("x")).await;

    let idb = id_of(&b).await;
    let ca = a.client().await;
    ca.remove_device(&idb).await.unwrap();
    let devices = ca.devices().await.unwrap().devices;
    assert!(
        devices.iter().all(|d| d.id != idb),
        "A still lists B: {devices:?}"
    );

    std::fs::write(a.path("after.txt"), "y").unwrap();
    // The same nudged syncs that delivered before.txt, now for five seconds: nothing may arrive.
    let until = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < until {
        a.sync_now().await;
        b.sync_now().await;
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    assert!(
        !b.path("after.txt").exists(),
        "B got a change after it was removed"
    );
}

/// What an ordinary person does: saves a file and waits. Nothing here asks the daemons to sync.
/// The file is written right after the first sync, before the schedule's next tick has started
/// the folder watchers (wasl#38): the change must still arrive, from the catch-up run.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_file_syncs_by_itself_without_pressing_sync() {
    let (_root, a, b) = two().await;
    pair(&a, &b).await;
    share_folder(&a, &b).await;
    std::fs::write(a.path("by-itself.txt"), "hello").unwrap();
    expect_file_with(&b, &[&a, &b], "by-itself.txt", Some("hello"), false).await;
}

/// What a person typing the code from another screen might produce: lower case, spaces for the
/// dash, a trailing space.
fn sloppy(short: &str) -> String {
    format!("{} ", short.to_lowercase().replace('-', "  "))
}

/// B types `code` (and says where A listens: loopback has no discovery) and A shows its words;
/// both confirm. Returns B's and A's four words.
async fn join_with_short_code(a: &Device, b: &Device, code: &str) -> (Vec<String>, Vec<String>) {
    let (ca, cb) = (a.client().await, b.client().await);
    let listen = ca.devices().await.unwrap().listen.expect("A listens");
    cb.join_device_at(code, &[listen]).await.unwrap();
    let wa = at_stage(&ca, a, PairingStage::Confirm).await.words;
    let wb = at_stage(&cb, b, PairingStage::Confirm).await.words;
    assert_eq!(wa, wb, "both screens show the same words");
    ca.confirm_pairing(true).await.unwrap();
    cb.confirm_pairing(true).await.unwrap();
    at_stage(&ca, a, PairingStage::Paired).await;
    at_stage(&cb, b, PairingStage::Paired).await;
    (wb, wa)
}

/// Waits for `c`'s pairing to fail and returns it.
async fn failed(c: &wasl_ipc::Client, d: &Device) -> wasl_ipc::PairingStatus {
    support::eventually(&format!("{} to show a failed pairing", d.name), &[d], || async {
        let s = c.pairing_status().await.ok()?;
        (s.stage == PairingStage::Failed).then_some(s)
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_typed_short_code_pairs_two_devices_that_then_sync() {
    let (_root, a, b) = two().await;
    let (ca, cb) = (a.client().await, b.client().await);
    let invited = ca.invite_device().await.unwrap();
    let short = invited
        .short_code
        .clone()
        .expect("the invitation shows 8 characters");
    assert_eq!(9, short.len(), "XXXX-XXXX: {short}");
    assert!(invited.code.is_some(), "the QR code is still there");

    let (words, _) = join_with_short_code(&a, &b, &sloppy(&short)).await;
    assert_eq!(4, words.len());
    // The code was used up by the pairing.
    assert_eq!(None, ca.pairing_status().await.unwrap().short_code);

    let (la, lb) = (
        ca.devices().await.unwrap().listen.unwrap(),
        cb.devices().await.unwrap().listen.unwrap(),
    );
    ca.connect_device(&lb).await.unwrap();
    cb.connect_device(&la).await.unwrap();
    for (d, other) in [(&a, "B"), (&b, "A")] {
        let names: Vec<String> = d
            .client()
            .await
            .devices()
            .await
            .unwrap()
            .devices
            .into_iter()
            .map(|x| x.name)
            .collect();
        assert!(
            names.iter().any(|n| n == other),
            "{} lists {other}: {names:?}",
            d.name
        );
    }
    share_folder(&a, &b).await;
    std::fs::write(a.path("typed.txt"), "paired by a typed code").unwrap();
    expect_file(&b, &[&a, &b], "typed.txt", Some("paired by a typed code")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_short_code_fails_in_plain_words_on_both_devices() {
    let (_root, a, b) = two().await;
    let (ca, cb) = (a.client().await, b.client().await);
    let short = ca.invite_device().await.unwrap().short_code.unwrap();
    // Same shape, other characters: change the first one.
    let first = short.chars().next().unwrap();
    let other = if first == '2' { '3' } else { '2' };
    let wrong = format!("{other}{}", &short[1..]);
    let listen = ca.devices().await.unwrap().listen.unwrap();
    cb.join_device_at(&wrong, &[listen]).await.unwrap();

    let sb = failed(&cb, &b).await;
    assert_eq!(Some("pair.wrong_code"), sb.failure.as_deref(), "{sb:?}");
    assert!(sb.message.unwrap().contains("didn't match"));
    // The device showing the code is told too, instead of waiting on a dead code.
    let sa = failed(&ca, &a).await;
    assert_eq!(Some("pair.cancelled"), sa.failure.as_deref(), "{sa:?}");

    // Not a code at all: nothing is started.
    cb.join_device_at("hello", &[]).await.unwrap();
    let junk = failed(&cb, &b).await;
    assert_eq!(Some("pair.bad_code"), junk.failure.as_deref());
    // Two devices that never met share nothing, on either side.
    assert_eq!(1, ca.devices().await.unwrap().devices.len());
    assert_eq!(1, cb.devices().await.unwrap().devices.len());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_removed_device_that_never_heard_of_it_pairs_again_as_a_new_one() {
    let (_root, a, mut b) = two().await;
    pair(&a, &b).await;
    let (ca, old_b) = (a.client().await, id_of(&b).await);

    // B is away (closed laptop) when A removes it, so it never learns of the removal.
    b.stop();
    ca.remove_device(&old_b).await.unwrap();
    b.restart().await;
    let cb = b.client().await;
    assert_eq!(old_b, id_of(&b).await, "B still holds its old key");

    // B tries to come back with its old key on a fresh code.
    let short = ca.invite_device().await.unwrap().short_code.unwrap();
    let listen = ca.devices().await.unwrap().listen.unwrap();
    cb.join_device_at(&short, std::slice::from_ref(&listen))
        .await
        .unwrap();
    let refused = failed(&cb, &b).await;
    assert_eq!(
        Some("pair.device_removed"),
        refused.failure.as_deref(),
        "{refused:?}"
    );
    // A refused it before the code was used up: A is still waiting with the same code.
    let still = ca.pairing_status().await.unwrap();
    assert_eq!(PairingStage::Waiting, still.stage, "{still:?}");
    assert_eq!(Some(short.clone()), still.short_code);

    // On restart B has recorded the removal: a new key and no leftovers.
    b.restart().await;
    let new_b = id_of(&b).await;
    assert_ne!(old_b, new_b, "B took a new key");
    assert_eq!(1, b.client().await.devices().await.unwrap().devices.len());

    // The very same code still works for it.
    join_with_short_code(&a, &b, &short).await;
    let ids: Vec<String> = ca
        .devices()
        .await
        .unwrap()
        .devices
        .into_iter()
        .map(|d| d.id)
        .collect();
    assert!(ids.contains(&new_b), "A lists the new B: {ids:?}");
    assert!(!ids.contains(&old_b), "A does not list the old B: {ids:?}");

    // The new B is a full member: a shared folder syncs both ways.
    let cb = b.client().await;
    let (la, lb) = (
        ca.devices().await.unwrap().listen.unwrap(),
        cb.devices().await.unwrap().listen.unwrap(),
    );
    ca.connect_device(&lb).await.unwrap();
    cb.connect_device(&la).await.unwrap();
    share_folder(&a, &b).await;
    let both = [&a, &b];
    std::fs::write(a.path("again-from-a.txt"), "welcome back").unwrap();
    expect_file(&b, &both, "again-from-a.txt", Some("welcome back")).await;
    std::fs::write(b.path("again-from-b.txt"), "thanks").unwrap();
    expect_file(&a, &both, "again-from-b.txt", Some("thanks")).await;
}
