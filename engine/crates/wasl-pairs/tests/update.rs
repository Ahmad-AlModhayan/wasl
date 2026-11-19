//! Editing and deleting a pair through the pair rules (ADR-0001, wasl#12), the same for every app:
//! the desktop service and the phone both call these, so a pair can't be edited into a state
//! creating it would have refused.

use std::path::PathBuf;
use std::sync::Arc;
use wasl_core::conditions::DeviceState;
use wasl_engine::{Engine, EngineConfig, EngineError, Environment, Remote, SystemClock};
use wasl_fs::LocalFs;
use wasl_pairs::{PairError, Pairs, PairsEnv, Problem};
use wasl_store::{Changeset, ChangesetStatus, Pair, SharedStore, Store};

struct Env {
    own: Vec<PathBuf>,
}

#[async_trait::async_trait]
impl PairsEnv for Env {
    fn own_dirs(&self) -> Vec<PathBuf> {
        self.own.clone()
    }
}

/// An engine that never runs a pair: saving and deleting need only its locks and its store.
struct Idle;

impl Environment for Idle {
    fn local_fs(&self, _: &Pair) -> wasl_engine::Result<Arc<dyn LocalFs>> {
        Err(EngineError::Other("not used".into()))
    }
    fn remote(&self, _: &Pair) -> wasl_engine::Result<Arc<dyn Remote>> {
        Err(EngineError::Other("not used".into()))
    }
    fn device_state(&self) -> DeviceState {
        DeviceState::unrestricted()
    }
}

struct World {
    store: SharedStore,
    pairs: Pairs,
    engine: Engine,
    dir: tempfile::TempDir,
}

fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let store = SharedStore::new(Store::open_in_memory().unwrap());
    let env = Arc::new(Env {
        own: vec![dir.path().join("wasl-data")],
    });
    let engine = Engine::new(
        store.clone(),
        Arc::new(Idle),
        Arc::new(SystemClock),
        EngineConfig::default(),
        None,
    );
    World {
        pairs: Pairs::new(store.clone(), env),
        engine,
        store,
        dir,
    }
}

fn drive_pair(id: &str, name: &str, root: &str) -> Pair {
    let mut p = Pair::new(id, name, root);
    p.account_id = Some("me@example.com".into());
    p.drive_folder_id = Some("folder".into());
    p
}

impl World {
    fn path(&self, rel: &str) -> String {
        self.dir.path().join(rel).to_string_lossy().into_owned()
    }
    async fn make(&self, id: &str, name: &str, rel: &str) -> Pair {
        self.pairs
            .create(drive_pair(id, name, &self.path(rel)))
            .await
            .unwrap()
    }
    fn stored(&self, id: &str) -> Pair {
        self.store.lock().get_pair(id).unwrap().unwrap()
    }
    fn open_agent_changes(&self, pair: &str) {
        self.store
            .lock()
            .create_changeset(
                &Changeset {
                    id: "cs".into(),
                    pair_id: Some(pair.into()),
                    actor: "agent:test".into(),
                    label: None,
                    scope: String::new(),
                    session: None,
                    status: ChangesetStatus::Open,
                    started_at: 1,
                    ended_at: None,
                    decided_at: None,
                },
                &[],
            )
            .unwrap();
    }
}

fn rule<T: std::fmt::Debug>(r: Result<T, PairError>) -> Problem {
    match r {
        Err(PairError::Rule(p)) => p,
        other => panic!("expected a broken rule, got {other:?}"),
    }
}

#[tokio::test]
async fn an_edit_onto_another_pairs_files_is_refused_with_creates_reason() {
    let w = world();
    w.make("a", "Work", "home/work").await;
    let b = w.make("b", "Play", "home/play").await;

    let mut onto = b.clone();
    onto.local_root = w.path("home/work/notes");
    let edit = rule(w.pairs.update(&w.engine, onto.clone()).await);
    onto.id = "new".into();
    let create = rule(w.pairs.create(onto).await);
    assert_eq!(create, edit, "the same reason, whichever way the pair gets there");
    assert!(
        matches!(&edit, Problem::Overlaps { pair, .. } if pair == "Work"),
        "{edit:?}"
    );
    assert_eq!(
        w.path("home/play"),
        w.stored("b").local_root,
        "nothing refused was saved"
    );
}

#[tokio::test]
async fn an_edit_is_checked_for_every_rule_a_new_pair_is() {
    let w = world();
    let p = w.make("p", "Work", "work").await;

    let mut own = p.clone();
    own.local_root = w.path("wasl-data/x");
    assert!(matches!(
        rule(w.pairs.update(&w.engine, own).await),
        Problem::OwnData { .. }
    ));

    let mut none = p.clone();
    none.local_root = String::new();
    assert_eq!(Problem::NoFolder, rule(w.pairs.update(&w.engine, none).await));

    let mut no_drive = p.clone();
    no_drive.drive_folder_id = None;
    assert_eq!(
        Problem::NoDriveFolder,
        rule(w.pairs.update(&w.engine, no_drive).await)
    );

    let mut bad_id = p.clone();
    bad_id.id = "a/b".into();
    assert_eq!(Problem::InvalidId, rule(w.pairs.update(&w.engine, bad_id).await));
    assert_eq!(p.local_root, w.stored("p").local_root);
}

#[tokio::test]
async fn an_edit_that_keeps_the_rules_saves_and_says_whether_the_pair_moved() {
    let w = world();
    let p = w.make("p", "Work", "work").await;

    let mut renamed = p.clone();
    renamed.name = "Renamed".into();
    assert!(
        !w.pairs.update(&w.engine, renamed).await.unwrap(),
        "same files: not moved"
    );
    assert_eq!("Renamed", w.stored("p").name);

    let mut moved = w.stored("p");
    moved.drive_folder_id = Some("other".into());
    moved.status.hold_for_preview = false;
    assert!(
        w.pairs.update(&w.engine, moved).await.unwrap(),
        "other Drive folder: moved"
    );
    assert!(
        w.stored("p").status.hold_for_preview,
        "a moved pair waits for its preview again"
    );
}

#[tokio::test]
async fn an_agents_waiting_changes_refuse_a_move_and_a_delete_the_same_typed_way() {
    let w = world();
    let p = w.make("p", "Work", "work").await;
    w.open_agent_changes("p");

    let mut moved = p.clone();
    moved.drive_folder_id = Some("other".into());
    assert_eq!(
        Problem::AgentChanges,
        rule(w.pairs.update(&w.engine, moved).await)
    );
    assert_eq!(Problem::AgentChanges, rule(w.pairs.delete(&w.engine, "p")));
    assert_eq!(
        wasl_store::AGENT_CHANGES_PENDING,
        Problem::AgentChanges.to_string(),
        "the words are the store's"
    );
    assert_eq!(
        Some("folder".into()),
        w.stored("p").drive_folder_id,
        "nothing changed"
    );

    // A rename keeps the pair where it is, so it still saves.
    let mut renamed = p;
    renamed.name = "Renamed".into();
    w.pairs.update(&w.engine, renamed).await.unwrap();
}

#[tokio::test]
async fn deleting_a_pair_removes_it() {
    let w = world();
    w.make("p", "Work", "work").await;
    w.pairs.delete(&w.engine, "p").unwrap();
    assert!(w.store.lock().get_pair("p").unwrap().is_none());
}
