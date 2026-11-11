//! Ignored items are never uploaded, downloaded or deleted on any side, and adding a rule never
//! deletes copies that already exist elsewhere (SRS FR-G4, FR-G5).

use wasl_core::guards::GuardConfig;
use wasl_core::{
    BaselineEntry, ConflictPolicy, DocsFormat, LocalEntry, PlanInput, Planner, PlannerConfig, RemoteEntry,
    SyncMode,
};
use wasl_ignore::{FileKind, IgnoreRules, Preset};

fn cfg(mode: SyncMode) -> PlannerConfig {
    PlannerConfig {
        mode,
        conflict_policy: ConflictPolicy::KeepBoth,
        propagate_deletions: true,
        max_file_size: 0,
        docs_export: true,
        docs_format: DocsFormat::Office,
        guards: GuardConfig {
            mass_delete_guard: false,
            first_merge_guard: false,
            ..GuardConfig::default()
        },
    }
}

fn base(key: &str, is_dir: bool) -> BaselineEntry {
    BaselineEntry {
        key: key.into(),
        local_rel: key.into(),
        is_dir,
        local_size: 1,
        local_mtime: 1,
        local_hash: (!is_dir).then(|| "h".into()),
        remote_id: Some(format!("id-{key}")),
        remote_md5: (!is_dir).then(|| "h".into()),
        remote_size: Some(1),
        remote_mtime: Some(1),
        remote_version: Some(1),
        export_mime: None,
    }
}

fn local(key: &str, is_dir: bool, mtime: i64) -> LocalEntry {
    LocalEntry {
        rel: key.into(),
        is_dir,
        size: 1,
        mtime,
    }
}

fn remote(key: &str, is_dir: bool, md5: &str) -> RemoteEntry {
    RemoteEntry {
        key: key.into(),
        id: format!("id-{key}"),
        name: key.rsplit('/').next().unwrap().into(),
        is_dir,
        size: (!is_dir).then_some(1),
        md5: (!is_dir).then(|| md5.into()),
        mtime: 1,
        version: Some(1),
        google_mime: None,
        parent_id: None,
    }
}

fn input(
    baseline: Vec<BaselineEntry>,
    locals: Vec<(&str, LocalEntry)>,
    remotes: Vec<RemoteEntry>,
) -> PlanInput {
    let mut i = PlanInput::default();
    for b in baseline {
        i.local_rel_by_key.insert(b.key.clone(), b.local_rel.clone());
        i.baseline.insert(b.key.clone(), b);
    }
    for (k, l) in locals {
        i.local_rel_by_key.insert(k.into(), l.rel.clone());
        i.local.insert(k.into(), l);
    }
    for r in remotes {
        i.remote.insert(r.key.clone(), r);
    }
    i
}

#[test]
fn ignored_items_produce_no_action_in_any_method() {
    let rules = IgnoreRules::builder()
        .presets([Preset::Node, Preset::OsJunk])
        .file("", FileKind::WaslIgnore, "*.log\n")
        .build()
        .unwrap();
    // A mix that would otherwise upload, download, delete and conflict on ignored paths.
    let world = input(
        vec![
            base("node_modules", true),
            base("node_modules/a.js", false),
            base("old.log", false),
            base("src", true),
            base("src/main.rs", false),
        ],
        vec![
            ("node_modules", local("node_modules", true, 1)),
            ("node_modules/b.js", local("node_modules/b.js", false, 5)),
            (".DS_Store", local(".DS_Store", false, 5)),
            ("src", local("src", true, 1)),
            ("src/main.rs", local("src/main.rs", false, 1)),
            ("new.log", local("new.log", false, 5)),
        ],
        vec![
            remote("node_modules", true, ""),
            remote("node_modules/a.js", false, "changed"),
            remote("node_modules/c.js", false, "x"),
            remote("old.log", false, "changed"),
            remote("src", true, ""),
            remote("src/main.rs", false, "h"),
            remote("dl.log", false, "x"),
        ],
    );
    let filtered = world.clone().without_ignored(|k, d| rules.is_ignored(k, d));
    for mode in SyncMode::ALL {
        let plan = Planner::new(cfg(mode)).plan_with(&filtered, |_| "h".into());
        for a in &plan.actions {
            assert!(
                !rules.is_ignored(a.key(), false) && !rules.is_ignored(a.key(), true),
                "{mode:?}: {a:?}"
            );
        }
    }
    // Without the filter the same world does touch ignored paths, so the test is not vacuous.
    let unfiltered = Planner::new(cfg(SyncMode::TwoWay)).plan_with(&world, |_| "other".into());
    assert!(
        unfiltered
            .actions
            .iter()
            .any(|a| rules.is_ignored(a.key(), false) || rules.is_ignored(a.key(), true))
    );
}

#[test]
fn adding_a_rule_never_deletes_copies_elsewhere() {
    // build/ was synced; the user adds a rule for it and deletes the local copy.
    let world = input(
        vec![base("build", true), base("build/app.bin", false)],
        vec![],
        vec![remote("build", true, ""), remote("build/app.bin", false, "h")],
    );
    let before = Planner::new(cfg(SyncMode::TwoWay)).plan_with(&world, |_| "h".into());
    assert!(
        before.summary.delete_remote > 0,
        "without the rule the deletion would propagate"
    );

    let rules = IgnoreRules::builder()
        .file("", FileKind::WaslIgnore, "build/\n")
        .build()
        .unwrap();
    for mode in SyncMode::ALL {
        let filtered = world.clone().without_ignored(|k, d| rules.is_ignored(k, d));
        let plan = Planner::new(cfg(mode)).plan_with(&filtered, |_| "h".into());
        assert!(plan.actions.is_empty(), "{mode:?}: {:?}", plan.actions);
    }
}
