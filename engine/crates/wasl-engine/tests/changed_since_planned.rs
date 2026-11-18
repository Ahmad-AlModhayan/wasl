//! A file edited after the plan was made is left alone, not binned or overwritten (wasl#16).

mod common;

use common::{Hooked, World};
use std::path::PathBuf;
use wasl_core::RunStatus;
use wasl_engine::CHANGED_SINCE_PLANNED;

/// A world whose first download runs `edit` against the pair folder (given its path).
fn world_editing(edit: impl FnOnce(PathBuf) + Send + 'static) -> World {
    // The folder is known once the world exists: a shared cell fills it in.
    let cell: std::sync::Arc<std::sync::Mutex<Option<PathBuf>>> = Default::default();
    let c = cell.clone();
    let w = World::with(
        |_| {},
        Some(Hooked::wrap(move || {
            let root = c.lock().unwrap().clone().unwrap();
            edit(root)
        })),
    );
    *cell.lock().unwrap() = Some(w.env.root.clone());
    w
}

fn reported(w: &World, run_id: &str) -> Vec<String> {
    w.engine
        .store()
        .lock()
        .run_events(run_id)
        .unwrap()
        .into_iter()
        .filter(|e| e.action == CHANGED_SINCE_PLANNED)
        .filter_map(|e| e.key)
        .collect()
}

#[tokio::test]
async fn a_file_edited_after_planning_is_not_binned_by_a_local_delete() {
    let w = world_editing(|root| std::fs::write(root.join("a.txt"), "edited after the plan").unwrap());
    w.write("a.txt", "synced");
    // The first world run has no download yet: the hook waits for b.txt.
    assert_eq!(RunStatus::Success, w.sync().await.status);
    w.drive.trash_path("Sync/a.txt");
    w.put_drive("b.txt", "new on drive");
    let run = w.sync().await;
    assert_eq!(Some("edited after the plan".to_owned()), w.read("a.txt"));
    assert_eq!(vec!["a.txt".to_owned()], reported(&w, &run.run_id));
    assert!(
        w.engine.store().lock().bin_entries(None).unwrap().is_empty(),
        "nothing was binned"
    );
    // The next run plans it again from what is on disk: the edit survives it.
    w.sync().await;
    assert_eq!(Some("edited after the plan".to_owned()), w.read("a.txt"));
}

#[tokio::test]
async fn a_file_edited_after_planning_is_not_overwritten_by_a_download() {
    let w = world_editing(|root| std::fs::write(root.join("a.txt"), "my edit while it downloaded").unwrap());
    w.write("a.txt", "old");
    // Nothing downloads yet, so the hook is still waiting.
    assert_eq!(RunStatus::Success, w.sync().await.status);
    w.put_drive("a.txt", "new from drive");
    let run = w.sync().await;
    assert_eq!(vec!["a.txt".to_owned()], reported(&w, &run.run_id));
    assert_eq!(Some("my edit while it downloaded".to_owned()), w.read("a.txt"));
    // Planned again: both versions are kept, neither is lost.
    w.sync().await;
    let all: Vec<String> = w.local_files().into_values().collect();
    assert!(all.contains(&"my edit while it downloaded".to_owned()), "{all:?}");
    assert!(all.contains(&"new from drive".to_owned()), "{all:?}");
}

#[tokio::test]
async fn a_file_replaced_by_a_folder_after_planning_is_left_alone() {
    let w = world_editing(|root| {
        std::fs::remove_file(root.join("a.txt")).unwrap();
        std::fs::create_dir_all(root.join("a.txt")).unwrap();
        std::fs::write(root.join("a.txt/inner.txt"), "inside").unwrap();
    });
    w.write("a.txt", "synced");
    assert_eq!(RunStatus::Success, w.sync().await.status);
    w.drive.trash_path("Sync/a.txt");
    w.put_drive("b.txt", "new on drive");
    let run = w.sync().await;
    assert_eq!(vec!["a.txt".to_owned()], reported(&w, &run.run_id));
    assert_eq!(Some("inside".to_owned()), w.read("a.txt/inner.txt"));
}

#[tokio::test]
async fn a_file_that_still_matches_the_plan_goes_to_the_bin_as_before() {
    let w = World::new();
    w.write("a.txt", "synced");
    // Others stay, so the deletion is not "everything" (which a sync holds back for review).
    for n in ["k1.txt", "k2.txt", "k3.txt", "k4.txt"] {
        w.write(n, "kept");
    }
    assert_eq!(RunStatus::Success, w.sync().await.status);
    w.drive.trash_path("Sync/a.txt");
    let run = w.sync().await;
    assert_eq!(None, w.read("a.txt"));
    assert!(reported(&w, &run.run_id).is_empty());
    assert_eq!(1, w.engine.store().lock().bin_entries(None).unwrap().len());
}
