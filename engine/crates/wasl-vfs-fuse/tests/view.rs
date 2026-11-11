//! What the mount serves, without FUSE (every OS): lookups, listings, reads through the hydrator,
//! open files keeping their version across a refresh, the pin attribute, and the refresh's
//! invalidations.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use wasl_vfs::{BLOCK, BlockCache, FileRef, Hydrator, Node, ReadAhead, Result, Source, Tree};
use wasl_vfs_fuse::{Change, FsError, Kind, PINNED_XATTR, ROOT, View, XattrReply};

/// Content is a function of (version, offset); every request is recorded.
#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<(String, u64, u64)>>,
}

fn byte(version: &str, at: u64) -> u8 {
    (at.wrapping_mul(31) ^ version.len() as u64 ^ (at >> 20)) as u8
}

#[async_trait]
impl Source for Fake {
    async fn read(&self, f: &FileRef, offset: u64, len: u64) -> Result<Vec<u8>> {
        self.calls.lock().unwrap().push((f.version.clone(), offset, len));
        let end = (offset + len).min(f.size);
        Ok((offset..end).map(|i| byte(&f.version, i)).collect())
    }
}

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

fn expected(version: &str, from: u64, to: u64) -> Vec<u8> {
    (from..to).map(|i| byte(version, i)).collect()
}

fn setup(nodes: Vec<Node>) -> (View, Arc<Fake>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let cache = Arc::new(BlockCache::open(dir.path().join("cache"), 1 << 30).unwrap());
    let fake = Arc::new(Fake::default());
    let h = Arc::new(Hydrator::new(fake.clone(), cache, ReadAhead::default()));
    (View::new(Tree::new(nodes), h), fake, dir)
}

#[test]
fn lookups_and_listings_follow_the_tree() {
    let (v, _f, _d) = setup(vec![
        file("docs/a.txt", "1", 10),
        file("docs/sub/b.txt", "1", 20),
        file("top.bin", "1", 3 * BLOCK),
    ]);
    let docs = v.lookup(ROOT, "docs").unwrap();
    assert_eq!(Kind::Dir, docs.kind);
    let a = v.lookup(docs.ino, "a.txt").unwrap();
    assert_eq!((Kind::File, 10), (a.kind, a.size));
    assert_eq!(a, v.getattr(a.ino).unwrap());
    assert_eq!(Err(FsError::NotFound), v.lookup(ROOT, "nope"));
    assert_eq!(Err(FsError::NotFound), v.lookup(ROOT, ".."));
    assert_eq!(Err(FsError::NotDir), v.lookup(a.ino, "x"));
    assert_eq!(Err(FsError::NotFound), v.lookup(4242, "x"));

    let all = v.readdir(ROOT, 0).unwrap();
    let names: Vec<&str> = all.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(vec![".", "..", "docs", "top.bin"], names);
    assert_eq!(vec![1, 2, 3, 4], all.iter().map(|e| e.offset).collect::<Vec<_>>());
    // The kernel continues from the offset of the last entry it took.
    let rest = v.readdir(ROOT, 3).unwrap();
    assert_eq!(1, rest.len());
    assert_eq!("top.bin", rest[0].name);
    assert!(v.readdir(ROOT, 99).unwrap().is_empty());
    let sub = v.readdir(docs.ino, 0).unwrap();
    assert_eq!(ROOT, sub[1].ino, "`..` of docs is the root");
    assert_eq!(
        vec![("a.txt", Kind::File), ("sub", Kind::Dir)],
        sub[2..]
            .iter()
            .map(|e| (e.name.as_str(), e.kind))
            .collect::<Vec<_>>()
    );
    assert_eq!(Err(FsError::NotDir), v.readdir(a.ino, 0));
    assert_eq!(Err(FsError::NotFound), v.readdir(4242, 0));
}

#[tokio::test]
async fn reads_go_through_the_hydrator_and_writes_are_refused() {
    let (v, fake, _d) = setup(vec![file("big.bin", "1", 64 * BLOCK + 7)]);
    let ino = v.ino("big.bin").unwrap();
    assert_eq!(Err(FsError::ReadOnly), v.open(ino, true));
    assert_eq!(Err(FsError::IsDir), v.open(ROOT, false));
    let fh = v.open(ino, false).unwrap();
    // A seek far into the file fetches about one block, not the file.
    let at = 40 * BLOCK + 100;
    assert_eq!(expected("1", at, at + 4096), v.read(fh, at, 4096).await.unwrap());
    let fetched: u64 = fake.calls.lock().unwrap().iter().map(|c| c.2).sum();
    assert_eq!(BLOCK, fetched);
    // Past the end: nothing, not an error.
    assert!(v.read(fh, 64 * BLOCK + 7, 10).await.unwrap().is_empty());
    assert_eq!(
        expected("1", 64 * BLOCK, 64 * BLOCK + 7),
        v.read(fh, 64 * BLOCK, 4096).await.unwrap()
    );
    v.release(fh);
    assert_eq!(Err(FsError::BadHandle), v.read(fh, 0, 1).await);
    assert_eq!(0, v.open_count());
}

#[tokio::test]
async fn open_files_keep_their_version_across_a_refresh() {
    let (v, _f, _d) = setup(vec![file("doc.txt", "v1", 100), file("gone.txt", "1", 5)]);
    let ino = v.ino("doc.txt").unwrap();
    let gone = v.ino("gone.txt").unwrap();
    let old = v.open(ino, false).unwrap();
    let changes = v.refresh(Tree::new(vec![
        file("doc.txt", "v2-longer", 200),
        file("new.txt", "1", 1),
    ]));
    assert_eq!(Some(ino), v.ino("doc.txt"), "the path keeps its inode");
    assert!(changes.contains(&Change::Inode(ino)));
    assert!(changes.contains(&Change::Inode(gone)));
    assert!(changes.contains(&Change::Entry {
        parent: ROOT,
        name: "gone.txt".into()
    }));
    assert!(changes.contains(&Change::Entry {
        parent: ROOT,
        name: "new.txt".into()
    }));
    assert_eq!(200, v.getattr(ino).unwrap().size);
    assert_eq!(Err(FsError::NotFound), v.getattr(gone));
    // The handle opened before reads v1; a new open reads v2.
    assert_eq!(expected("v1", 0, 100), v.read(old, 0, 1000).await.unwrap());
    let new = v.open(ino, false).unwrap();
    assert_eq!(expected("v2-longer", 0, 200), v.read(new, 0, 1000).await.unwrap());
    // Refreshing with the same tree changes nothing.
    let same = Tree::new(vec![file("doc.txt", "v2-longer", 200), file("new.txt", "1", 1)]);
    assert!(v.refresh(same).is_empty());
}

#[tokio::test]
async fn the_pin_attribute_follows_pins() {
    let (v, fake, _d) = setup(vec![file("p/one.bin", "1", 3 * BLOCK), file("p/two.txt", "1", 9)]);
    let one = v.ino("p/one.bin").unwrap();
    let p = v.ino("p").unwrap();
    assert_eq!(b"0".to_vec(), v.getxattr(one, PINNED_XATTR).unwrap());
    assert_eq!(Err(FsError::NoAttr), v.getxattr(one, "user.other"));
    assert_eq!(Err(FsError::NoAttr), v.getxattr(p, PINNED_XATTR));
    assert_eq!(
        format!("{PINNED_XATTR}\0").into_bytes(),
        v.listxattr(one).unwrap()
    );
    assert!(v.listxattr(p).unwrap().is_empty());

    // Pinning a folder pins and fetches every file in it.
    v.pin("p").await.unwrap();
    assert!(v.is_pinned("p/one.bin") && v.is_pinned("p/two.txt"));
    assert_eq!(b"1".to_vec(), v.getxattr(one, PINNED_XATTR).unwrap());
    let fetched: u64 = fake.calls.lock().unwrap().iter().map(|c| c.2).sum();
    assert_eq!(3 * BLOCK + 9, fetched);
    assert!(v.hydrator().complete(&FileRef {
        id: "id:p/one.bin".into(),
        version: "1".into(),
        size: 3 * BLOCK,
    }));
    v.unpin("p/one.bin").unwrap();
    assert!(!v.is_pinned("p/one.bin") && v.is_pinned("p/two.txt"));
    assert!(v.pin("missing").await.is_err());
}

#[test]
fn xattr_buffers_are_sized_like_the_syscall() {
    assert_eq!(Ok(XattrReply::Size(1)), XattrReply::for_buffer(0, b"1".to_vec()));
    assert_eq!(
        Ok(XattrReply::Data(b"1".to_vec())),
        XattrReply::for_buffer(64, b"1".to_vec())
    );
    assert_eq!(Err(FsError::Range), XattrReply::for_buffer(1, b"12".to_vec()));
}
