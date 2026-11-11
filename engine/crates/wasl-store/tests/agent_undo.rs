//! Agent undo's tables (schema v12, docs/AGENT_UNDO.md §6): the watch list and session snapshots.

use wasl_store::{DEFAULT_WATCH_IGNORES, SnapshotOrigin, SnapshotRow, Store, StoreError};

const V11: [&str; 11] = [
    include_str!("../migrations/0001_initial.sql"),
    include_str!("../migrations/0002_engine.sql"),
    include_str!("../migrations/0003_devices.sql"),
    include_str!("../migrations/0004_vault_versions.sql"),
    include_str!("../migrations/0005_views.sql"),
    include_str!("../migrations/0006_vaults_desktop.sql"),
    include_str!("../migrations/0007_vault_orphans.sql"),
    include_str!("../migrations/0008_agent_changesets.sql"),
    include_str!("../migrations/0009_agent_integrity.sql"),
    include_str!("../migrations/0010_introductions.sql"),
    include_str!("../migrations/0011_oplog.sql"),
];

fn session(s: &Store, id: &str, watch_id: &str, started_at: i64) {
    s.connection()
        .execute(
            "INSERT INTO agent_changesets (id, watch_id, actor, scope, status, started_at)
             VALUES (?1, ?2, 'agent:claude-code', '', 'open', ?3)",
            rusqlite::params![id, watch_id, started_at],
        )
        .unwrap();
}

fn row(rel: &str, size: u64, hash: &str) -> SnapshotRow {
    SnapshotRow {
        rel: rel.into(),
        size,
        mtime: 1_000,
        ctime: Some(2_000),
        file_id: Some(size as i64 + 7),
        blake3: Some(hash.into()),
        origin: SnapshotOrigin::Stored,
        location: Some(format!("objects/{}/{hash}", &hash[..2])),
    }
}

/// A database as schema v11 left it, with a pair and two agent sessions, then `extra` (run with
/// foreign keys off).
fn v11(path: &std::path::Path, extra: &str) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    for sql in V11 {
        conn.execute_batch(sql).unwrap();
    }
    conn.pragma_update(None, "user_version", 11).unwrap();
    conn.execute_batch(
        "INSERT INTO pairs (id, name, local_root, route, reach, method, conflict_policy)
             VALUES ('p', 'Notes', '/d', 'drive', 'lan', 'TWO_WAY', 'KEEP_BOTH');
         INSERT INTO agent_changesets (id, pair_id, actor, label, scope, session, status, started_at, ended_at, decided_at, decided_by)
             VALUES ('19a-1', 'p', 'agent:claude-code', 'Tidy docs', 'docs', 's-1', 'held', 1, 2, NULL, NULL),
                    ('19a-2', 'p', 'agent:gemini', NULL, '', NULL, 'accepted', 3, 4, 5, 'desktop');
         INSERT INTO agent_manifest (changeset_id, rel, is_dir, size, mtime)
             VALUES ('19a-1', 'docs/a.md', 0, 3, 1);
         INSERT INTO agent_entries (changeset_id, rel, key, kind, is_dir, after_size, after_mtime, source)
             VALUES ('19a-1', 'docs/a.md', 'docs/a.md', 'changed', 0, 4, 2, 'saved');",
    )
    .unwrap();
    conn.pragma_update(None, "foreign_keys", "OFF").unwrap();
    conn.execute_batch(extra).unwrap();
}

fn changesets(c: &rusqlite::Connection) -> Vec<String> {
    c.prepare(
        "SELECT id || ' ' || pair_id || ' ' || ifnull(watch_id, '-') || ' ' || actor || ' ' || ifnull(label, '-')
                || ' ' || scope || ' ' || ifnull(session, '-') || ' ' || status || ' ' || started_at || ' '
                || ifnull(ended_at, '-') || ' ' || ifnull(decided_at, '-') || ' ' || ifnull(decided_by, '-')
         FROM agent_changesets ORDER BY id",
    )
    .unwrap()
    .query_map([], |r| r.get(0))
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

#[test]
fn a_v11_database_keeps_its_agent_changesets_and_their_files_in_v12() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wasl.db");
    v11(&path, "");
    let s = Store::open(&path).unwrap();
    assert_eq!(12, s.schema_version().unwrap());
    let c = s.connection();
    assert_eq!(
        vec![
            "19a-1 p - agent:claude-code Tidy docs docs s-1 held 1 2 - -",
            "19a-2 p - agent:gemini -  - accepted 3 4 5 desktop"
        ],
        changesets(c)
    );
    // The rebuild dropped the old table with foreign keys off: nothing cascaded.
    let files: i64 = c
        .query_row(
            "SELECT (SELECT count(*) FROM agent_manifest) + (SELECT count(*) FROM agent_entries)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(2, files);
    // Foreign keys are on again, and still point at the rebuilt table.
    assert!(
        c.execute(
            "INSERT INTO agent_entries (changeset_id, rel, key, kind, is_dir, source) VALUES ('nope', 'x', 'x', 'added', 0, 'none')",
            [],
        )
        .is_err()
    );
    c.execute("DELETE FROM pairs WHERE id = 'p'", []).unwrap();
    let left: i64 = c
        .query_row("SELECT count(*) FROM agent_entries", [], |r| r.get(0))
        .unwrap();
    assert_eq!(0, left);
}

/// A row elsewhere that already pointed at a missing row is not v12's doing: the store still opens.
#[test]
fn a_v11_database_with_an_unrelated_orphan_row_migrates() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wasl.db");
    v11(
        &path,
        "INSERT INTO vault_versions (pair_id, file_id, version) VALUES ('gone', 'f', 1);",
    );
    let s = Store::open(&path).unwrap();
    assert_eq!(12, s.schema_version().unwrap());
    assert_eq!(2, changesets(s.connection()).len());
}

#[test]
fn a_failing_v12_leaves_the_database_at_v11_with_its_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wasl.db");
    v11(&path, "CREATE TABLE agent_watches (id TEXT);");
    assert!(Store::open(&path).is_err());
    let conn = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(11, version);
    let rows: i64 = conn
        .query_row(
            "SELECT (SELECT count(*) FROM agent_changesets) + (SELECT count(*) FROM agent_entries)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(3, rows);
}

#[test]
fn a_changeset_is_in_a_pair_or_a_watched_folder_never_both_or_neither() {
    let mut s = Store::open_in_memory().unwrap();
    s.add_watch("w1", "/home/me/site", 1).unwrap();
    let c = s.connection();
    c.execute_batch(
        "INSERT INTO pairs (id, name, local_root, route, reach, method, conflict_policy)
             VALUES ('p', 'Notes', '/d', 'drive', 'lan', 'TWO_WAY', 'KEEP_BOTH');",
    )
    .unwrap();
    let insert = |pair: Option<&str>, watch: Option<&str>| {
        c.execute(
            "INSERT INTO agent_changesets (id, pair_id, watch_id, actor, scope, status, started_at)
             VALUES (?1, ?2, ?3, 'agent:x', '', 'open', 1)",
            rusqlite::params![format!("{pair:?}{watch:?}"), pair, watch],
        )
    };
    assert!(insert(Some("p"), None).is_ok());
    assert!(insert(None, Some("w1")).is_ok());
    assert!(insert(Some("p"), Some("w1")).is_err());
    assert!(insert(None, None).is_err());
}

#[test]
fn the_watch_list_adds_lists_and_removes_folders_with_their_default_ignores() {
    let mut s = Store::open_in_memory().unwrap();
    let w = s.add_watch("w1", "/home/me/site", 10).unwrap();
    assert_eq!(DEFAULT_WATCH_IGNORES.map(String::from).to_vec(), w.ignores);
    s.add_watch("w2", "/home/me/api", 11).unwrap();
    assert!(matches!(
        s.add_watch("w3", "/home/me/site", 12),
        Err(StoreError::Refused(_))
    ));
    let roots: Vec<String> = s.watches().unwrap().into_iter().map(|w| w.root).collect();
    assert_eq!(vec!["/home/me/api", "/home/me/site"], roots);

    s.set_watch_ignores("w1", &["target/".into()]).unwrap();
    assert_eq!(
        vec!["target/"],
        s.watch_at("/home/me/site").unwrap().unwrap().ignores
    );
    s.set_watch_ignores("w1", &[]).unwrap();
    assert!(s.watch_at("/home/me/site").unwrap().unwrap().ignores.is_empty());
    assert!(s.set_watch_ignores("w1", &["a\nb".into()]).is_err());

    assert!(s.remove_watch("w1", 20).unwrap());
    assert!(!s.remove_watch("w1", 21).unwrap());
    assert_eq!(None, s.watch_at("/home/me/site").unwrap());
    // Watching it again is a new watch; the old one stays for its sessions.
    assert_eq!("w4", s.add_watch("w4", "/home/me/site", 30).unwrap().id);
}

#[test]
fn the_last_snapshot_of_a_watch_is_the_newest_sessions_and_goes_with_its_session() {
    let mut s = Store::open_in_memory().unwrap();
    s.add_watch("w1", "/home/me/site", 1).unwrap();
    s.add_watch("w2", "/home/me/api", 1).unwrap();
    assert!(s.last_snapshot("w1").unwrap().is_empty());
    session(&s, "c1", "w1", 100);
    session(&s, "c2", "w2", 200);
    let old = vec![row("a.txt", 1, "aa11"), row("b/c.txt", 2, "bb22")];
    s.put_snapshot("c1", "w1", &old).unwrap();
    let mut clone = row("a.txt", 3, "cc33");
    clone.blake3 = None;
    clone.origin = SnapshotOrigin::Cloned;
    clone.location = Some("clones/c2/0".into());
    let mut synced = row("z.txt", 4, "dd44");
    synced.origin = SnapshotOrigin::Synced;
    synced.location = None;
    let new = vec![clone, synced];
    // A pair session (c2) covers every watched folder too.
    s.put_snapshot("c2", "w1", &new).unwrap();
    assert_eq!(new, s.last_snapshot("w1").unwrap());
    assert_eq!(old, s.snapshot("c1", "w1").unwrap());
    assert!(s.last_snapshot("w2").unwrap().is_empty());
    // Recording again replaces.
    s.put_snapshot("c1", "w1", &old[..1]).unwrap();
    assert_eq!(1, s.snapshot("c1", "w1").unwrap().len());
    s.connection()
        .execute("DELETE FROM agent_changesets WHERE id = 'c2'", [])
        .unwrap();
    assert_eq!(old[..1].to_vec(), s.last_snapshot("w1").unwrap());
}
