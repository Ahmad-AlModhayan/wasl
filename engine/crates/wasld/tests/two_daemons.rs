//! Two real `wasld` processes, each with its own home, on loopback, driven only through the IPC
//! API the way an app does (feature `miqyas`: the test build serves a test folder only).
//! Run with: `cargo test -p wasld --features miqyas --test two_daemons`.

mod support;
use std::time::Duration;
use support::{CURRENT, Device, expect_file, expect_file_with, id_of, pair, share_folder};

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
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(
        !b.path("after.txt").exists(),
        "B got a change after it was removed"
    );
}

/// What an ordinary person does: saves a file and waits. Nothing here asks the daemons to sync.
/// Ignored until wasl#38 is fixed: on Windows the change never arrives within 60 s.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "wasl#38: a new file does not sync by itself on Windows"]
async fn a_new_file_syncs_by_itself_without_pressing_sync() {
    let (_root, a, b) = two().await;
    pair(&a, &b).await;
    share_folder(&a, &b).await;
    std::fs::write(a.path("by-itself.txt"), "hello").unwrap();
    expect_file_with(&b, &[&a, &b], "by-itself.txt", Some("hello"), false).await;
}
