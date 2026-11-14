//! An in-memory Drive: real ids, versions, MD5s, a bin, duplicate names and failure injection, so
//! whole runs can be tested without the network.

use crate::remote::{ItemUpdate, Listing, ProgressFn, Remote, RemoteInfo, Revision, UploadSpec};
use async_trait::async_trait;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::Path;
use std::sync::Mutex;
use wasl_core::RemoteEntry;
use wasl_drive::{DriveError, Result};

pub const ROOT_ID: &str = "root";

#[derive(Debug, Clone)]
struct Node {
    name: String,
    parent: Option<String>,
    is_dir: bool,
    content: Vec<u8>,
    version: i64,
    mtime: i64,
    trashed: bool,
    google_mime: Option<String>,
    shortcut: bool,
    /// Earlier contents, oldest first: (revision id, content, mtime, keep forever).
    revisions: Vec<(String, Vec<u8>, i64, bool)>,
    /// Drive's appProperties.
    properties: BTreeMap<String, String>,
}

impl Node {
    /// Replaces the content, keeping the old one as a revision.
    fn new_version(&mut self, content: Vec<u8>, mtime: i64) {
        let old = std::mem::replace(&mut self.content, content);
        self.revisions
            .push((format!("rev{}", self.version), old, self.mtime, false));
        self.version += 1;
        self.mtime = mtime;
    }
}

/// Which call a queued failure applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Op {
    List,
    /// [`Remote::list_top_level`]: one folder's own items, apart from whole-subtree listings.
    ListTopLevel,
    Download,
    Export,
    Upload,
    CreateFolder,
    Trash,
}

#[derive(Default)]
struct State {
    nodes: HashMap<String, Node>,
    next_id: u64,
    clock: i64,
    failures: HashMap<Op, VecDeque<DriveError>>,
    /// Names whose transfers fail every time (a file Drive refuses).
    poisoned: Vec<String>,
    calls: BTreeMap<String, u32>,
    /// Content bytes received by uploads and sent by downloads.
    bytes_up: u64,
    bytes_down: u64,
    /// A listing that lags behind Drive, as Drive's change list does for a few seconds.
    lagging: Option<HashMap<String, Node>>,
}

pub struct MemoryRemote {
    state: Mutex<State>,
}

impl Default for MemoryRemote {
    fn default() -> Self {
        Self::new()
    }
}

fn md5_hex(bytes: &[u8]) -> String {
    wasl_fs::hash_reader(bytes).map(|h| h.md5).unwrap_or_default()
}

impl MemoryRemote {
    /// An empty Drive with a root folder.
    pub fn new() -> MemoryRemote {
        let mut s = State {
            clock: 1_700_000_000_000,
            ..State::default()
        };
        s.nodes.insert(
            ROOT_ID.into(),
            Node {
                name: "My Drive".into(),
                parent: None,
                is_dir: true,
                content: Vec::new(),
                version: 1,
                mtime: 0,
                trashed: false,
                google_mime: None,
                shortcut: false,
                revisions: Vec::new(),
                properties: BTreeMap::new(),
            },
        );
        MemoryRemote { state: Mutex::new(s) }
    }

    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Creates folders and a file at `path` (relative to the root) and returns the file's id.
    pub fn put_file(&self, path: &str, content: &[u8]) -> String {
        let mut s = self.st();
        let parent = ensure_dirs(&mut s, parent_path(path));
        let name = name_of(path).to_owned();
        if let Some(id) = child(&s, &parent, &name) {
            let t = tick(&mut s);
            s.nodes.get_mut(&id).unwrap().new_version(content.to_vec(), t);
            return id;
        }
        insert(&mut s, &parent, &name, false, content.to_vec(), None, false)
    }

    /// Creates a Google document (exported, never uploaded).
    pub fn put_google_doc(&self, path: &str, mime: &str) -> String {
        let mut s = self.st();
        let parent = ensure_dirs(&mut s, parent_path(path));
        insert(
            &mut s,
            &parent,
            name_of(path),
            false,
            b"doc".to_vec(),
            Some(mime.into()),
            false,
        )
    }

    /// A second file with the same name in the same folder (Drive allows it).
    pub fn put_duplicate(&self, path: &str, content: &[u8]) -> String {
        let mut s = self.st();
        let parent = ensure_dirs(&mut s, parent_path(path));
        insert(
            &mut s,
            &parent,
            name_of(path),
            false,
            content.to_vec(),
            None,
            false,
        )
    }

    pub fn put_shortcut(&self, path: &str) -> String {
        let mut s = self.st();
        let parent = ensure_dirs(&mut s, parent_path(path));
        insert(&mut s, &parent, name_of(path), false, Vec::new(), None, true)
    }

    pub fn mkdirs(&self, path: &str) -> String {
        let mut s = self.st();
        ensure_dirs(&mut s, path)
    }

    /// Renames or moves the item at `from` to `to`, keeping its id, as a user would on the web.
    pub fn move_path(&self, from: &str, to: &str) -> bool {
        let mut s = self.st();
        let Some(id) = resolve(&s, from) else { return false };
        let parent = ensure_dirs(&mut s, parent_path(to));
        let n = s.nodes.get_mut(&id).unwrap();
        n.parent = Some(parent);
        n.name = name_of(to).to_owned();
        n.version += 1;
        true
    }

    /// Moves the item at `path` to the bin, as a user would on the web.
    pub fn trash_path(&self, path: &str) -> bool {
        let mut s = self.st();
        match resolve(&s, path) {
            Some(id) => {
                s.nodes.get_mut(&id).unwrap().trashed = true;
                true
            }
            None => false,
        }
    }

    /// Content of the live file at `path`.
    pub fn content(&self, path: &str) -> Option<Vec<u8>> {
        let s = self.st();
        resolve(&s, path).map(|id| s.nodes[&id].content.clone())
    }

    pub fn id_of(&self, path: &str) -> Option<String> {
        resolve(&self.st(), path)
    }

    pub fn is_trashed(&self, id: &str) -> bool {
        self.st().nodes.get(id).is_some_and(|n| n.trashed)
    }

    /// Every live file as path → content.
    pub fn files(&self) -> BTreeMap<String, Vec<u8>> {
        let s = self.st();
        let mut out = BTreeMap::new();
        collect(&s, ROOT_ID, "", &mut out);
        out
    }

    /// The next call of `op` fails with `e` (queued in order).
    pub fn fail_next(&self, op: Op, e: DriveError) {
        self.st().failures.entry(op).or_default().push_back(e);
    }

    /// Every transfer of a file with this name fails.
    pub fn poison(&self, name: &str) {
        self.st().poisoned.push(name.to_owned());
    }

    /// Content bytes uploaded so far (file contents written by uploads).
    pub fn bytes_uploaded(&self) -> u64 {
        self.st().bytes_up
    }

    /// Content bytes downloaded so far (downloads and revision downloads).
    pub fn bytes_downloaded(&self) -> u64 {
        self.st().bytes_down
    }

    /// The appProperties of item `id`.
    pub fn properties(&self, id: &str) -> BTreeMap<String, String> {
        self.st()
            .nodes
            .get(id)
            .map(|n| n.properties.clone())
            .unwrap_or_default()
    }

    /// Sets (`Some`) or removes (`None`) one appProperty, as another client could.
    pub fn set_property(&self, id: &str, key: &str, value: Option<&str>) {
        if let Some(n) = self.st().nodes.get_mut(id) {
            match value {
                Some(v) => n.properties.insert(key.into(), v.into()),
                None => n.properties.remove(key),
            };
        }
    }

    /// Raises item `id`'s version without changing it, as Drive does on its own shortly after
    /// many uploads (the create response's version is often one or two behind).
    pub fn bump_version(&self, id: &str) {
        if let Some(n) = self.st().nodes.get_mut(id) {
            n.version += 1;
        }
    }

    /// Whether item `id` exists at all (live or in the bin).
    pub fn exists(&self, id: &str) -> bool {
        self.st().nodes.contains_key(id)
    }

    /// How often each call was made.
    /// From now on listings show Drive as it is now, while files keep changing underneath: Drive's
    /// change list reports other devices' uploads seconds late. `false` catches up.
    pub fn lag_listing(&self, lag: bool) {
        let mut s = self.st();
        s.lagging = lag.then(|| s.nodes.clone());
    }

    /// What a listing of `root_id` shows: everything under it, or (not `deep`) its own items.
    fn listing(&self, root_id: &str, deep: bool) -> Result<Listing> {
        let s = self.st();
        match s.nodes.get(root_id) {
            Some(n) if n.is_dir && !n.trashed => {}
            _ => return Err(DriveError::NotFound("The Drive folder is gone".into())),
        }
        let mut out = Listing::default();
        match &s.lagging {
            Some(nodes) => {
                let seen = State {
                    nodes: nodes.clone(),
                    ..State::default()
                };
                walk(&seen, root_id, "", &mut out, deep);
            }
            None => walk(&s, root_id, "", &mut out, deep),
        }
        Ok(out)
    }

    pub fn calls(&self, op: &str) -> u32 {
        self.st().calls.get(op).copied().unwrap_or(0)
    }

    fn enter(&self, op: Op, name: Option<&str>) -> Result<()> {
        let mut s = self.st();
        *s.calls.entry(format!("{op:?}")).or_default() += 1;
        if let Some(e) = s.failures.get_mut(&op).and_then(VecDeque::pop_front) {
            return Err(e);
        }
        if let Some(n) = name
            && s.poisoned.iter().any(|p| p == n)
        {
            return Err(DriveError::Http {
                code: 400,
                reason: Some("poisoned".into()),
                message: format!("{n} is refused"),
            });
        }
        Ok(())
    }
}

fn parent_path(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(p, _)| p)
}

fn name_of(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, n)| n)
}

fn tick(s: &mut State) -> i64 {
    s.clock += 1000;
    s.clock
}

fn child(s: &State, parent: &str, name: &str) -> Option<String> {
    let mut found: Vec<(&String, &Node)> = s
        .nodes
        .iter()
        .filter(|(_, n)| !n.trashed && n.parent.as_deref() == Some(parent) && n.name == name)
        .collect();
    found.sort_by_key(|(id, n)| (std::cmp::Reverse(n.mtime), (*id).clone()));
    found.first().map(|(id, _)| (*id).clone())
}

fn insert(
    s: &mut State,
    parent: &str,
    name: &str,
    is_dir: bool,
    content: Vec<u8>,
    google: Option<String>,
    shortcut: bool,
) -> String {
    s.next_id += 1;
    let id = format!("m{}", s.next_id);
    let t = tick(s);
    s.nodes.insert(
        id.clone(),
        Node {
            name: name.into(),
            parent: Some(parent.into()),
            is_dir,
            content,
            version: 1,
            mtime: t,
            trashed: false,
            google_mime: google,
            shortcut,
            revisions: Vec::new(),
            properties: BTreeMap::new(),
        },
    );
    id
}

fn ensure_dirs(s: &mut State, path: &str) -> String {
    let mut cur = ROOT_ID.to_owned();
    for part in path.split('/').filter(|p| !p.is_empty()) {
        cur = match child(s, &cur, part) {
            Some(id) => id,
            None => insert(s, &cur.clone(), part, true, Vec::new(), None, false),
        };
    }
    cur
}

fn resolve(s: &State, path: &str) -> Option<String> {
    let mut cur = ROOT_ID.to_owned();
    for part in path.split('/').filter(|p| !p.is_empty()) {
        cur = child(s, &cur, part)?;
    }
    Some(cur)
}

fn collect(s: &State, dir: &str, prefix: &str, out: &mut BTreeMap<String, Vec<u8>>) {
    for (id, n) in s
        .nodes
        .iter()
        .filter(|(_, n)| !n.trashed && n.parent.as_deref() == Some(dir))
    {
        let path = if prefix.is_empty() {
            n.name.clone()
        } else {
            format!("{prefix}/{}", n.name)
        };
        if n.is_dir {
            collect(s, id, &path, out);
        } else if !n.shortcut {
            out.insert(path, n.content.clone());
        }
    }
}

fn info(id: &str, n: &Node) -> RemoteInfo {
    RemoteInfo {
        id: id.to_owned(),
        name: n.name.clone(),
        size: (!n.is_dir).then_some(n.content.len() as u64),
        md5: (!n.is_dir && n.google_mime.is_none()).then(|| md5_hex(&n.content)),
        mtime: n.mtime,
        version: Some(n.version),
    }
}

#[async_trait]
impl Remote for MemoryRemote {
    async fn list_subtree(&self, root_id: &str, _drive_id: Option<&str>) -> Result<Listing> {
        self.enter(Op::List, None)?;
        self.listing(root_id, true)
    }

    /// One level, as [`crate::DriveRemote`] reads it; counted and failed as its own call
    /// ([`Op::ListTopLevel`]), so tests can tell it from a whole-subtree listing.
    async fn list_top_level(&self, folder_id: &str, _drive_id: Option<&str>) -> Result<Listing> {
        self.enter(Op::ListTopLevel, None)?;
        self.listing(folder_id, false)
    }

    async fn read_range(&self, id: &str, expect: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
        self.enter(Op::Download, None)?;
        let mut s = self.st();
        let node = s
            .nodes
            .get(id)
            .filter(|n| !n.trashed)
            .ok_or_else(|| DriveError::NotFound(id.to_owned()))?;
        if expect != node.version.to_string() && expect != md5_hex(&node.content) {
            return Err(DriveError::Integrity(format!(
                "{id} changed on Drive while it was being read"
            )));
        }
        let content = node.content.clone();
        let from = (offset as usize).min(content.len());
        let to = ((offset + len) as usize).min(content.len());
        s.bytes_down += (to - from) as u64;
        Ok(content[from..to].to_vec())
    }

    async fn download(&self, entry: &RemoteEntry, dest: &Path, progress: ProgressFn<'_>) -> Result<()> {
        self.enter(Op::Download, Some(&entry.name))?;
        let content = {
            let s = self.st();
            let n = s
                .nodes
                .get(&entry.id)
                .filter(|n| !n.trashed)
                .ok_or_else(|| DriveError::NotFound(entry.id.clone()))?;
            n.content.clone()
        };
        self.st().bytes_down += content.len() as u64;
        write(dest, &content)?;
        progress(content.len() as u64);
        Ok(())
    }

    async fn export(
        &self,
        entry: &RemoteEntry,
        mime: &str,
        dest: &Path,
        progress: ProgressFn<'_>,
    ) -> Result<()> {
        self.enter(Op::Export, Some(&entry.name))?;
        let body = format!("export of {} as {mime}", entry.id).into_bytes();
        write(dest, &body)?;
        progress(body.len() as u64);
        Ok(())
    }

    async fn upload(&self, spec: UploadSpec<'_>, progress: ProgressFn<'_>) -> Result<RemoteInfo> {
        self.upload_with_properties(spec, &[], progress).await
    }

    async fn current(&self, id: &str) -> Result<Option<RemoteInfo>> {
        let s = self.st();
        match s.nodes.get(id) {
            Some(n) if !n.trashed => Ok(Some(info(id, n))),
            _ => Err(DriveError::NotFound(id.into())),
        }
    }

    async fn upload_with_properties(
        &self,
        spec: UploadSpec<'_>,
        properties: &[(&str, &str)],
        progress: ProgressFn<'_>,
    ) -> Result<RemoteInfo> {
        self.enter(Op::Upload, Some(spec.name))?;
        let content = std::fs::read(spec.src).map_err(|e| DriveError::Io(e.to_string()))?;
        progress(content.len() as u64);
        let mut s = self.st();
        s.bytes_up += content.len() as u64;
        let t = tick(&mut s);
        let id = match spec.existing_id {
            Some(id) => {
                let n = s
                    .nodes
                    .get_mut(id)
                    .filter(|n| !n.trashed)
                    .ok_or_else(|| DriveError::NotFound(id.into()))?;
                n.new_version(content, spec.mtime_ms.max(t));
                n.name = spec.name.into();
                id.to_owned()
            }
            None => {
                if !s
                    .nodes
                    .get(spec.parent_id)
                    .is_some_and(|n| n.is_dir && !n.trashed)
                {
                    return Err(DriveError::NotFound(format!("parent {}", spec.parent_id)));
                }
                let id = insert(&mut s, spec.parent_id, spec.name, false, content, None, false);
                s.nodes.get_mut(&id).unwrap().mtime = spec.mtime_ms.max(1);
                id
            }
        };
        let n = s.nodes.get_mut(&id).unwrap();
        for (k, v) in properties {
            n.properties.insert((*k).into(), (*v).into());
        }
        Ok(info(&id, &s.nodes[&id]))
    }

    async fn create_folder(&self, parent_id: &str, name: &str) -> Result<RemoteInfo> {
        self.create_folder_with_properties(parent_id, name, &[]).await
    }

    async fn create_folder_with_properties(
        &self,
        parent_id: &str,
        name: &str,
        properties: &[(&str, &str)],
    ) -> Result<RemoteInfo> {
        self.enter(Op::CreateFolder, Some(name))?;
        let mut s = self.st();
        if !s.nodes.get(parent_id).is_some_and(|n| n.is_dir && !n.trashed) {
            return Err(DriveError::NotFound(format!("parent {parent_id}")));
        }
        let id = insert(&mut s, parent_id, name, true, Vec::new(), None, false);
        let n = s.nodes.get_mut(&id).unwrap();
        for (k, v) in properties {
            n.properties.insert((*k).into(), (*v).into());
        }
        Ok(info(&id, &s.nodes[&id]))
    }

    async fn update_item(&self, id: &str, update: ItemUpdate<'_>) -> Result<RemoteInfo> {
        let mut s = self.st();
        if let Some((_, to)) = update.parents
            && !s.nodes.get(to).is_some_and(|n| n.is_dir && !n.trashed)
        {
            return Err(DriveError::NotFound(format!("parent {to}")));
        }
        let n = s
            .nodes
            .get_mut(id)
            .filter(|n| !n.trashed)
            .ok_or_else(|| DriveError::NotFound(id.into()))?;
        if let Some((from, to)) = update.parents {
            if n.parent.as_deref() != Some(from) {
                return Err(DriveError::Http {
                    code: 400,
                    reason: Some("notInParent".into()),
                    message: format!("{id} is not in {from}"),
                });
            }
            n.parent = Some(to.into());
        }
        if let Some(name) = update.name {
            n.name = name.into();
        }
        for (k, v) in update.properties {
            n.properties.insert((*k).into(), (*v).into());
        }
        n.version += 1;
        Ok(info(id, n))
    }

    async fn trash(&self, id: &str) -> Result<()> {
        self.enter(Op::Trash, None)?;
        let mut s = self.st();
        let n = s
            .nodes
            .get_mut(id)
            .ok_or_else(|| DriveError::NotFound(id.into()))?;
        n.trashed = true;
        Ok(())
    }

    async fn delete_permanently(&self, id: &str) -> Result<()> {
        self.st().nodes.remove(id);
        Ok(())
    }

    async fn list_revisions(&self, id: &str) -> Result<Vec<Revision>> {
        let s = self.st();
        let n = s.nodes.get(id).ok_or_else(|| DriveError::NotFound(id.into()))?;
        let mut out: Vec<Revision> = n
            .revisions
            .iter()
            .map(|(rid, content, mtime, keep)| Revision {
                id: rid.clone(),
                modified_ms: *mtime,
                size: Some(content.len() as u64),
                md5: Some(md5_hex(content)),
                keep_forever: *keep,
                modified_by: None,
            })
            .collect();
        out.push(Revision {
            id: format!("rev{}", n.version),
            modified_ms: n.mtime,
            size: Some(n.content.len() as u64),
            md5: Some(md5_hex(&n.content)),
            keep_forever: false,
            modified_by: None,
        });
        Ok(out)
    }

    async fn download_revision(&self, id: &str, revision_id: &str, dest: &Path) -> Result<()> {
        let content = {
            let s = self.st();
            let n = s.nodes.get(id).ok_or_else(|| DriveError::NotFound(id.into()))?;
            if revision_id == format!("rev{}", n.version) {
                n.content.clone()
            } else {
                n.revisions
                    .iter()
                    .find(|(rid, ..)| rid == revision_id)
                    .map(|(_, c, ..)| c.clone())
                    .ok_or_else(|| DriveError::NotFound(revision_id.into()))?
            }
        };
        self.st().bytes_down += content.len() as u64;
        write(dest, &content)
    }

    async fn keep_revision(&self, id: &str, revision_id: &str, keep: bool) -> Result<()> {
        let mut s = self.st();
        let n = s
            .nodes
            .get_mut(id)
            .ok_or_else(|| DriveError::NotFound(id.into()))?;
        let r = n
            .revisions
            .iter_mut()
            .find(|(rid, ..)| rid == revision_id)
            .ok_or_else(|| DriveError::NotFound(revision_id.into()))?;
        r.3 = keep;
        Ok(())
    }

    async fn untrash(&self, id: &str) -> Result<()> {
        let mut s = self.st();
        let n = s
            .nodes
            .get_mut(id)
            .ok_or_else(|| DriveError::NotFound(id.into()))?;
        n.trashed = false;
        Ok(())
    }
}

fn write(dest: &Path, content: &[u8]) -> Result<()> {
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p).map_err(|e| DriveError::Io(e.to_string()))?;
    }
    std::fs::write(dest, content).map_err(|e| DriveError::Io(e.to_string()))
}

/// Port of the Drive listing walk: duplicates (case-insensitive, newest wins) and shortcuts are
/// reported, not listed.
fn walk(s: &State, dir: &str, prefix: &str, out: &mut Listing, deep: bool) {
    let mut kids: Vec<(&String, &Node)> = s
        .nodes
        .iter()
        .filter(|(_, n)| !n.trashed && n.parent.as_deref() == Some(dir))
        .collect();
    kids.sort_by_key(|(id, n)| (std::cmp::Reverse(n.mtime), (*id).clone()));
    let mut seen = std::collections::HashSet::new();
    for (id, n) in kids {
        let key = if prefix.is_empty() {
            n.name.clone()
        } else {
            format!("{prefix}/{}", n.name)
        };
        if n.shortcut {
            out.shortcuts.push(key);
            continue;
        }
        if !seen.insert(wasl_core::identity::name_key(&n.name)) {
            out.duplicates.push(key);
            continue;
        }
        let i = info(id, n);
        if !n.properties.is_empty() {
            out.properties.insert(id.clone(), n.properties.clone());
        }
        out.entries.push(RemoteEntry {
            key: key.clone(),
            id: id.clone(),
            name: n.name.clone(),
            is_dir: n.is_dir,
            size: i.size,
            md5: i.md5,
            mtime: n.mtime,
            version: i.version,
            google_mime: n.google_mime.clone(),
            parent_id: n.parent.clone(),
        });
        if n.is_dir && deep {
            walk(s, id, &key, out, deep);
        }
    }
}
