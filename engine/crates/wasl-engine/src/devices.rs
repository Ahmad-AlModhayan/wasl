//! Sync between devices (docs/DEVICE_SYNC.md §4–6): turning the pair folder into versions,
//! pulling what a peer changed, and answering peers.
//!
//! A [`DeviceSync`] works on one pair. [`DeviceSync::scan`] hashes what changed on disk (BLAKE3,
//! and a chunk list per file) into this device's index. [`DeviceSync::pull`] reads a peer's
//! changes since the last pull, plans them with [`wasl_core::nway::plan_with`] and applies them:
//! a file is built from chunks already here where it can be (an edit costs its changed chunks;
//! a moved or copied file costs nothing) and from the peer for the rest, checked chunk by chunk
//! and as a whole, then installed in one rename. Nothing local is overwritten or deleted without
//! going through the bin, and a local file that changed since the last scan is left alone until
//! the next scan has recorded it. [`DeviceService`] answers peers from the same store.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use wasl_chunk::{ChunkList, ChunkLocation, ChunkParams, ChunkSource, Hash, plan_fetch};
use wasl_core::nway::{ConflictMode, Index, Scanned, Step, Version, plan_with};
use wasl_core::vv::ShortId;
use wasl_fs::LocalFs;
use wasl_ignore::IgnoreRules;
use wasl_net::{DeviceId, IndexPage, Offer, PeerError, PeerLink, PeerResult, PeerService, ShareId};
use wasl_store::{BinSide, NewBinEntry, Pair, SharedStore};

use crate::error::{EngineError, Result};
use crate::executor::BIN_RETENTION_MS;
use crate::support::{Clock, new_id};
use futures_util::StreamExt;

/// The chunking scheme a pair's stored chunk lists follow (`wasl_chunk::ChunkParams::for_size`);
/// a pair recorded under an older one is cut again once.
const CHUNK_TIER: &str = "2";

/// Chunks requested at once from a peer while building a file of at most 16 MiB (chunks up to
/// 256 KiB), and while building a larger one (chunks up to 16 MiB).
const FETCH_AHEAD_SMALL: usize = 8;
const FETCH_AHEAD_LARGE: usize = 3;

/// How long a deletion is remembered, so a peer that was away brings nothing back (ms).
pub const TOMBSTONE_RETENTION_MS: i64 = 90 * 24 * 3600 * 1000;

/// Index entries asked for per page.
const PAGE: usize = 1024;

/// Most index entries one pull collects from a peer before acting on them; the rest is read from
/// where it stopped at the next run (security review 2026-10: a peer that never stops paging
/// cannot make this device hold its whole index, or more, in memory).
pub const MAX_PULL_ENTRIES: usize = 200_000;
/// Most bytes of index entries (keys and versions, roughly) one pull collects from a peer.
pub const MAX_PULL_BYTES: usize = 128 << 20;

/// What a scan changed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScanReport {
    pub changed: usize,
    pub moves: usize,
    pub hashed: usize,
}

/// What a pull did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PullReport {
    /// Files and folders created, replaced or deleted here.
    pub taken: usize,
    /// Of those, the ones deleted here (to the bin, or an empty folder removed).
    pub deleted: usize,
    /// Versions that already matched (only their history was merged).
    pub adopted: usize,
    /// Conflicts resolved here (the losing content kept as a copy or binned).
    pub conflicts: usize,
    /// Changes left for later: the local file changed since the last scan, or a step failed.
    pub deferred: usize,
    /// The peer had more changes than one pull collects ([`MAX_PULL_ENTRIES`]); the next pull
    /// continues from where this one stopped.
    pub truncated: bool,
    /// Entries a peer sent that this device will not act on (ignored or internal paths,
    /// inconsistent versions).
    pub rejected: usize,
    /// Files received and installed here (new or replaced content).
    pub received: usize,
    /// The spellings of names held twice, here or on the peer (ADR-0004 rule 2): everything with
    /// those names stays out of the pull, and is read again, until one is renamed. Their steps
    /// count in `deferred` too.
    pub clashes: Vec<String>,
    pub bytes_fetched: u64,
    pub bytes_local: u64,
}

/// One pair's device sync on this device.
pub struct DeviceSync {
    store: SharedStore,
    fs: Arc<dyn LocalFs>,
    pair: Pair,
    share: ShareId,
    me: ShortId,
    clock: Arc<dyn Clock>,
    mode: ConflictMode,
    /// The rules the last scan compiled, rule files included.
    rules: Mutex<Option<Arc<IgnoreRules>>>,
    /// Most entries and bytes one pull collects from a peer.
    pull_limit: (usize, usize),
}

impl DeviceSync {
    pub fn new(
        store: SharedStore,
        fs: Arc<dyn LocalFs>,
        pair: Pair,
        share: ShareId,
        me: DeviceId,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let mode = match pair.conflict_policy {
            wasl_core::ConflictPolicy::KeepBoth => ConflictMode::KeepBoth,
            _ => ConflictMode::NewestWins,
        };
        Self {
            store,
            fs,
            pair,
            share,
            me: me.short_id(),
            clock,
            mode,
            rules: Mutex::new(None),
            pull_limit: (MAX_PULL_ENTRIES, MAX_PULL_BYTES),
        }
    }

    /// Collects at most `entries` index entries and `bytes` of them per pull (tests use small
    /// limits; the defaults are [`MAX_PULL_ENTRIES`] and [`MAX_PULL_BYTES`]).
    pub fn with_pull_limit(mut self, entries: usize, bytes: usize) -> Self {
        self.pull_limit = (entries.max(1), bytes.max(1));
        self
    }

    /// The pair's rules as the last scan compiled them, rule files included. A pull before any
    /// scan is refused rather than judged by partial rules.
    fn rules(&self) -> Result<Arc<IgnoreRules>> {
        self.rules
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or_else(|| EngineError::Other("scan the folder before pulling".into()))
    }

    fn load_index(&self) -> Result<Index> {
        Ok(self
            .store
            .lock()
            .load_index(&self.pair.id)?
            .unwrap_or_else(|| Index::new(self.me)))
    }

    /// Records what changed on disk since the last scan.
    pub async fn scan(&self) -> Result<ScanReport> {
        let (store, fs, pair_id, now) = (
            self.store.clone(),
            self.fs.clone(),
            self.pair.id.clone(),
            self.clock.now_ms(),
        );
        let mut index = self.load_index()?;
        let builder = crate::engine::rules_builder(&self.pair);
        let (report, rules) = tokio::task::spawn_blocking(move || {
            let scan = wasl_fs::scan(fs.as_ref(), builder, &|rel: &str| rel.to_owned(), || false)?;
            if !scan.unreadable.is_empty() {
                // A folder that could not be read would look deleted: record nothing this time.
                return Err(EngineError::Io(format!(
                    "couldn't read {} folder(s); the scan was not recorded",
                    scan.unreadable.len()
                )));
            }
            let mut report = ScanReport::default();
            let scan_rules = Arc::new(scan.rules);
            let mut touched: Vec<String> = Vec::new();
            let mut scanned = Vec::with_capacity(scan.entries.len());
            // An agent's changes wait for the user (docs/AGENT_CHANGES.md): a held path is recorded
            // as the index last knew it, so peers see no change, and their changes to it wait too
            // (the file no longer matches the index).
            let held = {
                let local: Vec<&wasl_core::LocalEntry> = scan.entries.iter().collect();
                crate::agent::held_rels(&store.lock(), &pair_id, &local)?
            };
            let as_indexed = |rel: &str| {
                index.get(rel).filter(|v| !v.deleted).map(|v| Scanned {
                    key: rel.to_owned(),
                    is_dir: v.is_dir,
                    size: index.entries[rel].local_size,
                    mtime: index.entries[rel].local_mtime,
                    hash: None,
                })
            };
            let mut held_seen: Vec<Scanned> = held.iter().filter_map(|rel| as_indexed(rel)).collect();
            // Files cut before the medium tier (one chunk up to 16 MiB) are cut again once, so an
            // edit to them costs a small chunk from then on. Their content is the same, so this
            // makes no new version.
            let tier_key = format!("chunk_tier:{pair_id}");
            let recut = store.lock().setting(&tier_key)?.as_deref() != Some(CHUNK_TIER);
            let medium = u64::from(ChunkParams::MEDIUM.max())..=wasl_chunk::TIER_LIMIT;
            for e in scan.entries {
                if wasl_fs::is_internal_name(name_of(&e.rel)) || held.contains(&e.rel) {
                    continue;
                }
                let stale_cut = recut && !e.is_dir && medium.contains(&e.size);
                let hash = if e.is_dir || !(stale_cut || index.needs_hash(&e.rel, e.size, e.mtime)) {
                    None
                } else {
                    let params = ChunkParams::for_size(e.size);
                    let list = wasl_chunk::chunk_reader(fs.open_read(&e.rel)?, &params)?;
                    remember_chunks(&store, &pair_id, &e.rel, &list)?;
                    report.hashed += 1;
                    touched.push(e.rel.clone());
                    Some(wasl_chunk::to_hex(&list.blake3))
                };
                scanned.push(Scanned {
                    key: e.rel,
                    is_dir: e.is_dir,
                    size: e.size,
                    mtime: e.mtime,
                    hash,
                });
            }
            scanned.append(&mut held_seen);
            let changes = index.apply_scan(&scanned, now);
            report.changed = changes.changed.len();
            report.moves = changes.moves.len();
            for k in &changes.changed {
                if index.get(k).is_some_and(|v| v.deleted) {
                    store.lock().forget_chunks(&pair_id, k)?;
                }
            }
            let before = now - TOMBSTONE_RETENTION_MS;
            let pruned: Vec<String> = index
                .entries
                .iter()
                .filter(|(_, e)| e.version.deleted && e.version.mtime < before)
                .map(|(k, _)| k.clone())
                .collect();
            index.prune_tombstones(before);
            touched.extend(changes.changed);
            store
                .lock()
                .save_index(&pair_id, &index, Some(&touched), &pruned)?;
            if recut {
                store.lock().set_setting(&tier_key, CHUNK_TIER)?;
            }
            Ok::<_, EngineError>((report, scan_rules))
        })
        .await
        .map_err(|e| EngineError::Other(e.to_string()))??;
        *self.rules.lock().unwrap_or_else(|p| p.into_inner()) = Some(rules);
        Ok(report)
    }

    /// Applies what `link`'s device changed since the last pull from it.
    pub async fn pull(&self, link: &dyn PeerLink) -> Result<PullReport> {
        let peer = link.peer().to_string();
        let start = self.store.lock().peer_since(&self.pair.id, &peer)?;
        let mut since = start;
        let mut remote: BTreeMap<String, Version> = BTreeMap::new();
        let (max_entries, max_bytes) = self.pull_limit;
        let (mut collected, mut bytes, mut truncated) = (0usize, 0usize, false);
        loop {
            let page: IndexPage = link
                .changes_since(self.share, since, PAGE)
                .await
                .map_err(peer_error)?;
            for (k, v) in page.entries {
                if k.is_empty() || wasl_fs::check_rel(&k).is_err() {
                    return Err(EngineError::Integrity(format!("peer sent a bad name: {k:?}")));
                }
                collected += 1;
                bytes += entry_bytes(&k, &v);
                remote.insert(k, v);
            }
            if page.next_since <= since && page.more {
                return Err(EngineError::Integrity("peer's index pages do not advance".into()));
            }
            since = page.next_since;
            if !page.more {
                break;
            }
            // Whole pages only, so `since` covers exactly what was collected.
            if collected >= max_entries || bytes >= max_bytes {
                truncated = true;
                break;
            }
        }
        // The pair's rules apply to what peers send as much as to what the scan finds: an
        // ignored or internal path, or a file version without a valid hash, is never acted on.
        let rules = self.rules()?;
        let mut report = PullReport {
            truncated,
            ..PullReport::default()
        };
        let remote: Vec<(String, Version)> = remote
            .into_iter()
            .filter(|(k, v)| {
                let ok = acceptable(&rules, k, v);
                if !ok {
                    report.rejected += 1;
                }
                ok
            })
            .collect();
        let mut index = self.load_index()?;
        let steps = plan_with(&index, &remote, self.mode);
        let mut changed: Vec<String> = Vec::new();
        let steps = self.same_names(&mut index, steps, &mut changed, &mut report);

        // Merged histories first, then folders (parents first), files, and deletions (children
        // first), so a file's folder exists and a folder is empty when it goes.
        let mut ordered: Vec<Step> = steps;
        ordered.sort_by_key(step_order);
        for step in ordered {
            let key = step_key(&step).to_owned();
            let outcome = match step {
                Step::Adopt {
                    key,
                    vv,
                    mtime,
                    author,
                } => {
                    if let Some(e) = index.entries.get(&key).cloned() {
                        let mut v = e.version;
                        v.vv = vv;
                        v.mtime = mtime;
                        v.author = author;
                        record(&mut index, &key, v, e.local_size, e.local_mtime);
                    }
                    report.adopted += 1;
                    Ok(())
                }
                Step::Take { key, version } => self.take(link, &mut index, &key, &version, &mut report).await,
                Step::Conflict {
                    key,
                    winner,
                    copy_key,
                } => {
                    let r = self
                        .resolve(&mut index, &key, copy_key.as_deref(), &mut changed)
                        .await;
                    match r {
                        Ok(()) => {
                            report.conflicts += 1;
                            self.take(link, &mut index, &key, &winner, &mut report).await
                        }
                        Err(e) => Err(e),
                    }
                }
            };
            match outcome {
                Ok(()) => changed.push(key),
                Err(Deferred::Later) => report.deferred += 1,
                // Something the peer got wrong: skipped for good, it does not hold back the rest.
                Err(Deferred::Never) => report.rejected += 1,
            }
        }
        {
            let mut s = self.store.lock();
            s.save_index(&self.pair.id, &index, Some(&changed), &[])?;
            // Read again from the same point next time unless everything was applied.
            if report.deferred == 0 {
                s.set_peer_since(&self.pair.id, &peer, since)?;
            }
        }
        Ok(report)
    }

    /// Applies the name rule (ADR-0004, #173) to a pull's steps, on this device's names and the
    /// peer's alike:
    /// - A file the peer renamed to another spelling of the same name (`Foo.txt` to `foo.txt`, NFD
    ///   to NFC, in one folder) is renamed here in place, so nothing is binned or fetched.
    /// - A name held twice is a **name clash**: two spellings live here, two live in what the peer
    ///   sent, or one each that are not a rename. Every key with that name, and everything under
    ///   it, stays out of this pull (and is read again at the next) until one of them is renamed;
    ///   the spellings are listed in [`PullReport::clashes`].
    fn same_names(
        &self,
        index: &mut Index,
        steps: Vec<Step>,
        changed: &mut Vec<String>,
        report: &mut PullReport,
    ) -> Vec<Step> {
        use wasl_core::identity::name_key;
        // Every live name on either side, by name key: (spelling, on the peer's side).
        let mut names: BTreeMap<String, BTreeSet<(String, bool)>> = BTreeMap::new();
        for (k, e) in &index.entries {
            if !e.version.deleted {
                names.entry(name_key(k)).or_default().insert((k.clone(), false));
            }
        }
        let gone: HashMap<&str, &Version> = steps
            .iter()
            .filter_map(|s| match s {
                Step::Take { key, version } if version.deleted => Some((key.as_str(), version)),
                _ => None,
            })
            .collect();
        for s in &steps {
            let (key, v) = match s {
                Step::Take { key, version } => (key, version),
                Step::Conflict { key, winner, .. } => (key, winner),
                Step::Adopt { .. } => continue,
            };
            if !v.deleted {
                names
                    .entry(name_key(key))
                    .or_default()
                    .insert((key.clone(), true));
            }
        }
        let mut renamed: HashSet<String> = HashSet::new();
        let mut clashing: BTreeSet<String> = BTreeSet::new(); // name keys
        for (nk, spellings) in &names {
            let keys: BTreeSet<&str> = spellings.iter().map(|(k, _)| k.as_str()).collect();
            if keys.len() < 2 {
                continue;
            }
            let here: Vec<&str> = spellings
                .iter()
                .filter(|(_, p)| !p)
                .map(|(k, _)| k.as_str())
                .collect();
            let theirs: Vec<&str> = spellings
                .iter()
                .filter(|(_, p)| *p)
                .map(|(k, _)| k.as_str())
                .collect();
            // The peer renamed our one spelling to its one new spelling: a rename, not a clash.
            if let ([old], [new]) = (here.as_slice(), theirs.as_slice())
                && old != new
                && let Some(tomb) = gone.get(old)
            {
                let is_file = index.get(old).is_some_and(|v| !v.is_dir);
                // If the rename can't be made, the steps run as usual and wait where the folder
                // isn't as indexed. A folder renamed that way still waits as before (#190).
                if is_file && parent_of(old) == parent_of(new) && self.rename_in_place(index, old, new, tomb)
                {
                    renamed.insert((*old).to_owned());
                    changed.extend([(*old).to_owned(), (*new).to_owned()]);
                }
                continue;
            }
            clashing.insert(nk.clone());
            report.clashes.extend(keys.iter().map(|k| (*k).to_owned()));
        }
        report.clashes.sort();
        report.clashes.dedup();
        // A key is held when its name, or a folder above it, clashes.
        let held = |key: &str| {
            !clashing.is_empty()
                && (clashing.contains(&name_key(key))
                    || key
                        .match_indices('/')
                        .any(|(i, _)| clashing.contains(&name_key(&key[..i]))))
        };
        let mut kept = Vec::with_capacity(steps.len());
        for s in steps {
            let key = step_key(&s);
            if renamed.contains(key) {
                continue;
            }
            if held(key) {
                report.deferred += 1;
                continue;
            }
            kept.push(s);
        }
        kept
    }

    /// Renames the unchanged local file `old` to `new` (the same name, spelled as the peer now
    /// spells it) and moves its index entry and chunks with it; `old` takes the peer's tombstone.
    fn rename_in_place(&self, index: &mut Index, old: &str, new: &str, gone: &Version) -> bool {
        let Some(entry) = index.entries.get(old).cloned() else {
            return false;
        };
        if !self.clean(index, old).unwrap_or(false) || self.fs.rename(old, new).is_err() {
            return false;
        }
        record(index, new, entry.version, entry.local_size, entry.local_mtime);
        record(index, old, gone.clone(), 0, 0);
        let list = {
            let s = self.store.lock();
            s.chunked_blake3(&self.pair.id, old)
                .ok()
                .flatten()
                .and_then(|b| s.chunk_list(&self.pair.id, old, &b).ok().flatten())
        };
        if let Some(list) = list.and_then(|bytes| ChunkList::from_bytes(&bytes).ok()) {
            let _ = remember_chunks(&self.store, &self.pair.id, new, &list);
        }
        let _ = self.store.lock().forget_chunks(&self.pair.id, old);
        true
    }

    /// Whether the local item at `key` is as the index last recorded it (or absent where the
    /// index says it is absent).
    fn clean(&self, index: &Index, key: &str) -> io::Result<bool> {
        let disk = self.fs.stat(key)?;
        Ok(match (index.entries.get(key), disk) {
            (None, None) => true,
            (Some(e), None) => e.version.deleted,
            (Some(e), Some(d)) if !e.version.deleted => {
                d.is_dir == e.version.is_dir
                    && (d.is_dir || (d.size == e.local_size && d.mtime_ms == e.local_mtime))
            }
            _ => false,
        })
    }

    async fn take(
        &self,
        link: &dyn PeerLink,
        index: &mut Index,
        key: &str,
        v: &Version,
        report: &mut PullReport,
    ) -> Result<(), Deferred> {
        if v.deleted && index.get(key).is_none_or(|l| l.deleted) {
            // Nothing here under that key to delete: only its history is recorded. The disk is
            // not looked at, so on a folder that folds names the deletion of a spelling never
            // reaches another spelling's file; a file made here meanwhile is a newer version at
            // the next scan.
            record(index, key, v.clone(), 0, 0);
            return Ok(());
        }
        if !self.clean(index, key).map_err(|_| Deferred::Later)? {
            return Err(Deferred::Later);
        }
        let exists = self.fs.stat(key).map_err(|_| Deferred::Later)?;
        if v.deleted {
            let was_here = exists.is_some();
            match exists {
                Some(d) if d.is_dir => {
                    // Only an empty folder goes; anything left in it keeps it.
                    if !self.fs.remove_dir_if_empty(key).map_err(|_| Deferred::Later)? {
                        return Err(Deferred::Later);
                    }
                }
                Some(_) => self.to_bin(key, "DELETED").map_err(|_| Deferred::Later)?,
                None => {}
            }
            let _ = self.store.lock().forget_chunks(&self.pair.id, key);
            record(index, key, v.clone(), 0, 0);
            report.taken += 1;
            report.deleted += usize::from(was_here);
            return Ok(());
        }
        if v.is_dir {
            if exists.as_ref().is_some_and(|d| !d.is_dir) {
                self.to_bin(key, "OVERWRITTEN").map_err(|_| Deferred::Later)?;
            }
            self.fs.mkdir(key).map_err(|_| Deferred::Later)?;
            record(index, key, v.clone(), 0, 0);
            report.taken += 1;
            return Ok(());
        }
        let blake3 = v.hash.as_deref().and_then(parse_hash).ok_or(Deferred::Never)?;
        let here = self
            .store
            .lock()
            .chunked_blake3(&self.pair.id, key)
            .ok()
            .flatten();
        if let Some(d) = &exists
            && !d.is_dir
            && here == Some(blake3)
        {
            // Already this content (made here too): only the version is new.
            record(index, key, v.clone(), d.size, d.mtime_ms);
            report.taken += 1;
            return Ok(());
        }
        let list = self.chunk_list(link, key, blake3).await.ok_or(Deferred::Later)?;
        if list.size != v.size {
            return Err(Deferred::Never);
        }
        let staged = self
            .build(link, key, &list, report)
            .await
            .ok_or(Deferred::Later)?;
        if let Some(parent) = parent_of(key) {
            self.fs.mkdir(parent).map_err(|_| Deferred::Later)?;
        }
        if let Some(d) = &exists {
            let reason = if d.is_dir { "DELETED" } else { "OVERWRITTEN" };
            if d.is_dir && !self.fs.remove_dir_if_empty(key).map_err(|_| Deferred::Later)? {
                let _ = std::fs::remove_file(&staged);
                return Err(Deferred::Later);
            }
            if !d.is_dir {
                self.to_bin(key, reason).map_err(|_| Deferred::Later)?;
            }
        }
        let installed = self
            .fs
            .install(&staged, key, Some(v.mtime))
            .map_err(|_| Deferred::Later)?;
        remember_chunks(&self.store, &self.pair.id, key, &list).map_err(|_| Deferred::Later)?;
        record(index, key, v.clone(), installed.size, installed.mtime_ms);
        report.taken += 1;
        report.received += 1;
        Ok(())
    }

    /// The chunk list for `blake3`: from a local file with that content, else from the peer.
    async fn chunk_list(&self, link: &dyn PeerLink, key: &str, blake3: Hash) -> Option<ChunkList> {
        let local = self
            .store
            .lock()
            .chunk_list_by_content(&self.pair.id, &blake3)
            .ok()
            .flatten();
        if let Some((_, bytes)) = local
            && let Ok(list) = ChunkList::from_bytes(&bytes)
            && list.blake3 == blake3
        {
            return Some(list);
        }
        let list = link.chunk_list(self.share, key, blake3).await.ok()?;
        (list.blake3 == blake3 && list.check().is_ok()).then_some(list)
    }

    /// Builds the file `list` describes in a staging file: chunks found here are copied, the
    /// rest fetched from the peer. Returns the staged path.
    async fn build(
        &self,
        link: &dyn PeerLink,
        key: &str,
        list: &ChunkList,
        report: &mut PullReport,
    ) -> Option<PathBuf> {
        let source = StoreChunks {
            store: self.store.clone(),
            pair_id: self.pair.id.clone(),
        };
        let plan = plan_fetch(list, &source);
        let staged = self.fs.staging_path(key).ok()?;
        let spool = spool_dir(&staged, &new_id(self.clock.as_ref()));
        // Exclusive: never reuse (or later delete) a folder this pull did not make.
        std::fs::create_dir(&spool).ok()?;
        // Fetch what is not here, checked as it arrives; then assemble without the network. A
        // local chunk that turns out stale is fetched and the assembly runs again.
        let mut wanted: Vec<Hash> = plan.to_fetch().iter().map(|c| c.hash).collect();
        let mut result = None;
        // Several chunks in flight hide the round trip of each request (most of the time on an
        // internet path); fewer for large chunks, so memory stays bounded.
        let in_flight = if list.size <= wasl_chunk::TIER_LIMIT {
            FETCH_AHEAD_SMALL
        } else {
            FETCH_AHEAD_LARGE
        };
        for _ in 0..2 {
            let todo: Vec<Hash> = wanted
                .drain(..)
                .collect::<HashSet<_>>()
                .into_iter()
                .filter(|h| !spool.join(wasl_chunk::to_hex(h)).exists())
                .collect();
            let mut fetches = futures_util::stream::iter(todo)
                .map(|h| async move { (h, link.chunk(self.share, h).await) })
                .buffer_unordered(in_flight);
            while let Some((h, got)) = fetches.next().await {
                let Ok(bytes) = got else {
                    let _ = std::fs::remove_dir_all(&spool);
                    return None;
                };
                if *blake3::hash(&bytes).as_bytes() != h {
                    let _ = std::fs::remove_dir_all(&spool);
                    return None;
                }
                report.bytes_fetched += bytes.len() as u64;
                if std::fs::write(spool.join(wasl_chunk::to_hex(&h)), &bytes).is_err() {
                    let _ = std::fs::remove_dir_all(&spool);
                    return None;
                }
            }
            let (fs, plan2, staged2, spool2) = (self.fs.clone(), plan.clone(), staged.clone(), spool.clone());
            let outcome = tokio::task::spawn_blocking(move || {
                let mut out = io::BufWriter::new(File::create(&staged2)?);
                let stats = wasl_chunk::assemble(
                    &plan2,
                    |at| read_at(fs.as_ref(), at),
                    |h| std::fs::read(spool2.join(wasl_chunk::to_hex(h))),
                    &mut out,
                );
                out.flush()?;
                Ok::<_, io::Error>(stats)
            })
            .await
            .ok()?
            .ok()?;
            match outcome {
                Ok(stats) => {
                    report.bytes_local += plan.bytes_local().saturating_sub(stats.bytes_fetched);
                    result = Some(staged.clone());
                    break;
                }
                Err(wasl_chunk::AssembleError::Fetch { hash, .. }) => {
                    if let Some(h) = parse_hash(&hash) {
                        wanted.push(h);
                    }
                }
                Err(_) => break,
            }
        }
        let _ = std::fs::remove_dir_all(&spool);
        if result.is_none() {
            let _ = std::fs::remove_file(&staged);
        }
        result
    }

    /// Before taking the winner of a conflict: keeps the local (losing) content as a conflict
    /// copy, or bins it.
    async fn resolve(
        &self,
        index: &mut Index,
        key: &str,
        copy_key: Option<&str>,
        changed: &mut Vec<String>,
    ) -> Result<(), Deferred> {
        if !self.clean(index, key).map_err(|_| Deferred::Later)? {
            return Err(Deferred::Later);
        }
        let Some(local) = index.entries.get(key).cloned() else {
            return Ok(());
        };
        let disk = self.fs.stat(key).map_err(|_| Deferred::Later)?;
        match (copy_key, disk) {
            (Some(copy), Some(d)) if !d.is_dir => {
                let copy = self.free_copy_name(index, copy)?;
                let copy = copy.as_str();
                self.fs.rename(key, copy).map_err(|_| Deferred::Later)?;
                // The copy is a new file here, with the losing content. Its vector descends from
                // whatever the index held at that name (an earlier copy's tombstone), so no old
                // deletion of that name can bin it (#141).
                let mut v = local.version.clone();
                v.vv = index
                    .get(copy)
                    .map(|old| old.vv.clone())
                    .unwrap_or_default()
                    .ticked(self.me);
                v.author = self.me;
                record(index, copy, v, local.local_size, local.local_mtime);
                let old_list = {
                    let s = self.store.lock();
                    s.chunked_blake3(&self.pair.id, key)
                        .ok()
                        .flatten()
                        .and_then(|b| s.chunk_list(&self.pair.id, key, &b).ok().flatten())
                };
                if let Some(list) = old_list.and_then(|bytes| ChunkList::from_bytes(&bytes).ok()) {
                    let _ = remember_chunks(&self.store, &self.pair.id, copy, &list);
                }
                let _ = self.store.lock().forget_chunks(&self.pair.id, key);
                // The key is empty now; the index must say so before the winner is taken.
                let mut gone = local.version;
                gone.deleted = true;
                record(index, key, gone, 0, 0);
                changed.push(copy.to_owned());
            }
            (_, Some(d)) if !d.is_dir => {
                self.to_bin(key, "CONFLICT").map_err(|_| Deferred::Later)?;
                let mut gone = local.version;
                gone.deleted = true;
                record(index, key, gone, 0, 0);
            }
            _ => {}
        }
        Ok(())
    }

    /// `copy`, or `copy` numbered (`f (conflict 1a2b 9f3c07d2 2).txt`) when something already holds
    /// that name here, so a conflict never waits for an earlier copy to go (#141).
    fn free_copy_name(&self, index: &Index, copy: &str) -> Result<String, Deferred> {
        let taken = |name: &str| -> Result<bool, Deferred> {
            Ok(index.get(name).is_some_and(|v| !v.deleted)
                || self.fs.stat(name).map_err(|_| Deferred::Later)?.is_some())
        };
        if !taken(copy)? {
            return Ok(copy.to_owned());
        }
        let (stem, ext) = match copy.rsplit_once(')') {
            Some((s, e)) => (s, e),
            None => (copy, ""),
        };
        for n in 2..100 {
            let name = format!("{stem} {n}){ext}");
            if !taken(&name)? {
                return Ok(name);
            }
        }
        Err(Deferred::Later)
    }

    /// Moves the local file at `key` to the bin and records it there.
    fn to_bin(&self, key: &str, reason: &str) -> Result<()> {
        let size = self.fs.stat(key)?.map_or(0, |d| d.size);
        let location = self.fs.move_to_bin(key)?;
        let now = self.clock.now_ms();
        let entry = NewBinEntry {
            id: new_id(self.clock.as_ref()),
            pair_id: self.pair.id.clone(),
            side: BinSide::Local,
            key: key.to_owned(),
            reason: reason.to_owned(),
            location,
            deleted_at: now,
            expires_at: now + BIN_RETENTION_MS,
            size,
            account_id: None,
            remote_file_id: None,
            display_name: Some(name_of(key).to_owned()),
            pair_name: Some(self.pair.name.clone()),
        };
        self.store.lock().add_to_bin(&entry)?;
        Ok(())
    }
}

/// A step not applied: `Later` is read again at the next pull (a local file changed since the
/// scan, the peer was unreachable); `Never` was the peer's mistake and is skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Deferred {
    Later,
    Never,
}

/// Whether a peer's entry is one this device may act on: no internal name in its path, nothing
/// the pair's rules ignore (the entry or a folder above it), and a file version with a valid hash.
fn acceptable(rules: &IgnoreRules, key: &str, v: &Version) -> bool {
    if key.split('/').any(wasl_fs::is_internal_name) {
        return false;
    }
    let mut at = 0;
    while let Some(i) = key[at..].find('/') {
        if rules.is_ignored(&key[..at + i], true) {
            return false;
        }
        at += i + 1;
    }
    if rules.is_ignored(key, v.is_dir) {
        return false;
    }
    v.deleted || v.is_dir || v.hash.as_deref().and_then(parse_hash).is_some()
}

/// Roughly what one index entry holds in memory: its key, its version's fixed part and hash, and
/// its version vector.
fn entry_bytes(key: &str, v: &Version) -> usize {
    key.len() + 128 + v.vv.entries().count() * 16
}

fn record(index: &mut Index, key: &str, v: Version, local_size: u64, local_mtime: i64) {
    index.record(key, v);
    if let Some(e) = index.entries.get_mut(key) {
        e.local_size = local_size;
        e.local_mtime = local_mtime;
    }
}

fn step_key(s: &Step) -> &str {
    match s {
        Step::Take { key, .. } | Step::Adopt { key, .. } | Step::Conflict { key, .. } => key,
    }
}

/// Adopt, then folders by depth, files, deletions deepest first.
fn step_order(s: &Step) -> (u8, i64, String) {
    let depth = step_key(s).matches('/').count() as i64;
    let key = step_key(s).to_owned();
    match s {
        Step::Adopt { .. } => (0, 0, key),
        Step::Take { version, .. } | Step::Conflict { winner: version, .. } => {
            if version.deleted {
                (3, -depth, key)
            } else if version.is_dir {
                (1, depth, key)
            } else {
                (2, 0, key)
            }
        }
    }
}

fn name_of(rel: &str) -> &str {
    rel.rsplit_once('/').map_or(rel, |(_, n)| n)
}

fn parent_of(rel: &str) -> Option<&str> {
    rel.rsplit_once('/').map(|(p, _)| p)
}

fn parse_hash(hex: &str) -> Option<Hash> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

fn peer_error(e: PeerError) -> EngineError {
    match e {
        PeerError::NotShared => EngineError::NotFound("this folder is not shared with that device".into()),
        e => EngineError::Io(e.to_string()),
    }
}

/// `<staged>.<id>.wasl-part`: next to the staging file, named so scans skip it.
fn spool_dir(staged: &Path, id: &str) -> PathBuf {
    let mut name = staged.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{id}"));
    name.push(wasl_fs::PART_SUFFIX);
    staged.with_file_name(name)
}

fn remember_chunks(store: &SharedStore, pair_id: &str, key: &str, list: &ChunkList) -> Result<()> {
    let rows: Vec<(Hash, u64, u64)> = list
        .chunks
        .iter()
        .map(|c| (c.hash, c.offset, u64::from(c.len)))
        .collect();
    store
        .lock()
        .put_chunk_list(pair_id, key, &list.blake3, &list.to_bytes(), &rows)?;
    Ok(())
}

/// Reads one chunk from a local file.
fn read_at(fs: &dyn LocalFs, at: &ChunkLocation) -> io::Result<Vec<u8>> {
    let readable = fs.readable_path(&at.file)?;
    let mut f = File::open(&readable.path)?;
    f.seek(SeekFrom::Start(at.offset))?;
    let mut buf = vec![0u8; at.len as usize];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

/// Chunks of the pair's local files, as the store's chunk table places them.
struct StoreChunks {
    store: SharedStore,
    pair_id: String,
}

impl ChunkSource for StoreChunks {
    fn locate(&self, hash: &Hash) -> Option<ChunkLocation> {
        let rows = self.store.lock().locate_chunks(&self.pair_id, hash).ok()?;
        rows.into_iter().next().map(|r| ChunkLocation {
            file: r.key,
            offset: r.offset,
            len: u32::try_from(r.len).unwrap_or(u32::MAX),
        })
    }
}

/// Opens a pair's local folder.
type OpenFolder = dyn Fn(&Pair) -> Result<Arc<dyn LocalFs>> + Send + Sync;

/// Answers peers for every shared pair of this device.
pub struct DeviceService {
    store: SharedStore,
    local_fs: Arc<OpenFolder>,
    on_notify: Arc<dyn Fn(DeviceId, String, u64) + Send + Sync>,
    /// Where the request being answered came from (`None`: in process, or not known).
    remote: Option<SocketAddr>,
}

impl DeviceService {
    /// `local_fs` opens a pair's folder; `on_notify(peer, pair_id, seq)` is called when a peer
    /// says a pair changed (start a pull).
    pub fn new(
        store: SharedStore,
        local_fs: impl Fn(&Pair) -> Result<Arc<dyn LocalFs>> + Send + Sync + 'static,
        on_notify: impl Fn(DeviceId, String, u64) + Send + Sync + 'static,
    ) -> Self {
        Self {
            store,
            local_fs: Arc::new(local_fs),
            on_notify: Arc::new(on_notify),
            remote: None,
        }
    }

    /// The same service answering a request that came from `remote`.
    pub fn from_remote(&self, remote: SocketAddr) -> DeviceService {
        DeviceService {
            store: self.store.clone(),
            local_fs: self.local_fs.clone(),
            on_notify: self.on_notify.clone(),
            remote: Some(remote),
        }
    }

    /// The pair shared as `share`, if `peer` may sync it. The transport only lets members
    /// connect; on top of that (Introductions, 2026-10, docs/INTRODUCTIONS.md §3.2) a pair is
    /// served only to a device on its list, or, when it is shared with all devices, to a member
    /// known on this device: a member another device added stays "new" here and gets nothing until
    /// its person keeps it. The check uses the peer the transport authenticated, on every request.
    /// A pair whose reach is the LAN is never served to a request from beyond it (security review
    /// 2026-10, FR-N11).
    fn pair(&self, share: ShareId, peer: DeviceId) -> PeerResult<Pair> {
        let s = self.store.lock();
        let remote = |e: wasl_store::StoreError| PeerError::Remote(e.to_string());
        let id = s
            .pair_for_share(&share.to_hex())
            .map_err(remote)?
            .ok_or(PeerError::NotShared)?;
        let pair = s.get_pair(&id).map_err(remote)?.ok_or(PeerError::NotShared)?;
        if !pair.enabled || pair.route == wasl_core::Route::Drive {
            return Err(PeerError::NotShared);
        }
        if !s.share_allows(&id, &peer.to_string()).map_err(remote)? {
            return Err(PeerError::NotShared);
        }
        if pair.reach == wasl_core::Reach::Lan
            && self
                .remote
                .is_some_and(|a| !wasl_net::rendezvous::is_lan_ip(a.ip()))
        {
            return Err(PeerError::NotShared);
        }
        Ok(pair)
    }
}

#[async_trait]
impl PeerService for DeviceService {
    async fn shared(&self, peer: DeviceId) -> Vec<ShareId> {
        let shares = self.store.lock().shares().unwrap_or_default();
        shares
            .into_iter()
            .filter_map(|(_, s)| ShareId::from_hex(&s))
            .filter(|s| self.pair(*s, peer).is_ok())
            .collect()
    }

    /// The pairs that list `peer` by name and would be served to it now (§3.3): their name, and
    /// files and bytes from this device's index.
    async fn offers(&self, peer: DeviceId) -> Vec<Offer> {
        let listed = self
            .store
            .lock()
            .shares_offered_to(&peer.to_string())
            .unwrap_or_default();
        let mut out = Vec::new();
        for (_, share) in listed {
            let Some(share) = ShareId::from_hex(&share) else {
                continue;
            };
            let Ok(pair) = self.pair(share, peer) else {
                continue;
            };
            let (files, bytes) = self.store.lock().index_totals(&pair.id).unwrap_or((0, 0));
            out.push(Offer {
                share,
                name: wasl_net::peer::offer_name(&pair.name),
                files,
                bytes,
            });
            if out.len() == wasl_net::peer::MAX_OFFERS {
                break;
            }
        }
        out
    }

    async fn changes_since(
        &self,
        peer: DeviceId,
        pair: ShareId,
        since: u64,
        limit: usize,
    ) -> PeerResult<IndexPage> {
        let pair = self.pair(pair, peer)?;
        let (rows, more) = self
            .store
            .lock()
            .index_changes_since(&pair.id, since, limit)
            .map_err(|e| PeerError::Remote(e.to_string()))?;
        let next_since = rows.last().map_or(since, |(_, _, seq)| *seq);
        Ok(IndexPage {
            entries: rows.into_iter().map(|(k, v, _)| (k, v)).collect(),
            next_since,
            more,
        })
    }

    async fn chunk_list(
        &self,
        peer: DeviceId,
        pair: ShareId,
        key: &str,
        blake3: Hash,
    ) -> PeerResult<ChunkList> {
        let pair = self.pair(pair, peer)?;
        let bytes = self
            .store
            .lock()
            .chunk_list(&pair.id, key, &blake3)
            .map_err(|e| PeerError::Remote(e.to_string()))?
            .ok_or(PeerError::Gone)?;
        ChunkList::from_bytes(&bytes).map_err(|_| PeerError::Gone)
    }

    async fn chunk(&self, peer: DeviceId, pair: ShareId, hash: Hash) -> PeerResult<Vec<u8>> {
        let pair = self.pair(pair, peer)?;
        let fs = (self.local_fs)(&pair).map_err(|e| PeerError::Remote(e.to_string()))?;
        let rows = self
            .store
            .lock()
            .locate_chunks(&pair.id, &hash)
            .map_err(|e| PeerError::Remote(e.to_string()))?;
        tokio::task::spawn_blocking(move || {
            for r in rows {
                let at = ChunkLocation {
                    file: r.key,
                    offset: r.offset,
                    len: u32::try_from(r.len).unwrap_or(u32::MAX),
                };
                // The file may have changed since it was chunked: send only what still matches.
                if let Ok(bytes) = read_at(fs.as_ref(), &at)
                    && *blake3::hash(&bytes).as_bytes() == hash
                {
                    return Ok(bytes);
                }
            }
            Err(PeerError::Gone)
        })
        .await
        .map_err(|e| PeerError::Remote(e.to_string()))?
    }

    async fn notify(&self, peer: DeviceId, pair: ShareId, seq: u64) {
        if let Ok(p) = self.pair(pair, peer) {
            (self.on_notify)(peer, p.id, seq);
        }
    }

    fn for_remote(&self, remote: SocketAddr) -> Option<Arc<dyn PeerService>> {
        Some(Arc::new(self.from_remote(remote)))
    }
}

/// Every share ID → pair ID this store holds.
pub fn shares_by_id(store: &SharedStore) -> Result<HashMap<ShareId, String>> {
    Ok(store
        .lock()
        .shares()?
        .into_iter()
        .filter_map(|(p, s)| ShareId::from_hex(&s).map(|s| (s, p)))
        .collect())
}
