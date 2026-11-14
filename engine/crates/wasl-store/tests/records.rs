//! Pairs, history, bin, conflicts, settings, the persistent Drive cache and upload sessions, and the
//! v1 → v2 upgrade.

mod common;

use common::root;

use wasl_core::conditions::Conditions;
use wasl_core::{BaselineEntry, ConflictPolicy, DocsFormat, ErrorKind, Reach, Route, RunStatus, SyncMode};
use wasl_drive::{AccountState, DriveNode, NodeStore, SessionStore};
use wasl_store::{
    BinSide, ConflictRecord, EventLevel, NewBinEntry, Pair, PairStatus, RunEvent, RunStats, SharedStore,
    StorageMode, Store,
};

fn store_with(ids: &[&str]) -> Store {
    let s = Store::open_in_memory().unwrap();
    for id in ids {
        s.create_pair(&Pair::new(id, id, &root("data"))).unwrap();
    }
    s
}

fn entry(key: &str) -> BaselineEntry {
    BaselineEntry {
        key: key.into(),
        local_rel: key.into(),
        is_dir: false,
        local_size: 1,
        local_mtime: 1,
        local_hash: Some("h".into()),
        remote_id: Some(format!("id-{key}")),
        remote_md5: Some("h".into()),
        remote_size: Some(1),
        remote_mtime: Some(1),
        remote_version: Some(1),
        export_mime: None,
    }
}

// ---------------------------------------------------------------- pairs

fn full_pair() -> Pair {
    Pair {
        id: "p1".into(),
        name: "Projects".into(),
        account_id: Some("me@example.com".into()),
        local_root: "content://com.android.externalstorage.documents/tree/primary%3AProjects".into(),
        storage_mode: StorageMode::Saf,
        drive_folder_id: Some("F1".into()),
        remote_path: "My Drive/Projects".into(),
        drive_id: Some("D1".into()),
        route: Route::DriveAndDevices,
        reach: Reach::LanAndInternet,
        method: SyncMode::UploadMirror,
        conflict_policy: ConflictPolicy::NewerWins,
        enabled: false,
        paused: true,
        pause_reason: Some("Drive is full".into()),
        conditions: Conditions {
            wifi_only: false,
            allow_metered: true,
            allow_roaming: true,
            require_charging: true,
            min_battery_percent: 30,
            ssid_allow: vec!["Home".into(), "مكتب".into()],
            ssid_deny: vec!["Cafe".into()],
            require_vpn: true,
        },
        include_patterns: vec!["*.jpg".into(), "*.png".into()],
        exclude_patterns: vec!["~*".into()],
        excluded_subfolders: vec!["Raw".into()],
        sync_hidden: true,
        size_limit: 2_000_000_000,
        propagate_deletions: false,
        mass_delete_guard: false,
        docs_export: false,
        docs_format: DocsFormat::Pdf,
        preserve_mtime: false,
        vault_id: Some("v1".into()),
        honor_gitignore: true,
        presets: vec!["node".into(), "rust".into()],
        interval_minutes: 30,
        instant_sync: false,
        created_at: 100,
        updated_at: 200,
        status: PairStatus {
            last_run_at: Some(300),
            last_success_at: Some(250),
            last_status: Some(RunStatus::Partial),
            last_error: Some("2 item(s) failed".into()),
            last_error_kind: Some(ErrorKind::Network),
            consecutive_failures: 3,
            pending_approval: true,
            approve_once: true,
            hold_for_preview: true,
        },
    }
}

#[test]
fn every_pair_field_round_trips() {
    let s = Store::open_in_memory().unwrap();
    let p = full_pair();
    s.create_pair(&p).unwrap();
    assert_eq!(Some(p.clone()), s.get_pair("p1").unwrap());
    let defaults = Pair::new("p2", "Defaults", "/x");
    s.create_pair(&defaults).unwrap();
    assert_eq!(Some(defaults), s.get_pair("p2").unwrap());
    assert_eq!(2, s.list_pairs().unwrap().len());
}

#[test]
fn update_saves_options_but_not_status() {
    let s = Store::open_in_memory().unwrap();
    let p = full_pair();
    s.create_pair(&p).unwrap();
    let mut edited = p.clone();
    edited.name = "Renamed".into();
    edited.method = SyncMode::TwoWay;
    edited.status = PairStatus::default(); // the UI's copy may be stale
    s.update_pair(&edited).unwrap();
    let got = s.get_pair("p1").unwrap().unwrap();
    assert_eq!("Renamed", got.name);
    assert_eq!(SyncMode::TwoWay, got.method);
    assert_eq!(p.status, got.status, "status is the engine's to write");
}

#[tokio::test]
async fn repointing_a_pair_drops_its_sync_state_and_holds_it_for_preview() {
    let shared = SharedStore::new(store_with(&["p", "q"]));
    let ss: &dyn SessionStore = &shared;
    ss.put("p|a.txt|h", "https://upload/1").await.unwrap();
    ss.put("q|a.txt|h", "https://upload/2").await.unwrap();
    {
        let mut s = shared.lock();
        s.put_baseline("p", &entry("a.txt")).unwrap();
        s.put_baseline("q", &entry("a.txt")).unwrap();
        s.start_run("r1", "p", "manual", 10).unwrap();
        let stats = RunStats {
            skipped: 2,
            ..Default::default()
        };
        s.finish_run("r1", RunStatus::Success, &stats, None, 15).unwrap();
        s.put_blocked_plan("p", "mass delete", "{}", 20).unwrap();
        assert_eq!(Some(1), s.folder_bytes("p").unwrap());
        assert_eq!(Some(2), s.last_skipped("p").unwrap());

        let mut moved = s.get_pair("p").unwrap().unwrap();
        moved.local_root = root("elsewhere");
        assert!(s.update_pair(&moved).unwrap(), "another folder moves the pair");
        let got = s.get_pair("p").unwrap().unwrap();
        assert_eq!(root("elsewhere"), got.local_root);
        assert!(
            got.status.hold_for_preview,
            "the first run after the change is a preview"
        );
        assert!(!got.status.pending_approval && !got.status.approve_once);
        assert!(s.load_baseline("p").unwrap().is_empty());
        assert_eq!(None, s.folder_bytes("p").unwrap());
        assert_eq!(None, s.blocked_plan("p").unwrap());
        assert_eq!(1, s.runs(Some("p"), 10).unwrap().len(), "history is kept");
        assert_eq!(1, s.load_baseline("q").unwrap().len(), "other pairs keep theirs");
    }
    assert_eq!(None, ss.get("p|a.txt|h").await.unwrap());
    assert!(ss.get("q|a.txt|h").await.unwrap().is_some());
}

#[test]
fn status_and_pause_are_written_separately() {
    let s = store_with(&["p"]);
    let st = PairStatus {
        last_status: Some(RunStatus::Blocked),
        pending_approval: true,
        ..Default::default()
    };
    s.set_pair_status("p", &st).unwrap();
    s.set_paused("p", true, Some("Storage full")).unwrap();
    let got = s.get_pair("p").unwrap().unwrap();
    assert_eq!(st, got.status);
    assert!(got.paused);
    assert_eq!(Some("Storage full".into()), got.pause_reason);
    s.set_paused("p", false, Some("ignored")).unwrap();
    assert_eq!(None, s.get_pair("p").unwrap().unwrap().pause_reason);
}

#[test]
fn blocked_plans_round_trip() {
    let s = store_with(&["p"]);
    assert_eq!(None, s.blocked_plan("p").unwrap());
    s.put_blocked_plan("p", "would delete 30 files", "{\"deleteRemote\":30}", 9)
        .unwrap();
    assert_eq!(
        Some(("would delete 30 files".into(), "{\"deleteRemote\":30}".into(), 9)),
        s.blocked_plan("p").unwrap()
    );
    s.delete_blocked_plan("p").unwrap();
    assert_eq!(None, s.blocked_plan("p").unwrap());
}

// ---------------------------------------------------------------- baseline subtree

#[test]
fn subtree_removal_matches_the_folder_exactly() {
    let mut s = store_with(&["p"]);
    for k in ["a", "a/x", "a/b/y", "ab", "a_c", "a%d/z", "b/a/x"] {
        s.put_baseline("p", &entry(k)).unwrap();
    }
    s.remove_baseline_subtree("p", "a").unwrap();
    let left: Vec<String> = s.load_baseline("p").unwrap().into_keys().collect();
    assert_eq!(
        vec!["a%d/z", "a_c", "ab", "b/a/x"],
        left,
        "LIKE wildcards must not widen the match"
    );
    assert_eq!(None, s.baseline_entry("p", "a/x").unwrap());
    assert_eq!(Some(entry("ab")), s.baseline_entry("p", "ab").unwrap());
}

// ---------------------------------------------------------------- history

#[test]
fn runs_and_their_events_round_trip_in_order() {
    let mut s = store_with(&["p", "q"]);
    s.start_run("r1", "p", "manual", 10).unwrap();
    s.start_run("r2", "q", "schedule", 20).unwrap();
    assert_eq!(RunStatus::Running, s.run("r1").unwrap().unwrap().status);
    let ev = |n: u64| RunEvent {
        at: n as i64,
        level: if n == 2 {
            EventLevel::Error
        } else {
            EventLevel::Info
        },
        action: "UPLOAD".into(),
        key: Some(format!("f{n}")),
        message: format!("m{n}"),
        detail: (n == 2).then(|| "md5 mismatch".into()),
        bytes: n * 10,
    };
    s.append_events("r1", &[ev(1), ev(2)]).unwrap();
    s.append_events("r1", &[ev(3)]).unwrap();
    assert_eq!(vec![ev(1), ev(2), ev(3)], s.run_events("r1").unwrap());

    let stats = RunStats {
        uploaded: 2,
        errors: 1,
        bytes_up: 30,
        ..Default::default()
    };
    s.finish_run("r1", RunStatus::Partial, &stats, Some("1 item(s) failed"), 15)
        .unwrap();
    let r = s.run("r1").unwrap().unwrap();
    assert_eq!(
        (
            RunStatus::Partial,
            stats,
            Some("1 item(s) failed".into()),
            Some(15)
        ),
        (r.status, r.stats, r.message, r.ended_at)
    );
    assert_eq!(
        vec!["r2", "r1"],
        s.runs(None, 10)
            .unwrap()
            .iter()
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(1, s.runs(Some("p"), 10).unwrap().len());
}

#[test]
fn stale_runs_close_and_old_runs_prune() {
    let s = store_with(&["p"]);
    s.start_run("old", "p", "manual", 10).unwrap();
    s.start_run("new", "p", "manual", 1000).unwrap();
    assert_eq!(2, s.close_stale_runs("Stopped before it finished", 2000).unwrap());
    assert_eq!(RunStatus::Cancelled, s.run("new").unwrap().unwrap().status);
    assert_eq!(1, s.prune_runs(500).unwrap());
    assert_eq!(None, s.run("old").unwrap());
    assert!(s.run("new").unwrap().is_some());
}

#[test]
fn conflicts_open_and_resolve() {
    let s = store_with(&["p"]);
    let c = ConflictRecord {
        id: "c1".into(),
        pair_id: "p".into(),
        key: "a.txt".into(),
        detected_at: 5,
        policy: ConflictPolicy::KeepBoth,
        local_copy_rel: Some("a (conflicted copy from Pixel, s).txt".into()),
        remote_file_id: Some("R".into()),
        message: "Both sides changed".into(),
        resolved: false,
        resolved_at: None,
    };
    s.add_conflict(&c).unwrap();
    assert_eq!(vec![c.clone()], s.open_conflicts(Some("p")).unwrap());
    s.resolve_conflict("c1", 9).unwrap();
    assert!(s.open_conflicts(None).unwrap().is_empty());
}

// ---------------------------------------------------------------- bin

fn binned(id: &str, key: &str, side: BinSide, at: i64) -> NewBinEntry {
    NewBinEntry {
        id: id.into(),
        pair_id: "p".into(),
        side,
        key: key.into(),
        reason: "OVERWRITTEN".into(),
        location: format!("loc-{id}"),
        deleted_at: at,
        expires_at: at + 30,
        size: 7,
        account_id: None,
        remote_file_id: None,
        display_name: Some("a.txt".into()),
        pair_name: Some("p".into()),
    }
}

#[test]
fn bin_numbers_versions_per_key_and_side() {
    let s = store_with(&["p"]);
    assert_eq!(1, s.add_to_bin(&binned("1", "a.txt", BinSide::Local, 1)).unwrap());
    assert_eq!(2, s.add_to_bin(&binned("2", "a.txt", BinSide::Local, 2)).unwrap());
    assert_eq!(1, s.add_to_bin(&binned("3", "a.txt", BinSide::Drive, 3)).unwrap());
    assert_eq!(1, s.add_to_bin(&binned("4", "b.txt", BinSide::Local, 4)).unwrap());
    let versions: Vec<(String, u32)> = s
        .bin_versions("p", "a.txt")
        .unwrap()
        .into_iter()
        .map(|b| (b.entry.id, b.version_no))
        .collect();
    assert_eq!(3, versions.len());
    assert_eq!(
        vec!["4", "3", "2", "1"],
        s.bin_entries(Some("p"))
            .unwrap()
            .iter()
            .map(|b| b.entry.id.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        binned("2", "a.txt", BinSide::Local, 2),
        s.bin_entry("2").unwrap().unwrap().entry
    );
}

#[test]
fn bin_expiry_and_version_cap() {
    let s = store_with(&["p"]);
    for v in 1..=7 {
        s.add_to_bin(&binned(&v.to_string(), "a.txt", BinSide::Local, v))
            .unwrap();
    }
    // Keep the newest 5: versions 1 and 2 are excess.
    let excess: Vec<u32> = s
        .excess_local_versions("p", "a.txt", 5)
        .unwrap()
        .iter()
        .map(|b| b.version_no)
        .collect();
    assert_eq!(vec![2, 1], excess);
    // Entry v expires at v + 30.
    let expired: Vec<String> = s
        .expired_bin_entries(33)
        .unwrap()
        .into_iter()
        .map(|b| b.entry.id)
        .collect();
    assert_eq!(vec!["1", "2", "3"], expired);
    s.remove_from_bin("1").unwrap();
    assert_eq!(None, s.bin_entry("1").unwrap());
}

#[test]
fn bin_entries_go_with_their_pair() {
    let s = store_with(&["p"]);
    s.add_to_bin(&binned("1", "a.txt", BinSide::Local, 1)).unwrap();
    s.delete_pair("p").unwrap();
    assert!(s.bin_entries(None).unwrap().is_empty());
}

// ---------------------------------------------------------------- settings

#[test]
fn settings_round_trip() {
    let s = Store::open_in_memory().unwrap();
    assert_eq!(None, s.setting("device_name").unwrap());
    s.set_setting("device_name", "Pixel 9").unwrap();
    s.set_setting("device_name", "جهازي").unwrap();
    assert_eq!(Some("جهازي".into()), s.setting("device_name").unwrap());
    s.remove_setting("device_name").unwrap();
    assert_eq!(None, s.setting("device_name").unwrap());
}

// ---------------------------------------------------------------- Drive cache and sessions

fn node(id: &str, parent: Option<&str>, trashed: bool) -> DriveNode {
    DriveNode {
        account_id: "me".into(),
        id: id.into(),
        name: format!("n-{id}"),
        mime_type: "text/plain".into(),
        parent_id: parent.map(Into::into),
        drive_id: None,
        size: Some(12),
        md5: Some("m".into()),
        modified_time: 1_700_000_000_000,
        version: Some(4),
        trashed,
        is_folder: false,
        is_shortcut: false,
        shortcut_target_id: None,
        shortcut_target_mime: None,
        shared: true,
        permission_count: Some(2),
        // Round-trips through the cache's JSON column (vault v2 listings read these).
        app_properties: Some(
            [("wasl.m".to_string(), format!("blob-{id}"))]
                .into_iter()
                .collect(),
        ),
    }
}

#[tokio::test]
async fn the_node_store_persists_the_drive_cache() {
    let shared = SharedStore::new(Store::open_in_memory().unwrap());
    let ns: &dyn NodeStore = &shared;
    assert_eq!(None, ns.account("me").await.unwrap());
    let acct = AccountState {
        id: "me".into(),
        root_folder_id: Some("ROOT".into()),
        changes_token: Some("100".into()),
        last_crawl_at: Some(5),
        last_delta_at: None,
    };
    ns.upsert_account(acct.clone()).await.unwrap();
    assert_eq!(Some(acct), ns.account("me").await.unwrap());
    ns.set_changes_token("me", Some("101"), 9).await.unwrap();
    let a = ns.account("me").await.unwrap().unwrap();
    assert_eq!((Some("101".into()), Some(9)), (a.changes_token, a.last_delta_at));

    ns.upsert_all(&[
        node("a", Some("ROOT"), false),
        node("b", Some("a"), false),
        node("t", None, true),
    ])
    .await
    .unwrap();
    assert_eq!(3, ns.count("me").await.unwrap());
    let mut live = ns.all_for_account("me").await.unwrap();
    live.sort_by(|x, y| x.id.cmp(&y.id));
    assert_eq!(
        vec![node("a", Some("ROOT"), false), node("b", Some("a"), false)],
        live,
        "trashed nodes are not listed"
    );
    ns.delete_ids("me", &["a".into()]).await.unwrap();
    assert_eq!(2, ns.count("me").await.unwrap());
    ns.clear_account("me").await.unwrap();
    assert_eq!(0, ns.count("me").await.unwrap());
}

#[tokio::test]
async fn the_session_store_persists_upload_sessions() {
    let shared = SharedStore::new(Store::open_in_memory().unwrap());
    let ss: &dyn SessionStore = &shared;
    assert_eq!(None, ss.get("me|p|a.bin|hash").await.unwrap());
    ss.put(
        "me|p|a.bin|hash",
        "https://www.googleapis.com/upload/drive/v3/files?upload_id=1",
    )
    .await
    .unwrap();
    assert_eq!(
        Some("https://www.googleapis.com/upload/drive/v3/files?upload_id=1".into()),
        ss.get("me|p|a.bin|hash").await.unwrap()
    );
    ss.remove("me|p|a.bin|hash").await.unwrap();
    assert_eq!(None, ss.get("me|p|a.bin|hash").await.unwrap());
}

// ---------------------------------------------------------------- upgrade

#[test]
fn a_v1_database_upgrades_without_losing_its_baseline() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wasl.db");
    {
        // A database exactly as schema v1 left it, with a pair and a baseline row.
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(include_str!("../migrations/0001_initial.sql"))
            .unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute(
            "INSERT INTO pairs (id, name, local_root, route, reach, method, conflict_policy) VALUES ('p','P','/d','drive','lan','TWO_WAY','KEEP_BOTH')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO file_state (pair_id, key, side, is_dir, size, mtime_ns, md5, local_name) VALUES ('p','a.txt','local',0,5,1000000,'h','a.txt')",
            [],
        )
        .unwrap();
    }
    let s = Store::open(&path).unwrap();
    assert_eq!(12, s.schema_version().unwrap());
    let p = s.get_pair("p").unwrap().unwrap();
    assert_eq!(
        ("P", true, StorageMode::Files),
        (p.name.as_str(), p.propagate_deletions, p.storage_mode)
    );
    let b = s.baseline_entry("p", "a.txt").unwrap().unwrap();
    assert_eq!(
        (5, 1, Some("h".to_owned())),
        (b.local_size, b.local_mtime, b.local_hash)
    );
}

#[test]
fn skipped_counts_the_last_run_that_carried_out_its_plan() {
    let s = store_with(&["p"]);
    let stats = |skipped| RunStats {
        skipped,
        ..Default::default()
    };
    s.start_run("r1", "p", "manual", 10).unwrap();
    s.finish_run("r1", RunStatus::Success, &stats(2), None, 15)
        .unwrap();
    s.start_run("r2", "p", "manual", 20).unwrap();
    s.finish_run("r2", RunStatus::Failed, &stats(9), None, 25)
        .unwrap();
    assert_eq!(
        Some(2),
        s.last_skipped("p").unwrap(),
        "a failed run carried out no plan"
    );
    s.connection()
        .execute("UPDATE runs SET stats_json = 'not json' WHERE id = 'r1'", [])
        .unwrap();
    assert_eq!(
        None,
        s.last_skipped("p").unwrap(),
        "unreadable stats are unknown, not zero"
    );
}
