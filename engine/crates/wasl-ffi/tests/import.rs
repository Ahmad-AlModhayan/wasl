//! The Drive Sync 1.1 import (SRS FR-K2, docs/MIGRATION.md): a pair's baseline, history, conflicts
//! and bin arrive once, and the first run after it is incremental.

use std::sync::Arc;
use wasl_core::conditions::{Conditions, DeviceState};
use wasl_core::{BaselineEntry, ConflictPolicy, RunStatus};
use wasl_mobile::{
    EngineInit, ImportCounts, ImportedRun, Platform, SafBridge, TokenSource, WaslEngine, WaslError,
};
use wasl_store::{
    BinSide, ConflictRecord, EventLevel, NewBinEntry, Pair, RunEvent, RunRecord, RunStats, StorageMode,
};

struct Tokens;
impl TokenSource for Tokens {
    fn access_token(&self, _account: String) -> Result<String, WaslError> {
        Ok("tok".into())
    }
    fn invalidate(&self, _account: String) {}
}

struct Device;
impl Platform for Device {
    fn device_state(&self) -> DeviceState {
        DeviceState::unrestricted()
    }
    fn saf(&self) -> Option<Arc<dyn SafBridge>> {
        None
    }
    fn vault_access(&self, _vault_id: String) -> Option<wasl_mobile::VaultAccess> {
        None
    }
}

fn engine(dir: &std::path::Path) -> Arc<WaslEngine> {
    WaslEngine::new(
        EngineInit {
            db_path: dir.join("wasl.db").to_string_lossy().into_owned(),
            bin_dir: dir.join("bin").to_string_lossy().into_owned(),
            staging_dir: dir.join("staging").to_string_lossy().into_owned(),
            device_name: "Pixel".into(),
            drive_base_url: Some("http://127.0.0.1:9/drive/v3".into()),
            drive_upload_url: Some("http://127.0.0.1:9/upload/drive/v3".into()),
        },
        Arc::new(Tokens),
        Arc::new(Device),
        None,
    )
    .unwrap()
}

fn pair(root: &std::path::Path) -> Pair {
    let mut p = Pair::new("p", "Notes", root.to_str().unwrap());
    p.account_id = Some("me@example.com".into());
    p.drive_folder_id = Some("A".into());
    p.conditions = Conditions::none();
    p.storage_mode = StorageMode::Files;
    p
}

fn file(key: &str) -> BaselineEntry {
    BaselineEntry {
        key: key.into(),
        local_rel: key.into(),
        is_dir: false,
        local_size: 3,
        local_mtime: 1_700_000_000_000,
        local_hash: None,
        remote_id: Some(format!("id-{key}")),
        remote_md5: Some("acbd18db4cc2f85cedef654fccc4a4d8".into()),
        remote_size: Some(3),
        remote_mtime: Some(1_700_000_000_000),
        remote_version: Some(4),
        export_mime: None,
    }
}

fn run(id: &str, status: RunStatus, started: i64) -> ImportedRun {
    ImportedRun {
        run: RunRecord {
            id: id.into(),
            pair_id: "p".into(),
            trigger: "schedule".into(),
            status,
            stats: RunStats {
                uploaded: 2,
                ..RunStats::default()
            },
            message: None,
            started_at: started,
            ended_at: (status != RunStatus::Running).then_some(started + 1000),
        },
        events: vec![RunEvent {
            at: started + 10,
            level: EventLevel::Info,
            action: "UPLOAD".into(),
            key: Some("a.txt".into()),
            message: "Uploaded".into(),
            detail: None,
            bytes: 3,
        }],
    }
}

fn conflict(id: &str, resolved: bool) -> ConflictRecord {
    ConflictRecord {
        id: id.into(),
        pair_id: "p".into(),
        key: "b.txt".into(),
        detected_at: 1_700_000_000_000,
        policy: ConflictPolicy::KeepBoth,
        local_copy_rel: Some("b (conflict).txt".into()),
        remote_file_id: None,
        message: "Changed on both sides".into(),
        resolved,
        resolved_at: None,
    }
}

fn bin(id: &str, side: BinSide, key: &str) -> NewBinEntry {
    NewBinEntry {
        id: id.into(),
        pair_id: "p".into(),
        side,
        key: key.into(),
        reason: "DELETED".into(),
        location: "/storage/emulated/0/Android/data/com.almodhayan.drivesync/files/trash/x".into(),
        deleted_at: 1_700_000_000_000,
        expires_at: 1_702_592_000_000,
        size: 5,
        account_id: Some("me@example.com".into()),
        remote_file_id: (side == BinSide::Drive).then(|| "drive-bin-id".into()),
        display_name: Some("old.txt".into()),
        pair_name: Some("Notes".into()),
    }
}

type State = (Vec<BaselineEntry>, Vec<ImportedRun>, Vec<ConflictRecord>);

fn state() -> State {
    (
        vec![file("a.txt"), file("dir/b.txt")],
        vec![
            run("r1", RunStatus::Success, 1_700_000_000_000),
            run("r2", RunStatus::Running, 1_700_000_100_000),
        ],
        vec![conflict("c1", false), conflict("c2", true)],
    )
}

#[test]
fn a_pair_arrives_with_its_baseline_history_conflicts_and_bin_once() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Notes");
    std::fs::create_dir_all(&root).unwrap();
    let e = engine(dir.path());
    e.create_pair(pair(&root)).unwrap();

    let (baseline, runs, conflicts) = state();
    let counts = e
        .import_pair_state("p".into(), baseline, runs, conflicts)
        .unwrap();
    assert_eq!(
        ImportCounts {
            baseline: 2,
            runs: 2,
            conflicts: 1
        },
        counts
    );

    assert_eq!(2, e.baseline("p".into()).unwrap().len());
    let history = e.runs(Some("p".into()), 10).unwrap();
    assert_eq!(
        vec![RunStatus::Failed, RunStatus::Success],
        history.iter().map(|r| r.status).collect::<Vec<_>>(),
        "an unfinished 1.1 run is closed as failed"
    );
    assert_eq!("UPLOAD", e.run_events("r1".into()).unwrap()[0].action);
    assert_eq!(
        vec!["c1".to_string()],
        e.open_conflicts(Some("p".into()))
            .unwrap()
            .into_iter()
            .map(|c| c.id)
            .collect::<Vec<_>>()
    );

    // The bin: a phone file moves into Wasl's bin; a Drive row needs no file.
    let exported = dir.path().join("export-bin-t1");
    std::fs::write(&exported, "older").unwrap();
    assert!(
        e.import_bin_entry(
            bin("t1", BinSide::Local, "dir/old.txt"),
            Some(exported.to_string_lossy().into_owned())
        )
        .unwrap()
    );
    assert!(
        e.import_bin_entry(bin("t2", BinSide::Drive, "gone.txt"), None)
            .unwrap()
    );
    assert!(!exported.exists(), "the file moved");
    let t1 = e.bin_entry("t1".into()).unwrap().unwrap();
    assert_eq!("1700000000000_import_old.txt", t1.entry.location);
    assert_eq!(
        "older",
        std::fs::read_to_string(dir.path().join("bin/p").join(&t1.entry.location)).unwrap()
    );
    assert_eq!("", e.bin_entry("t2".into()).unwrap().unwrap().entry.location);

    // Twice changes nothing.
    let (baseline, runs, conflicts) = state();
    assert_eq!(
        ImportCounts::default(),
        e.import_pair_state("p".into(), baseline, runs, conflicts)
            .unwrap()
    );
    assert!(
        !e.import_bin_entry(bin("t1", BinSide::Local, "dir/old.txt"), None)
            .unwrap()
    );
    assert_eq!(2, e.runs(Some("p".into()), 10).unwrap().len());
    assert_eq!(2, e.bin_entries(Some("p".into())).unwrap().len());
}

#[tokio::test]
async fn an_imported_bin_file_restores_to_its_folder() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Notes");
    std::fs::create_dir_all(&root).unwrap();
    let e = engine(dir.path());
    e.create_pair(pair(&root)).unwrap();
    let exported = dir.path().join("x");
    std::fs::write(&exported, "older").unwrap();
    e.import_bin_entry(
        bin("t1", BinSide::Local, "dir/old.txt"),
        Some(exported.to_string_lossy().into_owned()),
    )
    .unwrap();

    let r = e.restore_bin_entries(vec!["t1".into()]).await.unwrap();
    assert_eq!(1, r.restored.len(), "{:?}", r.failed);
    assert_eq!(
        "older",
        std::fs::read_to_string(root.join("dir/old.txt")).unwrap()
    );
}

#[test]
fn a_phone_bin_entry_without_its_file_is_refused_and_unknown_pairs_too() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path());
    let root = dir.path().join("Notes");
    std::fs::create_dir_all(&root).unwrap();
    assert!(matches!(
        e.import_pair_state("p".into(), vec![], vec![], vec![]),
        Err(WaslError::NotFound { .. })
    ));
    e.create_pair(pair(&root)).unwrap();
    assert!(matches!(
        e.import_bin_entry(bin("t1", BinSide::Local, "a.txt"), None),
        Err(WaslError::NotFound { .. })
    ));
    assert!(e.bin_entry("t1".into()).unwrap().is_none());
}

#[test]
fn ids_that_would_leave_the_bin_folder_and_foreign_drive_rows_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path());
    let root = dir.path().join("Notes");
    std::fs::create_dir_all(&root).unwrap();
    let mut evil = pair(&root);
    evil.id = "../escape".into();
    assert!(
        matches!(e.create_pair(evil.clone()), Err(WaslError::PairRule { .. })),
        "the pair rules refuse it at the door"
    );
    // As a database from before the rules could hold it: the import must refuse it too.
    wasl_store::Store::open(dir.path().join("wasl.db"))
        .unwrap()
        .create_pair(&evil)
        .unwrap();
    let exported = dir.path().join("x");
    std::fs::write(&exported, "older").unwrap();
    let mut b = bin("t1", BinSide::Local, "a.txt");
    b.pair_id = "../escape".into();
    assert!(matches!(
        e.import_bin_entry(b, Some(exported.to_string_lossy().into_owned())),
        Err(WaslError::Permission { .. })
    ));
    assert!(exported.exists(), "nothing moved");
    assert!(matches!(
        e.import_pair_state("../escape".into(), vec![file("a.txt")], vec![], vec![]),
        Err(WaslError::Permission { .. })
    ));

    // Its own folder: two pairs on one folder are refused (pair rules).
    let other = dir.path().join("Other");
    std::fs::create_dir_all(&other).unwrap();
    e.create_pair(pair(&other)).unwrap();
    let mut foreign = bin("t2", BinSide::Drive, "a.txt");
    foreign.account_id = Some("someone@else.com".into());
    assert!(matches!(
        e.import_bin_entry(foreign, None),
        Err(WaslError::Permission { .. })
    ));
}
