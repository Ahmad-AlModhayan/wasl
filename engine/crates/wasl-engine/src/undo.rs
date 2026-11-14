//! Agent undo (docs/AGENT_UNDO.md): the watch list's rules (UND-1) and the snapshot a session takes
//! of every watched folder at its start (UND-2). The journal of what it changes follows (#57).
//!
//! The undo store sits in Wasl's data folder, which no watch may be in or around, so it is outside
//! every watched folder; its folders are owner-only and nothing in it is synced. Its layout:
//! `objects/<2 hex>/<BLAKE3 hex>` (content kept by its id), `clones/<changeset>/<n>` (copy-on-write
//! clones, hashed later) and `tmp/` (copies being written).

use crate::engine::Engine;
use crate::error::{EngineError, Result};
use crate::support::new_id;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use wasl_ignore::IgnoreRules;
use wasl_ignore::legacy::PathFilter;
use wasl_store::{SnapshotOrigin, SnapshotRow, StatKey, Watch};

/// Why `dir` can't be watched, or its full path (symlinks resolved). `own_dirs` are Wasl's data
/// folders: a watch in or around one is refused, so the undo store stays outside every watch.
pub fn check_watch_root(dir: &Path, own_dirs: &[PathBuf]) -> std::result::Result<PathBuf, String> {
    let shown = dir.display();
    if is_root_text(&dir.to_string_lossy()) {
        return Err(format!(
            "{shown} is the top of a disk. Watch the folders an agent works in instead."
        ));
    }
    let full = fs::canonicalize(dir).map_err(|e| format!("{shown}: {e}"))?;
    if !full.is_dir() {
        return Err(format!("{shown} is not a folder."));
    }
    let full = plain(full);
    if full.parent().is_none() {
        return Err(format!(
            "{shown} is the top of a disk. Watch the folders an agent works in instead."
        ));
    }
    for own in own_dirs {
        let own = full_path(own);
        if full.starts_with(&own) || own.starts_with(&full) {
            return Err(format!(
                "{shown} is in or around {}, where Wasl keeps its own data. Choose another folder.",
                own.display()
            ));
        }
    }
    Ok(full)
}

/// `p` with symlinks resolved, also when only its parent exists yet (the undo store before its
/// first snapshot).
fn full_path(p: &Path) -> PathBuf {
    fs::canonicalize(p)
        .or_else(|e| match (p.parent(), p.file_name()) {
            (Some(parent), Some(name)) => fs::canonicalize(parent).map(|d| d.join(name)),
            _ => Err(e),
        })
        .map(plain)
        .unwrap_or_else(|_| p.to_path_buf())
}

/// A disk's top folder as typed, on any OS: `/`, `\`, `C:`, `C:\`, `\\?\C:\`, `\\server\share`.
fn is_root_text(s: &str) -> bool {
    let s = s.strip_prefix(r"\\?\").unwrap_or(s);
    let t = s.trim_end_matches(['/', '\\']);
    if t.is_empty() {
        return !s.is_empty();
    }
    let b = t.as_bytes();
    if b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return true;
    }
    // A UNC share root: two leading separators, then exactly server and share.
    if let Some(rest) = s.strip_prefix(r"\\").or_else(|| s.strip_prefix("//")) {
        return rest.trim_end_matches(['/', '\\']).split(['/', '\\']).count() <= 2;
    }
    false
}

/// Without Windows' `\\?\` prefix, as pairs record paths.
fn plain(p: PathBuf) -> PathBuf {
    #[cfg(windows)]
    if let Some(s) = strip_verbatim(&p.to_string_lossy()) {
        return PathBuf::from(s);
    }
    p
}

/// `\\?\C:\x` as `C:\x` and `\\?\UNC\server\share\x` as `\\server\share\x`; `None` without such a
/// prefix.
#[cfg_attr(not(windows), allow(dead_code))]
fn strip_verbatim(s: &str) -> Option<String> {
    let rest = s.strip_prefix(r"\\?\")?;
    if let Some(unc) = rest.strip_prefix(r"UNC\") {
        return Some(format!(r"\\{unc}"));
    }
    (rest.as_bytes().get(1) == Some(&b':')).then(|| rest.to_owned())
}

/// What a snapshot cost.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SnapshotStats {
    pub files: u64,
    pub bytes: u64,
    /// Bytes read from the folder (and from clones being hashed).
    pub bytes_read: u64,
    /// Removed between the scan and the read: not there at the session's start, so not kept.
    pub vanished: u64,
    /// Unchanged since the last snapshot (same size, mtime, ctime and inode): nothing read.
    pub reused: u64,
    pub cloned: u64,
    pub stored: u64,
    pub synced: u64,
}

/// Each watch a session snapshotted, with what it cost or why it failed.
pub type WatchSnapshots = Vec<(Watch, std::result::Result<SnapshotStats, String>)>;

/// The undo store's folder.
pub struct UndoStore {
    pub(crate) root: PathBuf,
}

impl UndoStore {
    pub fn new(root: impl Into<PathBuf>) -> UndoStore {
        UndoStore { root: root.into() }
    }

    fn dir(&self, rel: &str) -> io::Result<PathBuf> {
        let d = self.root.join(rel);
        owner_only_dir(&self.root)?;
        owner_only_dir(&d)?;
        Ok(d)
    }

    /// The full path of a `location` this store handed out.
    pub fn path(&self, location: &str) -> PathBuf {
        self.root.join(location)
    }

    /// Copies `src` into the store by its BLAKE3, reading it once. Returns (BLAKE3 hex, size,
    /// location, whether the object is new). A new object isn't flushed to disk yet: see
    /// [`sync_objects`].
    pub(crate) fn store(&self, src: &Path) -> io::Result<(String, u64, String, bool)> {
        let tmp_dir = self.dir("tmp")?;
        let tmp = tmp_dir.join(format!("{}-{}", std::process::id(), unique()));
        let mut input = File::open(src)?;
        let mut out = create_private(&tmp)?;
        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; 256 * 1024];
        let mut size = 0u64;
        let copied = loop {
            match input.read(&mut buf) {
                Ok(0) => break Ok(()),
                Ok(n) => {
                    hasher.update(&buf[..n]);
                    size += n as u64;
                    if let Err(e) = out.write_all(&buf[..n]) {
                        break Err(e);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => break Err(e),
            }
        };
        drop(out);
        if let Err(e) = copied {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        let hex = hasher.finalize().to_hex().to_string();
        let location = format!("objects/{}/{hex}", &hex[..2]);
        let dst = self.root.join(&location);
        self.dir(&format!("objects/{}", &hex[..2]))?;
        let fresh = !dst.exists();
        if fresh {
            fs::rename(&tmp, &dst)?;
        } else {
            let _ = fs::remove_file(&tmp);
        }
        Ok((hex, size, location, fresh))
    }

    /// Clones `src` into the store sharing its blocks; `None` where the file system can't.
    fn clone_in(&self, clone_dir: &str, n: usize, src: &Path) -> io::Result<Option<String>> {
        let location = format!("clones/{clone_dir}/{n}");
        let dir = self.dir(&format!("clones/{clone_dir}"))?;
        let dst = dir.join(n.to_string());
        let _ = fs::remove_file(&dst);
        let cloned = clone_file(src, &dst)?;
        if cloned {
            // A clone starts with the source's mode.
            set_private(&dst)?;
        }
        Ok(cloned.then_some(location))
    }
}

/// Creates the new file `p` readable by its owner only (UND-11). On Windows it inherits the data
/// folder's access list.
fn create_private(p: &Path) -> io::Result<File> {
    let mut o = fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(p)
}

fn set_private(p: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(p, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = p;
    Ok(())
}

/// Makes the new objects of a snapshot durable before its rows are recorded: all are written
/// first, then flushed together, not one full flush per file (UND-12's 30 s).
#[cfg(target_os = "linux")]
pub(crate) fn sync_objects(root: &Path, fresh: &[PathBuf]) -> io::Result<()> {
    if !fresh.is_empty() {
        // One flush of the file system the store is on: the files, their folders, the renames.
        rustix::fs::syncfs(File::open(root)?)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn sync_objects(root: &Path, fresh: &[PathBuf]) -> io::Result<()> {
    if fresh.is_empty() {
        return Ok(());
    }
    // fsync hands each file and folder to the disk; one F_FULLFSYNC then flushes the disk's cache.
    let dirs: std::collections::BTreeSet<&Path> = fresh.iter().filter_map(|p| p.parent()).collect();
    for p in fresh.iter().map(PathBuf::as_path).chain(dirs) {
        rustix::fs::fsync(File::open(p)?)?;
    }
    rustix::fs::fcntl_fullfsync(File::open(root)?)?;
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn sync_objects(_root: &Path, fresh: &[PathBuf]) -> io::Result<()> {
    for p in fresh {
        fs::OpenOptions::new().write(true).open(p)?.sync_all()?;
    }
    Ok(())
}

fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

/// Creates `dir` readable by its owner only (UND-11). On Windows it inherits the data folder's
/// access list, which is the user's own.
fn owner_only_dir(dir: &Path) -> io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(dir)
}

/// Clones `src` to `dst` (which doesn't exist) sharing its blocks: APFS `clonefile`, the Linux
/// `FICLONE` ioctl (Btrfs, XFS). `Ok(false)` where the file system or OS can't; nothing is left.
#[cfg(target_os = "macos")]
fn clone_file(src: &Path, dst: &Path) -> io::Result<bool> {
    use rustix::fs::{CWD, CloneFlags, fclonefileat};
    use rustix::io::Errno;
    let s = File::open(src)?;
    match fclonefileat(&s, CWD, dst, CloneFlags::NOFOLLOW) {
        Ok(()) => Ok(true),
        Err(e) if e == Errno::NOTSUP || e == Errno::XDEV => Ok(false),
        Err(e) => Err(e.into()),
    }
}

#[cfg(target_os = "linux")]
fn clone_file(src: &Path, dst: &Path) -> io::Result<bool> {
    use rustix::io::Errno;
    let s = File::open(src)?;
    let d = fs::OpenOptions::new().write(true).create_new(true).open(dst)?;
    match rustix::fs::ioctl_ficlone(&d, &s) {
        Ok(()) => Ok(true),
        Err(e) => {
            drop(d);
            let _ = fs::remove_file(dst);
            if [
                Errno::OPNOTSUPP,
                Errno::XDEV,
                Errno::INVAL,
                Errno::NOTTY,
                Errno::BADF,
            ]
            .contains(&e)
            {
                Ok(false)
            } else {
                Err(e.into())
            }
        }
    }
}

/// Windows (ReFS block cloning) and the rest: always through the store for now.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn clone_file(_src: &Path, _dst: &Path) -> io::Result<bool> {
    Ok(false)
}

/// How a snapshot keeps each file's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepBy {
    /// Copy-on-write clones where the file system has them, else the store.
    CloneOrStore,
    /// Always the store (BLAKE3 content ids).
    Store,
}

/// The ignore rules of a watch: its `.waslignore` files (read by the scan) and the default ignores
/// it still applies. Hidden files count.
pub(crate) fn watch_rules(ignores: &[String]) -> wasl_ignore::rules::IgnoreRulesBuilder {
    IgnoreRules::builder().pair_patterns(PathFilter::new(&[] as &[String], ignores, &[], true))
}

/// Snapshots the folder `root` (UND-2). A file with the same size, mtime, ctime and inode as in
/// `previous` reuses its row without reading it (see [`unchanged`]); one `synced` knows (rel, size, mtime → BLAKE3 of the synced version)
/// reuses that; the rest are cloned or copied into `store`. Clones go in `clones/<clone_dir>/`.
pub fn snapshot_folder(
    root: &Path,
    ignores: &[String],
    store: &UndoStore,
    clone_dir: &str,
    previous: &[SnapshotRow],
    synced: &dyn Fn(&str, u64, i64) -> Option<String>,
    keep: KeepBy,
) -> Result<(Vec<SnapshotRow>, SnapshotStats)> {
    let fs_ = wasl_fs::StdFs::new(root, store.root.join("tmp"));
    let scan = wasl_fs::scan(&fs_, watch_rules(ignores), &|rel: &str| rel.to_owned(), || false)?;
    if let Some(first) = scan.unreadable.first() {
        return Err(EngineError::Io(format!(
            "{} files in {} can't be read, for example {first}",
            scan.unreadable.len(),
            root.display()
        )));
    }
    let before: HashMap<&str, &SnapshotRow> = previous.iter().map(|r| (r.rel.as_str(), r)).collect();
    let mut stats = SnapshotStats::default();
    let mut rows = Vec::new();
    let mut can_clone = keep == KeepBy::CloneOrStore;
    let mut fresh = Vec::new();
    for e in scan.entries.into_iter().filter(|e| !e.is_dir) {
        let src = root.join(&e.rel);
        let (ctime, file_id) = match fs::metadata(&src) {
            Ok(m) => identity(&m),
            Err(e) if gone(&e) => {
                stats.vanished += 1;
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        if let Some(p) = before.get(e.rel.as_str())
            && unchanged(p, e.size, e.mtime, ctime, file_id)
            && p.location.as_deref().is_none_or(|l| store.path(l).exists())
        {
            stats.reused += 1;
            rows.push((*p).clone());
            continue;
        }
        if let Some(hash) = synced(&e.rel, e.size, e.mtime) {
            stats.synced += 1;
            rows.push(SnapshotRow {
                rel: e.rel,
                size: e.size,
                mtime: e.mtime,
                ctime,
                file_id,
                blake3: Some(hash),
                origin: SnapshotOrigin::Synced,
                location: None,
            });
            continue;
        }
        if can_clone {
            match store.clone_in(clone_dir, rows.len(), &src) {
                Ok(Some(location)) => {
                    stats.cloned += 1;
                    // Its row is committed after this returns: the clone must be on disk first.
                    fresh.push(store.path(&location));
                    rows.push(SnapshotRow {
                        rel: e.rel,
                        size: e.size,
                        mtime: e.mtime,
                        ctime,
                        file_id,
                        blake3: None,
                        origin: SnapshotOrigin::Cloned,
                        location: Some(location),
                    });
                    continue;
                }
                // One refusal is the file system's answer for the whole folder.
                Ok(None) => can_clone = false,
                Err(e) if gone(&e) => {
                    stats.vanished += 1;
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
        }
        let (hash, size, location, new) = match store.store(&src) {
            Ok(stored) => stored,
            Err(e) if gone(&e) => {
                stats.vanished += 1;
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        if new {
            fresh.push(store.path(&location));
        }
        stats.stored += 1;
        stats.bytes_read += size;
        rows.push(SnapshotRow {
            rel: e.rel,
            size,
            mtime: e.mtime,
            ctime,
            file_id,
            blake3: Some(hash),
            origin: SnapshotOrigin::Stored,
            location: Some(location),
        });
    }
    sync_objects(&store.root, &fresh)?;
    stats.files = rows.len() as u64;
    stats.bytes = rows.iter().map(|r| r.size).sum();
    Ok((rows, stats))
}

/// Whether a file still has the row `p` of the previous snapshot: same size, mtime, ctime and file
/// id. Without a file id (Windows, until it reads the NTFS file index) never: an in-place overwrite
/// keeps the creation time and can restore the mtime.
fn unchanged(p: &SnapshotRow, size: u64, mtime: i64, ctime: Option<i64>, file_id: Option<i64>) -> bool {
    same_file(
        (p.size, p.mtime, p.ctime, p.file_id),
        (size, mtime, ctime, file_id),
    )
}

/// Whether a file whose reuse key was `was` is unchanged now (see [`unchanged`]).
pub(crate) fn same_file(was: StatKey, now: StatKey) -> bool {
    now.3.is_some() && was == now
}

/// `path`'s reuse key, when it is a file.
pub(crate) fn stat_key(path: &Path) -> Option<StatKey> {
    let m = fs::symlink_metadata(path).ok().filter(|m| m.is_file())?;
    let (ctime, file_id) = identity(&m);
    let mtime = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64);
    Some((m.len(), mtime, ctime, file_id))
}

/// The file was removed after the scan listed it.
pub(crate) fn gone(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::NotFound
}

/// A file's status change time (ns) and inode: with size and mtime, what tells a replaced file
/// (`cp -p`, tar, `touch -r`) from an unchanged one.
#[cfg(unix)]
fn identity(m: &fs::Metadata) -> (Option<i64>, Option<i64>) {
    use std::os::unix::fs::MetadataExt;
    let ctime = m
        .ctime()
        .saturating_mul(1_000_000_000)
        .saturating_add(m.ctime_nsec());
    (Some(ctime), Some(m.ino() as i64))
}

/// Windows: the creation time (a copy over the file is a new file); the file id needs
/// `GetFileInformationByHandle`, which follows.
#[cfg(windows)]
fn identity(m: &fs::Metadata) -> (Option<i64>, Option<i64>) {
    use std::os::windows::fs::MetadataExt;
    (Some(m.creation_time() as i64), None)
}

#[cfg(not(any(unix, windows)))]
fn identity(_: &fs::Metadata) -> (Option<i64>, Option<i64>) {
    (None, None)
}

/// Fills in the BLAKE3 of each clone in `rows`, reading the clones (not the watched folder).
pub fn hash_clones(store: &UndoStore, rows: &mut [SnapshotRow], stats: &mut SnapshotStats) -> Result<()> {
    for r in rows.iter_mut().filter(|r| r.blake3.is_none()) {
        let Some(location) = &r.location else { continue };
        let mut hasher = blake3::Hasher::new();
        hasher.update_reader(File::open(store.path(location))?)?;
        stats.bytes_read += r.size;
        r.blake3 = Some(hasher.finalize().to_hex().to_string());
    }
    Ok(())
}

impl Engine {
    pub(crate) fn undo_store(&self) -> Result<UndoStore> {
        let dir = self.config.undo_dir.clone().ok_or_else(|| {
            EngineError::Other("agent undo is not set up on this device (no undo folder)".into())
        })?;
        Ok(UndoStore::new(dir))
    }

    /// Adds `root` (checked with [`check_watch_root`]) to the watch list (UND-1). A folder in or
    /// around a watched one is refused: one watch covers it already, or would be covered twice. So
    /// is one in or around the undo store or the agent folder, whoever calls this.
    pub fn agent_watch(&self, root: &Path) -> Result<Watch> {
        let own: Vec<PathBuf> = [&self.config.undo_dir, &self.config.agent_dir]
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        let root = &check_watch_root(root, &own).map_err(wasl_store::StoreError::Refused)?;
        let text = root.to_string_lossy().into_owned();
        let mut store = self.store.lock();
        for w in store.watches()? {
            let other = Path::new(&w.root);
            if other == root {
                return Err(wasl_store::StoreError::Refused(format!("{text} is watched already.")).into());
            }
            if root.starts_with(other) || other.starts_with(root) {
                return Err(wasl_store::StoreError::Refused(format!(
                    "{text} is in or around {}, which is watched already. Unwatch it first.",
                    w.root
                ))
                .into());
            }
        }
        Ok(store.add_watch(&new_id(self.clock.as_ref()), &text, self.clock.now_ms())?)
    }

    /// Takes a folder (by watch id or path) off the watch list. Its snapshots stay with their sessions.
    pub fn agent_unwatch(&self, id_or_root: &str) -> Result<Watch> {
        let full = fs::canonicalize(id_or_root).map(plain).ok();
        let mut store = self.store.lock();
        let w = store
            .watches()?
            .into_iter()
            .find(|w| {
                w.id == id_or_root
                    || w.root == id_or_root
                    || full.as_deref().is_some_and(|f| Path::new(&w.root) == f)
            })
            .ok_or_else(|| EngineError::NotFound(format!("{id_or_root} is not watched")))?;
        store.remove_watch(&w.id, self.clock.now_ms())?;
        Ok(w)
    }

    /// What the session `changeset_id` found in the watched folder `watch_id`, by path.
    pub fn agent_snapshot(&self, changeset_id: &str, watch_id: &str) -> Result<Vec<SnapshotRow>> {
        Ok(self.store.lock().snapshot(changeset_id, watch_id)?)
    }

    /// The watched folders the session `changeset_id` couldn't snapshot at its start: (watch id,
    /// why).
    pub fn agent_snapshot_failures(&self, changeset_id: &str) -> Result<Vec<(String, String)>> {
        Ok(self.store.lock().snapshot_failures(changeset_id)?)
    }

    pub fn agent_watches(&self) -> Result<Vec<Watch>> {
        Ok(self.store.lock().watches()?)
    }

    /// Sets the default ignores a watch still applies (removing one stops it applying).
    pub fn agent_watch_ignores(&self, id: &str, ignores: &[String]) -> Result<()> {
        Ok(self.store.lock().set_watch_ignores(id, ignores)?)
    }

    /// The session-start snapshot of every watched folder (UND-2), recorded for `changeset_id`.
    /// Returns each watch with what its snapshot cost, or why it failed; one failure doesn't stop
    /// the others.
    pub async fn snapshot_watches(&self, changeset_id: &str) -> Result<WatchSnapshots> {
        let watches = self.agent_watches()?;
        if watches.is_empty() {
            return Ok(Vec::new());
        }
        let store_dir = self.undo_store()?.root;
        let mut out = Vec::new();
        for w in watches {
            // One watch's database error doesn't stop the others.
            let previous = match self.store.lock().last_snapshot(&w.id) {
                Ok(previous) => previous,
                Err(e) => {
                    out.push((w, Err(e.to_string())));
                    continue;
                }
            };
            let synced = self.synced_versions(Path::new(&w.root));
            let (root, ignores, cs, dir) = (
                PathBuf::from(&w.root),
                w.ignores.clone(),
                format!("{changeset_id}/{}", w.id),
                store_dir.clone(),
            );
            let taken = tokio::task::spawn_blocking(move || {
                let lookup = |rel: &str, size: u64, mtime: i64| {
                    synced
                        .get(rel)
                        .filter(|(s, m, _)| *s == size && *m == mtime)
                        .map(|(_, _, h)| h.clone())
                };
                let store = UndoStore::new(dir);
                let (mut rows, mut stats) = snapshot_folder(
                    &root,
                    &ignores,
                    &store,
                    &cs,
                    &previous,
                    &lookup,
                    KeepBy::CloneOrStore,
                )?;
                // Every row is recorded with its BLAKE3: the journal and restores (#57) compare by it.
                hash_clones(&store, &mut rows, &mut stats)?;
                Ok::<_, EngineError>((rows, stats))
            })
            .await
            .map_err(|e| EngineError::Other(e.to_string()))?;
            let taken = taken.and_then(|(rows, stats)| {
                self.store.lock().put_snapshot(changeset_id, &w.id, &rows)?;
                Ok(stats)
            });
            let result = match taken {
                Ok(stats) => Ok(stats),
                Err(e) => {
                    // Kept with the session, whose report lists it (#57); best effort, so the
                    // other watches still get their snapshot.
                    let mut reason = e.to_string();
                    if let Err(db) = self.store.lock().put_snapshot_failure(
                        changeset_id,
                        &w.id,
                        &reason,
                        self.clock.now_ms(),
                    ) {
                        reason = format!("{reason} (not recorded: {db})");
                    }
                    Err(reason)
                }
            };
            out.push((w, result));
        }
        Ok(out)
    }

    /// The synced versions of files under `root` that device sync knows (pairs whose folder holds
    /// `root`, or lies in it): rel to `root` → (size, mtime on this disk, BLAKE3).
    fn synced_versions(&self, root: &Path) -> HashMap<String, (u64, i64, String)> {
        let mut out = HashMap::new();
        let store = self.store.lock();
        for pair in store.list_pairs().unwrap_or_default() {
            let pair_root = PathBuf::from(&pair.local_root);
            let (prefix, strip) = if let Ok(r) = root.strip_prefix(&pair_root) {
                (r.to_string_lossy().replace('\\', "/"), true)
            } else if let Ok(r) = pair_root.strip_prefix(root) {
                (r.to_string_lossy().replace('\\', "/"), false)
            } else {
                continue;
            };
            let Ok(Some(index)) = store.load_index(&pair.id) else {
                continue;
            };
            for (key, e) in index.entries {
                let (Some(hash), false, false) = (e.version.hash, e.version.deleted, e.version.is_dir) else {
                    continue;
                };
                let rel = if strip {
                    match key.strip_prefix(&prefix) {
                        Some(r) if prefix.is_empty() => r.to_owned(),
                        Some(r) if r.starts_with('/') => r[1..].to_owned(),
                        _ => continue,
                    }
                } else if prefix.is_empty() {
                    key
                } else {
                    format!("{prefix}/{key}")
                };
                out.insert(rel, (e.local_size, e.local_mtime, hash));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn previous(file_id: Option<i64>) -> SnapshotRow {
        SnapshotRow {
            rel: "a.txt".into(),
            size: 5,
            mtime: 10,
            ctime: Some(20),
            file_id,
            blake3: Some("h".into()),
            origin: SnapshotOrigin::Stored,
            location: Some("objects/h".into()),
        }
    }

    #[test]
    fn a_file_with_its_size_times_and_file_id_unchanged_reuses_its_row() {
        assert!(unchanged(&previous(Some(7)), 5, 10, Some(20), Some(7)));
        assert!(!unchanged(&previous(Some(7)), 5, 10, Some(20), Some(8)));
        assert!(!unchanged(&previous(Some(7)), 5, 10, Some(21), Some(7)));
    }

    #[test]
    fn without_a_file_id_a_file_is_always_read_again() {
        // Windows: an in-place overwrite keeps the creation time and can restore the mtime.
        assert!(!unchanged(&previous(None), 5, 10, Some(20), None));
    }

    #[test]
    fn windows_verbatim_paths_lose_their_prefix_for_disks_and_shares() {
        assert_eq!(Some(r"C:\x".to_owned()), strip_verbatim(r"\\?\C:\x"));
        assert_eq!(
            Some(r"\\server\share\x".to_owned()),
            strip_verbatim(r"\\?\UNC\server\share\x")
        );
        assert_eq!(None, strip_verbatim(r"\\?\Volume{1}\x"));
        assert_eq!(None, strip_verbatim("/home/me"));
    }
}
