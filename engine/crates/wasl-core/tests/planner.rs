//! Port of `sync/PlannerTest.kt` (Android 1.1.4). One Rust test per Kotlin test, same order.
//!
//! The planner never touches the file system: a local entry carries the hash the test hasher
//! reports for it, and the hasher records which files were hashed (laziness is asserted).

use std::collections::BTreeMap;
use wasl_core::docs::{
    GOOGLE_DOC, GOOGLE_SHEET, GOOGLE_SLIDES, export_distinct_name, export_target, is_export_distinct_name,
    is_export_name,
};
use wasl_core::guards::GuardConfig;
use wasl_core::paths::{name_of, parent_of};
use wasl_core::planner::{export_local_rel, mapped_local_rel, over_size_limit};
use wasl_core::{
    Action, BaselineEntry, BlockKind, BlockSide, ConflictPolicy, DocsFormat, LocalEntry, PlanInput,
    PlanSummary, Planner, PlannerConfig, RemoteEntry, SyncMode, SyncPlan,
};

// ------------------------------------------------------------------ fixtures

const T0: i64 = 1_700_000_000_000;
const T1: i64 = T0 + 60_000;
const DOCX: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
const XLSX: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
const PPTX: &str = "application/vnd.openxmlformats-officedocument.presentationml.presentation";
const PDF: &str = "application/pdf";

/// 1.1.4 test defaults: guards off, 10 files / 30% when switched on.
fn cfg() -> PlannerConfig {
    PlannerConfig {
        mode: SyncMode::TwoWay,
        conflict_policy: ConflictPolicy::KeepBoth,
        propagate_deletions: true,
        max_file_size: 0,
        docs_export: true,
        docs_format: DocsFormat::Office,
        guards: GuardConfig {
            mass_delete_guard: false,
            mass_delete_min_files: 10,
            mass_delete_threshold_percent: 30,
            first_merge_guard: false,
            approved_once: false,
        },
    }
}

fn mode(mode: SyncMode) -> PlannerConfig {
    PlannerConfig { mode, ..cfg() }
}

fn policy(conflict_policy: ConflictPolicy) -> PlannerConfig {
    PlannerConfig {
        conflict_policy,
        ..cfg()
    }
}

fn no_deletions() -> PlannerConfig {
    PlannerConfig {
        propagate_deletions: false,
        ..cfg()
    }
}

fn guarded(min_files: u32, threshold_percent: u32) -> PlannerConfig {
    let mut c = cfg();
    c.guards.mass_delete_guard = true;
    c.guards.mass_delete_min_files = min_files;
    c.guards.mass_delete_threshold_percent = threshold_percent;
    c
}

fn first_merge(mut c: PlannerConfig) -> PlannerConfig {
    c.guards.first_merge_guard = true;
    c
}

fn approved(mut c: PlannerConfig) -> PlannerConfig {
    c.guards.approved_once = true;
    c
}

/// Baseline row: by default a file whose local hash equals the remote md5 (fully in sync).
fn state(key: &str) -> BaselineEntry {
    BaselineEntry {
        key: key.into(),
        local_rel: key.into(),
        is_dir: false,
        local_size: 100,
        local_mtime: T0,
        local_hash: Some("h1".into()),
        remote_id: Some(format!("id-{key}")),
        remote_md5: Some("h1".into()),
        remote_size: Some(100),
        remote_mtime: Some(T0),
        remote_version: Some(1),
        export_mime: None,
    }
}

fn dir_state(key: &str) -> BaselineEntry {
    BaselineEntry {
        is_dir: true,
        local_size: 0,
        local_hash: None,
        remote_md5: None,
        remote_size: Some(0),
        remote_version: None,
        ..state(key)
    }
}

trait BaselineExt {
    fn remote_id(self, id: &str) -> Self;
    fn local_rel(self, rel: &str) -> Self;
    fn size(self, size: u64) -> Self;
    fn remote_md5(self, md5: Option<&str>) -> Self;
    fn remote_version(self, version: Option<i64>) -> Self;
}

impl BaselineExt for BaselineEntry {
    fn remote_id(self, id: &str) -> Self {
        BaselineEntry {
            remote_id: Some(id.into()),
            ..self
        }
    }
    fn local_rel(self, rel: &str) -> Self {
        BaselineEntry {
            local_rel: rel.into(),
            ..self
        }
    }
    fn size(self, size: u64) -> Self {
        BaselineEntry {
            local_size: size,
            remote_size: Some(size),
            ..self
        }
    }
    fn remote_md5(self, md5: Option<&str>) -> Self {
        BaselineEntry {
            remote_md5: md5.map(Into::into),
            ..self
        }
    }
    fn remote_version(self, version: Option<i64>) -> Self {
        BaselineEntry {
            remote_version: version,
            ..self
        }
    }
}

/// A local entry with the hash the injected hasher reports for it.
#[derive(Clone)]
struct L {
    key: String,
    entry: LocalEntry,
    hash: String,
}

fn local(key: &str) -> L {
    L {
        key: key.into(),
        entry: LocalEntry {
            rel: key.into(),
            is_dir: false,
            size: 100,
            mtime: T0,
        },
        hash: "h1".into(),
    }
}

impl L {
    fn size(mut self, size: u64) -> Self {
        self.entry.size = size;
        self
    }
    fn mtime(mut self, mtime: i64) -> Self {
        self.entry.mtime = mtime;
        self
    }
    fn hash(mut self, hash: &str) -> Self {
        self.hash = hash.into();
        self
    }
    fn rel(mut self, rel: &str) -> Self {
        self.entry.rel = rel.into();
        self
    }
    fn dir(mut self) -> Self {
        self.entry.is_dir = true;
        self.entry.size = 0;
        self
    }
}

fn remote(key: &str) -> RemoteEntry {
    RemoteEntry {
        key: key.into(),
        id: format!("id-{key}"),
        name: name_of(key).into(),
        is_dir: false,
        size: Some(100),
        md5: Some("h1".into()),
        mtime: T0,
        version: Some(1),
        google_mime: None,
        parent_id: Some(format!("parent-of-{key}")),
    }
}

trait RemoteExt {
    fn id(self, id: &str) -> Self;
    fn md5(self, md5: Option<&str>) -> Self;
    fn mtime(self, mtime: i64) -> Self;
    fn version(self, version: Option<i64>) -> Self;
    fn size(self, size: Option<u64>) -> Self;
    fn dir(self) -> Self;
}

impl RemoteExt for RemoteEntry {
    fn id(self, id: &str) -> Self {
        RemoteEntry {
            id: id.into(),
            ..self
        }
    }
    fn md5(self, md5: Option<&str>) -> Self {
        RemoteEntry {
            md5: md5.map(Into::into),
            ..self
        }
    }
    fn mtime(self, mtime: i64) -> Self {
        RemoteEntry { mtime, ..self }
    }
    fn version(self, version: Option<i64>) -> Self {
        RemoteEntry { version, ..self }
    }
    fn size(self, size: Option<u64>) -> Self {
        RemoteEntry { size, ..self }
    }
    fn dir(self) -> Self {
        RemoteEntry {
            is_dir: true,
            size: None,
            md5: None,
            ..self
        }
    }
}

struct Run {
    plan: SyncPlan,
    hash_calls: Vec<String>,
}

impl Run {
    fn actions(&self) -> &[Action] {
        &self.plan.actions
    }
    fn summary(&self) -> &PlanSummary {
        &self.plan.summary
    }
    fn all(&self, kind: &str) -> Vec<&Action> {
        self.plan.actions.iter().filter(|a| a.kind() == kind).collect()
    }
    fn single(&self, kind: &str) -> &Action {
        let found = self.all(kind);
        assert_eq!(
            1,
            found.len(),
            "expected exactly one {kind} in {:?}",
            self.plan.actions
        );
        found[0]
    }
    fn assert_none(&self, kind: &str) {
        assert!(
            self.all(kind).is_empty(),
            "expected no {kind} in {:?}",
            self.plan.actions
        );
    }
    fn assert_only(&self, kinds: &[&str]) {
        assert_eq!(
            kinds.to_vec(),
            self.plan.actions.iter().map(Action::kind).collect::<Vec<_>>()
        );
    }
    fn kinds(&self) -> Vec<&'static str> {
        self.plan.actions.iter().map(Action::kind).collect()
    }
}

/// Builds the input the way the engine does (baseline mapping first, then local names) and plans.
fn plan(cfg: PlannerConfig, baseline: Vec<BaselineEntry>, local: Vec<L>, remote: Vec<RemoteEntry>) -> Run {
    let mut input = PlanInput::default();
    for b in &baseline {
        input.local_rel_by_key.insert(b.key.clone(), b.local_rel.clone());
    }
    for l in &local {
        input.local_rel_by_key.insert(l.key.clone(), l.entry.rel.clone());
    }
    let hash_by_rel: BTreeMap<String, String> = local
        .iter()
        .map(|l| (l.entry.rel.clone(), l.hash.clone()))
        .collect();
    input.baseline = baseline.into_iter().map(|b| (b.key.clone(), b)).collect();
    input.local = local.into_iter().map(|l| (l.key, l.entry)).collect();
    input.remote = remote.into_iter().map(|r| (r.key.clone(), r)).collect();
    let mut calls = Vec::new();
    let plan = Planner::new(cfg).plan_with(&input, |e| {
        calls.push(e.rel.clone());
        hash_by_rel
            .get(&e.rel)
            .unwrap_or_else(|| panic!("no hash registered for {}", e.rel))
            .clone()
    });
    Run {
        plan,
        hash_calls: calls,
    }
}

/// Field accessors so assertions read like the Kotlin ones. Each panics on a variant without it.
trait Fields {
    fn reason(&self) -> &str;
    fn local_rel(&self) -> &str;
    fn existing_remote_id(&self) -> Option<&str>;
    fn parent_key(&self) -> &str;
    fn name(&self) -> &str;
    fn overwrites(&self) -> bool;
    fn is_dir(&self) -> bool;
    fn remote_id(&self) -> &str;
    fn warn(&self) -> bool;
    fn remote(&self) -> &RemoteEntry;
    fn local(&self) -> &LocalEntry;
    fn mime(&self) -> &str;
}

impl Fields for Action {
    fn reason(&self) -> &str {
        match self {
            Action::Upload { reason, .. }
            | Action::Download { reason, .. }
            | Action::Export { reason, .. }
            | Action::DeleteLocal { reason, .. }
            | Action::DeleteRemote { reason, .. }
            | Action::Skip { reason, .. } => reason,
            other => panic!("{} has no reason", other.kind()),
        }
    }
    fn local_rel(&self) -> &str {
        match self {
            Action::MkdirLocal { local_rel, .. }
            | Action::Download { local_rel, .. }
            | Action::Export { local_rel, .. }
            | Action::DeleteLocal { local_rel, .. }
            | Action::Conflict { local_rel, .. }
            | Action::Adopt { local_rel, .. }
            | Action::RefreshState { local_rel, .. } => local_rel,
            other => panic!("{} has no local_rel", other.kind()),
        }
    }
    fn existing_remote_id(&self) -> Option<&str> {
        match self {
            Action::Upload {
                existing_remote_id, ..
            } => existing_remote_id.as_deref(),
            other => panic!("{} has no existing_remote_id", other.kind()),
        }
    }
    fn parent_key(&self) -> &str {
        match self {
            Action::MkdirRemote { parent_key, .. }
            | Action::Upload { parent_key, .. }
            | Action::Conflict { parent_key, .. } => parent_key,
            other => panic!("{} has no parent_key", other.kind()),
        }
    }
    fn name(&self) -> &str {
        match self {
            Action::MkdirRemote { name, .. } | Action::Upload { name, .. } => name,
            other => panic!("{} has no name", other.kind()),
        }
    }
    fn overwrites(&self) -> bool {
        match self {
            Action::Download { overwrites, .. } => *overwrites,
            other => panic!("{} has no overwrites", other.kind()),
        }
    }
    fn is_dir(&self) -> bool {
        match self {
            Action::DeleteLocal { is_dir, .. } | Action::DeleteRemote { is_dir, .. } => *is_dir,
            other => panic!("{} has no is_dir", other.kind()),
        }
    }
    fn remote_id(&self) -> &str {
        match self {
            Action::DeleteRemote { remote_id, .. } => remote_id,
            other => panic!("{} has no remote_id", other.kind()),
        }
    }
    fn warn(&self) -> bool {
        match self {
            Action::Skip { warn, .. } => *warn,
            other => panic!("{} has no warn", other.kind()),
        }
    }
    fn remote(&self) -> &RemoteEntry {
        match self {
            Action::Download { remote, .. }
            | Action::Export { remote, .. }
            | Action::Conflict { remote, .. }
            | Action::Adopt { remote, .. } => remote,
            other => panic!("{} has no remote", other.kind()),
        }
    }
    fn local(&self) -> &LocalEntry {
        match self {
            Action::Upload { local, .. } | Action::Conflict { local, .. } | Action::Adopt { local, .. } => {
                local
            }
            other => panic!("{} has no local", other.kind()),
        }
    }
    fn mime(&self) -> &str {
        match self {
            Action::Export { mime, .. } => mime,
            other => panic!("{} has no mime", other.kind()),
        }
    }
}

fn keys(actions: &[&Action]) -> Vec<String> {
    actions.iter().map(|a| a.key().to_owned()).collect()
}

fn by_key<'a>(actions: &[&'a Action]) -> BTreeMap<String, &'a Action> {
    actions.iter().map(|a| (a.key().to_owned(), *a)).collect()
}

fn synced(n: usize) -> Vec<String> {
    (1..=n).map(|i| format!("f{i:02}.txt")).collect()
}

fn states(keys: &[String]) -> Vec<BaselineEntry> {
    keys.iter().map(|k| state(k)).collect()
}

fn locals(keys: &[String]) -> Vec<L> {
    keys.iter().map(|k| local(k)).collect()
}

fn remotes(keys: &[String]) -> Vec<RemoteEntry> {
    keys.iter().map(|k| remote(k)).collect()
}

// ================================================================ TWO_WAY: basic file matrix

#[test]
fn two_way_new_local_file_is_uploaded() {
    let r = plan(cfg(), vec![], vec![local("a.txt").size(123)], vec![]);
    let up = r.single("Upload");
    assert_eq!("a.txt", up.key());
    assert_eq!(None, up.existing_remote_id());
    assert_eq!("", up.parent_key());
    assert_eq!("a.txt", up.name());
    assert_eq!("new on device", up.reason());
    assert_eq!("a.txt", up.local().rel);
    r.assert_only(&["Upload"]);
    assert_eq!(1, r.summary().uploads);
    assert_eq!(123, r.summary().upload_bytes);
    assert_eq!(1, r.summary().local_files);
    assert_eq!(0, r.summary().remote_files);
    assert!(
        r.hash_calls.is_empty(),
        "a new file without a remote counterpart must not be hashed"
    );
}

#[test]
fn two_way_new_nested_local_file_targets_parent_key_and_name() {
    let r = plan(cfg(), vec![], vec![local("docs/2026/a.txt")], vec![]);
    let up = r.single("Upload");
    assert_eq!("docs/2026", up.parent_key());
    assert_eq!("a.txt", up.name());
}

#[test]
fn two_way_new_remote_file_is_downloaded() {
    let rem = remote("a.txt").id("r1").size(Some(456));
    let r = plan(cfg(), vec![], vec![], vec![rem.clone()]);
    let d = r.single("Download");
    assert_eq!(&rem, d.remote());
    assert_eq!("a.txt", d.local_rel());
    assert!(!d.overwrites());
    assert_eq!("new on Drive", d.reason());
    r.assert_only(&["Download"]);
    assert_eq!(1, r.summary().downloads);
    assert_eq!(456, r.summary().download_bytes);
    assert_eq!(0, r.summary().local_files);
    assert_eq!(1, r.summary().remote_files);
}

#[test]
fn two_way_changed_local_unchanged_remote_uploads_over_existing_id() {
    let r = plan(
        cfg(),
        vec![state("a.txt").remote_id("r1")],
        vec![local("a.txt").size(120).mtime(T1).hash("h2")],
        vec![remote("a.txt").id("r1")],
    );
    let up = r.single("Upload");
    assert_eq!(Some("r1"), up.existing_remote_id());
    assert_eq!("changed on device", up.reason());
    r.assert_only(&["Upload"]);
    assert_eq!(vec!["a.txt"], r.hash_calls);
    assert_eq!(120, r.summary().upload_bytes);
}

#[test]
fn two_way_touched_local_with_same_hash_is_unchanged() {
    let r = plan(
        cfg(),
        vec![state("a.txt")],
        vec![local("a.txt").mtime(T1).hash("h1")], // mtime bumped, content identical
        vec![remote("a.txt")],
    );
    // The new time is recorded once, or the file is hashed again on every run.
    r.assert_only(&["RefreshState"]);
    assert!(r.summary().is_empty());
    assert_eq!(
        vec!["a.txt"],
        r.hash_calls,
        "size/mtime differ so the hash decides"
    );
}

#[test]
fn two_way_unchanged_both_sides_produces_nothing_and_never_hashes() {
    let r = plan(
        cfg(),
        vec![state("a.txt")],
        vec![local("a.txt")],
        vec![remote("a.txt")],
    );
    assert!(r.actions().is_empty());
    assert!(r.hash_calls.is_empty());
}

#[test]
fn two_way_unchanged_local_changed_remote_downloads_and_overwrites() {
    let r = plan(
        cfg(),
        vec![state("a.txt")],
        vec![local("a.txt")],
        vec![remote("a.txt").md5(Some("h2")).size(Some(200))],
    );
    let d = r.single("Download");
    assert!(d.overwrites());
    assert_eq!("changed on Drive", d.reason());
    assert_eq!(200, r.summary().download_bytes);
    r.assert_only(&["Download"]);
}

#[test]
fn two_way_remote_without_md5_is_compared_by_version() {
    let base = state("a.txt").remote_md5(None).remote_version(Some(1));
    let same = plan(
        cfg(),
        vec![base.clone()],
        vec![local("a.txt")],
        vec![remote("a.txt").md5(None).version(Some(1))],
    );
    assert!(same.actions().is_empty());
    let bumped = plan(
        cfg(),
        vec![base],
        vec![local("a.txt")],
        vec![remote("a.txt").md5(None).version(Some(2))],
    );
    assert_eq!("changed on Drive", bumped.single("Download").reason());
}

#[test]
fn two_way_remote_with_the_same_md5_is_unchanged_whatever_its_version() {
    // Drive raises a version by itself after many uploads; a vault reports its manifest's md5.
    let r = plan(
        cfg(),
        vec![state("a.txt")],
        vec![local("a.txt")],
        vec![remote("a.txt").version(Some(3))],
    );
    assert!(r.actions().is_empty());
}

#[test]
fn a_row_without_md5_keeps_its_version_identity_and_records_the_md5() {
    // A vault row from before vault listings carried the manifest's md5.
    let base = state("a.txt").remote_md5(None).remote_version(Some(1));
    let same = plan(
        cfg(),
        vec![base.clone()],
        vec![local("a.txt")],
        vec![remote("a.txt").md5(Some("c1")).version(Some(1))],
    );
    same.assert_only(&["RefreshState"]);
    let bumped = plan(
        cfg(),
        vec![base],
        vec![local("a.txt")],
        vec![remote("a.txt").md5(Some("c1")).version(Some(2))],
    );
    assert_eq!("changed on Drive", bumped.single("Download").reason());
}

#[test]
fn two_way_remote_replaced_by_file_with_same_md5_is_not_transferred() {
    let r = plan(
        cfg(),
        vec![state("a.txt").remote_id("r1")],
        vec![local("a.txt")],
        vec![remote("a.txt").id("r2").md5(Some("h1"))],
    );
    assert!(
        r.actions()
            .iter()
            .all(|a| !matches!(a.kind(), "Download" | "Upload" | "Conflict"))
    );
}

// ================================================================ TWO_WAY: conflicts

fn both_changed(conflict_policy: ConflictPolicy, local_mtime: i64, remote_mtime: i64) -> Run {
    plan(
        policy(conflict_policy),
        vec![state("docs/a.txt").remote_id("r1")],
        vec![local("docs/a.txt").size(120).mtime(local_mtime).hash("h2")],
        vec![
            remote("docs/a.txt")
                .id("r1")
                .md5(Some("h3"))
                .mtime(remote_mtime)
                .size(Some(300)),
        ],
    )
}

fn both_changed_default(conflict_policy: ConflictPolicy) -> Run {
    both_changed(conflict_policy, T1, T0 + 30_000)
}

#[test]
fn conflict_keep_both_produces_conflict_action() {
    let r = both_changed_default(ConflictPolicy::KeepBoth);
    let c = r.single("Conflict");
    assert_eq!("docs/a.txt", c.key());
    assert_eq!("docs/a.txt", c.local_rel());
    assert_eq!("docs", c.parent_key());
    assert_eq!("r1", c.remote().id);
    assert_eq!("docs/a.txt", c.local().rel);
    r.assert_only(&["Conflict"]);
    assert_eq!(1, r.summary().conflicts);
    assert_eq!(0, r.summary().uploads);
    assert_eq!(0, r.summary().downloads);
    assert_eq!(vec!["docs/a.txt"], r.hash_calls, "hashed once and memoised");
}

#[test]
fn conflict_local_wins_uploads() {
    let r = both_changed_default(ConflictPolicy::LocalWins);
    let up = r.single("Upload");
    assert_eq!("conflict: device wins", up.reason());
    assert_eq!(Some("r1"), up.existing_remote_id());
    assert_eq!(0, r.summary().conflicts);
    assert_eq!(120, r.summary().upload_bytes);
}

#[test]
fn conflict_remote_wins_downloads() {
    let r = both_changed_default(ConflictPolicy::RemoteWins);
    let d = r.single("Download");
    assert_eq!("conflict: Drive wins", d.reason());
    assert!(d.overwrites());
    assert_eq!(300, r.summary().download_bytes);
}

#[test]
fn conflict_newer_wins_compares_modification_times() {
    let local_newer = both_changed(ConflictPolicy::NewerWins, T1, T0);
    assert_eq!(
        "conflict: device copy is newer",
        local_newer.single("Upload").reason()
    );

    let remote_newer = both_changed(ConflictPolicy::NewerWins, T1, T1 + 1);
    assert_eq!(
        "conflict: Drive copy is newer",
        remote_newer.single("Download").reason()
    );

    let tie = both_changed(ConflictPolicy::NewerWins, T1, T1);
    assert_eq!(
        "conflict: device copy is newer",
        tie.single("Upload").reason(),
        "ties go to the device copy"
    );
}

#[test]
fn conflict_skip_reports_warning() {
    let r = both_changed_default(ConflictPolicy::Skip);
    let s = r.single("Skip");
    assert_eq!("modified on both sides", s.reason());
    assert!(s.warn());
    assert_eq!(1, r.summary().skips);
    assert_eq!(0, r.summary().conflicts);
    r.assert_only(&["Skip"]);
}

#[test]
fn conflict_both_changed_to_identical_content_adopts() {
    let r = plan(
        policy(ConflictPolicy::Skip),
        vec![state("a.txt")],
        vec![local("a.txt").size(120).mtime(T1).hash("h2")],
        vec![remote("a.txt").md5(Some("h2")).size(Some(120))],
    );
    let a = r.single("Adopt");
    assert_eq!("a.txt", a.key());
    assert_eq!("a.txt", a.local_rel());
    assert_eq!(Some("h2"), a.remote().md5.as_deref());
    r.assert_only(&["Adopt"]);
    assert!(r.summary().is_empty());
}

#[test]
fn files_of_different_sizes_are_never_read_to_compare() {
    let r = plan(
        cfg(),
        vec![],
        vec![local("a.txt").size(120).hash("h1")],
        vec![remote("a.txt").md5(Some("h2")).size(Some(300))],
    );
    assert!(r.actions().iter().all(|a| a.kind() != "Adopt"));
    assert!(r.hash_calls.is_empty(), "the sizes already differ");
}

#[test]
fn same_content_ignores_md5_case() {
    let r = plan(
        cfg(),
        vec![],
        vec![local("a.txt").hash("abc123")],
        vec![remote("a.txt").md5(Some("ABC123"))],
    );
    r.single("Adopt");
    r.assert_only(&["Adopt"]);
}

#[test]
fn two_way_new_on_both_sides_with_different_content_is_a_conflict() {
    let r = plan(
        cfg(),
        vec![],
        vec![local("a.txt").hash("h1")],
        vec![remote("a.txt").md5(Some("h2"))],
    );
    r.single("Conflict");
    assert_eq!(1, r.summary().conflicts);
}

// ================================================================ TWO_WAY: deletions

#[test]
fn two_way_local_deleted_deletes_remote() {
    let r = plan(
        cfg(),
        vec![state("a.txt").remote_id("r1")],
        vec![],
        vec![remote("a.txt").id("r1")],
    );
    let d = r.single("DeleteRemote");
    assert_eq!("r1", d.remote_id());
    assert!(!d.is_dir());
    assert_eq!("deleted on device", d.reason());
    r.assert_only(&["DeleteRemote"]);
    assert_eq!(1, r.summary().delete_remote);
    assert_eq!(vec!["Drive: a.txt"], r.summary().sample_deletes);
}

#[test]
fn two_way_local_deleted_deletions_off_restores_from_drive() {
    let r = plan(
        no_deletions(),
        vec![state("a.txt")],
        vec![],
        vec![remote("a.txt")],
    );
    let d = r.single("Download");
    assert_eq!(
        "restoring: deleted on device but deletions are not propagated",
        d.reason()
    );
    assert!(!d.overwrites());
    assert_eq!(0, r.summary().delete_remote);
    r.assert_none("DeleteRemote");
}

#[test]
fn two_way_remote_deleted_deletes_local() {
    let r = plan(cfg(), vec![state("a.txt")], vec![local("a.txt")], vec![]);
    let d = r.single("DeleteLocal");
    assert_eq!("a.txt", d.local_rel());
    assert!(!d.is_dir());
    assert_eq!("deleted on Drive", d.reason());
    assert_eq!(1, r.summary().delete_local);
    assert_eq!(vec!["device: a.txt"], r.summary().sample_deletes);
}

#[test]
fn two_way_remote_deleted_uses_the_mapped_local_name() {
    let r = plan(
        cfg(),
        vec![state("a?b.txt").local_rel("a？b.txt")],
        vec![local("a?b.txt").rel("a？b.txt")],
        vec![],
    );
    let d = r.single("DeleteLocal");
    assert_eq!("a?b.txt", d.key());
    assert_eq!("a？b.txt", d.local_rel());
    assert_eq!(vec!["device: a？b.txt"], r.summary().sample_deletes);
}

#[test]
fn two_way_remote_deleted_deletions_off_re_uploads() {
    let r = plan(no_deletions(), vec![state("a.txt")], vec![local("a.txt")], vec![]);
    let up = r.single("Upload");
    assert_eq!(
        "re-uploading: deleted on Drive but deletions are not propagated",
        up.reason()
    );
    assert_eq!(None, up.existing_remote_id(), "the old Drive id is gone");
    r.assert_none("DeleteLocal");
}

#[test]
fn two_way_local_deleted_remote_changed_downloads() {
    let r = plan(
        cfg(),
        vec![state("a.txt")],
        vec![],
        vec![remote("a.txt").md5(Some("h2"))],
    );
    let d = r.single("Download");
    assert_eq!("changed on Drive after local delete", d.reason());
    assert!(!d.overwrites());
    r.assert_none("DeleteRemote");
}

#[test]
fn two_way_local_changed_remote_deleted_uploads() {
    let r = plan(
        cfg(),
        vec![state("a.txt")],
        vec![local("a.txt").size(5).mtime(T1).hash("h2")],
        vec![],
    );
    let up = r.single("Upload");
    assert_eq!("changed on device", up.reason());
    assert_eq!(None, up.existing_remote_id());
    r.assert_none("DeleteLocal");
}

#[test]
fn two_way_gone_on_both_sides_forgets() {
    let r = plan(cfg(), vec![state("a.txt")], vec![], vec![]);
    assert_eq!("a.txt", r.single("Forget").key());
    r.assert_only(&["Forget"]);
    assert!(r.summary().is_empty());
}

#[test]
fn two_way_unchanged_both_sides_with_renamed_local_rel_refreshes_state() {
    // the baseline still records the unmapped name, the scan found the mapped one
    let r = plan(
        cfg(),
        vec![state("a?b.txt").local_rel("a?b.txt")],
        vec![local("a?b.txt").rel("a？b.txt")],
        vec![remote("a?b.txt")],
    );
    let rs = r.single("RefreshState");
    let Action::RefreshState {
        local_rel,
        local,
        remote,
        export_mime,
        ..
    } = rs
    else {
        unreachable!()
    };
    assert_eq!("a？b.txt", local_rel);
    assert!(local.is_some());
    assert!(remote.is_some());
    assert!(export_mime.is_none());
    r.assert_only(&["RefreshState"]);
}

// ================================================================ UPLOAD_ONLY / UPLOAD_THEN_DELETE

#[test]
fn upload_only_ignores_remote_only_files() {
    let r = plan(
        mode(SyncMode::UploadOnly),
        vec![],
        vec![],
        vec![remote("only-on-drive.txt")],
    );
    assert!(r.actions().is_empty());
    assert_eq!(1, r.summary().remote_files);
}

#[test]
fn upload_only_local_deleted_never_deletes_remote() {
    let r = plan(
        mode(SyncMode::UploadOnly),
        vec![state("a.txt")],
        vec![],
        vec![remote("a.txt")],
    );
    r.single("Forget");
    r.assert_only(&["Forget"]);
    assert_eq!(0, r.summary().delete_remote);
}

#[test]
fn upload_only_new_and_changed_local_files_are_uploaded() {
    let r = plan(
        mode(SyncMode::UploadOnly),
        vec![state("c.txt").remote_id("rc")],
        vec![local("n.txt"), local("c.txt").size(300).mtime(T1).hash("h2")],
        vec![remote("c.txt").id("rc")],
    );
    let ups = by_key(&r.all("Upload"));
    assert_eq!(vec!["c.txt", "n.txt"], ups.keys().collect::<Vec<_>>());
    assert_eq!(None, ups["n.txt"].existing_remote_id());
    assert_eq!(Some("rc"), ups["c.txt"].existing_remote_id());
    assert!(ups.values().all(|u| u.reason() == "new or changed on device"));
    assert_eq!(2, r.summary().uploads);
    assert_eq!(400, r.summary().upload_bytes);
}

#[test]
fn upload_only_both_changed_uploads_without_conflict() {
    let r = plan(
        PlannerConfig {
            mode: SyncMode::UploadOnly,
            ..policy(ConflictPolicy::Skip)
        },
        vec![state("a.txt")],
        vec![local("a.txt").size(120).mtime(T1).hash("h2")],
        vec![remote("a.txt").md5(Some("h3"))],
    );
    r.single("Upload");
    assert_eq!(0, r.summary().conflicts);
    assert_eq!(0, r.summary().skips);
}

#[test]
fn upload_only_remote_deleted_after_upload_is_skipped_not_reuploaded() {
    let r = plan(
        mode(SyncMode::UploadOnly),
        vec![state("a.txt")],
        vec![local("a.txt")],
        vec![],
    );
    let s = r.single("Skip");
    assert_eq!(
        "already uploaded once; not re-uploading after Drive deletion",
        s.reason()
    );
    assert!(!s.warn());
    assert_eq!(1, r.summary().skips);
    assert_eq!(0, r.summary().uploads);
}

#[test]
fn upload_only_remote_changed_local_unchanged_refreshes_state() {
    let r = plan(
        mode(SyncMode::UploadOnly),
        vec![state("a.txt")],
        vec![local("a.txt")],
        vec![remote("a.txt").md5(Some("h2"))],
    );
    r.single("RefreshState");
    r.assert_only(&["RefreshState"]);
}

#[test]
fn upload_only_new_local_matching_remote_content_adopts() {
    let r = plan(
        mode(SyncMode::UploadOnly),
        vec![],
        vec![local("a.txt").hash("h1")],
        vec![remote("a.txt").md5(Some("h1"))],
    );
    r.single("Adopt");
    assert_eq!(0, r.summary().uploads);
}

#[test]
fn upload_then_delete_plans_uploads_only_executor_deletes_locally() {
    let r = plan(
        mode(SyncMode::UploadThenDelete),
        vec![],
        vec![local("a.txt")],
        vec![remote("stays-on-drive.txt")],
    );
    r.single("Upload");
    r.assert_only(&["Upload"]);
    assert_eq!(0, r.summary().delete_local);
    assert_eq!(0, r.summary().delete_remote);
    // The preview still says the file leaves the device (E2E 2026-10: it showed "0 to bin").
    assert_eq!(1, r.summary().moved_after_transfer);
}

// ================================================================ UPLOAD_MIRROR

#[test]
fn upload_mirror_overwrites_remote_changes() {
    let r = plan(
        mode(SyncMode::UploadMirror),
        vec![state("a.txt").remote_id("r1")],
        vec![local("a.txt")],
        vec![remote("a.txt").id("r1").md5(Some("h2"))],
    );
    let up = r.single("Upload");
    assert_eq!("mirror: overwriting Drive change", up.reason());
    assert_eq!(Some("r1"), up.existing_remote_id());
    r.assert_none("Download");
}

#[test]
fn upload_mirror_deletes_remote_extras_even_when_deletions_are_off() {
    for propagate in [true, false] {
        let c = PlannerConfig {
            propagate_deletions: propagate,
            ..mode(SyncMode::UploadMirror)
        };
        let r = plan(c, vec![], vec![], vec![remote("extra.txt").id("rx")]);
        let d = r.single("DeleteRemote");
        assert_eq!("rx", d.remote_id());
        assert_eq!("mirror: not on device", d.reason());
        assert_eq!(1, r.summary().delete_remote);
    }
}

#[test]
fn upload_mirror_local_deleted_after_sync_deletes_remote() {
    let r = plan(
        mode(SyncMode::UploadMirror),
        vec![state("a.txt")],
        vec![],
        vec![remote("a.txt")],
    );
    assert_eq!("mirror: not on device", r.single("DeleteRemote").reason());
}

#[test]
fn upload_mirror_remote_deleted_re_uploads() {
    let r = plan(
        mode(SyncMode::UploadMirror),
        vec![state("a.txt")],
        vec![local("a.txt")],
        vec![],
    );
    assert_eq!("mirror: missing on Drive", r.single("Upload").reason());
}

#[test]
fn upload_mirror_new_local_uploads_and_unchanged_is_silent() {
    let r = plan(
        mode(SyncMode::UploadMirror),
        vec![state("same.txt")],
        vec![local("same.txt"), local("new.txt")],
        vec![remote("same.txt")],
    );
    assert_eq!("new.txt", r.single("Upload").key());
    assert_eq!("mirror: device version", r.single("Upload").reason());
    r.assert_only(&["Upload"]);
}

// ================================================================ DOWNLOAD_ONLY / DOWNLOAD_THEN_DELETE

#[test]
fn download_only_ignores_local_only_files() {
    let r = plan(
        mode(SyncMode::DownloadOnly),
        vec![],
        vec![local("only-local.txt")],
        vec![],
    );
    assert!(r.actions().is_empty());
}

#[test]
fn download_only_remote_deleted_never_deletes_local() {
    let r = plan(
        mode(SyncMode::DownloadOnly),
        vec![state("a.txt")],
        vec![local("a.txt")],
        vec![],
    );
    r.single("Forget");
    r.assert_only(&["Forget"]);
    assert_eq!(0, r.summary().delete_local);
}

#[test]
fn download_only_new_and_changed_remote_files_are_downloaded() {
    let r = plan(
        mode(SyncMode::DownloadOnly),
        vec![state("c.txt")],
        vec![local("c.txt")],
        vec![
            remote("n.txt").size(Some(10)),
            remote("c.txt").md5(Some("h2")).size(Some(20)),
        ],
    );
    let downloads = by_key(&r.all("Download"));
    assert_eq!(vec!["c.txt", "n.txt"], downloads.keys().collect::<Vec<_>>());
    assert!(!downloads["n.txt"].overwrites());
    assert!(downloads["c.txt"].overwrites());
    assert!(
        downloads
            .values()
            .all(|d| d.reason() == "new or changed on Drive")
    );
    assert_eq!(30, r.summary().download_bytes);
}

#[test]
fn download_only_both_changed_downloads_without_conflict() {
    let r = plan(
        PlannerConfig {
            mode: SyncMode::DownloadOnly,
            ..policy(ConflictPolicy::Skip)
        },
        vec![state("a.txt")],
        vec![local("a.txt").size(120).mtime(T1).hash("h2")],
        vec![remote("a.txt").md5(Some("h3"))],
    );
    assert!(r.single("Download").overwrites());
    assert_eq!(0, r.summary().conflicts);
}

#[test]
fn download_only_local_deleted_after_download_is_skipped_not_redownloaded() {
    let r = plan(
        mode(SyncMode::DownloadOnly),
        vec![state("a.txt")],
        vec![],
        vec![remote("a.txt")],
    );
    let s = r.single("Skip");
    assert_eq!("already downloaded once; device copy was removed", s.reason());
    assert!(!s.warn());
    assert_eq!(1, r.summary().skips);
    assert_eq!(0, r.summary().downloads);
}

#[test]
fn download_only_local_changed_remote_unchanged_refreshes_state() {
    let r = plan(
        mode(SyncMode::DownloadOnly),
        vec![state("a.txt")],
        vec![local("a.txt").size(7).mtime(T1).hash("h2")],
        vec![remote("a.txt")],
    );
    r.single("RefreshState");
    r.assert_only(&["RefreshState"]);
    assert_eq!(vec!["a.txt"], r.hash_calls);
}

#[test]
fn download_only_new_remote_matching_local_content_adopts() {
    let r = plan(
        mode(SyncMode::DownloadOnly),
        vec![],
        vec![local("a.txt").hash("h1")],
        vec![remote("a.txt").md5(Some("h1"))],
    );
    r.single("Adopt");
    assert_eq!(0, r.summary().downloads);
}

#[test]
fn download_then_delete_plans_downloads_only_executor_trashes_remote() {
    let r = plan(
        mode(SyncMode::DownloadThenDelete),
        vec![],
        vec![local("stays-local.txt")],
        vec![remote("a.txt")],
    );
    r.single("Download");
    r.assert_only(&["Download"]);
    assert_eq!(0, r.summary().delete_remote);
    assert_eq!(0, r.summary().delete_local);
    assert_eq!(1, r.summary().moved_after_transfer);
}

// ================================================================ DOWNLOAD_MIRROR

#[test]
fn download_mirror_overwrites_local_changes() {
    let r = plan(
        mode(SyncMode::DownloadMirror),
        vec![state("a.txt")],
        vec![local("a.txt").size(120).mtime(T1).hash("h2")],
        vec![remote("a.txt")],
    );
    let d = r.single("Download");
    assert_eq!("mirror: overwriting device change", d.reason());
    assert!(d.overwrites());
    r.assert_none("Upload");
}

#[test]
fn download_mirror_deletes_local_extras_even_when_deletions_are_off() {
    for propagate in [true, false] {
        let c = PlannerConfig {
            propagate_deletions: propagate,
            ..mode(SyncMode::DownloadMirror)
        };
        let r = plan(c, vec![], vec![local("extra.txt")], vec![]);
        let d = r.single("DeleteLocal");
        assert_eq!("extra.txt", d.local_rel());
        assert_eq!("mirror: not on Drive", d.reason());
        assert_eq!(1, r.summary().delete_local);
    }
}

#[test]
fn download_mirror_remote_deleted_after_sync_deletes_local() {
    let r = plan(
        mode(SyncMode::DownloadMirror),
        vec![state("a.txt")],
        vec![local("a.txt")],
        vec![],
    );
    assert_eq!("mirror: not on Drive", r.single("DeleteLocal").reason());
}

#[test]
fn download_mirror_local_deleted_restores_from_drive() {
    let r = plan(
        mode(SyncMode::DownloadMirror),
        vec![state("a.txt")],
        vec![],
        vec![remote("a.txt")],
    );
    let d = r.single("Download");
    assert_eq!("mirror: missing on device", d.reason());
    assert!(!d.overwrites());
}

#[test]
fn download_mirror_new_remote_downloads_and_unchanged_is_silent() {
    let r = plan(
        mode(SyncMode::DownloadMirror),
        vec![state("same.txt")],
        vec![local("same.txt")],
        vec![remote("same.txt"), remote("new.txt")],
    );
    assert_eq!("new.txt", r.single("Download").key());
    assert_eq!("mirror: Drive version", r.single("Download").reason());
    r.assert_only(&["Download"]);
}

// ================================================================ folders

#[test]
fn folders_new_remote_folders_are_created_locally_shallow_first() {
    let r = plan(
        cfg(),
        vec![],
        vec![],
        vec![
            remote("b/c/d").dir(),
            remote("b/c/d/f.txt"),
            remote("a").dir(),
            remote("b/c").dir(),
            remote("b").dir(),
        ],
    );
    let mkdirs = r.all("MkdirLocal");
    assert_eq!(vec!["a", "b", "b/c", "b/c/d"], keys(&mkdirs));
    assert_eq!(
        vec!["a", "b", "b/c", "b/c/d"],
        mkdirs.iter().map(|a| a.local_rel()).collect::<Vec<_>>()
    );
    assert_eq!(4, r.summary().mkdirs);
    assert_eq!(1, r.summary().remote_files);
    let last_mkdir = r
        .actions()
        .iter()
        .rposition(|a| a.kind() == "MkdirLocal")
        .unwrap();
    let download = r.actions().iter().position(|a| a.kind() == "Download").unwrap();
    assert!(last_mkdir < download, "directories come before transfers");
    assert_eq!("b/c/d/f.txt", r.single("Download").local_rel());
}

#[test]
fn folders_new_local_folders_are_created_on_drive_shallow_first() {
    let r = plan(
        cfg(),
        vec![],
        vec![local("a/b/c.txt"), local("a/b").dir(), local("a").dir()],
        vec![],
    );
    let mk = r.all("MkdirRemote");
    assert_eq!(vec!["a", "a/b"], keys(&mk));
    assert_eq!(
        vec![("", "a"), ("a", "b")],
        mk.iter().map(|a| (a.parent_key(), a.name())).collect::<Vec<_>>()
    );
    let up = r.single("Upload");
    assert_eq!("a/b", up.parent_key());
    assert_eq!("c.txt", up.name());
    let up_index = r.actions().iter().position(|a| a.kind() == "Upload").unwrap();
    assert!(
        r.actions()
            .iter()
            .rposition(|a| a.kind() == "MkdirRemote")
            .unwrap()
            < up_index
    );
    assert_eq!(2, r.summary().mkdirs);
    assert_eq!(1, r.summary().local_files);
}

#[test]
fn folders_present_on_both_sides_without_baseline_refresh_state() {
    let r = plan(cfg(), vec![], vec![local("d").dir()], vec![remote("d").dir()]);
    let rs = r.single("RefreshState");
    assert_eq!("d", rs.key());
    assert_eq!("d", rs.local_rel());
    r.assert_only(&["RefreshState"]);

    let synced = plan(
        cfg(),
        vec![dir_state("d")],
        vec![local("d").dir()],
        vec![remote("d").dir()],
    );
    assert!(synced.actions().is_empty());
}

#[test]
fn folders_removed_on_drive_after_sync_is_deleted_locally_children_first() {
    let r = plan(
        cfg(),
        vec![dir_state("d"), state("d/f.txt")],
        vec![local("d").dir(), local("d/f.txt")],
        vec![],
    );
    let dels = r.all("DeleteLocal");
    assert_eq!(vec!["d/f.txt", "d"], keys(&dels));
    assert!(!dels[0].is_dir());
    assert!(dels[1].is_dir());
    assert_eq!("folder removed on Drive", dels[1].reason());
    assert_eq!(2, r.summary().delete_local);
    r.assert_only(&["DeleteLocal", "DeleteLocal"]);
}

#[test]
fn folders_removed_on_drive_after_sync_deletions_off_is_recreated() {
    let r = plan(
        no_deletions(),
        vec![dir_state("d"), state("d/f.txt")],
        vec![local("d").dir(), local("d/f.txt")],
        vec![],
    );
    let mk = r.single("MkdirRemote");
    assert_eq!("d", mk.key());
    assert_eq!("d", r.single("Upload").parent_key());
    r.assert_none("DeleteLocal");
}

#[test]
fn folders_removed_on_device_after_sync_is_deleted_on_drive() {
    let r = plan(
        cfg(),
        vec![dir_state("d").remote_id("rd"), state("d/f.txt").remote_id("rf")],
        vec![],
        vec![remote("d").id("rd").dir(), remote("d/f.txt").id("rf")],
    );
    let dels = r.all("DeleteRemote");
    assert_eq!(vec!["d/f.txt", "d"], keys(&dels));
    assert_eq!(
        vec!["rf", "rd"],
        dels.iter().map(|a| a.remote_id()).collect::<Vec<_>>()
    );
    assert!(dels[1].is_dir());
    assert_eq!("folder removed on device", dels[1].reason());
    assert_eq!(2, r.summary().delete_remote);
}

#[test]
fn folders_removed_on_device_while_drive_got_new_files_inside_keeps_the_folder_on_drive() {
    // Trashing the Drive folder would take the new file with it, after downloading it.
    let r = plan(
        cfg(),
        vec![dir_state("d").remote_id("rd"), state("d/f.txt").remote_id("rf")],
        vec![],
        vec![
            remote("d").id("rd").dir(),
            remote("d/f.txt").id("rf"),
            remote("d/new.txt").id("rn"),
        ],
    );
    assert_eq!(
        vec!["d/f.txt"],
        keys(&r.all("DeleteRemote")),
        "only the synced file goes"
    );
    assert_eq!("d/new.txt", r.single("Download").key());
}

#[test]
fn folders_removed_on_device_while_a_file_inside_changed_on_drive_keeps_the_folder_on_drive() {
    let r = plan(
        cfg(),
        vec![dir_state("d").remote_id("rd"), state("d/f.txt").remote_id("rf")],
        vec![],
        vec![
            remote("d").id("rd").dir(),
            remote("d/f.txt").id("rf").md5(Some("h2")).version(Some(2)),
        ],
    );
    assert!(
        r.all("DeleteRemote").iter().all(|a| a.key() != "d"),
        "the folder holding a changed file was trashed"
    );
}

#[test]
fn folders_removed_on_device_after_sync_deletions_off_is_recreated() {
    let r = plan(
        no_deletions(),
        vec![dir_state("d").remote_id("rd")],
        vec![],
        vec![remote("d").id("rd").dir()],
    );
    assert_eq!("d", r.single("MkdirLocal").local_rel());
    r.assert_none("DeleteRemote");
}

#[test]
fn folders_type_mismatch_is_a_conflict() {
    let file_vs_dir = plan(cfg(), vec![], vec![local("x")], vec![remote("x").dir()]);
    let c = file_vs_dir.single("Conflict");
    assert_eq!("x", c.key());
    assert_eq!("", c.parent_key());
    assert_eq!(1, file_vs_dir.summary().conflicts);

    let dir_vs_file = plan(cfg(), vec![], vec![local("p/x").dir()], vec![remote("p/x")]);
    assert_eq!("p", dir_vs_file.single("Conflict").parent_key());
}

#[test]
fn folders_gone_on_both_sides_forgets() {
    let r = plan(cfg(), vec![dir_state("d")], vec![], vec![]);
    assert_eq!("d", r.single("Forget").key());
}

#[test]
fn folders_one_way_modes_ignore_folders_of_the_other_side() {
    let down = plan(
        mode(SyncMode::DownloadOnly),
        vec![],
        vec![local("only-local").dir()],
        vec![],
    );
    assert!(down.actions().is_empty());
    let up = plan(
        mode(SyncMode::UploadOnly),
        vec![],
        vec![],
        vec![remote("only-remote").dir()],
    );
    assert!(up.actions().is_empty());
}

#[test]
fn folders_mirror_modes_remove_folders_deleted_on_the_source_side() {
    let up = plan(
        mode(SyncMode::UploadMirror),
        vec![dir_state("d").remote_id("rd")],
        vec![],
        vec![remote("d").id("rd").dir()],
    );
    assert!(up.single("DeleteRemote").is_dir());
    let down = plan(
        mode(SyncMode::DownloadMirror),
        vec![dir_state("d")],
        vec![local("d").dir()],
        vec![],
    );
    assert!(down.single("DeleteLocal").is_dir());
}

// ================================================================ size limit

#[test]
fn the_size_limit_keeps_out_only_a_file_above_it_that_was_never_synced() {
    assert!(over_size_limit(1000, 1001, false));
    assert!(
        !over_size_limit(1000, 1000, false),
        "a file exactly at the limit syncs"
    );
    assert!(!over_size_limit(1000, 5000, true), "a synced file keeps syncing");
    assert!(!over_size_limit(0, u64::MAX, false), "0 means no limit");
}

#[test]
fn only_a_name_with_an_export_extension_can_be_written_by_an_export() {
    assert!(is_export_name("Notes.docx", false));
    assert!(is_export_name("Budget.XLSX", false), "case does not matter");
    assert!(
        is_export_name("code.json", false),
        "Apps Script exports to JSON in both formats"
    );
    assert!(!is_export_name("Notes.docx", true), "PDF exports write no .docx");
    assert!(is_export_name("Notes.pdf", true));
    assert!(!is_export_name("Notes.txt", false));
    assert!(
        !is_export_name(".docx", false),
        "a bare extension has no doc name"
    );
}

fn limited(max_file_size: u64) -> PlannerConfig {
    PlannerConfig {
        max_file_size,
        ..cfg()
    }
}

#[test]
fn max_file_size_skips_new_local_files_above_the_limit() {
    let r = plan(
        limited(1000),
        vec![],
        vec![local("big.bin").size(1001), local("ok.bin").size(1000)],
        vec![],
    );
    let s = r.single("Skip");
    assert_eq!("big.bin", s.key());
    assert_eq!("larger than the pair's size limit", s.reason());
    assert!(!s.warn());
    assert_eq!("ok.bin", r.single("Upload").key());
    assert_eq!(1, r.summary().skips);
    assert_eq!(1, r.summary().uploads);
    assert_eq!(1000, r.summary().upload_bytes);
}

#[test]
fn max_file_size_does_not_apply_to_files_already_synced() {
    let r = plan(
        limited(1000),
        vec![state("big.bin").size(2000)],
        vec![local("big.bin").size(2500).mtime(T1).hash("h2")],
        vec![remote("big.bin").size(Some(2000))],
    );
    assert_eq!("changed on device", r.single("Upload").reason());
    assert_eq!(0, r.summary().skips);
}

#[test]
fn max_file_size_zero_means_unlimited() {
    let r = plan(
        limited(0),
        vec![],
        vec![local("huge.bin").size(50 * 1024 * 1024 * 1024)],
        vec![],
    );
    r.single("Upload");
    assert_eq!(0, r.summary().skips);
}

// ================================================================ Google Docs export

fn doc(key: &str) -> RemoteEntry {
    RemoteEntry {
        id: format!("g-{key}"),
        md5: None,
        version: Some(3),
        google_mime: Some(GOOGLE_DOC.into()),
        size: None,
        ..remote(key)
    }
}

fn doc_of(key: &str, mime: &str) -> RemoteEntry {
    RemoteEntry {
        google_mime: Some(mime.into()),
        ..doc(key)
    }
}

fn export_state() -> BaselineEntry {
    BaselineEntry {
        local_rel: "notes.docx".into(),
        local_size: 5000,
        local_hash: Some("h9".into()),
        remote_id: Some("g-notes".into()),
        remote_md5: None,
        remote_size: Some(5000),
        remote_version: Some(3),
        export_mime: Some(DOCX.into()),
        ..state("notes")
    }
}

fn export_copy() -> L {
    local("notes").rel("notes.docx").size(5000).hash("h9")
}

#[test]
fn docs_new_google_doc_is_exported_as_docx() {
    let d = doc("notes");
    let r = plan(cfg(), vec![], vec![], vec![d.clone()]);
    let e = r.single("Export");
    assert_eq!("notes", e.key());
    assert_eq!(&d, e.remote());
    assert_eq!("notes.docx", e.local_rel());
    assert_eq!(DOCX, e.mime());
    assert_eq!("Google Doc changed on Drive", e.reason());
    r.assert_only(&["Export"]);
    assert_eq!(1, r.summary().exports);
    assert_eq!(0, r.summary().downloads);
    assert_eq!(0, r.summary().download_bytes);
}

#[test]
fn docs_pdf_format_exports_pdf() {
    let c = PlannerConfig {
        docs_format: DocsFormat::Pdf,
        ..cfg()
    };
    let r = plan(
        c,
        vec![],
        vec![],
        vec![doc("notes"), doc_of("budget", GOOGLE_SHEET)],
    );
    let exports = by_key(&r.all("Export"));
    assert_eq!("notes.pdf", exports["notes"].local_rel());
    assert_eq!(PDF, exports["notes"].mime());
    assert_eq!("budget.pdf", exports["budget"].local_rel());
    assert_eq!(PDF, exports["budget"].mime());
    assert_eq!(2, r.summary().exports);
}

#[test]
fn docs_office_format_maps_each_google_type() {
    let r = plan(
        cfg(),
        vec![],
        vec![],
        vec![doc_of("budget", GOOGLE_SHEET), doc_of("deck", GOOGLE_SLIDES)],
    );
    let exports = by_key(&r.all("Export"));
    assert_eq!("budget.xlsx", exports["budget"].local_rel());
    assert_eq!(XLSX, exports["budget"].mime());
    assert_eq!("deck.pptx", exports["deck"].local_rel());
    assert_eq!(PPTX, exports["deck"].mime());
}

#[test]
fn docs_nested_doc_with_unsafe_name_uses_mapped_local_path() {
    let r = plan(cfg(), vec![], vec![], vec![remote("Q?").dir(), doc("Q?/plan:b")]);
    assert_eq!("Q？", r.single("MkdirLocal").local_rel());
    assert_eq!("Q？/plan：b.docx", r.single("Export").local_rel());
}

#[test]
fn docs_unchanged_version_and_mtime_is_not_re_exported() {
    let r = plan(
        cfg(),
        vec![export_state()],
        vec![export_copy()],
        vec![doc("notes")],
    );
    assert!(r.actions().is_empty());
    assert_eq!(0, r.summary().exports);
    assert!(r.summary().warnings.is_empty());
    assert!(r.hash_calls.is_empty());
}

#[test]
fn docs_changed_version_re_exports_to_the_same_local_name() {
    let r = plan(
        cfg(),
        vec![export_state()],
        vec![export_copy()],
        vec![doc("notes").version(Some(4))],
    );
    let e = r.single("Export");
    assert_eq!(
        "notes.docx",
        e.local_rel(),
        "the existing extension is not duplicated"
    );
    assert_eq!(1, r.summary().exports);
}

#[test]
fn docs_changed_mtime_only_re_exports() {
    let r = plan(
        cfg(),
        vec![export_state()],
        vec![export_copy()],
        vec![doc("notes").mtime(T1)],
    );
    r.single("Export");
}

#[test]
fn docs_local_export_copy_missing_is_re_exported() {
    let r = plan(cfg(), vec![export_state()], vec![], vec![doc("notes")]);
    assert_eq!("notes.docx", r.single("Export").local_rel());
}

#[test]
fn docs_locally_modified_export_copy_is_never_uploaded_back() {
    let r = plan(
        cfg(),
        vec![export_state()],
        vec![export_copy().size(6000).mtime(T1).hash("h10")],
        vec![doc("notes")],
    );
    r.assert_none("Upload");
    r.assert_none("Conflict");
    assert_eq!(1, r.summary().warnings.len());
    assert!(r.summary().warnings[0].contains("notes.docx"));
    assert!(r.summary().warnings[0].contains("cannot be uploaded back"));
}

#[test]
fn docs_export_mirror_is_never_uploaded_in_upload_modes() {
    let r = plan(
        mode(SyncMode::UploadOnly),
        vec![export_state()],
        vec![export_copy().size(6000).mtime(T1).hash("h10")],
        vec![doc("notes")],
    );
    r.assert_none("Upload");
    let s = r.single("Skip");
    assert_eq!(
        "Google Docs file ignored (export disabled or upload mode)",
        s.reason()
    );
    assert_eq!(1, r.summary().skips);
}

#[test]
fn docs_remote_doc_deleted_deletes_the_local_export() {
    let r = plan(cfg(), vec![export_state()], vec![export_copy()], vec![]);
    let d = r.single("DeleteLocal");
    assert_eq!("notes", d.key());
    assert_eq!("notes.docx", d.local_rel());
    assert!(!d.is_dir());
    assert_eq!("exported Google Doc removed on Drive", d.reason());
    r.assert_only(&["DeleteLocal"]);
}

#[test]
fn docs_remote_doc_deleted_and_local_export_gone_forgets() {
    let r = plan(cfg(), vec![export_state()], vec![], vec![]);
    assert_eq!("notes", r.single("Forget").key());
}

#[test]
fn docs_export_disabled_skips() {
    let r = plan(
        PlannerConfig {
            docs_export: false,
            ..cfg()
        },
        vec![],
        vec![],
        vec![doc("notes")],
    );
    let s = r.single("Skip");
    assert_eq!("notes", s.key());
    assert_eq!(
        "Google Docs file ignored (export disabled or upload mode)",
        s.reason()
    );
    assert!(!s.warn());
    assert_eq!(1, r.summary().skips);
    assert_eq!(0, r.summary().exports);
    r.assert_none("Export");
}

#[test]
fn docs_upload_only_mode_skips() {
    let r = plan(mode(SyncMode::UploadOnly), vec![], vec![], vec![doc("notes")]);
    r.single("Skip");
    assert_eq!(1, r.summary().skips);
}

#[test]
fn docs_unsupported_google_type_skips() {
    let r = plan(
        cfg(),
        vec![],
        vec![],
        vec![doc_of("survey", "application/vnd.google-apps.form")],
    );
    let s = r.single("Skip");
    assert_eq!(
        "no export format for application/vnd.google-apps.form",
        s.reason()
    );
    assert_eq!(1, r.summary().skips);
}

#[test]
fn docs_remote_doc_replaced_by_regular_file_drops_the_mirror_instead_of_crashing() {
    // The Google Doc "notes" was deleted on Drive and a plain file named "notes" took its place.
    let plain = || remote("notes").id("f2").md5(Some("zz")).size(Some(10));
    let r = plan(cfg(), vec![export_state()], vec![export_copy()], vec![plain()]);
    let d = r.single("DeleteLocal");
    assert_eq!("notes", d.key());
    assert_eq!("notes.docx", d.local_rel());
    r.assert_none("Upload");
    r.assert_none("Export");

    // without a local copy the stale baseline row is simply forgotten
    let gone = plan(cfg(), vec![export_state()], vec![], vec![plain()]);
    assert_eq!("notes", gone.single("Forget").key());
}

// ================================================================ mass-delete guard

#[test]
fn mass_delete_guard_blocks_when_count_exceeds_the_limit() {
    // "More than N files": 4 deletions with a limit of 3 stop the sync even though 40% < 90%.
    let keys = synced(10);
    let r = plan(guarded(3, 90), states(&keys), locals(&keys[4..]), remotes(&keys)); // 4 of 10 deleted on the phone
    assert!(r.plan.blocked);
    assert_eq!(
        Some(
            "This sync would delete 4 files on Drive, 40% of the folder. Drive still has every file until you decide."
        ),
        r.summary().blocked_reason.as_deref()
    );
    assert_eq!(Some(BlockKind::Deletions), r.summary().blocked_kind);
    assert_eq!(Some(BlockSide::Drive), r.summary().blocked_side);
    assert_eq!(4, r.summary().blocked_count);
    assert_eq!(40, r.summary().blocked_percent);
    assert_eq!(
        4,
        r.all("DeleteRemote").len(),
        "the actions are still planned; the engine decides"
    );
    assert_eq!(4, r.summary().delete_remote);
    assert_eq!(
        vec![
            "Drive: f01.txt",
            "Drive: f02.txt",
            "Drive: f03.txt",
            "Drive: f04.txt"
        ],
        r.summary().sample_deletes
    );
}

#[test]
fn mass_delete_guard_blocks_when_percent_exceeds_the_limit() {
    // "...or P% of the folder": 5 of 10 (50%) with a 30% limit, well under the 20-file limit.
    let keys = synced(10);
    let r = plan(guarded(20, 30), states(&keys), locals(&keys[5..]), remotes(&keys));
    assert!(r.plan.blocked);
    assert_eq!(50, r.summary().blocked_percent);
}

#[test]
fn mass_delete_guard_percent_rule_ignores_batches_below_the_floor() {
    // 4 of 10 is 40%, above 30%, but fewer than PERCENT_FLOOR files: deleting a few files from a
    // small folder doesn't stop the sync.
    let keys = synced(10);
    let r = plan(guarded(20, 30), states(&keys), locals(&keys[4..]), remotes(&keys));
    assert!(!r.plan.blocked);
    assert_eq!(4, r.summary().delete_remote);
}

#[test]
fn first_merge_guard_blocks_the_first_sync_of_two_non_empty_folders() {
    let r = plan(
        first_merge(cfg()),
        vec![],
        locals(&synced(3)),
        vec![remote("other.txt")],
    );
    assert!(r.plan.blocked);
    assert_eq!(Some(BlockKind::FirstMerge), r.summary().blocked_kind);
    assert_eq!(
        Some(
            "The first sync merges two folders that both have files: 3 on this device and 1 on Drive. Nothing is deleted."
        ),
        r.summary().blocked_reason.as_deref()
    );
}

/// A mirror's first sync deletes what only one side has; its stop says so instead of "Nothing is
/// deleted" (E2E 2026-10, download mirror on the computer).
#[test]
fn first_merge_guard_of_a_mirror_says_what_it_deletes() {
    let r = plan(
        first_merge(PlannerConfig {
            mode: SyncMode::DownloadMirror,
            ..cfg()
        }),
        vec![],
        locals(&synced(3)),
        vec![remote("other.txt")],
    );
    assert_eq!(Some(BlockKind::FirstMerge), r.summary().blocked_kind);
    assert_eq!(
        Some(
            "The first sync merges two folders that both have files: 3 on this device and 1 on Drive. 3 files only on this device go to its recycle bin, so it matches Drive."
        ),
        r.summary().blocked_reason.as_deref()
    );
}

#[test]
fn first_merge_guard_allows_an_empty_side_and_approved_runs() {
    assert!(
        !plan(first_merge(cfg()), vec![], locals(&synced(3)), vec![])
            .plan
            .blocked
    );
    assert!(
        !plan(
            approved(first_merge(cfg())),
            vec![],
            locals(&synced(3)),
            vec![remote("other.txt")]
        )
        .plan
        .blocked
    );
}

#[test]
fn summary_counts_files_that_already_match() {
    let keys = synced(4);
    let mut l = locals(&keys);
    l.push(local("new.txt"));
    let r = plan(cfg(), states(&keys), l, remotes(&keys));
    assert_eq!(4, r.summary().unchanged);
    assert_eq!(1, r.summary().uploads);
}

#[test]
fn mass_delete_guard_not_blocked_below_min_files() {
    let keys = synced(10);
    let r = plan(guarded(4, 30), states(&keys), locals(&keys[3..]), remotes(&keys));
    assert!(!r.plan.blocked);
    assert_eq!(None, r.summary().blocked_reason);
    assert_eq!(3, r.summary().delete_remote);
}

#[test]
fn mass_delete_guard_not_blocked_below_threshold_percent() {
    let keys = synced(10);
    let r = plan(guarded(3, 31), states(&keys), locals(&keys[3..]), remotes(&keys));
    assert!(!r.plan.blocked);
    assert_eq!(None, r.summary().blocked_reason);
}

#[test]
fn mass_delete_guard_not_blocked_when_approved_once() {
    let keys = synced(10);
    let r = plan(
        approved(guarded(3, 30)),
        states(&keys),
        locals(&keys[3..]),
        remotes(&keys),
    );
    assert!(!r.plan.blocked);
    assert_eq!(None, r.summary().blocked_reason);
    assert_eq!(3, r.summary().delete_remote);
}

#[test]
fn mass_delete_guard_not_blocked_when_disabled() {
    let keys = synced(10);
    let mut c = guarded(1, 1);
    c.guards.mass_delete_guard = false;
    let r = plan(c, states(&keys), vec![], remotes(&keys));
    assert!(!r.plan.blocked);
    assert_eq!(10, r.summary().delete_remote);
}

#[test]
fn mass_delete_guard_counts_local_deletions_against_the_device_folder() {
    let keys = synced(10);
    let r = plan(guarded(3, 30), states(&keys), locals(&keys), remotes(&keys[4..])); // 4 of 10 deleted on Drive
    assert!(r.plan.blocked);
    assert_eq!(
        Some(
            "This sync would delete 4 files on this device, 40% of the folder. This device still has every file until you decide."
        ),
        r.summary().blocked_reason.as_deref()
    );
    assert_eq!(Some(BlockSide::Device), r.summary().blocked_side);
    assert_eq!(4, r.summary().delete_local);
    assert_eq!(
        vec![
            "device: f01.txt",
            "device: f02.txt",
            "device: f03.txt",
            "device: f04.txt"
        ],
        r.summary().sample_deletes
    );
}

#[test]
fn mass_delete_guard_empty_device_folder_after_previous_sync_is_blocked_regardless_of_thresholds() {
    let keys = synced(2);
    let r = plan(guarded(10, 90), states(&keys), vec![], remotes(&keys));
    assert!(r.plan.blocked);
    assert_eq!(
        Some("The folder on this device is empty but was synced before. Wasl won't empty Drive to match it."),
        r.summary().blocked_reason.as_deref()
    );
}

#[test]
fn mass_delete_guard_empty_drive_folder_after_previous_sync_is_blocked_regardless_of_thresholds() {
    let keys = synced(2);
    let r = plan(guarded(10, 90), states(&keys), locals(&keys), vec![]);
    assert!(r.plan.blocked);
    assert_eq!(
        Some(
            "The Drive folder is empty but was synced before. Wasl won't empty this device's folder to match it."
        ),
        r.summary().blocked_reason.as_deref()
    );
}

#[test]
fn mass_delete_guard_first_sync_without_baseline_is_not_blocked_by_the_empty_folder_rule() {
    let r = plan(guarded(3, 30), vec![], locals(&synced(5)), vec![]);
    assert!(!r.plan.blocked);
    assert_eq!(5, r.summary().uploads);
}

#[test]
fn mass_delete_guard_mirror_wipe_of_a_folder_never_synced_is_blocked_by_percentage() {
    // DOWNLOAD_MIRROR into a folder with 5 unrelated files and an empty Drive folder: 5 of 5 deletions
    let c = PlannerConfig {
        mode: SyncMode::DownloadMirror,
        ..guarded(3, 30)
    };
    let r = plan(c, vec![], locals(&synced(5)), vec![]);
    assert!(r.plan.blocked);
    assert_eq!(
        Some(
            "This sync would delete 5 files on this device, 100% of the folder. This device still has every file until you decide."
        ),
        r.summary().blocked_reason.as_deref()
    );
}

#[test]
fn sample_deletes_are_capped_at_ten() {
    let keys = synced(12);
    let r = plan(cfg(), states(&keys), vec![], remotes(&keys));
    assert_eq!(12, r.summary().delete_remote);
    assert_eq!(10, r.summary().sample_deletes.len());
    assert!(
        r.summary()
            .sample_deletes
            .iter()
            .all(|s| s.starts_with("Drive: "))
    );
}

// ================================================================ ordering

#[test]
fn ordering_mkdirs_then_transfers_then_deletions_deepest_first() {
    let r = plan(
        cfg(),
        vec![
            dir_state("d"),
            state("d/x.txt"),
            dir_state("d/e"),
            state("d/e/y.txt"),
            state("u.txt"),
        ],
        vec![
            local("d").dir(),
            local("d/x.txt"),
            local("d/e").dir(),
            local("d/e/y.txt"),
            local("u.txt").size(150).mtime(T1).hash("h2"),
        ],
        vec![remote("u.txt"), remote("n").dir(), remote("n/f.txt")],
    );
    assert_eq!(
        vec![
            "MkdirLocal",
            "Upload",
            "Download",
            "DeleteLocal",
            "DeleteLocal",
            "DeleteLocal",
            "DeleteLocal"
        ],
        r.kinds()
    );
    assert_eq!("n", r.actions()[0].key());
    assert_eq!("u.txt", r.actions()[1].key());
    assert_eq!("n/f.txt", r.actions()[2].key());
    let deletes = keys(&r.all("DeleteLocal"));
    assert_eq!(vec!["d/e/y.txt", "d/e", "d/x.txt", "d"], deletes);
    let depths: Vec<usize> = deletes.iter().map(|k| k.matches('/').count()).collect();
    let mut sorted = depths.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(sorted, depths, "deepest first");
}

// ================================================================ summary

#[test]
fn summary_counts_every_category() {
    let r = plan(
        limited(1000),
        vec![
            state("u2.txt").remote_id("r-u2"),
            state("dl-gone.txt"),
            state("dr-gone.txt"),
            state("c.txt"),
        ],
        vec![
            local("u1.txt").size(100),                      // new -> upload
            local("u2.txt").size(250).mtime(T1).hash("h2"), // changed -> upload
            local("dl-gone.txt"),                           // remote deleted -> delete local
            local("c.txt").size(120).mtime(T1).hash("h2"),  // both changed -> conflict
            local("big.bin").size(5000),                    // above limit -> skip
            local("ld").dir(),                              // new folder -> mkdir remote
        ],
        vec![
            remote("u2.txt").id("r-u2"),
            remote("dl.txt").size(Some(400)), // new -> download
            doc("doc"),                       // new Google Doc -> export
            remote("dr-gone.txt"),            // local deleted -> delete remote
            remote("c.txt").md5(Some("h3")).size(Some(130)),
            remote("nd").dir(), // new folder -> mkdir local
        ],
    );
    let s = r.summary();
    assert_eq!(2, s.uploads);
    assert_eq!(350, s.upload_bytes);
    assert_eq!(1, s.downloads);
    assert_eq!(400, s.download_bytes);
    assert_eq!(1, s.exports);
    assert_eq!(1, s.delete_local);
    assert_eq!(1, s.delete_remote);
    assert_eq!(1, s.conflicts);
    assert_eq!(2, s.mkdirs);
    assert_eq!(1, s.skips);
    assert_eq!(5, s.local_files);
    assert_eq!(5, s.remote_files);
    assert!(s.warnings.is_empty());
    assert_eq!(None, s.blocked_reason);
    assert!(!r.plan.blocked);
    assert!(!s.is_empty());
    assert_eq!(
        vec!["device: dl-gone.txt", "Drive: dr-gone.txt"],
        s.sample_deletes
    );
    assert_eq!(10, r.actions().len());
}

#[test]
fn summary_is_empty_when_only_bookkeeping_is_planned() {
    let r = plan(
        cfg(),
        vec![state("gone.txt")],
        vec![local("same.txt").hash("h1")],
        vec![remote("same.txt").md5(Some("h1"))],
    );
    let mut kinds = r.kinds();
    kinds.sort();
    kinds.dedup();
    assert_eq!(vec!["Adopt", "Forget"], kinds);
    assert!(r.summary().is_empty());
}

// ================================================================ name mapping

#[test]
fn mapped_local_rel_maps_every_segment_of_a_new_remote_path() {
    let r = plan(
        cfg(),
        vec![],
        vec![],
        vec![remote("x?").dir(), remote("x?/a?b.txt")],
    );
    let mk = r.single("MkdirLocal");
    assert_eq!("x?", mk.key());
    assert_eq!("x？", mk.local_rel());
    let d = r.single("Download");
    assert_eq!("x?/a?b.txt", d.key());
    assert_eq!("x？/a？b.txt", d.local_rel());
}

#[test]
fn mapped_local_rel_reuses_the_known_mapping_of_the_parent() {
    let r = plan(
        cfg(),
        vec![dir_state("x?").local_rel("custom")],
        vec![local("x?").rel("custom").dir()],
        vec![remote("x?").dir(), remote("x?/a?b.txt")],
    );
    assert_eq!("custom/a？b.txt", r.single("Download").local_rel());
}

#[test]
fn mapped_local_rel_prefers_the_baseline_name_for_known_keys() {
    let r = plan(
        cfg(),
        vec![state("a?b.txt").local_rel("a？b.txt")],
        vec![],
        vec![remote("a?b.txt").md5(Some("h2"))],
    );
    assert_eq!("a？b.txt", r.single("Download").local_rel());
}

#[test]
fn mapped_local_rel_companion_helper() {
    let input = PlanInput {
        baseline: [("p".to_owned(), dir_state("p").local_rel("P-mapped"))].into(),
        local_rel_by_key: [("q?".to_owned(), "q-from-scan".to_owned())].into(),
        ..PlanInput::default()
    };
    assert_eq!("a？b.txt", mapped_local_rel("a?b.txt", &input));
    assert_eq!("x？/a？b.txt", mapped_local_rel("x?/a?b.txt", &input));
    assert_eq!("q-from-scan/c：d", mapped_local_rel("q?/c:d", &input));
    assert_eq!("P-mapped/e＊", mapped_local_rel("p/e*", &input));
    assert_eq!("plain/name.txt", mapped_local_rel("plain/name.txt", &input));
}

#[test]
fn export_local_rel_companion_helper() {
    let docx = export_target(GOOGLE_DOC, false).unwrap();
    assert_eq!("docs/notes.docx", export_local_rel("docs", "notes", docx));
    assert_eq!("docs/notes.docx", export_local_rel("docs", "notes.docx", docx));
    assert_eq!("docs/notes.DOCX", export_local_rel("docs", "notes.DOCX", docx));
    assert_eq!("a？b.docx", export_local_rel("", "a?b", docx));
    assert_eq!(
        "notes.pdf",
        export_local_rel("", "notes", export_target(GOOGLE_DOC, true).unwrap())
    );
}

#[test]
fn path_helpers() {
    assert_eq!("a/b", parent_of("a/b/c"));
    assert_eq!("", parent_of("a"));
    assert_eq!("c", name_of("a/b/c"));
    assert_eq!("a", name_of("a"));
    assert_eq!(0, wasl_core::guards::pct(3, 0));
    assert_eq!(33, wasl_core::guards::pct(1, 3));
    assert_eq!(100, wasl_core::guards::pct(4, 4));
}

#[test]
fn a_distinct_export_name_says_what_kind_of_google_file_it_is_and_counts_up() {
    let docx = export_target(GOOGLE_DOC, false).unwrap();
    let xlsx = export_target(GOOGLE_SHEET, false).unwrap();
    assert_eq!(
        "Report (Google Doc).docx",
        export_distinct_name("Report", GOOGLE_DOC, docx, 1)
    );
    assert_eq!(
        "Report (Google Doc 2).docx",
        export_distinct_name("Report", GOOGLE_DOC, docx, 2)
    );
    assert_eq!(
        "Report (Google Doc).docx",
        export_distinct_name("Report.docx", GOOGLE_DOC, docx, 1),
        "an existing extension is not doubled"
    );
    assert_eq!(
        "Budget (Google Sheet).xlsx",
        export_distinct_name("Budget", GOOGLE_SHEET, xlsx, 1)
    );
    assert!(is_export_distinct_name(
        "Report (Google Doc).docx",
        "Report",
        GOOGLE_DOC,
        docx
    ));
    assert!(is_export_distinct_name(
        "Report (Google Doc 12).docx",
        "Report",
        GOOGLE_DOC,
        docx
    ));
    assert!(!is_export_distinct_name(
        "Report (Google Doc 0).docx",
        "Report",
        GOOGLE_DOC,
        docx
    ));
    assert!(!is_export_distinct_name(
        "Report (Google Doc x).docx",
        "Report",
        GOOGLE_DOC,
        docx
    ));
    assert!(!is_export_distinct_name(
        "Report.docx",
        "Report",
        GOOGLE_DOC,
        docx
    ));
    assert!(!is_export_distinct_name(
        "Other (Google Doc).docx",
        "Report",
        GOOGLE_DOC,
        docx
    ));
}
