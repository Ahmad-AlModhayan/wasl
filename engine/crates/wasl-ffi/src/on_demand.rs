//! Files on demand for the phone (docs/ON_DEMAND.md §2.2, Android row): byte ranges of a Drive
//! file or of a vault file, read through one [`Hydrator`] per origin over one bounded
//! [`BlockCache`], so the DocumentsProvider fetches only the blocks (vault v2: the chunks) a reader
//! touches, and a pinned file stays complete on the phone.
//!
//! Sources: a plain Drive file reads with range requests ([`wasl_engine::DriveSource`]); a vault v1
//! file a 1 MiB chunk at a time ([`VaultV1Source`]); a vault v2 file through its manifest and only
//! the chunks covering the range ([`VaultV2Remote`], which must have listed its folder first). A
//! vault file's blocks are cached under a name of their own, so the same Drive file read as plain
//! (ciphertext) and through its vault (plaintext) never share a block.
//!
//! The reader never writes to Drive: nothing here uploads, and vaults are opened without their
//! `vault.json` check, which could create one in an empty folder.
//!
//! **Locking.** Every read and pin of a vault file first asks whether the vault is still unlocked
//! on the device ([`OnDemandRemotes::vault_unlocked`], a cheap check), and a locked vault fails
//! with `Locked`. [`OnDemandReader::close_vault`] (called when a vault is locked or removed) drops
//! the vault's opened state, keys included, and forgets the decrypted blocks read through it.
//! Which files those are is kept per vault next to the cache (`vaults/<vault id>.json`: ids and
//! versions only, no names), so blocks cached by an earlier run go too; if that record cannot be
//! read, closing the vault evicts every unpinned block of the cache. Pinned vault files stay:
//! the person asked to keep them on the device, as a synced vault pair keeps its plaintext folder
//! (local plaintext is outside the vault's threat model, docs/ENCRYPTION.md §7). A read that was
//! running while its vault closed forgets what it fetched and fails `Locked`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use wasl_core::RemoteEntry;
use wasl_crypto::v2::VaultKeysV2;
use wasl_crypto::{VaultCipher, VaultKeys};
use wasl_engine::{EngineError, Remote, VaultRemote, VaultV1Source, VaultV2Remote};
use wasl_vfs::{BlockCache, FileRef, Hydrator, ReadAhead, Source, VfsError};
use zeroize::Zeroizing;

use crate::{VaultAccess, WaslError, vaults};

/// The phone keeps at most this much unpinned content unless told otherwise (docs/ON_DEMAND.md
/// §2.1: 2 GB on the phone).
pub const DEFAULT_CACHE_LIMIT: u64 = 2 << 30;

/// Prefix of a vault file's id in the cache: the plaintext of a Drive file is not its ciphertext.
const VAULT_PREFIX: &str = "vault:";

/// One version of one remote file.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OnDemandFile {
    /// The Drive file id (a vault v2 file: its manifest's Drive id, as listed).
    pub id: String,
    /// Changes whenever the content does (Drive's version, or its MD5).
    pub version: String,
    /// The size a reader sees (a vault file: its plaintext size).
    pub size: u64,
}

/// Where a file's bytes come from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum OnDemandOrigin {
    /// A plain file of the account's Drive.
    Drive { account: String },
    /// A file of the encrypted vault (v1 or v2) kept in the Drive folder `folder_id`.
    Vault {
        account: String,
        vault_id: String,
        folder_id: String,
        drive_id: Option<String>,
    },
}

/// A file or folder of a vault folder, with its name decrypted.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct VaultItem {
    /// `/`-separated path below the vault folder, plaintext names.
    pub path: String,
    pub name: String,
    /// The Drive id (a v2 file: its manifest's).
    pub id: String,
    pub is_dir: bool,
    /// Plaintext size; 0 for folders.
    pub size: u64,
    pub mtime: i64,
    /// What [`OnDemandFile::version`] is for this item.
    pub version: String,
}

/// What the cache holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct CacheUsage {
    /// Bytes on disk, pinned included.
    pub used: u64,
    /// Bytes of pinned files (never evicted).
    pub pinned: u64,
    /// Bytes of unpinned content kept at most.
    pub limit: u64,
}

/// What a reader needs from the app: an account's Drive and a vault's keys. The engine provides
/// it in the app; tests use an in-memory Drive.
pub trait OnDemandRemotes: Send + Sync {
    /// The plain (unencrypted) remote of an account.
    fn remote(&self, account: &str) -> Result<Arc<dyn Remote>, WaslError>;
    /// Where an account's plain files are read from (Drive range requests).
    fn source(&self, account: &str) -> Result<Arc<dyn Source>, WaslError>;
    /// An unlocked vault's manifest and key material; `None` while it is locked. May block.
    fn vault_access(&self, vault_id: &str) -> Option<VaultAccess>;
    /// Whether the vault is unlocked on this device, asked before every vault read: should be
    /// cheap (no key unwrapped). By default, whether [`Self::vault_access`] gives keys.
    fn vault_unlocked(&self, vault_id: &str) -> bool {
        self.vault_access(vault_id).is_some()
    }
}

/// The app's cheap answer to "is this vault unlocked here" (its key file exists), so a vault read
/// re-checks the lock without unwrapping keys.
#[uniffi::export(with_foreign)]
pub trait VaultLocks: Send + Sync {
    fn is_unlocked(&self, vault_id: String) -> bool;
}

/// One file version read through a vault: `(cache id, version, size)`.
type Seen = (String, String, u64);

/// A vault file's id in the cache, unwrapped before its source sees it.
struct Namespaced(Arc<dyn Source>);

fn unwrapped(f: &FileRef) -> FileRef {
    FileRef {
        id: f.id.strip_prefix(VAULT_PREFIX).unwrap_or(&f.id).to_owned(),
        version: f.version.clone(),
        size: f.size,
    }
}

#[async_trait]
impl Source for Namespaced {
    async fn read(&self, file: &FileRef, offset: u64, len: u64) -> Result<Vec<u8>, VfsError> {
        self.0.read(&unwrapped(file), offset, len).await
    }

    fn natural_range(&self, file: &FileRef, offset: u64, len: u64) -> (u64, u64) {
        self.0.natural_range(&unwrapped(file), offset, len)
    }
}

/// For cache-only calls (unpin, completeness): never asked for bytes.
struct NoSource;

#[async_trait]
impl Source for NoSource {
    async fn read(&self, file: &FileRef, _offset: u64, _len: u64) -> Result<Vec<u8>, VfsError> {
        Err(VfsError::Unavailable(format!("{} is not open", file.id)))
    }
}

/// A vault opened for reading: what lists its names, and whether a v2 vault has listed yet (its
/// reads need the listing).
struct VaultSide {
    lister: Arc<dyn Remote>,
    v2: bool,
    listed: tokio::sync::Mutex<bool>,
    folder_id: String,
    drive_id: Option<String>,
}

struct Opened {
    hydrator: Hydrator,
    vault: Option<VaultSide>,
}

/// Byte ranges of on-demand files, pins, and the decrypted listing of vault folders.
#[derive(uniffi::Object)]
pub struct OnDemandReader {
    cache: Arc<BlockCache>,
    remotes: Arc<dyn OnDemandRemotes>,
    tmp: PathBuf,
    /// `<dir>/vaults`: per vault, the file versions read through it (to forget on lock).
    seen_dir: PathBuf,
    opened: Mutex<HashMap<OnDemandOrigin, Arc<Opened>>>,
    /// Per vault id, the file versions read through it (loaded from `seen_dir` on first use).
    seen: Mutex<HashMap<String, SeenRecord>>,
    /// Per vault id, bumped by every close. A whole close runs under this lock, so a read that
    /// compares the count before returning bytes never misses a close that overlapped it.
    closes: Mutex<HashMap<String, u64>>,
}

/// What a vault's blocks are, as far as the reader knows.
#[derive(Default)]
struct SeenRecord {
    files: HashSet<Seen>,
    /// The record on disk could not be read (damaged, unreadable): what earlier runs read through
    /// the vault is unknown. It is not overwritten, and closing the vault then evicts every
    /// unpinned block of the cache instead of the vault's alone.
    unknown: bool,
}

impl std::fmt::Debug for OnDemandReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OnDemandReader { .. }")
    }
}

fn vfs(e: VfsError) -> WaslError {
    match e {
        VfsError::Unavailable(detail) => WaslError::Network { detail },
        VfsError::Integrity(detail) => WaslError::Io { detail },
        VfsError::Cache(e) => WaslError::Io {
            detail: format!("The on-demand cache could not be used: {e}"),
        },
        VfsError::OutOfRange => WaslError::Other {
            detail: "Read past the end of the file".into(),
        },
    }
}

fn drive(e: wasl_drive::DriveError) -> WaslError {
    WaslError::from(EngineError::Drive(e))
}

fn locked() -> WaslError {
    WaslError::Locked {
        detail: "The vault is locked on this device. Unlock it to open its files.".into(),
    }
}

/// A file's content identity from a listing: Drive's version, else its MD5, else size and time.
fn version_of(e: &RemoteEntry) -> String {
    e.version
        .map(|v| v.to_string())
        .or_else(|| e.md5.clone())
        .unwrap_or_else(|| format!("{}-{}", e.size.unwrap_or(0), e.mtime))
}

/// One path component made of an id, for a folder name.
fn folder_name(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

impl OnDemandReader {
    /// A reader keeping its cache in `dir` (`dir/blocks`, temporaries in `dir/tmp`), at most
    /// `limit` bytes of unpinned content. One reader per folder.
    pub fn open(
        dir: impl Into<PathBuf>,
        limit: u64,
        remotes: Arc<dyn OnDemandRemotes>,
    ) -> Result<Arc<OnDemandReader>, WaslError> {
        let dir = dir.into();
        let cache = BlockCache::open(dir.join("blocks"), limit).map_err(|e| WaslError::Io {
            detail: format!("The on-demand cache can't be opened: {e}"),
        })?;
        Ok(Arc::new(OnDemandReader {
            cache: Arc::new(cache),
            remotes,
            tmp: dir.join("tmp"),
            seen_dir: dir.join("vaults"),
            opened: Mutex::new(HashMap::new()),
            seen: Mutex::new(HashMap::new()),
            closes: Mutex::new(HashMap::new()),
        }))
    }

    fn opened_map(&self) -> std::sync::MutexGuard<'_, HashMap<OnDemandOrigin, Arc<Opened>>> {
        self.opened.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn seen_map(&self) -> std::sync::MutexGuard<'_, HashMap<String, SeenRecord>> {
        self.seen.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn closes_map(&self) -> std::sync::MutexGuard<'_, HashMap<String, u64>> {
        self.closes.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn closes_of(&self, vault_id: &str) -> u64 {
        self.closes_map().get(vault_id).copied().unwrap_or(0)
    }

    fn seen_file(&self, vault_id: &str) -> PathBuf {
        self.seen_dir.join(format!("{}.json", folder_name(vault_id)))
    }

    /// The vault's read file versions, from memory or its file (`seen` locked by the caller). Only
    /// a missing file means "none"; any other failure makes the record unknown (fail closed).
    fn seen_of<'a>(&self, seen: &'a mut HashMap<String, SeenRecord>, vault_id: &str) -> &'a mut SeenRecord {
        seen.entry(vault_id.to_owned())
            .or_insert_with(|| match std::fs::read(self.seen_file(vault_id)) {
                Ok(b) => match serde_json::from_slice::<Vec<Seen>>(&b) {
                    Ok(list) => SeenRecord {
                        files: list.into_iter().collect(),
                        unknown: false,
                    },
                    Err(_) => SeenRecord {
                        files: HashSet::new(),
                        unknown: true,
                    },
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => SeenRecord::default(),
                Err(_) => SeenRecord {
                    files: HashSet::new(),
                    unknown: true,
                },
            })
    }

    fn write_seen(&self, vault_id: &str, files: &HashSet<Seen>) -> Result<(), WaslError> {
        let path = self.seen_file(vault_id);
        let io = |e: std::io::Error| WaslError::Io {
            detail: format!("The on-demand cache could not be used: {e}"),
        };
        if files.is_empty() {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io(e)),
                _ => Ok(()),
            };
        }
        std::fs::create_dir_all(&self.seen_dir).map_err(io)?;
        let mut list: Vec<&Seen> = files.iter().collect();
        list.sort();
        let tmp = path.with_extension("json.part");
        let bytes = serde_json::to_vec(&list).map_err(|e| io(std::io::Error::other(e)))?;
        std::fs::write(&tmp, bytes).map_err(io)?;
        std::fs::rename(&tmp, &path).map_err(io)
    }

    /// Notes that `f` is read through the vault, before its blocks are cached, so a lock forgets
    /// them.
    fn remember(&self, vault_id: &str, f: &FileRef) -> Result<(), WaslError> {
        let mut seen = self.seen_map();
        let record = self.seen_of(&mut seen, vault_id);
        // An unknown record is left as it is on disk: closing the vault evicts everything then.
        if record.files.insert((f.id.clone(), f.version.clone(), f.size)) && !record.unknown {
            let snapshot = record.files.clone();
            self.write_seen(vault_id, &snapshot)?;
        }
        Ok(())
    }

    /// Forgets every unpinned block read through the vault; pinned files stay (see the module
    /// docs). When the vault's record is unknown, every unpinned block of the cache goes (plain
    /// Drive ones too: the reader cannot tell them apart on disk), and the record starts again.
    fn forget_vault_blocks(&self, vault_id: &str) {
        let mut seen = self.seen_map();
        let record = self.seen_of(&mut seen, vault_id);
        if record.unknown {
            let limit = self.cache.limit();
            self.cache.set_limit(0);
            self.cache.set_limit(limit);
            record.files.retain(|(id, ..)| self.cache.is_pinned(id));
            let snapshot = record.files.clone();
            // Only a record written anew is known again; otherwise the next close evicts again.
            record.unknown = self.write_seen_whole(vault_id, &snapshot).is_err();
            return;
        }
        record.files.retain(|(id, version, size)| {
            if self.cache.is_pinned(id) {
                return true;
            }
            self.cache.forget(&FileRef {
                id: id.clone(),
                version: version.clone(),
                size: *size,
            });
            false
        });
        let snapshot = record.files.clone();
        let _ = self.write_seen(vault_id, &snapshot);
    }

    /// [`Self::write_seen`], but an empty record is written as an empty list rather than removed
    /// (replacing a damaged file for sure).
    fn write_seen_whole(&self, vault_id: &str, files: &HashSet<Seen>) -> Result<(), WaslError> {
        if !files.is_empty() {
            return self.write_seen(vault_id, files);
        }
        let io = |e: std::io::Error| WaslError::Io {
            detail: format!("The on-demand cache could not be used: {e}"),
        };
        std::fs::create_dir_all(&self.seen_dir).map_err(io)?;
        let path = self.seen_file(vault_id);
        let tmp = path.with_extension("json.part");
        std::fs::write(&tmp, b"[]").map_err(io)?;
        std::fs::rename(&tmp, &path).map_err(io)
    }

    async fn unlocked(&self, vault_id: &str) -> Result<bool, WaslError> {
        let remotes = self.remotes.clone();
        let id = vault_id.to_owned();
        tokio::task::spawn_blocking(move || remotes.vault_unlocked(&id))
            .await
            .map_err(|e| WaslError::Other {
                detail: e.to_string(),
            })
    }

    /// For a vault origin: fails `Locked` (and closes the vault) unless it is unlocked, notes the
    /// file, and returns the vault's close count to compare after the read.
    async fn check_vault(
        &self,
        origin: &OnDemandOrigin,
        f: &FileRef,
    ) -> Result<Option<(String, u64)>, WaslError> {
        let OnDemandOrigin::Vault { vault_id, .. } = origin else {
            return Ok(None);
        };
        // The count first: a close after this point, even one before the lock check below, makes
        // the count differ when the read compares it.
        let closes = self.closes_of(vault_id);
        if !self.unlocked(vault_id).await? {
            self.close_vault(vault_id.clone());
            return Err(locked());
        }
        self.remember(vault_id, f)?;
        Ok(Some((vault_id.clone(), closes)))
    }

    /// After a vault read: if the vault closed meanwhile, what was fetched goes and the read fails.
    fn still_open(&self, check: &Option<(String, u64)>, f: &FileRef) -> Result<(), WaslError> {
        let Some((vault_id, closes)) = check else {
            return Ok(());
        };
        // Under the lock a close holds throughout: either the close is over (the count moved) or it
        // has not begun (and will forget these blocks itself).
        let now = self.closes_map();
        if now.get(vault_id).copied().unwrap_or(0) == *closes {
            return Ok(());
        }
        if !self.cache.is_pinned(&f.id) {
            self.cache.forget(f);
        }
        Err(locked())
    }

    fn file_ref(origin: &OnDemandOrigin, f: &OnDemandFile) -> FileRef {
        let id = match origin {
            OnDemandOrigin::Drive { .. } => f.id.clone(),
            OnDemandOrigin::Vault { .. } => format!("{VAULT_PREFIX}{}", f.id),
        };
        FileRef {
            id,
            version: f.version.clone(),
            size: f.size,
        }
    }

    /// A hydrator that only looks at the cache.
    fn offline(&self) -> Hydrator {
        Hydrator::new(Arc::new(NoSource), self.cache.clone(), ReadAhead::default())
    }

    async fn access(&self, vault_id: &str) -> Result<Option<VaultAccess>, WaslError> {
        let remotes = self.remotes.clone();
        let id = vault_id.to_owned();
        tokio::task::spawn_blocking(move || remotes.vault_access(&id))
            .await
            .map_err(|e| WaslError::Other {
                detail: e.to_string(),
            })
    }

    /// The origin's hydrator, opened on first use (a vault needs its keys).
    async fn open_origin(&self, origin: &OnDemandOrigin) -> Result<Arc<Opened>, WaslError> {
        if let Some(o) = self.opened_map().get(origin) {
            return Ok(o.clone());
        }
        let opened = match origin {
            OnDemandOrigin::Drive { account } => Opened {
                hydrator: Hydrator::new(
                    self.remotes.source(account)?,
                    self.cache.clone(),
                    ReadAhead::default(),
                ),
                vault: None,
            },
            OnDemandOrigin::Vault {
                account,
                vault_id,
                folder_id,
                drive_id,
            } => {
                let access = self.access(vault_id).await?.ok_or_else(locked)?;
                self.open_vault(account, vault_id, folder_id, drive_id.clone(), access)?
            }
        };
        let mut map = self.opened_map();
        Ok(map
            .entry(origin.clone())
            .or_insert_with(|| Arc::new(opened))
            .clone())
    }

    fn open_vault(
        &self,
        account: &str,
        vault_id: &str,
        folder_id: &str,
        drive_id: Option<String>,
        access: VaultAccess,
    ) -> Result<Opened, WaslError> {
        let material = Zeroizing::new(access.keys);
        let inner = self.remotes.remote(account)?;
        let tmp = self.tmp.join(folder_name(vault_id));
        let unusable = |e: &dyn std::fmt::Display| WaslError::Locked {
            detail: format!("The vault's keys are unusable: {e}"),
        };
        let (source, lister, v2): (Arc<dyn Source>, Arc<dyn Remote>, bool) =
            if vaults::vault_version(access.manifest_json) == Some(2) {
                let mut ikm = Zeroizing::new([0u8; 32]);
                if material.len() != ikm.len() {
                    return Err(unusable(&"a v2 vault keeps 32 bytes"));
                }
                ikm.copy_from_slice(&material);
                let keys = Arc::new(VaultKeysV2::from_ikm(&ikm));
                let remote = Arc::new(VaultV2Remote::new(inner, keys, tmp));
                (remote.clone(), remote, true)
            } else {
                let keys = VaultKeys::from_master(&material).map_err(|e| unusable(&e))?;
                let cipher = Arc::new(VaultCipher::new(&keys));
                let lister = Arc::new(VaultRemote::new(inner.clone(), Arc::new(keys), tmp));
                (Arc::new(VaultV1Source::new(inner, cipher)), lister, false)
            };
        Ok(Opened {
            hydrator: Hydrator::new(
                Arc::new(Namespaced(source)),
                self.cache.clone(),
                ReadAhead::default(),
            ),
            vault: Some(VaultSide {
                lister,
                v2,
                listed: tokio::sync::Mutex::new(false),
                folder_id: folder_id.to_owned(),
                drive_id,
            }),
        })
    }

    /// Lists a vault folder with its names decrypted (a v2 vault's reads need this first).
    async fn list_vault(side: &VaultSide) -> Result<Vec<RemoteEntry>, WaslError> {
        let mut listed = side.listed.lock().await;
        let drive_id = side.drive_id.as_deref();
        side.lister
            .prepare(&side.folder_id, drive_id)
            .await
            .map_err(drive)?;
        let listing = side
            .lister
            .list_subtree(&side.folder_id, drive_id)
            .await
            .map_err(drive)?;
        *listed = true;
        Ok(listing.entries)
    }

    async fn ensure_listed(side: &VaultSide) -> Result<(), WaslError> {
        if side.v2 && !*side.listed.lock().await {
            Self::list_vault(side).await?;
        }
        Ok(())
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl OnDemandReader {
    /// Bytes `offset..offset+len` of `file` (fewer at its end, none past it), from the cache or
    /// fetched: only the blocks (vault v2: the chunks) the range needs, with read-ahead when a
    /// reader goes through the file in order.
    pub async fn read(
        &self,
        origin: OnDemandOrigin,
        file: OnDemandFile,
        offset: u64,
        len: u64,
    ) -> Result<Vec<u8>, WaslError> {
        let f = Self::file_ref(&origin, &file);
        let check = self.check_vault(&origin, &f).await?;
        let opened = self.open_origin(&origin).await?;
        if let Some(side) = &opened.vault {
            Self::ensure_listed(side).await?;
        }
        let got = match opened.hydrator.read(&f, offset, len).await {
            // A v2 file added since the listing is not known yet: list once more and retry.
            Err(VfsError::Unavailable(first)) if opened.vault.as_ref().is_some_and(|s| s.v2) => {
                let side = opened.vault.as_ref().expect("a vault");
                if Self::list_vault(side).await.is_err() {
                    return Err(vfs(VfsError::Unavailable(first)));
                }
                opened.hydrator.read(&f, offset, len).await.map_err(vfs)
            }
            other => other.map_err(vfs),
        };
        self.still_open(&check, &f)?;
        got
    }

    /// Fetches every block of the file and keeps it on the device until [`Self::unpin`] (FR-O5).
    pub async fn pin(&self, origin: OnDemandOrigin, file: OnDemandFile) -> Result<(), WaslError> {
        let f = Self::file_ref(&origin, &file);
        let check = self.check_vault(&origin, &f).await?;
        let opened = self.open_origin(&origin).await?;
        if let Some(side) = &opened.vault {
            Self::ensure_listed(side).await?;
        }
        let pinned = opened.hydrator.pin(&f).await.map_err(vfs);
        if self.still_open(&check, &f).is_err() {
            // Closed while pinning: the pin does not outlive the lock.
            let _ = self.offline().unpin(&f);
            self.cache.forget(&f);
            return Err(locked());
        }
        pinned
    }

    /// Lets the file's blocks be evicted again.
    pub fn unpin(&self, origin: OnDemandOrigin, file: OnDemandFile) -> Result<(), WaslError> {
        self.offline().unpin(&Self::file_ref(&origin, &file)).map_err(vfs)
    }

    /// Whether the file (any version) is pinned.
    pub fn is_pinned(&self, origin: OnDemandOrigin, file: OnDemandFile) -> bool {
        self.cache.is_pinned(&Self::file_ref(&origin, &file).id)
    }

    /// Whether every block of this version of the file is on the device.
    pub fn is_complete(&self, origin: OnDemandOrigin, file: OnDemandFile) -> bool {
        self.offline().complete(&Self::file_ref(&origin, &file))
    }

    /// Drops every cached block of the file (deleted or replaced remotely).
    pub fn forget(&self, origin: OnDemandOrigin, file: OnDemandFile) {
        self.cache.forget(&Self::file_ref(&origin, &file));
    }

    /// The files and folders of a vault folder, names decrypted. Fails with `Locked` while the
    /// vault is locked on this device (and forgets what it had opened of it).
    pub async fn vault_items(&self, origin: OnDemandOrigin) -> Result<Vec<VaultItem>, WaslError> {
        let OnDemandOrigin::Vault { vault_id, .. } = &origin else {
            return Err(WaslError::Other {
                detail: "Only a vault folder has a vault listing".into(),
            });
        };
        if self.access(vault_id).await?.is_none() {
            self.close_vault(vault_id.clone());
            return Err(locked());
        }
        let opened = self.open_origin(&origin).await?;
        let side = opened.vault.as_ref().expect("a vault origin opens a vault");
        let entries = Self::list_vault(side).await?;
        Ok(entries
            .iter()
            .filter(|e| e.google_mime.is_none())
            .map(|e| VaultItem {
                path: e.key.clone(),
                name: e.name.clone(),
                id: e.id.clone(),
                is_dir: e.is_dir,
                size: if e.is_dir { 0 } else { e.size.unwrap_or(0) },
                mtime: e.mtime,
                version: version_of(e),
            })
            .collect())
    }

    /// Call when a vault is locked or removed from the device: drops its opened state (keys
    /// included) and forgets the decrypted blocks read through it, by this run or an earlier one.
    /// Pinned files stay (the person asked to keep them on the device); reading them needs the
    /// vault unlocked again.
    pub fn close_vault(&self, vault_id: String) {
        // The whole close under the count's lock: a read comparing the count sees it either
        // before the close began (its blocks are forgotten here) or after it ended (it forgets
        // its own and fails).
        let mut closes = self.closes_map();
        self.opened_map().retain(|o, _| match o {
            OnDemandOrigin::Vault { vault_id: v, .. } => *v != vault_id,
            OnDemandOrigin::Drive { .. } => true,
        });
        self.forget_vault_blocks(&vault_id);
        *closes.entry(vault_id).or_insert(0) += 1;
    }

    pub fn usage(&self) -> CacheUsage {
        let (used, pinned) = self.cache.usage();
        CacheUsage {
            used,
            pinned,
            limit: self.cache.limit(),
        }
    }

    /// Changes the limit of unpinned content; a lower one evicts least-recently-used blocks now.
    pub fn set_limit(&self, limit: u64) {
        self.cache.set_limit(limit);
    }
}
