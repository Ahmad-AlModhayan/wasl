//! The block cache (docs/ON_DEMAND.md §2.1): file content in fixed blocks on disk, an LRU over
//! bytes, and pins that are never evicted (FR-O5).
//!
//! A block belongs to one version of one file ([`FileRef::id`] and [`FileRef::version`]); a new
//! version never reads an old version's blocks. Blocks are files under the cache folder, named by
//! a hash of their file and index, so nothing about the file's name or folder shows on disk.
//! Pins are kept in `pins.json` there. The index is rebuilt from the folder at start (a block's
//! last use is its file's modification time), so a crash loses at most the order of recent uses.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::FileRef;

/// Block size of the cache: 1 MiB.
pub const BLOCK: u64 = 1 << 20;

/// One block's place in the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct BlockKey {
    file: [u8; 16],
    index: u64,
}

impl BlockKey {
    pub(crate) fn new(f: &FileRef, index: u64) -> Self {
        let h = blake3::hash(format!("{}\0{}", f.id, f.version).as_bytes());
        let mut file = [0u8; 16];
        file.copy_from_slice(&h.as_bytes()[..16]);
        Self { file, index }
    }

    fn file_hex(&self) -> String {
        self.file.iter().map(|b| format!("{b:02x}")).collect()
    }
}

struct Entry {
    size: u64,
    /// A logical clock: higher is more recent.
    used: u64,
}

struct State {
    entries: HashMap<BlockKey, Entry>,
    /// used → key, for eviction in LRU order.
    order: BTreeMap<u64, BlockKey>,
    clock: u64,
    total: u64,
    /// Pinned files, by their cache id (file id + version hash).
    pinned: HashSet<[u8; 16]>,
    /// Pinned files by their stable id, as the person pinned them (`pins.json`).
    pins: BTreeMap<String, String>,
}

/// File content on disk, within a byte budget.
pub struct BlockCache {
    dir: PathBuf,
    limit: std::sync::atomic::AtomicU64,
    state: Mutex<State>,
}

impl BlockCache {
    /// Opens (or creates) the cache at `dir`, keeping at most `limit` bytes of unpinned blocks.
    pub fn open(dir: impl Into<PathBuf>, limit: u64) -> io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        let mut st = State {
            entries: HashMap::new(),
            order: BTreeMap::new(),
            clock: 0,
            total: 0,
            pinned: HashSet::new(),
            pins: read_pins(&dir.join("pins.json")),
        };
        // Rebuild the index, oldest use first.
        let mut found: Vec<(SystemTime, BlockKey, u64)> = Vec::new();
        for shard in fs::read_dir(&dir)?.flatten() {
            if !shard.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let Some(file) = parse_hex16(&shard.file_name().to_string_lossy()) else {
                continue;
            };
            for b in fs::read_dir(shard.path())?.flatten() {
                let name = b.file_name().to_string_lossy().into_owned();
                let Some(index) = name.strip_suffix(".blk").and_then(|n| n.parse::<u64>().ok()) else {
                    // A half-written block from a crash.
                    let _ = fs::remove_file(b.path());
                    continue;
                };
                let meta = b.metadata()?;
                found.push((
                    meta.modified().unwrap_or(UNIX_EPOCH),
                    BlockKey { file, index },
                    meta.len(),
                ));
            }
        }
        let pinned: Vec<[u8; 16]> = st
            .pins
            .iter()
            .map(|(id, version)| {
                BlockKey::new(
                    &FileRef {
                        id: id.clone(),
                        version: version.clone(),
                        size: 0,
                    },
                    0,
                )
                .file
            })
            .collect();
        st.pinned.extend(pinned);
        found.sort_by_key(|(t, ..)| *t);
        for (_, key, size) in found {
            st.clock += 1;
            st.order.insert(st.clock, key);
            st.entries.insert(key, Entry { size, used: st.clock });
            st.total += size;
        }
        let cache = Self {
            dir,
            limit: std::sync::atomic::AtomicU64::new(limit),
            state: Mutex::new(st),
        };
        Ok(cache)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn path(&self, key: &BlockKey) -> PathBuf {
        self.dir.join(key.file_hex()).join(format!("{}.blk", key.index))
    }

    /// The block, if cached (and marks it used).
    pub(crate) fn get(&self, key: &BlockKey) -> Option<Vec<u8>> {
        let mut st = self.lock();
        let used = st.entries.get(key)?.used;
        let bytes = fs::read(self.path(key)).ok();
        match bytes {
            Some(b) => {
                st.order.remove(&used);
                st.clock += 1;
                let now = st.clock;
                st.order.insert(now, *key);
                if let Some(e) = st.entries.get_mut(key) {
                    e.used = now;
                }
                Some(b)
            }
            None => {
                // Gone from disk behind our back: forget it.
                if let Some(e) = st.entries.remove(key) {
                    st.order.remove(&e.used);
                    st.total -= e.size;
                }
                None
            }
        }
    }

    /// Whether the block is cached, without touching its use.
    pub(crate) fn has(&self, key: &BlockKey) -> bool {
        self.lock().entries.contains_key(key)
    }

    /// Stores a block (written to a temporary file, then renamed), then evicts down to the limit.
    pub(crate) fn put(&self, key: BlockKey, bytes: &[u8]) -> io::Result<()> {
        let path = self.path(&key);
        fs::create_dir_all(path.parent().expect("block in a folder"))?;
        // A name of its own per write: two readers fetching the same block at once must not
        // rename each other's temporary file away.
        static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = path.with_extension(format!("{}.{n}.part", std::process::id()));
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, &path)?;
        let mut st = self.lock();
        if let Some(old) = st.entries.remove(&key) {
            st.order.remove(&old.used);
            st.total -= old.size;
        }
        st.clock += 1;
        let now = st.clock;
        st.order.insert(now, key);
        st.entries.insert(
            key,
            Entry {
                size: bytes.len() as u64,
                used: now,
            },
        );
        st.total += bytes.len() as u64;
        self.evict(&mut st);
        Ok(())
    }

    /// Drops least-recently-used unpinned blocks until unpinned bytes fit the limit.
    fn evict(&self, st: &mut State) {
        let pinned_bytes: u64 = st
            .entries
            .iter()
            .filter(|(k, _)| st.pinned.contains(&k.file))
            .map(|(_, e)| e.size)
            .sum();
        let mut unpinned = st.total - pinned_bytes;
        let limit = self.limit();
        let victims: Vec<(u64, BlockKey)> = st
            .order
            .iter()
            .filter(|(_, k)| !st.pinned.contains(&k.file))
            .map(|(u, k)| (*u, *k))
            .collect();
        for (used, key) in victims {
            if unpinned <= limit {
                break;
            }
            let size = st.entries.get(&key).map_or(0, |e| e.size);
            if fs::remove_file(self.path(&key)).is_ok() || !self.path(&key).exists() {
                st.order.remove(&used);
                st.entries.remove(&key);
                st.total -= size;
                unpinned -= size;
            }
        }
    }

    /// Marks the file's current version as pinned (its blocks are never evicted).
    pub(crate) fn set_pinned(&self, f: &FileRef, pinned: bool) -> io::Result<()> {
        let id = BlockKey::new(f, 0).file;
        let mut st = self.lock();
        if pinned {
            st.pinned.insert(id);
            st.pins.insert(f.id.clone(), f.version.clone());
        } else {
            st.pinned.remove(&id);
            st.pins.remove(&f.id);
        }
        write_pins(&self.dir.join("pins.json"), &st.pins)?;
        self.evict(&mut st);
        Ok(())
    }

    /// Whether the file (any version) is pinned.
    pub fn is_pinned(&self, id: &str) -> bool {
        self.lock().pins.contains_key(id)
    }

    /// Re-applies a pin to a file's current version (its old version's blocks become ordinary).
    pub(crate) fn refresh_pin(&self, f: &FileRef) {
        let mut st = self.lock();
        if let Some(old) = st.pins.get(&f.id).cloned()
            && old != f.version
        {
            let old_id = BlockKey::new(
                &FileRef {
                    id: f.id.clone(),
                    version: old,
                    size: 0,
                },
                0,
            )
            .file;
            st.pinned.remove(&old_id);
            st.pinned.insert(BlockKey::new(f, 0).file);
            st.pins.insert(f.id.clone(), f.version.clone());
            let _ = write_pins(&self.dir.join("pins.json"), &st.pins);
        } else if st.pins.contains_key(&f.id) {
            st.pinned.insert(BlockKey::new(f, 0).file);
        }
    }

    /// Bytes of unpinned blocks the cache keeps at most.
    pub fn limit(&self) -> u64 {
        self.limit.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Changes the limit; a lower one evicts least-recently-used unpinned blocks now.
    pub fn set_limit(&self, limit: u64) {
        self.limit.store(limit, std::sync::atomic::Ordering::Relaxed);
        let mut st = self.lock();
        self.evict(&mut st);
    }

    /// Bytes on disk: (all blocks, pinned blocks).
    pub fn usage(&self) -> (u64, u64) {
        let st = self.lock();
        let pinned = st
            .entries
            .iter()
            .filter(|(k, _)| st.pinned.contains(&k.file))
            .map(|(_, e)| e.size)
            .sum();
        (st.total, pinned)
    }

    /// Forgets every block of every file that is not pinned: what a locked vault's view keeps
    /// (only the files someone chose to keep on the device stay).
    pub fn forget_unpinned(&self) {
        let mut st = self.lock();
        let gone: Vec<BlockKey> = st
            .entries
            .keys()
            .filter(|k| !st.pinned.contains(&k.file))
            .copied()
            .collect();
        let mut files = HashSet::new();
        for k in gone {
            if let Some(e) = st.entries.remove(&k) {
                st.order.remove(&e.used);
                st.total -= e.size;
            }
            files.insert(k.file);
        }
        for file in files {
            let _ = fs::remove_dir_all(self.dir.join(BlockKey { file, index: 0 }.file_hex()));
        }
    }

    /// Forgets every block of every version of a file (deleted remotely).
    pub fn forget(&self, f: &FileRef) {
        let file = BlockKey::new(f, 0).file;
        let mut st = self.lock();
        let keys: Vec<BlockKey> = st.entries.keys().filter(|k| k.file == file).copied().collect();
        for k in keys {
            if let Some(e) = st.entries.remove(&k) {
                st.order.remove(&e.used);
                st.total -= e.size;
            }
        }
        st.pinned.remove(&file);
        let _ = fs::remove_dir_all(self.dir.join(BlockKey { file, index: 0 }.file_hex()));
    }
}

fn parse_hex16(s: &str) -> Option<[u8; 16]> {
    if s.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

fn read_pins(path: &Path) -> BTreeMap<String, String> {
    fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_pins(path: &Path, pins: &BTreeMap<String, String>) -> io::Result<()> {
    let tmp = path.with_extension("json.part");
    fs::write(&tmp, serde_json::to_vec_pretty(pins).map_err(io::Error::other)?)?;
    fs::rename(tmp, path)
}
