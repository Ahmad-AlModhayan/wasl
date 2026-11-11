//! The Windows back end: registration (WinRT `StorageProviderSyncRootManager`, so Explorer shows
//! the folder and its pin menu), the connection that serves fetches (`fetch`), placeholders kept
//! in step with the pair's tree (`sync`) and the pin-state watcher (`watch`).

mod cf;
mod fetch;
mod sync;
mod watch;

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::c_void;
use std::fs;
use std::future::Future;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use tokio::runtime::{Builder, Handle, Runtime};
use wasl_vfs::{FileRef, Hydrator, VfsError};
use windows::Security::Cryptography::CryptographicBuffer;
use windows::Storage::Provider::{
    StorageProviderHardlinkPolicy, StorageProviderHydrationPolicy, StorageProviderHydrationPolicyModifier,
    StorageProviderInSyncPolicy, StorageProviderPopulationPolicy, StorageProviderProtectionMode,
    StorageProviderSyncRootInfo, StorageProviderSyncRootManager,
};
use windows::Storage::StorageFolder;
use windows::core::HSTRING;

use crate::identity::{self, Identity};
use crate::{Error, FetchRecord, PROVIDER, PlaceholderState, Result, RootConfig};
use cf::{CF_CONNECTION_KEY, CF_PIN_STATE_PINNED, CF_PIN_STATE_UNSPECIFIED};
use fetch::Inflight;
use watch::Watcher;

const FILE_ATTRIBUTE_PINNED: u32 = 0x0008_0000;
const FILE_ATTRIBUTE_UNPINNED: u32 = 0x0010_0000;
/// Problems kept for [`OnDemandRoot::problems`] (the newest).
const PROBLEMS_KEPT: usize = 100;
/// Fetches kept for [`OnDemandRoot::recent_fetches`] (the newest).
const FETCHES_KEPT: usize = 200;

/// What the callbacks, the watcher and the root share.
pub(crate) struct Shared {
    pub(crate) hydrator: Arc<Hydrator>,
    pub(crate) rt: Handle,
    pub(crate) root: PathBuf,
    /// Placeholders are made read-only ([`RootConfig::read_only`]).
    pub(crate) read_only: bool,
    /// Where a changed placeholder of a read-only root is moved before Drive's version comes
    /// back ([`RootConfig::set_aside`]).
    pub(crate) set_aside: Option<PathBuf>,
    inflight: Mutex<HashMap<i64, Vec<Inflight>>>,
    problems: Mutex<Vec<String>>,
    fetches: Mutex<VecDeque<FetchRecord>>,
    /// One pin or unpin at a time (the CLI's and the watcher's for the same change would
    /// otherwise fetch the same blocks twice at once).
    pinning: Mutex<()>,
    /// The file ids of the tree the folder shows (`None` until the first `sync`). A fetch is
    /// served only for these: a placeholder's identity is written into the file, so a program
    /// that makes its own placeholder in the folder could otherwise have Wasl fetch any file
    /// the account can read (security review 2026-10).
    known: Mutex<Option<HashSet<String>>>,
}

impl Shared {
    /// Records the files of `tree`. During a sync both the old and the new tree's files are
    /// served (`keep_old`), so a placeholder being replaced never fails a read; afterwards only
    /// the new tree's.
    pub(crate) fn know(&self, tree: &wasl_vfs::Tree, keep_old: bool) {
        let ids = tree.iter().filter_map(|n| n.file.as_ref().map(|f| f.id.clone()));
        let mut known = lock(&self.known);
        match (keep_old, known.as_mut()) {
            (true, Some(k)) => k.extend(ids),
            _ => *known = Some(ids.collect()),
        }
    }

    /// Whether `id` is a file of the tree the folder shows.
    pub(crate) fn knows(&self, id: &str) -> bool {
        lock(&self.known).as_ref().is_some_and(|k| k.contains(id))
    }

    fn track(&self, transfer_key: i64, f: Inflight) {
        lock(&self.inflight).entry(transfer_key).or_default().push(f);
    }

    fn untrack(&self, transfer_key: i64, cancelled: &Arc<AtomicBool>) {
        let mut map = lock(&self.inflight);
        if let Some(v) = map.get_mut(&transfer_key) {
            v.retain(|f| !Arc::ptr_eq(&f.cancelled, cancelled));
            if v.is_empty() {
                map.remove(&transfer_key);
            }
        }
    }

    fn cancel(&self, transfer_key: i64, start: u64, end: u64) {
        if let Some(v) = lock(&self.inflight).get(&transfer_key) {
            for f in v.iter().filter(|f| f.start < end.max(start + 1) && start < f.end) {
                f.cancelled.store(true, Ordering::Release);
            }
        }
    }

    pub(crate) fn fetched(&self, r: FetchRecord) {
        let mut v = lock(&self.fetches);
        if v.len() >= FETCHES_KEPT {
            v.pop_front();
        }
        v.push_back(r);
    }

    fn problem(&self, p: String) {
        let mut v = lock(&self.problems);
        if v.len() >= PROBLEMS_KEPT {
            v.remove(0);
        }
        v.push(p);
    }

    /// Runs a future on Wasl's runtime and waits for it (from a thread that is not a runtime
    /// worker: the CLI, the watcher).
    fn run<T: Send + 'static>(&self, fut: impl Future<Output = T> + Send + 'static) -> Result<T> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.rt.spawn(async move {
            let _ = tx.send(fut.await);
        });
        rx.recv()
            .map_err(|_| Error::Vfs(VfsError::Unavailable("Wasl's runtime has stopped".into())))
    }

    /// The file a placeholder stands for (`None` for a folder).
    fn file_ref(&self, path: &Path, f: &fs::File) -> Result<(Option<FileRef>, cf::Info)> {
        let info = cf::info(f)
            .map_err(|e| os("reading the placeholder", path, e))?
            .ok_or_else(|| Error::NotOurs(path.display().to_string()))?;
        match identity::decode(&info.identity) {
            Some(Identity::File(r)) => Ok((Some(r), info)),
            Some(Identity::Dir) => Ok((None, info)),
            None => Err(Error::NotOurs(path.display().to_string())),
        }
    }

    /// Keeps a file complete on the device: its blocks pinned in the cache, the placeholder
    /// hydrated.
    fn pin_file(&self, path: &Path) -> Result<()> {
        let _one = lock(&self.pinning);
        let f = cf::open(path, true).map_err(|e| os("opening", path, e))?;
        let (Some(r), info) = self.file_ref(path, &f)? else {
            return Ok(());
        };
        if !self.hydrator.cache().is_pinned(&r.id) {
            let (h, r) = (self.hydrator.clone(), r.clone());
            self.run(async move { h.pin(&r).await })??;
        }
        if info.on_disk < r.size {
            cf::hydrate(&f).map_err(|e| os("hydrating", path, e))?;
        }
        Ok(())
    }

    /// Lets a file go: unpinned in the cache, its content dropped from the disk, its pin state
    /// back to the default.
    fn free_file(&self, path: &Path) -> Result<()> {
        let _one = lock(&self.pinning);
        let f = cf::open(path, true).map_err(|e| os("opening", path, e))?;
        let (Some(r), info) = self.file_ref(path, &f)? else {
            return Ok(());
        };
        self.hydrator.unpin(&r)?;
        if info.pin != CF_PIN_STATE_UNSPECIFIED {
            cf::set_pin(&f, CF_PIN_STATE_UNSPECIFIED, false)
                .map_err(|e| os("setting the pin state", path, e))?;
        }
        if info.on_disk > 0 {
            cf::dehydrate(&f).map_err(|e| os("freeing up space", path, e))?;
        }
        Ok(())
    }

    /// Acts on a path's pin state after its attributes changed (Explorer's menu, or a rescan when
    /// `path` is the root).
    fn apply_pin_state(&self, path: &Path) {
        if path == self.root {
            for p in walk(&self.root).into_iter().map(|e| e.path) {
                self.apply_pin_state(&p);
            }
            return;
        }
        let Ok(meta) = fs::symlink_metadata(path) else {
            return;
        };
        let attrs = meta.file_attributes();
        let result = if meta.is_dir() {
            if attrs & FILE_ATTRIBUTE_PINNED != 0 {
                self.each_file(path, |p| self.pin_file(p))
            } else if attrs & FILE_ATTRIBUTE_UNPINNED != 0 {
                // "Free up space" on a folder is a one-off request, like on a file.
                let r = self.each_file(path, |p| self.free_file(p));
                cf::open(path, true)
                    .and_then(|f| cf::set_pin(&f, CF_PIN_STATE_UNSPECIFIED, false))
                    .map_err(|e| os("setting the pin state", path, e))
                    .and(r)
            } else {
                Ok(())
            }
        } else if attrs & FILE_ATTRIBUTE_PINNED != 0 {
            self.pin_file(path)
        } else if attrs & FILE_ATTRIBUTE_UNPINNED != 0 {
            self.free_file(path)
        } else {
            Ok(())
        };
        match result {
            Ok(()) | Err(Error::NotOurs(_)) => {}
            Err(e) => self.problem(e.to_string()),
        }
    }

    /// Runs `f` on every placeholder file at or under `path`; the first error after trying all.
    fn each_file(&self, path: &Path, f: impl Fn(&Path) -> Result<()>) -> Result<()> {
        let files: Vec<PathBuf> = if fs::symlink_metadata(path)?.is_dir() {
            walk(path)
                .into_iter()
                .filter(|e| !e.is_dir)
                .map(|e| e.path)
                .collect()
        } else {
            vec![path.to_path_buf()]
        };
        let mut first = None;
        for p in files {
            match f(&p) {
                Ok(()) | Err(Error::NotOurs(_)) => {}
                Err(e) => {
                    first.get_or_insert(e);
                }
            }
        }
        first.map_or(Ok(()), Err)
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn os(op: &'static str, path: &Path, source: std::io::Error) -> Error {
    Error::Os {
        op,
        path: path.display().to_string(),
        source,
    }
}

/// Removes a file's read-only attribute (folders are left as they are).
fn clear_read_only(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_file() && meta.permissions().readonly() {
        let mut perms = meta.permissions();
        #[allow(clippy::permissions_set_readonly_false)] // Windows: just the attribute
        perms.set_readonly(false);
        fs::set_permissions(path, perms).map_err(|e| os("making writable", path, e))?;
    }
    Ok(())
}

fn winrt<'a>(op: &'static str, path: &'a Path) -> impl FnOnce(windows::core::Error) -> Error + 'a {
    move |e| os(op, path, e.into())
}

/// A file or folder found on disk.
pub(crate) struct Entry {
    /// `/`-separated, relative to the walk's start.
    pub(crate) rel: String,
    pub(crate) path: PathBuf,
    pub(crate) is_dir: bool,
}

/// Everything under `dir` (not following links), parents before children.
pub(crate) fn walk(dir: &Path) -> Vec<Entry> {
    fn go(dir: &Path, prefix: &str, out: &mut Vec<Entry>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        let mut items: Vec<_> = rd.flatten().collect();
        items.sort_by_key(|e| e.file_name());
        for e in items {
            let Ok(t) = e.file_type() else { continue };
            if t.is_symlink() {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let path = e.path();
            out.push(Entry {
                rel: rel.clone(),
                path: path.clone(),
                is_dir: t.is_dir(),
            });
            if t.is_dir() {
                go(&path, &rel, out);
            }
        }
    }
    let mut out = Vec::new();
    go(dir, "", &mut out);
    out
}

/// A pair's folder as a Cloud Files sync root, connected: its placeholders are served through
/// the [`Hydrator`] while this lives. Dropping it disconnects (the registration stays, as for
/// OneDrive while it is not running; reading a dehydrated file then fails until Wasl connects
/// again). [`OnDemandRoot::unregister`] or [`OnDemandRoot::hydrate_all_and_unregister`] end it.
///
/// The methods block (they wait for hydration); call them from a thread that is not a runtime
/// worker (`spawn_blocking` from async code).
pub struct OnDemandRoot {
    shared: Arc<Shared>,
    id: String,
    connection: Option<CF_CONNECTION_KEY>,
    /// The `Box<Arc<Shared>>` every callback gets, freed after disconnecting.
    context: usize,
    watcher: Option<Watcher>,
    runtime: Option<Runtime>,
}

impl OnDemandRoot {
    /// Registers `config.root` as a sync root (updating an existing registration of the same
    /// pair), connects, and starts acting on pin states. Fetches run on a runtime of its own.
    pub fn register(config: RootConfig, hydrator: Arc<Hydrator>) -> Result<Self> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(4)
            .thread_name("wasl-cf-fetch")
            .enable_all()
            .build()?;
        let handle = runtime.handle().clone();
        let mut root = Self::register_on(config, hydrator, handle)?;
        root.runtime = Some(runtime);
        Ok(root)
    }

    /// As [`OnDemandRoot::register`], with fetches on the given runtime (the one the
    /// [`Hydrator`]'s source belongs to).
    pub fn register_on(config: RootConfig, hydrator: Arc<Hydrator>, rt: Handle) -> Result<Self> {
        if config.pair_id.is_empty() || config.pair_id.contains('!') {
            return Err(Error::BadPairId(config.pair_id));
        }
        fs::create_dir_all(&config.root)?;
        let root = std::path::absolute(&config.root)?;
        let sid = cf::user_sid().map_err(|e| os("reading the user's SID", &root, e))?;
        let id = config.sync_root_id(&sid);
        register_sync_root(&config, &root, &id)?;
        let shared = Arc::new(Shared {
            hydrator,
            rt,
            root: root.clone(),
            read_only: config.read_only,
            set_aside: config.set_aside.clone().filter(|_| config.read_only),
            inflight: Mutex::new(HashMap::new()),
            problems: Mutex::new(Vec::new()),
            fetches: Mutex::new(VecDeque::new()),
            pinning: Mutex::new(()),
            known: Mutex::new(None),
        });
        let context = Box::into_raw(Box::new(shared.clone()));
        // SAFETY: CALLBACKS is 'static and ends with CF_CALLBACK_TYPE_NONE; `context` is a live
        // Box<Arc<Shared>>, freed only in `disconnect` after CfDisconnectSyncRoot returns (or
        // just below when connecting fails), which is what the callbacks assume.
        let connected = unsafe {
            cf::connect(
                &root,
                &fetch::CALLBACKS,
                context.cast::<c_void>(),
                config.block_self_hydration,
            )
        };
        let connection = match connected {
            Ok(k) => k,
            Err(e) => {
                // SAFETY: not connected, so no callback can hold the pointer; freed once.
                drop(unsafe { Box::from_raw(context) });
                return Err(os("connecting the sync root", &root, e));
            }
        };
        let mut me = Self {
            shared: shared.clone(),
            id,
            connection: Some(connection),
            context: context as usize,
            watcher: None,
            runtime: None,
        };
        me.watcher = Some(Watcher::start(shared).map_err(|e| os("watching", &root, e))?);
        Ok(me)
    }

    /// The pair's folder.
    pub fn root(&self) -> &Path {
        &self.shared.root
    }

    /// The sync root id (`Wasl!<SID>!<pair id>`).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The ranges Windows asked for lately, oldest first, with the program that caused each.
    pub fn recent_fetches(&self) -> Vec<FetchRecord> {
        lock(&self.shared.fetches).iter().cloned().collect()
    }

    /// Recent problems serving fetches or pin changes (they also show as errors to the reader).
    pub fn problems(&self) -> Vec<String> {
        lock(&self.shared.problems).clone()
    }

    /// A file's or folder's placeholder state; `None` when it is not a placeholder (a local
    /// file, or one made regular again).
    pub fn state(&self, path: impl AsRef<Path>) -> Result<Option<PlaceholderState>> {
        let p = self.resolve(path.as_ref())?;
        let f = cf::open(&p, false).map_err(|e| os("opening", &p, e))?;
        let Some(info) = cf::info(&f).map_err(|e| os("reading the placeholder", &p, e))? else {
            return Ok(None);
        };
        let file = match identity::decode(&info.identity) {
            Some(Identity::File(r)) => Some(r),
            _ => None,
        };
        Ok(Some(PlaceholderState {
            pinned: info.pin == CF_PIN_STATE_PINNED,
            in_sync: info.in_sync,
            on_disk: info.on_disk,
            file,
        }))
    }

    /// Keeps a file (or every file of a folder) complete on the device: pinned in the cache,
    /// hydrated, and shown as "Always keep on this device".
    pub fn pin(&self, path: impl AsRef<Path>) -> Result<()> {
        let p = self.resolve(path.as_ref())?;
        let is_dir = fs::metadata(&p)?.is_dir();
        let f = cf::open(&p, true).map_err(|e| os("opening", &p, e))?;
        cf::set_pin(&f, CF_PIN_STATE_PINNED, is_dir).map_err(|e| os("pinning", &p, e))?;
        drop(f);
        self.shared.each_file(&p, |f| self.shared.pin_file(f))
    }

    /// Ends a pin: the content stays until the cache or Windows needs the space.
    pub fn unpin(&self, path: impl AsRef<Path>) -> Result<()> {
        let p = self.resolve(path.as_ref())?;
        let is_dir = fs::metadata(&p)?.is_dir();
        let f = cf::open(&p, true).map_err(|e| os("opening", &p, e))?;
        cf::set_pin(&f, CF_PIN_STATE_UNSPECIFIED, is_dir).map_err(|e| os("unpinning", &p, e))?;
        drop(f);
        self.shared.each_file(&p, |path| {
            let f = cf::open(path, false).map_err(|e| os("opening", path, e))?;
            if let (Some(r), _) = self.shared.file_ref(path, &f)? {
                self.shared.hydrator.unpin(&r)?;
            }
            Ok(())
        })
    }

    /// Frees up space: unpins and drops the content of a file (or every file of a folder); it
    /// stays as a placeholder and is read again on demand.
    pub fn dehydrate(&self, path: impl AsRef<Path>) -> Result<()> {
        let p = self.resolve(path.as_ref())?;
        if fs::metadata(&p)?.is_dir() {
            let f = cf::open(&p, true).map_err(|e| os("opening", &p, e))?;
            cf::set_pin(&f, CF_PIN_STATE_UNSPECIFIED, false).map_err(|e| os("unpinning", &p, e))?;
        }
        self.shared.each_file(&p, |f| self.shared.free_file(f))
    }

    /// Drops the content of every file that is not pinned, so nothing read on demand stays on
    /// the disk; they stay placeholders, read again on demand. For a vault's view when the vault
    /// is locked: disconnecting alone would leave decrypted content in the folder (security
    /// review 2026-10). Pinned files ("Always keep on this device") keep theirs: the person chose
    /// to have them on this computer. Files that are not Wasl's are left alone. A file changed
    /// here is set aside first and freed too in a read-only root with a set-aside folder
    /// ([`RootConfig::set_aside`]; E2E 2026-10: a vault view's edited file kept its plaintext
    /// after the lock), and left alone otherwise. Every file is tried; the first error is
    /// returned.
    pub fn dehydrate_unpinned(&self) -> Result<()> {
        let root = self.root().to_path_buf();
        self.shared.each_file(&root, |p| {
            let _one = lock(&self.shared.pinning);
            let f = cf::open(p, false).map_err(|e| os("opening", p, e))?;
            let Some(info) = cf::info(&f).map_err(|e| os("reading the placeholder", p, e))? else {
                return Ok(());
            };
            if !info.in_sync
                && info.on_disk > 0
                && let Some(aside) = &self.shared.set_aside
            {
                // Changed here in a read-only root (its attribute cleared): the person's bytes
                // go aside, and Drive's version is read again on demand. Pinned or not, the
                // folder keeps nothing written here.
                drop(f);
                return self.set_aside_and_free(p, aside).map(|_| ());
            }
            if info.pin == CF_PIN_STATE_PINNED || info.on_disk == 0 || !info.in_sync {
                return Ok(());
            }
            drop(f);
            let f = cf::open(p, true).map_err(|e| os("opening", p, e))?;
            cf::dehydrate(&f).map_err(|e| os("freeing up space", p, e))
        })
    }

    /// After the engine uploaded a local change: the file is in sync again as `file` (a regular
    /// file in the folder becomes a full placeholder).
    pub fn mark_in_sync(&self, path: impl AsRef<Path>, file: &FileRef) -> Result<()> {
        let p = self.resolve(path.as_ref())?;
        let blob = identity::encode_file(file).ok_or_else(|| Error::NotOurs(file.id.clone()))?;
        let f = cf::open(&p, true).map_err(|e| os("opening", &p, e))?;
        match cf::info(&f).map_err(|e| os("reading the placeholder", &p, e))? {
            Some(_) => cf::mark_in_sync(&f, &blob).map_err(|e| os("marking in sync", &p, e)),
            None => cf::convert(&f, &blob).map_err(|e| os("converting", &p, e)),
        }
    }

    /// Turning on-demand off without losing anything: hydrates every file, makes every
    /// placeholder a regular file again, then unregisters. Nothing is unregistered if a file
    /// cannot be fetched (the error says which); the folder stays on demand.
    pub fn hydrate_all_and_unregister(mut self) -> Result<()> {
        if let Some(mut w) = self.watcher.take() {
            w.stop();
        }
        let entries = walk(self.root());
        let mut placeholders = Vec::new();
        let outcome = (|| {
            for e in &entries {
                // Read-only first: local files that are not placeholders are left as they are.
                let f = cf::open(&e.path, false).map_err(|x| os("opening", &e.path, x))?;
                let Some(info) = cf::info(&f).map_err(|x| os("reading", &e.path, x))? else {
                    continue;
                };
                if !e.is_dir && info.on_disk < fs::metadata(&e.path)?.len() {
                    let f = cf::open(&e.path, true).map_err(|x| os("opening", &e.path, x))?;
                    cf::hydrate(&f).map_err(|x| os("hydrating", &e.path, x))?;
                }
                placeholders.push(e.path.clone());
            }
            // Children before their folders.
            for p in placeholders.iter().rev() {
                let f = cf::open(p, true).map_err(|x| os("opening", p, x))?;
                cf::revert(&f).map_err(|x| os("making a regular file", p, x))?;
                drop(f);
                if self.shared.read_only {
                    // The files are the person's own now: writable like any other.
                    clear_read_only(p)?;
                }
            }
            Ok(())
        })();
        if let Err(e) = outcome {
            match Watcher::start(self.shared.clone()) {
                Ok(w) => self.watcher = Some(w),
                Err(w) => self.shared.problem(format!("watching pin states again: {w}")),
            }
            return Err(e);
        }
        self.unregister()
    }

    /// Disconnects and removes the registration; placeholders stay as they are (dehydrated ones
    /// cannot be read until the folder is registered again).
    pub fn unregister(mut self) -> Result<()> {
        self.disconnect()?;
        unregister_sync_root(&self.id)
    }

    fn disconnect(&mut self) -> Result<()> {
        if let Some(mut w) = self.watcher.take() {
            w.stop();
        }
        let mut out = Ok(());
        if let Some(key) = self.connection.take() {
            out = cf::disconnect(key).map_err(|e| os("disconnecting", &self.shared.root, e));
            if self.context != 0 {
                // SAFETY: CfDisconnectSyncRoot has returned, so no callback runs any more and none
                // holds the pointer; it came from Box::into_raw in `register_on`, freed once
                // (the field is cleared). Even when disconnecting reports an error the connection
                // is gone, as nothing can use the key again.
                drop(unsafe { Box::from_raw(self.context as *mut Arc<Shared>) });
                self.context = 0;
            }
        }
        out
    }

    /// An absolute path in the folder, from a path relative to it (`/` or `\`) or absolute.
    fn resolve(&self, p: &Path) -> Result<PathBuf> {
        let root = self.root();
        let abs = if p.is_absolute() {
            std::path::absolute(p)?
        } else {
            root.join(p.to_string_lossy().replace('/', "\\"))
        };
        let (a, r) = (
            abs.to_string_lossy().to_lowercase(),
            root.to_string_lossy().to_lowercase(),
        );
        let r = r.trim_end_matches('\\');
        if a.len() > r.len() && a.starts_with(r) && a[r.len()..].starts_with('\\') {
            Ok(abs)
        } else {
            Err(Error::Outside(p.display().to_string()))
        }
    }
}

impl Drop for OnDemandRoot {
    fn drop(&mut self) {
        let _ = self.disconnect();
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_background();
        }
    }
}

/// Registers (or updates) the sync root with the shell, so Explorer shows it with its status
/// icons and the "Always keep on this device" / "Free up space" menu.
fn register_sync_root(config: &RootConfig, root: &Path, id: &str) -> Result<()> {
    let icon = config.icon.clone().unwrap_or_else(|| {
        let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        format!(r"{sys}\System32\imageres.dll,-1043")
    });
    let e = |op| winrt(op, root);
    let folder = StorageFolder::GetFolderFromPathAsync(&HSTRING::from(root))
        .and_then(|op| op.join())
        .map_err(e("opening the folder"))?;
    let info = StorageProviderSyncRootInfo::new().map_err(e("describing the sync root"))?;
    (|| {
        info.SetId(&HSTRING::from(id))?;
        info.SetPath(&folder)?;
        info.SetDisplayNameResource(&HSTRING::from(config.display_name.as_str()))?;
        info.SetIconResource(&HSTRING::from(icon.as_str()))?;
        info.SetVersion(&HSTRING::from(config.version.as_str()))?;
        // Partial: a read fetches only the range it needs and the rest stays in the cloud.
        info.SetHydrationPolicy(StorageProviderHydrationPolicy::Partial)?;
        info.SetHydrationPolicyModifier(StorageProviderHydrationPolicyModifier::AutoDehydrationAllowed)?;
        // Wasl creates every placeholder itself, so Windows never asks for a folder's listing.
        info.SetPopulationPolicy(StorageProviderPopulationPolicy::AlwaysFull)?;
        info.SetInSyncPolicy(StorageProviderInSyncPolicy::Default)?;
        info.SetHardlinkPolicy(StorageProviderHardlinkPolicy::None)?;
        info.SetProtectionMode(StorageProviderProtectionMode::Unknown)?;
        info.SetShowSiblingsAsGroup(false)?;
        info.SetAllowPinning(true)?;
        info.SetContext(&CryptographicBuffer::CreateFromByteArray(
            config.pair_id.as_bytes(),
        )?)?;
        StorageProviderSyncRootManager::Register(&info)
    })()
    .map_err(e("registering the sync root"))
}

/// Whether `path` is a registered sync root or inside one.
pub fn is_sync_root(path: &Path) -> bool {
    cf::in_sync_root(path)
}

/// Where the shell keeps sync root registrations: a key per id, with the folder per user SID.
const SYNC_ROOT_MANAGER: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\SyncRootManager";

/// Wasl's registered sync roots for this user: (id, folder). Read from the shell's registry
/// keys, because `StorageProviderSyncRootManager::GetCurrentSyncRoots` lists only the roots of
/// the calling app's package (none for an unpackaged program like wasld).
pub fn registered_sync_roots() -> Result<Vec<(String, PathBuf)>> {
    let sid = cf::user_sid().map_err(|e| os("reading the user's SID", Path::new(""), e))?;
    let reg = |e: windows::core::Error| os("reading", Path::new(SYNC_ROOT_MANAGER), e.into());
    let manager = windows_registry::LOCAL_MACHINE
        .open(SYNC_ROOT_MANAGER)
        .map_err(reg)?;
    let prefix = format!("{PROVIDER}!{sid}!");
    let mut out = Vec::new();
    for id in manager.keys().map_err(reg)? {
        if !id.starts_with(&prefix) {
            continue;
        }
        let path = manager
            .open(format!(r"{id}\UserSyncRoots"))
            .and_then(|k| k.get_string(&sid))
            .map(PathBuf::from)
            .unwrap_or_default();
        out.push((id, path));
    }
    Ok(out)
}

/// Removes a registration by id (a pair removed while Wasl was not running, or a crash's),
/// also when its folder has been deleted since.
pub fn unregister_sync_root(id: &str) -> Result<()> {
    let unregister = || {
        StorageProviderSyncRootManager::Unregister(&HSTRING::from(id))
            .map_err(winrt("unregistering the sync root", Path::new(id)))
    };
    let Err(e) = unregister() else { return Ok(()) };
    let Some((_, folder)) = registered_sync_roots()?.into_iter().find(|(i, _)| i == id) else {
        return Err(e);
    };
    if folder.as_os_str().is_empty() {
        // Half written (the process stopped while registering): complete it, then remove it.
        let tmp = std::env::temp_dir().join(format!("wasl-unregister-{}", std::process::id()));
        fs::create_dir_all(&tmp)?;
        let config = RootConfig {
            root: tmp.clone(),
            pair_id: id.rsplit('!').next().unwrap_or(id).to_owned(),
            display_name: PROVIDER.into(),
            version: "0".into(),
            icon: None,
            block_self_hydration: false,
            read_only: false,
            set_aside: None,
        };
        let r = register_sync_root(&config, &tmp, id).and_then(|()| unregister());
        let _ = fs::remove_dir(&tmp);
        r
    } else if !folder.exists() {
        // Windows will not unregister a root whose folder is gone: put an empty one back for it.
        fs::create_dir_all(&folder)?;
        let r = unregister();
        let _ = fs::remove_dir(&folder);
        r
    } else {
        Err(e)
    }
}
