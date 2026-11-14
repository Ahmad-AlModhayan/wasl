//! Agent undo (docs/AGENT_UNDO.md): the watch list (UND-1), session-start snapshots (UND-2) and
//! the journal (UND-3, UND-4).

mod common;

use common::{PAIR, World};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use wasl_engine::{
    EngineConfig, KeepBy, SnapshotStats, UndoStore, check_watch_root, hash_clones, snapshot_folder,
};
use wasl_store::{DEFAULT_WATCH_IGNORES, SnapshotOrigin, SnapshotRow};

fn defaults() -> Vec<String> {
    DEFAULT_WATCH_IGNORES.map(String::from).to_vec()
}

fn write(root: &Path, rel: &str, body: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn hash_of(p: &Path) -> String {
    blake3::hash(&std::fs::read(p).unwrap()).to_hex().to_string()
}

fn no_sync(_: &str, _: u64, _: i64) -> Option<String> {
    None
}

fn snap(
    root: &Path,
    store: &UndoStore,
    previous: &[SnapshotRow],
    keep: KeepBy,
) -> (Vec<SnapshotRow>, SnapshotStats) {
    snapshot_folder(root, &defaults(), store, "c1", previous, &no_sync, keep).unwrap()
}

/// A small project: code, a `.waslignore`, and folders the default ignores leave out.
fn project(root: &Path) {
    write(root, "src/main.rs", b"fn main() {}\n");
    write(root, "src/lib.rs", &[7u8; 300_000]);
    write(root, "README.md", "# Site\n".as_bytes());
    write(root, ".env", b"KEY=1\n");
    write(
        root,
        "notes/\u{0645}\u{0644}\u{0627}\u{062d}\u{0638}\u{0627}\u{062a}.txt",
        b"\xd9\x85\n",
    );
    write(root, ".waslignore", b"*.log\n");
    write(root, "debug.log", b"noise\n");
    write(root, "node_modules/left-pad/index.js", b"module.exports = 1;\n");
    write(root, "web/node_modules/x.js", b"1\n");
    write(root, "target/debug/app", b"\x7fELF");
    write(root, "build/out.js", b"2\n");
    write(root, ".venv/bin/python", b"#!\n");
}

#[test]
fn the_top_of_a_disk_cant_be_watched_on_any_os() {
    for top in [
        "/",
        "\\",
        "C:\\",
        "C:",
        "c:/",
        r"\\?\C:\",
        r"\\server\share",
        r"\\server\share\",
    ] {
        let e = check_watch_root(Path::new(top), &[]).unwrap_err();
        assert!(e.contains("top of a disk"), "{top}: {e}");
    }
}

#[test]
fn wasl_s_data_folder_and_the_folders_around_it_cant_be_watched() {
    let home = tempfile::tempdir().unwrap();
    let data = home.path().join("Wasl");
    std::fs::create_dir_all(data.join("agent-undo")).unwrap();
    for dir in [home.path().to_owned(), data.clone(), data.join("agent-undo")] {
        let e = check_watch_root(&dir, std::slice::from_ref(&data)).unwrap_err();
        assert!(e.contains("keeps its own data"), "{}: {e}", dir.display());
    }
    let site = home.path().join("site");
    std::fs::create_dir_all(&site).unwrap();
    let ok = check_watch_root(&site, &[data]).unwrap();
    // The full path as pairs record it: on Windows without the `\\?\` that `canonicalize` adds.
    assert_eq!(
        std::fs::canonicalize(&site).unwrap(),
        std::fs::canonicalize(&ok).unwrap()
    );
    assert!(!ok.to_string_lossy().starts_with(r"\\?\"), "{}", ok.display());
    write(home.path(), "file.txt", b"x");
    assert!(check_watch_root(&home.path().join("file.txt"), &[]).is_err());
    assert!(check_watch_root(&home.path().join("missing"), &[]).is_err());
}

/// UND-2 acceptance: the snapshot holds every file the rules keep, each with the folder's BLAKE3,
/// whether the file system cloned it or the store copied it.
#[test]
fn snapshot_hashes_equal_the_folder_s_hashes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("site");
    project(&root);
    let expected: BTreeMap<String, String> = [
        ".env",
        ".waslignore",
        "README.md",
        "notes/\u{0645}\u{0644}\u{0627}\u{062d}\u{0638}\u{0627}\u{062a}.txt",
        "src/lib.rs",
        "src/main.rs",
    ]
    .into_iter()
    .map(|rel| (rel.to_owned(), hash_of(&root.join(rel))))
    .collect();
    for keep in [KeepBy::Store, KeepBy::CloneOrStore] {
        let store = UndoStore::new(dir.path().join(format!("undo-{keep:?}")));
        let (mut rows, mut stats) = snap(&root, &store, &[], keep);
        hash_clones(&store, &mut rows, &mut stats).unwrap();
        let got: BTreeMap<String, String> = rows
            .iter()
            .map(|r| (r.rel.clone(), r.blake3.clone().unwrap()))
            .collect();
        assert_eq!(expected, got, "{keep:?}");
        for r in &rows {
            // The kept bytes are the folder's bytes.
            assert_eq!(
                r.blake3.as_deref(),
                Some(hash_of(&store.path(r.location.as_deref().unwrap())).as_str())
            );
            assert_eq!(r.size, std::fs::metadata(root.join(&r.rel)).unwrap().len());
        }
        assert_eq!(6, stats.files);
        if keep == KeepBy::Store {
            assert_eq!(6, stats.stored);
            assert!(rows.iter().all(|r| r.origin == SnapshotOrigin::Stored));
        }
        // macOS temp folders are on APFS, so the clone path must really clone there.
        #[cfg(target_os = "macos")]
        if keep == KeepBy::CloneOrStore {
            assert_eq!(6, stats.cloned);
            assert!(rows.iter().all(|r| r.origin == SnapshotOrigin::Cloned));
        }
    }
}

#[test]
fn a_removed_default_ignore_stops_applying() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("site");
    project(&root);
    let store = UndoStore::new(dir.path().join("undo"));
    let ignores: Vec<String> = defaults().into_iter().filter(|i| i != "build/").collect();
    let (rows, _) = snapshot_folder(&root, &ignores, &store, "c1", &[], &no_sync, KeepBy::Store).unwrap();
    let rels: Vec<&str> = rows.iter().map(|r| r.rel.as_str()).collect();
    assert!(rels.contains(&"build/out.js"), "{rels:?}");
    assert!(
        !rels
            .iter()
            .any(|r| r.contains("node_modules") || r.starts_with("target/"))
    );
}

/// UND-2 acceptance, scaled down from 10 GB: a snapshot of an unchanged folder reads under 1% of
/// it, and one changed file costs only that file.
#[test]
fn an_unchanged_re_snapshot_reads_under_one_percent() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("data");
    for i in 0..400 {
        write(
            &root,
            &format!("d{}/f{i}.bin", i % 7),
            &vec![(i % 251) as u8; 16 * 1024],
        );
    }
    let store = UndoStore::new(dir.path().join("undo"));
    let (first, stats) = snap(&root, &store, &[], KeepBy::Store);
    assert_eq!(stats.bytes, stats.bytes_read);
    let (again, stats) = snap(&root, &store, &first, KeepBy::Store);
    assert_eq!(first, again);
    if cfg!(windows) {
        // No file id on Windows yet (#194): every file is read again, so the target is macOS and
        // Linux only until then (AGENT_UNDO, UND-2).
        assert_eq!((0, stats.bytes), (stats.reused, stats.bytes_read));
        return;
    }
    assert_eq!((400, 0), (stats.reused, stats.bytes_read));
    assert!(stats.bytes_read * 100 < stats.bytes);

    let changed = root.join("d3/f10.bin");
    std::fs::write(&changed, vec![9u8; 16 * 1024 + 1]).unwrap();
    let (third, stats) = snap(&root, &store, &again, KeepBy::Store);
    assert_eq!((399, 1), (stats.reused, stats.stored));
    assert!(stats.bytes_read * 100 < stats.bytes, "{stats:?}");
    let row = third.iter().find(|r| r.rel == "d3/f10.bin").unwrap();
    assert_eq!(Some(hash_of(&changed)), row.blake3);
}

/// `cp -p`, tar and sync tools put back the old mtime; size and mtime alone would reuse the old row.
#[test]
fn a_replacement_with_the_same_size_and_restored_mtime_is_read_again() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("data");
    write(&root, "a.txt", b"aaaa");
    let store = UndoStore::new(dir.path().join("undo"));
    let (first, _) = snap(&root, &store, &[], KeepBy::Store);
    let mtime = std::fs::metadata(root.join("a.txt")).unwrap().modified().unwrap();
    write(&root, "a.new", b"bbbb");
    let new = std::fs::File::options()
        .write(true)
        .open(root.join("a.new"))
        .unwrap();
    new.set_modified(mtime).unwrap();
    drop(new);
    std::fs::rename(root.join("a.new"), root.join("a.txt")).unwrap();
    let (again, stats) = snap(&root, &store, &first, KeepBy::Store);
    assert_eq!((first[0].size, first[0].mtime), (again[0].size, again[0].mtime));
    assert_eq!((0, 1), (stats.reused, stats.stored));
    assert_eq!(Some(hash_of(&root.join("a.txt"))), again[0].blake3);
}

#[test]
fn a_synced_pair_file_reuses_the_synced_version_without_reading_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("site");
    write(&root, "a.txt", b"synced");
    write(&root, "b.txt", b"local only");
    let a = root.join("a.txt");
    let size = std::fs::metadata(&a).unwrap().len();
    let store = UndoStore::new(dir.path().join("undo"));
    let lookup = |rel: &str, s: u64, _mtime: i64| (rel == "a.txt" && s == size).then(|| "ab".repeat(32));
    let (rows, stats) =
        snapshot_folder(&root, &defaults(), &store, "c1", &[], &lookup, KeepBy::Store).unwrap();
    assert_eq!((1, 1), (stats.synced, stats.stored));
    assert_eq!(b"local only".len() as u64, stats.bytes_read);
    let a = rows.iter().find(|r| r.rel == "a.txt").unwrap();
    assert_eq!((SnapshotOrigin::Synced, None), (a.origin, a.location.clone()));
}

#[test]
fn a_file_removed_between_the_scan_and_the_read_is_left_out_and_the_rest_kept() {
    for keep in [KeepBy::Store, KeepBy::CloneOrStore] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("data");
        for rel in ["a.txt", "b.txt", "c.txt"] {
            write(&root, rel, rel.as_bytes());
        }
        let undo = dir.path().join("undo");
        // Called between the scan and the read of each file.
        let remove_b = |rel: &str, _: u64, _: i64| {
            if rel == "b.txt" {
                std::fs::remove_file(root.join(rel)).unwrap();
            }
            None
        };
        let (rows, stats) = snapshot_folder(
            &root,
            &defaults(),
            &UndoStore::new(&undo),
            "c1",
            &[],
            &remove_b,
            keep,
        )
        .unwrap();
        let mut kept: Vec<&str> = rows.iter().map(|r| r.rel.as_str()).collect();
        kept.sort();
        assert_eq!(vec!["a.txt", "c.txt"], kept, "{keep:?}");
        assert_eq!((2, 1), (stats.files, stats.vanished));
        assert_eq!(
            0,
            std::fs::read_dir(undo.join("tmp"))
                .map(|d| d.count())
                .unwrap_or(0)
        );
    }
}

#[cfg(unix)]
#[test]
fn the_undo_store_is_readable_by_its_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("site");
    write(&root, "a.txt", b"a");
    let undo = dir.path().join("undo");
    std::fs::set_permissions(root.join("a.txt"), std::fs::Permissions::from_mode(0o644)).unwrap();
    let store = UndoStore::new(&undo);
    snap(&root, &store, &[], KeepBy::Store);
    for d in [undo.clone(), undo.join("objects"), undo.join("tmp")] {
        let mode = std::fs::metadata(&d).unwrap().permissions().mode() & 0o777;
        assert_eq!(0o700, mode, "{}", d.display());
    }
    // Objects and clones (which start with the source's mode) alike.
    for keep in [KeepBy::Store, KeepBy::CloneOrStore] {
        let (rows, _) = snapshot_folder(&root, &defaults(), &store, "c2", &[], &no_sync, keep).unwrap();
        let file = store.path(rows[0].location.as_deref().unwrap());
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(0o600, mode, "{keep:?} {}", file.display());
    }
}

fn world() -> (World, tempfile::TempDir) {
    let data = tempfile::tempdir().unwrap();
    let w = World::with_config(
        |_| {},
        None,
        EngineConfig {
            device_name: "PC".into(),
            agent_dir: Some(data.path().join("agent")),
            undo_dir: Some(data.path().join("agent-undo")),
            ..EngineConfig::default()
        },
    );
    (w, data)
}

fn folder(w: &World, name: &str) -> PathBuf {
    let p = w.dir.path().join(name);
    std::fs::create_dir_all(&p).unwrap();
    std::fs::canonicalize(p).unwrap()
}

#[test]
fn a_folder_in_or_around_a_watched_one_is_refused_and_unwatch_takes_it_off() {
    let (w, _data) = world();
    let site = folder(&w, "site");
    let e = &w.engine;
    let watch = e.agent_watch(&site).unwrap();
    assert_eq!(defaults(), watch.ignores);
    assert!(e.agent_watch(&site).is_err());
    assert!(e.agent_watch(&folder(&w, "site/web")).is_err());
    assert!(
        e.agent_watch(&std::fs::canonicalize(w.dir.path()).unwrap())
            .is_err()
    );
    e.agent_watch(&folder(&w, "api")).unwrap();
    assert_eq!(2, e.agent_watches().unwrap().len());
    assert_eq!(watch.id, e.agent_unwatch(&site.to_string_lossy()).unwrap().id);
    assert!(e.agent_unwatch(&watch.id).is_err());
    assert_eq!(1, e.agent_watches().unwrap().len());
}

#[test]
fn the_engine_refuses_to_watch_its_undo_store_or_the_folders_around_it() {
    let (w, data) = world();
    let undo = data.path().join("agent-undo");
    // Before the first snapshot the undo store's folder doesn't exist yet.
    assert!(w.engine.agent_watch(data.path()).is_err());
    std::fs::create_dir_all(undo.join("objects")).unwrap();
    for dir in [data.path().to_path_buf(), undo.clone(), undo.join("objects")] {
        assert!(w.engine.agent_watch(&dir).is_err(), "{}", dir.display());
    }
    assert!(w.engine.agent_watches().unwrap().is_empty());
}

#[tokio::test]
async fn a_watched_folder_that_cant_be_snapshotted_is_kept_with_the_session_and_why() {
    let (w, _data) = world();
    let site = folder(&w, "site");
    let watch = w.engine.agent_watch(&site).unwrap();
    std::fs::remove_dir_all(&site).unwrap();
    let cs = w
        .engine
        .agent_begin(PAIR, "", "claude-code", None, None)
        .await
        .unwrap();
    let failures = w.engine.agent_snapshot_failures(&cs.id).unwrap();
    assert_eq!(1, failures.len());
    assert_eq!(watch.id, failures[0].0);
    assert!(!failures[0].1.is_empty());
    assert!(w.engine.agent_snapshot(&cs.id, &watch.id).unwrap().is_empty());
}

/// A session covers its own folder and every watched folder (UND-2).
#[tokio::test]
async fn a_session_start_snapshots_every_watched_folder() {
    let (w, _data) = world();
    let site = folder(&w, "site");
    write(&site, "index.html", b"<h1>hi</h1>");
    write(&site, "node_modules/x.js", b"1");
    let watch = w.engine.agent_watch(&site).unwrap();
    let cs = w
        .engine
        .agent_begin(PAIR, "", "claude-code", None, None)
        .await
        .unwrap();
    let rows = w.engine.agent_snapshot(&cs.id, &watch.id).unwrap();
    assert_eq!(
        vec!["index.html"],
        rows.iter().map(|r| r.rel.as_str()).collect::<Vec<_>>()
    );
    for row in &rows {
        assert_eq!(Some(hash_of(&site.join(&row.rel))), row.blake3, "{row:?}");
    }
}

// ---------------------------------------------------------------- the journal (UND-3, UND-4)

/// Begins a pair session that covers the watched folder `site`; returns (changeset id, watch id).
async fn session_on(w: &World, site: &Path) -> (String, String) {
    let watch = w.engine.agent_watch(site).unwrap();
    let cs = w
        .engine
        .agent_begin(PAIR, "", "claude-code", None, None)
        .await
        .unwrap();
    (cs.id, watch.id)
}

/// The bytes a version can be had back from.
fn bytes_of(v: &wasl_engine::Version) -> Vec<u8> {
    std::fs::read(v.path.as_ref().expect("a kept version")).unwrap()
}

#[tokio::test]
async fn a_file_changed_five_times_a_second_apart_restores_to_all_six_versions() {
    let (w, _data) = world();
    let site = folder(&w, "site");
    write(&site, "notes.md", b"version 0");
    let (cs, watch) = session_on(&w, &site).await;
    for i in 1..=5 {
        w.clock.advance(1000);
        write(&site, "notes.md", format!("version {i}").as_bytes());
        let e = w
            .engine
            .agent_capture(&cs, &watch, "notes.md", None)
            .await
            .unwrap()
            .expect("a change is journaled");
        assert_eq!("update", e.op);
        assert!(e.captured);
    }
    let versions = w.engine.agent_versions(&cs, &watch, "notes.md").unwrap();
    assert_eq!(6, versions.len());
    for (i, v) in versions.iter().enumerate() {
        let body = bytes_of(v);
        assert_eq!(format!("version {i}").into_bytes(), body);
        assert_eq!(v.blake3.as_deref(), Some(blake3::hash(&body).to_hex().as_str()));
    }
    // Each entry names the version before it.
    let journal = w.engine.agent_journal(&cs).unwrap();
    for (pair, e) in versions.windows(2).zip(&journal.entries) {
        assert_eq!(pair[0].blake3, e.prev_blake3);
        assert_eq!(pair[1].blake3, e.next_blake3);
    }
    assert!(journal.not_captured.is_empty());
}

#[tokio::test]
async fn an_unchanged_file_journals_nothing() {
    let (w, _data) = world();
    let site = folder(&w, "site");
    write(&site, "a.txt", b"same");
    let (cs, watch) = session_on(&w, &site).await;
    write(&site, "a.txt", b"same");
    assert_eq!(
        None,
        w.engine.agent_capture(&cs, &watch, "a.txt", None).await.unwrap()
    );
    assert_eq!(
        None,
        w.engine
            .agent_capture(&cs, &watch, "never.txt", None)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn creates_deletes_and_renames_are_journaled_and_written_to_the_op_log() {
    let (w, _data) = world();
    let site = folder(&w, "site");
    write(&site, "old.txt", b"moved");
    write(&site, "gone.txt", b"bye");
    let (cs, watch) = session_on(&w, &site).await;

    write(&site, "new.txt", b"hello");
    let created = w
        .engine
        .agent_capture(&cs, &watch, "new.txt", None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        ("create", None),
        (created.op.as_str(), created.prev_blake3.as_deref())
    );

    std::fs::remove_file(site.join("gone.txt")).unwrap();
    let deleted = w
        .engine
        .agent_capture(&cs, &watch, "gone.txt", None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!("delete", deleted.op);
    assert_eq!(
        Some(blake3::hash(b"bye").to_hex().to_string()),
        deleted.prev_blake3
    );
    assert_eq!(None, deleted.next_blake3);

    std::fs::rename(site.join("old.txt"), site.join("moved.txt")).unwrap();
    let renamed = w
        .engine
        .agent_capture_rename(&cs, &watch, "old.txt", "moved.txt")
        .await
        .unwrap();
    assert_eq!(
        vec![
            ("delete", "old.txt", None),
            ("create", "moved.txt", Some("old.txt"))
        ],
        renamed
            .iter()
            .map(|e| (e.op.as_str(), e.rel.as_str(), e.old_rel.as_deref()))
            .collect::<Vec<_>>()
    );
    // The deleted file can be had back from the snapshot.
    let versions = w.engine.agent_versions(&cs, &watch, "gone.txt").unwrap();
    assert_eq!(b"bye".to_vec(), bytes_of(&versions[0]));

    // One op-log operation per entry, on file://<watch id>/<rel>, in order.
    let page = w.engine.oplog_show(None, &cs).unwrap();
    let ops = page.changesets[0].operations.clone().unwrap();
    assert_eq!(
        vec![
            ("create", format!("file://{watch}/new.txt")),
            ("delete", format!("file://{watch}/gone.txt")),
            ("delete", format!("file://{watch}/old.txt")),
            ("create", format!("file://{watch}/moved.txt")),
        ],
        ops.iter()
            .map(|o| (o.op.as_str(), o.entity.clone()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        Some(blake3::hash(b"hello").to_hex().to_string()),
        ops[0].next.as_ref().unwrap()["blake3"]
            .as_str()
            .map(str::to_owned)
    );
    // The rename's create names where it came from, in the journal and in its operation alike.
    assert_eq!(
        Some("old.txt"),
        w.engine.agent_journal(&cs).unwrap().entries[3].old_rel.as_deref()
    );
    assert_eq!("old.txt", ops[3].next.as_ref().unwrap()["renamed_from"]);
}

#[tokio::test]
async fn a_session_that_changed_only_a_watched_folder_ends_committed() {
    let (w, _data) = world();
    let site = folder(&w, "site");
    let (cs, watch) = session_on(&w, &site).await;
    write(&site, "a.txt", b"1");
    w.engine
        .agent_capture(&cs, &watch, "a.txt", None)
        .await
        .unwrap()
        .unwrap();
    let ended = w.engine.agent_end(&cs).await.unwrap().expect("kept");
    assert_eq!(wasl_store::ChangesetStatus::Accepted, ended.status);
    let page = w.engine.oplog_show(None, &cs).unwrap();
    assert_eq!(wasl_store::OplogStatus::Committed, page.changesets[0].status);
    assert_eq!(1, w.engine.agent_journal(&cs).unwrap().entries.len());
}

#[tokio::test]
async fn ignored_paths_are_not_journaled() {
    let (w, _data) = world();
    let site = folder(&w, "site");
    let (cs, watch) = session_on(&w, &site).await;
    write(&site, "node_modules/x/index.js", b"1");
    assert_eq!(
        None,
        w.engine
            .agent_capture(&cs, &watch, "node_modules/x/index.js", None)
            .await
            .unwrap()
    );
}

/// UND-4, scaled down from 50,000 files: the watcher overflows partway through a large delete, so
/// only some deletes were seen; the rescan finds the rest. Every file is then restorable or
/// listed as not captured.
#[tokio::test]
async fn a_forced_overflow_during_a_large_delete_leaves_every_file_restorable_or_listed() {
    const FILES: usize = 2_000;
    let (w, data) = world();
    let site = folder(&w, "site");
    for i in 0..FILES {
        write(
            &site,
            &format!("d{}/f{i}.txt", i % 20),
            format!("file {i}").as_bytes(),
        );
    }
    let (cs, watch) = session_on(&w, &site).await;
    // One file whose kept version is lost from the undo store: it must be listed, not restorable.
    let lost = w
        .engine
        .agent_snapshot(&cs, &watch)
        .unwrap()
        .into_iter()
        .find(|r| r.rel == "d0/f0.txt")
        .unwrap();
    std::fs::remove_file(data.path().join("agent-undo").join(lost.location.unwrap())).unwrap();

    let mut seen = 0;
    for i in 0..FILES {
        std::fs::remove_file(site.join(format!("d{}/f{i}.txt", i % 20))).unwrap();
        // The watcher delivers the first 300 events, then overflows.
        if i < 300 && i != 0 {
            w.engine
                .agent_capture(&cs, &watch, &format!("d{}/f{i}.txt", i % 20), None)
                .await
                .unwrap()
                .unwrap();
            seen += 1;
        }
    }
    let found = w.engine.agent_rescan(&cs, "a watcher overflow").await.unwrap();
    assert_eq!(FILES - seen, found.len());

    let journal = w.engine.agent_journal(&cs).unwrap();
    let mut listed = 0;
    for i in 0..FILES {
        let rel = format!("d{}/f{i}.txt", i % 20);
        let versions = w.engine.agent_versions(&cs, &watch, &rel).unwrap();
        let delete = journal
            .entries
            .iter()
            .find(|e| e.rel == rel && e.op == "delete")
            .unwrap_or_else(|| panic!("{rel} is journaled"));
        let restorable =
            versions[0].path.as_ref().is_some_and(|p| p.exists()) && delete.prev_blake3 == versions[0].blake3;
        let not_captured = journal.not_captured.iter().any(|e| e.rel == rel && !e.captured);
        assert!(restorable || not_captured, "{rel}: neither restorable nor listed");
        if !restorable {
            listed += 1;
            assert!(
                delete.reason.as_deref().unwrap().contains("not kept"),
                "{delete:?}"
            );
        }
    }
    assert_eq!(1, listed);
    // A rescan's finds say so; a second rescan finds nothing new.
    assert!(
        found
            .iter()
            .all(|e| e.reason.as_deref().unwrap().contains("a watcher overflow"))
    );
    assert!(
        w.engine
            .agent_rescan(&cs, "a daemon restart")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_rescan_captures_files_created_and_changed_while_unwatched() {
    let (w, _data) = world();
    let site = folder(&w, "site");
    write(&site, "keep.txt", b"same");
    write(&site, "edit.txt", b"before");
    let (cs, watch) = session_on(&w, &site).await;
    write(&site, "edit.txt", b"after, longer");
    write(&site, "new/born.txt", b"new");
    let found = w.engine.agent_rescan(&cs, "a daemon restart").await.unwrap();
    let mut ops: Vec<_> = found
        .iter()
        .map(|e| (e.op.as_str(), e.rel.as_str(), e.captured))
        .collect();
    ops.sort();
    assert_eq!(
        vec![("create", "new/born.txt", true), ("update", "edit.txt", true)],
        ops
    );
    let versions = w.engine.agent_versions(&cs, &watch, "edit.txt").unwrap();
    assert_eq!(b"after, longer".to_vec(), bytes_of(&versions[1]));
}

#[cfg(unix)]
#[tokio::test]
async fn a_folder_that_cant_be_rescanned_lists_its_files_and_the_others_still_are() {
    use std::os::unix::fs::PermissionsExt;
    let (w, _data) = world();
    let (shut, open) = (folder(&w, "shut"), folder(&w, "open"));
    write(&shut, "a.txt", b"a");
    write(&shut, "sub/b.txt", b"b");
    write(&open, "c.txt", b"c");
    let shut_watch = w.engine.agent_watch(&shut).unwrap();
    let (cs, open_watch) = session_on(&w, &open).await;
    write(&open, "c.txt", b"changed");
    std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o000)).unwrap();
    let err = w.engine.agent_rescan(&cs, "a watcher overflow").await;
    std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(err.is_err(), "the caller tries again");
    let report = w.engine.agent_journal(&cs).unwrap();
    // The other folder was still rescanned.
    assert_eq!(
        vec![(open_watch.as_str(), "c.txt")],
        report
            .entries
            .iter()
            .map(|e| (e.root.as_str(), e.rel.as_str()))
            .collect::<Vec<_>>()
    );
    let missed: Vec<_> = report
        .gaps
        .iter()
        .map(|g| (g.watch_id.as_str(), g.rel.as_deref()))
        .collect();
    assert_eq!(
        vec![
            (shut_watch.id.as_str(), Some("a.txt")),
            (shut_watch.id.as_str(), Some("sub/b.txt"))
        ],
        missed
    );
    assert!(report.gaps[0].reason.starts_with("can't rescan"));

    // Readable again: the rescan goes through and the files are no longer listed.
    w.engine.agent_rescan(&cs, "a watcher overflow").await.unwrap();
    assert!(w.engine.agent_journal(&cs).unwrap().gaps.is_empty());
}

#[tokio::test]
async fn a_folder_that_couldnt_be_watched_is_listed_in_the_report() {
    let (w, _data) = world();
    let site = folder(&w, "site");
    let (cs, watch) = session_on(&w, &site).await;
    w.engine
        .agent_watch_gap(&cs, &watch, "can't watch site: too many watches")
        .unwrap();
    let gaps = w.engine.agent_journal(&cs).unwrap().gaps;
    assert_eq!(1, gaps.len());
    assert_eq!(
        (watch.as_str(), None),
        (gaps[0].watch_id.as_str(), gaps[0].rel.as_deref())
    );
    // A rescan that goes through keeps it: watching was still down for a while.
    w.engine.agent_rescan(&cs, "the watcher started").await.unwrap();
    assert_eq!(1, w.engine.agent_journal(&cs).unwrap().gaps.len());
}

#[tokio::test]
async fn a_rescan_finds_a_same_size_replacement_with_its_mtime_put_back() {
    let (w, _data) = world();
    let site = folder(&w, "site");
    write(&site, "a.txt", b"aaaa");
    let (cs, watch) = session_on(&w, &site).await;
    let was = std::fs::metadata(site.join("a.txt")).unwrap().modified().unwrap();
    // As `cp -p` or an unpacked archive leaves it: same size, same mtime, a new file.
    write(&site, "a.new", b"bbbb");
    std::fs::File::options()
        .write(true)
        .open(site.join("a.new"))
        .unwrap()
        .set_modified(was)
        .unwrap();
    std::fs::rename(site.join("a.new"), site.join("a.txt")).unwrap();
    let found = w.engine.agent_rescan(&cs, "a watcher overflow").await.unwrap();
    assert_eq!(
        vec![("update", "a.txt")],
        found
            .iter()
            .map(|e| (e.op.as_str(), e.rel.as_str()))
            .collect::<Vec<_>>()
    );
    let versions = w.engine.agent_versions(&cs, &watch, "a.txt").unwrap();
    assert_eq!(b"bbbb".to_vec(), bytes_of(versions.last().unwrap()));
    // A second rescan finds nothing new: the journaled file is unchanged by its stat.
    assert!(
        w.engine
            .agent_rescan(&cs, "a watcher overflow")
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(1, w.engine.agent_journal(&cs).unwrap().entries.len());
}

#[tokio::test]
async fn a_change_not_journaled_yet_is_found_when_the_session_ends() {
    let (w, _data) = world();
    let site = folder(&w, "site");
    let (cs, watch) = session_on(&w, &site).await;
    // The watcher's event for it was still queued when the session was asked to end.
    write(&site, "late.txt", b"late");
    w.engine.agent_end(&cs).await.unwrap().expect("kept");
    let report = w.engine.agent_journal(&cs).unwrap();
    assert_eq!(1, report.entries.len());
    assert_eq!(
        ("create", "late.txt"),
        (report.entries[0].op.as_str(), report.entries[0].rel.as_str())
    );
    let versions = w.engine.agent_versions(&cs, &watch, "late.txt").unwrap();
    assert_eq!(b"late".to_vec(), bytes_of(versions.last().unwrap()));
}
