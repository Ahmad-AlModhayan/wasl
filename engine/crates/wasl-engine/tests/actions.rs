//! What the user does after a run: review and restore a stopped deletion, settle conflicts, restore
//! versions, work the recycle bin.

mod common;

use common::{World, map};
use wasl_core::RunStatus;
use wasl_engine::{ConflictChoice, VersionRef, VersionSource};
use wasl_store::BinSide;

async fn synced_world(files: &[(&str, &str)]) -> World {
    let w = World::new();
    for (k, v) in files {
        w.write(k, v);
    }
    assert_eq!(RunStatus::Success, w.sync().await.status);
    w
}

// ---------------------------------------------------------------- stopped deletions

#[tokio::test]
async fn a_stopped_deletion_can_be_reviewed_and_restored_on_both_sides() {
    let mut files: Vec<(String, String)> = (0..12)
        .map(|i| (format!("docs/f{i:02}.txt"), format!("doc {i}")))
        .collect();
    files.extend((0..12).map(|i| (format!("pics/p{i:02}.jpg"), format!("pic {i}"))));
    files.push(("keep.txt".into(), "k".into()));
    let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let w = synced_world(&refs).await;

    // A script wipes docs/ on the device and someone empties pics/ on Drive.
    std::fs::remove_dir_all(w.path("docs")).unwrap();
    w.drive.trash_path("Sync/pics");
    assert_eq!(RunStatus::Blocked, w.sync().await.status);

    let review = w.engine.deletion_review(&w.pair_id).await.unwrap();
    assert_eq!((12, 12), (review.files_on_drive, review.files_on_device));
    let folders: Vec<(&str, bool, u32)> = review
        .items
        .iter()
        .filter(|i| i.is_dir)
        .map(|i| (i.key.as_str(), i.on_drive, i.files))
        .collect();
    assert_eq!(
        vec![("docs", true, 12), ("pics", false, 12)],
        folders,
        "folders first, with what is inside"
    );
    assert!(
        review.items.iter().all(|i| i.is_dir),
        "files inside a deleted folder are not listed twice"
    );
    assert_eq!(
        review.bytes,
        files
            .iter()
            .filter(|(k, _)| k != "keep.txt")
            .map(|(_, v)| v.len() as u64)
            .sum::<u64>()
    );

    assert_eq!(24, w.engine.restore_deletions(&w.pair_id).await.unwrap());
    let all: std::collections::BTreeMap<String, String> = files.iter().cloned().collect();
    assert_eq!(all, w.local_files());
    assert_eq!(all, w.drive_files());
    assert!(!w.pair().status.pending_approval);
    assert!(
        w.engine
            .store()
            .lock()
            .blocked_plan(&w.pair_id)
            .unwrap()
            .is_none()
    );
    assert!(w.sync().await.summary.unwrap().is_empty(), "nothing left to do");
}

// ---------------------------------------------------------------- conflicts

async fn conflicted() -> (World, String, String) {
    let w = synced_world(&[("notes.txt", "base")]).await;
    w.write("notes.txt", "device edit");
    w.put_drive("notes.txt", "drive edit");
    assert_eq!(RunStatus::Success, w.sync().await.status);
    let c = w
        .engine
        .store()
        .lock()
        .open_conflicts(Some(&w.pair_id))
        .unwrap()
        .remove(0);
    let copy = c.local_copy_rel.clone().unwrap();
    (w, c.id, copy)
}

#[tokio::test]
async fn conflict_details_show_both_sides() {
    let (w, id, _) = conflicted().await;
    let d = w.engine.conflict_details(&id).await.unwrap().unwrap();
    assert_eq!(("notes.txt", ""), (d.name.as_str(), d.folder.as_str()));
    assert_eq!(Some("device edit".len() as u64), d.device.map(|s| s.size));
    assert_eq!(Some("drive edit".len() as u64), d.drive.map(|s| s.size));
}

#[tokio::test]
async fn keep_drive_bins_the_device_copy_and_undo_brings_it_back() {
    let (w, id, copy) = conflicted().await;
    let undo = w
        .engine
        .resolve_conflict(&id, ConflictChoice::KeepDrive)
        .await
        .unwrap();
    assert_eq!(None, w.read(&copy));
    assert!(w.engine.store().lock().open_conflicts(None).unwrap().is_empty());
    w.sync().await;
    assert_eq!(
        map(&[("notes.txt", "drive edit")]),
        w.drive_files(),
        "the choice reaches Drive"
    );

    w.engine.undo_conflict(&undo).await.unwrap();
    assert_eq!(Some("device edit".into()), w.read(&copy));
    assert_eq!(1, w.engine.store().lock().open_conflicts(None).unwrap().len());
}

#[tokio::test]
async fn keep_device_puts_the_device_version_back_under_the_original_name() {
    let (w, id, copy) = conflicted().await;
    let undo = w
        .engine
        .resolve_conflict(&id, ConflictChoice::KeepDevice)
        .await
        .unwrap();
    assert_eq!(map(&[("notes.txt", "device edit")]), w.local_files());
    w.sync().await;
    assert_eq!(map(&[("notes.txt", "device edit")]), w.drive_files());

    w.engine.undo_conflict(&undo).await.unwrap();
    assert_eq!(
        Some("device edit".into()),
        w.read(&copy),
        "the rename is reversed"
    );
    assert_eq!(
        Some("drive edit".into()),
        w.read("notes.txt"),
        "and the binned file is back"
    );
}

#[tokio::test]
async fn keep_both_only_closes_the_conflict() {
    let (w, id, copy) = conflicted().await;
    w.engine
        .resolve_conflict(&id, ConflictChoice::KeepBoth)
        .await
        .unwrap();
    assert_eq!(Some("device edit".into()), w.read(&copy));
    assert_eq!(Some("drive edit".into()), w.read("notes.txt"));
    assert!(w.engine.store().lock().open_conflicts(None).unwrap().is_empty());
}

// ---------------------------------------------------------------- versions

#[tokio::test]
async fn versions_merge_drive_revisions_and_local_copies_and_restore() {
    let w = synced_world(&[("a.txt", "v1 device")]).await;
    w.put_drive("a.txt", "v2 drive");
    w.sync().await; // the device's v1 goes to the local bin
    w.put_drive("a.txt", "v3 drive");
    w.sync().await;

    let v = w.engine.file_versions(&w.pair_id, "a.txt").await.unwrap();
    assert_eq!(
        ("a.txt", "My Drive/Sync/a.txt", "Sync"),
        (v.name.as_str(), v.path.as_str(), v.pair_name.as_str())
    );
    let drive: Vec<&wasl_engine::VersionEntry> = v
        .entries
        .iter()
        .filter(|e| matches!(e.version, VersionRef::Drive { .. }))
        .collect();
    assert_eq!(3, drive.len());
    assert_eq!(1, v.entries.iter().filter(|e| e.current).count());
    assert!(v.entries.iter().find(|e| e.current).unwrap().at >= drive.iter().map(|e| e.at).max().unwrap());
    assert!(
        drive.iter().any(|e| e.source == VersionSource::Device),
        "v1 was uploaded from this device"
    );
    assert!(
        v.entries
            .iter()
            .any(|e| matches!(e.version, VersionRef::Local { .. })),
        "replaced local copies are listed"
    );

    // Restore the oldest Drive revision: current on the device now, on Drive after a sync.
    // The revision this device uploaded is v1.
    let oldest = drive
        .iter()
        .find(|e| e.source == VersionSource::Device)
        .unwrap()
        .version
        .clone();
    w.engine
        .restore_version(&w.pair_id, "a.txt", &oldest)
        .await
        .unwrap();
    assert_eq!(Some("v1 device".into()), w.read("a.txt"));
    assert_eq!(RunStatus::Success, w.sync().await.status);
    assert_eq!("v1 device", w.drive_files()["a.txt"]);

    // Save a copy of a revision beside the file.
    let copy = w
        .engine
        .save_version_copy(&w.pair_id, "a.txt", &oldest, 1_760_000_000_000)
        .await
        .unwrap();
    assert!(
        copy.starts_with("a (version ") && copy.ends_with(").txt"),
        "{copy}"
    );
    assert_eq!(Some("v1 device".into()), w.read(&copy));

    // Keep forever.
    let rev = match &oldest {
        VersionRef::Drive { revision_id } => revision_id.clone(),
        _ => unreachable!(),
    };
    w.engine
        .keep_revision(&w.pair_id, "a.txt", &rev, true)
        .await
        .unwrap();
    let again = w.engine.file_versions(&w.pair_id, "a.txt").await.unwrap();
    assert!(again.entries.iter().any(|e| e.keep_forever));
}

// ---------------------------------------------------------------- recycle bin

#[tokio::test]
async fn restoring_over_a_newer_file_bins_the_newer_one_first() {
    let w = synced_world(&[("a.txt", "old")]).await;
    w.put_drive("a.txt", "new");
    w.sync().await; // "old" is in the bin
    let old = w.engine.store().lock().bin_entries(None).unwrap().remove(0);
    let r = w
        .engine
        .restore_bin_entries(std::slice::from_ref(&old.entry.id))
        .await
        .unwrap();
    assert_eq!((1, 0), (r.restored.len(), r.failed.len()));
    assert_eq!(Some("old".into()), w.read("a.txt"));
    let bin = w.engine.store().lock().bin_entries(None).unwrap();
    assert_eq!(1, bin.len(), "the newer file took its place in the bin");

    // Undo puts "old" back in the bin and "new" stays in the bin too.
    w.engine.undo_bin_restore(&r.restored).await.unwrap();
    assert_eq!(None, w.read("a.txt"));
    assert_eq!(2, w.engine.store().lock().bin_entries(None).unwrap().len());
}

#[tokio::test]
async fn delete_forever_and_empty_the_bin_on_both_sides() {
    let w = synced_world(&[("a.txt", "a"), ("b.txt", "b")]).await;
    w.remove("a.txt");
    w.drive.trash_path("Sync/b.txt");
    w.sync().await;
    let bin = w.engine.store().lock().bin_entries(None).unwrap();
    assert_eq!(2, bin.len());
    let drive_entry = bin
        .iter()
        .find(|b| b.entry.side == BinSide::Drive)
        .unwrap()
        .clone();
    let drive_id = drive_entry.entry.remote_file_id.clone().unwrap();
    assert_eq!(
        1,
        w.engine
            .delete_bin_forever(std::slice::from_ref(&drive_entry.entry.id))
            .await
            .unwrap()
    );
    assert!(
        !w.drive.is_trashed(&drive_id) && w.drive.id_of("Sync/a.txt").is_none(),
        "gone from Drive's bin"
    );
    assert_eq!(1, w.engine.empty_bin().await.unwrap());
    assert!(w.engine.store().lock().bin_entries(None).unwrap().is_empty());
    assert_eq!(
        0,
        std::fs::read_dir(&w.env.bin).map_or(0, |d| d.count()),
        "local bin files deleted"
    );
}
