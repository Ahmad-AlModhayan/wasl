//! Property tests for the planner (SRS FR-E2, FR-F7 and §8 "Planner").
//!
//! A small in-memory model plays the executor: it applies a plan to a local tree, a Drive tree and
//! the baseline, so convergence can be checked by planning again.

use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;
use std::collections::BTreeMap;
use wasl_core::guards::GuardConfig;
use wasl_core::names::conflict_name;
use wasl_core::{
    Action, BaselineEntry, ConflictPolicy, DocsFormat, LocalEntry, PlanInput, Planner, PlannerConfig,
    RemoteEntry, SyncMode, SyncPlan,
};

const SIZE: u64 = 100; // every version has the same size, so only mtime or the hash tells them apart
const T_BASE: i64 = 1_000;
const T_EDIT: i64 = 2_000;

fn hash(content: u8) -> String {
    format!("c{content}")
}

/// One file of the pair as the test generates it.
#[derive(Debug, Clone)]
struct Case {
    /// Content at the last sync, if the file was synced.
    base: Option<u8>,
    /// Current local content and whether its mtime was bumped without a content change.
    local: Option<(u8, bool)>,
    /// Current Drive content and whether Drive replaced the file (new id).
    remote: Option<(u8, bool)>,
}

fn case() -> impl Strategy<Value = Case> {
    (
        proptest::option::of(0u8..3),
        proptest::option::of((0u8..3, any::<bool>())),
        proptest::option::of((0u8..3, any::<bool>())),
    )
        .prop_map(|(base, local, remote)| Case { base, local, remote })
}

fn cases() -> impl Strategy<Value = Vec<Case>> {
    proptest::collection::vec(case(), 1..7)
}

fn mode() -> impl Strategy<Value = SyncMode> {
    proptest::sample::select(SyncMode::ALL.to_vec())
}

/// (method, propagate deletions) combinations that must never delete anything.
fn non_deleting() -> impl Strategy<Value = (SyncMode, bool)> {
    prop_oneof![
        (
            proptest::sample::select(vec![
                SyncMode::UploadOnly,
                SyncMode::UploadThenDelete,
                SyncMode::DownloadOnly,
                SyncMode::DownloadThenDelete,
            ]),
            any::<bool>()
        ),
        Just((SyncMode::TwoWay, false)),
    ]
}

fn policy() -> impl Strategy<Value = ConflictPolicy> {
    proptest::sample::select(ConflictPolicy::ALL.to_vec())
}

fn config(mode: SyncMode, conflict_policy: ConflictPolicy, propagate_deletions: bool) -> PlannerConfig {
    PlannerConfig {
        mode,
        conflict_policy,
        propagate_deletions,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalFile {
    content: String,
    mtime: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DriveFile {
    id: String,
    content: String,
    version: i64,
    mtime: i64,
}

/// The world the planner looks at, plus a clock and an id counter for the model executor.
#[derive(Debug, Clone)]
struct World {
    local: BTreeMap<String, LocalFile>,
    remote: BTreeMap<String, DriveFile>,
    baseline: BTreeMap<String, BaselineEntry>,
    clock: i64,
    next_id: u32,
}

impl World {
    fn from_cases(cases: &[Case]) -> World {
        let mut w = World {
            local: BTreeMap::new(),
            remote: BTreeMap::new(),
            baseline: BTreeMap::new(),
            clock: 10_000,
            next_id: 0,
        };
        for (i, c) in cases.iter().enumerate() {
            let key = format!("k{i}.txt");
            let original_id = format!("id-{key}");
            if let Some(b) = c.base {
                w.baseline.insert(
                    key.clone(),
                    BaselineEntry {
                        key: key.clone(),
                        local_rel: key.clone(),
                        is_dir: false,
                        local_size: SIZE,
                        local_mtime: T_BASE,
                        local_hash: Some(hash(b)),
                        remote_id: Some(original_id.clone()),
                        remote_md5: Some(hash(b)),
                        remote_size: Some(SIZE),
                        remote_mtime: Some(T_BASE),
                        remote_version: Some(1),
                        export_mime: None,
                    },
                );
            }
            if let Some((content, touched)) = c.local {
                // A changed file always has a new mtime; an unchanged one only when touched.
                let same = c.base == Some(content);
                let mtime = if same && !touched { T_BASE } else { T_EDIT };
                w.local.insert(
                    key.clone(),
                    LocalFile {
                        content: hash(content),
                        mtime,
                    },
                );
            }
            if let Some((content, replaced)) = c.remote {
                let same = c.base == Some(content);
                let id = if replaced || c.base.is_none() {
                    format!("new-{key}")
                } else {
                    original_id
                };
                let version = if same { 1 } else { 2 };
                let mtime = if same { T_BASE } else { T_EDIT };
                w.remote.insert(
                    key,
                    DriveFile {
                        id,
                        content: hash(content),
                        version,
                        mtime,
                    },
                );
            }
        }
        w
    }

    fn input(&self) -> PlanInput {
        let mut input = PlanInput {
            baseline: self.baseline.clone(),
            ..PlanInput::default()
        };
        for (k, b) in &self.baseline {
            input.local_rel_by_key.insert(k.clone(), b.local_rel.clone());
        }
        for (k, f) in &self.local {
            input.local_rel_by_key.insert(k.clone(), k.clone());
            input.local.insert(
                k.clone(),
                LocalEntry {
                    rel: k.clone(),
                    is_dir: false,
                    size: SIZE,
                    mtime: f.mtime,
                },
            );
        }
        for (k, f) in &self.remote {
            input.remote.insert(
                k.clone(),
                RemoteEntry {
                    key: k.clone(),
                    id: f.id.clone(),
                    name: k.clone(),
                    is_dir: false,
                    size: Some(SIZE),
                    md5: Some(f.content.clone()),
                    mtime: f.mtime,
                    version: Some(f.version),
                    google_mime: None,
                    parent_id: None,
                },
            );
        }
        input
    }

    fn plan(&self, cfg: &PlannerConfig) -> SyncPlan {
        Planner::new(cfg.clone()).plan_with(&self.input(), |l| self.local[&l.rel].content.clone())
    }

    fn tick(&mut self) -> i64 {
        self.clock += 1;
        self.clock
    }

    fn record(&mut self, key: &str) {
        let l = &self.local[key];
        let r = &self.remote[key];
        self.baseline.insert(
            key.to_owned(),
            BaselineEntry {
                key: key.to_owned(),
                local_rel: key.to_owned(),
                is_dir: false,
                local_size: SIZE,
                local_mtime: l.mtime,
                local_hash: Some(l.content.clone()),
                remote_id: Some(r.id.clone()),
                remote_md5: Some(r.content.clone()),
                remote_size: Some(SIZE),
                remote_mtime: Some(r.mtime),
                remote_version: Some(r.version),
                export_mime: None,
            },
        );
    }

    fn upload(&mut self, key: &str, existing_id: Option<&str>) {
        let t = self.tick();
        let content = self.local[key].content.clone();
        let id = match existing_id {
            Some(id) => id.to_owned(),
            None => {
                self.next_id += 1;
                format!("up-{}", self.next_id)
            }
        };
        let version = self.remote.get(key).map_or(1, |r| r.version + 1);
        self.remote.insert(
            key.to_owned(),
            DriveFile {
                id,
                content,
                version,
                mtime: t,
            },
        );
        self.record(key);
    }

    fn download(&mut self, key: &str) {
        let t = self.tick();
        let content = self.remote[key].content.clone();
        self.local.insert(key.to_owned(), LocalFile { content, mtime: t });
        self.record(key);
    }

    /// Applies a plan the way the 1.1.4 executor does.
    fn apply(&mut self, plan: &SyncPlan, mode: SyncMode) {
        for a in &plan.actions {
            match a {
                Action::Upload {
                    key,
                    existing_remote_id,
                    ..
                } => {
                    self.upload(key, existing_remote_id.as_deref());
                    if mode == SyncMode::UploadThenDelete {
                        self.local.remove(key);
                    }
                }
                Action::Download { key, .. } => {
                    self.download(key);
                    if mode == SyncMode::DownloadThenDelete {
                        self.remote.remove(key);
                    }
                }
                Action::DeleteLocal { key, .. } => {
                    self.local.remove(key);
                    self.baseline.remove(key);
                }
                Action::DeleteRemote { key, .. } => {
                    self.remote.remove(key);
                    self.baseline.remove(key);
                }
                Action::Conflict { key, .. } => {
                    // Keep both: the local copy is renamed, then the Drive version is downloaded.
                    let copy = self.local.remove(key).expect("conflict has a local copy");
                    self.local.insert(conflict_name(key, "Test", "s"), copy);
                    self.download(key);
                }
                Action::Adopt { key, .. } => self.record(key),
                Action::Forget { key } => {
                    self.baseline.remove(key);
                }
                Action::RefreshState { key, .. } => {
                    if self.local.contains_key(key) && self.remote.contains_key(key) {
                        self.record(key);
                    }
                }
                Action::Skip { .. } => {}
                other => panic!("unexpected {} for files", other.kind()),
            }
        }
    }

    fn contents(files: impl Iterator<Item = (String, String)>) -> BTreeMap<String, String> {
        files.collect()
    }

    fn local_contents(&self) -> BTreeMap<String, String> {
        Self::contents(self.local.iter().map(|(k, f)| (k.clone(), f.content.clone())))
    }

    fn remote_contents(&self) -> BTreeMap<String, String> {
        Self::contents(self.remote.iter().map(|(k, f)| (k.clone(), f.content.clone())))
    }
}

/// True when the plan transfers, deletes, creates or conflicts nothing.
fn is_quiet(plan: &SyncPlan) -> bool {
    plan.actions.iter().all(|a| {
        matches!(
            a,
            Action::Adopt { .. } | Action::Forget { .. } | Action::RefreshState { .. } | Action::Skip { .. }
        )
    })
}

fn deletes(plan: &SyncPlan) -> impl Iterator<Item = &Action> {
    plan.actions.iter().filter(|a| a.is_delete())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 2_000,
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct("tests/properties.proptest-regressions"))),
        ..ProptestConfig::default()
    })]

    /// FR-E2: same input, same plan.
    #[test]
    fn planning_is_deterministic(cases in cases(), mode in mode(), policy in policy(), propagate in any::<bool>()) {
        let w = World::from_cases(&cases);
        let cfg = config(mode, policy, propagate);
        prop_assert_eq!(w.plan(&cfg), w.plan(&cfg));
    }

    /// FR-F7: outside the mirror methods, nothing is deleted that the baseline does not know.
    /// Mirror methods delete extras on the target side by definition (they go to the bin).
    #[test]
    fn no_deletion_without_a_baseline_entry(cases in cases(), mode in mode(), policy in policy(), propagate in any::<bool>()) {
        prop_assume!(!matches!(mode, SyncMode::UploadMirror | SyncMode::DownloadMirror));
        let w = World::from_cases(&cases);
        let plan = w.plan(&config(mode, policy, propagate));
        for a in deletes(&plan) {
            prop_assert!(w.baseline.contains_key(a.key()), "{} deleted without a baseline entry", a.key());
        }
    }

    /// One-way methods never delete; two-way with deletions off never deletes.
    #[test]
    fn methods_that_must_not_delete_never_do(cases in cases(), (mode, propagate) in non_deleting(), policy in policy()) {
        let plan = World::from_cases(&cases).plan(&config(mode, policy, propagate));
        prop_assert_eq!(0, deletes(&plan).count());
    }

    /// Upload methods never write to the device; download methods never write to Drive.
    #[test]
    fn one_way_methods_only_write_to_their_target(cases in cases(), mode in mode(), policy in policy(), propagate in any::<bool>()) {
        prop_assume!(mode != SyncMode::TwoWay);
        let plan = World::from_cases(&cases).plan(&config(mode, policy, propagate));
        for a in &plan.actions {
            let writes_local = matches!(a, Action::Download { .. } | Action::DeleteLocal { .. } | Action::MkdirLocal { .. } | Action::Conflict { .. });
            let writes_remote = matches!(a, Action::Upload { .. } | Action::DeleteRemote { .. } | Action::MkdirRemote { .. });
            if mode.uploads() { prop_assert!(!writes_local, "{:?} in {:?}", a, mode); }
            if mode.downloads() { prop_assert!(!writes_remote, "{:?} in {:?}", a, mode); }
        }
    }

    /// §8: mirror methods converge in one run: the target equals the source and a second plan is quiet.
    #[test]
    fn mirror_methods_converge_in_one_run(cases in cases(), upload in any::<bool>(), policy in policy(), propagate in any::<bool>()) {
        let mode = if upload { SyncMode::UploadMirror } else { SyncMode::DownloadMirror };
        let cfg = config(mode, policy, propagate);
        let mut w = World::from_cases(&cases);
        let first = w.plan(&cfg);
        w.apply(&first, mode);
        if upload {
            prop_assert_eq!(w.local_contents(), w.remote_contents());
        } else {
            prop_assert_eq!(w.remote_contents(), w.local_contents());
        }
        let second = w.plan(&cfg);
        prop_assert!(is_quiet(&second), "second run: {:?}", second.actions);
    }

    /// Two-way converges: both sides hold the same files and a later plan is quiet. Keep-both needs a
    /// second run to upload the conflict copies; "skip and report" keeps reporting by design.
    #[test]
    fn two_way_converges(cases in cases(), policy in policy(), propagate in any::<bool>()) {
        let cfg = config(SyncMode::TwoWay, policy, propagate);
        let mut w = World::from_cases(&cases);
        for _ in 0..3 {
            let plan = w.plan(&cfg);
            if is_quiet(&plan) { break; }
            w.apply(&plan, SyncMode::TwoWay);
        }
        let last = w.plan(&cfg);
        prop_assert!(is_quiet(&last), "did not converge: {:?}", last.actions);
        if policy != ConflictPolicy::Skip {
            prop_assert_eq!(w.local_contents(), w.remote_contents());
        }
    }

    /// Re-running a converged one-way method changes nothing.
    #[test]
    fn one_way_methods_are_idempotent(cases in cases(), mode in mode(), policy in policy(), propagate in any::<bool>()) {
        prop_assume!(mode != SyncMode::TwoWay);
        let cfg = config(mode, policy, propagate);
        let mut w = World::from_cases(&cases);
        for _ in 0..3 {
            let plan = w.plan(&cfg);
            if is_quiet(&plan) { break; }
            w.apply(&plan, mode);
        }
        let last = w.plan(&cfg);
        prop_assert!(is_quiet(&last), "not idempotent: {:?}", last.actions);
    }
}
