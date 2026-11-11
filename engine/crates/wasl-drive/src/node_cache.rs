//! Account-wide metadata cache (port of `DriveNodeCache.kt`, plus `DriveRemoteFs.listSubtree`).
//!
//! One full crawl per account, then cheap deltas through the Changes API. Every pair resolves its
//! subtree from this cache instead of crawling Drive folder by folder. Storage sits behind
//! [`NodeStore`] so the SQLite store can replace [`MemoryNodeStore`].
//!
//! The first crawl of a big Drive takes minutes. Until it is stored, [`DriveNodeCache::list_subtree`]
//! does not wait for it: the crawl runs in the background and the pair's own folder tree is listed
//! live from Drive, so the first sync of a new account starts in seconds. Once the crawl is stored,
//! a listing waits for the change list that follows it (usually a page or two) and then reads the
//! cache.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;

use crate::client::{DriveClient, ListPage};
use crate::models::{DriveFile, GOOGLE_APPS_PREFIX};
use crate::time::parse_rfc3339;
use crate::{DriveError, Result};

/// A cached Drive file or folder (the Kotlin `DriveNodeEntity`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DriveNode {
    pub account_id: String,
    pub id: String,
    pub name: String,
    pub mime_type: String,
    /// The first parent; Drive items have exactly one since 2020.
    pub parent_id: Option<String>,
    pub drive_id: Option<String>,
    pub size: Option<u64>,
    pub md5: Option<String>,
    /// Epoch milliseconds, 0 when Drive sent none.
    pub modified_time: i64,
    pub version: Option<u64>,
    pub trashed: bool,
    pub is_folder: bool,
    pub is_shortcut: bool,
    pub shortcut_target_id: Option<String>,
    pub shortcut_target_mime: Option<String>,
    pub shared: bool,
    pub permission_count: Option<u32>,
    /// Drive `appProperties`, when the item has any.
    pub app_properties: Option<std::collections::BTreeMap<String, String>>,
}

impl DriveNode {
    /// The Kotlin `DriveFile.toNode(accountId)`.
    pub fn from_file(f: &DriveFile, account_id: &str) -> Self {
        DriveNode {
            account_id: account_id.to_string(),
            id: f.id.clone(),
            name: f.name.clone(),
            mime_type: f.mime_type.clone(),
            parent_id: f.parents.as_ref().and_then(|p| p.first().cloned()),
            drive_id: f.drive_id.clone(),
            size: f.size_u64(),
            md5: f.md5_checksum.clone(),
            modified_time: parse_rfc3339(f.modified_time.as_deref()).unwrap_or(0),
            version: f.version_u64(),
            trashed: f.trashed,
            is_folder: f.is_folder(),
            is_shortcut: f.is_shortcut(),
            shortcut_target_id: f.shortcut_details.as_ref().and_then(|s| s.target_id.clone()),
            shortcut_target_mime: f
                .shortcut_details
                .as_ref()
                .and_then(|s| s.target_mime_type.clone()),
            shared: f.shared,
            permission_count: f
                .permission_ids
                .as_ref()
                .map(|p| u32::try_from(p.len()).unwrap_or(u32::MAX)),
            app_properties: f.app_properties.clone().filter(|p| !p.is_empty()),
        }
    }
}

/// The cache's per-account bookkeeping (the fields of the Kotlin `AccountEntity` it uses).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountState {
    pub id: String,
    pub root_folder_id: Option<String>,
    /// `None` until a full crawl completed: then the next refresh crawls again.
    pub changes_token: Option<String>,
    pub last_crawl_at: Option<i64>,
    pub last_delta_at: Option<i64>,
}

impl AccountState {
    pub fn new(id: &str) -> Self {
        AccountState {
            id: id.to_string(),
            ..AccountState::default()
        }
    }
}

/// Storage of the cache. Implementations should make `upsert_all` atomic.
#[async_trait]
pub trait NodeStore: Send + Sync {
    async fn account(&self, account_id: &str) -> Result<Option<AccountState>>;
    async fn upsert_account(&self, account: AccountState) -> Result<()>;
    /// Sets the changes token and stamps `last_delta_at`; no-op for an unknown account.
    async fn set_changes_token(&self, account_id: &str, token: Option<&str>, at: i64) -> Result<()>;
    /// Every non-trashed node of the account.
    async fn all_for_account(&self, account_id: &str) -> Result<Vec<DriveNode>>;
    /// Every node of the account, trashed ones included.
    async fn count(&self, account_id: &str) -> Result<usize>;
    async fn upsert_all(&self, nodes: &[DriveNode]) -> Result<()>;
    async fn delete_ids(&self, account_id: &str, ids: &[String]) -> Result<()>;
    async fn clear_account(&self, account_id: &str) -> Result<()>;
    async fn clear_drive(&self, account_id: &str, drive_id: &str) -> Result<()>;
}

/// A [`NodeStore`] in memory. Its inherent methods are for tests and tools.
#[derive(Debug, Default)]
pub struct MemoryNodeStore {
    inner: Mutex<MemoryInner>,
}

#[derive(Debug, Default)]
struct MemoryInner {
    accounts: BTreeMap<String, AccountState>,
    nodes: BTreeMap<(String, String), DriveNode>,
}

impl MemoryNodeStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, MemoryInner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn set_account(&self, account: AccountState) {
        self.lock().accounts.insert(account.id.clone(), account);
    }

    pub fn account_state(&self, account_id: &str) -> Option<AccountState> {
        self.lock().accounts.get(account_id).cloned()
    }

    pub fn put(&self, node: DriveNode) {
        self.lock()
            .nodes
            .insert((node.account_id.clone(), node.id.clone()), node);
    }

    /// A node, trashed or not.
    pub fn node(&self, account_id: &str, id: &str) -> Option<DriveNode> {
        self.lock()
            .nodes
            .get(&(account_id.to_string(), id.to_string()))
            .cloned()
    }

    /// Ids of every node of the account, trashed ones included.
    pub fn ids(&self, account_id: &str) -> std::collections::BTreeSet<String> {
        self.lock()
            .nodes
            .values()
            .filter(|n| n.account_id == account_id)
            .map(|n| n.id.clone())
            .collect()
    }
}

#[async_trait]
impl NodeStore for MemoryNodeStore {
    async fn account(&self, account_id: &str) -> Result<Option<AccountState>> {
        Ok(self.account_state(account_id))
    }

    async fn upsert_account(&self, account: AccountState) -> Result<()> {
        self.set_account(account);
        Ok(())
    }

    async fn set_changes_token(&self, account_id: &str, token: Option<&str>, at: i64) -> Result<()> {
        if let Some(a) = self.lock().accounts.get_mut(account_id) {
            a.changes_token = token.map(str::to_string);
            a.last_delta_at = Some(at);
        }
        Ok(())
    }

    async fn all_for_account(&self, account_id: &str) -> Result<Vec<DriveNode>> {
        Ok(self
            .lock()
            .nodes
            .values()
            .filter(|n| n.account_id == account_id && !n.trashed)
            .cloned()
            .collect())
    }

    async fn count(&self, account_id: &str) -> Result<usize> {
        Ok(self
            .lock()
            .nodes
            .values()
            .filter(|n| n.account_id == account_id)
            .count())
    }

    async fn upsert_all(&self, nodes: &[DriveNode]) -> Result<()> {
        let mut inner = self.lock();
        for n in nodes {
            inner
                .nodes
                .insert((n.account_id.clone(), n.id.clone()), n.clone());
        }
        Ok(())
    }

    async fn delete_ids(&self, account_id: &str, ids: &[String]) -> Result<()> {
        let mut inner = self.lock();
        for id in ids {
            inner.nodes.remove(&(account_id.to_string(), id.clone()));
        }
        Ok(())
    }

    async fn clear_account(&self, account_id: &str) -> Result<()> {
        self.lock().nodes.retain(|(a, _), _| a != account_id);
        Ok(())
    }

    async fn clear_drive(&self, account_id: &str, drive_id: &str) -> Result<()> {
        self.lock()
            .nodes
            .retain(|(a, _), n| !(a == account_id && n.drive_id.as_deref() == Some(drive_id)));
        Ok(())
    }
}

/// A read-only view of one account's cache.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub account_id: String,
    pub root_id: String,
    by_id: HashMap<String, DriveNode>,
    children: HashMap<Option<String>, Vec<DriveNode>>,
}

impl Snapshot {
    pub fn node(&self, id: &str) -> Option<&DriveNode> {
        self.by_id.get(id)
    }

    pub fn children_of(&self, id: &str) -> &[DriveNode] {
        self.children
            .get(&Some(id.to_string()))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Display path like "My Drive/Photos/2026". The Drive API never lists the roots themselves
    /// (My Drive, shared drives), so the walk usually stops at a node whose parent is not cached;
    /// that parent id is what identifies the root.
    pub fn path_of(&self, id: &str, drive_names: &HashMap<String, String>) -> String {
        let Some(mut cur) = self.by_id.get(id) else {
            return String::new();
        };
        let mut parts: Vec<&str> = Vec::new();
        let mut guard = 0;
        loop {
            parts.push(&cur.name);
            let Some(p) = cur.parent_id.as_deref() else {
                break;
            };
            let Some(next) = self.by_id.get(p) else {
                break;
            };
            cur = next;
            guard += 1;
            if guard >= 256 {
                // parity: a parent cycle stops here with `cur` not yet pushed, as in Kotlin.
                break;
            }
        }
        // `cur` is the top-most cached ancestor; its parent (if any) is not in the cache.
        let top = cur;
        let missing_parent = top.parent_id.as_deref();
        let prefix: Option<&str> = if top.id == self.root_id {
            parts.pop();
            Some("My Drive")
        } else if missing_parent == Some(self.root_id.as_str()) {
            Some("My Drive")
        } else if let Some(drive_id) = top.drive_id.as_deref() {
            if top.id == drive_id {
                parts.pop();
            }
            Some(
                drive_names
                    .get(drive_id)
                    .map(String::as_str)
                    .unwrap_or("Shared drive"),
            )
        } else if let Some(name) = missing_parent.and_then(|p| drive_names.get(p)) {
            Some(name.as_str())
        } else {
            None // shared-with-me item or orphan: its own name is the first segment
        };
        parts.reverse();
        prefix
            .into_iter()
            .chain(parts)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("/")
    }
}

/// One file or folder under a listed subtree (the Kotlin `RemoteEntry`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    /// Relative path with remote names, '/'-separated.
    pub key: String,
    pub id: String,
    pub name: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub md5: Option<String>,
    pub mtime: i64,
    pub version: Option<u64>,
    /// The Google-native MIME type of a document that must be exported.
    pub google_mime: Option<String>,
    pub parent_id: Option<String>,
    /// Drive `appProperties`, when the item has any.
    pub app_properties: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubtreeListing {
    pub entries: Vec<RemoteEntry>,
    /// Keys of same-named (case-insensitive) siblings that lost to a newer one.
    pub duplicates: Vec<String>,
    /// Keys of shortcuts, which are not followed.
    pub shortcuts: Vec<String>,
}

/// See the module documentation.
pub struct DriveNodeCache {
    api: Arc<DriveClient>,
    store: Arc<dyn NodeStore>,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    crawled_drives: Mutex<HashMap<String, HashSet<String>>>,
    pages_read: AtomicU64,
    /// First reads running in the background: `account` or `account\0drive_id`.
    first_reads: Mutex<HashSet<String>>,
}

/// Folders listed at once while a subtree is read live.
const LIVE_PARALLEL: usize = 8;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Drive no longer accepts the saved changes token.
fn rejected_token(e: &DriveError) -> bool {
    matches!(
        e,
        DriveError::NotFound(_) | DriveError::Http { code: 400 | 410, .. }
    )
}

const CRAWL_BATCH: usize = 500;

impl DriveNodeCache {
    pub fn new(api: Arc<DriveClient>, store: Arc<dyn NodeStore>) -> Self {
        DriveNodeCache {
            api,
            store,
            locks: Mutex::new(HashMap::new()),
            crawled_drives: Mutex::new(HashMap::new()),
            pages_read: AtomicU64::new(0),
            first_reads: Mutex::new(HashSet::new()),
        }
    }

    /// Pages read from Drive so far by any crawl or delta. The first read of a big Drive takes
    /// minutes without other visible progress; this tells a stall watchdog it is still moving.
    /// (Kotlin kept it process-wide; here it is per cache.)
    pub fn pages_read(&self) -> u64 {
        self.pages_read.load(Ordering::Relaxed)
    }

    /// Ensures the cache is initialised and applies pending changes; true when anything changed.
    /// With `drive_id`, that shared drive is crawled once when the cache has nothing for it.
    pub async fn refresh(&self, account: &str, drive_id: Option<&str>, force: bool) -> Result<bool> {
        let lock = locked(&self.locks)
            .entry(account.to_string())
            .or_default()
            .clone();
        let _guard = lock.lock().await;

        let Some(acct) = self.store.account(account).await? else {
            return Ok(false);
        };
        let mut changed = false;
        if force || self.never_read(&acct).await? {
            self.full_crawl(account).await?;
            changed = true;
        }
        if self.ensure_shared_drive(account, drive_id).await? {
            changed = true;
        }
        let delta = match self.apply_delta(account).await {
            Ok(d) => d,
            Err(e) if rejected_token(&e) => {
                // Drive no longer accepts the saved changes token. Read everything again instead
                // of failing every run of every pair of this account until it is re-added.
                self.full_crawl(account).await?;
                self.ensure_shared_drive(account, drive_id).await?;
                self.apply_delta(account).await?;
                true
            }
            Err(e) => return Err(e),
        };
        Ok(changed || delta)
    }

    /// Crawls a shared drive once per cache when it has no nodes; marked done only after it
    /// completed.
    async fn ensure_shared_drive(&self, account: &str, drive_id: Option<&str>) -> Result<bool> {
        let Some(drive_id) = drive_id else {
            return Ok(false);
        };
        let done = locked(&self.crawled_drives)
            .get(account)
            .is_some_and(|d| d.contains(drive_id));
        if done {
            return Ok(false);
        }
        let has_nodes = self
            .store
            .all_for_account(account)
            .await?
            .iter()
            .any(|n| n.drive_id.as_deref() == Some(drive_id));
        let mut crawled = false;
        if !has_nodes {
            self.crawl_shared_drive(account, drive_id).await?;
            crawled = true;
        }
        locked(&self.crawled_drives)
            .entry(account.to_string())
            .or_default()
            .insert(drive_id.to_string());
        Ok(crawled)
    }

    async fn full_crawl(&self, account: &str) -> Result<()> {
        let root_id = self.api.root_id(account).await?;
        // Taken before the crawl so nothing that changes during it is lost.
        let token = self.api.start_page_token(account, None).await?;
        // Until the crawl completes there is no token, so an interrupted crawl starts over next
        // time instead of leaving a half-read Drive that makes files look deleted.
        self.store.set_changes_token(account, None, now_ms()).await?;
        locked(&self.crawled_drives).remove(account); // clear_account drops shared-drive nodes too
        self.store.clear_account(account).await?;
        let mut page_token: Option<String> = None;
        let mut batch = Vec::with_capacity(1000);
        loop {
            let page = self
                .api
                .list_page(
                    account,
                    &ListPage {
                        q: Some("trashed = false"),
                        page_token: page_token.as_deref(),
                        ..ListPage::default()
                    },
                )
                .await?;
            self.pages_read.fetch_add(1, Ordering::Relaxed);
            batch.extend(page.files.iter().map(|f| DriveNode::from_file(f, account)));
            if batch.len() >= CRAWL_BATCH {
                self.store.upsert_all(&batch).await?;
                batch.clear();
            }
            match page.next_page_token {
                Some(t) => page_token = Some(t),
                None => break,
            }
        }
        if !batch.is_empty() {
            self.store.upsert_all(&batch).await?;
        }
        let acct = self
            .store
            .account(account)
            .await?
            .unwrap_or_else(|| AccountState::new(account));
        self.store
            .upsert_account(AccountState {
                root_folder_id: Some(root_id),
                changes_token: Some(token),
                last_crawl_at: Some(now_ms()),
                ..acct
            })
            .await
    }

    /// Reads a whole shared drive, then stores it in one go. Nothing is stored if the read stops
    /// half way: "no nodes for this drive" is what tells the next refresh to crawl it again.
    async fn crawl_shared_drive(&self, account: &str, drive_id: &str) -> Result<()> {
        self.store.clear_drive(account, drive_id).await?;
        let mut all = Vec::new();
        let mut page_token: Option<String> = None;
        let read = async {
            loop {
                let page = self
                    .api
                    .list_page(
                        account,
                        &ListPage {
                            q: Some("trashed = false"),
                            page_token: page_token.as_deref(),
                            drive_id: Some(drive_id),
                            ..ListPage::default()
                        },
                    )
                    .await?;
                self.pages_read.fetch_add(1, Ordering::Relaxed);
                all.extend(page.files.iter().map(|f| DriveNode::from_file(f, account)));
                match page.next_page_token {
                    Some(t) => page_token = Some(t),
                    None => break,
                }
            }
            // An atomic `upsert_all` keeps this all-or-nothing even if the future is dropped.
            self.store.upsert_all(&all).await
        };
        if let Err(e) = read.await {
            let _ = self.store.clear_drive(account, drive_id).await;
            return Err(e);
        }
        Ok(())
    }

    async fn apply_delta(&self, account: &str) -> Result<bool> {
        let Some(acct) = self.store.account(account).await? else {
            return Ok(false);
        };
        let Some(mut token) = acct.changes_token else {
            return Ok(false);
        };
        let mut changed = false;
        // The last change of each item across all pages: `None` removes it (#144).
        let mut last: HashMap<String, Option<DriveNode>> = HashMap::new();
        loop {
            let page = self.api.changes_page(account, &token, None).await?;
            self.pages_read.fetch_add(1, Ordering::Relaxed);
            for c in &page.changes {
                if c.change_type.as_deref() == Some("drive") {
                    continue;
                }
                let Some(id) = c.file_id.as_ref() else {
                    continue;
                };
                let state = match &c.file {
                    Some(f) if !c.removed && !f.trashed => Some(DriveNode::from_file(f, account)),
                    _ => None,
                };
                last.insert(id.clone(), state);
            }
            if let Some(next) = page.next_page_token {
                token = next;
                continue;
            }
            // Changes apply in order (last state wins), so an item removed on one page and restored
            // on a later one stays. Kotlin applied all upserts before all removals and lost it.
            let (mut upserts, mut removals) = (Vec::new(), Vec::new());
            for (id, state) in last.drain() {
                match state {
                    Some(n) => upserts.push(n),
                    None => removals.push(id),
                }
            }
            if !upserts.is_empty() {
                self.store.upsert_all(&upserts).await?;
                changed = true;
            }
            if !removals.is_empty() {
                for chunk in removals.chunks(500) {
                    self.store.delete_ids(account, chunk).await?;
                }
                changed = true;
            }
            let new_token = page.new_start_page_token.unwrap_or(token);
            self.store
                .set_changes_token(account, Some(&new_token), now_ms())
                .await?;
            return Ok(changed);
        }
    }

    pub async fn snapshot(&self, account: &str) -> Result<Snapshot> {
        let acct = self
            .store
            .account(account)
            .await?
            .ok_or_else(|| DriveError::Unknown(format!("account missing: {account}")))?;
        let all = self.store.all_for_account(account).await?;
        let mut children: HashMap<Option<String>, Vec<DriveNode>> = HashMap::new();
        for n in &all {
            children.entry(n.parent_id.clone()).or_default().push(n.clone());
        }
        Ok(Snapshot {
            account_id: account.to_string(),
            root_id: acct.root_folder_id.unwrap_or_else(|| "root".into()),
            by_id: all.into_iter().map(|n| (n.id.clone(), n)).collect(),
            children,
        })
    }

    /// Drops the changes token so the next refresh crawls again.
    pub async fn invalidate(&self, account: &str) -> Result<()> {
        let Some(acct) = self.store.account(account).await? else {
            return Ok(());
        };
        self.store
            .upsert_account(AccountState {
                changes_token: None,
                ..acct
            })
            .await?;
        locked(&self.crawled_drives).remove(account);
        Ok(())
    }

    /// Upserts a node we just created or updated ourselves, so the next planning pass sees it
    /// without waiting for a delta.
    pub async fn remember(&self, account: &str, f: &DriveFile) -> Result<()> {
        self.store.upsert_all(&[DriveNode::from_file(f, account)]).await
    }

    pub async fn forget(&self, account: &str, id: &str) -> Result<()> {
        self.store.delete_ids(account, &[id.to_string()]).await
    }

    /// Refreshes, then lists everything under `root_id` (port of `DriveRemoteFs.listSubtree`).
    /// A folder that is gone from Drive fails with [`DriveError::NotFound`] instead of listing as
    /// empty, which would read as "every file deleted on Drive".
    ///
    /// While the account (or the shared drive) has never been read, its full read starts in the
    /// background and this listing is read live from Drive instead.
    pub async fn list_subtree(
        self: &Arc<Self>,
        account: &str,
        root_id: &str,
        drive_id: Option<&str>,
    ) -> Result<SubtreeListing> {
        if self.first_read_pending(account, drive_id).await? {
            self.start_first_read(account, drive_id);
            return self.live_read(account, root_id, drive_id, true).await;
        }
        self.refresh(account, drive_id, false).await?;
        let snap = self.snapshot(account).await?;
        if root_id != snap.root_id
            && root_id != "root"
            && Some(root_id) != drive_id
            && snap.node(root_id).is_none()
        {
            // Not in the cache: gone, or made so recently that the change list has not reported
            // it yet. Drive itself tells which (and the live read fails when it is gone).
            return self.live_read(account, root_id, drive_id, true).await;
        }
        let mut out = SubtreeListing::default();
        walk(&snap, root_id, "", &mut out);
        Ok(out)
    }

    /// The items directly in `folder_id`, one level, for checks that need no more: whether a
    /// folder holds a vault, whether it is empty. Like [`Self::list_subtree`], but while the
    /// account has never been read it reads only this folder from Drive, so it stays quick however
    /// big the folder is (a live walk of a large folder outlasted the vault check before a new
    /// pair, which then read as "can't reach Google Drive").
    pub async fn list_top_level(
        self: &Arc<Self>,
        account: &str,
        folder_id: &str,
        drive_id: Option<&str>,
    ) -> Result<SubtreeListing> {
        if self.first_read_pending(account, drive_id).await? {
            self.start_first_read(account, drive_id);
            return self.live_read(account, folder_id, drive_id, false).await;
        }
        self.refresh(account, drive_id, false).await?;
        let snap = self.snapshot(account).await?;
        if folder_id != snap.root_id
            && folder_id != "root"
            && Some(folder_id) != drive_id
            && snap.node(folder_id).is_none()
        {
            return self.live_read(account, folder_id, drive_id, false).await;
        }
        let mut out = SubtreeListing::default();
        take_children(snap.children_of(folder_id).iter().collect(), "", &mut out);
        Ok(out)
    }
}

impl DriveNodeCache {
    /// Whether the account's own Drive still needs its full read. A completed crawl sets the
    /// changes token and the crawl time, which say so even for an empty Drive (#97); without the
    /// crawl time (a cache from before it was kept), no nodes means not read.
    async fn never_read(&self, acct: &AccountState) -> Result<bool> {
        Ok(acct.root_folder_id.is_none()
            || acct.changes_token.is_none()
            || (acct.last_crawl_at.is_none() && self.store.count(&acct.id).await? == 0))
    }

    fn first_read_key(account: &str, drive_id: Option<&str>) -> String {
        match drive_id {
            Some(d) => format!("{account}\0{d}"),
            None => account.to_string(),
        }
    }

    /// True while the account or the shared drive has not been read once (or is being read). The
    /// store tells, not the background read: that read still applies the change list after its
    /// crawl is stored, and a listing meanwhile should wait for the cache rather than read live.
    async fn first_read_pending(&self, account: &str, drive_id: Option<&str>) -> Result<bool> {
        let Some(acct) = self.store.account(account).await? else {
            return Ok(false);
        };
        if self.never_read(&acct).await? {
            return Ok(true);
        }
        if let Some(d) = drive_id {
            let done = locked(&self.crawled_drives)
                .get(account)
                .is_some_and(|set| set.contains(d));
            if !done {
                let has_nodes = self
                    .store
                    .all_for_account(account)
                    .await?
                    .iter()
                    .any(|n| n.drive_id.as_deref() == Some(d));
                return Ok(!has_nodes);
            }
        }
        Ok(false)
    }

    /// Runs the full read once in the background; later listings use the cache when it is done.
    fn start_first_read(self: &Arc<Self>, account: &str, drive_id: Option<&str>) {
        let key = Self::first_read_key(account, drive_id);
        if !locked(&self.first_reads).insert(key.clone()) {
            return;
        }
        let me = self.clone();
        let account = account.to_string();
        let drive_id = drive_id.map(str::to_string);
        tokio::spawn(async move {
            // A failed read is retried by the next listing (it finds the cache still empty).
            let _ = me.refresh(&account, drive_id.as_deref(), false).await;
            locked(&me.first_reads).remove(&key);
        });
    }

    /// The subtree read folder by folder from Drive, a level at a time with several folders at
    /// once. Same entries, duplicates and shortcuts as the cached walk. Not `deep`: the root's own
    /// items only.
    async fn live_read(
        &self,
        account: &str,
        root_id: &str,
        drive_id: Option<&str>,
        deep: bool,
    ) -> Result<SubtreeListing> {
        if root_id != "root" && Some(root_id) != drive_id {
            let gone = || {
                DriveError::NotFound(
                    "The Drive folder is gone (deleted, in the bin, or no longer shared)".into(),
                )
            };
            match self.api.get_file(account, root_id).await {
                Ok(f) if f.trashed => return Err(gone()),
                Ok(_) => {}
                Err(DriveError::NotFound(_)) => return Err(gone()),
                Err(e) => return Err(e),
            }
        }
        let mut out = SubtreeListing::default();
        let mut level: Vec<(String, String)> = vec![(root_id.to_string(), String::new())];
        // A folder is read once: a cycle in what Drive returns can never loop the walk.
        let mut visited: HashSet<String> = HashSet::from([root_id.to_string()]);
        while !level.is_empty() {
            let mut next = Vec::new();
            for chunk in level.chunks(LIVE_PARALLEL) {
                let mut set = tokio::task::JoinSet::new();
                for (i, (folder, prefix)) in chunk.iter().enumerate() {
                    let api = self.api.clone();
                    let (account, folder, drive_id) =
                        (account.to_string(), folder.clone(), drive_id.map(str::to_string));
                    let prefix = prefix.clone();
                    set.spawn(async move {
                        let files = api.list_children(&account, &folder, drive_id.as_deref()).await;
                        (i, folder, prefix, files)
                    });
                }
                let mut results = Vec::with_capacity(chunk.len());
                while let Some(r) = set.join_next().await {
                    results.push(r.map_err(|e| DriveError::Unknown(e.to_string()))?);
                }
                // Folder order does not depend on which answer came first.
                results.sort_by_key(|(i, _, _, _)| *i);
                for (_, folder, prefix, files) in results {
                    self.pages_read.fetch_add(1, Ordering::Relaxed);
                    // Only what names this folder as its parent ("root" is My Drive's alias).
                    let nodes: Vec<DriveNode> = files?
                        .iter()
                        .filter(|f| !f.trashed)
                        .filter(|f| {
                            folder == "root" || f.parents.as_ref().is_none_or(|ps| ps.contains(&folder))
                        })
                        .map(|f| DriveNode::from_file(f, account))
                        .collect();
                    for (id, key) in take_children(nodes.iter().collect(), &prefix, &mut out) {
                        if deep && visited.insert(id.clone()) {
                            next.push((id, key));
                        }
                    }
                }
            }
            level = next;
        }
        Ok(out)
    }
}

fn walk(snap: &Snapshot, folder_id: &str, prefix: &str, out: &mut SubtreeListing) {
    let kids: Vec<&DriveNode> = snap.children_of(folder_id).iter().collect();
    for (id, key) in take_children(kids, prefix, out) {
        walk(snap, &id, &key, out);
    }
}

/// Adds one folder's children to `out`; returns the child folders to descend into, as
/// (id, key). Among same-named siblings (case-insensitive) the most recently modified wins and
/// the others are reported as duplicates; shortcuts are reported, not followed.
fn take_children(mut kids: Vec<&DriveNode>, prefix: &str, out: &mut SubtreeListing) -> Vec<(String, String)> {
    // Prefer the most recently modified among duplicates so the newest content wins.
    kids.sort_by_key(|n| std::cmp::Reverse(n.modified_time));
    let mut seen: HashSet<String> = HashSet::new();
    let mut folders = Vec::new();
    for n in kids {
        let key = if prefix.is_empty() {
            n.name.clone()
        } else {
            format!("{prefix}/{}", n.name)
        };
        if n.is_shortcut {
            out.shortcuts.push(key);
            continue;
        }
        if !seen.insert(wasl_core::identity::name_key(&n.name)) {
            out.duplicates.push(key);
            continue;
        }
        out.entries.push(RemoteEntry {
            key: key.clone(),
            id: n.id.clone(),
            name: n.name.clone(),
            is_dir: n.is_folder,
            size: n.size,
            md5: n.md5.clone(),
            mtime: n.modified_time,
            version: n.version,
            google_mime: (n.mime_type.starts_with(GOOGLE_APPS_PREFIX) && !n.is_folder)
                .then(|| n.mime_type.clone()),
            parent_id: n.parent_id.clone(),
            app_properties: n.app_properties.clone(),
        });
        if n.is_folder {
            folders.push((n.id.clone(), key));
        }
    }
    folders
}
