//! Schema, migrations and the baseline round trip (SRS §6, FR-B3).

use proptest::prelude::*;
use wasl_core::{BaselineEntry, Reach, Route};
use wasl_store::{Pair, Store, StoreError};

fn pair(id: &str) -> Pair {
    let mut p = Pair::new(id, &format!("Pair {id}"), "/storage/emulated/0/Documents");
    p.drive_folder_id = Some("root-folder".into());
    p
}

fn store_with(pairs: &[&str]) -> Store {
    let s = Store::open_in_memory().unwrap();
    for p in pairs {
        s.create_pair(&pair(p)).unwrap();
    }
    s
}

fn entry(key: &str) -> BaselineEntry {
    BaselineEntry {
        key: key.into(),
        local_rel: key.into(),
        is_dir: false,
        local_size: 1234,
        local_mtime: 1_700_000_000_123,
        local_hash: Some("d41d8cd98f00b204e9800998ecf8427e".into()),
        remote_id: Some("1AbC".into()),
        remote_md5: Some("d41d8cd98f00b204e9800998ecf8427e".into()),
        remote_size: Some(1234),
        remote_mtime: Some(1_700_000_000_456),
        remote_version: Some(7),
        export_mime: None,
    }
}

const TABLES: [&str; 43] = [
    "accounts",
    "agent_changesets",
    "agent_entries",
    "agent_journal",
    "agent_journal_stats",
    "agent_manifest",
    "agent_markers",
    "agent_restores",
    "agent_saved",
    "agent_session_watches",
    "agent_snapshot_failures",
    "agent_snapshots",
    "agent_watch_gaps",
    "agent_watches",
    "blocked_plans",
    "chunk_lists",
    "chunks",
    "conflicts",
    "device_index",
    "device_index_meta",
    "device_notes",
    "devices",
    "drive_nodes",
    "file_state",
    "ignore_hits",
    "oplog_changesets",
    "oplog_ops",
    "pair_devices",
    "pair_share_devices",
    "pair_shares",
    "pairs",
    "peer_progress",
    "pins",
    "received_offers",
    "run_events",
    "runs",
    "settings",
    "trash",
    "upload_sessions",
    "vault_orphans",
    "vault_versions",
    "vaults",
    "views",
];

#[test]
fn migration_creates_every_table_of_the_srs() {
    let s = Store::open_in_memory().unwrap();
    assert_eq!(12, s.schema_version().unwrap());
    let mut stmt = s
        .connection()
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .unwrap();
    let names: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(TABLES.to_vec(), names);
}

#[test]
fn reopening_a_file_keeps_data_and_does_not_migrate_twice() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wasl.db");
    {
        let mut s = Store::open(&path).unwrap();
        s.create_pair(&pair("p")).unwrap();
        s.put_baseline("p", &entry("a.txt")).unwrap();
    }
    let s = Store::open(&path).unwrap();
    assert_eq!(12, s.schema_version().unwrap());
    assert_eq!(Some(&entry("a.txt")), s.load_baseline("p").unwrap().get("a.txt"));
}

#[test]
fn a_database_from_a_newer_build_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wasl.db");
    drop(Store::open(&path).unwrap());
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
    }
    match Store::open(&path) {
        Err(StoreError::TooNew { found: 99, known: 12 }) => {}
        other => panic!("expected TooNew, got {:?}", other.err()),
    }
}

#[test]
fn baseline_round_trips_files_folders_and_export_mirrors() {
    let mut s = store_with(&["p"]);
    let dir = BaselineEntry {
        is_dir: true,
        local_size: 0,
        local_hash: None,
        remote_md5: None,
        remote_size: None,
        remote_version: None,
        ..entry("Photos")
    };
    let mapped = BaselineEntry {
        local_rel: "a？b.txt".into(),
        ..entry("a?b.txt")
    };
    let export = BaselineEntry {
        local_rel: "notes.docx".into(),
        remote_md5: None,
        export_mime: Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document".into()),
        ..entry("notes")
    };
    let never_on_drive = BaselineEntry {
        remote_id: None,
        remote_md5: None,
        remote_size: None,
        remote_mtime: None,
        remote_version: None,
        ..entry("local-only.txt")
    };
    for e in [&dir, &mapped, &export, &never_on_drive] {
        s.put_baseline("p", e).unwrap();
    }
    let loaded = s.load_baseline("p").unwrap();
    assert_eq!(4, loaded.len());
    for e in [dir, mapped, export, never_on_drive] {
        assert_eq!(Some(&e), loaded.get(&e.key), "{}", e.key);
    }
}

#[test]
fn put_replaces_and_clears_a_side_that_is_gone() {
    let mut s = store_with(&["p"]);
    s.put_baseline("p", &entry("a.txt")).unwrap();
    let local_only = BaselineEntry {
        local_size: 99,
        remote_id: None,
        remote_md5: None,
        remote_size: None,
        remote_mtime: None,
        remote_version: None,
        ..entry("a.txt")
    };
    s.put_baseline("p", &local_only).unwrap();
    assert_eq!(Some(&local_only), s.load_baseline("p").unwrap().get("a.txt"));
}

#[test]
fn remove_forgets_every_side() {
    let mut s = store_with(&["p"]);
    s.put_baseline("p", &entry("a.txt")).unwrap();
    s.put_baseline("p", &entry("b.txt")).unwrap();
    s.remove_baseline("p", "a.txt").unwrap();
    let keys: Vec<String> = s.load_baseline("p").unwrap().into_keys().collect();
    assert_eq!(vec!["b.txt"], keys);
    let rows: i64 = s
        .connection()
        .query_row("SELECT COUNT(*) FROM file_state", [], |r| r.get(0))
        .unwrap();
    assert_eq!(2, rows, "b.txt keeps its local and drive rows");
}

#[test]
fn pairs_are_isolated_and_deleting_a_pair_cascades() {
    let mut s = store_with(&["p", "q"]);
    s.put_baselines("p", &[entry("a.txt"), entry("b.txt")]).unwrap();
    s.put_baseline("q", &entry("a.txt")).unwrap();
    assert_eq!(2, s.load_baseline("p").unwrap().len());
    assert_eq!(1, s.load_baseline("q").unwrap().len());
    s.delete_pair("p").unwrap();
    assert_eq!(None, s.get_pair("p").unwrap());
    assert!(s.load_baseline("p").unwrap().is_empty());
    assert_eq!(1, s.load_baseline("q").unwrap().len());
}

#[test]
fn baseline_needs_an_existing_pair() {
    let mut s = store_with(&[]);
    assert!(
        s.put_baseline("missing", &entry("a.txt")).is_err(),
        "foreign keys are enforced"
    );
}

#[test]
fn a_failed_batch_writes_nothing() {
    let mut s = store_with(&["p"]);
    let bad = BaselineEntry {
        local_mtime: i64::MAX,
        ..entry("bad.txt")
    };
    assert!(matches!(
        s.put_baselines("p", &[entry("good.txt"), bad]),
        Err(StoreError::TimeOutOfRange(_))
    ));
    assert!(
        s.load_baseline("p").unwrap().is_empty(),
        "the good row was rolled back with the bad one"
    );
}

#[test]
fn route_and_reach_constraints_hold() {
    let s = store_with(&[]);
    let err = s.connection().execute(
        "INSERT INTO pairs (id, name, local_root, route, reach, method, conflict_policy) VALUES ('x','x','/','cloud','lan','TWO_WAY','KEEP_BOTH')",
        [],
    );
    assert!(err.is_err());
    let mut both = pair("ok");
    both.route = Route::DriveAndDevices;
    both.reach = Reach::LanAndInternet;
    s.create_pair(&both).unwrap();
}

fn arb_entry() -> impl Strategy<Value = BaselineEntry> {
    let text = "[a-zA-Z0-9 ._?:/\u{00e9}\u{0627}-\u{064a}]{1,24}";
    // Milliseconds that fit the nanosecond column (about years 1678 to 2262).
    let ms = -9_000_000_000_000i64..9_000_000_000_000i64;
    (
        (
            text,
            text,
            any::<bool>(),
            0u64..i64::MAX as u64,
            ms.clone(),
            proptest::option::of("[0-9a-f]{32}"),
        ),
        (
            proptest::option::of("[A-Za-z0-9_-]{10,40}"),
            proptest::option::of("[0-9a-f]{32}"),
            proptest::option::of(0u64..i64::MAX as u64),
            proptest::option::of(ms),
            proptest::option::of(any::<i64>()),
            proptest::option::of("[a-z/.+-]{5,40}"),
        ),
    )
        .prop_map(
            |((key, local_rel, is_dir, local_size, local_mtime, local_hash), r)| BaselineEntry {
                key,
                local_rel,
                is_dir,
                local_size,
                local_mtime,
                local_hash,
                remote_id: r.0,
                remote_md5: r.1,
                remote_size: r.2,
                remote_mtime: r.3,
                remote_version: r.4,
                export_mime: r.5,
            },
        )
}

proptest! {
    #[test]
    fn any_baseline_entry_round_trips(e in arb_entry()) {
        let mut s = store_with(&["p"]);
        s.put_baseline("p", &e).unwrap();
        let loaded = s.load_baseline("p").unwrap();
        prop_assert_eq!(Some(&e), loaded.get(&e.key));
    }
}

/// Schema v9 (security review 2026-10): a saved copy keeps its hash, and an accept records who.
#[test]
fn agent_copies_keep_their_hash_and_accepts_their_client() {
    use wasl_store::{Changeset, ChangesetStatus, SavedCopy};
    let mut s = store_with(&["p"]);
    let cs = Changeset {
        id: "cs".into(),
        pair_id: Some("p".into()),
        actor: "agent:claude-code".into(),
        label: None,
        scope: String::new(),
        session: None,
        status: ChangesetStatus::Open,
        started_at: 1,
        ended_at: None,
        decided_at: None,
    };
    s.create_changeset(&cs, &[]).unwrap();
    let copy = SavedCopy {
        rel: "a.txt".into(),
        location: "x".into(),
        size: 2,
        mtime: 3,
        blake3: Some("ab".repeat(32)),
    };
    assert!(s.put_saved("cs", &copy).unwrap());
    assert_eq!(Some(copy.clone()), s.saved_copy("cs", "a.txt").unwrap());
    assert_eq!(vec![copy], s.saved("cs").unwrap());
    assert_eq!(None, s.saved_copy("cs", "b.txt").unwrap());

    assert_eq!(None, s.changeset_decided_by("cs").unwrap());
    s.decide_changeset_by("cs", ChangesetStatus::Accepted, 4, Some("cli"))
        .unwrap();
    assert_eq!(Some("cli".into()), s.changeset_decided_by("cs").unwrap());
}

/// Security review M1 (#134): the store itself refuses to drop a pair whose agent changeset is
/// open or held, in the same statement as the delete, so no request can slip a changeset in between
/// a check and the delete. Settled changesets go with the pair as before.
#[test]
fn a_pair_with_an_open_or_held_changeset_is_not_deleted() {
    let s = store_with(&["p"]);
    let add = |id: &str, status: &str| {
        s.connection()
            .execute(
                "INSERT INTO agent_changesets (id, pair_id, actor, scope, status, started_at)
                 VALUES (?1, 'p', 'agent:claude-code', '', ?2, 1)",
                rusqlite::params![id, status],
            )
            .unwrap();
    };
    for status in ["open", "held"] {
        add(status, status);
        let e = s.delete_pair("p").unwrap_err();
        assert!(matches!(e, StoreError::Refused(_)), "{status}: {e}");
        assert!(s.get_pair("p").unwrap().is_some(), "{status}");
        s.decide_changeset(status, wasl_store::ChangesetStatus::Reverted, 2)
            .unwrap();
    }
    s.delete_pair("p").unwrap();
    assert_eq!(None, s.get_pair("p").unwrap());
    assert!(s.changesets(None, true).unwrap().is_empty());
}
