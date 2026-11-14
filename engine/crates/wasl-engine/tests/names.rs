//! Name identity (docs/adr/0004-name-identity.md): names that differ only in case or in Unicode
//! normalisation are one name. Regression tests for #137, #138, #140 and #143. The device folders
//! fold names as Windows, macOS and Android do on any host (`Folding`), so Linux CI runs them too.
mod common;
use common::*;
use wasl_core::RunStatus;

const FOLDED: Option<Folding> = Some(Folding { android: false });
const PHONE: Option<Folding> = Some(Folding { android: true });

fn bin_is_empty(w: &World) -> bool {
    std::fs::read_dir(&w.env.bin).map_or(true, |mut d| d.next().is_none())
}

/// The device's spelling of the folder `name` (it may differ from Drive's by case or accents).
fn local_dir(w: &World, name: &str) -> String {
    std::fs::read_dir(&w.env.root)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .find(|n| wasl_core::identity::same_name(n, name))
        .unwrap_or_else(|| panic!("no folder {name} here"))
}

fn folded_world() -> World {
    let w = World::new();
    *w.env.folding.lock().unwrap() = FOLDED;
    w
}

/// #138: a case-only rename on Drive renames the device's file; nothing is binned or trashed.
#[tokio::test]
async fn case_only_rename_on_drive_renames_the_file_here() {
    let w = folded_world();
    w.put_drive("Foo.txt", "precious");
    w.put_drive("other.txt", "o");
    assert_eq!(RunStatus::Success, w.sync_from_preview().await.status);
    assert!(w.drive.move_path("Sync/Foo.txt", "Sync/foo.txt"));
    for _ in 0..3 {
        let o = w.sync().await;
        assert_eq!(RunStatus::Success, o.status, "{:?}", o.message);
    }
    assert_eq!(
        map(&[("foo.txt", "precious"), ("other.txt", "o")]),
        w.drive_files()
    );
    assert_eq!(
        map(&[("foo.txt", "precious"), ("other.txt", "o")]),
        w.local_files()
    );
}

/// #138, when the device's copy changed too: the rename cannot be followed, but the file is still
/// one file under Drive's name, and the edit reaches Drive.
#[tokio::test]
async fn case_only_rename_on_drive_of_a_file_edited_here_keeps_the_edit() {
    let w = folded_world();
    w.put_drive("Foo.txt", "v1");
    w.put_drive("other.txt", "o");
    assert_eq!(RunStatus::Success, w.sync_from_preview().await.status);
    assert!(w.drive.move_path("Sync/Foo.txt", "Sync/foo.txt"));
    w.clock.advance(10_000);
    w.write("Foo.txt", "v2, edited here");
    for _ in 0..3 {
        let o = w.sync().await;
        assert_eq!(RunStatus::Success, o.status, "{:?}", o.message);
    }
    assert_eq!(
        map(&[("foo.txt", "v2, edited here"), ("other.txt", "o")]),
        w.drive_files()
    );
    // The device keeps its spelling for the key Drive renamed; nothing went to the bin.
    assert_eq!(
        map(&[("Foo.txt", "v2, edited here"), ("other.txt", "o")]),
        w.local_files()
    );
    assert!(bin_is_empty(&w));
}

/// #140: a case variant added on Drive next to a synced file is reported; nothing is deleted on
/// either side.
#[tokio::test]
async fn case_variant_added_on_drive_is_reported_and_nothing_is_deleted() {
    let w = folded_world();
    w.put_drive("Report.txt", "mine");
    w.put_drive("other.txt", "o");
    assert_eq!(RunStatus::Success, w.sync_from_preview().await.status);
    w.clock.advance(10_000);
    w.put_drive("report.txt", "theirs");
    let notes = w.engine.preview(&w.pair_id).await.unwrap().notes;
    assert!(
        notes
            .iter()
            .any(|n| n.contains("same name twice") && n.contains("eport.txt")),
        "the clash is reported: {notes:?}"
    );
    for _ in 0..3 {
        let o = w.sync().await;
        assert_eq!(RunStatus::Success, o.status, "{:?}", o.message);
    }
    assert_eq!(
        map(&[
            ("Report.txt", "mine"),
            ("other.txt", "o"),
            ("report.txt", "theirs")
        ]),
        w.drive_files()
    );
    assert_eq!(
        map(&[("Report.txt", "mine"), ("other.txt", "o")]),
        w.local_files()
    );
}

/// #137: a case-only rename on the PC reaches a phone (casefolded storage) as one file with the
/// new name, never as `Meeting (1).txt`.
#[tokio::test]
async fn case_only_rename_on_a_device_reaches_the_phone_as_one_file() {
    let pc = folded_world();
    pc.put_drive("meeting.txt", "notes");
    pc.put_drive("other.txt", "o");
    assert_eq!(RunStatus::Success, pc.sync_from_preview().await.status);
    let phone = pc.another_device(PHONE);
    assert_eq!(RunStatus::Success, phone.sync_from_preview().await.status);
    assert_eq!(
        map(&[("meeting.txt", "notes"), ("other.txt", "o")]),
        phone.local_files()
    );

    // Two steps through a temporary name, as Explorer does.
    std::fs::rename(pc.path("meeting.txt"), pc.path("tmp.txt")).unwrap();
    std::fs::rename(pc.path("tmp.txt"), pc.path("Meeting.txt")).unwrap();
    for w in [&pc, &phone, &pc, &phone] {
        let o = w.sync().await;
        assert_eq!(RunStatus::Success, o.status, "{:?}", o.message);
    }
    let want = map(&[("Meeting.txt", "notes"), ("other.txt", "o")]);
    assert_eq!(want, pc.drive_files());
    assert_eq!(want, pc.local_files());
    assert_eq!(want, phone.local_files());
}

// NFC: U+0623 (alef with hamza above); NFD: U+0627 U+0654. Latin: é as U+00E9 and as e + U+0301.
const AHMAD_NFC: &str = "\u{0623}\u{062D}\u{0645}\u{062F}.txt";
const AHMAD_NFD: &str = "\u{0627}\u{0654}\u{062D}\u{0645}\u{062F}.txt";
const CAFE_NFC: &str = "caf\u{00E9}.txt";
const CAFE_NFD: &str = "cafe\u{0301}.txt";

async fn nfd_here_nfc_on_drive(w: World) {
    w.put_drive(AHMAD_NFC, "same");
    w.write(AHMAD_NFD, "same");
    w.put_drive(CAFE_NFC, "café");
    w.write(CAFE_NFD, "café");
    for _ in 0..2 {
        let o = w.sync_from_preview().await;
        assert_eq!(RunStatus::Success, o.status, "{:?}", o.message);
    }
    assert_eq!(map(&[(AHMAD_NFC, "same"), (CAFE_NFC, "café")]), w.drive_files());
    assert_eq!(2, w.local_files().len(), "{:?}", w.local_files());
}

/// #143: one name in NFD on the device and NFC on Drive is one file, on a folder that keeps both
/// spellings apart (NTFS, Linux)…
#[tokio::test]
async fn nfc_and_nfd_spellings_are_one_file() {
    nfd_here_nfc_on_drive(World::new()).await;
}

/// …and on one that folds them (APFS), where two keys would be one file deleted after download.
#[tokio::test]
async fn nfc_and_nfd_spellings_are_one_file_on_a_folding_folder() {
    nfd_here_nfc_on_drive(folded_world()).await;
}

/// A case-sensitive device (Linux) holding two spellings of one name uploads neither: Drive and the
/// other devices could not keep them apart. Both stay, reported.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn two_spellings_on_a_case_sensitive_device_are_reported_not_uploaded() {
    let w = World::new();
    w.write("Notes.txt", "a");
    w.write("notes.txt", "b");
    w.write("other.txt", "o");
    let notes = w.engine.preview(&w.pair_id).await.unwrap().notes;
    assert!(notes.iter().any(|n| n.contains("same name twice")), "{notes:?}");
    w.sync_from_preview().await;
    assert_eq!(map(&[("other.txt", "o")]), w.drive_files());
    assert_eq!(3, w.local_files().len());
}

/// A folder Drive respelled by case only: new files and folders made here inside it upload into it
/// (they used to fail every run with "parent folder … is missing on Drive"), and no empty old
/// folder is left beside it on a device that keeps both spellings apart.
async fn respelled_folder_takes_new_files(w: World) {
    w.put_drive("Docs/a.txt", "a");
    w.put_drive("other.txt", "o");
    assert_eq!(RunStatus::Success, w.sync_from_preview().await.status);
    assert!(w.drive.move_path("Sync/Docs", "Sync/docs"));
    let o = w.sync().await;
    assert_eq!(RunStatus::Success, o.status, "{:?}", o.message);
    let dir = local_dir(&w, "docs");
    w.write(&format!("{dir}/b.txt"), "b");
    w.write(&format!("{dir}/sub/c.txt"), "c");
    for _ in 0..2 {
        let o = w.sync().await;
        assert_eq!(RunStatus::Success, o.status, "{:?}", o.message);
    }
    assert_eq!(
        map(&[
            ("docs/a.txt", "a"),
            ("docs/b.txt", "b"),
            ("docs/sub/c.txt", "c"),
            ("other.txt", "o")
        ]),
        w.drive_files()
    );
    assert_eq!(4, w.local_files().len(), "{:?}", w.local_files());
    let notes = w.engine.preview(&w.pair_id).await.unwrap().notes;
    assert!(!notes.iter().any(|n| n.contains("same name twice")), "{notes:?}");
}

#[tokio::test]
async fn folder_respelled_on_drive_takes_new_files() {
    respelled_folder_takes_new_files(World::new()).await;
}

#[tokio::test]
async fn folder_respelled_on_drive_takes_new_files_on_a_folding_folder() {
    respelled_folder_takes_new_files(folded_world()).await;
}

/// #143 for a folder: an NFD folder here meets its NFC spelling on Drive; a file added in it later
/// uploads into Drive's folder.
#[tokio::test]
async fn nfd_folder_meeting_its_nfc_spelling_takes_new_files() {
    let w = World::new();
    w.put_drive("caf\u{e9}/a.txt", "a");
    w.write("cafe\u{301}/a.txt", "a");
    assert_eq!(RunStatus::Success, w.sync_from_preview().await.status);
    w.write("cafe\u{301}/b.txt", "b");
    for _ in 0..2 {
        let o = w.sync().await;
        assert_eq!(RunStatus::Success, o.status, "{:?}", o.message);
    }
    assert_eq!(
        map(&[("caf\u{e9}/a.txt", "a"), ("caf\u{e9}/b.txt", "b")]),
        w.drive_files()
    );
}

/// A rename here while Drive edited the old name is not a clash: Drive's edit reaches the device
/// and nothing is held back for good.
#[tokio::test]
async fn rename_here_while_drive_edits_the_file_keeps_drives_edit() {
    let w = folded_world();
    w.put_drive("meeting.txt", "v1");
    w.put_drive("other.txt", "o");
    assert_eq!(RunStatus::Success, w.sync_from_preview().await.status);
    std::fs::rename(w.path("meeting.txt"), w.path("Meeting.txt")).unwrap();
    w.clock.advance(10_000);
    w.put_drive("meeting.txt", "v2 from Drive");
    let notes = w.engine.preview(&w.pair_id).await.unwrap().notes;
    assert!(!notes.iter().any(|n| n.contains("same name twice")), "{notes:?}");
    for _ in 0..2 {
        let o = w.sync().await;
        assert_eq!(RunStatus::Success, o.status, "{:?}", o.message);
    }
    let local = w.local_files();
    assert!(local.values().any(|v| v == "v2 from Drive"), "{local:?}");
    assert_eq!(2, local.len(), "{local:?}");
    assert_eq!(
        map(&[("meeting.txt", "v2 from Drive"), ("other.txt", "o")]),
        w.drive_files()
    );
}

/// Renamed on both sides by case only, differently: one file on each side, content intact.
#[tokio::test]
async fn case_only_rename_on_both_sides_ends_as_one_file() {
    let w = folded_world();
    w.put_drive("meeting.txt", "notes");
    w.put_drive("other.txt", "o");
    assert_eq!(RunStatus::Success, w.sync_from_preview().await.status);
    std::fs::rename(w.path("meeting.txt"), w.path("Meeting.txt")).unwrap();
    assert!(w.drive.move_path("Sync/meeting.txt", "Sync/MEETING.txt"));
    for _ in 0..3 {
        let o = w.sync().await;
        assert_eq!(RunStatus::Success, o.status, "{:?}", o.message);
    }
    let (local, drive) = (w.local_files(), w.drive_files());
    assert_eq!(2, local.len(), "{local:?}");
    assert_eq!(2, drive.len(), "{drive:?}");
    assert_eq!(local.keys().collect::<Vec<_>>(), drive.keys().collect::<Vec<_>>());
    assert!(drive.values().any(|v| v == "notes"));
}

/// A clash ends once one of the names is renamed: both files sync.
#[tokio::test]
async fn a_clash_resolves_once_one_name_is_renamed() {
    let w = folded_world();
    w.put_drive("Report.txt", "mine");
    assert_eq!(RunStatus::Success, w.sync_from_preview().await.status);
    w.clock.advance(10_000);
    w.put_drive("report.txt", "theirs");
    w.sync().await;
    assert!(w.drive.move_path("Sync/report.txt", "Sync/report (theirs).txt"));
    for _ in 0..2 {
        let o = w.sync().await;
        assert_eq!(RunStatus::Success, o.status, "{:?}", o.message);
    }
    let want = map(&[("Report.txt", "mine"), ("report (theirs).txt", "theirs")]);
    assert_eq!(want, w.drive_files());
    assert_eq!(want, w.local_files());
}

/// A rename here by case while Drive deleted the file: the file uploads under its new name and
/// nothing is lost or held as a clash.
#[tokio::test]
async fn rename_here_while_drive_deletes_the_file_uploads_it() {
    let w = folded_world();
    w.put_drive("meeting.txt", "notes");
    w.put_drive("other.txt", "o");
    assert_eq!(RunStatus::Success, w.sync_from_preview().await.status);
    std::fs::rename(w.path("meeting.txt"), w.path("Meeting.txt")).unwrap();
    assert!(w.drive.trash_path("Sync/meeting.txt"));
    for _ in 0..2 {
        let o = w.sync().await;
        assert_eq!(RunStatus::Success, o.status, "{:?}", o.message);
    }
    let want = map(&[("Meeting.txt", "notes"), ("other.txt", "o")]);
    assert_eq!(want, w.drive_files());
    assert_eq!(want, w.local_files());
}

/// A file Drive moves into a folder whose name clashes (`Docs` and `docs` on Drive) is not followed
/// into it, and not deleted here as if Drive had deleted it: it waits with the clash.
#[tokio::test]
async fn a_move_into_a_clashing_folder_waits_with_the_clash() {
    let w = World::new();
    w.put_drive("a.txt", "a");
    w.put_drive("Docs/x.txt", "x");
    assert_eq!(RunStatus::Success, w.sync_from_preview().await.status);
    w.clock.advance(10_000);
    w.drive.mkdirs("Sync/docs");
    assert!(w.drive.move_path("Sync/a.txt", "Sync/docs/a.txt"));
    for _ in 0..2 {
        w.sync().await;
    }
    assert_eq!(map(&[("Docs/x.txt", "x"), ("a.txt", "a")]), w.local_files());
    assert!(bin_is_empty(&w));
    assert_eq!(map(&[("Docs/x.txt", "x"), ("docs/a.txt", "a")]), w.drive_files());
}
