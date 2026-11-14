//! Three-way merge between the baseline, the local tree and the Drive tree (SRS FR-E2).
//!
//! The planner is a pure function. 1.1.4 hashed files lazily through a callback while planning;
//! here hashing is the caller's job: [`Planner::plan`] either returns a plan or the exact set of
//! local files whose hash it needs (the same files 1.1.4 would have hashed), and the caller plans
//! again with those hashes. [`Planner::try_plan_with`] runs that loop with a caller-supplied hasher.

use crate::docs::{export_distinct_name, export_local_name, export_target, is_export_distinct_name};
use crate::guards::{self, GuardConfig, GuardInput};
use crate::identity::name_key;
use crate::model::{
    Action, BaselineEntry, ConflictPolicy, DocsFormat, LocalEntry, PlanInput, PlanSummary, RemoteEntry,
    SyncMode, SyncPlan,
};
use crate::names::to_local_name;
use crate::paths::{depth, name_of, parent_of};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Content hashes of local files by local relative path (MD5 hex on Drive pairs).
pub type Hashes = BTreeMap<String, String>;

/// Why the planner leaves a file out for its size; `wasl ignore --explain` says the same.
pub const SIZE_LIMIT_SKIP: &str = "larger than the pair's size limit";

/// Whether the pair's size limit (`0`: none) keeps a local file of `size` bytes out. As in 1.1.4,
/// only a file that was never synced is skipped. The planner and `wasl ignore --explain` both ask.
pub fn over_size_limit(limit: u64, size: u64, synced_before: bool) -> bool {
    limit > 0 && size > limit && !synced_before
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannerConfig {
    pub mode: SyncMode,
    pub conflict_policy: ConflictPolicy,
    pub propagate_deletions: bool,
    /// Files above this size are not synced if they were never synced before; 0 means no limit.
    pub max_file_size: u64,
    pub docs_export: bool,
    pub docs_format: DocsFormat,
    pub guards: GuardConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PlanOutcome {
    Ready(SyncPlan),
    /// Hash these local files (by local relative path) and plan again.
    NeedHashes(BTreeSet<String>),
}

pub struct Planner {
    cfg: PlannerConfig,
}

impl Planner {
    pub fn new(cfg: PlannerConfig) -> Planner {
        Planner { cfg }
    }

    pub fn config(&self) -> &PlannerConfig {
        &self.cfg
    }

    /// Plans with the hashes known so far.
    pub fn plan(&self, input: &PlanInput, hashes: &Hashes) -> PlanOutcome {
        let mut ctx = Ctx::new(&self.cfg, input, hashes);
        let mut keys: Vec<&str> = input
            .baseline
            .keys()
            .chain(input.local.keys())
            .chain(input.remote.keys())
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        // parity: 1.1.4 sorted by depth, then by UTF-16 order; byte order differs only for
        // characters above U+FFFF against U+E000..U+FFFF, which only reorders independent actions.
        keys.sort_by(|a, b| depth(a).cmp(&depth(b)).then_with(|| a.cmp(b)));
        for key in &keys {
            // A key that needs a hash is left out of this pass; nothing was recorded for it yet.
            let _ = ctx.plan_key(key);
        }
        if !ctx.missing.is_empty() {
            return PlanOutcome::NeedHashes(ctx.missing);
        }
        PlanOutcome::Ready(ctx.finish(&keys))
    }

    /// Plans, asking `hasher` for each hash the planner needs. Each file is hashed at most once.
    pub fn try_plan_with<E>(
        &self,
        input: &PlanInput,
        mut hasher: impl FnMut(&LocalEntry) -> Result<String, E>,
    ) -> Result<SyncPlan, E> {
        let by_rel: BTreeMap<&str, &LocalEntry> = input.local.values().map(|l| (l.rel.as_str(), l)).collect();
        let mut hashes = Hashes::new();
        loop {
            match self.plan(input, &hashes) {
                PlanOutcome::Ready(plan) => return Ok(plan),
                PlanOutcome::NeedHashes(rels) => {
                    for rel in rels {
                        let entry = by_rel
                            .get(rel.as_str())
                            .expect("the planner only asks for scanned files");
                        let hash = hasher(entry)?;
                        hashes.insert(rel, hash);
                    }
                }
            }
        }
    }

    /// [`Planner::try_plan_with`] for hashers that cannot fail.
    pub fn plan_with(&self, input: &PlanInput, mut hasher: impl FnMut(&LocalEntry) -> String) -> SyncPlan {
        match self.try_plan_with(input, |l| Ok::<_, std::convert::Infallible>(hasher(l))) {
            Ok(plan) => plan,
        }
    }
}

/// Maps every path segment of `key` to a local name, reusing already-mapped parents when known.
pub fn mapped_local_rel(key: &str, input: &PlanInput) -> String {
    let parent = parent_of(key);
    let parent_local = if parent.is_empty() {
        String::new()
    } else {
        input
            .local_rel_by_key
            .get(parent)
            .or_else(|| input.baseline.get(parent).map(|b| &b.local_rel))
            .cloned()
            .unwrap_or_else(|| mapped_local_rel(parent, input))
    };
    join(&parent_local, &to_local_name(name_of(key)))
}

/// Export mirror path: derived from the Drive name, so switching formats yields `notes.pdf`, not
/// `notes.docx.pdf`.
pub fn export_local_rel(
    parent_local_rel: &str,
    remote_name: &str,
    target: crate::docs::ExportTarget,
) -> String {
    join(
        parent_local_rel,
        &export_local_name(&to_local_name(remote_name), target),
    )
}

/// [`export_local_rel`] for a Google file whose plain export name is another item's local name:
/// `Report (Google Doc).docx` next to a real `Report.docx`, then `Report (Google Doc 2).docx`
/// (#139).
pub fn export_distinct_local_rel(
    parent_local_rel: &str,
    remote_name: &str,
    google_mime: &str,
    target: crate::docs::ExportTarget,
    n: u32,
) -> String {
    join(
        parent_local_rel,
        &export_distinct_name(&to_local_name(remote_name), google_mime, target, n),
    )
}

fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Unchanged,
    Changed,
    New,
    Deleted,
    Absent,
}

/// The planner needs a hash it doesn't have yet.
struct NeedHash;

#[derive(Default)]
struct Counts {
    uploads: u32,
    upload_bytes: u64,
    downloads: u32,
    download_bytes: u64,
    exports: u32,
    delete_local: u32,
    delete_remote: u32,
    conflicts: u32,
    mkdirs: u32,
    skips: u32,
}

const MAX_DELETE_SAMPLES: usize = 10;

struct Ctx<'a> {
    cfg: &'a PlannerConfig,
    input: &'a PlanInput,
    hashes: &'a Hashes,
    actions: Vec<Action>,
    warnings: Vec<String>,
    delete_samples: Vec<String>,
    n: Counts,
    missing: BTreeSet<String>,
    /// Local name under [`name_key`] -> the keys that hold it: items on Drive, on the device or in the
    /// baseline. Built on the first export.
    claims: Option<HashMap<String, Vec<&'a str>>>,
}

impl<'a> Ctx<'a> {
    fn new(cfg: &'a PlannerConfig, input: &'a PlanInput, hashes: &'a Hashes) -> Self {
        Ctx {
            cfg,
            input,
            hashes,
            actions: Vec::new(),
            warnings: Vec::new(),
            delete_samples: Vec::new(),
            n: Counts::default(),
            missing: BTreeSet::new(),
            claims: None,
        }
    }

    /// Whether an item on Drive, on the device or in the baseline other than `key` holds the local
    /// name `rel` (compared by [`name_key`], as on a case-insensitive device folder). A real file
    /// only on the device counts: it keeps its name and the export takes another (#139).
    fn claimed_by_other(&mut self, key: &str, rel: &str) -> bool {
        let input = self.input;
        let claims = self.claims.get_or_insert_with(|| {
            let mut m: HashMap<String, Vec<&'a str>> = HashMap::new();
            let rels = input
                .remote
                .keys()
                .filter_map(|k| input.local_rel_by_key.get(k).map(|r| (k, r)))
                .chain(input.local.iter().map(|(k, l)| (k, &l.rel)))
                .chain(input.baseline.iter().map(|(k, b)| (k, &b.local_rel)));
            for (k, r) in rels {
                m.entry(name_key(r)).or_default().push(k.as_str());
            }
            m
        });
        claims
            .get(&name_key(rel))
            .is_some_and(|keys| keys.iter().any(|k| *k != key))
    }

    /// The local name of a Google file's export: its plain name unless another item holds it, else
    /// the first free distinct name. An export already under a distinct name keeps it.
    fn pick_export_rel(
        &mut self,
        key: &str,
        base: Option<&BaselineEntry>,
        parent: &str,
        google_mime: &str,
        target: crate::docs::ExportTarget,
    ) -> String {
        let name = name_of(key);
        if let Some(b) = base
            && b.export_mime.is_some()
            && parent_of(&b.local_rel) == parent
            && is_export_distinct_name(name_of(&b.local_rel), &to_local_name(name), google_mime, target)
            && !self.claimed_by_other(key, &b.local_rel)
        {
            return b.local_rel.clone();
        }
        let plain = export_local_rel(parent, name, target);
        if !self.claimed_by_other(key, &plain) {
            return plain;
        }
        let mut n = 1;
        loop {
            let rel = export_distinct_local_rel(parent, name, google_mime, target, n);
            if !self.claimed_by_other(key, &rel) {
                return rel;
            }
            n += 1;
        }
    }

    /// A pair from 0.4.x where this export already overwrote a real file: the real file's row
    /// holds the export's name and bytes. Nothing here can tell what Drive kept, so the run says it.
    fn note_export_under_a_real_row(&mut self, key: &str, base: Option<&BaselineEntry>) {
        let Some(b) = base else { return };
        let Some(hash) = b.local_hash.as_deref() else {
            return;
        };
        let rel = name_key(&b.local_rel);
        let hit = self.input.baseline.iter().find(|(k, o)| {
            k.as_str() != key
                && o.export_mime.is_none()
                && !o.is_dir
                && name_key(&o.local_rel) == rel
                && o.local_hash.as_deref() == Some(hash)
        });
        if let Some((_, o)) = hit {
            self.warnings.push(format!(
                "'{}' may hold an export of the Google file '{key}' that an earlier version of Wasl wrote over it. If it isn't the real file, restore it from Drive's version history (Drive keeps 30 days).",
                o.local_rel
            ));
        }
    }

    fn hash_of(&mut self, l: &LocalEntry) -> Result<&'a str, NeedHash> {
        match self.hashes.get(&l.rel) {
            Some(h) => Ok(h),
            None => {
                self.missing.insert(l.rel.clone());
                Err(NeedHash)
            }
        }
    }

    /// True when the local file has the same content as the Drive file (MD5, case-insensitive).
    fn same_content(&mut self, l: Option<&LocalEntry>, r: Option<&RemoteEntry>) -> Result<bool, NeedHash> {
        match (l, r) {
            // Different sizes are different content: no need to read the file.
            (
                Some(l),
                Some(RemoteEntry {
                    size: Some(size),
                    md5: Some(_),
                    ..
                }),
            ) if *size != l.size => Ok(false),
            (Some(l), Some(RemoteEntry { md5: Some(md5), .. })) => {
                Ok(self.hash_of(l)?.eq_ignore_ascii_case(md5))
            }
            _ => Ok(false),
        }
    }

    /// True when Drive has a file under `dir` that is new or changed since the last sync.
    fn drive_changed_under(&self, dir: &str) -> bool {
        let prefix = format!("{dir}/");
        self.input
            .remote
            .range(prefix.clone()..)
            .take_while(|(k, _)| k.starts_with(&prefix))
            .any(|(k, r)| {
                if r.is_dir {
                    return false;
                }
                match self.input.baseline.get(k) {
                    None => true,
                    Some(b) => !remote_unchanged_by_identity(r, b),
                }
            })
    }

    /// The device side of [`Ctx::drive_changed_under`]: a file under `dir` is new or changed here, or
    /// a folder under it is new here.
    fn device_changed_under(&mut self, dir: &str) -> Result<bool, NeedHash> {
        let prefix = format!("{dir}/");
        let input = self.input;
        for (k, l) in input
            .local
            .range(prefix.clone()..)
            .take_while(|(k, _)| k.starts_with(&prefix))
        {
            let changed = match input.baseline.get(k) {
                None => true,
                Some(_) if l.is_dir => false,
                Some(b) if l.size == b.local_size && l.mtime == b.local_mtime => false,
                Some(b) => Some(self.hash_of(l)?) != b.local_hash.as_deref(),
            };
            if changed {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn sample_delete(&mut self, what: String) {
        if self.delete_samples.len() < MAX_DELETE_SAMPLES {
            self.delete_samples.push(what);
        }
    }

    fn upload(&mut self, key: &str, l: &LocalEntry, r: Option<&RemoteEntry>, reason: &str) {
        self.n.uploads += 1;
        self.n.upload_bytes += l.size;
        self.actions.push(Action::Upload {
            key: key.to_owned(),
            local: l.clone(),
            existing_remote_id: r.map(|r| r.id.clone()),
            parent_key: parent_of(key).to_owned(),
            name: name_of(key).to_owned(),
            reason: reason.to_owned(),
        });
    }

    fn download(
        &mut self,
        key: &str,
        r: &RemoteEntry,
        l: Option<&LocalEntry>,
        local_rel: &str,
        reason: &str,
    ) {
        self.n.downloads += 1;
        self.n.download_bytes += r.size.unwrap_or(0);
        self.actions.push(Action::Download {
            key: key.to_owned(),
            remote: r.clone(),
            local_rel: local_rel.to_owned(),
            overwrites: l.is_some(),
            reason: reason.to_owned(),
        });
    }

    fn delete_local(&mut self, key: &str, local_rel: &str, is_dir: bool, reason: &str) {
        self.n.delete_local += 1;
        if !is_dir {
            self.sample_delete(format!("device: {local_rel}"));
        }
        self.actions.push(Action::DeleteLocal {
            key: key.to_owned(),
            local_rel: local_rel.to_owned(),
            is_dir,
            reason: reason.to_owned(),
        });
    }

    fn delete_remote(&mut self, key: &str, r: &RemoteEntry, is_dir: bool, reason: &str) {
        self.n.delete_remote += 1;
        if !is_dir {
            self.sample_delete(format!("Drive: {key}"));
        }
        self.actions.push(Action::DeleteRemote {
            key: key.to_owned(),
            remote_id: r.id.clone(),
            is_dir,
            reason: reason.to_owned(),
        });
    }

    fn mkdir_local(&mut self, key: &str, local_rel: &str) {
        self.n.mkdirs += 1;
        self.actions.push(Action::MkdirLocal {
            key: key.to_owned(),
            local_rel: local_rel.to_owned(),
        });
    }

    fn mkdir_remote(&mut self, key: &str) {
        self.n.mkdirs += 1;
        self.actions.push(Action::MkdirRemote {
            key: key.to_owned(),
            parent_key: parent_of(key).to_owned(),
            name: name_of(key).to_owned(),
        });
    }

    fn skip(&mut self, key: &str, reason: impl Into<String>, warn: bool) {
        self.n.skips += 1;
        self.actions.push(Action::Skip {
            key: key.to_owned(),
            reason: reason.into(),
            warn,
        });
    }

    fn conflict_action(&mut self, key: &str, l: &LocalEntry, r: &RemoteEntry, local_rel: &str) {
        self.n.conflicts += 1;
        self.actions.push(Action::Conflict {
            key: key.to_owned(),
            local: l.clone(),
            remote: r.clone(),
            local_rel: local_rel.to_owned(),
            parent_key: parent_of(key).to_owned(),
        });
    }

    /// Adopts when both sides exist and have the same content; returns whether it did.
    fn adopt_if_same(
        &mut self,
        key: &str,
        l: Option<&LocalEntry>,
        r: Option<&RemoteEntry>,
        local_rel: &str,
    ) -> Result<bool, NeedHash> {
        match (l, r) {
            (Some(l), Some(r)) if self.same_content(Some(l), Some(r))? => {
                self.adopt(key, l, r, local_rel);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn adopt(&mut self, key: &str, l: &LocalEntry, r: &RemoteEntry, local_rel: &str) {
        self.actions.push(Action::Adopt {
            key: key.to_owned(),
            local: l.clone(),
            remote: r.clone(),
            local_rel: local_rel.to_owned(),
        });
    }

    fn forget(&mut self, key: &str) {
        self.actions.push(Action::Forget { key: key.to_owned() });
    }

    fn refresh(&mut self, key: &str, l: Option<&LocalEntry>, r: Option<&RemoteEntry>, local_rel: &str) {
        self.actions.push(Action::RefreshState {
            key: key.to_owned(),
            local: l.cloned(),
            remote: r.cloned(),
            local_rel: local_rel.to_owned(),
            export_mime: None,
        });
    }

    fn plan_key(&mut self, key: &str) -> Result<(), NeedHash> {
        let input = self.input;
        let base = input.baseline.get(key);
        let l = input.local.get(key);
        let r = input.remote.get(key);
        let local_rel = input
            .local_rel_by_key
            .get(key)
            .or(base.map(|b| &b.local_rel))
            .cloned()
            .unwrap_or_else(|| mapped_local_rel(key, input));

        if l.is_some_and(|l| l.is_dir) || r.is_some_and(|r| r.is_dir) || base.is_some_and(|b| b.is_dir) {
            return self.plan_dir(key, base, l, r, &local_rel);
        }
        if r.is_some_and(|r| r.google_mime.is_some()) || base.is_some_and(|b| b.export_mime.is_some()) {
            self.plan_export(key, base, l, r, &local_rel);
            return Ok(());
        }
        self.plan_file(key, base, l, r, &local_rel)
    }

    fn plan_dir(
        &mut self,
        key: &str,
        base: Option<&BaselineEntry>,
        l: Option<&LocalEntry>,
        r: Option<&RemoteEntry>,
        local_rel: &str,
    ) -> Result<(), NeedHash> {
        let mode = self.cfg.mode;
        let propagate = self.cfg.propagate_deletions;
        let base_dir = base.is_some_and(|b| b.is_dir);
        match (l, r) {
            (Some(l), Some(r)) if l.is_dir && r.is_dir => {
                if !base_dir {
                    self.refresh(key, Some(l), Some(r), local_rel);
                }
            }
            (Some(l), None)
                if l.is_dir
                    && base_dir
                    && mode == SyncMode::TwoWay
                    && propagate
                    && self.device_changed_under(key)? =>
            {
                // Removing the folder here would leave the device's new files with nowhere to go on
                // Drive, run after run (#142). The folder is made again on Drive for them; the synced
                // files in it are deleted one by one.
                self.mkdir_remote(key);
            }
            (Some(l), None) if l.is_dir => {
                if base_dir
                    && mode.downloads()
                    && propagate
                    && matches!(mode, SyncMode::TwoWay | SyncMode::DownloadMirror)
                {
                    self.delete_local(key, local_rel, true, "folder removed on Drive");
                } else if mode == SyncMode::DownloadMirror {
                    self.delete_local(key, local_rel, true, "mirror: folder not on Drive");
                } else if mode.uploads() {
                    if base.is_some() && mode == SyncMode::TwoWay && propagate {
                        // parity: reached when the baseline row was a file; 1.1.4 plans a folder
                        // deletion here (the executor only removes empty folders).
                        self.delete_local(key, local_rel, true, "folder removed on Drive");
                    } else {
                        self.mkdir_remote(key);
                    }
                }
            }
            (None, Some(r)) if r.is_dir => {
                if base_dir && mode == SyncMode::TwoWay && propagate && self.drive_changed_under(key) {
                    // Trashing the folder would take Drive's new files with it (after downloading them, so
                    // they would then be deleted on the device too). The synced files in it are deleted
                    // one by one; the folder stays.
                    self.skip(key, "kept on Drive: it has new or changed files", true);
                } else if base_dir
                    && mode.uploads()
                    && propagate
                    && matches!(mode, SyncMode::TwoWay | SyncMode::UploadMirror)
                {
                    self.delete_remote(key, r, true, "folder removed on device");
                } else if mode == SyncMode::UploadMirror {
                    self.delete_remote(key, r, true, "mirror: folder not on device");
                } else if mode.downloads() {
                    self.mkdir_local(key, local_rel);
                }
            }
            (None, None) => self.forget(key),
            // A file on one side and a folder on the other: keep both by renaming the local one.
            (Some(l), Some(r)) => self.conflict_action(key, l, r, local_rel),
            // parity: a baseline folder that is now a file on one side and gone on the other is left
            // alone, as in 1.1.4.
            _ => {}
        }
        Ok(())
    }

    fn plan_export(
        &mut self,
        key: &str,
        base: Option<&BaselineEntry>,
        l: Option<&LocalEntry>,
        r: Option<&RemoteEntry>,
        local_rel: &str,
    ) {
        if let Some(r) = r
            && r.google_mime.is_none()
        {
            // A regular file now sits where a Google Doc used to be: drop the export mirror; the file
            // syncs normally on the next run.
            if l.is_some() && base.is_some() {
                self.delete_local(
                    key,
                    local_rel,
                    false,
                    "Google Doc replaced by a regular file on Drive",
                );
            } else {
                self.forget(key);
            }
            return;
        }
        if !self.cfg.mode.downloads() || !self.cfg.docs_export {
            if r.is_some() {
                self.skip(
                    key,
                    "Google Docs file ignored (export disabled or upload mode)",
                    false,
                );
            }
            return;
        }
        let Some(r) = r else {
            // The doc was removed on Drive: remove the local export too (it is never the source of truth).
            if l.is_some() && base.is_some() {
                self.delete_local(key, local_rel, false, "exported Google Doc removed on Drive");
            } else {
                self.forget(key);
            }
            return;
        };
        let google_mime = r.google_mime.as_deref().unwrap_or_default();
        let Some(target) = export_target(google_mime, self.cfg.docs_format == DocsFormat::Pdf) else {
            self.skip(key, format!("no export format for {google_mime}"), false);
            return;
        };
        // A Google file never takes another item's local name: the export would overwrite that
        // file here and then read as its edit and be uploaded over it on Drive (#139).
        self.note_export_under_a_real_row(key, base);
        let export_rel = self.pick_export_rel(key, base, parent_of(local_rel), google_mime, target);
        let drifted = match (l, base) {
            (Some(l), Some(b)) => l.size != b.local_size || l.mtime != b.local_mtime,
            _ => false,
        };
        let changed = match base {
            None => true,
            Some(b) => {
                b.remote_version != r.version
                    || b.remote_mtime != Some(r.mtime)
                    || l.is_none()
                    || b.local_rel != export_rel
            }
        };
        if drifted {
            self.warnings.push(format!(
                "Local copy of exported doc '{export_rel}' was modified on the device; the edited copy goes to the recycle bin and the Drive version is exported again (Google Docs cannot be uploaded back)."
            ));
        }
        if changed || drifted {
            self.n.exports += 1;
            self.actions.push(Action::Export {
                key: key.to_owned(),
                remote: r.clone(),
                local_rel: export_rel,
                mime: target.mime.to_owned(),
                reason: if drifted {
                    "restoring Google Doc export (local copy modified)"
                } else {
                    "Google Doc changed on Drive"
                }
                .to_owned(),
            });
        }
    }

    fn plan_file(
        &mut self,
        key: &str,
        base: Option<&BaselineEntry>,
        l: Option<&LocalEntry>,
        r: Option<&RemoteEntry>,
        local_rel: &str,
    ) -> Result<(), NeedHash> {
        let cfg = self.cfg;
        let limit = cfg.max_file_size;
        if limit > 0 {
            // A Drive file is skipped only when there is no local copy either.
            let synced = base.is_some_and(|b| b.local_hash.is_some());
            let local_too_big = l.is_some_and(|l| over_size_limit(limit, l.size, synced));
            let remote_too_big =
                l.is_none() && r.is_some_and(|r| r.size.unwrap_or(0) > limit) && base.is_none();
            if local_too_big || remote_too_big {
                self.skip(key, SIZE_LIMIT_SKIP, false);
                return Ok(());
            }
        }

        let local_side = match (l, base) {
            (None, None) => Side::Absent,
            (None, Some(_)) => Side::Deleted,
            (Some(_), None) => Side::New,
            (Some(l), Some(b)) if l.size == b.local_size && l.mtime == b.local_mtime => Side::Unchanged,
            (Some(l), Some(b)) => {
                if Some(self.hash_of(l)?) == b.local_hash.as_deref() {
                    Side::Unchanged
                } else {
                    Side::Changed
                }
            }
        };
        let remote_side = match (r, base) {
            (None, None) => Side::Absent,
            (None, Some(_)) => Side::Deleted,
            (Some(_), None) => Side::New,
            (Some(r), Some(b)) => {
                if remote_unchanged_by_identity(r, b) || (r.md5.is_some() && r.md5 == b.remote_md5) {
                    Side::Unchanged
                } else {
                    Side::Changed
                }
            }
        };

        // Same content under a new size or time (a git checkout, a copy back): record the new stat
        // once, or the file is hashed again on every run.
        let stat_drifted = local_side == Side::Unchanged
            && remote_side == Side::Unchanged
            && matches!((l, base), (Some(l), Some(b)) if l.size != b.local_size || l.mtime != b.local_mtime);
        // An unchanged file whose row has no MD5 yet but whose listing has one: record it, so the
        // MD5 decides from now on and a version Drive raises by itself is not read as a change.
        let md5_learned = local_side == Side::Unchanged
            && remote_side == Side::Unchanged
            && matches!((r, base), (Some(r), Some(b)) if r.md5.is_some() && b.remote_md5.is_none());
        let planned = self.actions.len();
        let result = self.plan_by_mode(key, base, l, r, local_rel, local_side, remote_side);
        if result.is_ok() && (stat_drifted || md5_learned) && self.actions.len() == planned {
            self.refresh(key, l, r, local_rel);
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn plan_by_mode(
        &mut self,
        key: &str,
        base: Option<&BaselineEntry>,
        l: Option<&LocalEntry>,
        r: Option<&RemoteEntry>,
        local_rel: &str,
        local_side: Side,
        remote_side: Side,
    ) -> Result<(), NeedHash> {
        let cfg = self.cfg;
        // Each arm below only runs when its `Option`s are known to be `Some` by the side states;
        // `unwrap`s mirror 1.1.4's `!!` and are guaranteed by the state table.
        use Side::*;
        match cfg.mode {
            SyncMode::TwoWay => match local_side {
                Absent => match remote_side {
                    New | Changed | Unchanged => self.download(key, r.unwrap(), l, local_rel, "new on Drive"),
                    _ => self.forget(key),
                },
                New => match remote_side {
                    Absent | Deleted => self.upload(key, l.unwrap(), r, "new on device"),
                    _ => self.adopt_or_conflict(key, l.unwrap(), r.unwrap(), local_rel)?,
                },
                Unchanged => match remote_side {
                    Unchanged => {
                        if base.is_some_and(|b| b.local_rel != local_rel) {
                            self.refresh(key, l, r, local_rel);
                        }
                    }
                    Changed | New => {
                        if !self.adopt_if_same(key, l, r, local_rel)? {
                            self.download(key, r.unwrap(), l, local_rel, "changed on Drive");
                        }
                    }
                    Deleted => {
                        if cfg.propagate_deletions {
                            self.delete_local(key, local_rel, false, "deleted on Drive");
                        } else {
                            self.upload(
                                key,
                                l.unwrap(),
                                r,
                                "re-uploading: deleted on Drive but deletions are not propagated",
                            );
                        }
                    }
                    Absent => self.upload(key, l.unwrap(), r, "missing on Drive"),
                },
                Changed => match remote_side {
                    Unchanged | Absent | Deleted => self.upload(key, l.unwrap(), r, "changed on device"),
                    Changed | New => self.adopt_or_conflict(key, l.unwrap(), r.unwrap(), local_rel)?,
                },
                Deleted => match remote_side {
                    Unchanged => {
                        if cfg.propagate_deletions {
                            self.delete_remote(key, r.unwrap(), false, "deleted on device");
                        } else {
                            self.download(
                                key,
                                r.unwrap(),
                                l,
                                local_rel,
                                "restoring: deleted on device but deletions are not propagated",
                            );
                        }
                    }
                    Changed | New => self.download(
                        key,
                        r.unwrap(),
                        l,
                        local_rel,
                        "changed on Drive after local delete",
                    ),
                    Deleted | Absent => self.forget(key),
                },
            },
            SyncMode::UploadOnly | SyncMode::UploadThenDelete => match local_side {
                New | Changed => {
                    if !self.adopt_if_same(key, l, r, local_rel)? {
                        self.upload(key, l.unwrap(), r, "new or changed on device");
                    }
                }
                Unchanged => {
                    if r.is_none() && base.is_some_and(|b| b.remote_id.is_some()) {
                        // Already uploaded once; Drive deleted it on purpose.
                        self.skip(
                            key,
                            "already uploaded once; not re-uploading after Drive deletion",
                            false,
                        );
                    } else if r.is_some() && remote_side == Changed {
                        self.refresh(key, l, r, local_rel);
                    }
                }
                Deleted => self.forget(key),
                Absent => {
                    if r.is_none() {
                        self.forget(key);
                    }
                }
            },
            SyncMode::UploadMirror => match local_side {
                New | Changed => {
                    if !self.adopt_if_same(key, l, r, local_rel)? {
                        self.upload(key, l.unwrap(), r, "mirror: device version");
                    }
                }
                Unchanged => match remote_side {
                    Changed | New => {
                        if !self.adopt_if_same(key, l, r, local_rel)? {
                            self.upload(key, l.unwrap(), r, "mirror: overwriting Drive change");
                        }
                    }
                    Deleted | Absent => self.upload(key, l.unwrap(), r, "mirror: missing on Drive"),
                    Unchanged => {}
                },
                Deleted | Absent => match r {
                    Some(r) => self.delete_remote(key, r, false, "mirror: not on device"),
                    None => self.forget(key),
                },
            },
            SyncMode::DownloadOnly | SyncMode::DownloadThenDelete => match remote_side {
                New | Changed => {
                    if !self.adopt_if_same(key, l, r, local_rel)? {
                        self.download(key, r.unwrap(), l, local_rel, "new or changed on Drive");
                    }
                }
                Unchanged => {
                    if l.is_none() && base.is_some_and(|b| b.local_hash.is_some()) {
                        self.skip(key, "already downloaded once; device copy was removed", false);
                    } else if l.is_some() && local_side == Changed {
                        self.refresh(key, l, r, local_rel);
                    }
                }
                Deleted => self.forget(key),
                Absent => {
                    if l.is_none() {
                        self.forget(key);
                    }
                }
            },
            SyncMode::DownloadMirror => match remote_side {
                New | Changed => {
                    if !self.adopt_if_same(key, l, r, local_rel)? {
                        self.download(key, r.unwrap(), l, local_rel, "mirror: Drive version");
                    }
                }
                Unchanged => match local_side {
                    Changed | New => {
                        if !self.adopt_if_same(key, l, r, local_rel)? {
                            self.download(key, r.unwrap(), l, local_rel, "mirror: overwriting device change");
                        }
                    }
                    Deleted | Absent => {
                        self.download(key, r.unwrap(), l, local_rel, "mirror: missing on device")
                    }
                    Unchanged => {}
                },
                Deleted | Absent => {
                    if l.is_some() {
                        self.delete_local(key, local_rel, false, "mirror: not on Drive");
                    } else {
                        self.forget(key);
                    }
                }
            },
        }
        Ok(())
    }

    /// Both sides changed (or are new): identical content is adopted, anything else is a conflict
    /// resolved by the pair's policy.
    fn adopt_or_conflict(
        &mut self,
        key: &str,
        l: &LocalEntry,
        r: &RemoteEntry,
        local_rel: &str,
    ) -> Result<(), NeedHash> {
        if self.same_content(Some(l), Some(r))? {
            self.adopt(key, l, r, local_rel);
            return Ok(());
        }
        match self.cfg.conflict_policy {
            ConflictPolicy::KeepBoth => self.conflict_action(key, l, r, local_rel),
            ConflictPolicy::LocalWins => self.upload(key, l, Some(r), "conflict: device wins"),
            ConflictPolicy::RemoteWins => self.download(key, r, Some(l), local_rel, "conflict: Drive wins"),
            ConflictPolicy::NewerWins => {
                if l.mtime >= r.mtime {
                    self.upload(key, l, Some(r), "conflict: device copy is newer");
                } else {
                    self.download(key, r, Some(l), local_rel, "conflict: Drive copy is newer");
                }
            }
            ConflictPolicy::Skip => self.skip(key, "modified on both sides", true),
        }
        Ok(())
    }

    fn finish(self, keys: &[&str]) -> SyncPlan {
        let Ctx {
            input,
            cfg,
            actions,
            warnings,
            delete_samples,
            n,
            ..
        } = self;

        let mut ordered: Vec<Action> = Vec::with_capacity(actions.len());
        let mut mkdirs: Vec<&Action> = actions.iter().filter(|a| a.is_mkdir()).collect();
        mkdirs.sort_by_key(|a| depth(a.key()));
        let mut deletes: Vec<&Action> = actions.iter().filter(|a| a.is_delete()).collect();
        deletes.sort_by_key(|a| std::cmp::Reverse(depth(a.key())));
        ordered.extend(mkdirs.into_iter().cloned());
        ordered.extend(
            actions
                .iter()
                .filter(|a| !a.is_mkdir() && !a.is_delete())
                .cloned(),
        );
        ordered.extend(deletes.into_iter().cloned());

        let count_files = |it: &mut dyn Iterator<Item = bool>| it.filter(|is_dir| !is_dir).count() as u32;
        let local_files = count_files(&mut input.local.values().map(|l| l.is_dir));
        let remote_files = count_files(&mut input.remote.values().map(|r| r.is_dir));
        let baseline_files = count_files(&mut input.baseline.values().map(|b| b.is_dir));
        let delete_local_files = actions
            .iter()
            .filter(|a| matches!(a, Action::DeleteLocal { is_dir: false, .. }))
            .count() as u32;
        let delete_remote_files = actions
            .iter()
            .filter(|a| matches!(a, Action::DeleteRemote { is_dir: false, .. }))
            .count() as u32;
        let block = guards::evaluate(
            &cfg.guards,
            &GuardInput {
                baseline_files,
                local_files,
                remote_files,
                delete_local_files,
                delete_remote_files,
            },
        );

        // Files that already match and will not be touched (the preview's headline number).
        let touched: HashSet<&str> = actions
            .iter()
            .filter(|a| {
                matches!(
                    a,
                    Action::Upload { .. }
                        | Action::Download { .. }
                        | Action::Export { .. }
                        | Action::DeleteLocal { .. }
                        | Action::DeleteRemote { .. }
                        | Action::Conflict { .. }
                        | Action::Skip { .. }
                )
            })
            .map(Action::key)
            .collect();
        let unchanged = keys
            .iter()
            .filter(|k| {
                !touched.contains(**k)
                    && (input.local.get(**k).is_some_and(|l| !l.is_dir)
                        || input.remote.get(**k).is_some_and(|r| !r.is_dir))
            })
            .count() as u32;

        let blocked = block.is_some();
        let summary = PlanSummary {
            uploads: n.uploads,
            upload_bytes: n.upload_bytes,
            downloads: n.downloads,
            download_bytes: n.download_bytes,
            exports: n.exports,
            delete_local: n.delete_local,
            delete_remote: n.delete_remote,
            conflicts: n.conflicts,
            mkdirs: n.mkdirs,
            skips: n.skips,
            local_files,
            remote_files,
            warnings,
            sample_deletes: delete_samples,
            unchanged,
            blocked_kind: block.as_ref().map(|b| b.kind),
            blocked_side: block.as_ref().and_then(|b| b.side),
            blocked_percent: block.as_ref().map_or(0, |b| b.percent),
            blocked_count: block.as_ref().map_or(0, |b| b.count),
            blocked_reason: block.map(|b| b.reason),
            moved_after_transfer: match cfg.mode {
                SyncMode::UploadThenDelete => n.uploads,
                SyncMode::DownloadThenDelete => n.downloads,
                _ => 0,
            },
        };
        SyncPlan {
            actions: ordered,
            summary,
            blocked,
        }
    }
}

/// Same Drive file and same content as the baseline: same id and MD5, or same version when the
/// baseline has no MD5. A row recorded without one (Drive reported none then, as vaults did
/// before 2026-11) keeps its version identity once the listing reports an MD5, so an upgrade
/// transfers nothing; `plan_file` then records the MD5.
pub(crate) fn remote_unchanged_by_identity(r: &RemoteEntry, b: &BaselineEntry) -> bool {
    Some(&r.id) == b.remote_id.as_ref()
        && match (&r.md5, &b.remote_md5) {
            (Some(now), Some(then)) => now == then,
            (None, Some(_)) => false,
            // A version Drive left out proves nothing.
            (_, None) => r.version.is_some() && r.version == b.remote_version,
        }
}
