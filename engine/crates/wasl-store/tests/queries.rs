//! The screens' read queries.

use wasl_core::{BaselineEntry, ConflictPolicy, RunStatus};
use wasl_store::{BinSide, ConflictRecord, EventLevel, NewBinEntry, Pair, RunEvent, RunStats, Store};

fn store() -> Store {
    let s = Store::open_in_memory().unwrap();
    s.create_pair(&Pair::new("p", "P", "/p")).unwrap();
    s.create_pair(&Pair::new("q", "Q", "/q")).unwrap();
    s
}

fn file(key: &str, local: u64, remote: Option<u64>) -> BaselineEntry {
    BaselineEntry {
        key: key.into(),
        local_rel: key.replace(':', "："),
        is_dir: false,
        local_size: local,
        local_mtime: 1,
        local_hash: Some("h".into()),
        remote_id: remote.map(|_| format!("id-{key}")),
        remote_md5: remote.map(|_| "h".into()),
        remote_size: remote,
        remote_mtime: remote.map(|_| 1),
        remote_version: remote.map(|_| 1),
        export_mime: None,
    }
}

fn ev(at: i64, level: EventLevel, action: &str, key: &str) -> RunEvent {
    RunEvent {
        at,
        level,
        action: action.into(),
        key: Some(key.into()),
        message: format!("{action} {key}"),
        detail: None,
        bytes: 0,
    }
}

#[test]
fn totals_count_files_and_bytes_per_pair() {
    let mut s = store();
    s.put_baseline("p", &file("a", 10, Some(12))).unwrap();
    s.put_baseline("p", &file("b", 5, None)).unwrap();
    let mut dir = file("d", 0, Some(0));
    dir.is_dir = true;
    s.put_baseline("p", &dir).unwrap();
    s.put_baseline("q", &file("x", 1, Some(1))).unwrap();
    let t = s.pair_totals().unwrap();
    assert_eq!(2, t.len());
    assert_eq!(
        (2, 15, 12),
        (t[0].files, t[0].local_bytes, t[0].remote_bytes),
        "folders are not counted"
    );
    assert_eq!(2, s.baseline_file_count("p").unwrap());
    assert_eq!(0, s.baseline_file_count("none").unwrap());
}

#[test]
fn baseline_is_found_by_local_name() {
    let mut s = store();
    s.put_baseline("p", &file("a:b", 1, Some(1))).unwrap();
    assert_eq!("a:b", s.baseline_by_local_rel("p", "a：b").unwrap().unwrap().key);
    assert_eq!(None, s.baseline_by_local_rel("p", "a:b").unwrap());
}

#[test]
fn events_filter_by_pair_problems_and_time() {
    let mut s = store();
    s.start_run("r1", "p", "manual", 10).unwrap();
    s.start_run("r2", "q", "manual", 20).unwrap();
    s.append_events(
        "r1",
        &[
            ev(11, EventLevel::Info, "UPLOAD", "a"),
            ev(12, EventLevel::Error, "ERROR", "b"),
        ],
    )
    .unwrap();
    s.append_events(
        "r2",
        &[
            ev(21, EventLevel::Warn, "SKIP", "c"),
            ev(22, EventLevel::Info, "UPLOAD", "a"),
        ],
    )
    .unwrap();

    let keys =
        |v: Vec<wasl_store::PairEvent>| v.into_iter().map(|e| e.event.key.unwrap()).collect::<Vec<_>>();
    assert_eq!(
        vec!["a", "c", "b", "a"],
        keys(s.events(None, false, None, 10).unwrap()),
        "newest first"
    );
    assert_eq!(
        vec!["b", "a"],
        keys(s.events(Some("p"), false, None, 10).unwrap())
    );
    assert_eq!(vec!["c", "b"], keys(s.events(None, true, None, 10).unwrap()));
    assert_eq!(vec!["a", "c"], keys(s.events(None, false, Some(21), 10).unwrap()));
    assert_eq!(1, s.events(None, false, None, 1).unwrap().len());
    let e = &s.events(Some("q"), false, None, 1).unwrap()[0];
    assert_eq!(("r2", "q", 2), (e.run_id.as_str(), e.pair_id.as_str(), e.seq));
    assert_eq!(1, s.uploads_of("q", "a", 10).unwrap().len());
    assert!(
        s.uploads_of("q", "c", 10).unwrap().is_empty(),
        "skips are not uploads"
    );
}

#[test]
fn runs_since_a_time() {
    let s = store();
    s.start_run("old", "p", "manual", 10).unwrap();
    s.start_run("new", "p", "manual", 30).unwrap();
    s.finish_run("new", RunStatus::Success, &RunStats::default(), None, 31)
        .unwrap();
    assert_eq!(
        vec!["new"],
        s.runs_since(20)
            .unwrap()
            .iter()
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn bin_totals_and_age() {
    let s = store();
    for (id, at, size) in [("1", 10, 100), ("2", 20, 50)] {
        s.add_to_bin(&NewBinEntry {
            id: id.into(),
            pair_id: "p".into(),
            side: BinSide::Local,
            key: format!("k{id}"),
            reason: "DELETED".into(),
            location: format!("l{id}"),
            deleted_at: at,
            expires_at: at + 100,
            size,
            account_id: None,
            remote_file_id: None,
            display_name: None,
            pair_name: None,
        })
        .unwrap();
    }
    assert_eq!(150, s.bin_total_size().unwrap());
    assert_eq!(
        vec!["1"],
        s.bin_entries_before(15)
            .unwrap()
            .iter()
            .map(|b| b.entry.id.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_resolved_conflict_can_be_undone() {
    let s = store();
    let c = ConflictRecord {
        id: "c".into(),
        pair_id: "p".into(),
        key: "a".into(),
        detected_at: 1,
        policy: ConflictPolicy::KeepBoth,
        local_copy_rel: None,
        remote_file_id: None,
        message: "m".into(),
        resolved: false,
        resolved_at: None,
    };
    s.add_conflict(&c).unwrap();
    s.resolve_conflict("c", 5).unwrap();
    assert!(s.conflict("c").unwrap().unwrap().resolved);
    s.unresolve_conflict("c").unwrap();
    assert_eq!(Some(c), s.conflict("c").unwrap());
    assert_eq!(None, s.conflict("missing").unwrap());
}

#[test]
fn runs_started_in_the_same_millisecond_list_newest_first() {
    let mut s = store();
    for id in ["first", "second", "third"] {
        s.start_run(id, "p", "manual", 100).unwrap();
        s.append_events(id, &[ev(100, EventLevel::Info, "UPLOAD", id)])
            .unwrap();
    }
    let ids: Vec<String> = s.runs(Some("p"), 10).unwrap().into_iter().map(|r| r.id).collect();
    assert_eq!(vec!["third", "second", "first"], ids);
    assert_eq!(
        vec!["third", "second", "first"],
        s.runs_since(0)
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect::<Vec<_>>()
    );
    let keys: Vec<String> = s
        .events(None, false, None, 10)
        .unwrap()
        .into_iter()
        .map(|e| e.event.key.unwrap())
        .collect();
    assert_eq!(vec!["third", "second", "first"], keys);
}
