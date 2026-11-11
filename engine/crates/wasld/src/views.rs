//! Read-only on-demand views (docs/ON_DEMAND.md §2.3): a Drive folder shown at a folder on this
//! computer. Every file appears at full size and is fetched only when something opens it; pinned
//! files stay on the computer; nothing written there is ever uploaded.
//!
//! Views are not pairs: they have their own records (`views` table) and never go through the
//! planner, the baseline or the pair safety layer. [`Views`] keeps every enabled view mounted
//! (Cloud Files on Windows, FUSE on Linux, a File Provider domain on macOS), refreshes its tree
//! from the Drive listing on a schedule, and reports what is in the folder that is not Drive's:
//! local-only files are kept and never uploaded or deleted; a file changed in the folder (Windows
//! lets the read-only attribute be cleared) gets Drive's version back, its changed copy set aside in
//! the data folder.

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wasl_engine::{Remote, view_tree};
use wasl_ipc::{ErrorKind, LocalChange, LocalChangeKind, NewView, RemoteError, View, ViewState, ViewsInfo};
use wasl_store::SharedStore;
use wasl_vfs::{BlockCache, Hydrator, Kept, KeptKind, ReadAhead, Source, Tree};

/// A view keeps at most this much unpinned content unless told otherwise (docs/ON_DEMAND.md §2.1).
pub const DEFAULT_CACHE_LIMIT: u64 = 20 << 30;
/// How often a view reads its Drive folder again.
pub const REFRESH_EVERY: Duration = Duration::from_secs(5 * 60);
/// What a view of a vault says while the vault is locked on this computer.
pub const VAULT_LOCKED: &str = "This folder is an encrypted vault, locked on this computer. Unlock the vault \
     to show its files. While it is locked, Wasl keeps none of its content here except the files you \
     chose to keep on this computer.";

/// What a view reads through: the listing it shows (for a vault: decrypted names, plaintext
/// sizes) and where file contents come from.
#[derive(Clone)]
pub struct ViewRemote {
    pub remote: Arc<dyn Remote>,
    pub source: Arc<dyn Source>,
}

/// Opens what a view reads through.
#[async_trait]
pub trait ViewRemotes: Send + Sync {
    /// A view of a vault fails with `Locked` while the vault is locked on this computer.
    fn open(&self, view: &View) -> Result<ViewRemote, RemoteError>;

    /// The vault the view's folder holds, recorded on this computer (locked until unlocked
    /// there); `None` for a plain folder.
    async fn vault_of(&self, _view: &View) -> Result<Option<String>, RemoteError> {
        Ok(None)
    }
}

/// How a mounted view ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Close {
    /// The daemon stops or the view is turned off: the folder stays set up for next time.
    Disconnect,
    /// The view's vault was locked: as `Disconnect`, but first every file that is not pinned
    /// lets go of its content, so no decrypted file stays in the folder while the vault is
    /// locked (security review 2026-10). Pinned files keep theirs: the person chose to keep them
    /// on this computer.
    Lock,
    /// The view is removed: its placeholders go, pinned files stay as ordinary files.
    Remove,
    /// The view is removed, every file fetched and kept as an ordinary file.
    KeepFiles,
}

/// Where views appear: one implementation per platform, and one in memory for tests.
#[async_trait]
pub trait Backend: Send + Sync {
    /// Whether this computer can show views at all.
    fn supported(&self) -> bool {
        true
    }

    /// Sets up the view's folder to show `tree`, reading through `hydrator`. A file changed in
    /// the folder gets Drive's version back at the next refresh, its changed copy moved under
    /// `aside` first, where the platform lets files be changed at all.
    async fn mount(
        &self,
        view: &View,
        tree: Tree,
        hydrator: Arc<Hydrator>,
        aside: &Path,
    ) -> Result<Arc<dyn Mounted>, String>;

    /// The view's folder is still set up from an earlier run (it must be mounted to be removed
    /// cleanly).
    fn leftover(&self, _view: &View) -> bool {
        false
    }

    /// The view's vault is locked and the view is not showing: if its folder is still set up
    /// from an earlier run, every file that is not pinned lets go of its content, which may be
    /// decrypted content fetched before the vault was locked (security review S3-2). A file
    /// changed in the folder goes to `aside` first. Shows nothing: `hydrator` refuses every read.
    /// Returns whether there was a folder to clear.
    async fn scrub(&self, _view: &View, _hydrator: Arc<Hydrator>, _aside: &Path) -> Result<bool, String> {
        Ok(false)
    }

    /// Where the macOS File Provider extension reads what the views show, for the daemon to
    /// serve on the extension's socket ([`crate::provider`]).
    fn provider(&self) -> Option<Arc<crate::provider::ProviderHost>> {
        None
    }
}

/// What a refresh found in the folder.
#[derive(Debug, Default, Clone)]
pub struct Refreshed {
    /// Left alone, with why: local-only files, failures, and (where files cannot be put back)
    /// local changes.
    pub kept: Vec<Kept>,
    /// Files changed here that got Drive's version back, with where the changed copy went.
    pub restored: Vec<(String, PathBuf)>,
}

/// A mounted view.
#[async_trait]
pub trait Mounted: Send + Sync {
    /// Shows `tree`; returns what was left alone in the folder (local-only files, failures) with
    /// why, and what was put back.
    async fn refresh(&self, tree: Tree) -> Result<Refreshed, String>;
    /// Keeps a file, or every file of a folder (`""`: all of them), on this computer.
    async fn pin(&self, rel: &str, tree: &Tree) -> Result<(), String>;
    async fn unpin(&self, rel: &str, tree: &Tree) -> Result<(), String>;
    async fn close(&self, how: Close, tree: &Tree) -> Result<(), String>;
}

/// What a running view knows.
struct Active {
    /// The record as it was mounted (a change of folder or account mounts it again).
    view: View,
    remote: ViewRemote,
    hydrator: Arc<Hydrator>,
    mount: Arc<dyn Mounted>,
    live: Mutex<Live>,
    refreshing: tokio::sync::Mutex<()>,
}

struct Live {
    tree: Tree,
    kept: Vec<Kept>,
    /// Files put back since the view was mounted (the newest [`RESTORED_KEPT`]), so a change
    /// undone at one refresh is still reported after the next.
    restored: Vec<(String, PathBuf)>,
    refreshed: Instant,
}

/// How many put-back files a view keeps reporting.
const RESTORED_KEPT: usize = 100;

impl Live {
    fn note_restored(&mut self, restored: Vec<(String, PathBuf)>) {
        for (path, copy) in restored {
            self.restored.retain(|(p, _)| *p != path);
            self.restored.push((path, copy));
        }
        let over = self.restored.len().saturating_sub(RESTORED_KEPT);
        self.restored.drain(..over);
    }
}

impl Active {
    fn live(&self) -> std::sync::MutexGuard<'_, Live> {
        lock(&self.live)
    }
}

/// Every view of this computer.
pub struct Views {
    store: SharedStore,
    /// `<data>/views`: each view's cache under `<id>/cache`.
    dir: PathBuf,
    remotes: Arc<dyn ViewRemotes>,
    backend: Arc<dyn Backend>,
    every: Duration,
    active: Mutex<HashMap<String, Arc<Active>>>,
    /// Views that failed to mount, and when: tried again after `every` or when asked.
    failed: Mutex<HashMap<String, Instant>>,
    /// The kind of each view's last error (the store keeps only its message).
    error_kinds: Mutex<HashMap<String, ErrorKind>>,
    /// One mount, unmount or removal at a time.
    changing: tokio::sync::Mutex<()>,
    /// Views of locked vaults whose folders were cleared in this run ([`Backend::scrub`]).
    scrubbed: Mutex<std::collections::HashSet<String>>,
    changed: Arc<dyn Fn() + Send + Sync>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn err(kind: ErrorKind, message: impl Into<String>) -> RemoteError {
    RemoteError {
        kind,
        message: message.into(),
    }
}

fn change_kind(k: KeptKind) -> LocalChangeKind {
    match k {
        KeptKind::OnlyHere => LocalChangeKind::OnlyHere,
        KeptKind::ChangedHere => LocalChangeKind::ChangedHere,
        KeptKind::Differs => LocalChangeKind::Differs,
        KeptKind::Unsupported => LocalChangeKind::Unsupported,
        KeptKind::Failed => LocalChangeKind::Failed,
    }
}

fn drive_err(e: wasl_drive::DriveError) -> RemoteError {
    crate::daemon::drive_err(e)
}

impl Views {
    pub fn new(
        store: SharedStore,
        data_dir: &Path,
        remotes: Arc<dyn ViewRemotes>,
        backend: Arc<dyn Backend>,
        every: Duration,
        changed: Arc<dyn Fn() + Send + Sync>,
    ) -> Views {
        Views {
            store,
            dir: data_dir.join("views"),
            remotes,
            backend,
            every,
            active: Mutex::new(HashMap::new()),
            failed: Mutex::new(HashMap::new()),
            error_kinds: Mutex::new(HashMap::new()),
            changing: tokio::sync::Mutex::new(()),
            scrubbed: Mutex::new(std::collections::HashSet::new()),
            changed,
        }
    }

    fn records(&self) -> Result<Vec<View>, RemoteError> {
        self.store.lock().views().map_err(crate::daemon::store_err)
    }

    fn record(&self, id: &str) -> Result<View, RemoteError> {
        self.store
            .lock()
            .view(id)
            .map_err(crate::daemon::store_err)?
            .ok_or_else(|| err(ErrorKind::NotFound, format!("No view {id}")))
    }

    fn get(&self, id: &str) -> Option<Arc<Active>> {
        lock(&self.active).get(id).cloned()
    }

    /// What the macOS File Provider extension reads, where views are File Provider domains.
    pub fn provider(&self) -> Option<Arc<crate::provider::ProviderHost>> {
        self.backend.provider()
    }

    /// Every view with its state.
    pub fn info(&self) -> Result<ViewsInfo, RemoteError> {
        let views = self.records()?;
        let active = lock(&self.active).clone();
        let kinds = lock(&self.error_kinds).clone();
        let states = views
            .into_iter()
            .map(|view| match active.get(&view.id) {
                Some(a) => {
                    let live = a.live();
                    let (files, bytes) = live
                        .tree
                        .iter()
                        .filter(|n| !n.is_dir)
                        .fold((0, 0), |(n, b), f| (n + 1, b + f.size()));
                    let (cache_used, pinned_bytes) = a.hydrator.cache().usage();
                    ViewState {
                        last_error_kind: view.last_error.as_ref().and(kinds.get(&view.id).copied()),
                        view,
                        mounted: true,
                        files,
                        bytes,
                        cache_used,
                        pinned_bytes,
                        local_changes: live
                            .kept
                            .iter()
                            .map(|k| LocalChange {
                                path: k.path.clone(),
                                reason: k.detail.clone(),
                                kind: Some(change_kind(k.kind)),
                            })
                            .chain(live.restored.iter().map(|(path, copy)| LocalChange {
                                path: path.clone(),
                                reason: copy.display().to_string(),
                                kind: Some(LocalChangeKind::Restored),
                            }))
                            .collect(),
                    }
                }
                None => ViewState {
                    last_error_kind: view.last_error.as_ref().and(kinds.get(&view.id).copied()),
                    view,
                    mounted: false,
                    files: 0,
                    bytes: 0,
                    cache_used: 0,
                    pinned_bytes: 0,
                    local_changes: Vec::new(),
                },
            })
            .collect();
        Ok(ViewsInfo {
            supported: self.backend.supported(),
            views: states,
        })
    }

    /// Records a new view and mounts it. A vault folder is linked to its vault when it mounts (and
    /// shows `Locked` until the vault is unlocked here); a mount that fails is recorded on the
    /// view and tried again later.
    pub async fn add(&self, new: NewView, pairs: &[wasl_store::Pair]) -> Result<View, RemoteError> {
        let invalid = |m: String| err(ErrorKind::Invalid, m);
        if !self.backend.supported() {
            return Err(err(ErrorKind::Unsupported, UNSUPPORTED));
        }
        let name = new.name.trim().to_owned();
        if name.is_empty() {
            return Err(invalid("A view needs a name".into()));
        }
        if new.drive_folder_id.trim().is_empty() || new.account_id.trim().is_empty() {
            return Err(invalid("Choose a Google account and a Drive folder".into()));
        }
        let local = Path::new(&new.local_path);
        if !local.is_absolute() {
            return Err(invalid(
                "Choose the folder on this computer by its full path".into(),
            ));
        }
        let local = std::path::absolute(local).map_err(|e| invalid(e.to_string()))?;
        if local.parent().is_none() {
            return Err(invalid("A view can't be a whole drive".into()));
        }
        for p in pairs {
            if overlaps(&local, Path::new(&p.local_root)) {
                return Err(invalid(format!(
                    "{} is in or around the folder of the pair {}. A view needs a folder of its own.",
                    local.display(),
                    p.name
                )));
            }
        }
        for v in self.records()? {
            if overlaps(&local, Path::new(&v.local_path)) {
                return Err(invalid(format!(
                    "{} is in or around the folder of the view {}.",
                    local.display(),
                    v.name
                )));
            }
        }
        match std::fs::read_dir(&local) {
            Ok(mut entries) => {
                if entries.next().is_some() {
                    return Err(invalid(format!(
                        "{} isn't empty. Choose a new or empty folder for the view.",
                        local.display()
                    )));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(invalid(format!("{}: {e}", local.display()))),
        }
        let view = View {
            id: new_id(),
            name,
            account_id: new.account_id,
            drive_folder_id: new.drive_folder_id,
            drive_id: new.drive_id,
            remote_path: new.remote_path,
            local_path: local.display().to_string(),
            vault_id: None,
            cache_limit: new.cache_limit.unwrap_or(DEFAULT_CACHE_LIMIT),
            enabled: true,
            created_at: now_ms(),
            last_refresh_at: None,
            last_error: None,
        };
        {
            // As for a pair: the view names its account, whose row the Drive cache needs.
            let s = self.store.lock();
            let known = s
                .accounts()
                .map_err(crate::daemon::store_err)?
                .iter()
                .any(|(id, ..)| *id == view.account_id);
            if !known {
                s.set_account_email(&view.account_id, &view.account_id)
                    .map_err(crate::daemon::store_err)?;
            }
        }
        self.store
            .lock()
            .create_view(&view)
            .map_err(crate::daemon::store_err)?;
        {
            let _g = self.changing.lock().await;
            let _ = self.start(&view).await;
        }
        (self.changed)();
        self.record(&view.id)
    }

    fn note_kind(&self, id: &str, e: Option<&RemoteError>) {
        let mut kinds = lock(&self.error_kinds);
        match e {
            Some(e) => kinds.insert(id.to_owned(), e.kind),
            None => kinds.remove(id),
        };
    }

    /// Mounts a view: lists its folder, opens its cache and sets up its folder.
    async fn start(&self, view: &View) -> Result<(), RemoteError> {
        let r = self.try_start(view).await;
        let error = r.as_ref().err().map(|e| e.message.clone());
        self.note_kind(&view.id, r.as_ref().err());
        let _ = self
            .store
            .lock()
            .set_view_refreshed(&view.id, now_ms(), error.as_deref());
        if r.is_err() {
            lock(&self.failed).insert(view.id.clone(), Instant::now());
        } else {
            lock(&self.failed).remove(&view.id);
        }
        (self.changed)();
        r
    }

    async fn try_start(&self, view: &View) -> Result<(), RemoteError> {
        if !self.backend.supported() {
            return Err(err(ErrorKind::Unsupported, UNSUPPORTED));
        }
        let mut view = view.clone();
        let mut remote = match self.remotes.open(&view) {
            Ok(r) => r,
            Err(e) => {
                if e.kind == ErrorKind::Locked {
                    self.scrub(&view).await;
                    if let Some(said) = self.set_aside_note(&view.id) {
                        return Err(err(ErrorKind::Locked, said));
                    }
                }
                return Err(e);
            }
        };
        let mut listing = remote
            .remote
            .list_subtree(&view.drive_folder_id, view.drive_id.as_deref())
            .await
            .map_err(drive_err)?;
        if view.vault_id.is_none() && is_vault(&listing.entries) {
            // The folder is a vault: shown through it, never as ciphertext. A `vault.json` that
            // is not a Wasl vault's leaves the folder plain.
            if let Some(vault_id) = self.remotes.vault_of(&view).await? {
                self.store
                    .lock()
                    .set_view_vault(&view.id, Some(&vault_id))
                    .map_err(crate::daemon::store_err)?;
                view.vault_id = Some(vault_id);
                (self.changed)();
                remote = self.remotes.open(&view)?;
                listing = remote
                    .remote
                    .list_subtree(&view.drive_folder_id, view.drive_id.as_deref())
                    .await
                    .map_err(drive_err)?;
            }
        }
        let tree = view_tree(&listing.entries);
        let view = &view;
        let cache = BlockCache::open(self.cache_dir(&view.id), view.cache_limit)
            .map_err(|e| err(ErrorKind::Other, format!("The view's cache can't be opened: {e}")))?;
        let hydrator = Arc::new(Hydrator::new(
            remote.source.clone(),
            Arc::new(cache),
            ReadAhead::default(),
        ));
        let mount = self
            .backend
            .mount(view, tree.clone(), hydrator.clone(), &self.aside_dir(&view.id))
            .await
            .map_err(|m| err(ErrorKind::Other, m))?;
        let first = match mount.refresh(tree.clone()).await {
            Ok(k) => k,
            Err(m) => {
                let _ = mount.close(Close::Disconnect, &tree).await;
                return Err(err(ErrorKind::Other, m));
            }
        };
        self.scrubbed_mut().remove(&view.id);
        let active = Arc::new(Active {
            view: view.clone(),
            remote,
            hydrator,
            mount,
            live: Mutex::new(Live {
                tree,
                kept: first.kept,
                restored: first.restored,
                refreshed: Instant::now(),
            }),
            refreshing: tokio::sync::Mutex::new(()),
        });
        lock(&self.active).insert(view.id.clone(), active);
        Ok(())
    }

    fn cache_dir(&self, id: &str) -> PathBuf {
        self.dir.join(id).join("cache")
    }

    /// Where a file changed in the view goes before Drive's version comes back.
    fn aside_dir(&self, id: &str) -> PathBuf {
        self.dir.join(id).join("changed-here")
    }

    /// What a locked vault's view says when copies set aside from it (files someone edited in
    /// it) are on this computer: plaintext the person wrote, never deleted silently.
    fn set_aside_note(&self, id: &str) -> Option<String> {
        let aside = self.aside_dir(id);
        let mut entries = std::fs::read_dir(&aside).ok()?;
        entries.next()?.ok()?;
        Some(format!(
            "{VAULT_LOCKED} Files changed in this view were set aside unencrypted in {}: \
             delete them when you no longer need them.",
            aside.display()
        ))
    }

    fn scrubbed_mut(&self) -> std::sync::MutexGuard<'_, std::collections::HashSet<String>> {
        lock(&self.scrubbed)
    }

    /// Clears what a view of a locked vault may still hold from an earlier run, once per run of
    /// the daemon: content in its folder (through the back end) and in its cache, pinned files
    /// excepted (security review S3-2). Runs before the view could show anything.
    async fn scrub(&self, view: &View) {
        if view.vault_id.is_none() || self.scrubbed_mut().contains(&view.id) {
            return;
        }
        // A cache that can't be opened still gets the folder cleared, through an empty one; but
        // its own decrypted blocks are still there, so the view is not marked done and the next
        // start tries again.
        let spare = self.dir.join(&view.id).join("scrub-cache");
        let (cache, complete) = match BlockCache::open(self.cache_dir(&view.id), view.cache_limit) {
            Ok(c) => (c, true),
            Err(_) => match BlockCache::open(&spare, 0) {
                Ok(c) => (c, false),
                Err(_) => return,
            },
        };
        cache.forget_unpinned();
        let refusing = Arc::new(Hydrator::new(
            Arc::new(Refuse),
            Arc::new(cache),
            ReadAhead::default(),
        ));
        let cleared = self
            .backend
            .scrub(view, refusing, &self.aside_dir(&view.id))
            .await
            .is_ok();
        if let Some(said) = self.set_aside_note(&view.id) {
            let _ = self
                .store
                .lock()
                .set_view_refreshed(&view.id, now_ms(), Some(&said));
        }
        if !complete {
            let _ = std::fs::remove_dir_all(&spare);
        }
        if cleared && complete {
            self.scrubbed_mut().insert(view.id.clone());
        }
    }

    /// Reads a mounted view's folder again and shows what changed.
    async fn refresh(&self, a: &Arc<Active>) -> Result<(), RemoteError> {
        let _g = a.refreshing.lock().await;
        let r = async {
            let listing = a
                .remote
                .remote
                .list_subtree(&a.view.drive_folder_id, a.view.drive_id.as_deref())
                .await
                .map_err(drive_err)?;
            if a.view.vault_id.is_none()
                && is_vault(&listing.entries)
                && let Some(vault_id) = self.remotes.vault_of(&a.view).await?
            {
                // The folder became a vault: its ciphertext is never shown. Linked now, the
                // view is mounted again through the vault at the next tick (the record no
                // longer matches what is mounted).
                self.store
                    .lock()
                    .set_view_vault(&a.view.id, Some(&vault_id))
                    .map_err(crate::daemon::store_err)?;
                return Err(err(ErrorKind::Locked, VAULT_LOCKED));
            }
            let tree = view_tree(&listing.entries);
            let found = a
                .mount
                .refresh(tree.clone())
                .await
                .map_err(|m| err(ErrorKind::Other, m))?;
            let mut live = a.live();
            live.tree = tree;
            live.kept = found.kept;
            live.note_restored(found.restored);
            Ok(())
        }
        .await;
        a.live().refreshed = Instant::now();
        let error = r.as_ref().err().map(|e: &RemoteError| e.message.clone());
        self.note_kind(&a.view.id, r.as_ref().err());
        let _ = self
            .store
            .lock()
            .set_view_refreshed(&a.view.id, now_ms(), error.as_deref());
        (self.changed)();
        r
    }

    /// Brings the mounted views in line with the records: unmounts what was removed, turned off
    /// or moved, applies cache limits, mounts what should be, refreshes what is due.
    pub async fn tick(&self) {
        let Ok(views) = self.records() else { return };
        {
            let _g = self.changing.lock().await;
            let active: Vec<(String, Arc<Active>)> = lock(&self.active)
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            for (id, a) in active {
                let keep = views
                    .iter()
                    .find(|v| v.id == id)
                    .filter(|v| v.enabled && same_place(v, &a.view));
                match keep {
                    Some(v) => {
                        if a.hydrator.cache().limit() != v.cache_limit {
                            a.hydrator.cache().set_limit(v.cache_limit);
                        }
                    }
                    None => {
                        lock(&self.active).remove(&id);
                        let tree = a.live().tree.clone();
                        let _ = a.mount.close(Close::Disconnect, &tree).await;
                        (self.changed)();
                    }
                }
            }
            // Views of a vault that are turned off are never started, so their folders are
            // checked here; one that is on is checked as it starts.
            for v in views.iter().filter(|v| !v.enabled && v.vault_id.is_some()) {
                if self.get(&v.id).is_none()
                    && !self.scrubbed_mut().contains(&v.id)
                    && matches!(self.remotes.open(v), Err(e) if e.kind == ErrorKind::Locked)
                {
                    self.scrub(v).await;
                }
            }
            for v in views.iter().filter(|v| v.enabled) {
                if self.get(&v.id).is_some() {
                    continue;
                }
                let retry = lock(&self.failed)
                    .get(&v.id)
                    .is_none_or(|t| t.elapsed() >= self.every);
                if retry {
                    let _ = self.start(v).await;
                }
            }
        }
        let due: Vec<Arc<Active>> = lock(&self.active)
            .values()
            .filter(|a| a.live().refreshed.elapsed() >= self.every)
            .cloned()
            .collect();
        for a in due {
            let _ = self.refresh(&a).await;
        }
    }

    /// Reads the view's folder now (mounting it if it is not).
    pub async fn refresh_now(&self, id: &str) -> Result<(), RemoteError> {
        let view = self.record(id)?;
        if let Some(a) = self.get(id) {
            return self.refresh(&a).await;
        }
        if !view.enabled {
            return Err(err(ErrorKind::Invalid, format!("{} is turned off", view.name)));
        }
        let _g = self.changing.lock().await;
        match self.get(id) {
            Some(_) => Ok(()),
            None => self.start(&view).await,
        }
    }

    pub async fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), RemoteError> {
        self.record(id)?;
        self.store
            .lock()
            .set_view_enabled(id, enabled)
            .map_err(crate::daemon::store_err)?;
        lock(&self.failed).remove(id);
        self.tick().await;
        (self.changed)();
        Ok(())
    }

    pub async fn set_cache_limit(&self, id: &str, limit: u64) -> Result<(), RemoteError> {
        self.record(id)?;
        self.store
            .lock()
            .set_view_cache_limit(id, limit)
            .map_err(crate::daemon::store_err)?;
        if let Some(a) = self.get(id) {
            a.hydrator.cache().set_limit(limit);
        }
        (self.changed)();
        Ok(())
    }

    /// Removes a view: its folder stops being a view (see [`Close`]), then its record and cache
    /// go. Nothing is removed if its folder could not be closed cleanly.
    pub async fn remove(&self, id: &str, keep_files: bool) -> Result<(), RemoteError> {
        let view = self.record(id)?;
        let _g = self.changing.lock().await;
        let how = if keep_files {
            Close::KeepFiles
        } else {
            Close::Remove
        };
        let active = lock(&self.active).remove(id);
        let active = match active {
            Some(a) => Some(a),
            None if self.backend.leftover(&view) || keep_files => {
                // Set up from an earlier run: mount it again to close it cleanly.
                self.try_start(&view).await.map_err(|e| {
                    err(
                        e.kind,
                        format!("{} can't be removed cleanly: {}", view.name, e.message),
                    )
                })?;
                lock(&self.active).remove(id)
            }
            None => None,
        };
        if let Some(a) = active {
            let tree = a.live().tree.clone();
            if let Err(m) = a.mount.close(how, &tree).await {
                (self.changed)();
                return Err(err(ErrorKind::Other, m));
            }
        }
        self.store
            .lock()
            .delete_view(id)
            .map_err(crate::daemon::store_err)?;
        lock(&self.failed).remove(id);
        let _ = std::fs::remove_dir_all(self.dir.join(id));
        (self.changed)();
        Ok(())
    }

    /// Pins or unpins a file (or every file of a folder) by its full path on this computer.
    pub async fn set_pinned(&self, path: &str, pinned: bool) -> Result<(), RemoteError> {
        let path = std::path::absolute(path).map_err(|e| err(ErrorKind::Invalid, format!("{path}: {e}")))?;
        let views = self.records()?;
        // On macOS the view's folder links to its File Provider domain's folder, where Finder
        // shows the files: a path there is found through the link.
        let real = std::fs::canonicalize(&path).ok();
        let (view, rel) = views
            .iter()
            .find_map(|v| {
                relative(Path::new(&v.local_path), &path)
                    .or_else(|| {
                        let root = std::fs::canonicalize(&v.local_path).ok()?;
                        relative(&root, real.as_deref()?)
                    })
                    .map(|rel| (v, rel))
            })
            .ok_or_else(|| {
                err(
                    ErrorKind::NotFound,
                    format!("{} isn't in a view. See: wasl view list", path.display()),
                )
            })?;
        let a = self.get(&view.id).ok_or_else(|| {
            err(
                ErrorKind::Invalid,
                format!(
                    "{} isn't showing its files right now{}",
                    view.name,
                    view.last_error
                        .as_deref()
                        .map(|e| format!(": {e}"))
                        .unwrap_or_default()
                ),
            )
        })?;
        let tree = a.live().tree.clone();
        if !rel.is_empty() && tree.get(&rel).is_none() {
            return Err(err(
                ErrorKind::NotFound,
                format!("{} isn't in {} on Drive", rel, view.name),
            ));
        }
        let r = if pinned {
            a.mount.pin(&rel, &tree).await
        } else {
            a.mount.unpin(&rel, &tree).await
        };
        (self.changed)();
        r.map_err(|m| err(ErrorKind::Other, m))
    }

    /// The vault was locked here (its reads are already cut off, see `Vaults::lock`): its views
    /// stop showing files until it is unlocked again, and their caches let go of everything but
    /// pinned files. Their folders stay set up.
    pub async fn vault_locked(&self, vault_id: &str) {
        let _g = self.changing.lock().await;
        let gone: Vec<Arc<Active>> = {
            let mut active = lock(&self.active);
            let ids: Vec<String> = active
                .iter()
                .filter(|(_, a)| a.view.vault_id.as_deref() == Some(vault_id))
                .map(|(id, _)| id.clone())
                .collect();
            ids.iter().filter_map(|id| active.remove(id)).collect()
        };
        let mut closed = std::collections::HashSet::new();
        for a in gone {
            let tree = a.live().tree.clone();
            let _ = a.mount.close(Close::Lock, &tree).await;
            a.hydrator.cache().forget_unpinned();
            closed.insert(a.view.id.clone());
            let locked = err(ErrorKind::Locked, VAULT_LOCKED);
            self.note_kind(&a.view.id, Some(&locked));
            let said = self
                .set_aside_note(&a.view.id)
                .unwrap_or_else(|| VAULT_LOCKED.to_owned());
            let _ = self
                .store
                .lock()
                .set_view_refreshed(&a.view.id, now_ms(), Some(&said));
            lock(&self.failed).insert(a.view.id.clone(), Instant::now());
        }
        // Views of the vault that were not showing may still have content from an earlier run,
        // in their cache and in their folder.
        for id in &closed {
            self.scrubbed_mut().insert(id.clone());
        }
        for v in self.records().unwrap_or_default() {
            if v.vault_id.as_deref() == Some(vault_id) && !closed.contains(&v.id) {
                self.scrubbed_mut().remove(&v.id);
                self.scrub(&v).await;
            }
        }
        (self.changed)();
    }

    /// The vault was unlocked here: its views show their files now.
    pub async fn vault_unlocked(&self, vault_id: &str) {
        if let Ok(views) = self.records() {
            let mut failed = lock(&self.failed);
            for v in views.iter().filter(|v| v.vault_id.as_deref() == Some(vault_id)) {
                failed.remove(&v.id);
            }
        }
        self.tick().await;
    }

    /// Disconnects every view (the daemon is stopping); their folders stay set up.
    pub async fn close_all(&self) {
        let _g = self.changing.lock().await;
        let all: Vec<Arc<Active>> = lock(&self.active).drain().map(|(_, a)| a).collect();
        for a in all {
            let tree = a.live().tree.clone();
            let _ = a.mount.close(Close::Disconnect, &tree).await;
        }
    }
}

const UNSUPPORTED: &str = "Files on demand aren't available on this system yet.";

/// The source of a view being cleared while its vault is locked: nothing is read.
struct Refuse;

#[async_trait]
impl Source for Refuse {
    async fn read(&self, _: &wasl_vfs::FileRef, _: u64, _: u64) -> wasl_vfs::Result<Vec<u8>> {
        Err(wasl_vfs::VfsError::Unavailable(VAULT_LOCKED.into()))
    }
}

/// Runs the views until the daemon stops: everything at start, then every `tick`.
pub async fn run(views: Arc<Views>, tick: Duration) {
    loop {
        views.tick().await;
        tokio::time::sleep(tick).await;
    }
}

/// A vault folder (v1 or v2) holds its `vault.json` at the top.
fn is_vault(entries: &[wasl_core::RemoteEntry]) -> bool {
    entries.iter().any(|e| !e.is_dir && e.key == "vault.json")
}

/// The same folder, account and Drive folder: a mounted view needs no new mount.
fn same_place(a: &View, b: &View) -> bool {
    a.local_path == b.local_path
        && a.account_id == b.account_id
        && a.drive_folder_id == b.drive_folder_id
        && a.drive_id == b.drive_id
        && a.vault_id == b.vault_id
}

/// The parts of a path, compared as the OS compares names (case-insensitively on Windows).
fn parts(p: &Path) -> Vec<String> {
    p.components()
        .filter(|c| !matches!(c, Component::CurDir))
        .map(|c| {
            let s = c.as_os_str().to_string_lossy().into_owned();
            if cfg!(windows) { s.to_lowercase() } else { s }
        })
        .collect()
}

/// `path` relative to `root` (`/`-separated, `""` for the root itself), if it is inside it.
pub fn relative(root: &Path, path: &Path) -> Option<String> {
    let (r, p) = (parts(root), parts(path));
    if p.len() < r.len() || p[..r.len()] != r[..] {
        return None;
    }
    let rest: Vec<String> = path
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .skip(r.len())
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(rest.join("/"))
}

/// One folder is the other or inside it.
fn overlaps(a: &Path, b: &Path) -> bool {
    relative(a, b).is_some() || relative(b, a).is_some()
}

pub(crate) fn new_id() -> String {
    let mut b = [0u8; 16];
    if getrandom::fill(&mut b).is_err() {
        // No randomness from the OS: the time is unique enough for one computer's views.
        b[..16].copy_from_slice(&(now_ms() as u128).to_le_bytes());
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// The files under `rel` in `tree` (`""`: all of them; a file: itself).
fn files_under<'a>(tree: &'a Tree, rel: &'a str) -> impl Iterator<Item = &'a wasl_vfs::Node> + 'a {
    tree.iter().filter(move |n| {
        !n.is_dir
            && (rel.is_empty()
                || n.path == rel
                || (n.path.len() > rel.len()
                    && n.path.starts_with(rel)
                    && n.path.as_bytes()[rel.len()] == b'/'))
    })
}

// ---------------------------------------------------------------- back ends

/// The back end of this computer: Cloud Files on Windows, FUSE on Linux, the File Provider
/// extension on macOS (when wasld runs from Wasl.app, which holds it), none elsewhere.
pub fn platform_backend() -> Arc<dyn Backend> {
    #[cfg(windows)]
    {
        Arc::new(windows::CloudFiles)
    }
    #[cfg(target_os = "linux")]
    {
        Arc::new(linux::Fuse)
    }
    #[cfg(target_os = "macos")]
    {
        match macos::FileProvider::find() {
            Some(b) => Arc::new(b),
            None => Arc::new(Unsupported),
        }
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        Arc::new(Unsupported)
    }
}

/// No files on demand here (another system, or macOS without Wasl.app's File Provider
/// extension).
pub struct Unsupported;

#[async_trait]
impl Backend for Unsupported {
    fn supported(&self) -> bool {
        false
    }

    async fn mount(&self, _: &View, _: Tree, _: Arc<Hydrator>, _: &Path) -> Result<Arc<dyn Mounted>, String> {
        Err(UNSUPPORTED.into())
    }
}

/// A view kept in memory: the tree as shown, pins through the hydrator, and every close. For
/// tests, and for seeing what the daemon does without touching a folder.
#[derive(Default)]
pub struct MemoryBackend {
    /// Ids of the views mounted, in order.
    pub mounts: Arc<Mutex<Vec<String>>>,
    /// Every close: (view id, how).
    pub closes: Arc<Mutex<Vec<(String, Close)>>>,
    /// What every refresh reports as left alone in the folder.
    pub kept: Arc<Mutex<Vec<Kept>>>,
    /// What the next refresh reports as put back (taken by it).
    pub restored: Arc<Mutex<Vec<(String, PathBuf)>>>,
    /// Every [`Backend::scrub`]: (view id, whether anything was mounted for it then).
    pub scrubs: Arc<Mutex<Vec<(String, bool)>>>,
    /// The paths each mounted view shows now, by view id.
    pub shown: Arc<Mutex<HashMap<String, Vec<String>>>>,
    /// What each mounted view shows and reads through, by view id: open its files as an app
    /// would.
    pub reads: Reads,
}

/// What each mounted view shows and reads through, by view id.
pub type Reads = Arc<Mutex<HashMap<String, (Tree, Arc<Hydrator>)>>>;

struct MemoryMount {
    id: String,
    hydrator: Arc<Hydrator>,
    closes: Arc<Mutex<Vec<(String, Close)>>>,
    kept: Arc<Mutex<Vec<Kept>>>,
    restored: Arc<Mutex<Vec<(String, PathBuf)>>>,
    shown: Arc<Mutex<HashMap<String, Vec<String>>>>,
    reads: Reads,
}

#[async_trait]
impl Backend for MemoryBackend {
    async fn mount(
        &self,
        view: &View,
        tree: Tree,
        hydrator: Arc<Hydrator>,
        _aside: &Path,
    ) -> Result<Arc<dyn Mounted>, String> {
        lock(&self.mounts).push(view.id.clone());
        lock(&self.shown).insert(view.id.clone(), tree.iter().map(|n| n.path.clone()).collect());
        lock(&self.reads).insert(view.id.clone(), (tree, hydrator.clone()));
        Ok(Arc::new(MemoryMount {
            id: view.id.clone(),
            hydrator,
            closes: self.closes.clone(),
            kept: self.kept.clone(),
            restored: self.restored.clone(),
            shown: self.shown.clone(),
            reads: self.reads.clone(),
        }))
    }

    async fn scrub(&self, view: &View, _hydrator: Arc<Hydrator>, _aside: &Path) -> Result<bool, String> {
        let showing = lock(&self.shown).contains_key(&view.id);
        lock(&self.scrubs).push((view.id.clone(), showing));
        Ok(true)
    }
}

#[async_trait]
impl Mounted for MemoryMount {
    async fn refresh(&self, tree: Tree) -> Result<Refreshed, String> {
        lock(&self.shown).insert(self.id.clone(), tree.iter().map(|n| n.path.clone()).collect());
        lock(&self.reads).insert(self.id.clone(), (tree, self.hydrator.clone()));
        Ok(Refreshed {
            kept: lock(&self.kept).clone(),
            restored: std::mem::take(&mut *lock(&self.restored)),
        })
    }

    async fn pin(&self, rel: &str, tree: &Tree) -> Result<(), String> {
        for n in files_under(tree, rel) {
            if let Some(f) = &n.file {
                self.hydrator.pin(f).await.map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    async fn unpin(&self, rel: &str, tree: &Tree) -> Result<(), String> {
        for n in files_under(tree, rel) {
            if let Some(f) = &n.file {
                self.hydrator.unpin(f).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    async fn close(&self, how: Close, _tree: &Tree) -> Result<(), String> {
        lock(&self.closes).push((self.id.clone(), how));
        lock(&self.shown).remove(&self.id);
        lock(&self.reads).remove(&self.id);
        Ok(())
    }
}

/// Windows: the view's folder is a Cloud Files sync root of read-only placeholders.
#[cfg(windows)]
mod windows {
    use super::*;
    use wasl_vfs_windows::{OnDemandRoot, RootConfig, registered_sync_roots};

    pub struct CloudFiles;

    /// The sync root's own id for a view (the part after `Wasl!<SID>!`).
    fn root_id(view: &View) -> String {
        format!("view-{}", view.id)
    }

    fn config(view: &View, aside: Option<&Path>) -> RootConfig {
        RootConfig {
            root: PathBuf::from(&view.local_path),
            pair_id: root_id(view),
            display_name: view.name.clone(),
            version: env!("CARGO_PKG_VERSION").into(),
            icon: None,
            // A scan of the folder by Wasl itself must never download everything.
            block_self_hydration: true,
            read_only: true,
            // Read-only by attribute only: a file changed anyway gets Drive's version back
            // (security review S3-2).
            set_aside: aside.map(Path::to_path_buf),
        }
    }

    #[async_trait]
    impl Backend for CloudFiles {
        async fn mount(
            &self,
            view: &View,
            _tree: Tree,
            hydrator: Arc<Hydrator>,
            aside: &Path,
        ) -> Result<Arc<dyn Mounted>, String> {
            let config = config(view, Some(aside));
            // Fetches run on the daemon's runtime, where the Drive client lives.
            let rt = tokio::runtime::Handle::current();
            let root = tokio::task::spawn_blocking(move || OnDemandRoot::register_on(config, hydrator, rt))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
            Ok(Arc::new(Root {
                root: Mutex::new(Some(Arc::new(root))),
            }))
        }

        fn leftover(&self, view: &View) -> bool {
            let tail = format!("!{}", root_id(view));
            registered_sync_roots().is_ok_and(|roots| roots.iter().any(|(id, _)| id.ends_with(&tail)))
        }

        async fn scrub(&self, view: &View, hydrator: Arc<Hydrator>, aside: &Path) -> Result<bool, String> {
            if !self.leftover(view) || !Path::new(&view.local_path).is_dir() {
                return Ok(false);
            }
            // Connected while the content goes, so the placeholders stay Wasl's (as when a vault
            // is locked while its view shows); every read is refused meanwhile, and nothing has
            // been shown yet in this run. Files changed here go aside first.
            let config = config(view, Some(aside));
            let rt = tokio::runtime::Handle::current();
            tokio::task::spawn_blocking(move || {
                let root = OnDemandRoot::register_on(config, hydrator, rt)?;
                root.dehydrate_unpinned()
            })
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
            Ok(true)
        }
    }

    struct Root {
        root: Mutex<Option<Arc<OnDemandRoot>>>,
    }

    impl Root {
        /// Runs a blocking call on the root off the async threads.
        async fn blocking<T: Send + 'static>(
            &self,
            f: impl FnOnce(&OnDemandRoot) -> wasl_vfs_windows::Result<T> + Send + 'static,
        ) -> Result<T, String> {
            let root = lock(&self.root)
                .clone()
                .ok_or_else(|| "The view is closed.".to_owned())?;
            tokio::task::spawn_blocking(move || f(&root))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())
        }
    }

    /// What `pin` and `unpin` act on: a path, or every top-level item for the whole view.
    fn targets(rel: &str, tree: &Tree) -> Vec<String> {
        if rel.is_empty() {
            tree.children("").map(|n| n.path.clone()).collect()
        } else {
            vec![rel.to_owned()]
        }
    }

    /// Ends a root for good (see [`Close`]); the folder goes too when nothing is left in it.
    fn close_root(root: OnDemandRoot, how: Close, tree: &Tree) -> Result<(), String> {
        let folder = root.root().to_path_buf();
        let r = if how == Close::KeepFiles {
            root.hydrate_all_and_unregister()
        } else {
            // Pinned files stay, as ordinary files; every other placeholder of Wasl's goes.
            // Local changes and files only here are left alone by sync.
            let pinned: Vec<wasl_vfs::Node> = tree
                .iter()
                .filter(|n| !n.is_dir)
                .filter(|n| root.state(&n.path).ok().flatten().is_some_and(|s| s.pinned))
                .cloned()
                .collect();
            let keep = !pinned.is_empty();
            match root.sync(&Tree::new(pinned)) {
                Err(e) => Err(e),
                Ok(_) if keep => root.hydrate_all_and_unregister(),
                Ok(_) => root.unregister(),
            }
        };
        if r.is_ok() {
            let _ = std::fs::remove_dir(&folder);
        }
        r.map_err(|e| e.to_string())
    }

    #[async_trait]
    impl Mounted for Root {
        async fn refresh(&self, tree: Tree) -> Result<Refreshed, String> {
            self.blocking(move |r| r.sync(&tree))
                .await
                .map(|report| Refreshed {
                    kept: report.kept,
                    restored: report.restored,
                })
        }

        async fn pin(&self, rel: &str, tree: &Tree) -> Result<(), String> {
            let paths = targets(rel, tree);
            self.blocking(move |r| paths.iter().try_for_each(|p| r.pin(p)))
                .await
        }

        async fn unpin(&self, rel: &str, tree: &Tree) -> Result<(), String> {
            let paths = targets(rel, tree);
            self.blocking(move |r| paths.iter().try_for_each(|p| r.unpin(p)))
                .await
        }

        async fn close(&self, how: Close, tree: &Tree) -> Result<(), String> {
            let Some(root) = lock(&self.root).take() else {
                return Ok(());
            };
            if how == Close::Lock {
                // Still connected while the content goes, so the placeholders stay Wasl's. The
                // vault's reads are already cut off, so nothing can fetch it back meanwhile.
                let shared = root.clone();
                let freed = tokio::task::spawn_blocking(move || shared.dehydrate_unpinned())
                    .await
                    .map_err(|e| e.to_string())?;
                drop(root);
                return freed.map_err(|e| e.to_string());
            }
            if how == Close::Disconnect {
                // Dropping disconnects; the registration stays for next time.
                drop(root);
                return Ok(());
            }
            let root = match Arc::try_unwrap(root) {
                Ok(r) => r,
                Err(shared) => {
                    *lock(&self.root) = Some(shared);
                    return Err("The view is busy; try again in a moment.".into());
                }
            };
            let tree = tree.clone();
            tokio::task::spawn_blocking(move || close_root(root, how, &tree))
                .await
                .map_err(|e| e.to_string())?
        }
    }
}

/// Linux: the view's folder is a read-only FUSE mount while the daemon runs.
#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use wasl_vfs_fuse::{MountOptions, OnDemandMount};

    pub struct Fuse;

    #[async_trait]
    impl Backend for Fuse {
        async fn mount(
            &self,
            view: &View,
            tree: Tree,
            hydrator: Arc<Hydrator>,
            _aside: &Path,
        ) -> Result<Arc<dyn Mounted>, String> {
            let at = PathBuf::from(&view.local_path);
            let options = MountOptions {
                name: format!("wasl-{}", view.id),
                ..MountOptions::default()
            };
            let rt = tokio::runtime::Handle::current();
            let mount = tokio::task::spawn_blocking(move || {
                std::fs::create_dir_all(&at)?;
                OnDemandMount::mount(&at, tree, hydrator, rt, &options)
            })
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("{} can't be mounted: {e}", view.local_path))?;
            Ok(Arc::new(Mount {
                mount: Mutex::new(Some(Arc::new(mount))),
            }))
        }
    }

    struct Mount {
        mount: Mutex<Option<Arc<OnDemandMount>>>,
    }

    impl Mount {
        fn get(&self) -> Result<Arc<OnDemandMount>, String> {
            lock(&self.mount)
                .clone()
                .ok_or_else(|| "The view is closed.".to_owned())
        }
    }

    #[async_trait]
    impl Mounted for Mount {
        async fn refresh(&self, tree: Tree) -> Result<Refreshed, String> {
            // The mount is read-only: nothing can change in it here, and nothing stays in the
            // folder once it is unmounted (its content lives in the cache).
            self.get()?.refresh(tree);
            Ok(Refreshed::default())
        }

        async fn pin(&self, rel: &str, _tree: &Tree) -> Result<(), String> {
            self.get()?.pin(rel).await.map_err(|e| e.to_string())
        }

        async fn unpin(&self, rel: &str, _tree: &Tree) -> Result<(), String> {
            self.get()?.unpin(rel).map_err(|e| e.to_string())
        }

        async fn close(&self, how: Close, _tree: &Tree) -> Result<(), String> {
            if how == Close::KeepFiles {
                return Err("Keeping a view's files isn't available on Linux yet. Copy them out of the folder, then remove the view.".into());
            }
            let Some(mount) = lock(&self.mount).take() else {
                return Ok(());
            };
            tokio::task::spawn_blocking(move || match Arc::try_unwrap(mount) {
                Ok(m) => m.unmount().map_err(|e| e.to_string()),
                // A read in flight still holds it: dropped with it, it unmounts then.
                Err(shared) => {
                    drop(shared);
                    Ok(())
                }
            })
            .await
            .map_err(|e| e.to_string())?
        }
    }
}

/// macOS: each view is a File Provider domain of Wasl.app's extension (docs/ON_DEMAND.md §2.6).
/// The system keeps the domain's folder (under `~/Library/CloudStorage`, in Finder's sidebar
/// under Locations) and asks the extension for items and content; the extension asks wasld
/// ([`crate::provider`]). Domains are added, removed and told about changes by
/// `wasl-fileprovider`, a small helper next to wasld in the app, since only code inside the app
/// that holds the extension may manage its domains. The view's own folder becomes a link to the
/// domain's folder.
#[cfg(target_os = "macos")]
pub mod macos {
    use super::*;
    use crate::provider::ProviderHost;
    use std::process::{Command, Stdio};

    /// The helper's name, next to wasld in `Wasl.app/Contents/MacOS`.
    pub const CTL: &str = "wasl-fileprovider";

    pub struct FileProvider {
        ctl: PathBuf,
        host: Arc<ProviderHost>,
    }

    impl FileProvider {
        /// The extension's back end, when wasld runs from Wasl.app (or `WASL_FILEPROVIDER_CTL`
        /// names the helper): `None` when there is no helper or it can't find the app group's
        /// container (Wasl.app not signed for it).
        pub fn find() -> Option<FileProvider> {
            let ctl = match std::env::var_os("WASL_FILEPROVIDER_CTL") {
                Some(p) => PathBuf::from(p),
                None => std::env::current_exe().ok()?.parent()?.join(CTL),
            };
            Self::with_helper(ctl)
        }

        /// The back end driven by the helper at `ctl`.
        pub fn with_helper(ctl: PathBuf) -> Option<FileProvider> {
            if !ctl.is_file() {
                return None;
            }
            let dir = run(&ctl, &["container"], None).ok()?;
            let dir = PathBuf::from(dir.trim());
            dir.is_absolute().then(|| FileProvider {
                ctl,
                host: Arc::new(ProviderHost::new(dir)),
            })
        }
    }

    /// Runs the helper off the async threads: its standard output, or why it failed.
    async fn call(ctl: &Path, args: &[&str], input: Option<String>) -> Result<String, String> {
        let ctl = ctl.to_path_buf();
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        tokio::task::spawn_blocking(move || {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            run(&ctl, &args, input)
        })
        .await
        .map_err(|e| e.to_string())?
    }

    fn run(ctl: &Path, args: &[&str], input: Option<String>) -> Result<String, String> {
        use std::io::Write as _;
        let mut child = Command::new(ctl)
            .args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("{}: {e}", ctl.display()))?;
        if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
            stdin.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(format!(
                "The File Provider extension refused: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }

    /// Makes the view's folder a link to the domain's folder (`root`). A folder there must be
    /// empty: it is replaced by the link.
    fn link(local: &Path, root: &Path) -> Result<(), String> {
        let say = |e: std::io::Error| format!("{}: {e}", local.display());
        match std::fs::symlink_metadata(local) {
            Ok(m) if m.file_type().is_symlink() => {
                if std::fs::read_link(local).is_ok_and(|to| to == root) {
                    return Ok(());
                }
                std::fs::remove_file(local).map_err(say)?;
            }
            Ok(m) if m.is_dir() => std::fs::remove_dir(local).map_err(|e| {
                format!(
                    "{} isn't empty, so it can't show the view ({e}). Move its files elsewhere.",
                    local.display()
                )
            })?,
            Ok(_) => return Err(format!("{} isn't a folder", local.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(parent) = local.parent() {
                    std::fs::create_dir_all(parent).map_err(say)?;
                }
            }
            Err(e) => return Err(say(e)),
        }
        std::os::unix::fs::symlink(root, local).map_err(say)
    }

    #[async_trait]
    impl Backend for FileProvider {
        async fn mount(
            &self,
            view: &View,
            tree: Tree,
            hydrator: Arc<Hydrator>,
            _aside: &Path,
        ) -> Result<Arc<dyn Mounted>, String> {
            // Shown before the domain exists, so the extension's first questions have answers.
            self.host.show(&view.id, tree, hydrator.clone());
            let added = call(&self.ctl, &["add", "--id", &view.id, "--name", &view.name], None)
                .await
                .and_then(|root| link(Path::new(&view.local_path), Path::new(root.trim())));
            if let Err(e) = added {
                self.host.hide(&view.id);
                return Err(e);
            }
            // The domain may be from an earlier run: whatever the system holds is read again.
            let _ = call(&self.ctl, &["signal", "--id", &view.id], None).await;
            Ok(Arc::new(Domain {
                id: view.id.clone(),
                local: PathBuf::from(&view.local_path),
                ctl: self.ctl.clone(),
                host: self.host.clone(),
                hydrator,
            }))
        }

        fn leftover(&self, view: &View) -> bool {
            run(&self.ctl, &["list"], None).is_ok_and(|ids| ids.lines().any(|l| l.trim() == view.id))
        }

        async fn scrub(&self, view: &View, _hydrator: Arc<Hydrator>, _aside: &Path) -> Result<bool, String> {
            if !self.leftover(view) {
                return Ok(false);
            }
            // What the domain's folder holds from an earlier run is not known here (nothing is
            // shown yet), so the domain goes with all of it; pinned files stay in the view's
            // cache and come back when the vault is unlocked and the view shows again.
            self.host.forget_content(&view.id);
            call(&self.ctl, &["remove", "--id", &view.id], None).await?;
            Ok(true)
        }

        fn provider(&self) -> Option<Arc<ProviderHost>> {
            Some(self.host.clone())
        }
    }

    struct Domain {
        id: String,
        local: PathBuf,
        ctl: PathBuf,
        host: Arc<ProviderHost>,
        hydrator: Arc<Hydrator>,
    }

    impl Domain {
        /// The system reads the view's items again.
        async fn signal(&self) -> Result<(), String> {
            call(&self.ctl, &["signal", "--id", &self.id], None)
                .await
                .map(drop)
        }
    }

    #[async_trait]
    impl Mounted for Domain {
        async fn refresh(&self, tree: Tree) -> Result<Refreshed, String> {
            // The domain is read-only (its items do not allow writing), so nothing in it can
            // differ from Drive.
            if self.host.update(&self.id, tree) {
                self.signal().await?;
            }
            Ok(Refreshed::default())
        }

        async fn pin(&self, rel: &str, tree: &Tree) -> Result<(), String> {
            for n in files_under(tree, rel) {
                if let Some(f) = &n.file {
                    self.hydrator.pin(f).await.map_err(|e| e.to_string())?;
                }
            }
            // Pinned items ask the system to download them and keep them downloaded; their
            // content is in the cache by now.
            self.host.touch(&self.id);
            self.signal().await
        }

        async fn unpin(&self, rel: &str, tree: &Tree) -> Result<(), String> {
            for n in files_under(tree, rel) {
                if let Some(f) = &n.file {
                    self.hydrator.unpin(f).map_err(|e| e.to_string())?;
                }
            }
            self.host.touch(&self.id);
            self.signal().await
        }

        async fn close(&self, how: Close, tree: &Tree) -> Result<(), String> {
            match how {
                Close::Disconnect => {
                    // The domain stays for next time; meanwhile the extension says Wasl isn't
                    // showing it.
                    self.host.hide(&self.id);
                    Ok(())
                }
                Close::Lock => {
                    self.host.hide(&self.id);
                    self.host.forget_content(&self.id);
                    let unpinned: String = tree
                        .iter()
                        .filter(|n| {
                            n.file
                                .as_ref()
                                .is_some_and(|f| !self.hydrator.cache().is_pinned(&f.id))
                        })
                        .map(|n| format!("{}\n", n.path))
                        .collect();
                    call(&self.ctl, &["evict", "--id", &self.id], Some(unpinned))
                        .await
                        .map(drop)
                }
                Close::Remove => {
                    self.host.hide(&self.id);
                    self.host.forget_content(&self.id);
                    call(&self.ctl, &["remove", "--id", &self.id], None).await?;
                    if std::fs::symlink_metadata(&self.local).is_ok_and(|m| m.file_type().is_symlink()) {
                        let _ = std::fs::remove_file(&self.local);
                    }
                    Ok(())
                }
                Close::KeepFiles => Err(
                    "Keeping a view's files isn't available on macOS yet. Copy them out of the \
                     folder, then remove the view."
                        .into(),
                ),
            }
        }
    }
}
