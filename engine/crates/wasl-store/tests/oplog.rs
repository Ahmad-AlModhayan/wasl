//! The op-log (docs/adr/0002-one-op-log.md, SRS FR-OL1–OL6): schema v11, the moves and outcomes it
//! allows, actors, entity URIs, pruning and the one-time import.

use std::collections::BTreeMap;

use proptest::prelude::*;
use serde_json::json;
use wasl_store::{
    Decision, ImportOp, NewChangeset, NewOp, OPLOG_HISTORY_MS, OpOutcome, OplogChangeset, OplogError,
    OplogFilter, OplogKey, OplogStatus, Store, StoreError, file_uri, file_uri_parts, folder_uri,
};

/// A device id in the form `DeviceId` writes (the store checks the shape only).
const DEV: &str = "WASL-0123-4567-89AB-CDEF-GHJK-MNPQ-RSTV";
const AGENT: &str = "agent:claude-code";
const T0: i64 = 1_790_000_000_000;

fn person() -> String {
    format!("person:{DEV}")
}

fn key(id: &str) -> OplogKey {
    OplogKey::new(DEV, id)
}

fn new_changeset(id: &str) -> NewChangeset {
    NewChangeset {
        key: key(id),
        actor: AGENT.into(),
        label: Some("tidy the notes".into()),
        scope: folder_uri("p", ""),
        opened_at: T0,
    }
}

fn file_state(size: u64) -> serde_json::Value {
    json!({"kind": "file", "size": size, "mtime_ms": T0, "blake3": "ab"})
}

fn create(name: &str) -> NewOp {
    NewOp {
        entity_uri: file_uri("p", name),
        op: "create".into(),
        prev: None,
        next: Some(file_state(1)),
        at: T0 + 1,
    }
}

fn update(name: &str) -> NewOp {
    NewOp {
        entity_uri: file_uri("p", name),
        op: "update".into(),
        prev: Some(file_state(1)),
        next: Some(file_state(2)),
        at: T0 + 1,
    }
}

fn decision(actor: &str) -> Decision {
    Decision {
        by: "cli".into(),
        actor: actor.into(),
        at: T0 + 10,
    }
}

/// A changeset opened with `n` operations.
fn opened(s: &mut Store, id: &str, n: usize) -> OplogKey {
    s.oplog_open(&new_changeset(id)).unwrap();
    for i in 0..n {
        s.oplog_append(&key(id), &update(&format!("f{i}.txt"))).unwrap();
    }
    key(id)
}

/// A changeset accepted with `n` operations.
fn committed(s: &mut Store, id: &str, n: usize) -> OplogKey {
    let k = opened(s, id, n);
    s.oplog_end(&k, OplogStatus::Staged, T0 + 2).unwrap();
    s.oplog_decide(&k, OplogStatus::Committed, &decision(&person()))
        .unwrap();
    k
}

/// A changeset written by the import in any state, one operation per outcome.
fn imported(s: &mut Store, id: &str, status: OplogStatus, outcomes: &[Option<OpOutcome>]) -> OplogKey {
    let cs = OplogChangeset {
        origin_device: DEV.into(),
        id: id.into(),
        actor: AGENT.into(),
        label: None,
        scope: folder_uri("p", "docs"),
        status,
        opened_at: T0,
        closed_at: (status != OplogStatus::Open).then_some(T0 + 2),
        decided_at: (!matches!(status, OplogStatus::Open | OplogStatus::Staged)).then_some(T0 + 3),
        decided_by: Some("desktop".into()),
        decided_actor: Some(person()),
        imported_at: None,
    };
    let ops: Vec<ImportOp> = outcomes
        .iter()
        .enumerate()
        .map(|(i, &outcome)| ImportOp {
            op: update(&format!("docs/f{i}.txt")),
            reverts: None,
            outcome,
        })
        .collect();
    s.oplog_import(&cs, &ops, T0 + 100).unwrap();
    key(id)
}

fn refused(r: Result<impl std::fmt::Debug, StoreError>) -> OplogError {
    match r {
        Err(StoreError::Oplog(e)) => e,
        other => panic!("expected an op-log refusal, got {other:?}"),
    }
}

/// Every op-log row, by column, so a test can see that nothing changed.
#[derive(Debug, Clone, PartialEq)]
struct Snap {
    /// key → (status, the other columns)
    changesets: BTreeMap<String, (OplogStatus, String)>,
    /// (key, seq) → (the columns that never change, outcome)
    ops: BTreeMap<(String, u32), (String, Option<OpOutcome>)>,
}

fn snap(s: &Store) -> Snap {
    let c = s.connection();
    let mut stmt = c
        .prepare(
            "SELECT origin_device || '/' || id, status, actor, label, scope, opened_at, closed_at,
                    decided_at, decided_by, decided_actor, imported_at FROM oplog_changesets",
        )
        .unwrap();
    let changesets = stmt
        .query_map([], |r| {
            let rest: Vec<rusqlite::types::Value> = (2..11).map(|i| r.get(i)).collect::<Result<_, _>>()?;
            let status: String = r.get(1)?;
            Ok((
                r.get::<_, String>(0)?,
                (OplogStatus::parse(&status).unwrap(), format!("{rest:?}")),
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut stmt = c
        .prepare(
            "SELECT origin_device || '/' || changeset_id, seq, entity_uri, op, prev, next, reverts, at, outcome
             FROM oplog_ops",
        )
        .unwrap();
    let ops = stmt
        .query_map([], |r| {
            let fixed: Vec<rusqlite::types::Value> = (2..8).map(|i| r.get(i)).collect::<Result<_, _>>()?;
            let outcome: Option<String> = r.get(8)?;
            Ok((
                (r.get::<_, String>(0)?, r.get::<_, u32>(1)?),
                (
                    format!("{fixed:?}"),
                    outcome.map(|o| OpOutcome::parse(&o).unwrap()),
                ),
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    Snap { changesets, ops }
}

// ---------------------------------------------------------------- schema

/// Every row of every table, and each table's SQL, as text.
fn dump(c: &rusqlite::Connection) -> BTreeMap<String, (String, Vec<String>)> {
    let tables: Vec<(String, String)> = c
        .prepare("SELECT name, sql FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    tables
        .into_iter()
        .map(|(name, sql)| {
            let mut stmt = c.prepare(&format!("SELECT * FROM \"{name}\"")).unwrap();
            let n = stmt.column_count();
            let mut rows: Vec<String> = stmt
                .query_map([], |r| {
                    let v: Vec<rusqlite::types::Value> =
                        (0..n).map(|i| r.get(i)).collect::<Result<_, _>>()?;
                    Ok(format!("{v:?}"))
                })
                .unwrap()
                .map(Result::unwrap)
                .collect();
            rows.sort();
            (name, (sql, rows))
        })
        .collect()
}

/// Every column agent_changesets had before v12 rebuilt it, row by row.
fn changeset_rows(c: &rusqlite::Connection) -> Vec<String> {
    c.prepare(
        "SELECT quote(id) || quote(pair_id) || quote(actor) || quote(label) || quote(scope) || quote(session)
                || quote(status) || quote(started_at) || quote(ended_at) || quote(decided_at) || quote(decided_by)
         FROM agent_changesets ORDER BY id",
    )
    .unwrap()
    .query_map([], |r| r.get(0))
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

#[test]
fn a_v10_database_migrates_to_v11_with_its_data_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wasl.db");
    let before = {
        // A database exactly as schema v10 left it, with agent changes and introductions in it.
        let conn = rusqlite::Connection::open(&path).unwrap();
        for sql in [
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
        ] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 10).unwrap();
        conn.execute_batch(
            "INSERT INTO pairs (id, name, local_root, route, reach, method, conflict_policy)
                 VALUES ('p', 'Notes', '/d', 'drive', 'lan', 'TWO_WAY', 'KEEP_BOTH');
             INSERT INTO agent_changesets (id, pair_id, actor, label, scope, session, status, started_at, ended_at, decided_at, decided_by)
                 VALUES ('19a-1', 'p', 'agent:claude-code', 'Tidy docs', 'docs', 's-1', 'partly_reverted', 1, 2, 3, 'desktop'),
                        ('19a-2', 'p', 'agent:gemini', NULL, '', NULL, 'held', 4, 5, NULL, NULL);
             INSERT INTO agent_entries (changeset_id, rel, key, kind, is_dir, after_size, after_mtime, source, outcome)
                 VALUES ('19a-1', 'docs/a.md', 'docs/a.md', 'added', 0, 3, 2, 'none', 'reverted'),
                        ('19a-1', 'docs/b.md', 'docs/b.md', 'added', 0, 3, 2, 'none', 'skipped_changed');
             INSERT INTO device_notes (device_id, known) VALUES ('WASL-0123-4567-89AB-CDEF-GHJK-MNPQ-RSTV', 1);
             INSERT INTO received_offers (share, from_device, name, files, bytes, first_seen, state)
                 VALUES ('ab', 'WASL-0123-4567-89AB-CDEF-GHJK-MNPQ-RSTV', 'Photos', 2, 10, 7, 'new');",
        )
        .unwrap();
        (dump(&conn), changeset_rows(&conn))
    };
    let (before, changesets_before) = before;
    let s = Store::open(&path).unwrap();
    assert_eq!(12, s.schema_version().unwrap());
    assert_eq!(changesets_before, changeset_rows(s.connection()));
    let mut after = dump(s.connection());
    let changesets = after.remove("oplog_changesets").unwrap();
    let ops = after.remove("oplog_ops").unwrap();
    // Schema v12 (agent undo) adds its own tables and rebuilds agent_changesets; tests/agent_undo.rs
    // checks that its rows come through.
    for t in [
        "agent_watches",
        "agent_snapshots",
        "agent_journal",
        "agent_markers",
        "agent_restores",
        "agent_snapshot_failures",
        "agent_session_watches",
        "agent_watch_gaps",
        "agent_journal_stats",
    ] {
        assert!(after.remove(t).unwrap().1.is_empty(), "{t}");
    }
    // agent_changesets gained a column; its rows are compared above.
    let mut before = before;
    before.remove("agent_changesets");
    after.remove("agent_changesets");
    assert_eq!(before, after);
    assert!(changesets.1.is_empty() && ops.1.is_empty());
}

#[test]
fn op_log_rows_cannot_be_rewritten_even_with_sql() {
    let mut s = Store::open_in_memory().unwrap();
    committed(&mut s, "1", 1);
    let c = s.connection();
    assert!(
        c.execute("UPDATE oplog_ops SET entity_uri = 'file://p/x'", [])
            .is_err()
    );
    assert!(c.execute("UPDATE oplog_ops SET prev = NULL", []).is_err());
    assert!(
        c.execute("UPDATE oplog_changesets SET actor = 'agent:other'", [])
            .is_err()
    );
    assert!(
        c.execute("UPDATE oplog_ops SET outcome = 'nonsense'", [])
            .is_err()
    );
}

// ---------------------------------------------------------------- actors and entity URIs

#[test]
fn agents_and_people_are_accepted_as_actors() {
    let mut s = Store::open_in_memory().unwrap();
    let long = format!("agent:{}", "a".repeat(64));
    let person = person();
    for (i, actor) in [
        "agent:claude-code",
        "agent:gemini",
        "agent:qasd-planner",
        "agent:gpt.5_mini",
        "agent:0",
        long.as_str(),
        person.as_str(),
    ]
    .into_iter()
    .enumerate()
    {
        let mut cs = new_changeset(&i.to_string());
        cs.actor = actor.into();
        s.oplog_open(&cs).unwrap();
        assert_eq!(actor, s.oplog_changeset(&cs.key).unwrap().unwrap().actor);
    }
}

#[test]
fn anything_else_is_refused_as_an_actor() {
    let mut s = Store::open_in_memory().unwrap();
    let long = format!("agent:{}", "a".repeat(65));
    for actor in [
        "",
        "agent:",
        "agent:Claude",
        "agent:claude code",
        "agent:claude/code",
        "agent:ذكاء",
        long.as_str(),
        "claude-code",
        "Agent:claude-code",
        "user:ahmad",
        "person:",
        "person:ahmad",
        "person:wasl-0123-4567-89ab-cdef-ghjk-mnpq-rstv",
        "person:WASL-0123-4567-89AB-CDEF-GHJK-MNPQ-RSTI",
        "person:WASL-0123-4567-89AB-CDEF-GHJK-MNPQ",
        "person:WASL0123456789ABCDEFGHJKMNPQRSTV",
    ] {
        let mut cs = new_changeset("1");
        cs.actor = actor.into();
        assert_eq!(
            OplogError::BadActor(actor.into()),
            refused(s.oplog_open(&cs)),
            "{actor:?}"
        );
    }
    // A decision's actor too.
    let k = opened(&mut s, "2", 1);
    s.oplog_end(&k, OplogStatus::Staged, T0).unwrap();
    let bad = decision("person:me");
    assert_eq!(
        OplogError::BadActor("person:me".into()),
        refused(s.oplog_decide(&k, OplogStatus::Committed, &bad))
    );
    assert_eq!(
        OplogError::BadActor("person:me".into()),
        refused(s.oplog_finish_revert(&k, &bad))
    );
    assert_eq!(snap(&s).changesets.len(), 1);
}

#[test]
fn the_origin_device_must_be_a_device_id() {
    let mut s = Store::open_in_memory().unwrap();
    let mut cs = new_changeset("1");
    cs.key.origin_device = "this-mac".into();
    assert_eq!(
        OplogError::BadDevice("this-mac".into()),
        refused(s.oplog_open(&cs))
    );
}

#[test]
fn an_agent_revert_records_that_agent_as_the_decider() {
    let mut s = Store::open_in_memory().unwrap();
    let k = committed(&mut s, "1", 1);
    s.oplog_revert_op(&k, 1, T0 + 20).unwrap();
    s.oplog_finish_revert(&k, &decision("agent:gemini")).unwrap();
    let cs = s.oplog_changeset(&k).unwrap().unwrap();
    assert_eq!(
        (Some("cli"), Some("agent:gemini")),
        (cs.decided_by.as_deref(), cs.decided_actor.as_deref())
    );
}

#[test]
fn file_keys_with_spaces_hashes_percents_and_arabic_names_round_trip() {
    let mut s = Store::open_in_memory().unwrap();
    let k = opened(&mut s, "1", 0);
    let keys = [
        "notes/a b.md",
        "c#1/d?.txt",
        "100%/e%20f",
        "تقارير/الربع الأول.pdf",
    ];
    for key in keys {
        s.oplog_append(&k, &create(key)).unwrap();
    }
    let ops = s.oplog_ops(&k).unwrap();
    for (op, key) in ops.iter().zip(keys) {
        assert_eq!(file_uri("p", key), op.entity_uri);
        assert_eq!(
            Some(("p".to_owned(), key.to_owned())),
            file_uri_parts(&op.entity_uri)
        );
    }
    assert_eq!("file://p/notes/a%20b.md", ops[0].entity_uri);
    assert_eq!("file://p/c%231/d%3F.txt", ops[1].entity_uri);
    assert_eq!("file://p/100%25/e%2520f", ops[2].entity_uri);
}

#[test]
fn other_schemes_are_stored_byte_for_byte() {
    let mut s = Store::open_in_memory().unwrap();
    let mut cs = new_changeset("1");
    cs.scope = "qasd://plan/2026-11-01".into();
    s.oplog_open(&cs).unwrap();
    let uris = [
        "qasd://block/7f3a?view=day#Fajr",
        "qasd://block/ملاحظة",
        "raqeeb:habit/FAJR-1%zz",
        "urn:x-qasd:block:1",
    ];
    for uri in uris {
        let op = NewOp {
            entity_uri: uri.into(),
            op: "set_title".into(),
            prev: Some(json!({"title": "a"})),
            next: Some(json!({"title": "b"})),
            at: T0,
        };
        s.oplog_append(&cs.key, &op).unwrap();
    }
    let stored: Vec<String> = s
        .oplog_ops(&cs.key)
        .unwrap()
        .into_iter()
        .map(|o| o.entity_uri)
        .collect();
    assert_eq!(uris.to_vec(), stored);
    assert_eq!(
        "qasd://plan/2026-11-01",
        s.oplog_changeset(&cs.key).unwrap().unwrap().scope
    );
}

#[test]
fn malformed_entity_uris_and_file_operations_are_refused() {
    let mut s = Store::open_in_memory().unwrap();
    let k = opened(&mut s, "1", 0);
    for uri in [
        "",
        "notes/a.md",
        "/notes/a.md",
        "1qasd://x",
        "qasd://a b",
        "file:/p/a.md",
        "file://p",
        "file:///a.md",
        "file://p/a b.md",
        "file://p/a%2",
        "file://p/a//b.md",
        "file://p/../b.md",
        "FILE://p/a.md",
    ] {
        let mut op = create("a.md");
        op.entity_uri = uri.into();
        assert_eq!(
            OplogError::BadEntityUri(uri.into()),
            refused(s.oplog_append(&k, &op)),
            "{uri:?}"
        );
    }
    let mut cs = new_changeset("2");
    cs.scope = file_uri("p", "docs");
    assert_eq!(
        OplogError::BadEntityUri(cs.scope.clone()),
        refused(s.oplog_open(&cs))
    );
    for (name, prev, next) in [
        ("create", true, true),
        ("delete", false, true),
        ("update", false, true),
        ("move", true, true),
    ] {
        let op = NewOp {
            entity_uri: file_uri("p", "a.md"),
            op: name.into(),
            prev: prev.then(|| file_state(1)),
            next: next.then(|| file_state(1)),
            at: T0,
        };
        assert!(
            matches!(refused(s.oplog_append(&k, &op)), OplogError::BadOp { .. }),
            "{name}"
        );
    }
    assert!(s.oplog_ops(&k).unwrap().is_empty());
}

// ---------------------------------------------------------------- moves

#[test]
fn a_session_is_staged_then_accepted_with_every_operation_applied() {
    let mut s = Store::open_in_memory().unwrap();
    let k = opened(&mut s, "1", 2);
    assert_eq!(3, s.oplog_append(&k, &create("new.md")).unwrap());
    s.oplog_end(&k, OplogStatus::Staged, T0 + 2).unwrap();
    let cs = s.oplog_changeset(&k).unwrap().unwrap();
    assert_eq!(
        (OplogStatus::Staged, Some(T0 + 2), None),
        (cs.status, cs.closed_at, cs.decided_at)
    );
    assert!(s.oplog_ops(&k).unwrap().iter().all(|o| o.outcome.is_none()));
    s.oplog_decide(&k, OplogStatus::Committed, &decision(&person()))
        .unwrap();
    let cs = s.oplog_changeset(&k).unwrap().unwrap();
    assert_eq!(OplogStatus::Committed, cs.status);
    assert_eq!(
        (Some(T0 + 10), Some("cli"), Some(person().as_str()), None),
        (
            cs.decided_at,
            cs.decided_by.as_deref(),
            cs.decided_actor.as_deref(),
            cs.imported_at
        )
    );
    let ops = s.oplog_ops(&k).unwrap();
    assert_eq!(vec![1, 2, 3], ops.iter().map(|o| o.seq).collect::<Vec<_>>());
    assert!(ops.iter().all(|o| o.outcome == Some(OpOutcome::Applied)));
}

#[test]
fn a_session_with_nothing_to_hold_commits_at_its_end() {
    let mut s = Store::open_in_memory().unwrap();
    let k = opened(&mut s, "1", 2);
    s.oplog_end(&k, OplogStatus::Committed, T0 + 2).unwrap();
    let cs = s.oplog_changeset(&k).unwrap().unwrap();
    assert_eq!(
        (OplogStatus::Committed, Some(T0 + 2), Some(T0 + 2)),
        (cs.status, cs.closed_at, cs.decided_at)
    );
    assert!(
        s.oplog_ops(&k)
            .unwrap()
            .iter()
            .all(|o| o.outcome == Some(OpOutcome::Applied))
    );
}

#[test]
fn a_partial_accept_keeps_the_operations_left_out_discarded() {
    let mut s = Store::open_in_memory().unwrap();
    let k = opened(&mut s, "1", 2);
    s.oplog_end(&k, OplogStatus::Staged, T0 + 2).unwrap();
    s.oplog_set_outcome(&k, 2, OpOutcome::Discarded).unwrap();
    s.oplog_decide(&k, OplogStatus::Committed, &decision(&person()))
        .unwrap();
    let outcomes: Vec<_> = s.oplog_ops(&k).unwrap().into_iter().map(|o| o.outcome).collect();
    assert_eq!(
        vec![Some(OpOutcome::Applied), Some(OpOutcome::Discarded)],
        outcomes
    );
}

#[test]
fn a_discard_marks_every_operation_discarded_and_appends_nothing() {
    let mut s = Store::open_in_memory().unwrap();
    let k = opened(&mut s, "1", 2);
    s.oplog_end(&k, OplogStatus::Staged, T0 + 2).unwrap();
    s.oplog_decide(&k, OplogStatus::Discarded, &decision(&person()))
        .unwrap();
    assert_eq!(
        OplogStatus::Discarded,
        s.oplog_changeset(&k).unwrap().unwrap().status
    );
    let ops = s.oplog_ops(&k).unwrap();
    assert_eq!(2, ops.len());
    assert!(ops.iter().all(|o| o.outcome == Some(OpOutcome::Discarded)));
}

#[test]
fn an_empty_session_is_dropped_and_leaves_no_record() {
    let mut s = Store::open_in_memory().unwrap();
    let k = opened(&mut s, "1", 0);
    assert_eq!(
        OplogError::Empty("1".into()),
        refused(s.oplog_end(&k, OplogStatus::Staged, T0))
    );
    assert_eq!(
        OplogError::Empty("1".into()),
        refused(s.oplog_end(&k, OplogStatus::Committed, T0))
    );
    s.oplog_drop_empty(&k).unwrap();
    assert_eq!(None, s.oplog_changeset(&k).unwrap());
}

#[test]
fn a_changeset_with_operations_is_not_dropped() {
    let mut s = Store::open_in_memory().unwrap();
    let k = opened(&mut s, "1", 1);
    assert_eq!(OplogError::NotEmpty("1".into()), refused(s.oplog_drop_empty(&k)));
    s.oplog_end(&k, OplogStatus::Staged, T0).unwrap();
    assert!(matches!(
        refused(s.oplog_drop_empty(&k)),
        OplogError::NotOpen { .. }
    ));
    assert_eq!(1, s.oplog_ops(&k).unwrap().len());
}

#[test]
fn operations_are_appended_only_while_open() {
    let mut s = Store::open_in_memory().unwrap();
    let k = opened(&mut s, "1", 1);
    s.oplog_end(&k, OplogStatus::Staged, T0).unwrap();
    assert_eq!(
        OplogError::NotOpen {
            id: "1".into(),
            status: OplogStatus::Staged
        },
        refused(s.oplog_append(&k, &create("late.md")))
    );
    assert_eq!(
        OplogError::NoChangeset("9".into()),
        refused(s.oplog_append(&key("9"), &create("a.md")))
    );
}

#[test]
fn the_same_id_from_two_devices_is_two_changesets() {
    let mut s = Store::open_in_memory().unwrap();
    opened(&mut s, "1", 1);
    let mut other = new_changeset("1");
    other.key.origin_device = "WASL-ZZZZ-YYYY-XXXX-WWWW-VVVV-TTTT-SSSS".into();
    s.oplog_open(&other).unwrap();
    s.oplog_append(&other.key, &create("a.md")).unwrap();
    assert_eq!(1, s.oplog_ops(&key("1")).unwrap().len());
    assert_eq!(1, s.oplog_ops(&other.key).unwrap().len());
}

/// Tries to move a changeset that is `from` to `to` through the call that would make that move,
/// and checks it is refused with the right error and changes nothing.
fn refuse_move(from: OplogStatus, to: OplogStatus) {
    use OplogStatus::*;
    let mut s = Store::open_in_memory().unwrap();
    // One operation whose outcome makes a finished revert land on `to`.
    let outcome = if to == PartlyReverted {
        OpOutcome::Applied
    } else {
        OpOutcome::Reverted
    };
    let k = imported(&mut s, "1", from, &[Some(outcome)]);
    let before = snap(&s);
    let not_open = OplogError::NotOpen {
        id: "1".into(),
        status: from,
    };
    let not_staged = OplogError::NotStaged {
        id: "1".into(),
        status: from,
    };
    match to {
        Open => assert_eq!(
            OplogError::Exists("1".into()),
            refused(s.oplog_open(&new_changeset("1")))
        ),
        Staged => assert_eq!(not_open, refused(s.oplog_end(&k, to, T0 + 50))),
        Committed => {
            assert_eq!(not_open, refused(s.oplog_end(&k, to, T0 + 50)));
            assert_eq!(not_staged, refused(s.oplog_decide(&k, to, &decision(AGENT))));
        }
        Discarded => assert_eq!(not_staged, refused(s.oplog_decide(&k, to, &decision(AGENT)))),
        Reverted | PartlyReverted => assert_eq!(
            OplogError::BadMove { from, to },
            refused(s.oplog_finish_revert(&k, &decision(AGENT)))
        ),
    }
    assert_eq!(before, snap(&s));
}

macro_rules! refused_moves {
    ($($name:ident: $from:ident => $to:ident,)*) => {
        /// One test per move ADR-0002 doesn't allow (26 of the 36 pairs of states).
        mod refused_move {
            use super::*;
            $(
                #[test]
                fn $name() {
                    refuse_move(OplogStatus::$from, OplogStatus::$to);
                }
            )*
        }
    };
}

refused_moves! {
    open_to_open: Open => Open,
    staged_to_open: Staged => Open,
    committed_to_open: Committed => Open,
    discarded_to_open: Discarded => Open,
    reverted_to_open: Reverted => Open,
    partly_reverted_to_open: PartlyReverted => Open,
    staged_to_staged: Staged => Staged,
    committed_to_staged: Committed => Staged,
    discarded_to_staged: Discarded => Staged,
    reverted_to_staged: Reverted => Staged,
    partly_reverted_to_staged: PartlyReverted => Staged,
    committed_to_committed: Committed => Committed,
    discarded_to_committed: Discarded => Committed,
    reverted_to_committed: Reverted => Committed,
    partly_reverted_to_committed: PartlyReverted => Committed,
    open_to_discarded: Open => Discarded,
    committed_to_discarded: Committed => Discarded,
    discarded_to_discarded: Discarded => Discarded,
    reverted_to_discarded: Reverted => Discarded,
    partly_reverted_to_discarded: PartlyReverted => Discarded,
    open_to_reverted: Open => Reverted,
    discarded_to_reverted: Discarded => Reverted,
    reverted_to_reverted: Reverted => Reverted,
    open_to_partly_reverted: Open => PartlyReverted,
    discarded_to_partly_reverted: Discarded => PartlyReverted,
    reverted_to_partly_reverted: Reverted => PartlyReverted,
}

#[test]
fn a_decision_moves_only_to_committed_or_discarded() {
    let mut s = Store::open_in_memory().unwrap();
    let k = opened(&mut s, "1", 1);
    s.oplog_end(&k, OplogStatus::Staged, T0).unwrap();
    for to in [
        OplogStatus::Open,
        OplogStatus::Staged,
        OplogStatus::Reverted,
        OplogStatus::PartlyReverted,
    ] {
        assert_eq!(
            OplogError::BadMove {
                from: OplogStatus::Staged,
                to
            },
            refused(s.oplog_decide(&k, to, &decision(AGENT)))
        );
    }
    let k = opened(&mut s, "2", 1);
    for to in [OplogStatus::Open, OplogStatus::Discarded, OplogStatus::Reverted] {
        assert_eq!(
            OplogError::BadMove {
                from: OplogStatus::Open,
                to
            },
            refused(s.oplog_end(&k, to, T0))
        );
    }
}

// ---------------------------------------------------------------- reverts and outcomes

#[test]
fn a_full_revert_appends_one_inverse_operation_per_operation() {
    let mut s = Store::open_in_memory().unwrap();
    let k = opened(&mut s, "1", 0);
    s.oplog_append(&k, &create("a.md")).unwrap();
    s.oplog_append(&k, &update("b.md")).unwrap();
    let mut gone = update("c.md");
    gone.op = "delete".into();
    gone.next = None;
    s.oplog_append(&k, &gone).unwrap();
    s.oplog_end(&k, OplogStatus::Committed, T0 + 2).unwrap();
    for seq in 1..=3 {
        assert_eq!(seq + 3, s.oplog_revert_op(&k, seq, T0 + 30).unwrap());
    }
    assert_eq!(
        OplogStatus::Reverted,
        s.oplog_finish_revert(&k, &decision(AGENT)).unwrap()
    );
    let ops = s.oplog_ops(&k).unwrap();
    assert_eq!(6, ops.len());
    for (orig, inv) in ops[..3].iter().zip(&ops[3..]) {
        assert_eq!(Some(OpOutcome::Reverted), orig.outcome);
        assert_eq!(
            (
                Some(orig.seq),
                Some(OpOutcome::Applied),
                &orig.entity_uri,
                &orig.prev,
                &orig.next,
                T0 + 30
            ),
            (
                inv.reverts,
                inv.outcome,
                &inv.entity_uri,
                &inv.next,
                &inv.prev,
                inv.at
            )
        );
    }
    let names: Vec<&str> = ops[3..].iter().map(|o| o.op.as_str()).collect();
    assert_eq!(vec!["delete", "update", "create"], names);
    assert_eq!(
        OplogStatus::Reverted,
        s.oplog_changeset(&k).unwrap().unwrap().status
    );
}

#[test]
fn a_held_changeset_can_be_reverted_without_an_accept() {
    let mut s = Store::open_in_memory().unwrap();
    let k = opened(&mut s, "1", 2);
    s.oplog_end(&k, OplogStatus::Staged, T0 + 2).unwrap();
    s.oplog_revert_op(&k, 1, T0 + 3).unwrap();
    s.oplog_set_outcome(&k, 2, OpOutcome::SkippedChanged).unwrap();
    assert_eq!(
        OplogStatus::PartlyReverted,
        s.oplog_finish_revert(&k, &decision(&person())).unwrap()
    );
}

#[test]
fn a_second_partial_revert_stays_partly_reverted_and_changes_only_outcomes() {
    let mut s = Store::open_in_memory().unwrap();
    let k = committed(&mut s, "1", 3);
    s.oplog_revert_op(&k, 1, T0 + 20).unwrap();
    s.oplog_set_outcome(&k, 2, OpOutcome::SkippedChanged).unwrap();
    s.oplog_set_outcome(&k, 3, OpOutcome::Failed).unwrap();
    assert_eq!(
        OplogStatus::PartlyReverted,
        s.oplog_finish_revert(&k, &decision(AGENT)).unwrap()
    );
    let before = snap(&s);

    // A later revert takes back the failed one and still skips the other.
    s.oplog_set_outcome(&k, 2, OpOutcome::SkippedChanged).unwrap();
    s.oplog_revert_op(&k, 3, T0 + 40).unwrap();
    let mut later = decision(AGENT);
    later.at = T0 + 41;
    assert_eq!(
        OplogStatus::PartlyReverted,
        s.oplog_finish_revert(&k, &later).unwrap()
    );

    let after = snap(&s);
    let cs = |sn: &Snap| sn.changesets.values().next().unwrap().0;
    assert_eq!(
        (OplogStatus::PartlyReverted, OplogStatus::PartlyReverted),
        (cs(&before), cs(&after))
    );
    // Every row written before is still there with the same contents; only outcome 3 moved, and
    // its inverse was appended.
    for (seq_key, (fixed, outcome)) in &before.ops {
        let (fixed_now, outcome_now) = &after.ops[seq_key];
        assert_eq!(fixed, fixed_now);
        if seq_key.1 == 3 {
            assert_eq!(
                (Some(OpOutcome::Failed), Some(OpOutcome::Reverted)),
                (*outcome, *outcome_now)
            );
        } else {
            assert_eq!(outcome, outcome_now);
        }
    }
    assert_eq!(before.ops.len() + 1, after.ops.len());
    assert_eq!(Some(T0 + 41), s.oplog_changeset(&k).unwrap().unwrap().decided_at);

    // The last one, once the person includes it (UND-7).
    s.oplog_revert_op(&k, 2, T0 + 50).unwrap();
    assert_eq!(
        OplogStatus::Reverted,
        s.oplog_finish_revert(&k, &decision(&person())).unwrap()
    );
}

#[test]
fn a_failed_outcome_is_decided_again_until_the_changeset_is_reverted() {
    let mut s = Store::open_in_memory().unwrap();
    let k = committed(&mut s, "1", 2);
    // A revert takes back 1, fails on 2 and is cut short before it ends.
    s.oplog_revert_op(&k, 1, T0 + 20).unwrap();
    s.oplog_set_outcome(&k, 2, OpOutcome::Failed).unwrap();
    assert_eq!(
        OplogStatus::Committed,
        s.oplog_changeset(&k).unwrap().unwrap().status
    );
    // Resumed: 2 fails again, then goes through.
    s.oplog_set_outcome(&k, 2, OpOutcome::Failed).unwrap();
    s.oplog_set_outcome(&k, 2, OpOutcome::SkippedChanged).unwrap();
    s.oplog_revert_op(&k, 2, T0 + 30).unwrap();
    assert_eq!(
        OplogStatus::Reverted,
        s.oplog_finish_revert(&k, &decision(AGENT)).unwrap()
    );
    assert!(matches!(
        refused(s.oplog_set_outcome(&k, 2, OpOutcome::Failed)),
        OplogError::BadOutcome {
            status: OplogStatus::Reverted,
            ..
        }
    ));

    // A reverted changeset with a failed operation (only an import writes one): final too.
    let k = imported(&mut s, "2", OplogStatus::Reverted, &[Some(OpOutcome::Failed)]);
    let before = snap(&s);
    for to in [OpOutcome::Failed, OpOutcome::SkippedChanged] {
        assert_eq!(
            OplogError::BadOutcome {
                seq: 1,
                from: Some(OpOutcome::Failed),
                to,
                status: OplogStatus::Reverted
            },
            refused(s.oplog_set_outcome(&k, 1, to))
        );
    }
    assert_eq!(
        OplogError::BadOutcome {
            seq: 1,
            from: Some(OpOutcome::Failed),
            to: OpOutcome::Reverted,
            status: OplogStatus::Reverted
        },
        refused(s.oplog_revert_op(&k, 1, T0))
    );
    assert_eq!(before, snap(&s));
}

#[test]
fn outcomes_are_not_decided_while_open_or_once_discarded() {
    let mut s = Store::open_in_memory().unwrap();
    let k = opened(&mut s, "1", 1);
    assert!(matches!(
        refused(s.oplog_set_outcome(&k, 1, OpOutcome::Failed)),
        OplogError::BadOutcome {
            status: OplogStatus::Open,
            ..
        }
    ));
    assert!(matches!(
        refused(s.oplog_revert_op(&k, 1, T0)),
        OplogError::BadOutcome { .. }
    ));
    s.oplog_end(&k, OplogStatus::Staged, T0).unwrap();
    s.oplog_decide(&k, OplogStatus::Discarded, &decision(&person()))
        .unwrap();
    for to in [OpOutcome::Applied, OpOutcome::Failed, OpOutcome::SkippedChanged] {
        assert!(matches!(
            refused(s.oplog_set_outcome(&k, 1, to)),
            OplogError::BadOutcome {
                status: OplogStatus::Discarded,
                ..
            }
        ));
    }
    assert!(matches!(
        refused(s.oplog_revert_op(&k, 1, T0)),
        OplogError::BadOutcome { .. }
    ));
    assert_eq!(1, s.oplog_ops(&k).unwrap().len());
}

#[test]
fn reverted_is_set_only_by_appending_an_inverse() {
    let mut s = Store::open_in_memory().unwrap();
    let k = committed(&mut s, "1", 1);
    assert_eq!(
        OplogError::NeedsInverse(1),
        refused(s.oplog_set_outcome(&k, 1, OpOutcome::Reverted))
    );
    assert_eq!(Some(OpOutcome::Applied), s.oplog_ops(&k).unwrap()[0].outcome);
}

#[test]
fn an_inverse_operation_is_neither_reverted_nor_decided_again() {
    let mut s = Store::open_in_memory().unwrap();
    let k = committed(&mut s, "1", 2);
    let inv = s.oplog_revert_op(&k, 1, T0 + 20).unwrap();
    assert_eq!(
        OplogError::InverseOp(inv),
        refused(s.oplog_revert_op(&k, inv, T0 + 21))
    );
    assert_eq!(
        OplogError::InverseOp(inv),
        refused(s.oplog_set_outcome(&k, inv, OpOutcome::Failed))
    );
    assert_eq!(
        OplogError::NoOperation {
            id: "1".into(),
            seq: 9
        },
        refused(s.oplog_set_outcome(&k, 9, OpOutcome::Failed))
    );
}

/// Tries to move an operation's outcome from `from` to `to` while its changeset is committed.
fn refuse_outcome(from: OpOutcome, to: OpOutcome) {
    let mut s = Store::open_in_memory().unwrap();
    let k = imported(&mut s, "1", OplogStatus::Committed, &[Some(from)]);
    let before = snap(&s);
    let err = if to == OpOutcome::Reverted {
        refused(s.oplog_revert_op(&k, 1, T0 + 50))
    } else {
        refused(s.oplog_set_outcome(&k, 1, to))
    };
    assert_eq!(
        OplogError::BadOutcome {
            seq: 1,
            from: Some(from),
            to,
            status: OplogStatus::Committed
        },
        err
    );
    assert_eq!(before, snap(&s));
}

macro_rules! refused_outcomes {
    ($($name:ident: $from:ident => $to:ident,)*) => {
        /// One test per outcome move ADR-0002 doesn't allow while a revert may still run.
        mod refused_outcome {
            use super::*;
            $(
                #[test]
                fn $name() {
                    refuse_outcome(OpOutcome::$from, OpOutcome::$to);
                }
            )*
        }
    };
}

refused_outcomes! {
    applied_to_applied: Applied => Applied,
    applied_to_discarded: Applied => Discarded,
    skipped_changed_to_applied: SkippedChanged => Applied,
    skipped_changed_to_discarded: SkippedChanged => Discarded,
    failed_to_applied: Failed => Applied,
    failed_to_discarded: Failed => Discarded,
    discarded_to_applied: Discarded => Applied,
    discarded_to_discarded: Discarded => Discarded,
    discarded_to_skipped_changed: Discarded => SkippedChanged,
    discarded_to_failed: Discarded => Failed,
    discarded_to_reverted: Discarded => Reverted,
    reverted_to_applied: Reverted => Applied,
    reverted_to_discarded: Reverted => Discarded,
    reverted_to_skipped_changed: Reverted => SkippedChanged,
    reverted_to_failed: Reverted => Failed,
    reverted_to_reverted: Reverted => Reverted,
}

// ---------------------------------------------------------------- pruning and import

#[test]
fn decided_changesets_are_pruned_90_days_after_their_decision() {
    let mut s = Store::open_in_memory().unwrap();
    let mut now = T0;
    let accepted = committed(&mut s, "1", 2); // decided at T0 + 10
    let k = opened(&mut s, "2", 1);
    s.oplog_end(&k, OplogStatus::Staged, now).unwrap();
    let discarded = opened(&mut s, "3", 1);
    s.oplog_end(&discarded, OplogStatus::Staged, now).unwrap();
    s.oplog_decide(&discarded, OplogStatus::Discarded, &decision(&person()))
        .unwrap();
    let reverted = committed(&mut s, "4", 1);
    s.oplog_revert_op(&reverted, 1, T0 + 20).unwrap();
    let mut late = decision(AGENT);
    late.at = T0 + 1000;
    s.oplog_finish_revert(&reverted, &late).unwrap();
    opened(&mut s, "5", 1);

    now += 10 + OPLOG_HISTORY_MS;
    assert_eq!(0, s.oplog_prune(now).unwrap());
    now += 1;
    assert_eq!(2, s.oplog_prune(now).unwrap());
    assert_eq!(None, s.oplog_changeset(&accepted).unwrap());
    assert_eq!(None, s.oplog_changeset(&discarded).unwrap());
    assert!(s.oplog_ops(&accepted).unwrap().is_empty());
    assert!(s.oplog_changeset(&reverted).unwrap().is_some());
    now += 1000;
    assert_eq!(1, s.oplog_prune(now).unwrap());
    // Open and staged changesets are never pruned.
    now += 100 * OPLOG_HISTORY_MS;
    assert_eq!(0, s.oplog_prune(now).unwrap());
    let left: Vec<String> = snap(&s).changesets.into_keys().collect();
    assert_eq!(vec![format!("{DEV}/2"), format!("{DEV}/5")], left);
    assert_eq!(2, snap(&s).ops.len());
}

#[test]
fn an_import_keeps_a_reverted_changesets_status_and_outcomes_and_sets_imported_at() {
    let mut s = Store::open_in_memory().unwrap();
    let outcomes = [Some(OpOutcome::Reverted), Some(OpOutcome::Reverted)];
    let k = imported(&mut s, "19a-1", OplogStatus::Reverted, &outcomes);
    let cs = s.oplog_changeset(&k).unwrap().unwrap();
    assert_eq!(
        (OplogStatus::Reverted, Some(T0 + 3), Some(T0 + 100)),
        (cs.status, cs.decided_at, cs.imported_at)
    );
    let got: Vec<_> = s
        .oplog_ops(&k)
        .unwrap()
        .into_iter()
        .map(|o| (o.seq, o.outcome))
        .collect();
    assert_eq!(vec![(1, outcomes[0]), (2, outcomes[1])], got);

    let mixed = [
        Some(OpOutcome::Reverted),
        Some(OpOutcome::SkippedChanged),
        Some(OpOutcome::Failed),
    ];
    let k = imported(&mut s, "19a-2", OplogStatus::PartlyReverted, &mixed);
    let got: Vec<_> = s.oplog_ops(&k).unwrap().into_iter().map(|o| o.outcome).collect();
    assert_eq!(mixed.to_vec(), got);
    // An imported changeset carries on like any other.
    s.oplog_revert_op(&k, 2, T0 + 200).unwrap();
    s.oplog_revert_op(&k, 3, T0 + 200).unwrap();
    assert_eq!(
        OplogStatus::Reverted,
        s.oplog_finish_revert(&k, &decision(AGENT)).unwrap()
    );

    // Only the import sets imported_at, and it doesn't write a changeset twice.
    let fresh = committed(&mut s, "x", 1);
    assert_eq!(None, s.oplog_changeset(&fresh).unwrap().unwrap().imported_at);
    let again = s.oplog_changeset(&key("19a-1")).unwrap().unwrap();
    assert_eq!(
        OplogError::Exists("19a-1".into()),
        refused(s.oplog_import(&again, &[], T0))
    );
}

#[test]
fn an_import_still_checks_actors_uris_and_numbering() {
    let mut s = Store::open_in_memory().unwrap();
    let cs = OplogChangeset {
        origin_device: DEV.into(),
        id: "1".into(),
        actor: "claude".into(),
        label: None,
        scope: folder_uri("p", ""),
        status: OplogStatus::Reverted,
        opened_at: T0,
        closed_at: None,
        decided_at: None,
        decided_by: None,
        decided_actor: None,
        imported_at: None,
    };
    assert_eq!(
        OplogError::BadActor("claude".into()),
        refused(s.oplog_import(&cs, &[], T0))
    );
    let cs = OplogChangeset {
        actor: AGENT.into(),
        ..cs
    };
    let op = ImportOp {
        op: create("a.md"),
        reverts: Some(1),
        outcome: Some(OpOutcome::Applied),
    };
    assert_eq!(
        OplogError::BadReverts { seq: 1, reverts: 1 },
        refused(s.oplog_import(&cs, &[op], T0))
    );
    assert!(snap(&s).changesets.is_empty());
}

// ---------------------------------------------------------------- property

#[derive(Debug, Clone)]
enum Action {
    Open(usize),
    Append(usize),
    End(usize, bool),
    Drop(usize),
    Decide(usize, bool),
    RevertOp(usize, u32),
    SetOutcome(usize, u32, OpOutcome),
    FinishRevert(usize, bool),
}

fn action() -> impl Strategy<Value = Action> {
    let cs = 0..3usize;
    let seq = 1..7u32;
    prop_oneof![
        1 => cs.clone().prop_map(Action::Open),
        3 => cs.clone().prop_map(Action::Append),
        1 => (cs.clone(), any::<bool>()).prop_map(|(c, b)| Action::End(c, b)),
        1 => cs.clone().prop_map(Action::Drop),
        1 => (cs.clone(), any::<bool>()).prop_map(|(c, b)| Action::Decide(c, b)),
        2 => (cs.clone(), seq.clone()).prop_map(|(c, s)| Action::RevertOp(c, s)),
        2 => (cs.clone(), seq, proptest::sample::select(OpOutcome::ALL.to_vec()))
            .prop_map(|(c, s, o)| Action::SetOutcome(c, s, o)),
        1 => (cs, any::<bool>()).prop_map(|(c, b)| Action::FinishRevert(c, b)),
    ]
}

/// The moves of ADR-0002's table, written out again here so the test doesn't check the store
/// against its own rules.
const ALLOWED_MOVES: &[(OplogStatus, OplogStatus)] = {
    use OplogStatus::*;
    &[
        (Open, Staged),
        (Open, Committed),
        (Staged, Committed),
        (Staged, Discarded),
        (Staged, Reverted),
        (Staged, PartlyReverted),
        (Committed, Reverted),
        (Committed, PartlyReverted),
        (PartlyReverted, Reverted),
        (PartlyReverted, PartlyReverted),
    ]
};

/// Whether an outcome may move as observed, given the changeset's status before and after.
fn outcome_move_ok(
    before: OplogStatus,
    after: OplogStatus,
    from: Option<OpOutcome>,
    to: Option<OpOutcome>,
) -> bool {
    use OpOutcome as O;
    use OplogStatus as S;
    let Some(to) = to else { return false };
    // An accept, discard or commit at a session's end decides every undecided operation.
    if from.is_none()
        && before != after
        && matches!(
            (after, to),
            (S::Committed, O::Applied) | (S::Discarded, O::Discarded)
        )
    {
        return true;
    }
    matches!(before, S::Staged | S::Committed | S::PartlyReverted)
        && match from {
            None => true,
            Some(O::Applied | O::SkippedChanged | O::Failed) => {
                matches!(to, O::SkippedChanged | O::Failed | O::Reverted)
            }
            Some(O::Discarded | O::Reverted) => false,
        }
}

fn apply(s: &mut Store, a: &Action) -> Result<(), StoreError> {
    let k = |i: &usize| key(&i.to_string());
    let d = |agent: &bool| {
        decision(if *agent {
            AGENT
        } else {
            "person:WASL-0123-4567-89AB-CDEF-GHJK-MNPQ-RSTV"
        })
    };
    match a {
        Action::Open(i) => s.oplog_open(&new_changeset(&i.to_string())),
        Action::Append(i) => s.oplog_append(&k(i), &update("a.md")).map(drop),
        Action::End(i, stage) => {
            let to = if *stage {
                OplogStatus::Staged
            } else {
                OplogStatus::Committed
            };
            s.oplog_end(&k(i), to, T0 + 5)
        }
        Action::Drop(i) => s.oplog_drop_empty(&k(i)),
        Action::Decide(i, accept) => {
            let to = if *accept {
                OplogStatus::Committed
            } else {
                OplogStatus::Discarded
            };
            s.oplog_decide(&k(i), to, &d(accept))
        }
        Action::RevertOp(i, seq) => s.oplog_revert_op(&k(i), *seq, T0 + 6).map(drop),
        Action::SetOutcome(i, seq, o) => s.oplog_set_outcome(&k(i), *seq, *o),
        Action::FinishRevert(i, agent) => s.oplog_finish_revert(&k(i), &d(agent)).map(drop),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    #[test]
    fn random_sessions_only_move_as_the_adr_allows(actions in proptest::collection::vec(action(), 1..40)) {
        let mut s = Store::open_in_memory().unwrap();
        for a in &actions {
            let before = snap(&s);
            let result = apply(&mut s, a);
            let after = snap(&s);
            match &result {
                // Every refusal is typed and leaves every row as it was.
                Err(StoreError::Oplog(_)) => prop_assert_eq!(&before, &after, "{:?}", a),
                Err(e) => prop_assert!(false, "{:?}: untyped error {}", a, e),
                Ok(()) => {}
            }
            // Rows are never deleted, except an empty open changeset that is dropped.
            for (k, (status, _)) in &before.changesets {
                if !after.changesets.contains_key(k) {
                    let dropped = matches!(a, Action::Drop(_)) && result.is_ok()
                        && *status == OplogStatus::Open
                        && !before.ops.keys().any(|(c, _)| c == k);
                    prop_assert!(dropped, "{:?} deleted {}", a, k);
                }
            }
            prop_assert!(after.ops.len() >= before.ops.len());
            prop_assert!(after.changesets.len() + 1 >= before.changesets.len());
            for (op_key, (fixed, from)) in &before.ops {
                let (fixed_now, to) = &after.ops[op_key];
                prop_assert_eq!(fixed, fixed_now, "{:?} rewrote {:?}", a, op_key);
                if from != to {
                    let cs_before = before.changesets[&op_key.0].0;
                    let cs_after = after.changesets[&op_key.0].0;
                    prop_assert!(
                        outcome_move_ok(cs_before, cs_after, *from, *to),
                        "{:?}: outcome {:?} → {:?} while {:?}", a, from, to, cs_before
                    );
                }
            }
            // States only move as the table allows.
            for (k, (status, rest)) in &before.changesets {
                if let Some((now, rest_now)) = after.changesets.get(k) {
                    if status != now {
                        prop_assert!(ALLOWED_MOVES.contains(&(*status, *now)), "{:?}: {:?} → {:?}", a, status, now);
                    } else if *now != OplogStatus::PartlyReverted {
                        // Only partly_reverted → partly_reverted keeps its state and records a decision.
                        prop_assert_eq!(rest, rest_now);
                    }
                }
            }
            // New operations: appended while open, or inverse operations of a revert.
            for (op_key, (_, outcome)) in &after.ops {
                if !before.ops.contains_key(op_key) {
                    let cs_before = before.changesets.get(&op_key.0).map(|c| c.0);
                    prop_assert!(
                        cs_before == Some(OplogStatus::Open) && outcome.is_none()
                            || matches!(cs_before, Some(OplogStatus::Staged | OplogStatus::Committed | OplogStatus::PartlyReverted))
                                && *outcome == Some(OpOutcome::Applied),
                        "{:?} appended {:?}", a, op_key
                    );
                }
            }
            // seq runs 1, 2, 3 … without gaps in every changeset.
            for k in after.changesets.keys() {
                let seqs: Vec<u32> = after.ops.keys().filter(|(c, _)| c == k).map(|(_, n)| *n).collect();
                let n = seqs.len() as u32;
                prop_assert_eq!(seqs, (1..=n).collect::<Vec<u32>>());
            }
        }
    }
}

// ---------------------------------------------------------------- agent changes in the op-log (#47, FR-OL7)

/// A store with pair `p` whose agent changes are recorded under [`DEV`], as the engine sets it.
fn agent_store() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.create_pair(&wasl_store::Pair::new("p", "Notes", "/d")).unwrap();
    s.set_oplog_device(Some(DEV.into()));
    s
}

fn agent_changeset(id: &str) -> wasl_store::Changeset {
    wasl_store::Changeset {
        id: id.into(),
        pair_id: Some("p".into()),
        actor: AGENT.into(),
        label: None,
        scope: String::new(),
        session: None,
        status: wasl_store::ChangesetStatus::Open,
        started_at: T0,
        ended_at: None,
        decided_at: None,
    }
}

fn agent_entry(rel: &str, kind: wasl_store::EntryKind) -> wasl_store::AgentEntry {
    use wasl_store::EntryKind::{Added, Deleted};
    wasl_store::AgentEntry {
        rel: rel.into(),
        key: rel.into(),
        kind,
        is_dir: false,
        before_size: (kind != Added).then_some(2),
        before_mtime: (kind != Added).then_some(T0),
        after_size: (kind != Deleted).then_some(5),
        after_mtime: (kind != Deleted).then_some(T0 + 1),
        source: wasl_store::EarlierVersion::Synced,
        location: None,
        outcome: None,
    }
}

/// An operation as (seq, key, op, reverts, outcome).
type OpRow = (u32, String, String, Option<u32>, Option<OpOutcome>);

/// Each operation as an [`OpRow`].
fn op_rows(s: &Store, id: &str) -> Vec<OpRow> {
    s.oplog_ops(&key(id))
        .unwrap()
        .into_iter()
        .map(|o| {
            let (_, k) = file_uri_parts(&o.entity_uri).unwrap();
            (o.seq, k, o.op, o.reverts, o.outcome)
        })
        .collect()
}

fn op_row(seq: u32, key: &str, op: &str, reverts: Option<u32>, outcome: Option<OpOutcome>) -> OpRow {
    (seq, key.into(), op.into(), reverts, outcome)
}

#[test]
fn a_second_partial_revert_keeps_partly_reverted_and_changes_only_outcomes() {
    use wasl_store::ChangesetStatus::{PartlyReverted, Reverted};
    use wasl_store::EntryKind::{Added, Changed};
    let mut s = agent_store();
    s.create_changeset(&agent_changeset("cs"), &[]).unwrap();
    let entries = [
        agent_entry("a.txt", Changed),
        agent_entry("b.txt", Changed),
        agent_entry("c.txt", Added),
    ];
    s.end_changeset("cs", &entries, T0 + 2).unwrap();
    assert_eq!(
        OplogStatus::Staged,
        s.oplog_changeset(&key("cs")).unwrap().unwrap().status
    );

    // The first revert: a.txt back, b.txt changed since, c.txt failed.
    s.set_entry_outcome_at("cs", "a.txt", "reverted", T0 + 3).unwrap();
    s.set_entry_outcome_at("cs", "b.txt", "skipped_changed", T0 + 3)
        .unwrap();
    s.set_entry_outcome_at("cs", "c.txt", "failed: disk full", T0 + 3)
        .unwrap();
    s.decide_changeset("cs", PartlyReverted, T0 + 4).unwrap();
    let first = s.oplog_changeset(&key("cs")).unwrap().unwrap();
    assert_eq!(OplogStatus::PartlyReverted, first.status);
    assert_eq!(Some(T0 + 4), first.decided_at);
    assert_eq!(
        vec![
            op_row(1, "a.txt", "update", None, Some(OpOutcome::Reverted)),
            op_row(2, "b.txt", "update", None, Some(OpOutcome::SkippedChanged)),
            op_row(3, "c.txt", "create", None, Some(OpOutcome::Failed)),
            op_row(4, "a.txt", "update", Some(1), Some(OpOutcome::Applied)),
        ],
        op_rows(&s, "cs")
    );

    // A second revert takes b.txt back; c.txt fails again: still partly reverted.
    s.set_entry_outcome_at("cs", "b.txt", "reverted", T0 + 5).unwrap();
    s.set_entry_outcome_at("cs", "c.txt", "failed: still full", T0 + 5)
        .unwrap();
    s.decide_changeset("cs", PartlyReverted, T0 + 6).unwrap();
    let second = s.oplog_changeset(&key("cs")).unwrap().unwrap();
    assert_eq!(OplogStatus::PartlyReverted, second.status);
    assert_eq!(Some(T0 + 6), second.decided_at);
    assert_eq!(
        vec![
            op_row(1, "a.txt", "update", None, Some(OpOutcome::Reverted)),
            op_row(2, "b.txt", "update", None, Some(OpOutcome::Reverted)),
            op_row(3, "c.txt", "create", None, Some(OpOutcome::Failed)),
            op_row(4, "a.txt", "update", Some(1), Some(OpOutcome::Applied)),
            op_row(5, "b.txt", "update", Some(2), Some(OpOutcome::Applied)),
        ],
        op_rows(&s, "cs")
    );

    // The last one takes c.txt back: reverted, and every outcome is final.
    s.set_entry_outcome_at("cs", "c.txt", "reverted", T0 + 7).unwrap();
    s.decide_changeset("cs", Reverted, T0 + 8).unwrap();
    let done = s.oplog_changeset(&key("cs")).unwrap().unwrap();
    assert_eq!(OplogStatus::Reverted, done.status);
    assert_eq!(Some(person()), done.decided_actor);
    let ops = s.oplog_ops(&key("cs")).unwrap();
    assert_eq!(6, ops.len());
    assert_eq!(("delete", Some(3)), (ops[5].op.as_str(), ops[5].reverts));
    assert_eq!(T0 + 7, ops[5].at);
}

#[test]
fn an_entry_outcome_is_written_with_the_op_log_or_not_at_all() {
    use wasl_store::EntryKind::Changed;
    let mut s = agent_store();
    s.create_changeset(&agent_changeset("cs"), &[]).unwrap();
    s.end_changeset("cs", &[agent_entry("a.txt", Changed)], T0 + 2)
        .unwrap();
    // The op-log refuses an outcome it has no meaning for, and the entry keeps its own.
    assert!(matches!(
        s.set_entry_outcome_at("cs", "a.txt", "kept", T0 + 3),
        Err(StoreError::Refused(_))
    ));
    assert_eq!(None, s.agent_entries("cs").unwrap()[0].outcome);
    // A decision the operations don't bear out is refused with the changeset left held.
    assert!(
        s.decide_changeset("cs", wasl_store::ChangesetStatus::Reverted, T0 + 4)
            .is_err()
    );
    assert_eq!(
        wasl_store::ChangesetStatus::Held,
        s.changeset("cs").unwrap().unwrap().status
    );
    assert_eq!(
        OplogStatus::Staged,
        s.oplog_changeset(&key("cs")).unwrap().unwrap().status
    );
}

#[test]
fn without_a_device_id_agent_changes_wait_for_the_copy() {
    use wasl_store::EntryKind::{Added, Changed};
    let mut s = Store::open_in_memory().unwrap();
    s.create_pair(&wasl_store::Pair::new("p", "Notes", "/d")).unwrap();
    s.create_changeset(&agent_changeset("cs"), &[]).unwrap();
    s.end_changeset(
        "cs",
        &[agent_entry("a.txt", Changed), agent_entry("n.txt", Added)],
        T0 + 2,
    )
    .unwrap();
    let count = |s: &Store| -> i64 {
        s.connection()
            .query_row("SELECT COUNT(*) FROM oplog_changesets", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(0, count(&s));
    assert_eq!(0, s.copy_changesets_to_oplog(T0 + 3).unwrap());

    // Once the engine passes a device id, the start-up copy brings it in, held and undecided.
    s.set_oplog_device(Some(DEV.into()));
    assert_eq!(1, s.copy_changesets_to_oplog(T0 + 9).unwrap());
    let cs = s.oplog_changeset(&key("cs")).unwrap().unwrap();
    assert_eq!((OplogStatus::Staged, Some(T0 + 9)), (cs.status, cs.imported_at));
    assert_eq!(
        vec![
            op_row(1, "a.txt", "update", None, None),
            op_row(2, "n.txt", "create", None, None),
        ],
        op_rows(&s, "cs")
    );
    // From then on it is written as any other: accepting it commits it.
    s.decide_changeset_by("cs", wasl_store::ChangesetStatus::Accepted, T0 + 10, Some("cli"))
        .unwrap();
    let cs = s.oplog_changeset(&key("cs")).unwrap().unwrap();
    assert_eq!(
        (OplogStatus::Committed, Some("cli".to_owned())),
        (cs.status, cs.decided_by)
    );
    assert_eq!(0, s.copy_changesets_to_oplog(T0 + 11).unwrap());
}

#[test]
fn changesets_from_a_v10_database_are_copied_once_with_their_status_and_outcomes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wasl.db");
    let hash = "cd".repeat(32);
    {
        // A database as schema v10 left it: held, accepted, reverted, partly reverted and open
        // changesets.
        let conn = rusqlite::Connection::open(&path).unwrap();
        for sql in [
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
        ] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 10).unwrap();
        conn.execute_batch(&format!(
            "INSERT INTO pairs (id, name, local_root, route, reach, method, conflict_policy)
                 VALUES ('p', 'Notes', '/d', 'drive', 'lan', 'TWO_WAY', 'KEEP_BOTH');
             INSERT INTO agent_changesets (id, pair_id, actor, label, scope, status, started_at, ended_at, decided_at, decided_by)
                 VALUES ('19a-1', 'p', 'agent:claude-code', 'notes', 'docs', 'held', 1, 2, NULL, NULL),
                        ('19a-2', 'p', 'agent:gemini', NULL, '', 'accepted', 3, 4, 5, 'desktop'),
                        ('19a-3', 'p', 'agent:claude-code', NULL, '', 'reverted', 6, 7, 8, NULL),
                        ('19a-4', 'p', 'agent:claude-code', NULL, '', 'partly_reverted', 9, 10, 11, NULL),
                        ('19a-5', 'p', 'agent:gemini', NULL, 'docs', 'open', 12, NULL, NULL, NULL);
             INSERT INTO agent_saved (changeset_id, rel, location, size, mtime, blake3)
                 VALUES ('19a-1', 'docs/b.md', '1', 2, 1, '{hash}');
             INSERT INTO agent_manifest (changeset_id, rel, is_dir, size, mtime, synced)
                 VALUES ('19a-5', 'docs/a.md', 0, 2, 1, 1);
             INSERT INTO agent_entries (changeset_id, rel, key, kind, is_dir, before_size, before_mtime, after_size, after_mtime, source, location, outcome)
                 VALUES ('19a-1', 'docs/a.md', 'docs/a.md', 'added', 0, NULL, NULL, 3, 2, 'none', NULL, NULL),
                        ('19a-1', 'docs/b.md', 'docs/b.md', 'changed', 0, 2, 1, 3, 2, 'saved', '1', NULL),
                        ('19a-2', 'x.md', 'x.md', 'added', 0, NULL, NULL, 3, 4, 'none', NULL, NULL),
                        ('19a-2', 'new', 'new', 'added', 1, NULL, NULL, NULL, NULL, 'none', NULL, NULL),
                        ('19a-3', 'y.md', 'y.md', 'deleted', 0, 2, 6, NULL, NULL, 'synced', NULL, 'reverted'),
                        ('19a-4', 'a.md', 'a.md', 'changed', 0, 2, 9, 3, 10, 'synced', NULL, 'reverted'),
                        ('19a-4', 'b.md', 'b.md', 'changed', 0, 2, 9, 3, 10, 'synced', NULL, 'skipped_changed'),
                        ('19a-4', 'c.md', 'c.md', 'changed', 0, 2, 9, 3, 10, 'synced', NULL, 'failed: gone');"
        ))
        .unwrap();
    }
    let mut s = Store::open(&path).unwrap();
    // Nothing is copied before the engine passes the device id.
    assert_eq!(0, s.copy_changesets_to_oplog(T0).unwrap());
    s.set_oplog_device(Some(DEV.into()));
    assert_eq!(5, s.copy_changesets_to_oplog(T0).unwrap());

    let cs = |id: &str| s.oplog_changeset(&key(id)).unwrap().unwrap();
    // Held: staged, undecided; the saved copy's BLAKE3 is the state before.
    let held = cs("19a-1");
    assert_eq!(
        OplogChangeset {
            origin_device: DEV.into(),
            id: "19a-1".into(),
            actor: AGENT.into(),
            label: Some("notes".into()),
            scope: folder_uri("p", "docs"),
            status: OplogStatus::Staged,
            opened_at: 1,
            closed_at: Some(2),
            decided_at: None,
            decided_by: None,
            decided_actor: None,
            imported_at: Some(T0),
        },
        held
    );
    assert_eq!(
        vec![
            op_row(1, "docs/a.md", "create", None, None),
            op_row(2, "docs/b.md", "update", None, None),
        ],
        op_rows(&s, "19a-1")
    );
    let ops = s.oplog_ops(&key("19a-1")).unwrap();
    assert_eq!(
        Some(json!({"kind": "file", "size": 2, "mtime_ms": 1, "blake3": hash})),
        ops[1].prev
    );
    assert_eq!(
        Some(json!({"kind": "file", "size": 3, "mtime_ms": 2})),
        ops[1].next
    );

    // Accepted: committed, its operations applied, decided by the client it names.
    let accepted = cs("19a-2");
    assert_eq!(
        (
            OplogStatus::Committed,
            Some(5),
            Some("desktop".to_owned()),
            Some(person())
        ),
        (
            accepted.status,
            accepted.decided_at,
            accepted.decided_by,
            accepted.decided_actor
        )
    );
    assert_eq!(
        vec![
            op_row(1, "new", "create", None, Some(OpOutcome::Applied)),
            op_row(2, "x.md", "create", None, Some(OpOutcome::Applied)),
        ],
        op_rows(&s, "19a-2")
    );
    assert_eq!(
        Some(json!({"kind": "dir"})),
        s.oplog_ops(&key("19a-2")).unwrap()[0].next
    );

    // Reverted: the entry taken back has its inverse operation.
    let reverted = cs("19a-3");
    assert_eq!(
        (OplogStatus::Reverted, Some("unknown".to_owned())),
        (reverted.status, reverted.decided_by)
    );
    assert_eq!(
        vec![
            op_row(1, "y.md", "delete", None, Some(OpOutcome::Reverted)),
            op_row(2, "y.md", "create", Some(1), Some(OpOutcome::Applied)),
        ],
        op_rows(&s, "19a-3")
    );
    let ops = s.oplog_ops(&key("19a-3")).unwrap();
    assert_eq!((&ops[0].prev, &ops[0].next), (&ops[1].next, &ops[1].prev));
    assert_eq!(8, ops[1].at);

    // Partly reverted: outcomes as they are, though no allowed move leads there.
    assert_eq!(OplogStatus::PartlyReverted, cs("19a-4").status);
    assert_eq!(
        vec![
            op_row(1, "a.md", "update", None, Some(OpOutcome::Reverted)),
            op_row(2, "b.md", "update", None, Some(OpOutcome::SkippedChanged)),
            op_row(3, "c.md", "update", None, Some(OpOutcome::Failed)),
            op_row(4, "a.md", "update", Some(1), Some(OpOutcome::Applied)),
        ],
        op_rows(&s, "19a-4")
    );

    // Open: copied open and empty.
    assert_eq!(
        (OplogStatus::Open, folder_uri("p", "docs"), Some(T0)),
        (cs("19a-5").status, cs("19a-5").scope, cs("19a-5").imported_at)
    );
    assert!(op_rows(&s, "19a-5").is_empty());

    // A second start copies nothing and changes nothing.
    let before = dump(s.connection());
    assert_eq!(0, s.copy_changesets_to_oplog(T0 + 1).unwrap());
    assert_eq!(before, dump(s.connection()));

    // The open session's operations follow when it ends, as for a new one.
    s.end_changeset(
        "19a-5",
        &[agent_entry("docs/a.md", wasl_store::EntryKind::Changed)],
        T0 + 2,
    )
    .unwrap();
    let ended = s.oplog_changeset(&key("19a-5")).unwrap().unwrap();
    assert_eq!(
        (OplogStatus::Staged, Some(T0 + 2), Some(T0)),
        (ended.status, ended.closed_at, ended.imported_at)
    );
    assert_eq!(
        vec![op_row(1, "docs/a.md", "update", None, None)],
        op_rows(&s, "19a-5")
    );
}

#[test]
fn a_list_is_filtered_by_actor_status_time_and_pair_newest_first() {
    let mut s = Store::open_in_memory().unwrap();
    opened(&mut s, "a", 2);
    s.oplog_end(&key("a"), OplogStatus::Staged, T0 + 5).unwrap();
    let mut other = new_changeset("b");
    other.actor = "agent:gemini".into();
    other.scope = folder_uri("q", "notes");
    other.opened_at = T0 + 100;
    s.oplog_open(&other).unwrap();
    // A pair id that is a prefix of another must not match it.
    let mut longer = new_changeset("c");
    longer.scope = folder_uri("pp", "");
    longer.opened_at = T0 + 200;
    s.oplog_open(&longer).unwrap();

    let ids = |f: OplogFilter| -> Vec<(String, u32)> {
        s.oplog_list(&f)
            .unwrap()
            .into_iter()
            .map(|l| (l.changeset.id, l.op_count))
            .collect()
    };
    let all = ids(OplogFilter::default());
    assert_eq!(vec![("c".into(), 0), ("b".into(), 0), ("a".into(), 2)], all);
    let by_actor = ids(OplogFilter {
        actor: Some("agent:gemini".into()),
        ..Default::default()
    });
    assert_eq!(vec![("b".to_owned(), 0)], by_actor);
    let staged = ids(OplogFilter {
        status: Some(OplogStatus::Staged),
        ..Default::default()
    });
    assert_eq!(vec![("a".to_owned(), 2)], staged);
    let recent = ids(OplogFilter {
        since: Some(T0 + 100),
        ..Default::default()
    });
    assert_eq!(vec![("c".to_owned(), 0), ("b".to_owned(), 0)], recent);
    let in_p = ids(OplogFilter {
        pair: Some("p".into()),
        ..Default::default()
    });
    assert_eq!(vec![("a".to_owned(), 2)], in_p);
}

#[test]
fn a_limited_list_keeps_the_newest() {
    let mut s = Store::open_in_memory().unwrap();
    for (i, id) in ["a", "b", "c"].into_iter().enumerate() {
        let mut cs = new_changeset(id);
        cs.opened_at = T0 + i as i64;
        s.oplog_open(&cs).unwrap();
    }
    let ids = |limit| -> Vec<String> {
        s.oplog_list(&OplogFilter {
            limit,
            ..Default::default()
        })
        .unwrap()
        .into_iter()
        .map(|l| l.changeset.id)
        .collect()
    };
    assert_eq!(vec!["c", "b"], ids(Some(2)));
    assert_eq!(vec!["c", "b", "a"], ids(None), "no limit: all");
    assert!(ids(Some(0)).is_empty());
}
