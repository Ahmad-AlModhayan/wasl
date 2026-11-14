//! Agent changes (docs/AGENT_CHANGES.md): held until accepted, reverted in one step.

mod common;

use common::{PAIR, World};
use wasl_core::RunStatus;
use wasl_engine::{AGENT_REVERT, EngineConfig};
use wasl_store::{ChangesetStatus, EarlierVersion, EntryKind};

fn world() -> (World, tempfile::TempDir) {
    let agent = tempfile::tempdir().unwrap();
    let w = World::with_config(
        |_| {},
        None,
        EngineConfig {
            device_name: "PC".into(),
            agent_dir: Some(agent.path().to_owned()),
            ..EngineConfig::default()
        },
    );
    (w, agent)
}

/// A world with `a.txt`, `b.txt` and `notes/n.md` synced to Drive.
async fn synced() -> (World, tempfile::TempDir) {
    let (w, a) = world();
    w.write("a.txt", "a1");
    w.write("b.txt", "b1");
    w.write("notes/n.md", "n1");
    assert_eq!(RunStatus::Success, w.sync_from_preview().await.status);
    (w, a)
}

fn files(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

#[tokio::test]
async fn an_agents_changes_wait_until_accepted() {
    let (w, _a) = synced().await;
    let cs = w
        .engine
        .agent_begin(PAIR, "", "claude-code", Some("tidy notes"), None)
        .await
        .unwrap();
    assert_eq!("agent:claude-code", cs.actor);

    w.write("a.txt", "a2 by agent");
    w.write("c.txt", "new by agent");
    w.remove("b.txt");

    // While the session is open, sync leaves the agent's work alone.
    assert_eq!(RunStatus::Success, w.sync().await.status);
    let before = files(&[("a.txt", "a1"), ("b.txt", "b1"), ("notes/n.md", "n1")]);
    assert_eq!(before, w.drive_files());

    let ended = w.engine.agent_end(&cs.id).await.unwrap().unwrap();
    assert_eq!(ChangesetStatus::Held, ended.status);
    let d = w.engine.agent_changeset(&cs.id).unwrap();
    let kinds: Vec<(&str, EntryKind, EarlierVersion)> = d
        .entries
        .iter()
        .map(|e| (e.rel.as_str(), e.kind, e.source))
        .collect();
    assert_eq!(
        vec![
            ("a.txt", EntryKind::Changed, EarlierVersion::Synced),
            ("b.txt", EntryKind::Deleted, EarlierVersion::Synced),
            ("c.txt", EntryKind::Added, EarlierVersion::None),
        ],
        kinds
    );

    // Held after the session too.
    assert_eq!(RunStatus::Success, w.sync().await.status);
    assert_eq!(before, w.drive_files());

    w.engine.agent_accept(&cs.id).unwrap();
    assert_eq!(RunStatus::Success, w.sync().await.status);
    assert_eq!(
        files(&[
            ("a.txt", "a2 by agent"),
            ("c.txt", "new by agent"),
            ("notes/n.md", "n1")
        ]),
        w.drive_files()
    );
}

#[tokio::test]
async fn revert_puts_every_file_back_and_sync_has_nothing_to_do() {
    let (w, _a) = synced().await;
    let cs = w
        .engine
        .agent_begin(PAIR, "", "gemini", None, None)
        .await
        .unwrap();
    w.write("a.txt", "a2 by agent");
    w.write("notes/new/deep.md", "new by agent");
    w.remove("b.txt");
    w.engine.agent_end(&cs.id).await.unwrap();

    let report = w.engine.agent_revert(&cs.id).await.unwrap();
    assert!(
        report.skipped.is_empty() && report.failed.is_empty(),
        "{report:?}"
    );
    assert_eq!(
        files(&[("a.txt", "a1"), ("b.txt", "b1"), ("notes/n.md", "n1")]),
        w.local_files()
    );
    assert!(!w.path("notes/new").exists());
    assert_eq!(
        ChangesetStatus::Reverted,
        w.engine.agent_changeset(&cs.id).unwrap().changeset.status
    );

    // The agent's versions are in the bin, so the revert itself can be undone.
    let bin = w.engine.store().lock().bin_entries(Some(PAIR)).unwrap();
    let mut binned: Vec<&str> = bin
        .iter()
        .filter(|b| b.entry.reason == AGENT_REVERT)
        .map(|b| b.entry.key.as_str())
        .collect();
    binned.sort();
    assert_eq!(vec!["a.txt", "notes/new/deep.md"], binned);

    // Restored files match the baseline: nothing to sync either way.
    let p = w.engine.preview(PAIR).await.unwrap();
    assert!(
        p.plan
            .actions
            .iter()
            .all(|a| matches!(a, wasl_core::Action::Skip { .. })),
        "{:?}",
        p.plan.actions
    );
    assert_eq!(RunStatus::Success, w.sync().await.status);
    assert_eq!(
        files(&[("a.txt", "a1"), ("b.txt", "b1"), ("notes/n.md", "n1")]),
        w.drive_files()
    );
}

#[tokio::test]
async fn an_unsynced_edit_is_copied_at_begin_and_comes_back() {
    let (w, _a) = synced().await;
    w.write("a.txt", "my edit, not synced yet");
    let cs = w
        .engine
        .agent_begin(PAIR, "", "claude-code", None, None)
        .await
        .unwrap();
    w.write("a.txt", "agent overwrote it");
    w.engine.agent_end(&cs.id).await.unwrap();
    let e = &w.engine.agent_changeset(&cs.id).unwrap().entries[0];
    assert_eq!(EarlierVersion::Saved, e.source);

    w.engine.agent_revert(&cs.id).await.unwrap();
    assert_eq!(Some("my edit, not synced yet".into()), w.read("a.txt"));
    // The user's own edit syncs as usual afterwards.
    w.sync().await;
    assert_eq!(
        Some(&"my edit, not synced yet".to_owned()),
        w.drive_files().get("a.txt")
    );
}

#[tokio::test]
async fn a_copy_taken_before_a_write_is_the_earlier_version() {
    let (w, _a) = synced().await;
    let cs = w
        .engine
        .agent_begin(PAIR, "", "claude-code", None, Some("s1"))
        .await
        .unwrap();
    assert!(w.engine.agent_save(PAIR, "a.txt", Some("s1")).await.unwrap());
    // Only the first copy counts.
    assert!(!w.engine.agent_save(PAIR, "a.txt", Some("s1")).await.unwrap());
    w.write("a.txt", "agent");
    w.engine.agent_end(&cs.id).await.unwrap();
    let e = &w.engine.agent_changeset(&cs.id).unwrap().entries[0];
    assert_eq!(EarlierVersion::Saved, e.source);

    let diff = w.engine.agent_diff(&cs.id, "a.txt").await.unwrap();
    assert!(diff.contains("-a1") && diff.contains("+agent"), "{diff}");
    w.engine.agent_revert(&cs.id).await.unwrap();
    assert_eq!(Some("a1".into()), w.read("a.txt"));
}

/// Security review 2026-10: a program that can write the agent folder (an agent among them) must
/// not get its own content installed by a revert, nor shown as the earlier version by a diff.
#[tokio::test]
async fn a_copy_changed_after_it_was_saved_is_refused() {
    let (w, agent) = synced().await;
    let cs = w
        .engine
        .agent_begin(PAIR, "", "claude-code", None, Some("s1"))
        .await
        .unwrap();
    assert!(w.engine.agent_save(PAIR, "a.txt", Some("s1")).await.unwrap());
    w.write("a.txt", "agent");
    w.write("b.txt", "agent");
    w.engine.agent_end(&cs.id).await.unwrap();
    let d = w.engine.agent_changeset(&cs.id).unwrap();
    let e = d.entries.iter().find(|e| e.rel == "a.txt").unwrap();
    assert_eq!(EarlierVersion::Saved, e.source);
    let copy = agent.path().join(&cs.id).join(e.location.as_deref().unwrap());
    assert_eq!("a1", std::fs::read_to_string(&copy).unwrap());
    std::fs::write(&copy, "planted").unwrap();

    let err = w.engine.agent_diff(&cs.id, "a.txt").await.unwrap_err();
    assert!(
        err.to_string()
            .contains("the earlier copy was changed after it was saved"),
        "{err}"
    );
    let report = w.engine.agent_revert(&cs.id).await.unwrap();
    assert_eq!(vec!["b.txt".to_owned()], report.reverted);
    assert_eq!(1, report.failed.len(), "{report:?}");
    assert_eq!("a.txt", report.failed[0].0);
    assert!(
        report.failed[0]
            .1
            .contains("the earlier copy was changed after it was saved"),
        "{report:?}"
    );
    // The planted content never reached the pair; the agent's version stays.
    assert_eq!(Some("agent".into()), w.read("a.txt"));
    assert_eq!(Some("b1".into()), w.read("b.txt"));
    assert_eq!(
        ChangesetStatus::PartlyReverted,
        w.engine.agent_changeset(&cs.id).unwrap().changeset.status
    );
}

#[tokio::test]
async fn a_file_the_user_changed_after_the_session_is_not_reverted() {
    let (w, _a) = synced().await;
    let cs = w
        .engine
        .agent_begin(PAIR, "", "raqeeb", None, None)
        .await
        .unwrap();
    w.write("a.txt", "agent");
    w.write("b.txt", "agent");
    w.engine.agent_end(&cs.id).await.unwrap();
    w.write("a.txt", "mine, after the agent");

    let report = w.engine.agent_revert(&cs.id).await.unwrap();
    assert_eq!(vec!["a.txt".to_owned()], report.skipped);
    assert_eq!(vec!["b.txt".to_owned()], report.reverted);
    assert_eq!(Some("mine, after the agent".into()), w.read("a.txt"));
    assert_eq!(Some("b1".into()), w.read("b.txt"));
    assert_eq!(
        ChangesetStatus::PartlyReverted,
        w.engine.agent_changeset(&cs.id).unwrap().changeset.status
    );
}

#[tokio::test]
async fn only_the_scope_is_held() {
    let (w, _a) = synced().await;
    let cs = w
        .engine
        .agent_begin(PAIR, "notes", "claude-code", None, None)
        .await
        .unwrap();
    w.write("notes/n.md", "agent");
    w.write("a.txt", "mine, outside the agent's folder");
    w.sync().await;
    let drive = w.drive_files();
    assert_eq!(Some(&"n1".to_owned()), drive.get("notes/n.md"));
    assert_eq!(
        Some(&"mine, outside the agent's folder".to_owned()),
        drive.get("a.txt")
    );

    w.engine.agent_end(&cs.id).await.unwrap();
    let d = w.engine.agent_changeset(&cs.id).unwrap();
    assert_eq!(
        vec!["notes/n.md"],
        d.entries.iter().map(|e| e.rel.as_str()).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn overlapping_sessions_are_refused_and_a_session_id_resumes() {
    let (w, _a) = synced().await;
    let cs = w
        .engine
        .agent_begin(PAIR, "notes", "claude-code", None, Some("s1"))
        .await
        .unwrap();
    assert!(
        w.engine
            .agent_begin(PAIR, "", "gemini", None, None)
            .await
            .is_err()
    );
    assert!(
        w.engine
            .agent_begin(PAIR, "notes/x", "gemini", None, None)
            .await
            .is_err()
    );
    // A folder beside it is fine.
    let other = w
        .engine
        .agent_begin(PAIR, "docs", "gemini", None, None)
        .await
        .unwrap();
    assert_ne!(cs.id, other.id);
    let again = w
        .engine
        .agent_begin(PAIR, "notes", "claude-code", None, Some("s1"))
        .await
        .unwrap();
    assert_eq!(cs.id, again.id);
    assert!(wasl_engine::actor_id("Bad Name!").is_err());
}

#[tokio::test]
async fn what_wasl_downloads_during_a_session_is_not_the_agents() {
    let (w, _a) = synced().await;
    let cs = w
        .engine
        .agent_begin(PAIR, "", "claude-code", None, None)
        .await
        .unwrap();
    w.put_drive("from-laptop.txt", "made on the laptop");
    assert_eq!(RunStatus::Success, w.sync().await.status);
    assert_eq!(Some("made on the laptop".into()), w.read("from-laptop.txt"));
    // Nothing the agent did: the changeset goes away.
    assert_eq!(None, w.engine.agent_end(&cs.id).await.unwrap());
    assert!(w.engine.agent_changesets(Some(PAIR), true).unwrap().is_empty());
}

#[tokio::test]
async fn a_deletion_from_another_device_during_a_session_is_not_the_agents() {
    let (w, _a) = synced().await;
    let cs = w
        .engine
        .agent_begin(PAIR, "", "claude-code", None, None)
        .await
        .unwrap();
    w.write("a.txt", "agent");
    // The laptop deletes b.txt on Drive; this computer follows.
    assert!(w.drive.trash_path("Sync/b.txt"));
    assert_eq!(RunStatus::Success, w.sync().await.status);
    assert!(!w.path("b.txt").exists());

    w.engine.agent_end(&cs.id).await.unwrap().unwrap();
    let d = w.engine.agent_changeset(&cs.id).unwrap();
    assert_eq!(
        vec!["a.txt"],
        d.entries.iter().map(|e| e.rel.as_str()).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn an_interrupted_revert_carries_on() {
    let (w, _a) = synced().await;
    let cs = w
        .engine
        .agent_begin(PAIR, "", "claude-code", None, None)
        .await
        .unwrap();
    w.write("a.txt", "agent");
    w.write("b.txt", "agent");
    w.engine.agent_end(&cs.id).await.unwrap();
    // As if the process stopped after putting a.txt back.
    std::fs::write(w.path("a.txt"), "a1").unwrap();
    w.engine
        .store()
        .lock()
        .set_entry_outcome(&cs.id, "a.txt", "reverted")
        .unwrap();

    let report = w.engine.agent_revert(&cs.id).await.unwrap();
    assert!(
        report.skipped.is_empty() && report.failed.is_empty(),
        "{report:?}"
    );
    assert_eq!(2, report.reverted.len());
    assert_eq!(Some("b1".into()), w.read("b.txt"));
}

#[tokio::test]
async fn housekeeping_drops_old_decisions_and_stray_copies() {
    let (w, agent) = synced().await;
    let cs = w
        .engine
        .agent_begin(PAIR, "", "claude-code", None, None)
        .await
        .unwrap();
    w.write("a.txt", "agent");
    w.engine.agent_end(&cs.id).await.unwrap();
    w.engine.agent_accept(&cs.id).unwrap();
    let held = w
        .engine
        .agent_begin(PAIR, "", "gemini", None, None)
        .await
        .unwrap();
    w.write("b.txt", "unsynced, copied at begin");
    std::fs::create_dir_all(agent.path().join("left-by-a-crash")).unwrap();

    assert_eq!(0, w.engine.agent_housekeeping().unwrap(), "decided today: kept");
    assert!(!agent.path().join("left-by-a-crash").exists());
    w.clock.advance(wasl_engine::AGENT_HISTORY_MS + 1);
    assert_eq!(1, w.engine.agent_housekeeping().unwrap());
    let left: Vec<String> = w
        .engine
        .agent_changesets(None, true)
        .unwrap()
        .into_iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(vec![held.id], left);
}

#[tokio::test]
async fn a_session_in_an_ignored_or_deep_folder_scans_only_there() {
    let (w, _a) = synced().await;
    w.set_pair(|p| p.honor_gitignore = true);
    w.write("deep/er/x.md", "x1");
    w.write(".gitignore", "build/\n");
    w.write("build/out.bin", "o");
    w.sync().await;
    let cs = w
        .engine
        .agent_begin(PAIR, "deep/er", "claude-code", None, None)
        .await
        .unwrap();
    w.write("deep/er/x.md", "agent");
    w.write("a.txt", "outside the scope");
    let ended = w.engine.agent_end(&cs.id).await.unwrap().unwrap();
    let d = w.engine.agent_changeset(&ended.id).unwrap();
    assert_eq!(
        vec!["deep/er/x.md"],
        d.entries.iter().map(|e| e.rel.as_str()).collect::<Vec<_>>()
    );

    // An ignored folder holds nothing: Wasl never syncs it anyway.
    let cs = w
        .engine
        .agent_begin(PAIR, "build", "claude-code", None, None)
        .await
        .unwrap();
    w.write("build/out.bin", "rebuilt");
    assert_eq!(None, w.engine.agent_end(&cs.id).await.unwrap());
}

/// The same flow on a real folder tree: `WASL_AGENT_TREE=<folder> cargo test -p wasl-engine --test
/// agent -- --ignored --nocapture`. The tree is copied into the test's pair folder; Drive is the
/// in-memory one, standing in for the other computer.
#[tokio::test]
#[ignore = "needs WASL_AGENT_TREE"]
async fn a_real_tree_round_trips_through_hold_revert_and_accept() {
    let Some(src) = std::env::var_os("WASL_AGENT_TREE") else {
        return;
    };
    let scope = std::env::var("WASL_AGENT_SCOPE").ok();
    let (w, _a) = world();
    let root = w.path("");
    copy_tree(std::path::Path::new(&src), &root);
    let original = tree(&root);
    assert_eq!(RunStatus::Success, w.sync_from_preview().await.status);
    let drive_before = drive(&w);
    println!(
        "{} files here, {} synced to Drive",
        original.len(),
        drive_before.len()
    );

    // An agent works in one folder, editing, adding and deleting.
    let scope = scope.unwrap_or_else(|| {
        drive_before
            .keys()
            .find_map(|k| k.split_once('/').map(|(d, _)| d.to_owned()))
            .expect("a folder")
    });
    let mine: Vec<String> = drive_before
        .keys()
        .filter(|k| k.starts_with(&format!("{scope}/")))
        .cloned()
        .collect();
    println!("agent works in {scope} ({} synced files)", mine.len());
    let cs = w
        .engine
        .agent_begin(PAIR, &scope, "claude-code", None, None)
        .await
        .unwrap();
    w.write(&mine[0], "rewritten by the agent\n");
    if let Some(gone) = mine.get(1) {
        w.remove(gone);
    }
    w.write(&format!("{scope}/AGENT_NOTES.md"), "# added by the agent\n");
    w.sync().await;
    assert!(drive_before == drive(&w), "nothing leaves while the agent works");
    let held = w.engine.agent_end(&cs.id).await.unwrap().unwrap();
    println!(
        "held {:?}",
        w.engine
            .agent_changeset(&held.id)
            .unwrap()
            .entries
            .iter()
            .map(|e| (&e.rel, e.kind))
            .collect::<Vec<_>>()
    );

    let report = w.engine.agent_revert(&cs.id).await.unwrap();
    assert!(
        report.failed.is_empty() && report.skipped.is_empty(),
        "{report:?}"
    );
    assert!(original == tree(&root), "byte for byte as before");
    w.sync().await;
    assert!(drive_before == drive(&w));

    // Again, accepted this time: the other side gets exactly the agent's work.
    let cs = w
        .engine
        .agent_begin(PAIR, &scope, "gemini", None, None)
        .await
        .unwrap();
    w.write(&mine[0], "kept\n");
    w.engine.agent_end(&cs.id).await.unwrap().unwrap();
    w.engine.agent_accept(&cs.id).unwrap();
    assert_eq!(RunStatus::Success, w.sync().await.status);
    let mut expected = drive_before.clone();
    expected.insert(mine[0].clone(), b"kept\n".to_vec());
    assert!(expected == drive(&w));
}

fn drive(w: &World) -> std::collections::BTreeMap<String, Vec<u8>> {
    w.drive
        .files()
        .into_iter()
        .filter_map(|(k, v)| k.strip_prefix("Sync/").map(|k| (k.to_owned(), v)))
        .collect()
}

/// Every file under `root` with its bytes, the bin and part files left out.
fn tree(root: &std::path::Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            if e.file_type().unwrap().is_dir() {
                stack.push(p);
            } else if !p.to_string_lossy().ends_with(".wasl-part") {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                out.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    out
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        let ft = e.file_type().unwrap();
        if ft.is_dir() {
            copy_tree(&src, &dst);
        } else if ft.is_file() {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

// ---------------------------------------------------------------- the op-log record (#47, FR-OL7)

/// The op-log changeset of `id` and its operations.
fn oplog(w: &World, id: &str) -> (wasl_store::OplogChangeset, Vec<wasl_store::OplogOp>) {
    let k = wasl_store::OplogKey::new(common::DEVICE, id);
    let s = w.engine.store().lock();
    let cs = s
        .oplog_changeset(&k)
        .unwrap()
        .expect("the changeset is in the op-log");
    (cs, s.oplog_ops(&k).unwrap())
}

type OpRow = (u32, String, String, Option<u32>, Option<wasl_store::OpOutcome>);

/// Each operation as (seq, entity, op, reverts, outcome).
fn rows(ops: &[wasl_store::OplogOp]) -> Vec<OpRow> {
    ops.iter()
        .map(|o| (o.seq, o.entity_uri.clone(), o.op.clone(), o.reverts, o.outcome))
        .collect()
}

fn row(seq: u32, key: &str, op: &str, reverts: Option<u32>, outcome: Option<wasl_store::OpOutcome>) -> OpRow {
    (seq, format!("file://{PAIR}/{key}"), op.into(), reverts, outcome)
}

#[tokio::test]
async fn an_accepted_session_is_a_committed_changeset_with_one_operation_per_entry() {
    use wasl_store::{OpOutcome, OplogStatus};
    let (w, _a) = synced().await;
    let cs = w
        .engine
        .agent_begin(PAIR, "", "claude-code", Some("tidy notes"), Some("s1"))
        .await
        .unwrap();
    // Readers see the agent at work from the start.
    let (open, ops) = oplog(&w, &cs.id);
    assert_eq!(OplogStatus::Open, open.status);
    assert_eq!("agent:claude-code", open.actor);
    assert_eq!(Some("tidy notes".to_owned()), open.label);
    assert_eq!(format!("file://{PAIR}/"), open.scope);
    assert_eq!(cs.started_at, open.opened_at);
    assert!(ops.is_empty());

    assert!(w.engine.agent_save(PAIR, "a.txt", Some("s1")).await.unwrap());
    w.write("a.txt", "a2 by agent");
    w.write("c.txt", "new by agent");
    w.remove("b.txt");
    w.engine.agent_end(&cs.id).await.unwrap();

    let (held, ops) = oplog(&w, &cs.id);
    assert_eq!(OplogStatus::Staged, held.status);
    assert_eq!(None, held.decided_at);
    assert_eq!(
        vec![
            row(1, "a.txt", "update", None, None),
            row(2, "b.txt", "delete", None, None),
            row(3, "c.txt", "create", None, None),
        ],
        rows(&ops)
    );
    // Before and after: size, modification time, and BLAKE3 where Wasl knows it (the saved copy).
    let d = w.engine.agent_changeset(&cs.id).unwrap();
    let a = d.entries.iter().find(|e| e.rel == "a.txt").unwrap();
    assert_eq!(EarlierVersion::Saved, a.source);
    assert_eq!(
        Some(serde_json::json!({
            "kind": "file",
            "size": 2,
            "mtime_ms": a.before_mtime.unwrap(),
            "blake3": blake3::hash(b"a1").to_hex().to_string(),
        })),
        ops[0].prev
    );
    assert_eq!(
        Some(serde_json::json!({
            "kind": "file",
            "size": a.after_size.unwrap(),
            "mtime_ms": a.after_mtime.unwrap(),
        })),
        ops[0].next
    );
    let b = d.entries.iter().find(|e| e.rel == "b.txt").unwrap();
    assert_eq!(
        Some(serde_json::json!({"kind": "file", "size": 2, "mtime_ms": b.before_mtime.unwrap()})),
        ops[1].prev
    );
    assert_eq!(None, ops[1].next);
    assert_eq!((None, true), (ops[2].prev.clone(), ops[2].next.is_some()));

    w.engine.agent_accept_by(&cs.id, Some("desktop")).unwrap();
    let (done, ops) = oplog(&w, &cs.id);
    assert_eq!(OplogStatus::Committed, done.status);
    assert_eq!(Some("desktop".to_owned()), done.decided_by);
    assert_eq!(Some(format!("person:{}", common::DEVICE)), done.decided_actor);
    assert_eq!(
        w.engine.agent_changeset(&cs.id).unwrap().changeset.decided_at,
        done.decided_at
    );
    assert_eq!(None, done.imported_at);
    assert_eq!(3, ops.len());
    assert!(
        ops.iter()
            .all(|o| o.outcome == Some(OpOutcome::Applied) && o.reverts.is_none()),
        "{ops:?}"
    );
}

#[tokio::test]
async fn a_revert_appends_one_inverse_operation_per_entry() {
    use wasl_store::{OpOutcome, OplogStatus};
    let (w, _a) = synced().await;
    let cs = w
        .engine
        .agent_begin(PAIR, "", "gemini", None, None)
        .await
        .unwrap();
    w.write("a.txt", "a2 by agent");
    w.write("c.txt", "new by agent");
    w.remove("b.txt");
    w.engine.agent_end(&cs.id).await.unwrap();
    let (_, written) = oplog(&w, &cs.id);

    let report = w.engine.agent_revert(&cs.id).await.unwrap();
    assert_eq!(3, report.reverted.len(), "{report:?}");
    let (log, ops) = oplog(&w, &cs.id);
    assert_eq!(OplogStatus::Reverted, log.status);
    assert_eq!(Some("unknown".to_owned()), log.decided_by);
    assert_eq!(Some(format!("person:{}", common::DEVICE)), log.decided_actor);
    assert_eq!(6, ops.len());
    // The operations written at the end stand as they were, now reverted.
    for (o, before) in ops[..3].iter().zip(&written) {
        assert_eq!(
            (&before.entity_uri, &before.op, &before.prev, &before.next, None),
            (&o.entity_uri, &o.op, &o.prev, &o.next, o.reverts)
        );
        assert_eq!(Some(OpOutcome::Reverted), o.outcome);
    }
    // Each inverse takes one back: before and after swapped, create and delete swapped.
    let mut taken_back = Vec::new();
    for inv in &ops[3..] {
        let orig = &ops[inv.reverts.unwrap() as usize - 1];
        taken_back.push(orig.seq);
        assert_eq!(orig.entity_uri, inv.entity_uri);
        assert_eq!((&orig.prev, &orig.next), (&inv.next, &inv.prev));
        let inverse = match orig.op.as_str() {
            "create" => "delete",
            "delete" => "create",
            other => other,
        };
        assert_eq!(inverse, inv.op);
        assert_eq!(Some(OpOutcome::Applied), inv.outcome);
    }
    taken_back.sort();
    assert_eq!(vec![1, 2, 3], taken_back);
}

#[tokio::test]
async fn a_partial_revert_leaves_a_file_changed_since_without_an_inverse() {
    use wasl_store::{OpOutcome, OplogStatus};
    let (w, _a) = synced().await;
    let cs = w
        .engine
        .agent_begin(PAIR, "", "claude-code", None, None)
        .await
        .unwrap();
    w.write("a.txt", "agent");
    w.write("b.txt", "agent");
    w.engine.agent_end(&cs.id).await.unwrap();
    // You change b.txt after the session: you win.
    w.write("b.txt", "mine, after the session");

    let report = w.engine.agent_revert(&cs.id).await.unwrap();
    assert_eq!(vec!["b.txt".to_owned()], report.skipped);
    assert_eq!(
        ChangesetStatus::PartlyReverted,
        w.engine.agent_changeset(&cs.id).unwrap().changeset.status
    );
    let (log, ops) = oplog(&w, &cs.id);
    assert_eq!(OplogStatus::PartlyReverted, log.status);
    assert_eq!(
        vec![
            row(1, "a.txt", "update", None, Some(OpOutcome::Reverted)),
            row(2, "b.txt", "update", None, Some(OpOutcome::SkippedChanged)),
            row(3, "a.txt", "update", Some(1), Some(OpOutcome::Applied)),
        ],
        rows(&ops)
    );
}

#[tokio::test]
async fn a_revert_cut_short_between_entries_leaves_both_records_at_the_same_entry_and_resumes() {
    use wasl_store::{OpOutcome, OplogStatus};
    let (w, _a) = synced().await;
    let cs = w
        .engine
        .agent_begin(PAIR, "", "claude-code", None, None)
        .await
        .unwrap();
    w.write("a.txt", "agent");
    w.write("b.txt", "agent");
    w.engine.agent_end(&cs.id).await.unwrap();
    // As if the process stopped after putting b.txt back (a revert goes deepest path first).
    std::fs::write(w.path("b.txt"), "b1").unwrap();
    w.engine
        .store()
        .lock()
        .set_entry_outcome(&cs.id, "b.txt", "reverted")
        .unwrap();

    let d = w.engine.agent_changeset(&cs.id).unwrap();
    assert_eq!(ChangesetStatus::Held, d.changeset.status);
    let outcomes: Vec<(&str, Option<&str>)> = d
        .entries
        .iter()
        .map(|e| (e.rel.as_str(), e.outcome.as_deref()))
        .collect();
    assert_eq!(vec![("a.txt", None), ("b.txt", Some("reverted"))], outcomes);
    let (log, ops) = oplog(&w, &cs.id);
    assert_eq!(OplogStatus::Staged, log.status);
    assert_eq!(
        vec![
            row(1, "a.txt", "update", None, None),
            row(2, "b.txt", "update", None, Some(OpOutcome::Reverted)),
            row(3, "b.txt", "update", Some(2), Some(OpOutcome::Applied)),
        ],
        rows(&ops)
    );

    // The revert carries on where it stopped: b.txt is not taken back twice.
    let report = w.engine.agent_revert(&cs.id).await.unwrap();
    assert!(
        report.skipped.is_empty() && report.failed.is_empty(),
        "{report:?}"
    );
    assert_eq!(Some("a1".into()), w.read("a.txt"));
    let (log, ops) = oplog(&w, &cs.id);
    assert_eq!(OplogStatus::Reverted, log.status);
    assert_eq!(
        vec![
            row(1, "a.txt", "update", None, Some(OpOutcome::Reverted)),
            row(2, "b.txt", "update", None, Some(OpOutcome::Reverted)),
            row(3, "b.txt", "update", Some(2), Some(OpOutcome::Applied)),
            row(4, "a.txt", "update", Some(1), Some(OpOutcome::Applied)),
        ],
        rows(&ops)
    );
}

#[tokio::test]
async fn a_session_that_changed_nothing_leaves_no_op_log_record() {
    let (w, _a) = synced().await;
    let cs = w
        .engine
        .agent_begin(PAIR, "notes", "claude-code", None, None)
        .await
        .unwrap();
    let (open, _) = oplog(&w, &cs.id);
    assert_eq!(format!("file://{PAIR}/notes/"), open.scope);
    assert_eq!(None, w.engine.agent_end(&cs.id).await.unwrap());
    let k = wasl_store::OplogKey::new(common::DEVICE, &cs.id);
    assert_eq!(None, w.engine.store().lock().oplog_changeset(&k).unwrap());
}
