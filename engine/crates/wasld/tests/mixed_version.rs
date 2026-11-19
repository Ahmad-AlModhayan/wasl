//! The previous release next to this one (wasl#27): a `wasld` built from the ref named in
//! `engine/PREVIOUS_RELEASE` runs as a real second daemon beside the `wasld` of this tree. They pair
//! with an invitation, sync a folder both ways, and one removes the other, with each version in each
//! role. The previous release's `wasl` command line also talks to this tree's daemon.
//!
//! The tests drive both daemons through the IPC API of this tree, so they also show that this
//! tree's clients can talk to the previous daemon.
//!
//! Needs the previous release built: `engine/scripts/mixed-version-test.sh` builds it and runs
//! this test. By hand, set `WASL_PREVIOUS_WASLD` and `WASL_PREVIOUS_WASL` to the two binaries
//! (`engine/scripts/build-previous-release.sh` prints them). With either one missing the tests
//! fail and say so; they never skip.
//!
//! If the current protocol range is narrowed on purpose so that the previous release is out of it,
//! pairing must be refused with the "needs update" reason (`QuicError::NeedsUpdate`), and these
//! tests then expect that refusal instead of a pair.

mod support;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use support::{CURRENT, Device, expect_file, id_of, pair, share_folder};
use wasl_core::{Reach, Route};
use wasl_store::Pair;

const PREVIOUS_WASLD: &str = "WASL_PREVIOUS_WASLD";
const PREVIOUS_WASL: &str = "WASL_PREVIOUS_WASL";
const HOW: &str = "Run engine/scripts/mixed-version-test.sh: it builds the release named in \
    engine/PREVIOUS_RELEASE and runs this test. This test never skips.";

/// The binary the previous release's daemon (or command line) was built into.
fn previous(var: &str) -> String {
    let path = std::env::var(var).unwrap_or_else(|_| panic!("{var} is not set. {HOW}"));
    assert!(
        PathBuf::from(&path).is_file(),
        "{var} names {path}, which is not a file. {HOW}"
    );
    path
}

/// One device from the previous release and one from this tree, named `old` and `new`.
async fn mixed() -> (tempfile::TempDir, Device, Device) {
    let wasld = previous(PREVIOUS_WASLD);
    let root = tempfile::tempdir().unwrap();
    let old = Device::start(&wasld, root.path(), "Old").await;
    let new = Device::start(CURRENT, root.path(), "New").await;
    (root, old, new)
}

/// `a` makes the invitation and `b` joins: the same four words on both, a shared folder that
/// syncs creates, edits and deletes in both directions, and `a` removing `b` ends its changes.
async fn pair_sync_both_ways_and_remove(a: &Device, b: &Device) {
    let (wa, wb) = pair(a, b).await;
    assert_eq!(4, wa.len(), "{wa:?}");
    assert_eq!(wa, wb, "both screens show the same words");
    for (d, other) in [(a, &b.name), (b, &a.name)] {
        let devices = d.client().await.devices().await.unwrap().devices;
        let names: Vec<_> = devices.iter().map(|x| x.name.as_str()).collect();
        assert!(
            names.contains(&other.as_str()),
            "{} lists {other}: {names:?}",
            d.name
        );
    }

    share_folder(a, b).await;
    let both = [a, b];
    std::fs::write(a.path("from-a.txt"), "one").unwrap();
    expect_file(b, &both, "from-a.txt", Some("one")).await;
    std::fs::write(b.path("from-b.txt"), "two").unwrap();
    expect_file(a, &both, "from-b.txt", Some("two")).await;

    std::fs::write(a.path("from-a.txt"), "one, edited").unwrap();
    expect_file(b, &both, "from-a.txt", Some("one, edited")).await;
    std::fs::write(b.path("from-b.txt"), "two, edited").unwrap();
    expect_file(a, &both, "from-b.txt", Some("two, edited")).await;

    std::fs::remove_file(a.path("from-a.txt")).unwrap();
    expect_file(b, &both, "from-a.txt", None).await;
    std::fs::remove_file(b.path("from-b.txt")).unwrap();
    expect_file(a, &both, "from-b.txt", None).await;

    // One removes the other: it leaves the list, and a later change no longer reaches it.
    let idb = id_of(b).await;
    let ca = a.client().await;
    ca.remove_device(&idb).await.unwrap();
    let devices = ca.devices().await.unwrap().devices;
    assert!(
        devices.iter().all(|d| d.id != idb),
        "{} still lists {}: {devices:?}",
        a.name,
        b.name
    );
    std::fs::write(a.path("after.txt"), "y").unwrap();
    let until = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < until {
        a.sync_now().await;
        b.sync_now().await;
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    assert!(
        !b.path("after.txt").exists(),
        "{} got a change after {} removed it",
        b.name,
        a.name
    );
}

/// The previous release makes the invitation and this tree joins it, then the previous release's
/// daemon shares the folder.
#[tokio::test(flavor = "multi_thread")]
async fn the_previous_release_invites_this_one_pairs_syncs_both_ways_and_removes_it() {
    let (_root, old, new) = mixed().await;
    pair_sync_both_ways_and_remove(&old, &new).await;
}

/// This tree makes the invitation and the previous release joins it, then this tree's daemon
/// shares the folder.
#[tokio::test(flavor = "multi_thread")]
async fn this_release_invites_the_previous_one_pairs_syncs_both_ways_and_removes_it() {
    let (_root, old, new) = mixed().await;
    pair_sync_both_ways_and_remove(&new, &old).await;
}

/// The previous release's command line connects to this tree's daemon and lists its pairs.
#[tokio::test(flavor = "multi_thread")]
async fn the_previous_releases_command_line_lists_the_pairs_of_this_daemon() {
    let wasl = previous(PREVIOUS_WASL);
    let root = tempfile::tempdir().unwrap();
    let new = Device::start(CURRENT, root.path(), "New").await;
    let mut p = Pair::new(
        "cli-pair",
        "Listed by the old CLI",
        &new.folder.display().to_string(),
    );
    p.route = Route::Devices;
    p.reach = Reach::Lan;
    new.client().await.create_pair(p).await.unwrap();

    let run = |args: &'static [&'static str]| {
        let (wasl, home) = (wasl.clone(), new.home.clone());
        tokio::task::spawn_blocking(move || {
            Command::new(wasl)
                .args(args)
                .env("WASL_HOME", home)
                .env("WASL_LANG", "en")
                .env("NO_COLOR", "1")
                .output()
                .unwrap()
        })
    };
    for args in [
        &["pair", "list"][..],
        &["pair", "list", "--json"][..],
        &["status"][..],
    ] {
        let out = run(args).await.unwrap();
        let (stdout, stderr) = (
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        assert!(
            out.status.success(),
            "the previous `wasl {}` failed ({}) against this daemon:\n{stdout}\n{stderr}\n{}",
            args.join(" "),
            out.status,
            new.logs()
        );
        assert!(
            stdout.contains("Listed by the old CLI") || stdout.contains("cli-pair"),
            "the previous `wasl {}` does not list the pair:\n{stdout}\n{stderr}",
            args.join(" ")
        );
    }
}
