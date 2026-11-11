//! Whole runs against a real folder and the in-memory Drive.

mod common;

use common::{DAY, Damaging, PAIR, Saving, Slow, SlowOpens, World, map};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wasl_core::conditions::{Blocker, Conditions, DeviceState};
use wasl_core::docs::GOOGLE_DOC;
use wasl_core::{BlockKind, ErrorKind, RunStatus, SyncMode};
use wasl_drive::DriveError;
use wasl_engine::{EngineConfig, HOLD_MESSAGE, MAX_LOCAL_VERSIONS, Op, STALLED, Trigger};
use wasl_store::BinSide;

// ---------------------------------------------------------------- first sync and convergence

#[tokio::test]
async fn first_sync_merges_both_sides_and_the_next_run_is_quiet() {
    let w = World::new();
    w.write("a.txt", "local a");
    w.write("docs/b.txt", "local b");
    w.put_drive("c.txt", "drive c");
    w.put_drive("pics/d.jpg", "drive d");

    // Both sides have files and nothing was synced before: the first sync stops to ask (FR-F2).
    let first = w.sync().await;
    assert_eq!(RunStatus::Blocked, first.status);
    assert_eq!(Some(BlockKind::FirstMerge), first.summary.unwrap().blocked_kind);
    assert!(
        !w.drive_files().contains_key("a.txt"),
        "nothing moved before the decision"
    );

    let run = w.sync_from_preview().await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    let expected = map(&[
        ("a.txt", "local a"),
        ("docs/b.txt", "local b"),
        ("c.txt", "drive c"),
        ("pics/d.jpg", "drive d"),
    ]);
    assert_eq!(expected, w.local_files());
    assert_eq!(expected, w.drive_files());

    let again = w.sync().await;
    assert_eq!(RunStatus::Success, again.status);
    let s = again.summary.unwrap();
    assert!(s.is_empty(), "{s:?}");
    assert_eq!(4, s.unchanged);
    assert!(!w.pair().status.approve_once, "the approval was used up");
}

#[tokio::test]
async fn a_pair_held_for_its_preview_waits_until_started_from_it() {
    let w = World::with(|p| p.status.hold_for_preview = true, None);
    w.write("a.txt", "a");
    let held = w.sync().await;
    assert_eq!(
        (RunStatus::Skipped, Some(HOLD_MESSAGE.to_owned())),
        (held.status, held.message)
    );
    assert!(w.drive_files().is_empty());
    let preview = w.engine.preview(&w.pair_id).await.unwrap();
    assert_eq!(1, preview.plan.summary.uploads, "the preview shows the upload");
    assert!(w.drive_files().is_empty(), "previewing changes nothing");
    assert_eq!(RunStatus::Success, w.sync_from_preview().await.status);
    assert_eq!(map(&[("a.txt", "a")]), w.drive_files());
    assert!(!w.pair().status.hold_for_preview);
}

// ---------------------------------------------------------------- edits and the bin

#[tokio::test]
async fn edits_flow_both_ways_and_overwritten_versions_go_to_the_bin() {
    let w = World::new();
    w.write("a.txt", "v1");
    assert_eq!(RunStatus::Success, w.sync().await.status);
    let id = w.drive.id_of("Sync/a.txt").unwrap();

    w.write("a.txt", "v2 from device");
    assert_eq!(RunStatus::Success, w.sync().await.status);
    assert_eq!(
        Some(id.clone()),
        w.drive.id_of("Sync/a.txt"),
        "same Drive file, new version"
    );
    assert_eq!("v2 from device", w.drive_files()["a.txt"]);

    w.put_drive("a.txt", "v3 from drive");
    assert_eq!(RunStatus::Success, w.sync().await.status);
    assert_eq!(Some("v3 from drive".into()), w.read("a.txt"));

    // The device copy that Drive's version replaced is in the bin, and restores byte for byte.
    let bin = w.engine.store().lock().bin_entries(Some(&w.pair_id)).unwrap();
    assert_eq!(1, bin.len());
    assert_eq!(
        (BinSide::Local, "OVERWRITTEN"),
        (bin[0].entry.side, bin[0].entry.reason.as_str())
    );
    w.remove("a.txt");
    w.engine.restore_from_bin(&bin[0].entry.id).await.unwrap();
    assert_eq!(Some("v2 from device".into()), w.read("a.txt"));
    assert!(w.engine.store().lock().bin_entries(None).unwrap().is_empty());
}

#[tokio::test]
async fn a_rename_on_drive_is_a_rename_here_not_a_download() {
    let w = World::new();
    w.write("old/report.txt", "content");
    w.write("same-folder.txt", "two");
    w.write("edited-here.txt", "three");
    assert_eq!(RunStatus::Success, w.sync().await.status);
    let downloads = w.drive.calls("Download");

    w.drive
        .move_path("Sync/old/report.txt", "Sync/new/place/report-final.txt");
    w.drive.move_path("Sync/same-folder.txt", "Sync/renamed.txt");
    // Edited on the device before the rename arrives: not followed, the planner decides.
    w.drive
        .move_path("Sync/edited-here.txt", "Sync/edited-renamed.txt");
    std::thread::sleep(Duration::from_millis(20));
    w.write("edited-here.txt", "three, changed here");

    let run = w.sync().await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    assert_eq!(Some("content".into()), w.read("new/place/report-final.txt"));
    assert_eq!(None, w.read("old/report.txt"));
    assert_eq!(Some("two".into()), w.read("renamed.txt"));
    // Followed, not downloaded and binned: nothing of either went to the bin.
    let bin = w.engine.store().lock().bin_entries(Some(&w.pair_id)).unwrap();
    assert!(
        bin.iter()
            .all(|b| b.entry.key != "old/report.txt" && b.entry.key != "same-folder.txt"),
        "{bin:?}"
    );
    assert_eq!(
        downloads + 1,
        w.drive.calls("Download"),
        "only the file edited here is fetched under its new name"
    );
    assert_eq!(w.local_files(), w.drive_files());
    let again = w.sync().await;
    assert!(again.summary.unwrap().is_empty());
}

#[tokio::test]
async fn a_rename_into_an_ignored_folder_or_on_an_upload_pair_is_not_followed() {
    let w = World::new();
    w.write(".waslignore", "private/\n");
    w.write("doc.txt", "d");
    assert_eq!(RunStatus::Success, w.sync().await.status);
    w.drive.move_path("Sync/doc.txt", "Sync/private/doc.txt");
    w.sync().await;
    // Moved out of what syncs: it leaves the device through the bin, never into the ignored
    // folder.
    assert_eq!(
        None,
        w.read("private/doc.txt"),
        "never renamed into an ignored folder"
    );
    let bin = w.engine.store().lock().bin_entries(Some(&w.pair_id)).unwrap();
    assert!(bin.iter().any(|b| b.entry.key == "doc.txt"), "{bin:?}");

    let up = World::with(|p| p.method = SyncMode::UploadOnly, None);
    up.write("keep.txt", "k");
    assert_eq!(RunStatus::Success, up.sync().await.status);
    up.drive.move_path("Sync/keep.txt", "Sync/moved.txt");
    up.sync().await;
    assert_eq!(
        Some("k".into()),
        up.read("keep.txt"),
        "Drive never rearranges an upload pair"
    );
}

#[tokio::test]
async fn deletions_propagate_to_the_bins_on_both_sides_and_restore() {
    let w = World::new();
    w.write("keep.txt", "k");
    w.write("gone-local.txt", "l");
    w.write("gone-drive.txt", "d");
    assert_eq!(RunStatus::Success, w.sync().await.status);

    w.remove("gone-local.txt");
    w.drive.trash_path("Sync/gone-drive.txt");
    let run = w.sync().await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    assert_eq!(map(&[("keep.txt", "k")]), w.local_files());
    assert_eq!(map(&[("keep.txt", "k")]), w.drive_files());

    let bin = w.engine.store().lock().bin_entries(Some(&w.pair_id)).unwrap();
    let side_of = |name: &str| bin.iter().find(|b| b.entry.key == name).map(|b| b.entry.side);
    assert_eq!(
        Some(BinSide::Drive),
        side_of("gone-local.txt"),
        "deleted on the device → Drive's bin"
    );
    assert_eq!(
        Some(BinSide::Local),
        side_of("gone-drive.txt"),
        "deleted on Drive → the device's bin"
    );

    for b in &bin {
        w.engine.restore_from_bin(&b.entry.id).await.unwrap();
    }
    assert_eq!(Some("d".into()), w.read("gone-drive.txt"));
    assert_eq!(Some(&"l".to_owned()), w.drive_files().get("gone-local.txt"));
}

/// Restoring a deleted folder brings back the files deleted with it, not an empty folder
/// (E2E 2026-10: 50 files had to be restored one by one).
#[tokio::test]
async fn restoring_a_deleted_folder_restores_what_was_deleted_with_it() {
    let w = World::new();
    w.write("design/a.txt", "a");
    w.write("design/fonts/b.woff2", "b");
    w.write("other.txt", "o");
    w.sync().await;
    std::fs::remove_dir_all(w.path("design")).unwrap();
    let run = w.sync().await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    assert_eq!(map(&[("other.txt", "o")]), w.drive_files());

    let bin = w.engine.store().lock().bin_entries(Some(&w.pair_id)).unwrap();
    let folder = bin
        .iter()
        .find(|b| b.entry.key == "design")
        .expect("the folder is in the bin");
    let r = w
        .engine
        .restore_bin_entries(std::slice::from_ref(&folder.entry.id))
        .await
        .unwrap();
    assert!(r.failed.is_empty(), "{:?}", r.failed);
    assert_eq!(
        map(&[
            ("design/a.txt", "a"),
            ("design/fonts/b.woff2", "b"),
            ("other.txt", "o")
        ]),
        w.drive_files()
    );
}

#[tokio::test]
async fn local_versions_are_capped_and_expire() {
    let w = World::new();
    w.write("a.txt", "v0");
    w.sync().await;
    for v in 1..=7 {
        w.put_drive("a.txt", &format!("v{v}"));
        assert_eq!(RunStatus::Success, w.sync().await.status);
    }
    let versions = w.engine.store().lock().bin_versions(&w.pair_id, "a.txt").unwrap();
    assert_eq!(
        MAX_LOCAL_VERSIONS as usize,
        versions.len(),
        "only the newest versions are kept"
    );
    assert_eq!(
        MAX_LOCAL_VERSIONS as usize,
        std::fs::read_dir(&w.env.bin).unwrap().count(),
        "older files deleted"
    );

    w.clock.advance(31 * DAY);
    assert_eq!(
        MAX_LOCAL_VERSIONS as usize,
        w.engine.purge_expired_bin().await.unwrap()
    );
    assert_eq!(0, std::fs::read_dir(&w.env.bin).unwrap().count());
}

// ---------------------------------------------------------------- conflicts

#[tokio::test]
async fn keep_both_keeps_the_device_copy_under_a_conflict_name() {
    let w = World::new();
    w.write("notes.txt", "base");
    w.sync().await;
    w.write("notes.txt", "device edit");
    w.put_drive("notes.txt", "drive edit");
    let run = w.sync().await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    let copy = "notes (conflicted copy from Pixel, 2026-10-20 09-00).txt";
    let expected = map(&[("notes.txt", "drive edit"), (copy, "device edit")]);
    assert_eq!(expected, w.local_files());
    assert_eq!(expected, w.drive_files());
    let conflicts = w.engine.store().lock().open_conflicts(Some(&w.pair_id)).unwrap();
    assert_eq!(Some(copy.to_owned()), conflicts[0].local_copy_rel);
    assert!(w.sync().await.summary.unwrap().is_empty(), "converged");
}

/// #145 L2: the conflict stamp is to the minute, so a second conflict on one file within that
/// minute numbers its copy instead of failing the run on the first copy's name.
#[tokio::test]
async fn a_second_conflict_in_the_same_minute_keeps_both_copies() {
    let w = World::new();
    w.write("notes.txt", "base");
    w.sync().await;
    w.write("notes.txt", "device 1");
    w.put_drive("notes.txt", "drive 1");
    assert_eq!(RunStatus::Success, w.sync().await.status);
    w.write("notes.txt", "device 2");
    w.put_drive("notes.txt", "drive 2");
    let run = w.sync().await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    let first = "notes (conflicted copy from Pixel, 2026-10-20 09-00).txt";
    let second = "notes (conflicted copy from Pixel, 2026-10-20 09-00, 2).txt";
    let expected = map(&[
        ("notes.txt", "drive 2"),
        (first, "device 1"),
        (second, "device 2"),
    ]);
    assert_eq!(expected, w.local_files());
    assert_eq!(expected, w.drive_files());
}

/// Drive's change list reports another device's upload seconds late. An edit made here in that
/// window must not overwrite the other device's version on Drive: it waits, and once the listing
/// catches up both are kept (E2E 2026-10: the phone's edit was overwritten and only the bin had it).
#[tokio::test]
async fn an_upload_never_overwrites_a_drive_version_the_listing_has_not_shown_yet() {
    let w = World::new();
    w.write("notes.txt", "base");
    w.sync().await;
    w.drive.lag_listing(true);
    w.put_drive("notes.txt", "other device");
    w.write("notes.txt", "this device");

    let run = w.sync().await;
    assert_eq!(
        Some("other device".to_owned()),
        w.drive_files().get("notes.txt").cloned(),
        "Drive keeps the version this run had not seen: {:?}",
        run.message
    );

    w.drive.lag_listing(false);
    let run = w.sync().await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    let copy = "notes (conflicted copy from Pixel, 2026-10-20 09-00).txt";
    let expected = map(&[("notes.txt", "other device"), (copy, "this device")]);
    assert_eq!(expected, w.local_files());
    assert_eq!(expected, w.drive_files());
}

// ---------------------------------------------------------------- safety guards

#[tokio::test]
async fn a_mass_deletion_stops_until_approved_once() {
    let w = World::new();
    for i in 0..30 {
        w.write(&format!("f{i:02}.txt"), "x");
    }
    w.sync().await;
    for i in 0..25 {
        w.remove(&format!("f{i:02}.txt"));
    }
    let stopped = w.sync().await;
    assert_eq!(RunStatus::Blocked, stopped.status);
    assert_eq!(30, w.drive_files().len(), "Drive still has every file");
    assert!(w.pair().status.pending_approval);
    assert!(
        w.engine
            .store()
            .lock()
            .blocked_plan(&w.pair_id)
            .unwrap()
            .is_some()
    );

    // Stays stopped on its own…
    assert_eq!(RunStatus::Blocked, w.sync().await.status);
    // …until the user approves once.
    w.engine.approve_once(&w.pair_id).unwrap();
    assert_eq!(RunStatus::Success, w.sync().await.status);
    assert_eq!(5, w.drive_files().len());
    let p = w.pair();
    assert!(!p.status.pending_approval && !p.status.approve_once);
    assert!(
        w.engine
            .store()
            .lock()
            .blocked_plan(&w.pair_id)
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn emptying_a_synced_folder_never_empties_drive() {
    let w = World::new();
    w.write("a.txt", "a");
    w.write("b.txt", "b");
    w.sync().await;
    w.remove("a.txt");
    w.remove("b.txt");
    let r = w.sync().await;
    assert_eq!(RunStatus::Blocked, r.status);
    assert_eq!(Some(BlockKind::Wipe), r.summary.unwrap().blocked_kind);
    assert_eq!(2, w.drive_files().len());
}

// ---------------------------------------------------------------- ignore rules and names

#[tokio::test]
async fn ignored_folders_never_leave_the_device_and_new_rules_delete_nothing() {
    let w = World::with(|p| p.presets = vec!["node".into()], None);
    w.write("app/src/index.js", "code");
    w.write("app/node_modules/react/index.js", "dep");
    w.write(".waslignore", "*.log\n");
    w.write("debug.log", "noise");
    assert_eq!(RunStatus::Success, w.sync().await.status);
    assert_eq!(
        map(&[(".waslignore", "*.log\n"), ("app/src/index.js", "code")]),
        w.drive_files()
    );

    // A rule added after a file was synced keeps Drive's copy (FR-G5).
    w.write("build.out", "artifact");
    w.sync().await;
    assert!(w.drive_files().contains_key("build.out"));
    w.write(".waslignore", "*.log\n*.out\n");
    w.remove("build.out");
    assert_eq!(RunStatus::Success, w.sync().await.status);
    assert!(
        w.drive_files().contains_key("build.out"),
        "the new rule hides the file; it is not deleted"
    );
}

#[tokio::test]
async fn drive_names_the_device_rejects_are_mapped_and_round_trip() {
    let w = World::new();
    w.put_drive("Q&A: what?.txt", "answer");
    assert_eq!(RunStatus::Success, w.sync().await.status);
    assert_eq!(Some("answer".into()), w.read("Q&A： what？.txt"));
    assert!(
        w.sync().await.summary.unwrap().is_empty(),
        "the mapped name is recognised next time"
    );
    w.write("Q&A： what？.txt", "edited");
    assert_eq!(RunStatus::Success, w.sync().await.status);
    assert_eq!(
        map(&[("Q&A: what?.txt", "edited")]),
        w.drive_files(),
        "the Drive name is never changed"
    );
}

#[tokio::test]
async fn google_docs_arrive_as_office_mirrors_and_duplicates_are_reported() {
    let w = World::new();
    w.drive.put_google_doc("Sync/Plan", GOOGLE_DOC);
    w.put_drive("same.txt", "one");
    w.drive.put_duplicate("Sync/same.txt", b"two");
    w.drive.put_shortcut("Sync/link");
    let preview = w.engine.preview(&w.pair_id).await.unwrap();
    assert!(
        preview.notes.iter().any(|n| n.contains("has the same name")),
        "{:?}",
        preview.notes
    );
    // Not a problem: reported, but apart from what needs the user.
    assert!(
        preview.infos.iter().any(|n| n.contains("shortcut")),
        "{:?}",
        preview.infos
    );
    assert!(!preview.notes.iter().any(|n| n.contains("shortcut")));
    assert!(
        preview.infos.iter().any(|n| n.starts_with("Compared ")),
        "{:?}",
        preview.infos
    );
    assert_eq!(RunStatus::Success, w.sync().await.status);
    assert!(w.read("Plan.docx").unwrap().starts_with("export of"));
    assert_eq!(
        1,
        w.local_files().keys().filter(|k| k.starts_with("same")).count()
    );
}

// ---------------------------------------------------------------- methods

#[tokio::test]
async fn upload_then_delete_moves_files_off_the_device() {
    let w = World::with(|p| p.method = SyncMode::UploadThenDelete, None);
    w.write("photo.jpg", "pixels");
    assert_eq!(RunStatus::Success, w.sync().await.status);
    assert!(w.local_files().is_empty());
    assert_eq!(map(&[("photo.jpg", "pixels")]), w.drive_files());
    assert!(
        w.sync().await.summary.unwrap().is_empty(),
        "and nothing comes back"
    );
}

#[tokio::test]
async fn download_mirror_bins_local_extras() {
    let w = World::with(|p| p.method = SyncMode::DownloadMirror, None);
    w.put_drive("a.txt", "a");
    w.write("extra.txt", "mine");
    // Both sides have files: the first sync stops to ask, even for a mirror.
    assert_eq!(RunStatus::Blocked, w.sync().await.status);
    assert_eq!(RunStatus::Success, w.sync_from_preview().await.status);
    assert_eq!(map(&[("a.txt", "a")]), w.local_files());
    let bin = w.engine.store().lock().bin_entries(None).unwrap();
    assert_eq!(
        ("extra.txt", BinSide::Local),
        (bin[0].entry.key.as_str(), bin[0].entry.side)
    );
}

// ---------------------------------------------------------------- failures

#[tokio::test]
async fn one_bad_file_does_not_stop_the_others() {
    let w = World::new();
    w.write("good1.txt", "1");
    w.write("refused.txt", "x");
    w.write("good2.txt", "2");
    w.drive.poison("refused.txt");
    let r = w.sync().await;
    assert_eq!(RunStatus::Partial, r.status);
    assert_eq!(
        Some("refused.txt: Couldn't upload. Google Drive refused it (400, poisoned). The next sync tries again.".into()),
        r.message
    );
    assert_eq!(map(&[("good1.txt", "1"), ("good2.txt", "2")]), w.drive_files());
    let p = w.pair();
    assert_eq!(
        (Some(RunStatus::Partial), 1),
        (p.status.last_status, p.status.consecutive_failures)
    );
}

#[tokio::test]
async fn sign_in_failures_stop_the_run_and_are_recorded() {
    let w = World::new();
    w.write("a.txt", "a");
    w.drive.fail_next(
        Op::Upload,
        DriveError::Auth {
            message: "Sign in again".into(),
            user_action_required: true,
        },
    );
    let r = w.sync().await;
    assert_eq!(
        (RunStatus::Failed, Some(ErrorKind::Auth)),
        (r.status, r.error_kind)
    );
    assert_eq!(Some(ErrorKind::Auth), w.pair().status.last_error_kind);
    // Once Drive answers again, the next run succeeds and the failure count resets.
    assert_eq!(RunStatus::Success, w.sync().await.status);
    assert_eq!(0, w.pair().status.consecutive_failures);
}

#[tokio::test]
async fn a_full_drive_pauses_the_pair() {
    let w = World::new();
    w.write("a.txt", "a");
    w.drive
        .fail_next(Op::Upload, DriveError::StorageQuota("full".into()));
    assert_eq!(Some(ErrorKind::Quota), w.sync().await.error_kind);
    let p = w.pair();
    assert!(p.paused);
    assert_eq!(
        RunStatus::Skipped,
        w.engine.sync(&w.pair_id, Trigger::Schedule).await.status,
        "paused pairs wait"
    );
}

#[tokio::test]
async fn a_missing_drive_folder_fails_clearly_and_deletes_nothing() {
    let w = World::new();
    w.write("a.txt", "a");
    w.sync().await;
    w.drive.trash_path("Sync");
    let r = w.sync().await;
    assert_eq!(
        (RunStatus::Failed, Some(ErrorKind::NotFound)),
        (r.status, r.error_kind)
    );
    assert!(r.message.unwrap().contains("no longer exists"));
    assert_eq!(map(&[("a.txt", "a")]), w.local_files());
}

// ---------------------------------------------------------------- interruption (FR-B3, FR-B4)

#[tokio::test]
async fn a_cancelled_run_keeps_finished_files_and_the_next_run_does_only_the_rest() {
    let w = Arc::new(World::with(
        |_| {},
        Some(Box::new(|m| {
            Arc::new(Slow {
                inner: m,
                delay: Duration::from_millis(60),
                hang_downloads: false,
                hang_listing: false,
            })
        })),
    ));
    for i in 0..12 {
        w.write(&format!("f{i:02}.txt"), &format!("{i}"));
    }
    let runner = {
        let w = w.clone();
        tokio::spawn(async move { w.sync().await })
    };
    // Stop once a few uploads are done.
    loop {
        tokio::time::sleep(Duration::from_millis(20)).await;
        if w.drive.calls("Upload") >= 4 {
            break;
        }
    }
    w.engine.cancel(&w.pair_id);
    let first = runner.await.unwrap();
    assert_eq!(RunStatus::Cancelled, first.status);
    let done = w.drive_files().len();
    assert!((1..12).contains(&done), "some but not all uploaded: {done}");

    let before = w.drive.calls("Upload");
    assert_eq!(RunStatus::Success, w.sync().await.status);
    assert_eq!(12, w.drive_files().len());
    assert_eq!(
        12 - done as u32,
        w.drive.calls("Upload") - before,
        "finished files are not uploaded again"
    );
}

#[tokio::test]
async fn a_run_that_stops_moving_is_abandoned() {
    let config = EngineConfig {
        device_name: "Pixel".into(),
        stall_limit: Duration::from_millis(300),
        watchdog_tick: Duration::from_millis(50),
        ..EngineConfig::default()
    };
    let w = World::with_config(
        |_| {},
        Some(Box::new(|m| {
            Arc::new(Slow {
                inner: m,
                delay: Duration::ZERO,
                hang_downloads: true,
                hang_listing: false,
            })
        })),
        config,
    );
    w.put_drive("stuck.bin", "x");
    let r = w.sync().await;
    assert_eq!(
        (
            RunStatus::Failed,
            Some(STALLED.to_owned()),
            Some(ErrorKind::Network)
        ),
        (r.status, r.message, r.error_kind)
    );
    assert!(!w.engine.is_running(&w.pair_id));
}

/// E2E 2026-10, the owner's phone: a run stuck in a step that never looked at the stop flag kept
/// "Syncing" for 40 minutes; Stop, Pause and the 15-minute watchdog all only raised the flag.
/// Stopping now ends the run whatever step it is in.
#[tokio::test]
async fn stop_ends_a_run_stuck_while_listing_drive() {
    let w = Arc::new(World::with(
        |_| {},
        Some(Box::new(|m| {
            Arc::new(Slow {
                inner: m,
                delay: Duration::ZERO,
                hang_downloads: false,
                hang_listing: true,
            })
        })),
    ));
    let runner = {
        let w = w.clone();
        tokio::spawn(async move { w.sync().await })
    };
    while !w.engine.is_running(&w.pair_id) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    w.engine.cancel(&w.pair_id);
    let r = tokio::time::timeout(Duration::from_secs(5), runner)
        .await
        .expect("the run ends once stopped")
        .unwrap();
    assert_eq!(RunStatus::Cancelled, r.status);
    assert!(!w.engine.is_running(&w.pair_id));
}

/// A first sync compares every file both sides have by content, in one go (5.3 GB for the owner's
/// LifeOS). Stop used to end the run but leave those reads going until the last file.
#[tokio::test]
async fn stop_ends_a_runs_content_check_between_files() {
    let w = Arc::new(World::new());
    for i in 0..40 {
        w.write(&format!("f{i}.txt"), &format!("same {i}"));
        w.put_drive(&format!("f{i}.txt"), &format!("same {i}"));
    }
    let slow = Arc::new(SlowOpens {
        delay: Duration::from_millis(50),
        ..SlowOpens::default()
    });
    *w.env.slow_opens.lock().unwrap() = Some(slow.clone());
    let runner = {
        let w = w.clone();
        tokio::spawn(async move { w.sync().await })
    };
    while slow.opened.load(Ordering::SeqCst) == 0 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    w.engine.cancel(&w.pair_id);
    let r = tokio::time::timeout(Duration::from_secs(5), runner)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(RunStatus::Cancelled, r.status);
    // Long enough for the rest of the 40 files, had the reads carried on.
    tokio::time::sleep(Duration::from_millis(800)).await;
    let opened = slow.opened.load(Ordering::SeqCst);
    assert!(opened <= 8, "{opened} of 40 files read after Stop");
}

#[tokio::test]
async fn a_run_stuck_while_listing_drive_is_abandoned() {
    let config = EngineConfig {
        device_name: "Pixel".into(),
        stall_limit: Duration::from_millis(300),
        watchdog_tick: Duration::from_millis(50),
        ..EngineConfig::default()
    };
    let w = World::with_config(
        |_| {},
        Some(Box::new(|m| {
            Arc::new(Slow {
                inner: m,
                delay: Duration::ZERO,
                hang_downloads: false,
                hang_listing: true,
            })
        })),
        config,
    );
    let r = tokio::time::timeout(Duration::from_secs(5), w.sync())
        .await
        .expect("the watchdog ends it");
    assert_eq!(
        (RunStatus::Failed, Some(STALLED.to_owned())),
        (r.status, r.message)
    );
}

/// The owner's phone, 2026-10-29: previewing a 12,000-file folder already on Drive hashed every
/// file, and the sync started from the preview hashed them all again. A file whose size and time
/// are unchanged since the preview is not read again.
#[tokio::test]
async fn the_sync_after_a_preview_reuses_its_hashes() {
    let w = World::new();
    w.put_drive("a.txt", "hello");
    w.write("a.txt", "hello");
    let preview = w.engine.preview(&w.pair_id).await.unwrap();
    let s = &preview.plan.summary;
    assert_eq!(0, s.uploads + s.downloads + s.conflicts);

    // Same size and time, different bytes: only a file read again would notice.
    let p = w.path("a.txt");
    let mtime = filetime::FileTime::from_last_modification_time(&std::fs::metadata(&p).unwrap());
    std::fs::write(&p, "HELLO").unwrap();
    filetime::set_file_mtime(&p, mtime).unwrap();

    let r = w.engine.sync(&w.pair_id, Trigger::Preview).await;
    assert_eq!(RunStatus::Success, r.status);
    let s = r.summary.unwrap();
    assert_eq!(
        0,
        s.uploads + s.downloads + s.conflicts,
        "planned from the preview's hash"
    );
}

/// The owner's phone, 2026-10-29: two .xlsx files stopped a run with "Drive reported md5 … but the
/// device file hashes to …", which left the user nothing to do.
#[tokio::test]
async fn a_file_saved_just_before_its_upload_syncs_without_an_error() {
    let w = World::with(
        |_| {},
        Some(Box::new(|m| {
            Arc::new(Saving {
                inner: m,
                saves: Mutex::new(Some((Some("saved again"), None))),
            })
        })),
    );
    w.write("book.xlsx", "first");
    let r = w.sync().await;
    assert_eq!(RunStatus::Success, r.status, "{:?}", r.message);
    assert_eq!(
        "saved again",
        String::from_utf8(w.drive.content("Sync/book.xlsx").unwrap()).unwrap()
    );
    let again = w.sync().await;
    assert_eq!(RunStatus::Success, again.status);
    assert!(again.summary.unwrap().is_empty(), "nothing left to do");
}

#[tokio::test]
async fn a_file_still_being_saved_uploads_its_final_version_next_time() {
    let w = World::with(
        |_| {},
        Some(Box::new(|m| {
            Arc::new(Saving {
                inner: m,
                saves: Mutex::new(Some((None, Some("final version")))),
            })
        })),
    );
    w.write("book.xlsx", "first");
    let r = w.sync().await;
    assert_eq!(
        RunStatus::Success,
        r.status,
        "a warning, not a failure: {:?}",
        r.message
    );
    let next = w.sync().await;
    assert_eq!(RunStatus::Success, next.status);
    let s = next.summary.unwrap();
    assert_eq!(
        (1, 0),
        (s.uploads, s.conflicts),
        "uploaded over, no conflict copy"
    );
    assert_eq!(
        "final version",
        String::from_utf8(w.drive.content("Sync/book.xlsx").unwrap()).unwrap()
    );
}

/// Drive received the bytes damaged while the device file stayed the same: that is not a file
/// still being saved. The upload is sent again, and the run ends with Drive holding the right bytes.
#[tokio::test]
async fn an_upload_drive_received_damaged_is_sent_again() {
    let w = World::with(|_| {}, Some(Damaging::wrap("book.xlsx", 1, false)));
    w.write("book.xlsx", "first");
    let r = w.sync().await;
    assert_eq!(RunStatus::Success, r.status, "{:?}", r.message);
    assert_eq!(
        "first",
        String::from_utf8(w.drive.content("Sync/book.xlsx").unwrap()).unwrap()
    );
    let again = w.sync().await;
    assert_eq!(RunStatus::Success, again.status);
    assert!(again.summary.unwrap().is_empty(), "nothing left to do");
}

/// Damaged every time in this run: an error that says so (not "still being saved"), and the next
/// sync uploads over Wasl's damaged copy instead of keeping both as a conflict.
#[tokio::test]
async fn an_upload_damaged_twice_fails_and_the_next_sync_replaces_it() {
    let w = World::with(|_| {}, Some(Damaging::wrap("book.xlsx", 2, false)));
    w.write("book.xlsx", "first");
    let r = w.sync().await;
    assert_eq!(RunStatus::Partial, r.status);
    let message = r.message.unwrap_or_default();
    assert!(message.contains("damaged"), "{message}");
    let events = w.engine.store().lock().run_events(&r.run_id).unwrap();
    assert!(
        events.iter().all(|e| !e.message.contains("still being saved")),
        "not reported as a file still being saved: {events:?}"
    );
    let next = w.sync().await;
    assert_eq!(RunStatus::Success, next.status, "{:?}", next.message);
    let s = next.summary.unwrap();
    assert_eq!(
        (1, 0),
        (s.uploads, s.conflicts),
        "uploaded over, no conflict copy"
    );
    assert_eq!(
        "first",
        String::from_utf8(w.drive.content("Sync/book.xlsx").unwrap()).unwrap()
    );
}

/// Deleted on the device while it uploaded: the run says so, rather than promising the next sync
/// uploads a newer version (the next sync sees the file gone and treats it as deleted).
#[tokio::test]
async fn a_file_deleted_while_uploading_is_reported_as_deleted() {
    let w = World::with(|_| {}, Some(Damaging::wrap("book.xlsx", 1, true)));
    // Another file, so the folder isn't left empty (an emptied folder stops the next sync to ask).
    w.write("notes.txt", "stays");
    w.write("book.xlsx", "first");
    let r = w.sync().await;
    let events = w.engine.store().lock().run_events(&r.run_id).unwrap();
    let skip = events
        .iter()
        .find(|e| e.key.as_deref() == Some("book.xlsx") && e.action == "SKIP")
        .expect("a skip event for the file");
    assert!(skip.message.contains("Deleted on the device"), "{}", skip.message);
    let next = w.sync().await;
    assert_eq!(RunStatus::Success, next.status, "{:?}", next.message);
    assert_eq!(0, next.summary.unwrap().uploads, "nothing is uploaded again");
}

/// The owner's phone, 2026-10-29: a scheduled run stayed "Syncing" for two hours. Android stopped
/// the worker, which drops the engine's future mid-run; nothing closed the run or freed the pair,
/// so every later sync of it was skipped as already running until the app was killed.
#[tokio::test]
async fn a_run_whose_caller_gives_up_is_closed_and_frees_the_pair() {
    let w = World::with(
        |_| {},
        Some(Box::new(|m| {
            Arc::new(Slow {
                inner: m,
                delay: Duration::ZERO,
                hang_downloads: false,
                hang_listing: true,
            })
        })),
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(200), w.sync())
            .await
            .is_err(),
        "the run hangs while listing"
    );
    assert!(!w.engine.is_running(&w.pair_id), "the pair is free again");
    let runs = w.engine.store().lock().runs(Some(&w.pair_id), 10).unwrap();
    assert_eq!(1, runs.len());
    assert_eq!(RunStatus::Cancelled, runs[0].status);
    assert!(runs[0].ended_at.is_some(), "the run is closed, not left syncing");
}

#[tokio::test]
async fn stale_runs_from_a_killed_process_are_closed() {
    let w = World::new();
    w.engine
        .store()
        .lock()
        .start_run("ghost", &w.pair_id, "schedule", 1)
        .unwrap();
    assert_eq!(1, w.engine.recover().unwrap());
    assert_eq!(
        RunStatus::Cancelled,
        w.engine.store().lock().run("ghost").unwrap().unwrap().status
    );
}

// ---------------------------------------------------------------- conditions (FR-P4)

#[tokio::test]
async fn conditions_hold_scheduled_runs_but_not_manual_ones() {
    let w = World::with(
        |p| {
            p.conditions = Conditions {
                wifi_only: true,
                ..Conditions::none()
            }
        },
        None,
    );
    w.write("a.txt", "a");
    *w.env.device.lock().unwrap() = DeviceState {
        wifi: false,
        ..DeviceState::unrestricted()
    };
    let r = w.engine.sync(&w.pair_id, Trigger::Schedule).await;
    assert_eq!(
        (RunStatus::Skipped, Some(Blocker::WaitingForWifi)),
        (r.status, r.blocker)
    );
    assert_eq!(
        RunStatus::Skipped,
        w.engine.sync(&w.pair_id, Trigger::Automation).await.status,
        "automation is not manual"
    );
    assert_eq!(
        RunStatus::Success,
        w.sync().await.status,
        "Sync now overrides the pair's conditions"
    );

    *w.env.device.lock().unwrap() = DeviceState {
        online: false,
        ..DeviceState::unrestricted()
    };
    assert_eq!(
        Some(Blocker::Offline),
        w.sync().await.blocker,
        "but not a missing connection"
    );
}

#[tokio::test]
async fn a_pair_never_runs_twice_at_once() {
    let w = Arc::new(World::with(
        |_| {},
        Some(Box::new(|m| {
            Arc::new(Slow {
                inner: m,
                delay: Duration::from_millis(100),
                hang_downloads: false,
                hang_listing: false,
            })
        })),
    ));
    w.write("a.txt", "a");
    let a = {
        let w = w.clone();
        tokio::spawn(async move { w.sync().await })
    };
    tokio::time::sleep(Duration::from_millis(30)).await;
    let b = w.sync().await;
    assert_eq!(
        (RunStatus::Skipped, Some("Already running".to_owned())),
        (b.status, b.message)
    );
    assert_eq!(RunStatus::Success, a.await.unwrap().status);
}

/// #94: a run takes its pair's place before it reads the pair, and a folder change checks and
/// saves under the same lock. So the change can't slip in while a run holds the pair (the run
/// would sync the old folder, then clear the new folder's preview hold), and a run that starts
/// after it reads the moved pair and waits for its preview.
#[tokio::test]
async fn a_folder_change_is_refused_while_the_pair_runs_and_holds_the_next_run() {
    let w = Arc::new(World::with(
        |_| {},
        Some(Box::new(|m| {
            Arc::new(Slow {
                inner: m,
                delay: Duration::ZERO,
                hang_downloads: false,
                hang_listing: true,
            })
        })),
    ));
    let runner = {
        let w = w.clone();
        tokio::spawn(async move { w.sync().await })
    };
    while !w.engine.is_running(&w.pair_id) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut renamed = w.pair();
    renamed.name = "Renamed".into();
    assert!(
        !w.engine.update_pair(&renamed).unwrap(),
        "a new name is saved while the pair runs"
    );
    let mut moved = w.pair();
    moved.local_root = format!("{}-moved", moved.local_root);
    assert!(
        w.engine.update_pair(&moved).is_err(),
        "a new folder waits for the run"
    );
    assert_eq!(renamed.local_root, w.pair().local_root);

    w.engine.cancel(&w.pair_id);
    tokio::time::timeout(Duration::from_secs(5), runner)
        .await
        .expect("the run ends once stopped")
        .unwrap();
    assert!(
        w.engine.update_pair(&moved).unwrap(),
        "the pair moves once it is free"
    );
    assert!(w.pair().status.hold_for_preview);
    let r = w.sync().await;
    assert_eq!(RunStatus::Skipped, r.status, "the next run waits for its preview");
    assert!(
        w.pair().status.hold_for_preview,
        "nothing cleared the hold: {:?}",
        r.message
    );
}

// ---------------------------------------------------------------- the scheduler's own-write check

/// Issue #88: the scheduler drops a folder event whose path is as the last sync left it, so a
/// run's own download doesn't run the pair again, and an edit made after it does.
#[tokio::test]
async fn a_downloaded_file_is_as_last_synced_until_someone_edits_it() {
    let w = World::new();
    w.put_drive("a.txt", "from drive");
    w.put_drive("Q&A: what?.txt", "answer");
    assert_eq!(RunStatus::Success, w.sync().await.status);
    let id = w.pair_id.clone();
    assert!(w.engine.as_last_synced(&id, "a.txt"), "the run's own download");
    assert!(
        w.engine.as_last_synced(&id, "Q&A： what？.txt"),
        "by its local name"
    );
    assert!(
        w.engine.as_last_synced(&id, "a.txt.part"),
        "nothing where the baseline has nothing"
    );
    assert!(
        !w.engine.as_last_synced(&id, ""),
        "the folder itself names no change"
    );

    w.write("a.txt", "edited right after the download");
    assert!(
        !w.engine.as_last_synced(&id, "a.txt"),
        "an edit to the downloaded file"
    );
    w.write("new.txt", "new");
    assert!(!w.engine.as_last_synced(&id, "new.txt"), "a new file");
    w.remove("Q&A： what？.txt");
    assert!(
        !w.engine.as_last_synced(&id, "Q&A： what？.txt"),
        "a deleted file"
    );
}

/// A coarse file clock can give an edit made just after a run the time the run recorded, so a
/// file modified within 2 s of the last run is never taken as synced (git's racy-clean rule).
#[tokio::test]
async fn a_file_modified_near_the_last_run_is_never_as_last_synced() {
    let w = World::new();
    let now_s = w.clock.0.load(Ordering::SeqCst) / 1000;
    for (rel, secs) in [("fresh.txt", now_s), ("older.txt", now_s - 3600)] {
        w.write(rel, rel);
        filetime::set_file_mtime(w.path(rel), filetime::FileTime::from_unix_time(secs, 0)).unwrap();
    }
    assert_eq!(RunStatus::Success, w.sync().await.status);
    let id = w.pair_id.clone();
    assert!(
        !w.engine.as_last_synced(&id, "fresh.txt"),
        "modified as the run recorded it"
    );
    assert!(
        w.engine.as_last_synced(&id, "older.txt"),
        "modified an hour before"
    );
}

// ---------------------------------------------------------------- a file Android won't open (#127)

/// A folder-picker pair without All files access: Android refused to open the changed files by
/// their path, and the whole run failed as "Android removed access" with no file named.
#[tokio::test]
async fn a_changed_file_the_device_refuses_to_read_is_skipped_by_name_and_the_rest_syncs() {
    let w = World::new();
    w.write("todo.txt", "first");
    w.write("plan.md", "first");
    assert_eq!(RunStatus::Success, w.sync().await.status);

    w.write("todo.txt", "changed on the phone");
    w.write("plan.md", "changed on the phone too");
    let opens = Arc::new(SlowOpens::default());
    opens.denied.lock().unwrap().push("todo.txt".into());
    *w.env.slow_opens.lock().unwrap() = Some(opens.clone());

    let r = w.sync().await;
    // Not a silent Success: the run and the pair say a file was left out, and which.
    assert_eq!(RunStatus::Partial, r.status, "{:?}", r.message);
    let message = r.message.clone().unwrap_or_default();
    assert!(
        message.contains("Couldn't read todo.txt on the device") && message.contains("Permission denied"),
        "{message}"
    );
    let pair = w.pair();
    assert_eq!(Some(RunStatus::Partial), pair.status.last_status);
    assert_ne!(Some(ErrorKind::Permission), pair.status.last_error_kind);
    assert_eq!(
        "changed on the phone too",
        String::from_utf8(w.drive.content("Sync/plan.md").unwrap()).unwrap(),
        "the other file still syncs"
    );
    assert_eq!(
        "first",
        String::from_utf8(w.drive.content("Sync/todo.txt").unwrap()).unwrap(),
        "nothing happens to the unreadable file"
    );
    let events = w.engine.store().lock().run_events(&r.run_id).unwrap();
    assert!(
        events.iter().any(|e| e.level == wasl_store::EventLevel::Warn
            && e.message.contains("Couldn't read todo.txt on the device")
            && e.message.contains("Permission denied")),
        "the log names the file and why: {events:?}"
    );

    opens.denied.lock().unwrap().clear();
    let next = w.sync().await;
    assert_eq!(RunStatus::Success, next.status, "{:?}", next.message);
    assert_eq!(
        "changed on the phone",
        String::from_utf8(w.drive.content("Sync/todo.txt").unwrap()).unwrap()
    );
}

/// A file moved or deleted between the scan and the read (an app saving through a temp file and a
/// rename) failed the whole folder-picker pair with Android's raw error (#154). It is skipped by
/// name, the rest syncs, the run doesn't fail, and the next run picks it up.
#[tokio::test]
async fn a_file_that_vanishes_between_the_scan_and_the_read_is_skipped_not_failed() {
    let w = World::new();
    w.write("statements.xlsx", "first");
    w.write("plan.md", "first");
    assert_eq!(RunStatus::Success, w.sync().await.status);

    w.write("statements.xlsx", "saved again");
    w.write("plan.md", "changed");
    w.write("new.xlsx", "new");
    let opens = Arc::new(SlowOpens::default());
    opens.gone.lock().unwrap().push("statements.xlsx".into());
    opens.gone.lock().unwrap().push("new.xlsx".into());
    *w.env.slow_opens.lock().unwrap() = Some(opens.clone());

    let r = w.sync().await;
    assert_eq!(RunStatus::Success, r.status, "{:?}", r.message);
    assert_eq!(
        "changed",
        String::from_utf8(w.drive.content("Sync/plan.md").unwrap()).unwrap(),
        "the rest of the pair syncs"
    );
    let events = w.engine.store().lock().run_events(&r.run_id).unwrap();
    for name in ["statements.xlsx", "new.xlsx"] {
        assert!(
            events.iter().any(|e| e.level == wasl_store::EventLevel::Warn
                && e.message.contains(&format!("{name} was moved or deleted"))),
            "the log names {name} in plain words: {events:?}"
        );
    }
    assert!(
        !events.iter().any(|e| e.level == wasl_store::EventLevel::Error),
        "nothing is logged as an error: {events:?}"
    );

    opens.gone.lock().unwrap().clear();
    let next = w.sync().await;
    assert_eq!(RunStatus::Success, next.status, "{:?}", next.message);
    assert_eq!(
        "saved again",
        String::from_utf8(w.drive.content("Sync/statements.xlsx").unwrap()).unwrap(),
        "the next run picks it up"
    );
    assert_eq!(
        "new",
        String::from_utf8(w.drive.content("Sync/new.xlsx").unwrap()).unwrap()
    );
}

/// The pair's folder itself gone mid-run (a memory card taken out): every file reads as missing.
/// That fails the run with a clear message, not a Success with every file skipped (#154 review).
#[tokio::test]
async fn a_folder_gone_mid_run_fails_the_run_instead_of_skipping_every_file() {
    let w = World::new();
    w.write("a.txt", "first");
    assert_eq!(RunStatus::Success, w.sync().await.status);

    w.write("a.txt", "changed");
    w.write("b.txt", "new");
    let opens = Arc::new(SlowOpens::default());
    opens
        .gone
        .lock()
        .unwrap()
        .extend(["a.txt".to_owned(), "b.txt".to_owned()]);
    *opens.root_listings_left.lock().unwrap() = Some(1); // the scan's listing, then the card is out
    *w.env.slow_opens.lock().unwrap() = Some(opens.clone());

    let r = w.sync().await;
    assert_eq!(RunStatus::Failed, r.status, "{:?}", r.message);
    let message = r.message.unwrap_or_default();
    assert!(message.contains("isn't there any more"), "{message}");
}

#[tokio::test]
async fn a_new_file_the_device_refuses_to_read_fails_alone_not_as_lost_access() {
    let w = World::new();
    w.write("a.txt", "first");
    assert_eq!(RunStatus::Success, w.sync().await.status);

    w.write("new.txt", "made by another app");
    w.write("b.txt", "also new");
    let opens = Arc::new(SlowOpens::default());
    opens.denied.lock().unwrap().push("new.txt".into());
    *w.env.slow_opens.lock().unwrap() = Some(opens.clone());

    let r = w.sync().await;
    assert_eq!(RunStatus::Partial, r.status, "{:?}", r.message);
    let message = r.message.unwrap_or_default();
    assert!(message.contains("new.txt"), "{message}");
    assert_ne!(
        Some(ErrorKind::Permission),
        w.pair().status.last_error_kind,
        "{message}"
    );
    assert!(
        w.drive_files().contains_key("b.txt"),
        "the other file still uploads"
    );
}

/// #139: a Google Doc `Report` next to a real `Report.docx` ("Save as Google Docs" leaves both).
/// The export must never take the real file's local name, and nothing is uploaded from it.
#[tokio::test]
async fn a_google_doc_next_to_a_same_named_docx_never_replaces_the_real_file() {
    let w = World::new();
    w.put_drive("Report.docx", "real docx bytes");
    w.drive.put_google_doc("Sync/Report", GOOGLE_DOC);
    let first = w.sync_from_preview().await;
    assert_eq!(RunStatus::Success, first.status, "{:?}", first.message);
    for _ in 0..3 {
        let r = w.sync().await;
        assert_eq!(RunStatus::Success, r.status, "{:?}", r.message);
    }
    assert_eq!(
        Some("real docx bytes".to_owned()),
        w.drive_files().get("Report.docx").cloned(),
        "the real Report.docx on Drive is never replaced by the export"
    );
    assert_eq!(Some("real docx bytes".to_owned()), w.read("Report.docx"));
    let exports: Vec<String> = w
        .local_files()
        .into_keys()
        .filter(|p| p.starts_with("Report") && p != "Report.docx")
        .collect();
    assert_eq!(
        1,
        exports.len(),
        "the export is kept under its own name: {exports:?}"
    );
}

/// #142: a folder trashed on Drive while the device added a file in it.
#[tokio::test]
async fn a_folder_deleted_on_drive_with_a_new_file_here_is_recreated_and_settles() {
    let w = World::new();
    w.put_drive("D/a.txt", "a");
    w.put_drive("other.txt", "o");
    assert_eq!(RunStatus::Success, w.sync_from_preview().await.status);
    w.write("D/new.txt", "only here");
    assert!(w.drive.trash_path("Sync/D"));
    let r = w.sync().await;
    assert_eq!(RunStatus::Success, r.status, "{:?}", r.message);
    assert_eq!(Some("only here".to_owned()), w.read("D/new.txt"));
    assert_eq!(
        Some("only here".to_owned()),
        w.drive_files().get("D/new.txt").cloned(),
        "the new file reaches Drive"
    );
    assert_eq!(
        None,
        w.read("D/a.txt"),
        "the synced file follows Drive's deletion"
    );
    assert!(
        !w.drive_files().contains_key("D/a.txt"),
        "and is not uploaded again"
    );
    let r = w.sync().await;
    assert_eq!(RunStatus::Success, r.status, "the pair settles: {:?}", r.message);
}

/// #142, nested: the new file is two folders down.
#[tokio::test]
async fn a_folder_deleted_on_drive_with_a_new_file_deeper_here_is_recreated_to_that_depth() {
    let w = World::new();
    w.put_drive("D/a.txt", "a");
    w.put_drive("D/sub/b.txt", "b");
    w.put_drive("other.txt", "o");
    assert_eq!(RunStatus::Success, w.sync_from_preview().await.status);
    w.write("D/sub/new.txt", "only here");
    assert!(w.drive.trash_path("Sync/D"));
    let r = w.sync().await;
    assert_eq!(RunStatus::Success, r.status, "{:?}", r.message);
    let drive = w.drive_files();
    assert_eq!(Some("only here"), drive.get("D/sub/new.txt").map(String::as_str));
    assert!(
        !drive.contains_key("D/a.txt") && !drive.contains_key("D/sub/b.txt"),
        "{drive:?}"
    );
    assert_eq!((None, None), (w.read("D/a.txt"), w.read("D/sub/b.txt")));
    let r = w.sync().await;
    assert_eq!(RunStatus::Success, r.status, "the pair settles: {:?}", r.message);
}

/// #142, edited: a synced file edited here while its folder is trashed on Drive.
#[tokio::test]
async fn a_folder_deleted_on_drive_with_a_file_edited_here_is_recreated_with_the_edit() {
    let w = World::new();
    w.put_drive("D/a.txt", "a");
    w.put_drive("D/b.txt", "b");
    w.put_drive("other.txt", "o");
    assert_eq!(RunStatus::Success, w.sync_from_preview().await.status);
    w.write("D/a.txt", "edited here");
    assert!(w.drive.trash_path("Sync/D"));
    let r = w.sync().await;
    assert_eq!(RunStatus::Success, r.status, "{:?}", r.message);
    assert_eq!(
        Some("edited here"),
        w.drive_files().get("D/a.txt").map(String::as_str)
    );
    assert_eq!(
        None,
        w.read("D/b.txt"),
        "the unedited file follows Drive's deletion"
    );
    let r = w.sync().await;
    assert_eq!(RunStatus::Success, r.status, "the pair settles: {:?}", r.message);
}

/// #139, the other order: the export already holds `Report.docx` when a real `Report.docx` appears
/// on Drive. The real file takes the name; the export moves to its own.
#[tokio::test]
async fn a_real_docx_arriving_next_to_an_exported_doc_takes_its_name() {
    let w = World::new();
    w.drive.put_google_doc("Sync/Report", GOOGLE_DOC);
    w.put_drive("other.txt", "o");
    assert_eq!(RunStatus::Success, w.sync_from_preview().await.status);
    assert!(w.read("Report.docx").is_some(), "exported under its plain name");
    w.put_drive("Report.docx", "real docx bytes");
    for _ in 0..3 {
        let r = w.sync().await;
        assert_eq!(RunStatus::Success, r.status, "{:?}", r.message);
    }
    assert_eq!(
        Some("real docx bytes".to_owned()),
        w.drive_files().get("Report.docx").cloned(),
        "the real Report.docx on Drive is never replaced by the export"
    );
    assert_eq!(Some("real docx bytes".to_owned()), w.read("Report.docx"));
    assert!(
        w.read("Report (Google Doc).docx").is_some(),
        "{:?}",
        w.local_files().keys()
    );
}

/// #139, a file only on the device: a never-synced `Report.docx` here keeps its name and uploads;
/// the export of the Doc `Report` takes its own name and is never uploaded.
#[tokio::test]
async fn a_never_synced_docx_here_next_to_a_google_doc_keeps_its_name() {
    let w = World::new();
    w.write("Report.docx", "my real file");
    w.drive.put_google_doc("Sync/Report", GOOGLE_DOC);
    for i in 0..3 {
        let r = if i == 0 {
            w.sync_from_preview().await
        } else {
            w.sync().await
        };
        assert_eq!(RunStatus::Success, r.status, "{:?}", r.message);
    }
    assert_eq!(Some("my real file".to_owned()), w.read("Report.docx"));
    let drive = w.drive_files();
    assert_eq!(Some("my real file"), drive.get("Report.docx").map(String::as_str));
    assert!(
        drive.keys().all(|k| k == "Report.docx" || k == "Report"),
        "nothing from the export is uploaded: {drive:?}"
    );
    assert!(
        w.read("Report (Google Doc).docx").is_some(),
        "{:?}",
        w.local_files().keys()
    );
}

/// #139: the fallback name is checked too; a real `Report (Google Doc).docx` is never overwritten.
#[tokio::test]
async fn the_fallback_export_name_is_never_a_real_files() {
    let w = World::new();
    w.put_drive("Report.docx", "real docx bytes");
    w.put_drive("Report (Google Doc).docx", "another real file");
    w.drive.put_google_doc("Sync/Report", GOOGLE_DOC);
    for _ in 0..3 {
        let r = w.sync().await;
        assert_eq!(RunStatus::Success, r.status, "{:?}", r.message);
    }
    let drive = w.drive_files();
    assert_eq!(
        Some("real docx bytes"),
        drive.get("Report.docx").map(String::as_str)
    );
    assert_eq!(
        Some("another real file"),
        drive.get("Report (Google Doc).docx").map(String::as_str)
    );
    assert_eq!(
        Some("another real file".to_owned()),
        w.read("Report (Google Doc).docx")
    );
    assert!(
        w.read("Report (Google Doc 2).docx").is_some(),
        "{:?}",
        w.local_files().keys()
    );
}

/// #139: when the real `Report.docx` is deleted on Drive later, the export keeps its own name and
/// is not uploaded in the real file's place.
#[tokio::test]
async fn when_the_real_docx_goes_the_export_stays_under_its_own_name() {
    let w = World::new();
    w.put_drive("Report.docx", "real docx bytes");
    w.put_drive("other.txt", "o");
    w.drive.put_google_doc("Sync/Report", GOOGLE_DOC);
    assert_eq!(RunStatus::Success, w.sync_from_preview().await.status);
    assert!(w.drive.trash_path("Sync/Report.docx"));
    for _ in 0..3 {
        let r = w.sync().await;
        assert_eq!(RunStatus::Success, r.status, "{:?}", r.message);
    }
    assert_eq!(None, w.read("Report.docx"));
    assert!(
        w.read("Report (Google Doc).docx").is_some(),
        "{:?}",
        w.local_files().keys()
    );
    assert!(
        !w.drive_files().contains_key("Report.docx"),
        "{:?}",
        w.drive_files()
    );
}

fn export_row(key: &str, local_rel: &str, hash: &str) -> wasl_core::BaselineEntry {
    wasl_core::BaselineEntry {
        key: key.to_owned(),
        local_rel: local_rel.to_owned(),
        is_dir: false,
        local_size: 999,
        local_mtime: 1,
        local_hash: Some(hash.to_owned()),
        remote_id: None,
        remote_md5: None,
        remote_size: None,
        remote_mtime: None,
        remote_version: None,
        export_mime: Some(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document".to_owned(),
        ),
    }
}

/// #139, a pair from 0.4.x: an export row holds `Report.docx` while the real file is `report.docx`.
/// On a case-insensitive folder they are one file; moving the export never bins the real one.
#[tokio::test]
async fn an_old_export_row_on_a_real_files_name_in_another_case_never_bins_it() {
    let w = World::new();
    w.put_drive("report.docx", "real docx bytes");
    w.put_drive("other.txt", "o");
    assert_eq!(RunStatus::Success, w.sync_from_preview().await.status);
    w.drive.put_google_doc("Sync/Report", GOOGLE_DOC);
    w.engine
        .store()
        .lock()
        .put_baseline(PAIR, &export_row("Report", "Report.docx", "old export"))
        .unwrap();
    for _ in 0..3 {
        let r = w.sync().await;
        assert_eq!(RunStatus::Success, r.status, "{:?}", r.message);
    }
    assert_eq!(Some("real docx bytes".to_owned()), w.read("report.docx"));
    assert_eq!(
        Some("real docx bytes"),
        w.drive_files().get("report.docx").map(String::as_str)
    );
}

/// #139, a pair from 0.4.x where the export already overwrote the real file: the real row holds
/// the export's bytes. The run says so, so the user can restore it from Drive's version history.
#[tokio::test]
async fn export_bytes_under_a_real_files_row_are_reported() {
    let w = World::new();
    w.put_drive("Report.docx", "real docx bytes");
    w.put_drive("other.txt", "o");
    assert_eq!(RunStatus::Success, w.sync_from_preview().await.status);
    let real = w
        .engine
        .store()
        .lock()
        .baseline_entry(PAIR, "Report.docx")
        .unwrap()
        .unwrap();
    w.drive.put_google_doc("Sync/Report", GOOGLE_DOC);
    let hash = real.local_hash.clone().unwrap();
    w.engine
        .store()
        .lock()
        .put_baseline(PAIR, &export_row("Report", "Report.docx", &hash))
        .unwrap();
    let r = w.sync().await;
    let warnings = r.summary.map(|s| s.warnings).unwrap_or_default();
    assert!(
        warnings
            .iter()
            .any(|n| n.contains("'Report.docx'") && n.contains("version history")),
        "{warnings:?}"
    );
    assert_eq!(Some("real docx bytes".to_owned()), w.read("Report.docx"));
}

/// Security review M1 (#134): the engine refuses to point a pair at other files while an agent's
/// changeset on it is open or held, under the lock it saves with, with the store's refusal (sent
/// to clients as `Invalid`). A new name is still saved.
#[tokio::test]
async fn a_pair_with_agent_changes_waiting_is_not_repointed() {
    let w = World::new();
    w.engine
        .store()
        .lock()
        .connection()
        .execute(
            "INSERT INTO agent_changesets (id, pair_id, actor, scope, status, started_at)
             VALUES ('cs', ?1, 'agent:claude-code', '', 'held', 1)",
            [&w.pair_id],
        )
        .unwrap();
    let mut moved = w.pair();
    moved.local_root = format!("{}-moved", moved.local_root);
    let e = w.engine.update_pair(&moved).unwrap_err();
    assert!(
        matches!(
            e,
            wasl_engine::EngineError::Store(wasl_store::StoreError::Refused(_))
        ),
        "{e}"
    );
    assert_ne!(moved.local_root, w.pair().local_root);
    let mut renamed = w.pair();
    renamed.name = "Renamed".into();
    assert!(!w.engine.update_pair(&renamed).unwrap());
}

/// #139 under ADR-0004: a Google Doc `été` and a real `Été.docx` are one name on a case-insensitive
/// folder (non-ASCII case). The export takes its own name.
#[tokio::test]
async fn a_google_doc_whose_export_differs_from_a_real_file_only_in_non_ascii_case_gets_its_own_name() {
    let w = World::new();
    w.put_drive("Été.docx", "real docx bytes");
    w.drive.put_google_doc("Sync/été", GOOGLE_DOC);
    for i in 0..3 {
        let r = if i == 0 {
            w.sync_from_preview().await
        } else {
            w.sync().await
        };
        assert_eq!(RunStatus::Success, r.status, "{:?}", r.message);
    }
    assert_eq!(
        Some("real docx bytes"),
        w.drive_files().get("Été.docx").map(String::as_str)
    );
    assert_eq!(Some("real docx bytes".to_owned()), w.read("Été.docx"));
    assert!(
        w.read("été (Google Doc).docx").is_some(),
        "{:?}",
        w.local_files().keys()
    );
}

/// #139 under ADR-0004: a real `Résumé.docx` in NFD and a Google Doc `Résumé` in NFC are one name.
#[tokio::test]
async fn a_google_doc_whose_export_differs_from_a_real_file_only_in_normalisation_gets_its_own_name() {
    let nfd = "Re\u{301}sume\u{301}.docx";
    let w = World::new();
    w.put_drive(nfd, "real docx bytes");
    w.drive.put_google_doc("Sync/R\u{e9}sum\u{e9}", GOOGLE_DOC);
    for i in 0..3 {
        let r = if i == 0 {
            w.sync_from_preview().await
        } else {
            w.sync().await
        };
        assert_eq!(RunStatus::Success, r.status, "{:?}", r.message);
    }
    let drive = w.drive_files();
    assert_eq!(
        Some("real docx bytes"),
        drive.get(nfd).map(String::as_str),
        "{drive:?}"
    );
    let key = wasl_core::identity::name_key;
    let local = w.local_files();
    let real = local
        .iter()
        .find(|(p, _)| key(p) == key(nfd))
        .map(|(_, c)| c.as_str());
    assert_eq!(Some("real docx bytes"), real, "{local:?}");
    assert!(
        local.keys().any(|p| key(p) == key("Résumé (Google Doc).docx")),
        "{local:?}"
    );
}
