//! A real FUSE mount (Linux): files read with std::fs come through the hydrator from a fake
//! source. Skipped with a note where /dev/fuse (or the fusermount3 helper) is missing, unless
//! `WASL_FUSE_TESTS=require` (CI), where a missing FUSE fails the test instead.
#![cfg(target_os = "linux")]

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use async_trait::async_trait;
use tokio::runtime::Runtime;
use wasl_vfs::{BLOCK, BlockCache, FileRef, Hydrator, Node, ReadAhead, Result, Source, Tree};
use wasl_vfs_fuse::{MountOptions, OnDemandMount, PINNED_XATTR};

const EROFS: i32 = nix::errno::Errno::EROFS as i32;

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
        self.calls.lock().unwrap().push((f.id.clone(), offset, len));
        let end = (offset + len).min(f.size);
        Ok((offset..end).map(|i| byte(&f.version, i)).collect())
    }
}

impl Fake {
    fn calls_for(&self, id: &str) -> Vec<(u64, u64)> {
        let calls = self.calls.lock().unwrap();
        calls.iter().filter(|c| c.0 == id).map(|c| (c.1, c.2)).collect()
    }
}

const MTIME_MS: i64 = 1_700_000_000_000;

fn file(path: &str, version: &str, size: u64) -> Node {
    Node {
        path: path.into(),
        is_dir: false,
        mtime_ms: MTIME_MS,
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

/// Dropped in field order: the mount before the folders it lives in and the runtime its reads
/// run on.
struct Mounted {
    mount: Option<OnDemandMount>,
    fake: Arc<Fake>,
    dir: tempfile::TempDir,
    rt: Runtime,
}

impl Mounted {
    fn at(&self, rel: &str) -> PathBuf {
        self.dir.path().join("mnt").join(rel)
    }

    fn mount(&self) -> &OnDemandMount {
        self.mount.as_ref().unwrap()
    }
}

fn required() -> bool {
    std::env::var("WASL_FUSE_TESTS").is_ok_and(|v| v == "require")
}

fn skip(why: &str) -> Option<Mounted> {
    assert!(!required(), "FUSE is required here: {why}");
    eprintln!("note: skipping the FUSE mount test: {why}");
    None
}

fn mounted(nodes: Vec<Node>) -> Option<Mounted> {
    if !Path::new("/dev/fuse").exists() {
        return skip("/dev/fuse is not available");
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("mnt")).unwrap();
    let cache = Arc::new(BlockCache::open(dir.path().join("cache"), 1 << 30).unwrap());
    let fake = Arc::new(Fake::default());
    let h = Arc::new(Hydrator::new(fake.clone(), cache, ReadAhead::default()));
    let mount = match OnDemandMount::mount(
        dir.path().join("mnt"),
        Tree::new(nodes),
        h,
        rt.handle().clone(),
        &MountOptions::default(),
    ) {
        Ok(m) => m,
        Err(e) => return skip(&format!("the mount failed: {e}")),
    };
    Some(Mounted {
        mount: Some(mount),
        fake,
        dir,
        rt,
    })
}

#[test]
fn files_read_whole_with_their_attributes() {
    let Some(m) = mounted(vec![
        file("docs/a.txt", "1", 1000),
        file("docs/big.bin", "1", 3 * BLOCK + 17),
    ]) else {
        return;
    };
    assert_eq!(expected("1", 0, 1000), fs::read(m.at("docs/a.txt")).unwrap());
    assert_eq!(
        expected("1", 0, 3 * BLOCK + 17),
        fs::read(m.at("docs/big.bin")).unwrap()
    );
    let meta = fs::metadata(m.at("docs/big.bin")).unwrap();
    assert!(meta.is_file());
    assert_eq!(3 * BLOCK + 17, meta.len());
    assert_eq!(0o644, meta.permissions().mode() & 0o7777);
    assert_eq!(
        UNIX_EPOCH + Duration::from_millis(MTIME_MS as u64),
        meta.modified().unwrap()
    );
    let me = fs::metadata(m.dir.path()).unwrap();
    assert_eq!((me.uid(), me.gid()), (meta.uid(), meta.gid()));
    let docs = fs::metadata(m.at("docs")).unwrap();
    assert!(docs.is_dir());
    assert_eq!(0o755, docs.permissions().mode() & 0o7777);
}

#[test]
fn a_seek_fetches_about_one_block() {
    let size = 64 * BLOCK;
    let Some(m) = mounted(vec![file("huge.bin", "1", size)]) else {
        return;
    };
    let at = 40 * BLOCK + 7;
    let mut f = fs::File::open(m.at("huge.bin")).unwrap();
    f.seek(SeekFrom::Start(at)).unwrap();
    let mut buf = vec![0u8; 100];
    f.read_exact(&mut buf).unwrap();
    assert_eq!(expected("1", at, at + 100), buf);
    let calls = m.fake.calls_for("id:huge.bin");
    let fetched: u64 = calls.iter().map(|c| c.1).sum();
    // The block holding the range. The kernel reads ahead a little and, seeing that run, the
    // hydrator adds its window (4 blocks): a handful of the 64, never the file.
    assert!(calls.iter().all(|c| c.0 >= 40 * BLOCK), "{calls:?}");
    assert!(
        (BLOCK..=8 * BLOCK).contains(&fetched),
        "fetched {fetched} bytes: {calls:?}"
    );
}

#[test]
fn folders_list_their_entries() {
    let Some(m) = mounted(vec![
        file("b.txt", "1", 1),
        file("a/one", "1", 1),
        file("a/deeper/two", "1", 1),
        file("c d.txt", "1", 1),
    ]) else {
        return;
    };
    let names = |rel: &str| -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(m.at(rel))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        v.sort();
        v
    };
    assert_eq!(vec!["a", "b.txt", "c d.txt"], names(""));
    assert_eq!(vec!["deeper", "one"], names("a"));
    assert_eq!(vec!["two"], names("a/deeper"));
    assert!(fs::metadata(m.at("nope")).is_err());
}

#[test]
fn the_pinned_attribute_reads_and_follows_pins() {
    let Some(m) = mounted(vec![file("p.bin", "1", 2 * BLOCK)]) else {
        return;
    };
    let path = m.at("p.bin");
    assert_eq!(Some(b"0".to_vec()), xattr::get(&path, PINNED_XATTR).unwrap());
    let names: Vec<_> = xattr::list(&path).unwrap().collect();
    assert!(names.iter().any(|n| n == PINNED_XATTR), "{names:?}");
    m.rt.block_on(m.mount().pin("p.bin")).unwrap();
    assert!(m.mount().is_pinned("p.bin"));
    assert_eq!(Some(b"1".to_vec()), xattr::get(&path, PINNED_XATTR).unwrap());
    m.mount().unpin("p.bin").unwrap();
    assert_eq!(Some(b"0".to_vec()), xattr::get(&path, PINNED_XATTR).unwrap());
    assert!(xattr::get(&path, "user.other").unwrap().is_none());
}

#[test]
fn writes_fail_with_erofs() {
    let Some(m) = mounted(vec![file("r.txt", "1", 10), file("d/x", "1", 1)]) else {
        return;
    };
    let code = |r: std::io::Result<()>| r.unwrap_err().raw_os_error();
    let rofs = Some(EROFS);
    assert_eq!(
        rofs,
        code(fs::OpenOptions::new().write(true).open(m.at("r.txt")).map(drop))
    );
    assert_eq!(rofs, code(fs::write(m.at("new.txt"), b"x")));
    assert_eq!(rofs, code(fs::create_dir(m.at("newdir"))));
    assert_eq!(rofs, code(fs::remove_file(m.at("r.txt"))));
    assert_eq!(rofs, code(fs::rename(m.at("r.txt"), m.at("s.txt"))));
    assert_eq!(rofs, code(xattr::set(m.at("r.txt"), PINNED_XATTR, b"1")));
    // Still readable after all that.
    assert_eq!(expected("1", 0, 10), fs::read(m.at("r.txt")).unwrap());
}

#[test]
fn a_refresh_swaps_the_content_and_open_files_keep_theirs() {
    let size = 2 * BLOCK;
    let Some(m) = mounted(vec![
        file("doc.bin", "v1", size),
        file("grows.txt", "1", 10),
        file("gone.txt", "1", 5),
    ]) else {
        return;
    };
    assert_eq!(expected("v1", 0, size), fs::read(m.at("doc.bin")).unwrap());
    assert_eq!(10, fs::metadata(m.at("grows.txt")).unwrap().len());
    assert!(fs::metadata(m.at("gone.txt")).is_ok());

    // Open before the refresh and read the first half.
    let mut open = fs::File::open(m.at("doc.bin")).unwrap();
    let mut first = vec![0u8; BLOCK as usize];
    open.read_exact(&mut first).unwrap();

    // "v2-new" differs from "v1" in length, so every byte differs (same size, new content).
    m.mount().refresh(Tree::new(vec![
        file("doc.bin", "v2-new", size),
        file("grows.txt", "2", 5000),
        file("added.txt", "1", 3),
    ]));

    // The handle opened before still reads v1 to its end.
    let mut second = Vec::new();
    open.read_to_end(&mut second).unwrap();
    first.extend(second);
    assert_eq!(expected("v1", 0, size), first);
    drop(open);

    // A new open reads v2; sizes, additions and removals show at once.
    assert_eq!(expected("v2-new", 0, size), fs::read(m.at("doc.bin")).unwrap());
    assert_eq!(5000, fs::metadata(m.at("grows.txt")).unwrap().len());
    assert_eq!(expected("2", 0, 5000), fs::read(m.at("grows.txt")).unwrap());
    assert_eq!(expected("1", 0, 3), fs::read(m.at("added.txt")).unwrap());
    assert!(fs::metadata(m.at("gone.txt")).is_err());
}

#[test]
fn dropping_the_mount_unmounts_it() {
    let Some(mut m) = mounted(vec![file("x.txt", "1", 4)]) else {
        return;
    };
    assert_eq!(1, fs::read_dir(m.at("")).unwrap().count());
    m.mount.take().unwrap().unmount().unwrap();
    // The folder underneath is empty again.
    assert_eq!(0, fs::read_dir(m.at("")).unwrap().count());
    let me = fs::metadata(m.dir.path()).unwrap();
    assert_eq!(me.dev(), fs::metadata(m.at("")).unwrap().dev());
}
