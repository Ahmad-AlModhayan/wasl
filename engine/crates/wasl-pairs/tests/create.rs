//! Creating a pair through the pair rules (ADR-0001, ticket #35), the same for every app.

use std::path::PathBuf;
use std::sync::Arc;
use wasl_core::Route;
use wasl_pairs::{PairError, Pairs, PairsEnv, Problem};
use wasl_store::{Pair, SharedStore, Store, View};

struct Env {
    own: Vec<PathBuf>,
    vault_refused: bool,
}

#[async_trait::async_trait]
impl PairsEnv for Env {
    fn own_dirs(&self) -> Vec<PathBuf> {
        self.own.clone()
    }
    async fn link_vault(&self, pair: &mut Pair) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.vault_refused {
            return Err("Wasl can't tell yet whether that Drive folder is a vault. Check the connection and try again.".into());
        }
        pair.vault_id = Some("found".into());
        Ok(())
    }
}

struct World {
    store: SharedStore,
    pairs: Pairs,
    dir: tempfile::TempDir,
}

fn world_with(vault_refused: bool) -> World {
    let dir = tempfile::tempdir().unwrap();
    let store = SharedStore::new(Store::open_in_memory().unwrap());
    let env = Arc::new(Env {
        own: vec![dir.path().join("wasl-data")],
        vault_refused,
    });
    World {
        pairs: Pairs::new(store.clone(), env),
        store,
        dir,
    }
}

fn world() -> World {
    world_with(false)
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
    fn saved(&self) -> Vec<String> {
        self.store
            .lock()
            .list_pairs()
            .unwrap()
            .into_iter()
            .map(|p| p.id)
            .collect()
    }
}

fn rule(r: Result<Pair, PairError>) -> Problem {
    match r {
        Err(PairError::Rule(p)) => p,
        other => panic!("expected a broken rule, got {other:?}"),
    }
}

#[tokio::test]
async fn a_new_pair_is_recorded_held_for_its_first_preview() {
    let w = world();
    let made = w
        .pairs
        .create(drive_pair("p", "Work", &w.path("work")))
        .await
        .unwrap();
    assert!(made.status.hold_for_preview);
    let stored = w.store.lock().get_pair("p").unwrap().unwrap();
    assert!(stored.status.hold_for_preview, "held, whichever app made it");
    assert_eq!(
        Some("found".into()),
        stored.vault_id,
        "linked before it was recorded"
    );
}

#[tokio::test]
async fn a_pair_on_another_pairs_files_is_refused_naming_that_pair() {
    let w = world();
    w.pairs
        .create(drive_pair("p", "Work", &w.path("home/work")))
        .await
        .unwrap();
    for (id, root) in [
        ("inside", "home/work/notes"),
        ("around", "home"),
        ("same", "home/work"),
    ] {
        let p = rule(w.pairs.create(drive_pair(id, "Other", &w.path(root))).await);
        assert!(
            matches!(&p, Problem::Overlaps { pair, .. } if pair == "Work"),
            "{root}: {p:?}"
        );
    }
    assert!(
        w.pairs
            .create(drive_pair("beside", "Beside", &w.path("home/works")))
            .await
            .is_ok(),
        "a folder whose name starts the same is another folder"
    );
    assert_eq!(vec!["beside", "p"], w.saved(), "nothing refused was saved");
}

#[tokio::test]
async fn a_pair_in_or_around_wasls_own_data_is_refused() {
    let w = world();
    for root in ["wasl-data", "wasl-data/inner", ""] {
        let p = rule(w.pairs.create(drive_pair("p", "P", &w.path(root))).await);
        assert!(matches!(p, Problem::OwnData { .. }), "{root}: {p:?}");
    }
}

#[tokio::test]
async fn a_pair_in_or_around_a_view_is_refused() {
    let w = world();
    w.store
        .lock()
        .create_view(&View {
            id: "v".into(),
            name: "Photos view".into(),
            account_id: "me@example.com".into(),
            drive_folder_id: "f".into(),
            drive_id: None,
            remote_path: "My Drive/Photos".into(),
            local_path: w.path("views/photos"),
            vault_id: None,
            cache_limit: 0,
            enabled: true,
            created_at: 1,
            last_refresh_at: None,
            last_error: None,
        })
        .unwrap();
    let p = rule(w.pairs.create(drive_pair("p", "P", &w.path("views"))).await);
    assert!(
        matches!(&p, Problem::AroundView { view, .. } if view == "Photos view"),
        "{p:?}"
    );
}

// Android only: `/storage/emulated/0/...` is a full path on Android (and any Unix), not on Windows,
// where a pair can't have it.
#[cfg(unix)]
#[tokio::test]
async fn a_folder_picked_on_android_meets_a_path_pair_on_the_same_files() {
    let w = world();
    w.pairs
        .create(drive_pair("path", "Camera", "/storage/emulated/0/DCIM/Camera"))
        .await
        .unwrap();
    let picked = "content://com.android.externalstorage.documents/tree/primary%3ADCIM";
    let p = rule(w.pairs.create(drive_pair("picked", "DCIM", picked)).await);
    assert!(
        matches!(&p, Problem::Overlaps { pair, .. } if pair == "Camera"),
        "{p:?}"
    );
    let card = "content://com.android.externalstorage.documents/tree/1A2B-3C4D%3ADCIM";
    assert!(
        w.pairs.create(drive_pair("card", "Card", card)).await.is_ok(),
        "the same name on the SD card is another folder"
    );
}

#[tokio::test]
async fn a_drive_pair_needs_its_drive_folder_and_a_devices_pair_does_not() {
    let w = world();
    let p = rule(w.pairs.create(Pair::new("p", "P", &w.path("p"))).await);
    assert_eq!(Problem::NoDriveFolder, p);
    let mut devices = Pair::new("d", "D", &w.path("d"));
    devices.route = Route::Devices;
    assert!(w.pairs.create(devices).await.is_ok());
}

#[tokio::test]
async fn a_pair_needs_a_plain_id_and_a_chosen_folder() {
    let w = world();
    assert_eq!(
        Problem::InvalidId,
        rule(w.pairs.create(drive_pair("a/b", "P", &w.path("p"))).await)
    );
    assert_eq!(
        Problem::NoFolder,
        rule(w.pairs.create(drive_pair("p", "P", "relative/folder")).await)
    );
}

#[tokio::test]
async fn a_pair_whose_vault_cant_be_told_is_not_recorded() {
    let w = world_with(true);
    let err = w
        .pairs
        .create(drive_pair("p", "P", &w.path("p")))
        .await
        .unwrap_err();
    assert!(matches!(err, PairError::Vault(_)), "{err:?}");
    assert!(w.saved().is_empty());
}

#[tokio::test]
async fn check_gives_the_answer_a_save_would() {
    let w = world();
    w.pairs
        .create(drive_pair("p", "Work", &w.path("work")))
        .await
        .unwrap();
    let draft = drive_pair("q", "Notes", &w.path("work/notes"));
    let checked = w.pairs.check(&draft).unwrap_err();
    let saved = w.pairs.create(draft).await.unwrap_err();
    assert_eq!(checked.to_string(), saved.to_string());
}
