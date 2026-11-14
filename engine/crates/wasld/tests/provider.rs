//! What the macOS File Provider extension reads from wasld (docs/ON_DEMAND.md §2.6): items,
//! change sets and content of the mounted views, on a socket that answers nothing else; on macOS,
//! views as File Provider domains driven through a stand-in for the helper.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use wasl_ipc::{ErrorKind, ProviderChanges, ProviderContent, ProviderItem, ProviderListing, Request};
use wasl_vfs::{BlockCache, FileRef, Hydrator, Node, ReadAhead, Source, Tree};
use wasld::provider::{ProviderHost, ProviderIpc};

/// File contents by id, counting reads.
#[derive(Default)]
struct Bytes {
    files: HashMap<String, Vec<u8>>,
    reads: AtomicUsize,
}

#[async_trait::async_trait]
impl Source for Bytes {
    async fn read(&self, file: &FileRef, offset: u64, len: u64) -> wasl_vfs::Result<Vec<u8>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let b = &self.files[&file.id];
        let end = (offset + len).min(b.len() as u64);
        Ok(b[offset as usize..end as usize].to_vec())
    }
}

fn file(path: &str, content: &[u8], version: &str) -> Node {
    Node {
        path: path.into(),
        is_dir: false,
        mtime_ms: 1_700_000_000_000,
        file: Some(FileRef {
            id: format!("id-{path}"),
            version: version.into(),
            size: content.len() as u64,
        }),
    }
}

struct Setup {
    host: Arc<ProviderHost>,
    hydrator: Arc<Hydrator>,
    source: Arc<Bytes>,
    _dir: tempfile::TempDir,
}

const BIG: usize = 3 * (1 << 20) + 5;

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let big: Vec<u8> = (0..BIG).map(|i| (i % 251) as u8).collect();
    let mut source = Bytes::default();
    source.files.insert("id-a.txt".into(), b"hello".to_vec());
    source.files.insert("id-docs/big.bin".into(), big);
    let source = Arc::new(source);
    let cache = BlockCache::open(dir.path().join("cache"), 64 << 20).unwrap();
    let hydrator = Arc::new(Hydrator::new(
        source.clone(),
        Arc::new(cache),
        ReadAhead::default(),
    ));
    let host = Arc::new(ProviderHost::new(dir.path().join("group")));
    host.show("v1", tree(&[]), hydrator.clone());
    Setup {
        host,
        hydrator,
        source,
        _dir: dir,
    }
}

/// The view's files: `a.txt`, `docs/big.bin`, and `extra` more under `docs/`.
fn tree(extra: &[&str]) -> Tree {
    let mut nodes = vec![
        file("a.txt", b"hello", "1"),
        file("docs/big.bin", &vec![0; BIG], "7"),
    ];
    nodes.extend(extra.iter().map(|p| file(p, b"x", "1")));
    Tree::new(nodes)
}

fn ids(items: &[ProviderItem]) -> Vec<&str> {
    let mut v: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
    v.sort();
    v
}

#[test]
fn items_are_the_views_paths() {
    let s = setup();
    let root = s.host.item("v1", "").unwrap();
    assert!(root.is_dir && root.id.is_empty());

    let top = s.host.list("v1", "").unwrap();
    assert_eq!(vec!["a.txt", "docs"], ids(&top.items));
    let a = top.items.iter().find(|i| i.id == "a.txt").unwrap();
    assert_eq!(
        (false, 5, "1", "", "a.txt", false),
        (
            a.is_dir,
            a.size,
            a.version.as_str(),
            a.parent.as_str(),
            a.name.as_str(),
            a.pinned
        )
    );
    let docs = s.host.list("v1", "docs").unwrap();
    assert_eq!(vec!["docs/big.bin"], ids(&docs.items));
    assert_eq!(
        ("docs", "big.bin"),
        (docs.items[0].parent.as_str(), docs.items[0].name.as_str())
    );
    assert_eq!(top.anchor, docs.anchor);

    let all = s.host.all("v1").unwrap();
    assert_eq!(vec!["a.txt", "docs", "docs/big.bin"], ids(&all.items));

    assert_eq!(ErrorKind::NotFound, s.host.item("v1", "nope").unwrap_err().kind);
    assert_eq!(
        ErrorKind::NotFound,
        s.host.list("v1", "a.txt").unwrap_err().kind,
        "a file has no children"
    );
    // A view that isn't showing (the daemon lost it, its vault is locked) says so.
    assert_eq!(ErrorKind::Locked, s.host.list("other", "").unwrap_err().kind);
    s.host.hide("v1");
    assert_eq!(ErrorKind::Locked, s.host.item("v1", "a.txt").unwrap_err().kind);
}

#[test]
fn change_sets_follow_refreshes() {
    let s = setup();
    let first = s.host.list("v1", "").unwrap().anchor;
    let none = s.host.changes("v1", &first).unwrap();
    assert!(!none.expired && none.updated.is_empty() && none.deleted.is_empty());
    assert_eq!(first, none.anchor);

    assert!(!s.host.update("v1", tree(&[])), "the same tree is no change");
    // Drive: a.txt changed, docs/new.txt is new, docs/big.bin went.
    let next = Tree::new([file("a.txt", b"hello!", "2"), file("docs/new.txt", b"x", "1")]);
    assert!(s.host.update("v1", next));
    let c: ProviderChanges = s.host.changes("v1", &first).unwrap();
    assert!(!c.expired);
    assert_ne!(first, c.anchor);
    assert_eq!(vec!["", "a.txt", "docs/new.txt"], ids(&c.updated));
    assert_eq!(vec!["docs/big.bin".to_owned()], c.deleted);
    assert_eq!("2", c.updated.iter().find(|i| i.id == "a.txt").unwrap().version);

    // From the new anchor: nothing. From further back, or another run: list again.
    assert!(s.host.changes("v1", &c.anchor).unwrap().updated.is_empty());
    assert!(s.host.update("v1", tree(&[])));
    assert!(s.host.changes("v1", &first).unwrap().expired);
    let other_run = ProviderHost::new("/nowhere");
    other_run.show("v1", tree(&[]), s.hydrator.clone());
    let theirs = other_run.list("v1", "").unwrap().anchor;
    assert!(s.host.changes("v1", &theirs).unwrap().expired);

    // A pin changes what the items say, not the tree: everything is read again.
    let now = s.host.list("v1", "").unwrap().anchor;
    s.host.touch("v1");
    assert!(s.host.changes("v1", &now).unwrap().expired);
}

#[tokio::test]
async fn fetching_writes_the_content_to_a_file_of_its_own() {
    let s = setup();
    let whole: ProviderContent = s.host.fetch("v1", "a.txt", None, None).await.unwrap();
    assert_eq!(b"hello".to_vec(), std::fs::read(&whole.path).unwrap());
    assert_eq!(
        (0, 5, "1"),
        (whole.offset, whole.len, whole.item.version.as_str())
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&whole.path).unwrap().permissions().mode();
        assert_eq!(0o600, mode & 0o777, "this user's only");
    }

    // A range from the middle of a large file: only its blocks are fetched.
    let at = (1 << 20) + 10;
    let part = s
        .host
        .fetch("v1", "docs/big.bin", Some(at), Some(100))
        .await
        .unwrap();
    let got = std::fs::read(&part.path).unwrap();
    assert_eq!((at, 100), (part.offset, part.len));
    // At its own offset in the file, as the system reads it.
    assert_eq!(at as usize + 100, got.len());
    assert_eq!(
        (at..at + 100).map(|i| (i % 251) as u8).collect::<Vec<_>>(),
        got[at as usize..]
    );
    assert_ne!(whole.path, part.path);
    assert!(!s.hydrator.complete(&s.host_file("docs/big.bin")));

    // Past the end: clamped to the file.
    let tail = s.host.fetch("v1", "a.txt", Some(3), Some(100)).await.unwrap();
    assert_eq!((3, 2), (tail.offset, tail.len));
    assert_eq!(b"lo".to_vec(), std::fs::read(&tail.path).unwrap()[3..]);

    // The whole large file, then again from the cache.
    let all = s.host.fetch("v1", "docs/big.bin", None, None).await.unwrap();
    assert_eq!(BIG as u64, std::fs::metadata(&all.path).unwrap().len());
    let reads = s.source.reads.load(Ordering::SeqCst);
    s.host.fetch("v1", "docs/big.bin", None, None).await.unwrap();
    assert_eq!(
        reads,
        s.source.reads.load(Ordering::SeqCst),
        "served from the cache"
    );

    assert_eq!(
        ErrorKind::NotFound,
        s.host.fetch("v1", "docs", None, None).await.unwrap_err().kind
    );
    assert_eq!(
        ErrorKind::NotFound,
        s.host.fetch("v1", "gone", None, None).await.unwrap_err().kind
    );
}

#[tokio::test]
async fn content_left_behind_is_cleared() {
    let s = setup();
    s.host.show("v10", tree(&[]), s.hydrator.clone());
    let mine = s.host.fetch("v1", "a.txt", None, None).await.unwrap();
    let theirs = s.host.fetch("v10", "a.txt", None, None).await.unwrap();
    // The system took neither (a crash): the view's vault is locked, or the view removed.
    s.host.forget_content("v1");
    assert!(!std::path::Path::new(&mine.path).exists());
    assert!(
        std::path::Path::new(&theirs.path).exists(),
        "another view's stays"
    );
    // wasld starts again: nothing is waiting to be taken.
    s.host.forget_all_content();
    assert!(!std::path::Path::new(&theirs.path).exists());
}

#[tokio::test]
async fn pinned_files_say_so() {
    let s = setup();
    s.hydrator.pin(&s.host_file("a.txt")).await.unwrap();
    assert!(s.host.item("v1", "a.txt").unwrap().pinned);
    assert!(!s.host.item("v1", "docs/big.bin").unwrap().pinned);
}

impl Setup {
    fn host_file(&self, path: &str) -> FileRef {
        tree(&[]).get(path).unwrap().file.clone().unwrap()
    }
}

#[tokio::test]
async fn the_socket_answers_file_provider_requests_only() {
    let s = setup();
    let ipc = ProviderIpc(s.host.clone());
    use wasl_ipc::Handler as _;
    let listing: ProviderListing = serde_json::from_value(
        ipc.handle(Request::ProviderList {
            view: "v1".into(),
            parent: Some(String::new()),
        })
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(vec!["a.txt", "docs"], ids(&listing.items));
    let all: ProviderListing = serde_json::from_value(
        ipc.handle(Request::ProviderList {
            view: "v1".into(),
            parent: None,
        })
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(3, all.items.len());
    for refused in [
        Request::Views,
        Request::PinPath { path: "/x".into() },
        Request::LockVault { id: "v".into() },
        Request::Shutdown,
    ] {
        assert_eq!(ErrorKind::Permission, ipc.handle(refused).await.unwrap_err().kind);
    }
}

/// The extension's view of the wire: a real socket and handshake, as the Swift client speaks it.
#[cfg(unix)]
#[tokio::test]
async fn the_extension_reads_over_its_socket() {
    let s = setup();
    let paths = s.host.paths();
    let server = wasl_ipc::Server::bind(&paths, Arc::new(ProviderIpc(s.host.clone())), "test").unwrap();
    let serving = tokio::spawn(server.serve());
    let client = wasl_ipc::Client::connect(&paths).await.unwrap();
    let item: ProviderItem = client
        .call(Request::ProviderItem {
            view: "v1".into(),
            id: "docs/big.bin".into(),
        })
        .await
        .unwrap();
    assert_eq!(BIG as u64, item.size);
    let content: ProviderContent = client
        .call(Request::ProviderFetch {
            view: "v1".into(),
            id: "a.txt".into(),
            offset: None,
            len: None,
        })
        .await
        .unwrap();
    assert_eq!(b"hello".to_vec(), std::fs::read(&content.path).unwrap());
    assert!(client.call::<serde_json::Value>(Request::Views).await.is_err());
    serving.abort();
}

/// Views as File Provider domains, with a shell script standing in for `wasl-fileprovider`: it
/// records its calls and says where the system keeps each domain's folder.
#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::time::Duration;
    use wasl_engine::MemoryRemote;
    use wasl_ipc::{NewView, RemoteError, View};
    use wasl_store::{SharedStore, Store};
    use wasld::views::macos::FileProvider;
    use wasld::views::{Backend, ViewRemote, ViewRemotes, Views};

    struct Drive(Arc<MemoryRemote>);

    impl ViewRemotes for Drive {
        fn open(&self, _view: &View) -> Result<ViewRemote, RemoteError> {
            Ok(ViewRemote::plain(self.0.clone()))
        }
    }

    /// A helper that logs `args` to `calls`, keeps domains as folders under `cloud/`.
    fn fake_helper(dir: &Path) -> PathBuf {
        let ctl = dir.join("wasl-fileprovider");
        let script = format!(
            r#"#!/bin/sh
d='{d}'
echo "$*" >> "$d/calls"
case "$1" in
  container) echo "$d/group" ;;
  add) mkdir -p "$d/cloud/Wasl-$5" && touch "$d/domains/$3" && echo "$d/cloud/Wasl-$5" ;;
  remove) rm -f "$d/domains/$3" ;;
  list) ls "$d/domains" ;;
  evict) cat > "$d/evicted" ;;
  signal) ;;
  *) echo "unknown $1" >&2; exit 2 ;;
esac
"#,
            d = dir.display()
        );
        std::fs::create_dir_all(dir.join("domains")).unwrap();
        std::fs::write(&ctl, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ctl, std::fs::Permissions::from_mode(0o755)).unwrap();
        ctl
    }

    fn calls(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[tokio::test]
    async fn a_view_is_a_domain_its_folder_links_to() {
        let dir = tempfile::tempdir().unwrap();
        let ctl = fake_helper(dir.path());
        let backend = Arc::new(FileProvider::with_helper(ctl).expect("the helper answers"));
        let host = backend.provider().unwrap();
        assert_eq!(dir.path().join("group"), host.paths().data);

        let store = SharedStore::new(Store::open(dir.path().join("wasl.db")).unwrap());
        let drive = Arc::new(MemoryRemote::new());
        let folder = drive.mkdirs("Photos");
        drive.put_file("Photos/a.jpg", b"aaaa");
        drive.put_file("Photos/trip/b.jpg", b"bb");
        let views = Views::new(
            store,
            &dir.path().join("data"),
            Arc::new(Drive(drive.clone())),
            backend.clone(),
            Duration::from_secs(300),
            Arc::new(|| {}),
        );
        assert!(views.provider().is_some());
        let local = dir.path().join("Views/Photos");
        let view = views
            .add(
                NewView {
                    name: "Photos".into(),
                    account_id: "me@example.com".into(),
                    drive_folder_id: folder,
                    drive_id: None,
                    remote_path: "My Drive/Photos".into(),
                    local_path: local.display().to_string(),
                    cache_limit: Some(64 << 20),
                },
                &[],
            )
            .await
            .unwrap();
        assert_eq!(None, view.last_error);
        let id = view.id.clone();
        assert_eq!(
            vec![
                "container".to_owned(),
                format!("add --id {id} --name Photos"),
                format!("signal --id {id}")
            ],
            calls(dir.path())
        );
        // The view's folder is a link to the domain's; the extension sees the files.
        let root = dir.path().join("cloud/Wasl-Photos");
        assert_eq!(root, std::fs::read_link(&local).unwrap());
        assert_eq!(vec!["a.jpg", "trip"], ids(&host.list(&id, "").unwrap().items));
        let got = host.fetch(&id, "trip/b.jpg", None, None).await.unwrap();
        assert_eq!(b"bb".to_vec(), std::fs::read(got.path).unwrap());

        // A change on Drive: the system is told; no change: it is not.
        let before = calls(dir.path()).len();
        views.refresh_now(&id).await.unwrap();
        assert_eq!(before, calls(dir.path()).len());
        drive.put_file("Photos/c.jpg", b"c");
        views.refresh_now(&id).await.unwrap();
        assert_eq!(Some(&format!("signal --id {id}")), calls(dir.path()).last());
        assert!(host.item(&id, "c.jpg").is_ok());

        // Pinned by a path through the link, or in the domain's folder.
        views
            .set_pinned(&local.join("a.jpg").display().to_string(), true)
            .await
            .unwrap();
        assert!(host.item(&id, "a.jpg").unwrap().pinned);
        std::fs::create_dir_all(root.join("trip")).unwrap();
        std::fs::write(root.join("trip/b.jpg"), b"").unwrap();
        views
            .set_pinned(&root.join("trip/b.jpg").display().to_string(), true)
            .await
            .unwrap();
        assert!(host.item(&id, "trip/b.jpg").unwrap().pinned);

        // Removed: the domain goes, and the link with it.
        views.remove(&id, false).await.unwrap();
        assert_eq!(Some(&format!("remove --id {id}")), calls(dir.path()).last());
        assert!(std::fs::symlink_metadata(&local).is_err());
        assert_eq!(ErrorKind::Locked, host.list(&id, "").unwrap_err().kind);
    }

    #[tokio::test]
    async fn a_folder_with_files_is_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let backend = FileProvider::with_helper(fake_helper(dir.path())).unwrap();
        let local = dir.path().join("busy");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(local.join("mine.txt"), b"keep").unwrap();
        let view = View {
            id: "v".into(),
            name: "Busy".into(),
            account_id: "a".into(),
            drive_folder_id: "f".into(),
            drive_id: None,
            remote_path: "r".into(),
            local_path: local.display().to_string(),
            vault_id: None,
            cache_limit: 1 << 20,
            enabled: true,
            created_at: 0,
            last_refresh_at: None,
            last_error: None,
        };
        let cache = BlockCache::open(dir.path().join("cache"), 1 << 20).unwrap();
        let hydrator = Arc::new(Hydrator::new(
            Arc::new(Bytes::default()),
            Arc::new(cache),
            ReadAhead::default(),
        ));
        let e = match backend.mount(&view, Tree::default(), hydrator, dir.path()).await {
            Err(e) => e,
            Ok(_) => panic!("mounted over a folder with files"),
        };
        assert!(e.contains("isn't empty"), "{e}");
        assert_eq!(b"keep".to_vec(), std::fs::read(local.join("mine.txt")).unwrap());
        assert!(!backend.provider().unwrap().showing("v"));
    }
}
