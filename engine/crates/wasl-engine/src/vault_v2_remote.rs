//! An encrypted pair's remote in vault format v2 (`docs/ENCRYPTION.md` §11, design
//! `docs/ON_DEMAND.md` §1): [`VaultV2Remote`] wraps any [`Remote`] like [`crate::VaultRemote`]
//! does for v1, and the engine above sees plaintext keys, names, sizes and times.
//!
//! On Drive a v2 vault is `vault.json`, a tree of folders and file manifests with AES-SIV names,
//! and every chunk of every file under `.wasl-chunks/<00..ff>/`:
//!
//! * **Listing** walks the encrypted tree. A folder's dir id comes from its `wasl.dir` property
//!   or, when the remote does not report properties (or it was lost), from its `.wasl-dir` object;
//!   a folder whose id does not verify is skipped with everything under it. A file's plaintext
//!   size and time come from its `wasl.m` property (no manifest download); only when that is
//!   missing or does not verify is the manifest read. The listed id, version and MD5 are those of
//!   the manifest's Drive file. Its content changes with every write (fresh nonce), so its MD5 (of
//!   ciphertext, never compared with a local hash: [`Remote::plaintext_hashes`] is false) changes
//!   exactly then, and the planner's id + MD5 identity sees every write and nothing else: Drive
//!   raises a file's version by itself shortly after many uploads. `vault.json`, `.wasl-chunks`, `.wasl-dir`, Google
//!   documents and anything whose name does not decrypt are never listed (so never deleted).
//! * **Upload** encrypts with `encrypt_stream` (new file) or `encrypt_update` (existing file: its
//!   current manifest is opened first), uploads only the chunks the old version lacks, then writes
//!   the manifest (new content of the same Drive file, or a new file under its encrypted name) with
//!   `wasl.m` set in the same write, then deletes the chunks only the old version had. A crash
//!   leaves extra chunks (the sweep removes them), never a manifest naming a missing chunk.
//! * **Download** opens the manifest, fetches the chunks in order and decrypts with
//!   `decrypt_file`, which checks every chunk and then the size and whole-file BLAKE3.
//! * **Delete** trashes the manifest (or the folder) to Drive's bin and leaves the chunks, so a
//!   file restored from Drive's bin reads again. [`VaultV2Remote::sweep`] deletes a chunk object
//!   only once no live manifest has referenced it for the whole grace period (and it was uploaded
//!   at least that long ago); it runs with the recycle-bin purge ([`Remote::housekeeping`]). When
//!   each chunk was first seen unreferenced is kept per pair (the store's `vault_orphans`, in
//!   memory without a store, which only delays deletion), so a file binned today stays restorable
//!   for the whole grace period however old its chunks are: with the default (30 days, the bins'
//!   retention) as long as its bin entry. The sweep refuses to run while any manifest cannot be
//!   read or a folder cannot be verified, since their chunks would look unreferenced.
//! * **Rollback**: the highest manifest version seen of every file is kept (in the store's
//!   `vault_versions` table once [`VaultV2Remote::use_store`] is called, in memory otherwise) and
//!   an older manifest is refused with an integrity error. A format 2.1 writer also publishes its
//!   own state record at the vault root after every write (`wasl_crypto::v2::state`); a listing
//!   whose records went back, or lost one, fails with [`VAULT_ROLLED_BACK`] before anything is
//!   applied, and one that does not match the newest record is reported ([`StateCheck`]).
//! * **Format revision**: chunks are cut and sealed as the vault's `vault.json` says (2.0 or 2.1,
//!   [`Format`]); without one (views, tests) as the newest. Both revisions are read.
//! * **Times**: Drive's modification time of a manifest is the upload time, not the file's: the
//!   real time is only inside the manifest and its `wasl.m` blob (security review S4-3).
//! * **Files on demand**: [`VaultV2Remote`] is a [`wasl_vfs::Source`]. [`FileRef::id`] is the
//!   manifest's Drive id (as listed), [`FileRef::version`] its MD5 (`on_demand::view_tree`); a
//!   read fetches and decrypts only the chunks covering the range.

use crate::remote::{ItemUpdate, Listing, ProgressFn, Remote, RemoteInfo, Revision, UploadSpec};
use crate::vault_remote::{
    DIFFERENT_VAULT, DOCS_UNSUPPORTED, ManifestStatus, NEEDS_EMPTY_FOLDER, crypto_error, hex, io_error,
};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::io::{BufReader, BufWriter, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use wasl_core::RemoteEntry;
use wasl_crypto::CryptoError;
use wasl_crypto::v2::chunks::{CHUNKS_DIR, MAX_SEALED_BYTES};
use wasl_crypto::v2::manifest::{MAX_MANIFEST_BYTES, META_PROPERTY};
use wasl_crypto::v2::names::{self as v2names, DIR_MARKER_BYTES, DIR_MARKER_NAME, DIR_PROPERTY};
use wasl_crypto::v2::state::{RECORD_BYTES, StateRecord, WriterId, set_digest};
use wasl_crypto::v2::{
    ChunkId, DirId, FileId, FileManifest, Format, MetaBlob, PendingChunk, Placement, RangeReader,
    VaultKeysV2, VaultManifestV2, decrypt_file, encrypt_stream, encrypt_update, plan_update,
};
use wasl_drive::{DriveError, Result};
use wasl_store::SharedStore;
use wasl_vfs::{FileRef, Source, VfsError};

/// The folder holds a v1 vault but the pair is set up for v2.
pub const VAULT_IS_V1: &str = "The Drive folder holds a vault of format v1; this pair is set up for vault v2";
/// The folder holds a v2 vault but the pair is set up for v1.
pub const VAULT_IS_V2: &str = "The Drive folder holds a vault of format v2; this pair is set up for vault v1";
/// A listing found the vault's state records older than (or missing some of) what this device
/// already saw (security review S4-4).
pub const VAULT_ROLLED_BACK: &str = "This vault's folder on Drive is older than a state this device already saw: someone may have put back an earlier copy of it or removed its state records. Nothing was changed here";
/// The sweep's default grace: Drive keeps its bin 30 days.
pub const SWEEP_GRACE: Duration = Duration::from_secs(30 * 24 * 3600);

const TEMP_PREFIX: &str = "v2-";
const TEMP_MAX_AGE: Duration = Duration::from_secs(6 * 3600);
/// Chunks in flight between the encryptor and the uploader (each at most 4 MiB + 32).
const PIPELINE_DEPTH: usize = 2;

/// What [`VaultV2Remote::list_subtree`] left out of its last listing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct V2Skips {
    /// Items whose names do not decrypt in their folder (someone else's, moved, tampered).
    pub foreign: usize,
    /// Google-native documents.
    pub google_docs: usize,
    /// Folders whose dir id does not verify (neither `wasl.dir` nor `.wasl-dir`); their contents
    /// are not listed.
    pub unverified_folders: usize,
}

/// How the last listing compared with the vault's state records (ENCRYPTION.md §11.11). A record
/// that went back or disappeared is not reported here: the listing fails with
/// [`VAULT_ROLLED_BACK`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StateCheck {
    /// Not listed yet.
    #[default]
    Unknown,
    /// The folder has no state records (a 2.0 vault, or nothing written since 2.1).
    NoRecords,
    /// The files listed are exactly those the newest record describes.
    Consistent,
    /// The files listed differ from the newest record: a writer that has not published yet (still
    /// running, or stopped before it could), or files put back, removed or rolled back on Drive
    /// while the records were kept. The two cannot be told apart.
    Unpublished,
}

/// What a [`VaultV2Remote::sweep`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Chunk objects some live manifest lists.
    pub referenced: usize,
    /// Unreferenced chunk objects deleted.
    pub deleted: usize,
    /// Unreferenced chunk objects kept: unreferenced (or uploaded) less than the grace period ago.
    pub kept_recent: usize,
}

#[derive(Clone)]
enum Kind {
    Dir {
        dir_id: DirId,
        marker_id: Option<String>,
    },
    File,
}

#[derive(Clone)]
struct Item {
    parent_id: String,
    placement: Placement,
    kind: Kind,
}

#[derive(Clone, Copy)]
struct ChunkObject {
    mtime: i64,
    /// The size Drive lists.
    size: Option<u64>,
}

#[derive(Default)]
struct State {
    root_id: Option<String>,
    /// Folder Drive id → dir id (the root included).
    dirs: HashMap<String, DirId>,
    /// Vault items by Drive id (the manifest's id for files).
    items: HashMap<String, Item>,
    chunks_dir: Option<String>,
    /// Shard name (`00`..`ff`) → folder id.
    shards: HashMap<String, String>,
    /// Chunk object name → (Drive id, info).
    chunk_objects: HashMap<String, (String, ChunkObject)>,
    /// Folders whose id did not verify, by (parent id, encrypted name): reused by
    /// [`VaultV2Remote::create_folder`] instead of creating a same-named sibling.
    unverified: HashMap<(String, String), String>,
    /// Raw objects read during listings, by Drive id: (Drive version, bytes).
    raw_cache: HashMap<String, (Option<i64>, Vec<u8>)>,
    /// The size Drive listed for every object of the last listing (and objects written since),
    /// by Drive id: an object listed bigger than its format allows is never downloaded.
    listed_sizes: HashMap<String, u64>,
    /// Opened manifests for on-demand reads, by Drive id: ([`FileRef::version`], manifest).
    manifests: HashMap<String, (String, Arc<FileManifest>)>,
    /// Highest manifest version seen per file id (also in the store when attached).
    versions: HashMap<FileId, u64>,
    /// Chunk object name → when the sweep first saw it unreferenced (the store keeps it when
    /// attached).
    orphans: HashMap<String, i64>,
    skips: V2Skips,
    /// Live files as listed (and changed by this remote's own writes since): Drive id →
    /// (placement bytes, `wasl.m` blob), what a state record's digest covers.
    files: HashMap<String, (Vec<u8>, String)>,
    /// State records listed (and this remote's own since): writer → (Drive id, record).
    records: HashMap<WriterId, (String, StateRecord)>,
    /// State-record counters seen, by key (`state-w-<writer>`, `state-sum`; the store keeps them
    /// when attached), and this remote's writer id when no store keeps it.
    seen: HashMap<String, u64>,
    state_check: StateCheck,
    /// Items with vault-shaped names that did not decrypt, and duplicates: the sweep does not run
    /// while there are any (their chunks would look unreferenced).
    doubtful: usize,
}

pub struct VaultV2Remote {
    inner: Arc<dyn Remote>,
    keys: Arc<VaultKeysV2>,
    tmp_dir: PathBuf,
    manifest: Option<VaultManifestV2>,
    store: Mutex<Option<(SharedStore, String)>>,
    grace: Duration,
    state: Mutex<State>,
    /// Serializes creating `.wasl-chunks` and its shard folders.
    layout: tokio::sync::Mutex<()>,
    /// One state record publication at a time; `dirty` asks for another.
    publishing: tokio::sync::Mutex<()>,
    dirty: AtomicBool,
}

impl VaultV2Remote {
    /// Wraps `inner` with the vault's keys. `tmp_dir` holds small temporaries (one chunk or one
    /// manifest at a time); stale ones are removed here.
    pub fn new(inner: Arc<dyn Remote>, keys: Arc<VaultKeysV2>, tmp_dir: impl Into<PathBuf>) -> VaultV2Remote {
        let tmp_dir = tmp_dir.into();
        purge_stale_temps(&tmp_dir);
        VaultV2Remote {
            inner,
            keys,
            tmp_dir,
            manifest: None,
            store: Mutex::new(None),
            grace: SWEEP_GRACE,
            state: Mutex::new(State::default()),
            layout: tokio::sync::Mutex::new(()),
            publishing: tokio::sync::Mutex::new(()),
            dirty: AtomicBool::new(false),
        }
    }

    /// The format revision new chunks are written in: the vault's (from [`Self::with_manifest`]),
    /// or the newest when the remote was made without one (views read only; tests).
    pub fn write_format(&self) -> Format {
        self.manifest
            .as_ref()
            .map_or(Format::LATEST, VaultManifestV2::format)
    }

    /// How the last listing compared with the vault's state records.
    pub fn last_state_check(&self) -> StateCheck {
        self.st().state_check
    }

    /// Checks (or creates) the folder's `vault.json` before every listing ([`ensure_manifest_v2`]).
    pub fn with_manifest(mut self, manifest: VaultManifestV2) -> VaultV2Remote {
        self.manifest = Some(manifest);
        self
    }

    /// Keeps the highest manifest version seen per file in `store` under `pair_id` (rollback
    /// detection across restarts).
    pub fn with_store(self, store: SharedStore, pair_id: impl Into<String>) -> VaultV2Remote {
        self.use_store(store, pair_id);
        self
    }

    /// [`Self::with_store`] on a shared remote.
    pub fn use_store(&self, store: SharedStore, pair_id: impl Into<String>) {
        *self.store.lock().unwrap_or_else(|p| p.into_inner()) = Some((store, pair_id.into()));
    }

    /// The sweep's grace period ([`SWEEP_GRACE`] by default).
    pub fn with_sweep_grace(mut self, grace: Duration) -> VaultV2Remote {
        self.grace = grace;
        self
    }

    /// What the last listing skipped.
    pub fn last_skips(&self) -> V2Skips {
        self.st().skips
    }

    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn dir_of(&self, folder_id: &str) -> Result<DirId> {
        self.st().dirs.get(folder_id).copied().ok_or_else(|| {
            DriveError::Unknown(format!(
                "vault folder {folder_id} is not known yet (list the vault first)"
            ))
        })
    }

    fn temp_path(&self) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.tmp_dir).map_err(io_error)?;
        let r: [u8; 12] = random_bytes()?;
        Ok(self.tmp_dir.join(format!("{TEMP_PREFIX}{}", hex(&r))))
    }

    /// Downloads a whole object (a chunk, a manifest, a marker) of at most `max` bytes into
    /// memory. Security review 2026-10: anyone with write access to the folder can replace an
    /// object, so one listed bigger than `max` is refused before downloading, and a download that
    /// goes past `max` whatever was listed is stopped, so neither disk nor memory fills up.
    async fn fetch(&self, id: &str, name: &str, listed: Option<u64>, max: usize) -> Result<Vec<u8>> {
        let max = max as u64;
        let listed = listed.or_else(|| self.st().listed_sizes.get(id).copied());
        if let Some(size) = listed
            && size > max
        {
            return Err(too_big(name, max));
        }
        let temp = self.temp_path()?;
        let entry = RemoteEntry {
            key: name.to_owned(),
            id: id.to_owned(),
            name: name.to_owned(),
            is_dir: false,
            size: None,
            md5: None,
            mtime: 0,
            version: None,
            google_mime: None,
            parent_id: None,
        };
        let over = tokio::sync::Notify::new();
        let progress = |done: u64| {
            if done > max {
                over.notify_one();
            }
        };
        // Dropping the download future stops it (the Drive client reports progress at least
        // every megabyte, so it overshoots by about that much).
        let got = tokio::select! {
            r = self.inner.download(&entry, &temp, &progress) => r,
            () = over.notified() => Err(too_big(name, max)),
        };
        let bytes = got.and_then(|()| read_at_most(&temp, max).map_err(io_error));
        let _ = std::fs::remove_file(&temp);
        match bytes? {
            Some(bytes) => Ok(bytes),
            None => Err(too_big(name, max)),
        }
    }

    /// A listed object's bytes, read once per Drive version.
    async fn fetch_cached(
        &self,
        id: &str,
        name: &str,
        version: Option<i64>,
        listed: Option<u64>,
        max: usize,
    ) -> Result<Vec<u8>> {
        if let Some((v, bytes)) = self.st().raw_cache.get(id)
            && *v == version
            && version.is_some()
        {
            return Ok(bytes.clone());
        }
        let bytes = self.fetch(id, name, listed, max).await?;
        self.st()
            .raw_cache
            .insert(id.to_owned(), (version, bytes.clone()));
        Ok(bytes)
    }

    async fn fetch_chunk(&self, id: &ChunkId) -> Result<Vec<u8>> {
        let name = id.object_name();
        let object = self.st().chunk_objects.get(&name).cloned();
        let Some((drive_id, object)) = object else {
            return Err(DriveError::Integrity(format!(
                "chunk {name} of this file is missing from the vault"
            )));
        };
        match self.fetch(&drive_id, &name, object.size, MAX_SEALED_BYTES).await {
            Err(DriveError::NotFound(_)) => Err(DriveError::Integrity(format!(
                "chunk {name} of this file is missing from the vault"
            ))),
            other => other,
        }
    }

    /// Writes `bytes` to a temp file and uploads it.
    async fn put_bytes(
        &self,
        bytes: &[u8],
        name: &str,
        parent_id: &str,
        existing_id: Option<&str>,
        mtime_ms: i64,
        properties: &[(&str, &str)],
    ) -> Result<RemoteInfo> {
        let temp = self.temp_path()?;
        std::fs::write(&temp, bytes).map_err(io_error)?;
        let result = self
            .inner
            .upload_with_properties(
                UploadSpec {
                    src: &temp,
                    name,
                    parent_id,
                    existing_id,
                    mtime_ms,
                    session_key: None,
                },
                properties,
                &|_| {},
            )
            .await;
        let _ = std::fs::remove_file(&temp);
        if let Ok(info) = &result {
            self.st().listed_sizes.insert(info.id.clone(), bytes.len() as u64);
        }
        result
    }

    // ------------------------------------------------------------------ versions (rollback)

    fn highest_seen(&self, file_id: &FileId) -> Result<u64> {
        let mem = self.st().versions.get(file_id).copied().unwrap_or(0);
        let stored = match &*self.store.lock().unwrap_or_else(|p| p.into_inner()) {
            Some((store, pair)) => store
                .lock()
                .vault_version(pair, &hex(file_id.as_bytes()))
                .map_err(|e| DriveError::Unknown(format!("the store failed: {e}")))?
                .unwrap_or(0),
            None => 0,
        };
        Ok(mem.max(stored))
    }

    fn note_version(&self, m: &FileManifest) -> Result<()> {
        {
            let mut s = self.st();
            let v = s.versions.entry(m.file_id).or_default();
            *v = (*v).max(m.version);
        }
        if let Some((store, pair)) = &*self.store.lock().unwrap_or_else(|p| p.into_inner()) {
            store
                .lock()
                .note_vault_version(pair, &hex(m.file_id.as_bytes()), m.version)
                .map_err(|e| DriveError::Unknown(format!("the store failed: {e}")))?;
        }
        Ok(())
    }

    /// The highest value seen under `key` (state-record counters), from memory and the store.
    fn seen(&self, key: &str) -> Result<u64> {
        let mem = self.st().seen.get(key).copied().unwrap_or(0);
        let stored = match &*self.store.lock().unwrap_or_else(|p| p.into_inner()) {
            Some((store, pair)) => store
                .lock()
                .vault_version(pair, key)
                .map_err(|e| DriveError::Unknown(format!("the store failed: {e}")))?
                .unwrap_or(0),
            None => 0,
        };
        Ok(mem.max(stored))
    }

    /// Records `value` under `key`; what is kept only ever grows.
    fn note_seen(&self, key: &str, value: u64) -> Result<()> {
        {
            let mut s = self.st();
            let v = s.seen.entry(key.to_owned()).or_default();
            *v = (*v).max(value);
        }
        if let Some((store, pair)) = &*self.store.lock().unwrap_or_else(|p| p.into_inner()) {
            store
                .lock()
                .note_vault_version(pair, key, value)
                .map_err(|e| DriveError::Unknown(format!("the store failed: {e}")))?;
        }
        Ok(())
    }

    /// This device's writer id in this vault, made once and kept (in the store when attached:
    /// a restarted device goes on with its own record and counter).
    fn writer_id(&self) -> Result<WriterId> {
        let known = self.seen(WRITER_KEY)?;
        if known != 0 {
            return Ok(WriterId::from_u64(known));
        }
        // 63 random bits: the store keeps values as SQLite integers and clamps anything above
        // i64::MAX, which would give half of all devices the same id.
        let mut id = 0u64;
        while id == 0 {
            id = WriterId::random().map_err(crypto_error)?.to_u64() & (u64::MAX >> 1);
        }
        self.note_seen(WRITER_KEY, id)?;
        // Another thread may have made one first: the store keeps the larger.
        Ok(WriterId::from_u64(self.seen(WRITER_KEY)?))
    }

    /// Checks a listing's state records against what this device saw before, then remembers
    /// them. Refuses ([`VAULT_ROLLED_BACK`]) a writer's counter that went back, and a sum of all
    /// counters that went down (a record that disappeared): only its own device writes a record,
    /// and only ever forward, so neither happens unless the folder was put back or edited.
    fn check_records(&self, records: &HashMap<WriterId, (String, StateRecord)>) -> Result<()> {
        let mut sum = 0u64;
        for (writer, (_, r)) in records {
            let seen = self.seen(&writer_key(writer))?;
            if r.counter < seen {
                return Err(DriveError::Integrity(format!(
                    "{VAULT_ROLLED_BACK} (record {} is at {}, this device saw {seen})",
                    writer.object_name(),
                    r.counter
                )));
            }
            sum = sum.saturating_add(r.counter);
        }
        let seen_sum = self.seen(SUM_KEY)?;
        if sum < seen_sum {
            return Err(DriveError::Integrity(format!(
                "{VAULT_ROLLED_BACK} (its state records add up to {sum}, this device saw {seen_sum})"
            )));
        }
        for (writer, (_, r)) in records {
            self.note_seen(&writer_key(writer), r.counter)?;
        }
        self.note_seen(SUM_KEY, sum)
    }

    /// Publishes this device's state record after a write (format 2.1 only). Writes that finish
    /// while a record is going up are covered by the next one; a failure leaves it for the next
    /// write or the next run, never failing the write itself (the file is on Drive either way).
    async fn publish_state(&self) {
        if self.write_format() != Format::V2_1 {
            return;
        }
        self.dirty.store(true, Ordering::SeqCst);
        loop {
            let Ok(guard) = self.publishing.try_lock() else {
                // The holder publishes again before it lets go.
                return;
            };
            while self.dirty.swap(false, Ordering::SeqCst) {
                if self.publish_once().await.is_err() {
                    self.dirty.store(true, Ordering::SeqCst);
                    return;
                }
            }
            drop(guard);
            if !self.dirty.load(Ordering::SeqCst) {
                return;
            }
        }
    }

    async fn publish_once(&self) -> Result<()> {
        let writer = self.writer_id()?;
        let (root, digest, existing, listed) = {
            let s = self.st();
            let root = s
                .root_id
                .clone()
                .ok_or_else(|| DriveError::Unknown("the vault root is not known yet".into()))?;
            let digest = set_digest(s.files.values().map(|(p, b)| (p.as_slice(), b.as_str())));
            let own = s.records.get(&writer);
            (
                root,
                digest,
                own.map(|(id, _)| id.clone()),
                own.map_or(0, |(_, r)| r.counter),
            )
        };
        let counter = self
            .seen(&writer_key(&writer))?
            .max(listed)
            .checked_add(1)
            .ok_or_else(|| DriveError::Integrity("the vault state counter is exhausted".into()))?;
        let record = StateRecord {
            writer,
            counter,
            time_ms: now_ms(),
            digest,
        };
        let sealed = record.seal(self.keys.k_state());
        let name = writer.object_name();
        let put = self
            .put_bytes(&sealed, &name, &root, existing.as_deref(), now_ms(), &[])
            .await;
        let info = match put {
            Err(DriveError::NotFound(_)) if existing.is_some() => {
                self.put_bytes(&sealed, &name, &root, None, now_ms(), &[]).await?
            }
            other => other?,
        };
        self.note_seen(&writer_key(&writer), counter)?;
        let sum = {
            let mut s = self.st();
            s.raw_cache.insert(info.id.clone(), (info.version, sealed));
            s.records.insert(writer, (info.id, record));
            s.records
                .values()
                .fold(0u64, |a, (_, r)| a.saturating_add(r.counter))
        };
        self.note_seen(SUM_KEY, sum)
    }

    /// Opens sealed manifest bytes at `at`, refusing a rollback, and records its version.
    fn open_checked(&self, at: &Placement, sealed: &[u8]) -> Result<FileManifest> {
        let m = FileManifest::open(&self.keys, at, sealed).map_err(crypto_error)?;
        m.check_not_older_than(self.highest_seen(&m.file_id)?)
            .map_err(crypto_error)?;
        self.note_version(&m)?;
        Ok(m)
    }

    fn file_item(&self, id: &str) -> Result<Item> {
        match self.st().items.get(id) {
            Some(item @ Item { kind: Kind::File, .. }) => Ok(item.clone()),
            _ => Err(DriveError::Unknown(format!(
                "vault file {id} is not known yet (list the vault first)"
            ))),
        }
    }

    /// The current manifest of the listed file `id` (rollback-checked).
    async fn load_manifest(&self, id: &str) -> Result<(Item, FileManifest)> {
        let item = self.file_item(id)?;
        let sealed = self.fetch(id, "manifest", None, MAX_MANIFEST_BYTES).await?;
        let m = self.open_checked(&item.placement, &sealed)?;
        Ok((item, m))
    }

    /// Fetches `m`'s chunks in order and decrypts them into `dest` (removed on failure).
    async fn decrypt_into(&self, m: FileManifest, dest: &Path, progress: ProgressFn<'_>) -> Result<()> {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(PIPELINE_DEPTH);
        let keys = self.keys.clone();
        let out = dest.to_path_buf();
        let ids: Vec<ChunkId> = m.chunks.iter().map(|c| c.id).collect();
        let task = tokio::task::spawn_blocking(move || -> wasl_crypto::Result<u64> {
            let file = std::fs::File::create(&out)?;
            decrypt_file(
                &keys,
                &m,
                |_| {
                    rx.blocking_recv()
                        .ok_or_else(|| std::io::Error::other("the chunk download stopped"))
                },
                BufWriter::new(file),
            )
        });
        let mut fetch_error = None;
        let mut done = 0u64;
        for id in &ids {
            match self.fetch_chunk(id).await {
                Ok(bytes) => {
                    done += bytes.len() as u64;
                    if tx.send(bytes).await.is_err() {
                        break;
                    }
                    progress(done);
                }
                Err(e) => {
                    fetch_error = Some(e);
                    break;
                }
            }
        }
        drop(tx);
        let decrypted = task
            .await
            .map_err(|e| DriveError::Unknown(format!("decryption task failed: {e}")));
        let result = match (fetch_error, decrypted) {
            (Some(e), _) => Err(e),
            (None, Ok(Ok(_))) => Ok(()),
            (None, Ok(Err(e))) => Err(crypto_error(e)),
            (None, Err(e)) => Err(e),
        };
        if result.is_err() {
            let _ = std::fs::remove_file(dest);
        }
        result
    }

    // ------------------------------------------------------------------ chunks

    /// The shard folder for `shard`, creating `.wasl-chunks` and the shard when needed.
    async fn shard_folder(&self, shard: &str) -> Result<String> {
        if let Some(id) = self.st().shards.get(shard) {
            return Ok(id.clone());
        }
        let _guard = self.layout.lock().await;
        let (root, chunks_dir, known) = {
            let s = self.st();
            (
                s.root_id.clone(),
                s.chunks_dir.clone(),
                s.shards.get(shard).cloned(),
            )
        };
        if let Some(id) = known {
            return Ok(id);
        }
        let chunks_dir = match chunks_dir {
            Some(id) => id,
            None => {
                let root = root.ok_or_else(|| {
                    DriveError::Unknown("the vault root is not known yet (list the vault first)".into())
                })?;
                let info = self.inner.create_folder(&root, CHUNKS_DIR).await?;
                self.st().chunks_dir = Some(info.id.clone());
                info.id
            }
        };
        let info = self.inner.create_folder(&chunks_dir, shard).await?;
        self.st().shards.insert(shard.to_owned(), info.id.clone());
        Ok(info.id)
    }

    async fn put_chunk(&self, id: &ChunkId, sealed: &[u8]) -> Result<()> {
        let shard = self.shard_folder(&id.shard()).await?;
        let name = id.object_name();
        let now = now_ms();
        let info = self.put_bytes(sealed, &name, &shard, None, now, &[]).await?;
        self.st().chunk_objects.insert(
            name,
            (
                info.id,
                ChunkObject {
                    mtime: now,
                    size: Some(sealed.len() as u64),
                },
            ),
        );
        Ok(())
    }

    /// Deletes chunk objects for good (best effort: the sweep catches what is left).
    async fn delete_chunks(&self, ids: &[ChunkId]) {
        for id in ids {
            let name = id.object_name();
            let Some((drive_id, _)) = self.st().chunk_objects.get(&name).cloned() else {
                continue;
            };
            if self.delete_object(&drive_id).await.is_ok() {
                self.st().chunk_objects.remove(&name);
            }
        }
    }

    async fn delete_object(&self, id: &str) -> Result<()> {
        // Drive deletes for good only what is in its bin.
        match self.inner.trash(id).await {
            Ok(()) | Err(DriveError::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
        match self.inner.delete_permanently(id).await {
            Ok(()) | Err(DriveError::NotFound(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    // ------------------------------------------------------------------ rename / move

    /// Renames and/or moves the vault item `id` (a file's manifest or a folder) to `new_name`
    /// in the folder `new_parent_id`, re-sealing what is bound to its placement (ENCRYPTION.md
    /// §11.4–11.6):
    ///
    /// * a folder keeps its Drive id and dir id (its children are untouched); its `wasl.dir`
    ///   property is rewritten in the same update as the name and parent, then its `.wasl-dir`;
    /// * a file's manifest is re-sealed for the new placement (version + 1, same chunks) and
    ///   written as a new Drive file under the new name, then the old manifest is trashed. A crash
    ///   in between leaves both (a copy), never an unreadable file. The returned id is the new one.
    pub async fn move_item(&self, id: &str, new_parent_id: &str, new_name: &str) -> Result<RemoteInfo> {
        let item = self
            .st()
            .items
            .get(id)
            .cloned()
            .ok_or_else(|| DriveError::NotFound(format!("vault item {id}")))?;
        let new_dir = self.dir_of(new_parent_id)?;
        let enc = v2names::encrypt_name(&self.keys, &new_dir, new_name).map_err(crypto_error)?;
        let at = Placement::from_encrypted(&new_dir, &enc)
            .ok_or_else(|| DriveError::Unknown("name did not encrypt".into()))?;
        match item.kind {
            Kind::Dir { dir_id, marker_id } => {
                let prop = dir_id.to_property(&self.keys, &at);
                let parents =
                    (item.parent_id != new_parent_id).then_some((item.parent_id.as_str(), new_parent_id));
                let info = self
                    .inner
                    .update_item(
                        id,
                        ItemUpdate {
                            name: Some(&enc),
                            parents,
                            properties: &[(DIR_PROPERTY, &prop)],
                        },
                    )
                    .await?;
                let marker = v2names::dir_marker(&self.keys, &at, &dir_id);
                let m = self
                    .put_bytes(&marker, DIR_MARKER_NAME, id, marker_id.as_deref(), now_ms(), &[])
                    .await?;
                self.st().items.insert(
                    id.to_owned(),
                    Item {
                        parent_id: new_parent_id.to_owned(),
                        placement: at,
                        kind: Kind::Dir {
                            dir_id,
                            marker_id: Some(m.id),
                        },
                    },
                );
                Ok(RemoteInfo {
                    name: new_name.to_owned(),
                    md5: None,
                    ..info
                })
            }
            Kind::File => {
                let (_, old) = self.load_manifest(id).await?;
                let mut m = old;
                m.version = m
                    .version
                    .checked_add(1)
                    .ok_or_else(|| DriveError::Integrity("file version counter is exhausted".into()))?;
                let sealed = m.seal(&self.keys, &at).map_err(crypto_error)?;
                let blob = m.meta().encode(&self.keys, &at).map_err(crypto_error)?;
                // Drive's time is the upload's: the file's own is only in the manifest (S4-3).
                let info = self
                    .put_bytes(
                        &sealed,
                        &enc,
                        new_parent_id,
                        None,
                        now_ms(),
                        &[(META_PROPERTY, &blob)],
                    )
                    .await?;
                self.note_version(&m)?;
                {
                    let mut s = self.st();
                    s.files.insert(info.id.clone(), (at.to_bytes(), blob.clone()));
                    s.items.insert(
                        info.id.clone(),
                        Item {
                            parent_id: new_parent_id.to_owned(),
                            placement: at,
                            kind: Kind::File,
                        },
                    );
                }
                self.inner.trash(id).await?;
                {
                    let mut s = self.st();
                    s.items.remove(id);
                    s.files.remove(id);
                }
                self.publish_state().await;
                Ok(RemoteInfo {
                    id: info.id,
                    name: new_name.to_owned(),
                    size: Some(m.size),
                    md5: info.md5,
                    mtime: m.mtime_ms,
                    version: info.version,
                })
            }
        }
    }

    // ------------------------------------------------------------------ sweep

    /// Deletes chunk objects that no live manifest references and that were uploaded more than
    /// `grace` ago (see the module docs for what that means for restoring from Drive's bin).
    /// Refuses (integrity error, nothing deleted) while any manifest cannot be read or any vault
    /// item cannot be placed, since their chunks would look unreferenced.
    pub async fn sweep(&self, root_id: &str, drive_id: Option<&str>, grace: Duration) -> Result<SweepReport> {
        self.list_subtree(root_id, drive_id).await?;
        let (files, doubtful, unverified) = {
            let s = self.st();
            let files: Vec<String> = s
                .items
                .iter()
                .filter(|(_, i)| matches!(i.kind, Kind::File))
                .map(|(id, _)| id.clone())
                .collect();
            (files, s.doubtful, s.skips.unverified_folders)
        };
        if doubtful + unverified > 0 {
            return Err(DriveError::Integrity(format!(
                "the chunk sweep did not run: {} vault item(s) could not be placed",
                doubtful + unverified
            )));
        }
        let mut referenced: HashSet<String> = HashSet::new();
        let mut unreadable = 0usize;
        for id in &files {
            match self.load_manifest(id).await {
                Ok((_, m)) => referenced.extend(m.chunks.iter().map(|c| c.id.object_name())),
                Err(DriveError::Integrity(_)) => unreadable += 1,
                Err(e) => return Err(e),
            }
        }
        if unreadable > 0 {
            return Err(DriveError::Integrity(format!(
                "the chunk sweep did not run: {unreadable} file manifest(s) could not be read"
            )));
        }
        let now = now_ms();
        let grace_ms = i64::try_from(grace.as_millis()).unwrap_or(i64::MAX);
        let cutoff = now.saturating_sub(grace_ms);
        let known = self.orphans()?;
        let mut orphans: HashMap<String, i64> = HashMap::new();
        let objects: Vec<(String, String, ChunkObject)> = self
            .st()
            .chunk_objects
            .iter()
            .map(|(n, (id, o))| (n.clone(), id.clone(), *o))
            .collect();
        let mut report = SweepReport::default();
        for (name, drive_id, o) in objects {
            if referenced.contains(&name) {
                report.referenced += 1;
                continue;
            }
            // Unreferenced since the first sweep that saw it so (now, the first time).
            let since = known.get(&name).copied().unwrap_or(now).min(now);
            if since > cutoff || o.mtime > cutoff {
                orphans.insert(name, since);
                report.kept_recent += 1;
            } else {
                self.delete_object(&drive_id).await?;
                self.st().chunk_objects.remove(&name);
                report.deleted += 1;
            }
        }
        self.set_orphans(orphans)?;
        Ok(report)
    }

    /// When each chunk was first seen unreferenced: the store's record when attached, else memory.
    fn orphans(&self) -> Result<HashMap<String, i64>> {
        match &*self.store.lock().unwrap_or_else(|p| p.into_inner()) {
            Some((store, pair)) => store
                .lock()
                .vault_orphans(pair)
                .map_err(|e| DriveError::Unknown(format!("the store failed: {e}"))),
            None => Ok(self.st().orphans.clone()),
        }
    }

    fn set_orphans(&self, orphans: HashMap<String, i64>) -> Result<()> {
        if let Some((store, pair)) = &*self.store.lock().unwrap_or_else(|p| p.into_inner()) {
            store
                .lock()
                .set_vault_orphans(pair, &orphans)
                .map_err(|e| DriveError::Unknown(format!("the store failed: {e}")))?;
        }
        self.st().orphans = orphans;
        Ok(())
    }

    // ------------------------------------------------------------------ on demand

    /// The manifest behind an on-demand [`FileRef`], cached per [`FileRef::version`].
    async fn source_manifest(&self, file: &FileRef) -> Result<Arc<FileManifest>> {
        if let Some((v, m)) = self.st().manifests.get(&file.id)
            && *v == file.version
        {
            return Ok(m.clone());
        }
        let (_, m) = self.load_manifest(&file.id).await?;
        let m = Arc::new(m);
        self.st()
            .manifests
            .insert(file.id.clone(), (file.version.clone(), m.clone()));
        Ok(m)
    }
}

#[async_trait]
impl Remote for VaultV2Remote {
    /// Drive's MD5 is of ciphertext.
    fn plaintext_hashes(&self) -> bool {
        false
    }

    async fn prepare(&self, root_id: &str, drive_id: Option<&str>) -> Result<()> {
        {
            let mut s = self.st();
            s.root_id = Some(root_id.to_owned());
            s.dirs.insert(root_id.to_owned(), DirId::ROOT);
        }
        if let Some(m) = &self.manifest {
            ensure_manifest_v2(self.inner.as_ref(), root_id, drive_id, m, &self.tmp_dir).await?;
        }
        if self.dirty.load(Ordering::SeqCst) {
            // A record that could not go up after the last write: try again now.
            self.publish_state().await;
        }
        Ok(())
    }

    async fn list_subtree(&self, root_id: &str, drive_id: Option<&str>) -> Result<Listing> {
        let raw = self.inner.list_subtree(root_id, drive_id).await?;
        let keys = self.keys.clone();
        let by_key: HashMap<&str, &RemoteEntry> = raw.entries.iter().map(|e| (e.key.as_str(), e)).collect();

        let mut dirs: HashMap<String, DirId> = HashMap::from([(root_id.to_owned(), DirId::ROOT)]);
        let mut plain_of: HashMap<String, String> = HashMap::from([(root_id.to_owned(), String::new())]);
        // Encrypted key → (plaintext key, dir id) of listed folders, for duplicates and shortcuts.
        let mut folder_by_key: HashMap<String, (String, DirId)> =
            HashMap::from([(String::new(), (String::new(), DirId::ROOT))]);
        let mut items: HashMap<String, Item> = HashMap::new();
        let mut chunks_dir: Option<String> = None;
        let mut shards: HashMap<String, String> = HashMap::new();
        let mut shard_ids: HashSet<String> = HashSet::new();
        let mut chunk_objects: HashMap<String, (String, ChunkObject)> = HashMap::new();
        let mut unverified: HashMap<(String, String), String> = HashMap::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut skips = V2Skips::default();
        let mut doubtful = 0usize;
        let mut out = Listing::default();
        let mut files: HashMap<String, (Vec<u8>, String)> = HashMap::new();
        let mut record_entries: Vec<&RemoteEntry> = Vec::new();

        let mut entries: Vec<&RemoteEntry> = raw.entries.iter().collect();
        entries.sort_by_key(|e| e.key.matches('/').count());
        for e in entries {
            let parent = e.parent_id.clone().unwrap_or_default();
            if parent == root_id {
                if e.name == VaultManifestV2::FILE_NAME && !e.is_dir {
                    continue;
                }
                if e.name == CHUNKS_DIR && e.is_dir {
                    chunks_dir = Some(e.id.clone());
                    continue;
                }
                if !e.is_dir && WriterId::from_object_name(&e.name).is_some() {
                    record_entries.push(e);
                    continue;
                }
            }
            if chunks_dir.as_deref() == Some(parent.as_str()) {
                if e.is_dir && is_shard_name(&e.name) {
                    shards.insert(e.name.clone(), e.id.clone());
                    shard_ids.insert(e.id.clone());
                }
                continue;
            }
            if shard_ids.contains(&parent) {
                if !e.is_dir && ChunkId::from_object_name(&e.name).is_some() {
                    chunk_objects.insert(
                        e.name.clone(),
                        (
                            e.id.clone(),
                            ChunkObject {
                                mtime: e.mtime,
                                size: e.size,
                            },
                        ),
                    );
                }
                continue;
            }
            if e.name == DIR_MARKER_NAME && !e.is_dir {
                continue;
            }
            let Some(parent_dir) = dirs.get(&parent).copied() else {
                // Under a skipped folder (or outside the tree).
                continue;
            };
            if e.google_mime.is_some() {
                skips.google_docs += 1;
                continue;
            }
            let Some(name) = v2names::decrypt_name(&keys, &parent_dir, &e.name) else {
                skips.foreign += 1;
                if v2names::is_vault_name(&e.name) {
                    doubtful += 1;
                }
                continue;
            };
            let Some(at) = Placement::from_encrypted(&parent_dir, &e.name) else {
                skips.foreign += 1;
                continue;
            };
            let plain_parent = plain_of.get(&parent).cloned().unwrap_or_default();
            let plain_key = join_key(&plain_parent, &name);
            if !seen.insert(format!(
                "{plain_parent}\u{0}{}",
                wasl_core::identity::name_key(&name)
            )) {
                out.duplicates.push(plain_key);
                doubtful += 1;
                continue;
            }
            let props = raw.properties.get(&e.id);
            if e.is_dir {
                let marker = by_key
                    .get(format!("{}/{DIR_MARKER_NAME}", e.key).as_str())
                    .copied();
                let mut dir_id = props
                    .and_then(|p| p.get(DIR_PROPERTY))
                    .and_then(|v| DirId::from_property(&keys, &at, v));
                if dir_id.is_none()
                    && let Some(m) = marker
                {
                    let read = self.fetch_cached(&m.id, DIR_MARKER_NAME, m.version, m.size, DIR_MARKER_BYTES);
                    match read.await {
                        Ok(bytes) => dir_id = v2names::parse_dir_marker(&keys, &at, &bytes),
                        Err(err) if err.is_transient() => return Err(err),
                        Err(_) => {}
                    }
                }
                let Some(dir_id) = dir_id else {
                    skips.unverified_folders += 1;
                    unverified.insert((parent.clone(), e.name.clone()), e.id.clone());
                    continue;
                };
                dirs.insert(e.id.clone(), dir_id);
                plain_of.insert(e.id.clone(), plain_key.clone());
                folder_by_key.insert(e.key.clone(), (plain_key.clone(), dir_id));
                items.insert(
                    e.id.clone(),
                    Item {
                        parent_id: parent.clone(),
                        placement: at,
                        kind: Kind::Dir {
                            dir_id,
                            marker_id: marker.map(|m| m.id.clone()),
                        },
                    },
                );
                out.entries.push(RemoteEntry {
                    key: plain_key,
                    name,
                    md5: None,
                    ..e.clone()
                });
            } else {
                let blob = props.and_then(|p| p.get(META_PROPERTY));
                files.insert(e.id.clone(), (at.to_bytes(), blob.cloned().unwrap_or_default()));
                let mut meta = blob.and_then(|v| MetaBlob::decode(&keys, &at, v).ok());
                if meta.is_none() {
                    // No listing blob: the manifest says it (and a bad one is listed without a
                    // size, so the download fails for this file only).
                    let read = self.fetch_cached(&e.id, &e.name, e.version, e.size, MAX_MANIFEST_BYTES);
                    match read.await {
                        Ok(bytes) => meta = FileManifest::open(&keys, &at, &bytes).ok().map(|m| m.meta()),
                        Err(err) if err.is_transient() => return Err(err),
                        Err(_) => {}
                    }
                }
                items.insert(
                    e.id.clone(),
                    Item {
                        parent_id: parent.clone(),
                        placement: at,
                        kind: Kind::File,
                    },
                );
                // The manifest's own MD5 stays: the file's identity (module docs).
                out.entries.push(RemoteEntry {
                    key: plain_key,
                    name,
                    size: meta.map(|m| m.size),
                    md5: e.md5.clone(),
                    mtime: meta.map_or(e.mtime, |m| m.mtime_ms),
                    ..e.clone()
                });
            }
        }
        let plain_child = |enc_key: &str| -> Option<String> {
            let (enc_parent, enc_name) = enc_key.rsplit_once('/').unwrap_or(("", enc_key));
            let (plain_parent, dir) = folder_by_key.get(enc_parent)?;
            let name = v2names::decrypt_name(&keys, dir, enc_name)?;
            Some(join_key(plain_parent, &name))
        };
        for d in &raw.duplicates {
            if let Some(k) = plain_child(d) {
                out.duplicates.push(k);
                doubtful += 1;
            }
        }
        out.shortcuts = raw.shortcuts.iter().filter_map(|s| plain_child(s)).collect();

        // State records (S4-4): verified, then checked against what this device saw before
        // anything of this listing is used. One that does not verify (another vault's, junk) is
        // skipped like any foreign item: putting junk in place of a genuine record only makes
        // that record disappear, which the sum check below refuses.
        let mut records: HashMap<WriterId, (String, StateRecord)> = HashMap::new();
        for e in record_entries {
            let bytes = match self
                .fetch_cached(&e.id, &e.name, e.version, e.size, RECORD_BYTES)
                .await
            {
                Ok(bytes) => bytes,
                Err(err) if err.is_transient() => return Err(err),
                Err(_) => {
                    skips.foreign += 1;
                    continue;
                }
            };
            let Ok(r) = StateRecord::open(keys.k_state(), &e.name, &bytes) else {
                skips.foreign += 1;
                continue;
            };
            // Two objects of one writer (a copy put next to it): the higher counter counts.
            if records
                .get(&r.writer)
                .is_none_or(|(_, old)| old.counter < r.counter)
            {
                records.insert(r.writer, (e.id.clone(), r));
            }
        }
        self.check_records(&records)?;
        let state_check = match records.values().max_by_key(|(_, r)| (r.time_ms, r.writer)) {
            None => StateCheck::NoRecords,
            Some((_, newest)) => {
                let now = set_digest(files.values().map(|(p, b)| (p.as_slice(), b.as_str())));
                if now == newest.digest {
                    StateCheck::Consistent
                } else {
                    StateCheck::Unpublished
                }
            }
        };

        let mut s = self.st();
        s.root_id = Some(root_id.to_owned());
        s.dirs = dirs;
        s.items = items;
        s.chunks_dir = chunks_dir;
        s.shards = shards;
        s.chunk_objects = chunk_objects;
        s.unverified = unverified;
        s.skips = skips;
        s.doubtful = doubtful;
        s.files = files;
        s.records = records;
        s.state_check = state_check;
        let live: HashSet<&String> = by_key.values().map(|e| &e.id).collect();
        s.raw_cache.retain(|id, _| live.contains(id));
        s.listed_sizes = raw
            .entries
            .iter()
            .filter_map(|e| Some((e.id.clone(), e.size?)))
            .collect();
        Ok(out)
    }

    async fn download(&self, entry: &RemoteEntry, dest: &Path, progress: ProgressFn<'_>) -> Result<()> {
        let (_, m) = match self.load_manifest(&entry.id).await {
            Ok(x) => x,
            Err(e) => {
                let _ = std::fs::remove_file(dest);
                return Err(e);
            }
        };
        self.decrypt_into(m, dest, progress).await
    }

    async fn export(
        &self,
        _entry: &RemoteEntry,
        _mime: &str,
        _dest: &Path,
        _progress: ProgressFn<'_>,
    ) -> Result<()> {
        Err(DriveError::Unknown(DOCS_UNSUPPORTED.into()))
    }

    async fn upload(&self, spec: UploadSpec<'_>, progress: ProgressFn<'_>) -> Result<RemoteInfo> {
        let parent_dir = self.dir_of(spec.parent_id)?;
        let enc = v2names::encrypt_name(&self.keys, &parent_dir, spec.name).map_err(crypto_error)?;
        let at = Placement::from_encrypted(&parent_dir, &enc)
            .ok_or_else(|| DriveError::Unknown("name did not encrypt".into()))?;
        let previous = match spec.existing_id {
            Some(id) => match self.load_manifest(id).await {
                Ok((_, m)) => Some(m),
                // A manifest that does not open cannot be updated: the new content becomes a new
                // file (new file id) in the same Drive file; its old chunks go with the sweep.
                Err(DriveError::Integrity(_)) => None,
                Err(e) => return Err(e),
            },
            None => None,
        };

        let old_ids: HashSet<ChunkId> = previous
            .iter()
            .flat_map(|m| m.chunks.iter().map(|c| c.id))
            .collect();
        let present: HashSet<String> = self.st().chunk_objects.keys().cloned().collect();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(ChunkId, Vec<u8>)>(PIPELINE_DEPTH);
        let keys = self.keys.clone();
        let src = spec.src.to_path_buf();
        let mtime_ms = spec.mtime_ms;
        let format = self.write_format();
        let prev = previous.clone();
        let task = tokio::task::spawn_blocking(move || -> wasl_crypto::Result<FileManifest> {
            let input = BufReader::new(std::fs::File::open(&src)?);
            let sink = |c: PendingChunk<'_>| -> wasl_crypto::Result<()> {
                if old_ids.contains(c.id()) || present.contains(&c.id().object_name()) {
                    return Ok(());
                }
                let sealed = c.seal()?;
                tx.blocking_send((*c.id(), sealed))
                    .map_err(|_| CryptoError::Io(std::io::Error::other("the chunk upload stopped")))
            };
            match prev {
                Some(p) => encrypt_update(&keys, format, &p, mtime_ms, input, sink),
                None => encrypt_stream(&keys, format, FileId::random()?, mtime_ms, input, sink),
            }
        });
        let mut sent = 0u64;
        while let Some((id, sealed)) = rx.recv().await {
            // Returning drops `rx`: the encryptor stops at its next chunk.
            self.put_chunk(&id, &sealed).await?;
            sent += sealed.len() as u64;
            progress(sent);
        }
        let manifest = task
            .await
            .map_err(|e| DriveError::Unknown(format!("encryption task failed: {e}")))?
            .map_err(crypto_error)?;

        let sealed = manifest.seal(&self.keys, &at).map_err(crypto_error)?;
        let blob = manifest.meta().encode(&self.keys, &at).map_err(crypto_error)?;
        // Drive's modification time is the upload's, not the file's (S4-3): the real time is in
        // the manifest and `wasl.m`, which is what listings report and what the engine keeps.
        let info = self
            .put_bytes(
                &sealed,
                &enc,
                spec.parent_id,
                spec.existing_id,
                now_ms(),
                &[(META_PROPERTY, &blob)],
            )
            .await?;
        self.note_version(&manifest)?;
        {
            let mut s = self.st();
            s.files.insert(info.id.clone(), (at.to_bytes(), blob));
            s.items.insert(
                info.id.clone(),
                Item {
                    parent_id: spec.parent_id.to_owned(),
                    placement: at,
                    kind: Kind::File,
                },
            );
        }
        if let Some(old) = &previous {
            self.delete_chunks(&plan_update(old, &manifest).delete).await;
        }
        self.publish_state().await;
        // The manifest's MD5, as the next listing reports it (module docs).
        Ok(RemoteInfo {
            id: info.id,
            name: spec.name.to_owned(),
            size: Some(manifest.size),
            md5: info.md5,
            mtime: spec.mtime_ms,
            version: info.version,
        })
    }

    async fn create_folder(&self, parent_id: &str, name: &str) -> Result<RemoteInfo> {
        let parent_dir = self.dir_of(parent_id)?;
        let enc = v2names::encrypt_name(&self.keys, &parent_dir, name).map_err(crypto_error)?;
        let at = Placement::from_encrypted(&parent_dir, &enc)
            .ok_or_else(|| DriveError::Unknown("name did not encrypt".into()))?;
        let dir_id = DirId::random().map_err(crypto_error)?;
        let prop = dir_id.to_property(&self.keys, &at);
        let reuse = self.st().unverified.remove(&(parent_id.to_owned(), enc.clone()));
        let info = match reuse {
            // A folder left without a usable id (a crash before its marker): give it one. Its
            // old children, if any, no longer decrypt and stay ignored.
            Some(id) => {
                let update = ItemUpdate {
                    properties: &[(DIR_PROPERTY, &prop)],
                    ..ItemUpdate::default()
                };
                match self.inner.update_item(&id, update).await {
                    Ok(info) => info,
                    Err(_) => RemoteInfo {
                        id,
                        name: enc.clone(),
                        size: None,
                        md5: None,
                        mtime: now_ms(),
                        version: None,
                    },
                }
            }
            None => {
                self.inner
                    .create_folder_with_properties(parent_id, &enc, &[(DIR_PROPERTY, &prop)])
                    .await?
            }
        };
        let marker = v2names::dir_marker(&self.keys, &at, &dir_id);
        let m = self
            .put_bytes(&marker, DIR_MARKER_NAME, &info.id, None, now_ms(), &[])
            .await?;
        {
            let mut s = self.st();
            s.dirs.insert(info.id.clone(), dir_id);
            s.items.insert(
                info.id.clone(),
                Item {
                    parent_id: parent_id.to_owned(),
                    placement: at,
                    kind: Kind::Dir {
                        dir_id,
                        marker_id: Some(m.id),
                    },
                },
            );
        }
        Ok(RemoteInfo {
            name: name.to_owned(),
            md5: None,
            ..info
        })
    }

    /// Trashes the manifest (or folder); chunks stay for the sweep, so restoring from Drive's bin
    /// works (module docs).
    async fn trash(&self, id: &str) -> Result<()> {
        self.inner.trash(id).await?;
        {
            let mut s = self.st();
            // The files under a trashed folder leave the live set with it.
            let gone: Vec<String> = s
                .files
                .keys()
                .filter(|f| f.as_str() == id || is_under(&s.items, f, id))
                .cloned()
                .collect();
            for f in gone {
                s.files.remove(&f);
            }
            s.items.remove(id);
            s.dirs.remove(id);
            s.manifests.remove(id);
        }
        self.publish_state().await;
        Ok(())
    }

    async fn untrash(&self, id: &str) -> Result<()> {
        self.inner.untrash(id).await
    }

    async fn delete_permanently(&self, id: &str) -> Result<()> {
        self.inner.delete_permanently(id).await
    }

    async fn list_revisions(&self, id: &str) -> Result<Vec<Revision>> {
        // A manifest revision's size and checksum say nothing about the file.
        Ok(self
            .inner
            .list_revisions(id)
            .await?
            .into_iter()
            .map(|r| Revision {
                size: None,
                md5: None,
                ..r
            })
            .collect())
    }

    /// An older manifest revision, decrypted. No rollback check (the person asked for it); it
    /// fails with an integrity error when its chunks are no longer stored.
    async fn download_revision(&self, id: &str, revision_id: &str, dest: &Path) -> Result<()> {
        let item = self.file_item(id)?;
        let temp = self.temp_path()?;
        let fetched = self.inner.download_revision(id, revision_id, &temp).await;
        let bytes = fetched.and_then(|()| std::fs::read(&temp).map_err(io_error));
        let _ = std::fs::remove_file(&temp);
        let m = FileManifest::open(&self.keys, &item.placement, &bytes?).map_err(crypto_error)?;
        self.decrypt_into(m, dest, &|_| {}).await
    }

    async fn keep_revision(&self, id: &str, revision_id: &str, keep: bool) -> Result<()> {
        self.inner.keep_revision(id, revision_id, keep).await
    }

    async fn housekeeping(&self, root_id: &str, drive_id: Option<&str>) -> Result<()> {
        self.sweep(root_id, drive_id, self.grace).await.map(|_| ())
    }
}

#[async_trait]
impl Source for VaultV2Remote {
    async fn read(&self, file: &FileRef, offset: u64, len: u64) -> std::result::Result<Vec<u8>, VfsError> {
        let m = self.source_manifest(file).await.map_err(vfs_error)?;
        let reader = RangeReader::new(&self.keys, &m);
        let mut fetched: HashMap<ChunkId, Vec<u8>> = HashMap::new();
        for i in reader.covering(offset, len) {
            let id = m.chunks[i].id;
            if let std::collections::hash_map::Entry::Vacant(v) = fetched.entry(id) {
                v.insert(self.fetch_chunk(&id).await.map_err(vfs_error)?);
            }
        }
        reader
            .read(offset, len, |id| {
                fetched
                    .get(id)
                    .cloned()
                    .ok_or_else(|| std::io::Error::other("chunk was not fetched"))
            })
            .map_err(|e| vfs_error(crypto_error(e)))
    }

    /// Widens to the chunks covering the range, once the file's manifest has been read.
    fn natural_range(&self, file: &FileRef, offset: u64, len: u64) -> (u64, u64) {
        let s = self.st();
        let Some((v, m)) = s.manifests.get(&file.id) else {
            return (offset, len);
        };
        if *v != file.version {
            return (offset, len);
        }
        let reader = RangeReader::new(&self.keys, m);
        let range = reader.covering(offset, len);
        if range.is_empty() {
            return (offset, len);
        }
        let (start, _) = reader.chunk_span(range.start).unwrap_or((offset, 0));
        let (last, last_len) = reader.chunk_span(range.end - 1).unwrap_or((offset, 0));
        (start, last + u64::from(last_len) - start)
    }
}

fn too_big(name: &str, max: u64) -> DriveError {
    DriveError::Integrity(format!(
        "vault object {name} on Drive is bigger than the {max} bytes it can be"
    ))
}

/// The file's content, or `None` when it holds more than `max` bytes (only `max + 1` are read).
fn read_at_most(path: &Path, max: u64) -> std::io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.take(max + 1).read_to_end(&mut bytes)?;
    Ok((bytes.len() as u64 <= max).then_some(bytes))
}

fn vfs_error(e: DriveError) -> VfsError {
    match e {
        DriveError::Integrity(m) => VfsError::Integrity(m),
        other => VfsError::Unavailable(other.to_string()),
    }
}

/// Makes sure the Drive folder `folder_id` belongs to the v2 vault `manifest` before a v2 pair
/// syncs: an existing `vault.json` must be v2 with the same salt, cost and key check (a v1 one is
/// refused with [`VAULT_IS_V1`]); an empty folder gets `manifest` uploaded; a non-empty folder
/// without one is refused. `inner` is the plain (unencrypted) remote.
pub async fn ensure_manifest_v2(
    inner: &dyn Remote,
    folder_id: &str,
    drive_id: Option<&str>,
    manifest: &VaultManifestV2,
    tmp_dir: &Path,
) -> Result<ManifestStatus> {
    let listing = inner.list_subtree(folder_id, drive_id).await?;
    let existing = listing
        .entries
        .iter()
        .find(|e| e.key == VaultManifestV2::FILE_NAME && !e.is_dir && e.google_mime.is_none());
    std::fs::create_dir_all(tmp_dir).map_err(io_error)?;
    let r: [u8; 12] = random_bytes()?;
    let temp = tmp_dir.join(format!("{TEMP_PREFIX}vault-{}.json", hex(&r)));
    let outcome = async {
        match existing {
            Some(entry) => {
                inner.download(entry, &temp, &|_| {}).await?;
                let text = std::fs::read_to_string(&temp)
                    .map_err(|e| DriveError::Integrity(format!("vault.json cannot be read as text ({e})")))?;
                if vault_json_format(&text).as_deref() == Some(wasl_crypto::VaultManifest::FORMAT) {
                    return Err(DriveError::Integrity(VAULT_IS_V1.into()));
                }
                let remote = VaultManifestV2::parse(&text).map_err(crypto_error)?;
                if !remote.same_vault_as(manifest) {
                    return Err(DriveError::Integrity(DIFFERENT_VAULT.into()));
                }
                Ok(ManifestStatus::Verified)
            }
            None => {
                if !listing.entries.is_empty()
                    || !listing.duplicates.is_empty()
                    || !listing.shortcuts.is_empty()
                {
                    return Err(DriveError::Integrity(NEEDS_EMPTY_FOLDER.into()));
                }
                std::fs::write(&temp, manifest.to_json()).map_err(io_error)?;
                inner
                    .upload(
                        UploadSpec {
                            src: &temp,
                            name: VaultManifestV2::FILE_NAME,
                            parent_id: folder_id,
                            existing_id: None,
                            mtime_ms: now_ms(),
                            session_key: None,
                        },
                        &|_| {},
                    )
                    .await?;
                Ok(ManifestStatus::Created)
            }
        }
    }
    .await;
    let _ = std::fs::remove_file(&temp);
    outcome
}

/// What [`copy_vault`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CopyReport {
    /// Folders made in the new vault.
    pub folders: usize,
    /// Files copied.
    pub files: usize,
    /// Plaintext bytes copied.
    pub bytes: u64,
    /// Files the new vault already had from an earlier, interrupted copy (same size and time).
    pub already_there: usize,
    /// Items not copied because the old vault's listing could not place them (same names in one
    /// folder, a file where the new vault has a folder): their keys, as listed.
    pub left_out: Vec<String>,
    /// Files of the old vault that did not decrypt (damaged or tampered): their keys. Everything
    /// else was copied; these stay readable only as far as they ever were, in the old folder.
    pub failed: Vec<String>,
}

/// Copies every folder and file of a vault into another, new v2 vault: how a vault of format v1
/// or 2.0 moves to 2.1 (security review S4-2), since an existing vault's chunks cannot change
/// format in place without every device agreeing first.
///
/// `from` is the old vault's remote ([`crate::VaultRemote`] for v1, a [`VaultV2Remote`] for 2.0),
/// `to` the new vault's [`VaultV2Remote`] with its `vault.json` ([`VaultV2Remote::with_manifest`]),
/// so each side does its own decryption and encryption. Plaintext passes through `tmp`, one file at
/// a time, removed as soon as it is uploaded (`tmp` should be in the app's private data folder).
/// Each file keeps its modification time.
///
/// Restartable: a file the new vault already lists with the same size and time is not copied
/// again, and a folder it already has is reused, so running it again after a crash or a lost
/// connection continues where it stopped. A transient Drive error stops the copy (run it again);
/// a file of the old vault that does not decrypt is listed in [`CopyReport::failed`] and the copy
/// goes on. Nothing in the old vault's folder is ever changed or deleted.
pub async fn copy_vault(
    from: &dyn Remote,
    from_root: &str,
    from_drive: Option<&str>,
    to: &VaultV2Remote,
    to_root: &str,
    to_drive: Option<&str>,
    tmp: &Path,
) -> Result<CopyReport> {
    from.prepare(from_root, from_drive).await?;
    to.prepare(to_root, to_drive).await?;
    let source = from.list_subtree(from_root, from_drive).await?;
    let target = to.list_subtree(to_root, to_drive).await?;
    let there: HashMap<String, RemoteEntry> =
        target.entries.into_iter().map(|e| (e.key.clone(), e)).collect();
    let mut report = CopyReport {
        left_out: source.duplicates.clone(),
        ..CopyReport::default()
    };
    let mut folders: HashMap<String, String> = HashMap::from([(String::new(), to_root.to_owned())]);
    let mut entries = source.entries;
    entries.sort_by(|a, b| (a.key.matches('/').count(), &a.key).cmp(&(b.key.matches('/').count(), &b.key)));
    std::fs::create_dir_all(tmp).map_err(io_error)?;
    for e in entries {
        let (parent_key, name) = e.key.rsplit_once('/').unwrap_or(("", e.key.as_str()));
        let (Some(parent), None) = (folders.get(parent_key).cloned(), e.google_mime.as_ref()) else {
            report.left_out.push(e.key.clone());
            continue;
        };
        let existing = there.get(&e.key);
        if e.is_dir {
            let id = match existing {
                Some(d) if d.is_dir => d.id.clone(),
                Some(_) => {
                    report.left_out.push(e.key.clone());
                    continue;
                }
                None => {
                    report.folders += 1;
                    to.create_folder(&parent, name).await?.id
                }
            };
            folders.insert(e.key.clone(), id);
            continue;
        }
        let existing = match existing {
            Some(t) if t.is_dir => {
                report.left_out.push(e.key.clone());
                continue;
            }
            Some(t) if t.size == e.size && t.mtime == e.mtime => {
                report.already_there += 1;
                continue;
            }
            other => other,
        };
        let r: [u8; 12] = random_bytes()?;
        let temp = tmp.join(format!("{TEMP_PREFIX}copy-{}", hex(&r)));
        let copied = async {
            from.download(&e, &temp, &|_| {}).await?;
            to.upload(
                UploadSpec {
                    src: &temp,
                    name,
                    parent_id: &parent,
                    existing_id: existing.map(|t| t.id.as_str()),
                    mtime_ms: e.mtime,
                    session_key: None,
                },
                &|_| {},
            )
            .await
        }
        .await;
        let _ = std::fs::remove_file(&temp);
        match copied {
            Ok(info) => {
                report.files += 1;
                report.bytes += info.size.unwrap_or(0);
            }
            Err(DriveError::Integrity(_)) => report.failed.push(e.key.clone()),
            Err(err) => return Err(err),
        }
    }
    Ok(report)
}

/// The `format` field of a vault.json, if it is JSON with one.
pub(crate) fn vault_json_format(text: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    v.get("format")?.as_str().map(str::to_owned)
}

/// Store keys of the state-record bookkeeping (`vault_versions`, next to hex file ids, which are
/// 32 hex digits and never collide with these).
const WRITER_KEY: &str = "state-writer";
const SUM_KEY: &str = "state-sum";

fn writer_key(w: &WriterId) -> String {
    format!("state-w-{}", hex(&w.0))
}

/// Whether the item `id` lies (at any depth) under the folder `ancestor`.
fn is_under(items: &HashMap<String, Item>, id: &str, ancestor: &str) -> bool {
    let mut at = id;
    // A folder chain is never deeper than the items known; the bound stops a cycle.
    for _ in 0..=items.len() {
        match items.get(at) {
            Some(item) if item.parent_id == ancestor => return true,
            Some(item) => at = &item.parent_id,
            None => return false,
        }
    }
    false
}

fn is_shard_name(name: &str) -> bool {
    name.len() == 2 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn join_key(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut out = [0u8; N];
    let mut filled = 0;
    while filled < N {
        let id = FileId::random().map_err(crypto_error)?;
        let take = (N - filled).min(id.0.len());
        out[filled..filled + take].copy_from_slice(&id.0[..take]);
        filled += take;
    }
    Ok(out)
}

/// Removes temporaries left by crashed runs.
fn purge_stale_temps(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let now = SystemTime::now();
    for e in rd.flatten() {
        if !e.file_name().to_string_lossy().starts_with(TEMP_PREFIX) {
            continue;
        }
        let age = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok());
        if age.is_some_and(|a| a > TEMP_MAX_AGE) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}
