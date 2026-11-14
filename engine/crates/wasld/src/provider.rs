//! The daemon's side of the macOS File Provider extension (docs/ON_DEMAND.md §2.6). The system
//! runs the extension, sandboxed, whenever Finder or an app looks into a view; the extension asks
//! wasld, over a socket in the app group's container, for the items it shows and for file content.
//! [`ProviderHost`] answers from what each mounted view shows: items by their path in the view,
//! change sets between refreshes, and content read through the view's [`Hydrator`] into files the
//! extension hands to the system.
//!
//! The socket answers these requests only ([`ProviderIpc`]): a process that can reach the
//! container (this user's, or the extension) can read the views' files and nothing else.

use std::collections::HashMap;
use std::io::{Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use wasl_ipc::{
    ErrorKind, Handler, Paths, ProviderChanges, ProviderContent, ProviderItem, ProviderListing, RemoteError,
    Request, ok,
};
use wasl_vfs::{Hydrator, Node, Tree};

/// How much of a file is read through the hydrator at once while its content is written out.
const PIECE: u64 = 8 << 20;
/// Content files the system has not taken within this time are removed.
const STALE: Duration = Duration::from_secs(60 * 60);

/// What the mounted views show, for the extension.
pub struct ProviderHost {
    /// The app group's container: the socket, its token and `fetch/`.
    dir: PathBuf,
    /// Tells this run's anchors from an earlier run's.
    run: u64,
    shown: Mutex<HashMap<String, Arc<Shown>>>,
}

struct Shown {
    hydrator: Arc<Hydrator>,
    state: Mutex<Generation>,
}

struct Generation {
    n: u64,
    tree: Tree,
    /// The tree before the last change, so a change set from it can be told exactly.
    prev: Option<(u64, Tree)>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn err(kind: ErrorKind, message: impl Into<String>) -> RemoteError {
    RemoteError {
        kind,
        message: message.into(),
    }
}

impl ProviderHost {
    /// `dir` is the app group's container (or any folder only this user can reach, in tests).
    pub fn new(dir: impl Into<PathBuf>) -> ProviderHost {
        let mut b = [0u8; 8];
        let run = match getrandom::fill(&mut b) {
            Ok(()) => u64::from_le_bytes(b),
            Err(_) => SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos() as u64),
        };
        ProviderHost {
            dir: dir.into(),
            run,
            shown: Mutex::new(HashMap::new()),
        }
    }

    /// Where the extension's socket and token live.
    pub fn paths(&self) -> Paths {
        Paths::at(&self.dir)
    }

    fn fetch_dir(&self) -> PathBuf {
        self.dir.join("fetch")
    }

    /// The view shows `tree` now, reading through `hydrator`.
    pub fn show(&self, view: &str, tree: Tree, hydrator: Arc<Hydrator>) {
        let shown = Arc::new(Shown {
            hydrator,
            state: Mutex::new(Generation {
                n: 1,
                tree,
                prev: None,
            }),
        });
        lock(&self.shown).insert(view.to_owned(), shown);
    }

    /// The view shows `tree` now. Returns whether anything the extension shows changed.
    pub fn update(&self, view: &str, tree: Tree) -> bool {
        let Some(shown) = self.get(view) else {
            return false;
        };
        let mut g = lock(&shown.state);
        if g.tree.iter().eq(tree.iter()) {
            return false;
        }
        let old = std::mem::replace(&mut g.tree, tree);
        g.prev = Some((g.n, old));
        g.n += 1;
        true
    }

    /// Something about the items changed that the tree does not show (a pin): the extension
    /// reads them all again.
    pub fn touch(&self, view: &str) {
        if let Some(shown) = self.get(view) {
            let mut g = lock(&shown.state);
            g.prev = None;
            g.n += 1;
        }
    }

    /// The view shows nothing now: every request about it fails as `Locked` or `NotFound`.
    pub fn hide(&self, view: &str) {
        lock(&self.shown).remove(view);
    }

    /// Removes the view's content files the system has not taken (it takes them at once; these
    /// are left by a crash): for a vault, decrypted content (security review S3-2).
    pub fn forget_content(&self, view: &str) {
        remove_content(&self.fetch_dir(), |name| name.starts_with(&format!("{view}.")));
    }

    /// Removes every content file: at start, none is waiting to be taken.
    pub fn forget_all_content(&self) {
        remove_content(&self.fetch_dir(), |_| true);
    }

    pub fn showing(&self, view: &str) -> bool {
        lock(&self.shown).contains_key(view)
    }

    fn get(&self, view: &str) -> Option<Arc<Shown>> {
        lock(&self.shown).get(view).cloned()
    }

    fn shown(&self, view: &str) -> Result<Arc<Shown>, RemoteError> {
        self.get(view).ok_or_else(|| {
            err(
                ErrorKind::Locked,
                "Wasl isn't showing this view right now. Open Wasl to see why.",
            )
        })
    }

    fn anchor(&self, n: u64) -> String {
        format!("{:016x}.{n}", self.run)
    }

    pub fn item(&self, view: &str, id: &str) -> Result<ProviderItem, RemoteError> {
        let shown = self.shown(view)?;
        let g = lock(&shown.state);
        if id.is_empty() {
            return Ok(root_item(&g.tree));
        }
        g.tree
            .get(id)
            .map(|n| item(n, &shown.hydrator))
            .ok_or_else(|| not_found(id))
    }

    pub fn list(&self, view: &str, parent: &str) -> Result<ProviderListing, RemoteError> {
        let shown = self.shown(view)?;
        let g = lock(&shown.state);
        if !parent.is_empty() && !g.tree.get(parent).is_some_and(|n| n.is_dir) {
            return Err(not_found(parent));
        }
        Ok(ProviderListing {
            anchor: self.anchor(g.n),
            items: g
                .tree
                .children(parent)
                .map(|n| item(n, &shown.hydrator))
                .collect(),
        })
    }

    /// Every item of the view: what the system's working set holds.
    pub fn all(&self, view: &str) -> Result<ProviderListing, RemoteError> {
        let shown = self.shown(view)?;
        let g = lock(&shown.state);
        Ok(ProviderListing {
            anchor: self.anchor(g.n),
            items: g.tree.iter().map(|n| item(n, &shown.hydrator)).collect(),
        })
    }

    pub fn changes(&self, view: &str, anchor: &str) -> Result<ProviderChanges, RemoteError> {
        let shown = self.shown(view)?;
        let g = lock(&shown.state);
        let current = self.anchor(g.n);
        if anchor == current {
            return Ok(ProviderChanges {
                anchor: current,
                updated: Vec::new(),
                deleted: Vec::new(),
                expired: false,
            });
        }
        let Some((_, old)) = g.prev.as_ref().filter(|(n, _)| self.anchor(*n) == anchor) else {
            return Ok(ProviderChanges {
                anchor: current,
                updated: Vec::new(),
                deleted: Vec::new(),
                expired: true,
            });
        };
        let mut updated: Vec<ProviderItem> = g
            .tree
            .iter()
            .filter(|n| old.get(&n.path) != Some(*n))
            .map(|n| item(n, &shown.hydrator))
            .collect();
        if !old
            .iter()
            .map(|n| n.path.as_str())
            .eq(g.tree.iter().map(|n| n.path.as_str()))
        {
            // The root's children may have changed: its metadata says so.
            updated.push(root_item(&g.tree));
        }
        let deleted = old
            .iter()
            .filter(|n| g.tree.get(&n.path).is_none())
            .map(|n| n.path.clone())
            .collect();
        Ok(ProviderChanges {
            anchor: current,
            updated,
            deleted,
            expired: false,
        })
    }

    /// Writes the content of a file (or `len` bytes of it from `offset`) to a new file under
    /// `fetch/` and says where. A range is written at its own offset in that file, as the system
    /// takes it; what comes before it is a hole.
    pub async fn fetch(
        &self,
        view: &str,
        id: &str,
        offset: Option<u64>,
        len: Option<u64>,
    ) -> Result<ProviderContent, RemoteError> {
        let shown = self.shown(view)?;
        let node = lock(&shown.state)
            .tree
            .get(id)
            .filter(|n| !n.is_dir)
            .cloned()
            .ok_or_else(|| not_found(id))?;
        let file = node.file.clone().ok_or_else(|| not_found(id))?;
        let offset = offset.unwrap_or(0).min(file.size);
        let len = len.unwrap_or(file.size).min(file.size - offset);
        let dir = self.fetch_dir();
        sweep(&dir);
        let path = new_content_file(&dir, view)
            .map_err(|e| err(ErrorKind::Other, format!("{}: {e}", dir.display())))?;
        let written = async {
            let io = |e: std::io::Error| err(ErrorKind::Other, e.to_string());
            let mut out = std::fs::File::create(&path).map_err(io)?;
            out.seek(SeekFrom::Start(offset)).map_err(io)?;
            let mut at = offset;
            while at < offset + len {
                let n = PIECE.min(offset + len - at);
                let bytes = shown.hydrator.read(&file, at, n).await.map_err(vfs_err)?;
                out.write_all(&bytes).map_err(io)?;
                at += n;
            }
            out.sync_all().map_err(io)
        }
        .await;
        if let Err(e) = written {
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
        Ok(ProviderContent {
            path: path.display().to_string(),
            item: item(&node, &shown.hydrator),
            offset,
            len,
        })
    }
}

fn not_found(id: &str) -> RemoteError {
    err(ErrorKind::NotFound, format!("{id} isn't in this view on Drive"))
}

fn vfs_err(e: wasl_vfs::VfsError) -> RemoteError {
    let kind = match &e {
        wasl_vfs::VfsError::Unavailable(_) => ErrorKind::Network,
        wasl_vfs::VfsError::OutOfRange => ErrorKind::Invalid,
        _ => ErrorKind::Other,
    };
    err(kind, e.to_string())
}

fn item(n: &Node, hydrator: &Hydrator) -> ProviderItem {
    ProviderItem {
        id: n.path.clone(),
        parent: n.parent().to_owned(),
        name: n.name().to_owned(),
        is_dir: n.is_dir,
        size: n.size(),
        mtime_ms: n.mtime_ms,
        version: n.file.as_ref().map(|f| f.version.clone()).unwrap_or_default(),
        pinned: n.file.as_ref().is_some_and(|f| hydrator.cache().is_pinned(&f.id)),
    }
}

fn root_item(tree: &Tree) -> ProviderItem {
    ProviderItem {
        id: String::new(),
        parent: String::new(),
        name: String::new(),
        is_dir: true,
        size: 0,
        mtime_ms: tree.iter().map(|n| n.mtime_ms).max().unwrap_or(0),
        version: String::new(),
        pinned: false,
    }
}

/// A new, empty file under `dir` named `<view>.<random>`, readable by this user only.
fn new_content_file(dir: &Path, view: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|e| std::io::Error::other(e.to_string()))?;
    let name: String = b.iter().map(|x| format!("{x:02x}")).collect();
    let path = dir.join(format!("{view}.{name}"));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(&path)?;
    Ok(path)
}

/// Removes the content files whose names `pick` chooses.
fn remove_content(dir: &Path, pick: impl Fn(&str) -> bool) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        if e.file_name().to_str().is_some_and(&pick) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Removes content files the system never took (the extension was stopped mid-fetch).
fn sweep(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > STALE);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// The extension's socket: File Provider requests only.
pub struct ProviderIpc(pub Arc<ProviderHost>);

/// Answers a File Provider request; `None` for any other request.
pub async fn answer(
    host: &ProviderHost,
    request: &Request,
) -> Option<Result<serde_json::Value, RemoteError>> {
    Some(match request {
        Request::ProviderItem { view, id } => host.item(view, id).and_then(ok),
        Request::ProviderList {
            view,
            parent: Some(parent),
        } => host.list(view, parent).and_then(ok),
        Request::ProviderList { view, parent: None } => host.all(view).and_then(ok),
        Request::ProviderChanges { view, anchor } => host.changes(view, anchor).and_then(ok),
        Request::ProviderFetch {
            view,
            id,
            offset,
            len,
        } => host.fetch(view, id, *offset, *len).await.and_then(ok),
        _ => return None,
    })
}

#[async_trait::async_trait]
impl Handler for ProviderIpc {
    async fn handle(&self, request: Request) -> Result<serde_json::Value, RemoteError> {
        answer(&self.0, &request).await.unwrap_or_else(|| {
            Err(err(
                ErrorKind::Permission,
                "This socket answers the Wasl File Provider extension only.",
            ))
        })
    }
}
