//! `wasl ignore --explain` and the planner agree (SRS FR-A3, issue #51).

mod common;

use common::{PAIR, World};
use wasl_core::DocsFormat;
use wasl_core::docs::{GOOGLE_DOC, GOOGLE_DRAWING};
use wasl_core::planner::SIZE_LIMIT_SKIP;
use wasl_core::{Action, RunStatus, SyncMode};
use wasl_engine::{
    GOOGLE_ANY_SIZE, INTERNAL_SOURCE, Preview, SIZE_LIMIT_LOCAL_ONLY, export_beside_note, export_here_note,
    google_skipped_note, local_only_note,
};

fn uploads(p: &Preview, key: &str) -> bool {
    p.plan
        .actions
        .iter()
        .any(|a| a.key() == key && matches!(a, Action::Upload { .. }))
}

fn exports_to(p: &Preview, key: &str, rel: &str) -> bool {
    p.plan
        .actions
        .iter()
        .any(|a| a.key() == key && matches!(a, Action::Export { local_rel, .. } if local_rel == rel))
}

fn skipped(p: &Preview, key: &str) -> bool {
    p.plan
        .actions
        .iter()
        .any(|a| a.key() == key && matches!(a, Action::Skip { .. }))
}

fn skipped_for_size(p: &Preview, key: &str) -> bool {
    p.plan
        .actions
        .iter()
        .any(|a| a.key() == key && matches!(a, Action::Skip { reason, .. } if reason == SIZE_LIMIT_SKIP))
}

#[tokio::test]
async fn explain_and_the_preview_agree_on_hidden_files_left_out_subfolders_and_the_size_limit() {
    let w = World::with(
        |p| {
            p.excluded_subfolders = vec!["Private".into()];
            p.size_limit = 16;
        },
        None,
    );
    w.write(".claude/skills/SKILL.md", "skill");
    w.write("Private/notes.txt", "secret");
    w.write("big.bin", &"x".repeat(64));
    w.write("small.txt", "ok");
    w.write("photo.jpg.drivesync-part", "half");
    w.put_drive("huge.bin", &"y".repeat(64));
    let preview = w.engine.preview(PAIR).await.unwrap();

    for (key, ignored) in [
        (".claude/skills/SKILL.md", true),
        ("Private/notes.txt", true),
        ("big.bin", true),
        ("photo.jpg.drivesync-part", true),
        ("small.txt", false),
    ] {
        let v = w.engine.explain_ignore(PAIR, key, false).await.unwrap();
        assert_eq!(ignored, v.ignored, "{key}: {v:?}");
        assert_eq!(
            !ignored,
            uploads(&preview, key),
            "{key}: {:?}",
            preview.plan.actions
        );
    }
    assert!(
        skipped_for_size(&preview, "big.bin"),
        "{:?}",
        preview.plan.actions
    );
    let part = w
        .engine
        .explain_ignore(PAIR, "photo.jpg.drivesync-part", false)
        .await
        .unwrap();
    assert_eq!(
        Some(INTERNAL_SOURCE),
        part.source.as_deref(),
        "no setting changes it"
    );

    // Only on Drive and never synced: explain does not list Drive, and says it checked the device only.
    let huge = w.engine.explain_ignore(PAIR, "huge.bin", false).await.unwrap();
    let local_only = format!("{SIZE_LIMIT_LOCAL_ONLY} {GOOGLE_ANY_SIZE}");
    assert_eq!(
        (false, Some(local_only.as_str())),
        (huge.ignored, huge.note.as_deref())
    );
    assert!(
        skipped_for_size(&preview, "huge.bin"),
        "{:?}",
        preview.plan.actions
    );
}

#[tokio::test]
async fn a_synced_file_over_the_size_limit_keeps_syncing_in_explain_and_the_plan() {
    let w = World::new();
    w.write("notes.txt", "short");
    assert_eq!(RunStatus::Success, w.sync().await.status);
    w.set_pair(|p| p.size_limit = 16);
    w.clock.advance(5_000);
    w.write("notes.txt", &"longer than the limit ".repeat(4));

    let v = w.engine.explain_ignore(PAIR, "notes.txt", false).await.unwrap();
    assert!(!v.ignored && v.pattern.is_none() && v.note.is_none(), "{v:?}");
    let preview = w.engine.preview(PAIR).await.unwrap();
    assert!(uploads(&preview, "notes.txt"), "{:?}", preview.plan.actions);
}

#[tokio::test]
async fn an_exported_google_doc_over_the_size_limit_is_not_ignored() {
    let w = World::new();
    w.drive.put_google_doc("Sync/Notes", GOOGLE_DOC);
    assert_eq!(RunStatus::Success, w.sync().await.status);
    assert!(w.read("Notes.docx").is_some_and(|t| t.len() > 4));
    w.set_pair(|p| p.size_limit = 4);

    let v = w.engine.explain_ignore(PAIR, "Notes.docx", false).await.unwrap();
    assert!(!v.ignored && v.pattern.is_none() && v.note.is_none(), "{v:?}");
    let preview = w.engine.preview(PAIR).await.unwrap();
    assert!(
        !preview
            .plan
            .actions
            .iter()
            .any(|a| matches!(a, Action::Skip { .. })),
        "{:?}",
        preview.plan.actions
    );

    // Removed on the device, the doc is exported again whatever its size.
    w.remove("Notes.docx");
    let v = w.engine.explain_ignore(PAIR, "Notes.docx", false).await.unwrap();
    assert!(!v.ignored && v.note.is_none(), "{v:?}");
    let preview = w.engine.preview(PAIR).await.unwrap();
    assert!(
        preview
            .plan
            .actions
            .iter()
            .any(|a| a.key() == "Notes" && matches!(a, Action::Export { .. })),
        "{:?}",
        preview.plan.actions
    );
}

#[tokio::test]
async fn a_never_synced_copy_over_the_size_limit_is_skipped_and_a_doc_on_drive_is_noted() {
    let w = World::with(|p| p.size_limit = 4, None);
    let local = "a local copy longer than the limit";
    w.write("Notes.docx", local);

    // Explain can't see Drive: the verdict is the local file's, and a doc is only a note.
    let note = format!(
        "{} {}",
        export_beside_note(&["Notes"]),
        export_here_note(&["Notes.docx"], true)
    );
    let expected = (true, Some(SIZE_LIMIT_SKIP), Some(note.as_str()));
    let v = w.engine.explain_ignore(PAIR, "Notes.docx", false).await.unwrap();
    assert_eq!(expected, (v.ignored, v.pattern.as_deref(), v.note.as_deref()));
    let preview = w.engine.preview(PAIR).await.unwrap();
    assert!(
        skipped_for_size(&preview, "Notes.docx"),
        "{:?}",
        preview.plan.actions
    );

    // With a doc `Notes` on Drive the verdict is the same, and the note is what the plan does.
    w.drive.put_google_doc("Sync/Notes", GOOGLE_DOC);
    let v = w.engine.explain_ignore(PAIR, "Notes.docx", false).await.unwrap();
    assert_eq!(expected, (v.ignored, v.pattern.as_deref(), v.note.as_deref()));
    let preview = w.engine.preview(PAIR).await.unwrap();
    assert!(
        exports_to(&preview, "Notes", "Notes (Google Doc).docx"),
        "{:?}",
        preview.plan.actions
    );

    // Running the plan the preview showed writes the export beside the local copy, which stays
    // (#139).
    let out = w.sync_from_preview().await;
    assert_eq!(Some(local.to_owned()), w.read("Notes.docx"), "{out:?}");
    assert!(w.read("Notes (Google Doc).docx").is_some(), "{out:?}");
}

#[tokio::test]
async fn a_synced_export_is_checked_against_the_rules_by_its_key() {
    let w = World::new();
    w.drive.put_google_doc("Sync/Notes", GOOGLE_DOC);
    assert_eq!(RunStatus::Success, w.sync().await.status);
    w.write(".waslignore", "*.docx\n");
    w.remove("Notes.docx");

    // The planner checks the key `Notes`, which `*.docx` does not match, and exports it again.
    let v = w.engine.explain_ignore(PAIR, "Notes.docx", false).await.unwrap();
    assert!(!v.ignored && v.pattern.is_none() && v.note.is_none(), "{v:?}");
    let preview = w.engine.preview(PAIR).await.unwrap();
    assert!(
        exports_to(&preview, "Notes", "Notes.docx"),
        "{:?}",
        preview.plan.actions
    );

    // A rule on the key leaves the export out of both.
    w.write(".waslignore", "Notes\n");
    let v = w.engine.explain_ignore(PAIR, "Notes.docx", false).await.unwrap();
    assert_eq!((true, Some("Notes")), (v.ignored, v.pattern.as_deref()), "{v:?}");
    let preview = w.engine.preview(PAIR).await.unwrap();
    assert!(
        !preview.plan.actions.iter().any(|a| a.key() == "Notes"),
        "{:?}",
        preview.plan.actions
    );
}

#[tokio::test]
async fn a_doc_only_on_drive_gets_the_size_note_for_both_regular_and_google_files() {
    let w = World::with(|p| p.size_limit = 4, None);
    w.drive.put_google_doc("Sync/Notes", GOOGLE_DOC);

    // Explain can't tell a doc from a regular file on Drive, so its note covers both, and names
    // the doc that would be exported to a name an export writes.
    let size = format!("{SIZE_LIMIT_LOCAL_ONLY} {GOOGLE_ANY_SIZE}");
    let export = format!("{size} {}", export_here_note(&["Notes", "Notes.docx"], false));
    for (path, note) in [("Notes", &size), ("Notes.docx", &export)] {
        let v = w.engine.explain_ignore(PAIR, path, false).await.unwrap();
        assert_eq!(
            (false, None, Some(note.as_str())),
            (v.ignored, v.pattern.as_deref(), v.note.as_deref()),
            "{path}"
        );
    }
    // The planner never checks a doc's size.
    let preview = w.engine.preview(PAIR).await.unwrap();
    assert!(
        exports_to(&preview, "Notes", "Notes.docx") && !skipped_for_size(&preview, "Notes"),
        "{:?}",
        preview.plan.actions
    );
}

#[tokio::test]
async fn a_never_synced_copy_under_an_ignored_export_name_is_ignored_and_a_doc_on_drive_is_noted() {
    let w = World::new();
    w.write("Notes.docx", "a local copy");
    w.write(".waslignore", "*.docx\n");

    // `*.docx` ignores the local name, and a doc titled `Notes.docx` too; the planner checks a
    // doc `Notes` by its key, which the rule keeps, so only that one is noted.
    let note = export_here_note(&["Notes"], true);
    let expected = (true, Some("*.docx"), Some(".waslignore:1"), Some(note.as_str()));
    let v = w.engine.explain_ignore(PAIR, "Notes.docx", false).await.unwrap();
    let got = (
        v.ignored,
        v.pattern.as_deref(),
        v.source.as_deref(),
        v.note.as_deref(),
    );
    assert_eq!(expected, got);
    let preview = w.engine.preview(PAIR).await.unwrap();
    assert!(!uploads(&preview, "Notes.docx"), "{:?}", preview.plan.actions);

    w.drive.put_google_doc("Sync/Notes", GOOGLE_DOC);
    let v = w.engine.explain_ignore(PAIR, "Notes.docx", false).await.unwrap();
    let got = (
        v.ignored,
        v.pattern.as_deref(),
        v.source.as_deref(),
        v.note.as_deref(),
    );
    assert_eq!(expected, got);
    let preview = w.engine.preview(PAIR).await.unwrap();
    assert!(
        exports_to(&preview, "Notes", "Notes.docx"),
        "{:?}",
        preview.plan.actions
    );

    // A rule on the doc's key too leaves the Drive sentence out, and the doc out of the plan.
    w.write(".waslignore", "*.docx\nNotes\n");
    let v = w.engine.explain_ignore(PAIR, "Notes.docx", false).await.unwrap();
    assert_eq!(
        (true, Some("*.docx"), None),
        (v.ignored, v.pattern.as_deref(), v.note.as_deref()),
        "{v:?}"
    );
    let preview = w.engine.preview(PAIR).await.unwrap();
    assert!(
        !preview
            .plan
            .actions
            .iter()
            .any(|a| a.key() == "Notes" || a.key() == "Notes.docx"),
        "{:?}",
        preview.plan.actions
    );
}

#[tokio::test]
async fn a_never_synced_png_a_rule_ignores_is_ignored_and_a_drawing_on_drive_is_noted() {
    let w = World::new();
    w.write(".waslignore", "*.png\n");
    w.write("logo.png", "pixels");

    // The planner drops `logo.png`; a Google drawing `logo`, whose key `*.png` keeps, would be
    // exported over it.
    let note = export_here_note(&["logo"], true);
    assert_eq!(
        "If a Google file `logo` is on Drive, it is exported here and this file goes to the recycle bin.",
        note
    );
    let expected = (true, Some("*.png"), Some(".waslignore:1"), Some(note.as_str()));
    let v = w.engine.explain_ignore(PAIR, "logo.png", false).await.unwrap();
    let got = (
        v.ignored,
        v.pattern.as_deref(),
        v.source.as_deref(),
        v.note.as_deref(),
    );
    assert_eq!(expected, got);
    let preview = w.engine.preview(PAIR).await.unwrap();
    assert!(!uploads(&preview, "logo.png"), "{:?}", preview.plan.actions);

    w.drive.put_google_doc("Sync/logo", GOOGLE_DRAWING);
    let v = w.engine.explain_ignore(PAIR, "logo.png", false).await.unwrap();
    let got = (
        v.ignored,
        v.pattern.as_deref(),
        v.source.as_deref(),
        v.note.as_deref(),
    );
    assert_eq!(expected, got);
    let preview = w.engine.preview(PAIR).await.unwrap();
    assert!(
        exports_to(&preview, "logo", "logo.png"),
        "{:?}",
        preview.plan.actions
    );
}

#[tokio::test]
async fn a_never_synced_copy_under_an_export_name_syncs_and_a_doc_on_drive_is_noted() {
    let note = format!(
        "{} {}",
        export_beside_note(&["Notes"]),
        export_here_note(&["Notes.docx"], true)
    );
    assert_eq!(
        "If a Google file `Notes` is on Drive, it is exported beside this file under a name of its own, and this file stays. If a Google file `Notes.docx` is on Drive, it is exported here and this file goes to the recycle bin.",
        note
    );
    // A doc titled `Notes` or `Notes.docx` (an existing extension is not doubled) lands here.
    for title in [None, Some("Notes"), Some("Notes.docx")] {
        let w = World::new();
        w.write("Notes.docx", "a local copy");
        if let Some(title) = title {
            w.drive.put_google_doc(&format!("Sync/{title}"), GOOGLE_DOC);
        }
        let v = w.engine.explain_ignore(PAIR, "Notes.docx", false).await.unwrap();
        assert_eq!(
            (false, None, Some(note.as_str())),
            (v.ignored, v.pattern.as_deref(), v.note.as_deref()),
            "{title:?}"
        );
        let preview = w.engine.preview(PAIR).await.unwrap();
        let planned = match title {
            None => uploads(&preview, "Notes.docx"),
            // A doc `Notes` goes beside the real file, which still uploads (#139); a doc
            // `Notes.docx` shares its key and lands on it.
            Some("Notes") => {
                exports_to(&preview, "Notes", "Notes (Google Doc).docx") && uploads(&preview, "Notes.docx")
            }
            Some(title) => exports_to(&preview, title, "Notes.docx"),
        };
        assert!(planned, "{title:?}: {:?}", preview.plan.actions);
    }
}

#[tokio::test]
async fn with_pdf_exports_a_never_synced_pdf_is_noted_as_an_export_name() {
    let w = World::with(|p| p.docs_format = DocsFormat::Pdf, None);
    w.drive.put_google_doc("Sync/Notes", GOOGLE_DOC);
    w.write("Notes.pdf", "a local copy");

    let v = w.engine.explain_ignore(PAIR, "Notes.pdf", false).await.unwrap();
    let note = format!(
        "{} {}",
        export_beside_note(&["Notes"]),
        export_here_note(&["Notes.pdf"], true)
    );
    assert_eq!((false, Some(note.as_str())), (v.ignored, v.note.as_deref()));
    let preview = w.engine.preview(PAIR).await.unwrap();
    assert!(
        exports_to(&preview, "Notes", "Notes (Google Doc).pdf"),
        "{:?}",
        preview.plan.actions
    );
}

async fn a_doc_only_on_drive_is_noted_as_skipped(w: World) {
    w.drive.put_google_doc("Sync/Notes", GOOGLE_DOC);
    let v = w.engine.explain_ignore(PAIR, "Notes", false).await.unwrap();
    let note = google_skipped_note(&["Notes"]);
    assert_eq!(
        (false, None, Some(note.as_str())),
        (v.ignored, v.pattern.as_deref(), v.note.as_deref())
    );
    let preview = w.engine.preview(PAIR).await.unwrap();
    assert!(skipped(&preview, "Notes"), "{:?}", preview.plan.actions);

    // A never-synced copy on the device under a name an export writes syncs as a regular file,
    // and the note names the Google file's title, not the copy's name.
    w.write("Report.docx", "a local copy");
    let v = w.engine.explain_ignore(PAIR, "Report.docx", false).await.unwrap();
    let note = google_skipped_note(&["Report", "Report.docx"]);
    assert_eq!(
        (false, None, Some(note.as_str())),
        (v.ignored, v.pattern.as_deref(), v.note.as_deref())
    );
    let preview = w.engine.preview(PAIR).await.unwrap();
    assert!(uploads(&preview, "Report.docx"), "{:?}", preview.plan.actions);
}

#[tokio::test]
async fn with_docs_export_off_a_doc_only_on_drive_is_noted_as_skipped() {
    a_doc_only_on_drive_is_noted_as_skipped(World::with(|p| p.docs_export = false, None)).await;
}

#[tokio::test]
async fn in_an_upload_only_pair_a_doc_only_on_drive_is_noted_as_skipped() {
    a_doc_only_on_drive_is_noted_as_skipped(World::with(|p| p.method = SyncMode::UploadOnly, None)).await;
}

/// #99: a never-synced file only on the device is not uploaded in a pair that only downloads, and a
/// mirror from Drive bins it; explain says so, with the mode named, as the preview plans it.
#[tokio::test]
async fn explain_names_what_each_download_mode_does_with_a_file_only_on_the_device() {
    for mode in [
        SyncMode::DownloadOnly,
        SyncMode::DownloadThenDelete,
        SyncMode::DownloadMirror,
    ] {
        let w = World::with(|p| p.method = mode, None);
        w.write("local.txt", "here only");
        let preview = w.engine.preview(PAIR).await.unwrap();
        let planned: Vec<&Action> = preview
            .plan
            .actions
            .iter()
            .filter(|a| a.key() == "local.txt")
            .collect();
        let binned = planned.iter().any(|a| matches!(a, Action::DeleteLocal { .. }));
        assert_eq!(mode == SyncMode::DownloadMirror, binned, "{mode:?}: {planned:?}");
        assert!(!uploads(&preview, "local.txt"), "{mode:?}: {planned:?}");

        let v = w.engine.explain_ignore(PAIR, "local.txt", false).await.unwrap();
        assert!(!v.ignored, "{mode:?}: {v:?}");
        let note = local_only_note(mode).expect("a note for every download-only mode");
        assert_eq!(Some(note.as_str()), v.note.as_deref(), "{mode:?}");
    }
    // A pair that uploads sends the file to Drive: no note.
    assert_eq!(None, local_only_note(SyncMode::TwoWay));
    assert_eq!(None, local_only_note(SyncMode::UploadOnly));
}
