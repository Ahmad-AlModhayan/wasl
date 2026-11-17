//! A run stopped at every point of the apply path, then run again: nothing a person owns is lost,
//! locally or on the fake Drive. One command: `cargo test -p wasl-engine --test killpoints`.

mod common;

use common::kill::{Step, for_each_kill_point, for_each_kill_point_in};
use common::{World, map};
use std::sync::Arc;
use wasl_core::{RunStatus, SyncMode};
use wasl_engine::fault::{FaultPlan, Point};

/// A synced `a.txt` whose Drive copy then changed: the next run downloads over the local file.
fn replace_download() -> World {
    let w = World::new();
    w.write("a.txt", "old");
    futures_block(w.sync());
    w.put_drive("a.txt", "new");
    w
}

fn futures_block<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(f))
}

#[tokio::test(flavor = "multi_thread")]
async fn the_harness_reports_a_known_loss_at_the_point_it_happens() {
    // A scenario that does the unsafe thing on purpose: removes the file before the new one is in
    // place, with a point in between (what executor.rs did before the install was made safe).
    fn make() -> World {
        let w = World::new();
        w.write("a.txt", "mine");
        w
    }
    fn unsafe_swap<'a>(w: &'a World, plan: Arc<FaultPlan>) -> Step<'a> {
        Box::pin(async move {
            plan.pass(Point::AfterStage).ok();
            if plan.pass(Point::AfterFlush).is_err() {
                return;
            }
            w.remove("a.txt");
            if plan.pass(Point::AfterBin).is_err() {
                return;
            }
            w.write("a.txt", "theirs");
            plan.pass(Point::AfterRename).ok();
        })
    }
    let mut lost_at = Vec::new();
    for_each_kill_point_in(make, unsafe_swap, |seen| {
        let lost = seen.lost_locally(|_| vec!["mine".into(), "theirs".into()]);
        if !lost.is_empty() {
            lost_at.push((seen.index, seen.point, lost));
        }
    })
    .await;
    assert_eq!(
        vec![(2, Point::AfterBin, vec!["a.txt".to_owned()])],
        lost_at,
        "the file is lost exactly between the removal and the new file"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_download_over_a_file_never_leaves_the_path_empty() {
    for_each_kill_point(replace_download, |seen| {
        let at = format!("{:?}", seen.point);
        // At the stop itself, before any second run: the path holds the old or the new file.
        let at_stop = seen.local_at_stop.get("a.txt");
        assert!(
            matches!(at_stop.map(String::as_str), Some("old" | "new")),
            "{at}: at the stop the path held {at_stop:?}"
        );
        assert_eq!(RunStatus::Success, seen.status, "{at}");
        assert_eq!(
            Some(&"new".to_owned()),
            seen.local.get("a.txt"),
            "{at}: {:?}",
            seen.local
        );
        assert_eq!(map(&[("a.txt", "new")]), seen.drive, "{at}");
        assert!(
            seen.trashed.is_empty(),
            "{at}: trashed on Drive: {:?}",
            seen.trashed
        );
        // The old content is kept in the bin by the end, whichever point the stop was at (a stop
        // after the swap and before the bin is indexed leaves the copy unindexed: the next run
        // finds the file current and has nothing to bin).
        assert!(seen.binned <= 1, "{at}");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn move_mode_keeps_the_downloaded_file_whatever_the_stop() {
    fn make() -> World {
        let w = World::with(|p| p.method = SyncMode::DownloadThenDelete, None);
        w.put_drive("a.txt", "from drive");
        w
    }
    for_each_kill_point(make, |seen| {
        let at = format!("{:?}", seen.point);
        assert_eq!(
            Some(&"from drive".to_owned()),
            seen.local.get("a.txt"),
            "{at}: {:?}",
            seen.local
        );
    })
    .await;
}

/// Files that stay (so a deletion is not "everything") plus `a.txt`, synced; then `a.txt` goes
/// away on Drive.
fn delete_local() -> World {
    let w = World::new();
    w.write("a.txt", "mine");
    for n in ["k1.txt", "k2.txt", "k3.txt", "k4.txt"] {
        w.write(n, "kept");
    }
    futures_block(w.sync());
    w.drive.trash_path("Sync/a.txt");
    w
}

#[tokio::test(flavor = "multi_thread")]
async fn a_local_delete_never_loses_the_content() {
    for_each_kill_point(delete_local, |seen| {
        let at = format!("{:?}", seen.point);
        let kept_somewhere = seen.local.get("a.txt").is_some_and(|c| c == "mine")
            || seen.bin_contents.contains(&"mine".to_owned());
        assert!(kept_somewhere, "{at}: neither on the device nor in the bin");
        for n in ["k1.txt", "k2.txt", "k3.txt", "k4.txt"] {
            assert_eq!(Some(&"kept".to_owned()), seen.local.get(n), "{at}");
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_then_delete_never_loses_the_content() {
    fn make() -> World {
        let w = World::with(|p| p.method = SyncMode::UploadThenDelete, None);
        w.write("a.txt", "mine");
        w
    }
    for_each_kill_point(make, |seen| {
        let at = format!("{:?}", seen.point);
        let on_drive = seen.drive.get("a.txt").is_some_and(|c| c == "mine");
        let on_device = seen.local.get("a.txt").is_some_and(|c| c == "mine");
        assert!(on_drive || on_device, "{at}: not on Drive and not on the device");
    })
    .await;
}
