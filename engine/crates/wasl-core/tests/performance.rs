//! SRS NFR-3: planning 100,000 keys takes under 2 s on a desktop. Timed only in release builds
//! (`cargo test --release -p wasl-core --test performance`); debug builds just run it once.

use std::time::Instant;
use wasl_core::guards::GuardConfig;
use wasl_core::{
    BaselineEntry, ConflictPolicy, DocsFormat, LocalEntry, PlanInput, Planner, PlannerConfig, RemoteEntry,
    SyncMode,
};

#[test]
fn plans_100k_keys_within_budget() {
    let mut input = PlanInput::default();
    for i in 0..100_000u32 {
        let key = format!("d{}/sub{}/file{i}.bin", i % 100, i % 1000);
        let changed = i % 50 == 0;
        input.baseline.insert(
            key.clone(),
            BaselineEntry {
                key: key.clone(),
                local_rel: key.clone(),
                is_dir: false,
                local_size: 1000,
                local_mtime: 1,
                local_hash: Some("h".into()),
                remote_id: Some(format!("id{i}")),
                remote_md5: Some("h".into()),
                remote_size: Some(1000),
                remote_mtime: Some(1),
                remote_version: Some(1),
                export_mime: None,
            },
        );
        input.local.insert(
            key.clone(),
            LocalEntry {
                rel: key.clone(),
                is_dir: false,
                size: 1000,
                mtime: if changed { 2 } else { 1 },
            },
        );
        input.remote.insert(
            key.clone(),
            RemoteEntry {
                key: key.clone(),
                id: format!("id{i}"),
                name: format!("file{i}.bin"),
                is_dir: false,
                size: Some(1000),
                md5: Some("h".into()),
                mtime: 1,
                version: Some(1),
                google_mime: None,
                parent_id: None,
            },
        );
    }
    let planner = Planner::new(PlannerConfig {
        mode: SyncMode::TwoWay,
        conflict_policy: ConflictPolicy::KeepBoth,
        propagate_deletions: true,
        max_file_size: 0,
        docs_export: true,
        docs_format: DocsFormat::Office,
        guards: GuardConfig::default(),
    });
    let start = Instant::now();
    let plan = planner.plan_with(&input, |_| "h2".into());
    let elapsed = start.elapsed();
    assert_eq!(2_000, plan.summary.uploads);
    eprintln!("planned 100,000 keys in {elapsed:?}");
    if !cfg!(debug_assertions) {
        assert!(elapsed.as_secs_f64() < 2.0, "{elapsed:?}");
    }
}
