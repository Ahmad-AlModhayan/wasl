//! What an on-demand mount shows, apart from FUSE so it is tested on every OS: inode numbers for a
//! [`Tree`]'s paths, attributes, directory listings, open files bound to the version they opened,
//! the pin attribute, and refreshes that swap the tree in one step.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use wasl_vfs::{FileRef, Hydrator, Node, Tree, VfsError};

/// The root folder's inode number (FUSE's `FUSE_ROOT_ID`).
pub const ROOT: u64 = 1;

/// The extended attribute that says whether a file is pinned: `"1"` or `"0"`.
pub const PINNED_XATTR: &str = "user.wasl.pinned";

/// Permissions of every file: the content cannot be written through the mount yet, but a copy
/// out keeps the usual mode.
const FILE_PERM: u16 = 0o644;
const DIR_PERM: u16 = 0o755;

/// Why an operation failed; each maps to one errno.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FsError {
    #[error("no such file or folder")]
    NotFound,
    #[error("not a folder")]
    NotDir,
    #[error("is a folder")]
    IsDir,
    /// Writes are not served yet: local edits on on-demand pairs come in a later step.
    #[error("the on-demand folder is read-only")]
    ReadOnly,
    #[error("no such attribute")]
    NoAttr,
    #[error("the buffer is too small")]
    Range,
    #[error("the file is not open")]
    BadHandle,
    /// The content could not be fetched or did not check out.
    #[error("the content could not be read")]
    Io,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Dir,
    File,
}

/// A file's or folder's attributes, as `stat` shows them (the owner is added by the mount).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attr {
    pub ino: u64,
    pub kind: Kind,
    pub size: u64,
    /// 512-byte blocks: the full size, cached or not, so tools that skip "sparse" files still
    /// read on-demand ones.
    pub blocks: u64,
    pub mtime: SystemTime,
    pub perm: u16,
    pub nlink: u32,
}

/// One entry of a directory listing. `offset` is where the listing continues after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub ino: u64,
    pub kind: Kind,
    pub name: String,
    pub offset: u64,
}

/// What the kernel must forget after a refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The inode's attributes or content changed (or it is gone).
    Inode(u64),
    /// A name in a folder appeared or disappeared.
    Entry { parent: u64, name: String },
}

/// The answer to an extended-attribute request of a given buffer size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XattrReply {
    /// The caller asked for the size only (a zero-sized buffer).
    Size(u32),
    Data(Vec<u8>),
}

impl XattrReply {
    /// Size-only when `size` is 0, the data when it fits, [`FsError::Range`] when it does not.
    pub fn for_buffer(size: u32, data: Vec<u8>) -> Result<Self, FsError> {
        let len = u32::try_from(data.len()).map_err(|_| FsError::Range)?;
        if size == 0 {
            Ok(Self::Size(len))
        } else if len > size {
            Err(FsError::Range)
        } else {
            Ok(Self::Data(data))
        }
    }
}

/// One tree with its inode numbers. Immutable: a refresh builds the next one.
struct Layout {
    tree: Tree,
    by_path: HashMap<String, u64>,
    by_ino: HashMap<u64, String>,
    /// Each folder's children, in name order.
    children: HashMap<u64, Vec<u64>>,
    root_mtime: SystemTime,
    /// The next unused inode number. Numbers are never reused, so a stale kernel inode can never
    /// name a different file.
    next: u64,
}

impl Layout {
    fn build(tree: Tree, prev: Option<&Layout>) -> Self {
        let mut next = prev.map_or(ROOT + 1, |p| p.next);
        let mut by_path = HashMap::with_capacity(tree.len() + 1);
        by_path.insert(String::new(), ROOT);
        for n in tree.iter() {
            // Same path and same kind: same inode. A file that became a folder (or back) is new.
            let kept = prev.and_then(|p| {
                let ino = *p.by_path.get(&n.path)?;
                (p.tree.get(&n.path)?.is_dir == n.is_dir).then_some(ino)
            });
            let ino = kept.unwrap_or_else(|| {
                next += 1;
                next - 1
            });
            by_path.insert(n.path.clone(), ino);
        }
        let by_ino: HashMap<u64, String> = by_path.iter().map(|(p, i)| (*i, p.clone())).collect();
        let mut children: HashMap<u64, Vec<u64>> = HashMap::new();
        children.insert(ROOT, Vec::new());
        // The tree iterates in path order, so siblings arrive in name order.
        for n in tree.iter() {
            let ino = by_path[&n.path];
            if n.is_dir {
                children.entry(ino).or_default();
            }
            // Tree::new adds every missing parent folder, so the parent is always there.
            if let Some(parent) = by_path.get(n.parent()) {
                children.entry(*parent).or_default().push(ino);
            }
        }
        let root_mtime = tree
            .iter()
            .map(|n| n.mtime_ms)
            .max()
            .map_or(UNIX_EPOCH, system_time);
        Self {
            tree,
            by_path,
            by_ino,
            children,
            root_mtime,
            next,
        }
    }

    fn path(&self, ino: u64) -> Result<&str, FsError> {
        self.by_ino.get(&ino).map(String::as_str).ok_or(FsError::NotFound)
    }

    /// The node at an inode; `None` for the root.
    fn node(&self, ino: u64) -> Result<Option<&Node>, FsError> {
        let path = self.path(ino)?;
        if path.is_empty() {
            return Ok(None);
        }
        self.tree.get(path).map(Some).ok_or(FsError::NotFound)
    }

    fn attr(&self, ino: u64) -> Result<Attr, FsError> {
        Ok(match self.node(ino)? {
            None => dir_attr(ino, self.root_mtime),
            Some(n) => node_attr(ino, n),
        })
    }

    fn parent_of(&self, ino: u64) -> u64 {
        let Ok(path) = self.path(ino) else {
            return ROOT;
        };
        let parent = path.rsplit_once('/').map_or("", |(p, _)| p);
        self.by_path.get(parent).copied().unwrap_or(ROOT)
    }

    fn kind(&self, ino: u64) -> Kind {
        match self.node(ino) {
            Ok(Some(n)) if !n.is_dir => Kind::File,
            _ => Kind::Dir,
        }
    }

    /// Every file at or under `path` (`""` is the whole folder).
    fn files_under(&self, path: &str) -> Result<Vec<FileRef>, FsError> {
        if !path.is_empty() && self.tree.get(path).is_none() {
            return Err(FsError::NotFound);
        }
        let prefix = format!("{path}/");
        Ok(self
            .tree
            .iter()
            .filter(|n| path.is_empty() || n.path == path || n.path.starts_with(&prefix))
            .filter_map(|n| n.file.clone())
            .collect())
    }
}

fn system_time(ms: i64) -> SystemTime {
    let d = Duration::from_millis(ms.unsigned_abs());
    if ms >= 0 {
        UNIX_EPOCH.checked_add(d)
    } else {
        UNIX_EPOCH.checked_sub(d)
    }
    .unwrap_or(UNIX_EPOCH)
}

fn dir_attr(ino: u64, mtime: SystemTime) -> Attr {
    Attr {
        ino,
        kind: Kind::Dir,
        size: 0,
        blocks: 0,
        mtime,
        perm: DIR_PERM,
        nlink: 2,
    }
}

/// A node's attributes: files at full size whether or not their content is here.
pub(crate) fn node_attr(ino: u64, n: &Node) -> Attr {
    if n.is_dir {
        return dir_attr(ino, system_time(n.mtime_ms));
    }
    Attr {
        ino,
        kind: Kind::File,
        size: n.size(),
        blocks: n.size().div_ceil(512),
        mtime: system_time(n.mtime_ms),
        perm: FILE_PERM,
        nlink: 1,
    }
}

/// An on-demand folder's contents for a file system to serve. Shared between the FUSE thread,
/// the read tasks and the daemon (refreshes and pins).
pub struct View {
    layout: RwLock<Arc<Layout>>,
    hydrator: Arc<Hydrator>,
    /// Open files: each handle reads the version that was current when it was opened, even after
    /// a refresh.
    open: Mutex<HashMap<u64, FileRef>>,
    next_fh: AtomicU64,
}

impl View {
    pub fn new(tree: Tree, hydrator: Arc<Hydrator>) -> Self {
        Self {
            layout: RwLock::new(Arc::new(Layout::build(tree, None))),
            hydrator,
            open: Mutex::new(HashMap::new()),
            next_fh: AtomicU64::new(1),
        }
    }

    fn current(&self) -> Arc<Layout> {
        self.layout.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn handles(&self) -> std::sync::MutexGuard<'_, HashMap<u64, FileRef>> {
        self.open.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn hydrator(&self) -> &Arc<Hydrator> {
        &self.hydrator
    }

    /// The inode number of a path (`""` is the root), if it is in the current tree.
    pub fn ino(&self, path: &str) -> Option<u64> {
        self.current().by_path.get(path).copied()
    }

    /// `name` in the folder `parent`.
    pub fn lookup(&self, parent: u64, name: &str) -> Result<Attr, FsError> {
        let l = self.current();
        let dir = l.path(parent)?;
        if l.kind(parent) != Kind::Dir {
            return Err(FsError::NotDir);
        }
        if name.is_empty() || name == "." || name == ".." || name.contains('/') {
            return Err(FsError::NotFound);
        }
        let path = if dir.is_empty() {
            name.to_owned()
        } else {
            format!("{dir}/{name}")
        };
        let ino = *l.by_path.get(&path).ok_or(FsError::NotFound)?;
        l.attr(ino)
    }

    pub fn getattr(&self, ino: u64) -> Result<Attr, FsError> {
        self.current().attr(ino)
    }

    /// A folder's entries from `offset` on (`.` and `..` first), each with the offset after it.
    pub fn readdir(&self, ino: u64, offset: u64) -> Result<Vec<DirEntry>, FsError> {
        let l = self.current();
        let children = match l.children.get(&ino) {
            Some(c) => c,
            None => {
                l.path(ino)?;
                return Err(FsError::NotDir);
            }
        };
        let mut all = vec![(ino, Kind::Dir, ".".to_owned())];
        all.push((l.parent_of(ino), Kind::Dir, "..".to_owned()));
        for &c in children {
            let name = l.path(c)?.rsplit('/').next().unwrap_or_default().to_owned();
            all.push((c, l.kind(c), name));
        }
        Ok(all
            .into_iter()
            .enumerate()
            .skip(usize::try_from(offset).unwrap_or(usize::MAX))
            .map(|(i, (ino, kind, name))| DirEntry {
                ino,
                kind,
                name,
                offset: i as u64 + 1,
            })
            .collect())
    }

    /// Opens a file for reading and returns its handle. Writing is refused: see [`FsError::ReadOnly`].
    pub fn open(&self, ino: u64, write: bool) -> Result<u64, FsError> {
        let l = self.current();
        let file = match l.node(ino)? {
            None => return Err(FsError::IsDir),
            Some(n) if n.is_dir => return Err(FsError::IsDir),
            Some(n) => n.file.clone().ok_or(FsError::NotFound)?,
        };
        if write {
            return Err(FsError::ReadOnly);
        }
        let fh = self.next_fh.fetch_add(1, Ordering::Relaxed);
        self.handles().insert(fh, file);
        Ok(fh)
    }

    /// The version an open handle reads.
    pub fn opened(&self, fh: u64) -> Option<FileRef> {
        self.handles().get(&fh).cloned()
    }

    pub fn release(&self, fh: u64) {
        self.handles().remove(&fh);
    }

    /// How many files are open.
    pub fn open_count(&self) -> usize {
        self.handles().len()
    }

    /// Bytes `offset..offset+len` of an open file, through the hydrator (none past the end).
    pub async fn read(&self, fh: u64, offset: u64, len: u64) -> Result<Vec<u8>, FsError> {
        let file = self.opened(fh).ok_or(FsError::BadHandle)?;
        if offset >= file.size || len == 0 {
            return Ok(Vec::new());
        }
        match self.hydrator.read(&file, offset, len).await {
            Ok(b) => Ok(b),
            Err(VfsError::OutOfRange) => Ok(Vec::new()),
            Err(_) => Err(FsError::Io),
        }
    }

    /// An extended attribute's value. Only files have [`PINNED_XATTR`].
    pub fn getxattr(&self, ino: u64, name: &str) -> Result<Vec<u8>, FsError> {
        let l = self.current();
        match l.node(ino)? {
            Some(n) if !n.is_dir && name == PINNED_XATTR => {
                let id = n.file.as_ref().map(|f| f.id.as_str()).unwrap_or_default();
                let pinned = self.hydrator.cache().is_pinned(id);
                Ok(if pinned { b"1".to_vec() } else { b"0".to_vec() })
            }
            _ => Err(FsError::NoAttr),
        }
    }

    /// The names of an inode's extended attributes, each NUL-terminated.
    pub fn listxattr(&self, ino: u64) -> Result<Vec<u8>, FsError> {
        Ok(match self.current().node(ino)? {
            Some(n) if !n.is_dir => format!("{PINNED_XATTR}\0").into_bytes(),
            _ => Vec::new(),
        })
    }

    /// Replaces the tree in one step and says what the kernel must forget. A path keeps its inode
    /// number while it stays the same kind; open files keep reading the version they opened.
    pub fn refresh(&self, tree: Tree) -> Vec<Change> {
        let mut slot = self.layout.write().unwrap_or_else(|p| p.into_inner());
        let old = slot.clone();
        let new = Layout::build(tree, Some(&old));
        let mut changes = Vec::new();
        let entry = |l: &Layout, path: &str| {
            let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
            Change::Entry {
                parent: l.by_path.get(parent).copied().unwrap_or(ROOT),
                name: name.to_owned(),
            }
        };
        for n in old.tree.iter() {
            let ino = old.by_path[&n.path];
            match new.tree.get(&n.path) {
                Some(m) if new.by_path.get(&n.path) == Some(&ino) => {
                    if m != n {
                        changes.push(Change::Inode(ino));
                    }
                }
                _ => {
                    changes.push(entry(&old, &n.path));
                    changes.push(Change::Inode(ino));
                }
            }
        }
        for n in new.tree.iter() {
            if old.by_path.get(&n.path) != new.by_path.get(&n.path) {
                changes.push(entry(&new, &n.path));
            }
        }
        if old.root_mtime != new.root_mtime {
            changes.push(Change::Inode(ROOT));
        }
        *slot = Arc::new(new);
        changes
    }

    /// Pins a file, or every file in a folder (`""` is the whole folder): fetches what is missing
    /// and keeps it on the device (FR-O5).
    pub async fn pin(&self, path: &str) -> wasl_vfs::Result<()> {
        for f in self.files(path)? {
            self.hydrator.pin(&f).await?;
        }
        Ok(())
    }

    /// Lets a file's (or a folder's files') blocks be evicted again.
    pub fn unpin(&self, path: &str) -> wasl_vfs::Result<()> {
        for f in self.files(path)? {
            self.hydrator.unpin(&f)?;
        }
        Ok(())
    }

    /// Whether the file at `path` is pinned.
    pub fn is_pinned(&self, path: &str) -> bool {
        let l = self.current();
        l.tree
            .get(path)
            .and_then(|n| n.file.as_ref())
            .is_some_and(|f| self.hydrator.cache().is_pinned(&f.id))
    }

    fn files(&self, path: &str) -> wasl_vfs::Result<Vec<FileRef>> {
        self.current()
            .files_under(path)
            .map_err(|_| VfsError::Unavailable(format!("{path}: not in the on-demand folder")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, version: &str, size: u64) -> Node {
        Node {
            path: path.into(),
            is_dir: false,
            mtime_ms: 1_700_000_000_000,
            file: Some(FileRef {
                id: format!("id:{path}"),
                version: version.into(),
                size,
            }),
        }
    }

    fn dir(path: &str) -> Node {
        Node {
            path: path.into(),
            is_dir: true,
            mtime_ms: 5,
            file: None,
        }
    }

    fn layout(nodes: Vec<Node>) -> Layout {
        Layout::build(Tree::new(nodes), None)
    }

    #[test]
    fn inodes_are_stable_per_path_and_never_reused() {
        let a = layout(vec![file("a/x.txt", "1", 3), file("b.txt", "1", 3)]);
        let ax = a.by_path["a/x.txt"];
        let b = Layout::build(
            Tree::new(vec![file("a/x.txt", "2", 9), file("c.txt", "1", 1)]),
            Some(&a),
        );
        assert_eq!(ax, b.by_path["a/x.txt"], "same path, same inode");
        assert_eq!(a.by_path["a"], b.by_path["a"]);
        let all_a: Vec<u64> = a.by_path.values().copied().collect();
        assert!(
            !all_a.contains(&b.by_path["c.txt"]),
            "a new path gets a number never used before"
        );
        // b.txt went away; when it comes back it is a new inode.
        let c = Layout::build(Tree::new(vec![file("b.txt", "1", 3)]), Some(&b));
        assert_ne!(a.by_path["b.txt"], c.by_path["b.txt"]);
        // A file that becomes a folder is a new inode too.
        let d = Layout::build(Tree::new(vec![file("c.txt/inner", "1", 1)]), Some(&b));
        assert_ne!(b.by_path["c.txt"], d.by_path["c.txt"]);
        assert_eq!(ROOT, d.by_path[""]);
    }

    #[test]
    fn attributes_show_full_size_time_and_modes() {
        let l = layout(vec![file("big.bin", "1", 20 << 30), dir("empty")]);
        let f = l.attr(l.by_path["big.bin"]).unwrap();
        assert_eq!(Kind::File, f.kind);
        assert_eq!(20 << 30, f.size);
        assert_eq!((20u64 << 30) / 512, f.blocks);
        assert_eq!(0o644, f.perm);
        assert_eq!(UNIX_EPOCH + Duration::from_millis(1_700_000_000_000), f.mtime);
        let d = l.attr(l.by_path["empty"]).unwrap();
        assert_eq!((Kind::Dir, 0o755, 2), (d.kind, d.perm, d.nlink));
        let root = l.attr(ROOT).unwrap();
        assert_eq!(Kind::Dir, root.kind);
        assert_eq!(f.mtime, root.mtime, "the root shows the newest time");
        assert_eq!(Err(FsError::NotFound), l.attr(9999));
        assert_eq!(UNIX_EPOCH - Duration::from_millis(1500), system_time(-1500));
    }

    #[test]
    fn children_are_listed_in_name_order() {
        let l = layout(vec![
            file("z.txt", "1", 1),
            file("a b/1", "1", 1),
            file("a/2", "1", 1),
            file("a-b", "1", 1),
        ]);
        let names =
            |ino: u64| -> Vec<&str> { l.children[&ino].iter().map(|c| l.by_ino[c].as_str()).collect() };
        assert_eq!(vec!["a", "a b", "a-b", "z.txt"], names(ROOT));
        assert_eq!(vec!["a/2"], names(l.by_path["a"]));
    }
}
