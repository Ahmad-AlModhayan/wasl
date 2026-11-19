//! Folder status through the phone's binding (issue #31): the same situations the daemon's tests
//! seed read the same here, and the status callback fires once per change.

use std::sync::{Arc, Mutex};
use wasl_core::conditions::DeviceState;
use wasl_mobile::{
    EngineInit, Platform, SafBridge, StatusSink, TokenSource, WaslEngine, WaslError, outcome_is_quiet,
};
use wasl_proto::{FolderState, FolderStatus, Lang, OverallState, WaitReason};
use wasl_store::Store;

#[path = "../../wasl-engine/tests/common/status_scenarios.rs"]
mod scenarios;

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

/// Collects what the app is told.
#[derive(Default)]
struct Sink(Mutex<Vec<(String, FolderStatus)>>);
impl StatusSink for Sink {
    fn on_folder_status(&self, pair_id: String, status: FolderStatus) {
        self.0.lock().unwrap().push((pair_id, status));
    }
}
impl Sink {
    fn calls(&self) -> usize {
        self.0.lock().unwrap().len()
    }
    fn last(&self) -> FolderStatus {
        self.0.lock().unwrap().last().expect("a call").1.clone()
    }
}

/// An engine over a database the scenario seeded first (one process holds the store).
fn engine_with(dir: &std::path::Path, seed: impl FnOnce(&Store, &str)) -> Arc<WaslEngine> {
    let db = dir.join("wasl.db");
    let root = std::env::temp_dir().join("wasl-status-scenario");
    seed(&Store::open(&db).unwrap(), root.to_str().unwrap());
    WaslEngine::new(
        EngineInit {
            db_path: db.to_string_lossy().into_owned(),
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

#[test]
fn the_shared_status_scenarios_read_as_expected_through_the_binding() {
    for sc in scenarios::scenarios() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine_with(dir.path(), sc.seed);
        let statuses = engine.folder_statuses(Lang::En).unwrap();
        assert_eq!(1, statuses.len(), "{}", sc.name);
        assert_eq!(None, scenarios::mismatch(&statuses[0], &sc.expect), "{}", sc.name);
        assert_eq!(
            sc.expect.overall,
            engine.overall_status(Lang::En).unwrap().state,
            "{}: the whole app",
            sc.name
        );
        let ar = engine.folder_statuses(Lang::Ar).unwrap();
        assert_eq!(
            None,
            scenarios::mismatch(&ar[0], &sc.expect),
            "{} in Arabic: the same keys",
            sc.name
        );
        assert_ne!(
            statuses[0].words.text, ar[0].words.text,
            "{}: other words",
            sc.name
        );
    }
}

#[test]
fn the_callback_fires_once_per_status_change_and_not_for_a_repeat() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine_with(dir.path(), |s, root| {
        // Synced: the base pair, nothing held.
        s.set_account_email(scenarios::ACCOUNT, scenarios::ACCOUNT)
            .unwrap();
        s.create_pair(&scenarios::pair(root)).unwrap();
    });
    let sink = Arc::new(Sink::default());
    let now = engine.watch_status(Lang::En, sink.clone()).unwrap();
    assert_eq!(FolderState::Synced, now[0].state, "what the watch starts from");
    assert_eq!(0, sink.calls(), "the start is returned, not called back");

    engine.set_paused("p1".into(), true, None).unwrap();
    assert_eq!(1, sink.calls());
    assert_eq!(
        (FolderState::Waiting, Some(WaitReason::Paused)),
        (sink.last().state, sink.last().wait)
    );
    // The same state again is no change.
    engine.set_paused("p1".into(), true, None).unwrap();
    engine.refresh_status();
    assert_eq!(1, sink.calls(), "a repeat is not a change");

    engine.set_paused("p1".into(), false, None).unwrap();
    assert_eq!(2, sink.calls());
    assert_eq!(FolderState::Synced, sink.last().state);

    engine
        .set_pair_flags("p1".into(), None, None, Some(true))
        .unwrap();
    assert_eq!(3, sink.calls());
    assert_eq!("folder.needs_you", sink.last().words.key);
    assert_eq!("folder.first_preview", sink.last().needs_you[0].key);

    // What the call gave is what the app is told: the same status as a read.
    assert_eq!(engine.folder_statuses(Lang::En).unwrap()[0], sink.last());

    engine.unwatch_status();
    engine
        .set_pair_flags("p1".into(), None, None, Some(false))
        .unwrap();
    assert_eq!(3, sink.calls(), "no calls after unwatch");
}

#[test]
fn a_watch_in_arabic_is_told_in_arabic_with_the_same_keys() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine_with(dir.path(), |s, root| {
        s.set_account_email(scenarios::ACCOUNT, scenarios::ACCOUNT)
            .unwrap();
        s.create_pair(&scenarios::pair(root)).unwrap();
    });
    let sink = Arc::new(Sink::default());
    engine.watch_status(Lang::Ar, sink.clone()).unwrap();
    engine
        .set_pair_flags("p1".into(), None, None, Some(true))
        .unwrap();
    let told = sink.last();
    assert_eq!("folder.needs_you", told.words.key);
    assert!(told.words.text.contains("يحتاج"), "{}", told.words.text);
    let en = engine.folder_statuses(Lang::En).unwrap()[0].clone();
    assert_eq!(en.words.key, told.words.key);
    assert_eq!(en.needs_you[0].values, told.needs_you[0].values);
}

#[test]
fn the_older_calls_keep_working_beside_the_new_ones() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine_with(dir.path(), |s, root| {
        s.set_account_email(scenarios::ACCOUNT, scenarios::ACCOUNT)
            .unwrap();
        s.create_pair(&scenarios::pair(root)).unwrap();
    });
    assert_eq!(1, engine.pairs().unwrap().len());
    assert!(!engine.is_running("p1".into()));
    assert!(engine.runs(None, 5).unwrap().is_empty());
    assert!(outcome_is_quiet(&wasl_engine::RunOutcome {
        run_id: String::new(),
        status: wasl_core::RunStatus::Skipped,
        message: None,
        summary: None,
        error_kind: None,
        blocker: None,
    }));
    // A pair deleted drops out of the statuses, and an engine nobody watches pays nothing.
    engine.delete_pair("p1".into()).unwrap();
    assert!(engine.folder_statuses(Lang::En).unwrap().is_empty());
    assert_eq!(OverallState::Idle, engine.overall_status(Lang::En).unwrap().state);
}

#[test]
fn a_picked_folder_has_no_free_space_to_report() {
    // A picked folder (a content address) has no path to ask the system about: it can't read as a
    // full disk by itself, only by the error a run reports.
    let dir = tempfile::tempdir().unwrap();
    let engine = engine_with(dir.path(), |s, _| {
        s.set_account_email(scenarios::ACCOUNT, scenarios::ACCOUNT)
            .unwrap();
        let mut p = scenarios::pair("content://tree/primary%3ASync");
        p.storage_mode = wasl_store::StorageMode::Saf;
        s.create_pair(&p).unwrap();
    });
    let status = &engine.folder_statuses(Lang::En).unwrap()[0];
    assert_eq!(FolderState::Synced, status.state);
}
