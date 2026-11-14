//! A test world: a real temp folder, the in-memory Drive, a controllable clock and device state.
#![allow(dead_code)]

use async_trait::async_trait;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wasl_core::RemoteEntry;
use wasl_core::conditions::{Conditions, DeviceState};
use wasl_engine::{
    Clock, Engine, EngineConfig, Environment, Listing, MemoryRemote, Progress, ProgressFn, Remote,
    RemoteInfo, UploadSpec,
};
use wasl_fs::{LocalFs, StdFs};
use wasl_store::{Pair, SharedStore, Store};

pub struct TestClock(pub AtomicI64);

impl Clock for TestClock {
    fn now_ms(&self) -> i64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
    fn stamp(&self) -> String {
        "2026-10-20 09-00".into()
    }
}

impl TestClock {
    pub fn advance(&self, ms: i64) {
        self.0.fetch_add(ms, Ordering::Relaxed);
    }
}

pub struct TestEnv {
    pub root: PathBuf,
    pub bin: PathBuf,
    pub remote: Arc<dyn Remote>,
    pub device: Mutex<DeviceState>,
    /// Set to make every file open slow and counted (a phone's shared storage, to stop mid-way).
    pub slow_opens: Mutex<Option<Arc<SlowOpens>>>,
    /// Set to make the pair's folder fold case and accents on any host (see [`Folding`]).
    pub folding: Mutex<Option<Folding>>,
}

impl Environment for TestEnv {
    fn local_fs(&self, _pair: &Pair) -> wasl_engine::Result<Arc<dyn LocalFs>> {
        let fs = Arc::new(StdFs::new(&self.root, &self.bin));
        if let Some(folding) = *self.folding.lock().unwrap() {
            return Ok(Arc::new(FoldedFs {
                inner: fs,
                root: self.root.clone(),
                folding,
            }));
        }
        Ok(match self.slow_opens.lock().unwrap().clone() {
            Some(slow) => Arc::new(SlowOpenFs { inner: fs, slow }),
            None => fs,
        })
    }
    fn remote(&self, _pair: &Pair) -> wasl_engine::Result<Arc<dyn Remote>> {
        Ok(self.remote.clone())
    }
    fn device_state(&self) -> DeviceState {
        self.device.lock().unwrap().clone()
    }
}

pub struct World {
    pub dir: tempfile::TempDir,
    pub drive: Arc<MemoryRemote>,
    pub env: Arc<TestEnv>,
    pub clock: Arc<TestClock>,
    pub engine: Engine,
    pub pair_id: String,
    /// The most bytes any progress snapshot showed done, and that snapshot's total.
    pub most_done: Arc<Mutex<(u64, u64)>>,
}

pub const PAIR: &str = "p1";
/// This device's id in every world whose config names none, as the daemon passes it: agent
/// changes are written to the op-log under it (ADR-0002).
pub const DEVICE: &str = "WASL-0123-4567-89AB-CDEF-GHJK-MNPQ-RSTV";

/// Wraps the world's in-memory Drive (to slow it down, make it hang…).
pub type Wrap = Box<dyn FnOnce(Arc<MemoryRemote>) -> Arc<dyn Remote>>;
pub const DAY: i64 = 24 * 3600 * 1000;

impl World {
    pub fn new() -> World {
        Self::with(|_| {}, None)
    }

    /// A world whose pair is adjusted by `edit`; `wrap` wraps the in-memory Drive.
    pub fn with(edit: impl FnOnce(&mut Pair), wrap: Option<Wrap>) -> World {
        Self::with_config(
            edit,
            wrap,
            EngineConfig {
                device_name: "Pixel".into(),
                ..EngineConfig::default()
            },
        )
    }

    pub fn with_config(edit: impl FnOnce(&mut Pair), wrap: Option<Wrap>, config: EngineConfig) -> World {
        Self::on_drive(Arc::new(MemoryRemote::new()), edit, wrap, config)
    }

    /// A world for another device whose pair syncs with this world's Drive folder.
    pub fn another_device(&self, folding: Option<Folding>) -> World {
        let w = Self::on_drive(
            self.drive.clone(),
            |_| {},
            None,
            EngineConfig {
                device_name: "Phone".into(),
                ..EngineConfig::default()
            },
        );
        *w.env.folding.lock().unwrap() = folding;
        w
    }

    fn on_drive(
        drive: Arc<MemoryRemote>,
        edit: impl FnOnce(&mut Pair),
        wrap: Option<Wrap>,
        mut config: EngineConfig,
    ) -> World {
        config.device_id.get_or_insert_with(|| DEVICE.into());
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("pair");
        std::fs::create_dir_all(&root).unwrap();
        let folder = drive.mkdirs("Sync");
        let env = Arc::new(TestEnv {
            root,
            bin: dir.path().join("bin"),
            remote: match wrap {
                Some(w) => w(drive.clone()),
                None => drive.clone(),
            },
            device: Mutex::new(DeviceState::unrestricted()),
            slow_opens: Mutex::new(None),
            folding: Mutex::new(None),
        });
        let store = SharedStore::new(Store::open_in_memory().unwrap());
        let mut pair = Pair::new(PAIR, "Sync", env.root.to_str().unwrap());
        pair.drive_folder_id = Some(folder);
        pair.remote_path = "My Drive/Sync".into();
        pair.account_id = Some("me@example.com".into());
        pair.conditions = Conditions::none();
        edit(&mut pair);
        store.lock().create_pair(&pair).unwrap();
        let clock = Arc::new(TestClock(AtomicI64::new(1_760_000_000_000)));
        let most_done: Arc<Mutex<(u64, u64)>> = Arc::default();
        let listener = {
            let most_done = most_done.clone();
            Arc::new(move |p: &Progress| {
                let mut m = most_done.lock().unwrap();
                if p.bytes_done >= m.0 {
                    *m = (p.bytes_done, p.bytes_total);
                }
            })
        };
        let engine = Engine::new(store, env.clone(), clock.clone(), config, Some(listener));
        World {
            dir,
            drive,
            env,
            clock,
            engine,
            pair_id: PAIR.into(),
            most_done,
        }
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.env.root.join(rel)
    }

    pub fn write(&self, rel: &str, content: &str) {
        let p = self.path(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, content).unwrap();
        // Distinct modification times, like real edits seconds apart.
        let t = filetime::FileTime::from_unix_time(1_760_000_000 + (self.clock.now_ms() % 1_000_000), 0);
        filetime::set_file_mtime(&p, t).unwrap();
    }

    pub fn read(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.path(rel)).ok()
    }

    pub fn remove(&self, rel: &str) {
        std::fs::remove_file(self.path(rel)).unwrap();
    }

    /// Every local file under the pair as path → content (part files excluded).
    pub fn local_files(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        walk(&self.env.root, "", &mut out);
        out
    }

    /// Every Drive file under the pair's folder as path → content.
    pub fn drive_files(&self) -> BTreeMap<String, String> {
        self.drive
            .files()
            .into_iter()
            .filter_map(|(k, v)| {
                k.strip_prefix("Sync/")
                    .map(|k| (k.to_owned(), String::from_utf8(v).unwrap()))
            })
            .collect()
    }

    pub fn put_drive(&self, rel: &str, content: &str) -> String {
        self.drive.put_file(&format!("Sync/{rel}"), content.as_bytes())
    }

    pub fn pair(&self) -> Pair {
        self.engine
            .store()
            .lock()
            .get_pair(&self.pair_id)
            .unwrap()
            .unwrap()
    }

    pub fn set_pair(&self, edit: impl FnOnce(&mut Pair)) {
        let mut p = self.pair();
        edit(&mut p);
        let s = self.engine.store().lock();
        s.update_pair(&p).unwrap();
        s.set_pair_status(&p.id, &p.status).unwrap();
    }

    pub async fn sync(&self) -> wasl_engine::RunOutcome {
        self.engine
            .sync(&self.pair_id, wasl_engine::Trigger::Manual)
            .await
    }

    /// Syncs, approving the first-merge stop as the preview would.
    pub async fn sync_from_preview(&self) -> wasl_engine::RunOutcome {
        self.engine
            .sync(&self.pair_id, wasl_engine::Trigger::Preview)
            .await
    }
}

fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.ends_with(".wasl-part") {
            continue;
        }
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if e.file_type().unwrap().is_dir() {
            walk(&e.path(), &rel, out);
        } else {
            out.insert(rel, std::fs::read_to_string(e.path()).unwrap_or_default());
        }
    }
}

pub fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

/// A remote whose first upload finds the device file saved again by another app: just before it
/// reads the file (`before`) and/or just after (`after`).
pub struct Saving {
    pub inner: Arc<MemoryRemote>,
    pub saves: Mutex<Option<(Option<&'static str>, Option<&'static str>)>>,
}

#[async_trait]
impl Remote for Saving {
    async fn list_subtree(&self, root_id: &str, drive_id: Option<&str>) -> wasl_drive::Result<Listing> {
        self.inner.list_subtree(root_id, drive_id).await
    }
    async fn download(
        &self,
        entry: &RemoteEntry,
        dest: &Path,
        progress: ProgressFn<'_>,
    ) -> wasl_drive::Result<()> {
        self.inner.download(entry, dest, progress).await
    }
    async fn export(
        &self,
        entry: &RemoteEntry,
        mime: &str,
        dest: &Path,
        progress: ProgressFn<'_>,
    ) -> wasl_drive::Result<()> {
        self.inner.export(entry, mime, dest, progress).await
    }
    async fn upload(&self, spec: UploadSpec<'_>, progress: ProgressFn<'_>) -> wasl_drive::Result<RemoteInfo> {
        let saves = self.saves.lock().unwrap().take();
        let src = spec.src.to_path_buf();
        if let Some((Some(before), _)) = saves {
            std::fs::write(&src, before).unwrap();
        }
        let info = self.inner.upload(spec, progress).await;
        if let Some((_, Some(after))) = saves {
            std::fs::write(&src, after).unwrap();
        }
        info
    }
    async fn current(&self, id: &str) -> wasl_drive::Result<Option<RemoteInfo>> {
        self.inner.current(id).await
    }
    async fn create_folder(&self, parent_id: &str, name: &str) -> wasl_drive::Result<RemoteInfo> {
        self.inner.create_folder(parent_id, name).await
    }
    async fn trash(&self, id: &str) -> wasl_drive::Result<()> {
        self.inner.trash(id).await
    }
    async fn untrash(&self, id: &str) -> wasl_drive::Result<()> {
        self.inner.untrash(id).await
    }
}

/// A remote whose uploads of the file named `name` go wrong after the device file was read: the
/// first `damaged` of them store altered bytes (a transfer Drive received damaged), and with
/// `delete_after` the device file is deleted once its first upload is done.
pub struct Damaging {
    pub inner: Arc<MemoryRemote>,
    pub name: &'static str,
    pub damaged: Mutex<usize>,
    pub delete_after: Mutex<bool>,
}

impl Damaging {
    pub fn wrap(name: &'static str, damaged: usize, delete_after: bool) -> Wrap {
        Box::new(move |m| {
            Arc::new(Damaging {
                inner: m,
                name,
                damaged: Mutex::new(damaged),
                delete_after: Mutex::new(delete_after),
            })
        })
    }
}

#[async_trait]
impl Remote for Damaging {
    async fn list_subtree(&self, root_id: &str, drive_id: Option<&str>) -> wasl_drive::Result<Listing> {
        self.inner.list_subtree(root_id, drive_id).await
    }
    async fn download(
        &self,
        entry: &RemoteEntry,
        dest: &Path,
        progress: ProgressFn<'_>,
    ) -> wasl_drive::Result<()> {
        self.inner.download(entry, dest, progress).await
    }
    async fn export(
        &self,
        entry: &RemoteEntry,
        mime: &str,
        dest: &Path,
        progress: ProgressFn<'_>,
    ) -> wasl_drive::Result<()> {
        self.inner.export(entry, mime, dest, progress).await
    }
    async fn upload(&self, spec: UploadSpec<'_>, progress: ProgressFn<'_>) -> wasl_drive::Result<RemoteInfo> {
        if spec.name != self.name {
            return self.inner.upload(spec, progress).await;
        }
        let src = spec.src.to_path_buf();
        let damage = {
            let mut n = self.damaged.lock().unwrap();
            let d = *n > 0;
            *n = n.saturating_sub(1);
            d
        };
        let info = if damage {
            let mut bytes = std::fs::read(&src).unwrap();
            bytes.extend_from_slice(b" (damaged)");
            let altered = src.with_extension("damaged");
            std::fs::write(&altered, bytes).unwrap();
            let info = self
                .inner
                .upload(
                    UploadSpec {
                        src: &altered,
                        ..spec
                    },
                    progress,
                )
                .await;
            let _ = std::fs::remove_file(&altered);
            info
        } else {
            self.inner.upload(spec, progress).await
        };
        if std::mem::take(&mut *self.delete_after.lock().unwrap()) {
            std::fs::remove_file(&src).unwrap();
        }
        info
    }
    async fn current(&self, id: &str) -> wasl_drive::Result<Option<RemoteInfo>> {
        self.inner.current(id).await
    }
    async fn create_folder(&self, parent_id: &str, name: &str) -> wasl_drive::Result<RemoteInfo> {
        self.inner.create_folder(parent_id, name).await
    }
    async fn trash(&self, id: &str) -> wasl_drive::Result<()> {
        self.inner.trash(id).await
    }
    async fn untrash(&self, id: &str) -> wasl_drive::Result<()> {
        self.inner.untrash(id).await
    }
}

/// A remote that delays every transfer, or hangs forever on downloads, to exercise cancellation,
/// resume and the stall watchdog.
pub struct Slow {
    pub inner: Arc<MemoryRemote>,
    pub delay: Duration,
    pub hang_downloads: bool,
    /// Listing never answers, as a Drive call stuck past every timeout would.
    pub hang_listing: bool,
}

#[async_trait]
impl Remote for Slow {
    async fn list_subtree(&self, root_id: &str, drive_id: Option<&str>) -> wasl_drive::Result<Listing> {
        if self.hang_listing {
            std::future::pending::<()>().await;
        }
        self.inner.list_subtree(root_id, drive_id).await
    }
    async fn download(
        &self,
        entry: &RemoteEntry,
        dest: &Path,
        progress: ProgressFn<'_>,
    ) -> wasl_drive::Result<()> {
        if self.hang_downloads {
            std::future::pending::<()>().await;
        }
        tokio::time::sleep(self.delay).await;
        self.inner.download(entry, dest, progress).await
    }
    async fn export(
        &self,
        entry: &RemoteEntry,
        mime: &str,
        dest: &Path,
        progress: ProgressFn<'_>,
    ) -> wasl_drive::Result<()> {
        self.inner.export(entry, mime, dest, progress).await
    }
    async fn upload(&self, spec: UploadSpec<'_>, progress: ProgressFn<'_>) -> wasl_drive::Result<RemoteInfo> {
        tokio::time::sleep(self.delay).await;
        self.inner.upload(spec, progress).await
    }
    async fn current(&self, id: &str) -> wasl_drive::Result<Option<RemoteInfo>> {
        self.inner.current(id).await
    }
    async fn create_folder(&self, parent_id: &str, name: &str) -> wasl_drive::Result<RemoteInfo> {
        self.inner.create_folder(parent_id, name).await
    }
    async fn trash(&self, id: &str) -> wasl_drive::Result<()> {
        self.inner.trash(id).await
    }
    async fn untrash(&self, id: &str) -> wasl_drive::Result<()> {
        self.inner.untrash(id).await
    }
}

/// How slow each file open is, and how many there were.
#[derive(Default)]
pub struct SlowOpens {
    pub delay: Duration,
    pub opened: std::sync::atomic::AtomicUsize,
    /// Files whose opens fail with `Permission denied`, as Android refuses a file the app may not
    /// open by its path (#127).
    pub denied: Mutex<Vec<String>>,
    /// Files the scan listed but whose opens find nothing, as a file moved or deleted between the
    /// listing and the read (#154).
    pub gone: Mutex<Vec<String>>,
    /// Listings of the pair's root that still work; after that the root is gone (a card taken out).
    pub root_listings_left: Mutex<Option<usize>>,
}

impl SlowOpens {
    fn check(&self, rel: &str) -> std::io::Result<()> {
        if self.denied.lock().unwrap().iter().any(|d| d == rel) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Permission denied (os error 13)",
            ));
        }
        if self.gone.lock().unwrap().iter().any(|d| d == rel) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{rel} was moved or deleted while Wasl was reading it"),
            ));
        }
        Ok(())
    }
}

/// The pair's folder with every open made slow and counted.
struct SlowOpenFs {
    inner: Arc<StdFs>,
    slow: Arc<SlowOpens>,
}

impl LocalFs for SlowOpenFs {
    fn list(&self, dir: &str) -> std::io::Result<Vec<wasl_fs::FsEntry>> {
        if dir.is_empty()
            && let Some(left) = self.slow.root_listings_left.lock().unwrap().as_mut()
        {
            if *left == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "No such device",
                ));
            }
            *left -= 1;
        }
        self.inner.list(dir)
    }
    fn stat(&self, rel: &str) -> std::io::Result<Option<wasl_fs::FsEntry>> {
        self.inner.stat(rel)
    }
    fn open_read(&self, rel: &str) -> std::io::Result<Box<dyn std::io::Read + Send>> {
        std::thread::sleep(self.slow.delay);
        self.slow.opened.fetch_add(1, Ordering::SeqCst);
        self.slow.check(rel)?;
        self.inner.open_read(rel)
    }
    fn readable_path(&self, rel: &str) -> std::io::Result<wasl_fs::ReadablePath> {
        self.slow.check(rel)?;
        self.inner.readable_path(rel)
    }
    fn staging_path(&self, rel: &str) -> std::io::Result<PathBuf> {
        self.inner.staging_path(rel)
    }
    fn install(&self, staged: &Path, rel: &str, mtime_ms: Option<i64>) -> std::io::Result<wasl_fs::FsEntry> {
        self.inner.install(staged, rel, mtime_ms)
    }
    fn mkdir(&self, rel: &str) -> std::io::Result<()> {
        self.inner.mkdir(rel)
    }
    fn remove_file(&self, rel: &str) -> std::io::Result<()> {
        self.inner.remove_file(rel)
    }
    fn remove_dir_if_empty(&self, rel: &str) -> std::io::Result<bool> {
        self.inner.remove_dir_if_empty(rel)
    }
    fn rename(&self, from: &str, to: &str) -> std::io::Result<()> {
        self.inner.rename(from, to)
    }
    fn move_to_bin(&self, rel: &str) -> std::io::Result<String> {
        self.inner.move_to_bin(rel)
    }
    fn restore_from_bin(&self, location: &str, rel: &str) -> std::io::Result<()> {
        self.inner.restore_from_bin(location, rel)
    }
    fn purge_from_bin(&self, location: &str) -> std::io::Result<()> {
        self.inner.purge_from_bin(location)
    }
}

/// `root` as a folder that folds names (see [`FoldedFs`]), for tests that build their own device.
pub fn folded_fs(root: &Path, bin: &Path, folding: Folding) -> Arc<dyn LocalFs> {
    Arc::new(FoldedFs {
        inner: Arc::new(StdFs::new(root, bin)),
        root: root.to_path_buf(),
        folding,
    })
}

/// How a [`FoldedFs`] folds names.
#[derive(Clone, Copy, Debug)]
pub struct Folding {
    /// Android's shared storage: creating a file whose name is taken in another case makes
    /// `Name (1).ext` instead, as the system's document provider does (#137).
    pub android: bool,
}

/// The pair's folder as Windows, macOS or Android's shared storage see it, on any host: a name
/// finds the item spelled the same under `wasl_core::identity::name_key` (case and accents
/// folded), and a case-only rename renames in place.
struct FoldedFs {
    inner: Arc<StdFs>,
    root: PathBuf,
    folding: Folding,
}

impl FoldedFs {
    /// `rel` with each part replaced by the existing item it folds to.
    fn resolve(&self, rel: &str) -> String {
        let mut dir = self.root.clone();
        let mut out: Vec<String> = Vec::new();
        for part in rel.split('/').filter(|p| !p.is_empty()) {
            let found = std::fs::read_dir(&dir).ok().and_then(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter_map(|e| e.file_name().into_string().ok())
                    .find(|n| n == part)
                    .or_else(|| {
                        std::fs::read_dir(&dir).ok().and_then(|rd| {
                            rd.filter_map(|e| e.ok())
                                .filter_map(|e| e.file_name().into_string().ok())
                                .find(|n| wasl_core::identity::same_name(n, part))
                        })
                    })
            });
            let name = found.unwrap_or_else(|| part.to_owned());
            dir.push(&name);
            out.push(name);
        }
        out.join("/")
    }
}

impl LocalFs for FoldedFs {
    fn list(&self, dir: &str) -> std::io::Result<Vec<wasl_fs::FsEntry>> {
        self.inner.list(&self.resolve(dir))
    }
    fn stat(&self, rel: &str) -> std::io::Result<Option<wasl_fs::FsEntry>> {
        self.inner.stat(&self.resolve(rel))
    }
    fn open_read(&self, rel: &str) -> std::io::Result<Box<dyn std::io::Read + Send>> {
        self.inner.open_read(&self.resolve(rel))
    }
    fn readable_path(&self, rel: &str) -> std::io::Result<wasl_fs::ReadablePath> {
        self.inner.readable_path(&self.resolve(rel))
    }
    fn staging_path(&self, rel: &str) -> std::io::Result<PathBuf> {
        self.inner.staging_path(&self.resolve(rel))
    }
    fn install(&self, staged: &Path, rel: &str, mtime_ms: Option<i64>) -> std::io::Result<wasl_fs::FsEntry> {
        let found = self.resolve(rel);
        if self.folding.android && found != rel && self.inner.stat(&found)?.is_some() {
            // The provider keeps the other spelling and writes a numbered copy beside it.
            let (stem, ext) = match rel.rsplit_once('.') {
                Some((s, e)) if !s.ends_with('/') && !s.is_empty() => (s, format!(".{e}")),
                _ => (rel, String::new()),
            };
            return self.inner.install(staged, &format!("{stem} (1){ext}"), mtime_ms);
        }
        self.inner.install(staged, &found, mtime_ms)
    }
    fn mkdir(&self, rel: &str) -> std::io::Result<()> {
        self.inner.mkdir(&self.resolve(rel))
    }
    fn remove_file(&self, rel: &str) -> std::io::Result<()> {
        self.inner.remove_file(&self.resolve(rel))
    }
    fn remove_dir_if_empty(&self, rel: &str) -> std::io::Result<bool> {
        self.inner.remove_dir_if_empty(&self.resolve(rel))
    }
    fn rename(&self, from: &str, to: &str) -> std::io::Result<()> {
        let from = self.resolve(from);
        let found = self.resolve(to);
        if found == from {
            // A case-only rename of the file itself.
            let parent = from
                .rsplit_once('/')
                .map(|(p, _)| format!("{p}/"))
                .unwrap_or_default();
            let name = to.rsplit('/').next().unwrap_or(to);
            return self.inner.rename(&from, &format!("{parent}{name}"));
        }
        self.inner.rename(&from, &found)
    }
    fn is_same_file(&self, a: &str, b: &str) -> std::io::Result<bool> {
        let (a, b) = (self.resolve(a), self.resolve(b));
        Ok(a == b && self.inner.stat(&a)?.is_some())
    }
    fn move_to_bin(&self, rel: &str) -> std::io::Result<String> {
        self.inner.move_to_bin(&self.resolve(rel))
    }
    fn restore_from_bin(&self, location: &str, rel: &str) -> std::io::Result<()> {
        self.inner.restore_from_bin(location, &self.resolve(rel))
    }
    fn purge_from_bin(&self, location: &str) -> std::io::Result<()> {
        self.inner.purge_from_bin(location)
    }
}
