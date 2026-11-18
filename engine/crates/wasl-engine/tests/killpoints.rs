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
        // The old content is in the bin by the end wherever the run stopped after the copy was
        // made, and every file in the bin folder is indexed (an unindexed one would never expire).
        // A stop before the swap leaves an indexed copy of a file still in place, and the next
        // run copies it again.
        assert!(seen.binned <= 2, "{at}");
        assert_eq!(
            seen.bin_contents.len(),
            seen.binned,
            "{at}: a file in the bin folder is not in the index"
        );
        if matches!(
            seen.point,
            Point::AfterBin | Point::AfterRename | Point::BeforeRecord
        ) {
            assert!(
                seen.bin_contents.contains(&"old".to_owned()),
                "{at}: the old content is not in the bin: {:?}",
                seen.bin_contents
            );
        }
    })
    .await;
}

/// wasl#11, the other download site: a Google Doc exported again over its earlier export.
#[tokio::test(flavor = "multi_thread")]
async fn an_export_over_an_export_never_leaves_the_path_empty() {
    fn make() -> World {
        let w = World::new();
        w.drive.put_google_doc("Sync/Plan", wasl_core::docs::GOOGLE_DOC);
        futures_block(w.sync());
        // The doc changes on Drive: the next run exports it over the file from the first run.
        w.clock.advance(10_000);
        w.drive.put_file("Sync/Plan", b"edited");
        w
    }
    for_each_kill_point(make, |seen| {
        let at = format!("{:?}", seen.point);
        let name = "Plan.docx";
        assert!(
            seen.local_at_stop.contains_key(name),
            "{at}: at the stop the path was empty: {:?}",
            seen.local_at_stop
        );
        assert_eq!(RunStatus::Success, seen.status, "{at}");
        assert!(seen.local.contains_key(name), "{at}: {:?}", seen.local);
        assert!(
            seen.trashed.is_empty(),
            "{at}: trashed on Drive: {:?}",
            seen.trashed
        );
        assert_eq!(
            seen.bin_contents.len(),
            seen.binned,
            "{at}: an unindexed file in the bin"
        );
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
        // Once Drive's bin has the file, the restart must not bring it back. (A stop before that,
        // with the file installed but not yet recorded, is adopted on the restart and Drive keeps
        // its copy: a known gap, tracked apart from this change.)
        let trashed_already = matches!(seen.point, Point::AfterRemoteTrash | Point::AfterRowRemoved);
        assert!(
            !trashed_already || !seen.drive.contains_key("a.txt"),
            "{at}: still live on Drive: {:?}",
            seen.drive
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

/// wasl#41: a rename made on Drive is followed on the device through the apply layer, so its steps
/// are kill points like any other change to a local file.
#[tokio::test(flavor = "multi_thread")]
async fn a_rename_followed_from_drive_never_loses_the_file() {
    fn make() -> World {
        let w = World::new();
        w.write("a.txt", "mine");
        futures_block(w.sync());
        w.clock.advance(10_000);
        assert!(w.drive.move_path("Sync/a.txt", "Sync/b.txt"));
        w
    }
    for_each_kill_point(make, |seen| {
        let at = format!("{:?}", seen.point);
        assert_eq!(
            vec![&"mine".to_owned()],
            seen.local_at_stop.values().collect::<Vec<_>>(),
            "{at}: at the stop the file was not under exactly one name: {:?}",
            seen.local_at_stop
        );
        assert_eq!(RunStatus::Success, seen.status, "{at}");
        assert_eq!(map(&[("b.txt", "mine")]), seen.local, "{at}");
        assert_eq!(map(&[("b.txt", "mine")]), seen.drive, "{at}");
        assert!(seen.trashed.is_empty(), "{at}: {:?}", seen.trashed);
        assert_eq!(0, seen.binned, "{at}: a rename bins nothing");
    })
    .await;
}
