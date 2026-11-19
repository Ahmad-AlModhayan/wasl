//! A run keeps the kind of its failure (issue #57), and a run stored before that keeps none.

mod common;

use common::root;
use wasl_core::{ErrorKind, RunStatus};
use wasl_store::{Pair, RunStats, Store};

fn store() -> Store {
    let s = Store::open_in_memory().unwrap();
    s.create_pair(&Pair::new("p", "Notes", &root("data"))).unwrap();
    s
}

#[test]
fn a_failed_run_keeps_the_kind_of_its_failure() {
    let s = store();
    for (id, kind) in ErrorKind::ALL.into_iter().enumerate() {
        let id = format!("r{id}");
        s.start_run(&id, "p", "manual", 10).unwrap();
        s.finish_run_with_kind(
            &id,
            RunStatus::Failed,
            &RunStats::default(),
            Some("boom"),
            Some(kind),
            20,
        )
        .unwrap();
        assert_eq!(Some(kind), s.run(&id).unwrap().unwrap().error_kind, "{kind:?}");
    }
    // The list reads it too, not only the one-run lookup.
    let listed = s.runs(Some("p"), 100).unwrap();
    assert_eq!(ErrorKind::ALL.len(), listed.len());
    assert!(listed.iter().all(|r| r.error_kind.is_some()));
}

#[test]
fn a_run_finished_without_a_kind_has_none() {
    let s = store();
    s.start_run("ok", "p", "manual", 10).unwrap();
    assert_eq!(None, s.run("ok").unwrap().unwrap().error_kind, "still running");
    s.finish_run("ok", RunStatus::Success, &RunStats::default(), None, 20)
        .unwrap();
    assert_eq!(None, s.run("ok").unwrap().unwrap().error_kind);
}

#[test]
fn runs_stored_before_the_migration_read_with_no_kind_and_the_store_opens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wasl.db");
    {
        let s = Store::open(&path).unwrap();
        s.create_pair(&Pair::new("p", "Notes", &root("data"))).unwrap();
        s.start_run("old", "p", "manual", 10).unwrap();
        s.finish_run(
            "old",
            RunStatus::Failed,
            &RunStats::default(),
            Some("The network dropped"),
            20,
        )
        .unwrap();
    }
    // Put the file back as schema 13 left it: no kind column.
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("ALTER TABLE runs DROP COLUMN error_kind;")
            .unwrap();
        conn.pragma_update(None, "user_version", 13).unwrap();
    }
    let s = Store::open(&path).unwrap();
    assert_eq!(14, s.schema_version().unwrap());
    let old = s.run("old").unwrap().unwrap();
    assert_eq!(RunStatus::Failed, old.status);
    assert_eq!(Some("The network dropped"), old.message.as_deref());
    assert_eq!(None, old.error_kind, "the kind was never kept for it");
    // A run after the upgrade keeps its kind.
    s.start_run("new", "p", "manual", 30).unwrap();
    s.finish_run_with_kind(
        "new",
        RunStatus::Failed,
        &RunStats::default(),
        None,
        Some(ErrorKind::Network),
        40,
    )
    .unwrap();
    assert_eq!(
        Some(ErrorKind::Network),
        s.run("new").unwrap().unwrap().error_kind
    );
}
