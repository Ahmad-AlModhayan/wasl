//! Real Cloud Files tests: each registers a sync root under a temporary folder, with a fake source
//! (generated content, counted requests), and reads placeholders through plain file I/O in
//! another process, which makes Windows call back for the data. Every root has a unique id and is
//! unregistered when its fixture drops, even when the test fails; roots left by a killed run are
//! removed by the next.
#![cfg(windows)]

use std::collections::HashMap;
use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use wasl_vfs::{BlockCache, FileRef, Hydrator, Node, ReadAhead, Source, Tree, VfsError};
use wasl_vfs_windows::{OnDemandRoot, RootConfig, is_sync_root, registered_sync_roots, unregister_sync_root};

const GIB: u64 = 1 << 30;
const MIB: u64 = 1 << 20;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// Content made up from the file's id and version, so 20 GiB need no memory.
fn content(f: &FileRef, offset: u64, len: u64) -> Vec<u8> {
    let seed =
        f.id.bytes()
            .chain(f.version.bytes())
            .fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
                (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
            });
    (offset..offset + len)
        .map(|i| {
            let x = ((i / 8) ^ seed).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            (x >> (8 * (i % 8))) as u8
        })
        .collect()
}

#[derive(Default)]
struct FakeSource {
    requests: AtomicU64,
    bytes: AtomicU64,
    /// Simulated link speed in Mbit/s (the acceptance's 100 Mbit); none: instant.
    mbit: Option<u64>,
    log: Mutex<Vec<(String, u64, u64)>>,
}

#[async_trait]
impl Source for FakeSource {
    async fn read(&self, f: &FileRef, offset: u64, len: u64) -> wasl_vfs::Result<Vec<u8>> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        lock(&self.log).push((f.id.clone(), offset, len));
        if f.id.starts_with("broken") {
            return Err(VfsError::Unavailable("the fake source refuses this file".into()));
        }
        let len = len.min(f.size.saturating_sub(offset));
        self.bytes.fetch_add(len, Ordering::SeqCst);
        if let Some(mbit) = self.mbit {
            let secs = (len * 8) as f64 / (mbit as f64 * 1e6);
            tokio::time::sleep(Duration::from_secs_f64(secs)).await;
        }
        Ok(content(f, offset, len))
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Registering or removing sync roots from several threads at once fails ("Incorrect function.",
/// then "Element not found."), so fixtures take turns for that, as wasld takes turns mounting
/// (`views.rs`). Roots stay alive side by side once registered.
static REGISTERING: Mutex<()> = Mutex::new(());

/// A registered root under a temporary folder; unregistered on drop, whatever happened.
struct Fixture {
    root: Option<OnDemandRoot>,
    id: String,
    path: PathBuf,
    source: Arc<FakeSource>,
    hydrator: Arc<Hydrator>,
    _tmp: tempfile::TempDir,
}

impl Fixture {
    fn new(name: &str, source: FakeSource) -> Self {
        Self::with(name, source, false)
    }

    fn with(name: &str, source: FakeSource, read_only: bool) -> Self {
        let registering = lock(&REGISTERING);
        remove_stale_test_roots();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("pair");
        let source = Arc::new(source);
        let cache = Arc::new(BlockCache::open(tmp.path().join("cache"), 64 * MIB).unwrap());
        let hydrator = Arc::new(Hydrator::new(source.clone(), cache, ReadAhead::default()));
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let config = RootConfig {
            root: path.clone(),
            pair_id: format!("test-{name}-{}-{nanos}", std::process::id()),
            display_name: format!("Wasl test {name}"),
            version: "1.0".into(),
            icon: None,
            block_self_hydration: true,
            read_only,
            set_aside: read_only.then(|| tmp.path().join("aside")),
        };
        let root = OnDemandRoot::register(config, hydrator.clone()).expect("register a sync root");
        drop(registering);
        Self {
            id: root.id().to_owned(),
            path: root.root().to_path_buf(),
            root: Some(root),
            source,
            hydrator,
            _tmp: tmp,
        }
    }

    fn root(&self) -> &OnDemandRoot {
        self.root.as_ref().unwrap()
    }

    fn requests(&self) -> u64 {
        self.source.requests.load(Ordering::SeqCst)
    }

    fn fetched(&self) -> u64 {
        self.source.bytes.load(Ordering::SeqCst)
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.path.join(rel.replace('/', "\\"))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Disconnect first, then unregister whatever the test did.
        let _registering = lock(&REGISTERING);
        drop(self.root.take());
        if registered_sync_roots().map_or(true, |v| v.iter().any(|(id, _)| *id == self.id)) {
            let _ = unregister_sync_root(&self.id);
        }
    }
}

/// Test roots of earlier runs whose folder is gone (a run killed before its fixtures dropped).
/// This run's own roots are never touched: another test may be registering one right now.
fn remove_stale_test_roots() {
    let mine = format!("-{}-", std::process::id());
    for (id, path) in registered_sync_roots().unwrap() {
        if id.contains("!test-") && !id.contains(&mine) && !path.exists() {
            let _ = unregister_sync_root(&id);
        }
    }
}

fn file(path: &str, id: &str, version: &str, size: u64, mtime_ms: i64) -> Node {
    Node {
        path: path.into(),
        is_dir: false,
        mtime_ms,
        file: Some(FileRef {
            id: id.into(),
            version: version.into(),
            size,
        }),
    }
}

fn fref(id: &str, version: &str, size: u64) -> FileRef {
    FileRef {
        id: id.into(),
        version: version.into(),
        size,
    }
}

/// Reads `path` whole in a child process (this test binary again, `child_reader`), through
/// std::fs: a real reader is another process (the provider's own reads are refused, see
/// `wasls_own_reads_do_not_download_whole_files`).
fn read_in_child(path: &Path) -> Result<Vec<u8>, String> {
    let out = tempfile::NamedTempFile::new().unwrap().into_temp_path();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_reader", "--nocapture", "--test-threads", "1"])
        .env("WASL_CHILD_READ", format!("{}|{}", path.display(), out.display()))
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    wait_for(&mut child);
    let status = fs::read_to_string(&out).expect("the reader wrote its status");
    let data = format!("{}.bin", out.display());
    let r = match status.split_once(' ') {
        Some(("err", msg)) => Err(msg.to_owned()),
        _ => Ok(fs::read(&data).unwrap()),
    };
    let _ = fs::remove_file(data);
    r
}

fn read_all(path: &Path) -> Vec<u8> {
    read_in_child(path).expect("the reader read the file")
}

fn wait_for(child: &mut std::process::Child) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("the reader did not finish in 60 s (a fetch hung?)");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Not a test: the reader process `read_in_child` starts (does nothing in a normal run).
#[test]
fn child_reader() {
    let Ok(spec) = std::env::var("WASL_CHILD_READ") else {
        return;
    };
    let (path, out) = spec.split_once('|').unwrap();
    let status = match fs::read(path) {
        Ok(data) => {
            fs::write(format!("{out}.bin"), data).unwrap();
            "ok".to_owned()
        }
        Err(e) => format!("err {e}"),
    };
    fs::write(out, status).unwrap();
}

/// Opens `path`, seeks to `offset` and reads `len` bytes in Windows PowerShell (.NET's
/// FileStream, unbuffered). Returns the bytes and how long open + seek + read took, timed in the
/// reader (PowerShell's own start-up is not counted).
///
/// Why PowerShell: on the machine these tests were written on, every other program tried (this
/// binary, a copy of it, Python, cmd, PowerShell 7, a .NET Framework program built for the test)
/// has the whole file requested while it is still opening it, before it reads anything, although
/// the sync root asks for partial hydration; Windows PowerShell gets exactly the ranges it reads.
/// The provider serves whatever range Windows asks for; see the crate's notes.
fn seek_read_in_powershell(path: &Path, offset: u64, len: usize) -> (Vec<u8>, Duration) {
    let ps = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())
        + r"\System32\WindowsPowerShell\v1.0\powershell.exe";
    let script = format!(
        "$sw = [Diagnostics.Stopwatch]::StartNew(); \
         $f = New-Object IO.FileStream('{}', 'Open', 'Read', 'ReadWrite', 1); \
         [void]$f.Seek({offset}, 'Begin'); $b = New-Object byte[] {len}; $n = 0; \
         while ($n -lt {len}) {{ $r = $f.Read($b, $n, {len} - $n); if ($r -le 0) {{ break }}; $n += $r }}; \
         $f.Close(); $sw.Stop(); \
         [Console]::Out.Write([string][int64]($sw.Elapsed.TotalMilliseconds * 1000) + ' ' + [Convert]::ToBase64String($b, 0, $n))",
        path.display().to_string().replace('\'', "''"),
    );
    let out_file = tempfile::NamedTempFile::new().unwrap();
    let mut child = Command::new(ps)
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdout(out_file.reopen().unwrap())
        .spawn()
        .unwrap();
    wait_for(&mut child);
    let out = fs::read_to_string(out_file.path()).unwrap();
    let (micros, b64) = out
        .trim()
        .split_once(' ')
        .unwrap_or_else(|| panic!("PowerShell said {out:?}"));
    (base64_decode(b64), Duration::from_micros(micros.parse().unwrap()))
}

fn base64_decode(s: &str) -> Vec<u8> {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes().filter(|&c| c != b'=') {
        acc = (acc << 6) | A.iter().position(|&a| a == c).unwrap() as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ok() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn attrib(flag: &str, p: &Path) {
    let out = Command::new("attrib").arg(flag).arg(p).output().unwrap();
    assert!(out.status.success(), "attrib {flag}: {out:?}");
}

const T0: i64 = 1_700_000_000_123;

#[test]
fn placeholders_read_in_full_with_sizes_and_times() {
    let fx = Fixture::new("full", FakeSource::default());
    let small = fref("small", "1", 10_000);
    let big = fref("big", "1", 3 * MIB + 12_345);
    let tree = Tree::new([
        file("docs/a.txt", "small", "1", small.size, T0),
        file("b.bin", "big", "1", big.size, T0 + 1000),
        file("empty", "empty", "1", 0, T0),
    ]);
    let r = fx.root().sync(&tree).unwrap();
    assert_eq!(4, r.created, "{r:?}");
    assert_eq!(0, fx.requests(), "creating placeholders fetches nothing");

    let m = fs::metadata(fx.p("b.bin")).unwrap();
    assert_eq!(big.size, m.len());
    let mtime = m.modified().unwrap().duration_since(UNIX_EPOCH).unwrap();
    assert_eq!(T0 + 1000, mtime.as_millis() as i64);
    assert!(fs::metadata(fx.p("docs")).unwrap().is_dir());
    assert_eq!(0, fx.root().state("b.bin").unwrap().unwrap().on_disk);

    let (a, b, e) = (
        read_all(&fx.p("docs/a.txt")),
        read_all(&fx.p("b.bin")),
        read_all(&fx.p("empty")),
    );
    assert_eq!(content(&small, 0, small.size), a);
    assert!(b == content(&big, 0, big.size), "b.bin's content differs");
    assert!(e.is_empty());
    assert!(fx.root().problems().is_empty(), "{:?}", fx.root().problems());
}

/// wasld keeps several on-demand views mounted at once: two live roots each serve their own files.
#[test]
fn two_roots_alive_at_once_each_serve_their_own_files() {
    let one = Fixture::new("two-a", FakeSource::default());
    let two = Fixture::new("two-b", FakeSource::default());
    let (a, b) = (fref("a", "1", 10_000), fref("b", "1", 20_000));
    one.root()
        .sync(&Tree::new([file("a.txt", "a", "1", a.size, T0)]))
        .unwrap();
    two.root()
        .sync(&Tree::new([file("b.txt", "b", "1", b.size, T0)]))
        .unwrap();
    assert_eq!(content(&b, 0, b.size), read_all(&two.p("b.txt")));
    assert_eq!(content(&a, 0, a.size), read_all(&one.p("a.txt")));
    assert!(one.root().problems().is_empty() && two.root().problems().is_empty());
}

#[test]
fn a_seek_in_20_gib_fetches_one_block_in_under_2_s() {
    // The acceptance's link: 100 Mbit/s, so a 1 MiB block takes ~84 ms.
    let fx = Fixture::new(
        "seek",
        FakeSource {
            mbit: Some(100),
            ..Default::default()
        },
    );
    let huge = fref("huge", "1", 20 * GIB);
    fx.root()
        .sync(&Tree::new([file("movie.mkv", "huge", "1", huge.size, T0)]))
        .unwrap();
    assert_eq!(20 * GIB, fs::metadata(fx.p("movie.mkv")).unwrap().len());

    let (buf, took) = seek_read_in_powershell(&fx.p("movie.mkv"), 15 * GIB, 100);
    eprintln!(
        "open + seek + read 100 B at 15 GiB of 20 GiB (100 Mbit/s): {took:?}, {} source \
         request(s), {} bytes fetched",
        fx.requests(),
        fx.fetched()
    );
    assert_eq!(content(&huge, 15 * GIB, 100), buf);
    assert!(took < Duration::from_secs(2), "took {took:?}");
    assert!(fx.fetched() <= 2 * MIB, "fetched {} bytes", fx.fetched());
    assert!(fx.requests() <= 2, "{:?}", lock(&fx.source.log));

    let before = fx.requests();
    let (again, took2) = seek_read_in_powershell(&fx.p("movie.mkv"), 15 * GIB, 100);
    eprintln!("the same range again: {took2:?}");
    assert_eq!(buf, again);
    // The range read is on disk: nothing asks the source for it again. (Windows' own read-ahead
    // can ask for the next pages, and the hydrator then reads ahead for what looks like a
    // sequential reader; those are later blocks.)
    let log = lock(&fx.source.log).clone();
    eprintln!("source requests: {log:?}");
    assert!(
        log[before as usize..]
            .iter()
            .all(|(_, off, len)| off + len <= 15 * GIB || *off >= 15 * GIB + 100),
        "a second read of the same range reached the source: {log:?}"
    );
    let st = fx.root().state("movie.mkv").unwrap().unwrap();
    assert!(
        st.on_disk < 2 * MIB,
        "only the range read is on disk: {}",
        st.on_disk
    );
}

#[test]
fn pin_hydrates_and_dehydrate_frees() {
    let fx = Fixture::new("pin", FakeSource::default());
    let f = fref("pinme", "1", 5 * MIB + 7);
    fx.root()
        .sync(&Tree::new([file("dir/p.bin", "pinme", "1", f.size, T0)]))
        .unwrap();

    fx.root().pin("dir/p.bin").unwrap();
    let st = fx.root().state("dir/p.bin").unwrap().unwrap();
    assert!(st.pinned);
    assert_eq!(f.size, st.on_disk, "pinned means fully hydrated");
    assert!(fx.hydrator.cache().is_pinned("pinme"));
    assert!(fx.hydrator.complete(&f));
    let n = fx.requests();
    assert!(read_all(&fx.p("dir/p.bin")) == content(&f, 0, f.size));
    assert_eq!(n, fx.requests(), "a pinned file is read from the disk");

    fx.root().dehydrate("dir/p.bin").unwrap();
    let st = fx.root().state("dir/p.bin").unwrap().unwrap();
    assert!(!st.pinned);
    assert_eq!(0, st.on_disk);
    assert!(!fx.hydrator.cache().is_pinned("pinme"));
    // Still readable on demand.
    assert!(read_all(&fx.p("dir/p.bin")) == content(&f, 0, f.size));
}

/// Security review 2026-10: locking a vault's view drops what was read on demand, so no decrypted
/// content stays in the folder; pinned files are the documented exception.
#[test]
fn dehydrate_unpinned_keeps_only_pinned_content() {
    let fx = Fixture::new("lockdrop", FakeSource::default());
    let read = fref("read", "1", 3 * MIB + 5);
    let kept = fref("kept", "1", MIB + 3);
    fx.root()
        .sync(&Tree::new([
            file("dir/read.bin", "read", "1", read.size, T0),
            file("kept.bin", "kept", "1", kept.size, T0),
        ]))
        .unwrap();
    assert!(read_all(&fx.p("dir/read.bin")) == content(&read, 0, read.size));
    fx.root().pin("kept.bin").unwrap();
    assert!(fx.root().state("dir/read.bin").unwrap().unwrap().on_disk > 0);
    // A file of the person's own in the folder is not Wasl's to touch.
    fs::write(fx.p("mine.txt"), "mine").unwrap();

    fx.root().dehydrate_unpinned().unwrap();
    assert_eq!(0, fx.root().state("dir/read.bin").unwrap().unwrap().on_disk);
    let k = fx.root().state("kept.bin").unwrap().unwrap();
    assert!(k.pinned);
    assert_eq!(kept.size, k.on_disk);
    assert_eq!("mine", fs::read_to_string(fx.p("mine.txt")).unwrap());
}

/// Security review 2026-10: a placeholder whose identity names a file outside the folder's tree
/// (made by another program) is not served.
#[test]
fn a_placeholder_for_a_file_outside_the_tree_is_not_fetched() {
    let fx = Fixture::new("foreign", FakeSource::default());
    let a = fref("a", "1", 10_000);
    fx.root()
        .sync(&Tree::new([file("a.txt", "a", "1", a.size, T0)]))
        .unwrap();
    // A placeholder with an identity of its own: a regular file turned into one, then emptied.
    let other = fref("elsewhere-on-drive", "1", 4096);
    fs::write(fx.p("planted.bin"), vec![0u8; 4096]).unwrap();
    fx.root().mark_in_sync("planted.bin", &other).unwrap();
    fx.root().dehydrate("planted.bin").unwrap();
    assert_eq!(0, fx.root().state("planted.bin").unwrap().unwrap().on_disk);

    let before = fx.requests();
    assert!(read_in_child(&fx.p("planted.bin")).is_err());
    assert_eq!(before, fx.requests(), "nothing was asked of the source");
    assert!(
        fx.root()
            .problems()
            .iter()
            .any(|p| p.contains("not a file of this folder's listing")),
        "{:?}",
        fx.root().problems()
    );
    // The tree's own files still read.
    assert_eq!(content(&a, 0, a.size), read_all(&fx.p("a.txt")));
}

#[test]
fn explorer_pin_states_are_acted_on() {
    let fx = Fixture::new("explorer", FakeSource::default());
    let f = fref("ex", "1", 2 * MIB + 5);
    fx.root()
        .sync(&Tree::new([file("e.bin", "ex", "1", f.size, T0)]))
        .unwrap();
    // `attrib +P` / `+U` set the same pin states as Explorer's "Always keep on this device" and
    // "Free up space".
    attrib("+P", &fx.p("e.bin"));
    wait_until("the pinned file to be hydrated", || {
        fx.root().state("e.bin").unwrap().unwrap().on_disk == f.size
    });
    assert!(fx.hydrator.cache().is_pinned("ex"));

    attrib("-P", &fx.p("e.bin"));
    attrib("+U", &fx.p("e.bin"));
    wait_until("the unpinned file to be dehydrated", || {
        fx.root().state("e.bin").unwrap().unwrap().on_disk == 0
    });
    wait_until("the cache pin to end", || !fx.hydrator.cache().is_pinned("ex"));
    assert!(fx.root().problems().is_empty(), "{:?}", fx.root().problems());
}

#[test]
fn tree_sync_adds_removes_and_updates() {
    let fx = Fixture::new("tree", FakeSource::default());
    let one = Tree::new([
        file("a.txt", "a", "1", 100, T0),
        file("d/b.txt", "b", "1", 200, T0),
        file("d/e/c.txt", "c", "1", 300, T0),
        file("gone.txt", "g", "1", 10, T0),
    ]);
    let r = fx.root().sync(&one).unwrap();
    assert_eq!(6, r.created, "{r:?}");
    // Read a.txt so it is hydrated before it changes.
    read_all(&fx.p("a.txt"));
    // A file made here, not in the listing: left alone.
    fs::write(fx.p("local.txt"), b"mine").unwrap();

    let a2 = fref("a", "2", 150);
    let two = Tree::new([
        file("a.txt", "a", "2", a2.size, T0 + 5000),
        file("d/b.txt", "b", "1", 200, T0),
        file("new/n.txt", "n", "1", 50, T0),
    ]);
    let r = fx.root().sync(&two).unwrap();
    assert_eq!(1, r.updated, "{r:?}");
    assert_eq!(2, r.created, "{r:?}"); // new/ and new/n.txt
    assert_eq!(3, r.removed, "{r:?}"); // d/e/c.txt, d/e, gone.txt
    assert_eq!(
        vec![wasl_vfs::Kept::new(
            "local.txt",
            wasl_vfs::KeptKind::OnlyHere,
            "only here: not in the listing"
        )],
        r.kept
    );
    assert!(!fx.p("d/e").exists());
    assert!(!fx.p("gone.txt").exists());
    assert!(fx.p("new/n.txt").exists());
    assert_eq!(b"mine".to_vec(), fs::read(fx.p("local.txt")).unwrap());

    let st = fx.root().state("a.txt").unwrap().unwrap();
    assert_eq!(Some(a2.clone()), st.file);
    assert_eq!(0, st.on_disk, "a changed file's old content is dropped");
    let m = fs::metadata(fx.p("a.txt")).unwrap();
    assert_eq!(a2.size, m.len());
    assert_eq!(content(&a2, 0, a2.size), read_all(&fx.p("a.txt")));

    // Nothing to do the second time.
    let r = fx.root().sync(&two).unwrap();
    assert_eq!((0, 0, 0), (r.created, r.updated, r.removed), "{r:?}");
}

#[test]
fn a_failing_source_is_an_error_not_a_hang() {
    let fx = Fixture::new("fail", FakeSource::default());
    fx.root()
        .sync(&Tree::new([file("x.bin", "broken-1", "1", 3 * MIB, T0)]))
        .unwrap();
    let t = Instant::now();
    let r = read_in_child(&fx.p("x.bin"));
    let took = t.elapsed();
    eprintln!("a failed read returned in {took:?} (with the reader's start-up): {r:?}");
    assert!(r.is_err());
    assert!(took < Duration::from_secs(10));
    assert!(!fx.root().problems().is_empty());
}

#[test]
fn wasls_own_reads_do_not_download_whole_files() {
    let fx = Fixture::new("self", FakeSource::default());
    let f = fref("own", "1", 4 * MIB);
    fx.root()
        .sync(&Tree::new([file("own.bin", "own", "1", f.size, T0)]))
        .unwrap();
    // Windows would hydrate the whole file for the provider's own read: refused instead.
    let t = Instant::now();
    assert!(fs::read(fx.p("own.bin")).is_err());
    assert!(t.elapsed() < Duration::from_secs(5));
    assert_eq!(0, fx.requests());
    // Explicit hydration (pinning) still works.
    fx.root().pin("own.bin").unwrap();
    assert!(fs::read(fx.p("own.bin")).unwrap() == content(&f, 0, f.size));
}

#[test]
fn unregister_leaves_no_sync_root() {
    let mut fx = Fixture::new("unreg", FakeSource::default());
    let id = fx.id.clone();
    assert!(registered_sync_roots().unwrap().iter().any(|(i, _)| *i == id));
    assert!(is_sync_root(&fx.path));
    fx.root.take().unwrap().unregister().unwrap();
    assert!(!registered_sync_roots().unwrap().iter().any(|(i, _)| *i == id));
    assert!(!is_sync_root(&fx.path));
}

#[test]
fn hydrate_all_then_unregister_leaves_regular_files() {
    let mut fx = Fixture::new("off", FakeSource::default());
    let files: HashMap<&str, FileRef> = HashMap::from([
        ("one.bin", fref("o1", "1", 3 * MIB + 3)),
        ("sub/two.txt", fref("o2", "1", 1234)),
    ]);
    fx.root()
        .sync(&Tree::new(
            files.iter().map(|(p, f)| file(p, &f.id, &f.version, f.size, T0)),
        ))
        .unwrap();
    let id = fx.id.clone();
    fx.root.take().unwrap().hydrate_all_and_unregister().unwrap();
    assert!(!registered_sync_roots().unwrap().iter().any(|(i, _)| *i == id));
    assert!(!is_sync_root(&fx.path));
    let before = fx.requests();
    for (p, f) in &files {
        let path = fx.p(p);
        let m = fs::symlink_metadata(&path).unwrap();
        assert_eq!(
            0,
            m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT,
            "{p} is regular"
        );
        assert!(fs::read(&path).unwrap() == content(f, 0, f.size), "{p}'s content");
        let mtime = m.modified().unwrap().duration_since(UNIX_EPOCH).unwrap();
        assert_eq!(T0, mtime.as_millis() as i64);
    }
    let sub = fs::symlink_metadata(fx.p("sub")).unwrap();
    assert_eq!(0, sub.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT);
    assert_eq!(before, fx.requests());
}

#[test]
fn a_read_only_root_serves_read_only_placeholders() {
    let mut fx = Fixture::with("ro", FakeSource::default(), true);
    let a = fref("ra", "1", 2 * MIB + 9);
    let b = fref("rb", "1", 4321);
    fx.root()
        .sync(&Tree::new([
            file("a.bin", "ra", "1", a.size, T0),
            file("d/b.txt", "rb", "1", b.size, T0),
        ]))
        .unwrap();
    for p in ["a.bin", "d/b.txt"] {
        assert!(fs::metadata(fx.p(p)).unwrap().permissions().readonly(), "{p}");
        assert!(
            fs::OpenOptions::new().write(true).open(fx.p(p)).is_err(),
            "{p} cannot be opened for writing"
        );
    }
    assert!(!fs::metadata(fx.p("d")).unwrap().permissions().readonly());
    // Reading, pinning and freeing up space work as for any placeholder.
    assert!(read_all(&fx.p("a.bin")) == content(&a, 0, a.size));
    fx.root().pin("d/b.txt").unwrap();
    assert_eq!(b.size, fx.root().state("d/b.txt").unwrap().unwrap().on_disk);
    fx.root().dehydrate("d/b.txt").unwrap();
    assert_eq!(0, fx.root().state("d/b.txt").unwrap().unwrap().on_disk);

    // A new version on Drive updates the placeholder, which stays read-only.
    let a2 = fref("ra", "2", 3 * MIB);
    let r = fx
        .root()
        .sync(&Tree::new([
            file("a.bin", "ra", "2", a2.size, T0 + 1000),
            file("d/b.txt", "rb", "1", b.size, T0),
        ]))
        .unwrap();
    assert_eq!(1, r.updated, "{r:?}");
    assert!(r.kept.is_empty(), "{r:?}");
    assert!(fs::metadata(fx.p("a.bin")).unwrap().permissions().readonly());
    assert!(read_all(&fx.p("a.bin")) == content(&a2, 0, a2.size));

    // Keeping the files: regular, writable files again.
    fx.root.take().unwrap().hydrate_all_and_unregister().unwrap();
    for p in ["a.bin", "d/b.txt"] {
        let m = fs::symlink_metadata(fx.p(p)).unwrap();
        assert_eq!(
            0,
            m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT,
            "{p} is regular"
        );
        assert!(!m.permissions().readonly(), "{p} is writable");
    }
    assert!(fs::read(fx.p("a.bin")).unwrap() == content(&a2, 0, a2.size));
}

/// Security review S3-2: a read-only root's files are read-only by attribute only. One changed
/// anyway (attribute cleared, content written) gets Drive's version back at the next sync, and
/// the changed copy is set aside first; a file whose attribute alone changed needs no copy.
#[test]
fn a_changed_file_in_a_read_only_root_gets_drives_version_back() {
    let fx = Fixture::with("restore", FakeSource::default(), true);
    let a = fref("ca", "1", 300_000);
    let b = fref("cb", "1", 4096);
    let tree = Tree::new([
        file("d/a.bin", "ca", "1", a.size, T0),
        file("b.bin", "cb", "1", b.size, T0),
    ]);
    fx.root().sync(&tree).unwrap();
    assert!(read_all(&fx.p("d/a.bin")) == content(&a, 0, a.size));

    // Changed here: the attribute cleared, then new content written.
    attrib("-r", &fx.p("d/a.bin"));
    fs::write(fx.p("d/a.bin"), b"mine now").unwrap();
    assert!(!fx.root().state("d/a.bin").unwrap().unwrap().in_sync);
    // Only the attribute: nothing written, so nothing to keep.
    attrib("-r", &fx.p("b.bin"));

    let r = fx.root().sync(&tree).unwrap();
    assert!(r.kept.is_empty(), "{r:?}");
    assert_eq!(1, r.restored.len(), "{r:?}");
    let (path, copy) = &r.restored[0];
    assert_eq!("d/a.bin", path);
    assert_eq!(
        b"mine now".to_vec(),
        fs::read(copy).unwrap(),
        "the changed copy is kept"
    );
    assert!(!copy.starts_with(&fx.path), "outside the view");
    let st = fx.root().state("d/a.bin").unwrap().unwrap();
    assert!(st.in_sync);
    assert_eq!(0, st.on_disk, "Drive's version is read again on demand");
    assert_eq!(a.size, fs::metadata(fx.p("d/a.bin")).unwrap().len());
    assert!(fs::metadata(fx.p("d/a.bin")).unwrap().permissions().readonly());
    assert!(read_all(&fx.p("d/a.bin")) == content(&a, 0, a.size));
    assert!(fs::metadata(fx.p("b.bin")).unwrap().permissions().readonly());

    // Nothing left to put back.
    let r = fx.root().sync(&tree).unwrap();
    assert!(r.restored.is_empty() && r.kept.is_empty(), "{r:?}");
}

/// E2E 2026-10: a vault's view, locked with a file changed in it, kept that file's plaintext in
/// the folder (only in-sync placeholders were freed). With no listing at hand, the changed copy
/// goes aside and the placeholder is freed like the others.
#[test]
fn dehydrate_unpinned_sets_a_changed_file_aside_in_a_read_only_root() {
    let fx = Fixture::with("lockchanged", FakeSource::default(), true);
    let a = fref("la", "1", 300_000);
    let b = fref("lb", "1", 4096);
    fx.root()
        .sync(&Tree::new([
            file("d/a.bin", "la", "1", a.size, T0),
            file("b.bin", "lb", "1", b.size, T0),
        ]))
        .unwrap();
    assert!(read_all(&fx.p("d/a.bin")) == content(&a, 0, a.size));
    attrib("-r", &fx.p("d/a.bin"));
    fs::write(fx.p("d/a.bin"), b"my secret edit").unwrap();
    assert!(!fx.root().state("d/a.bin").unwrap().unwrap().in_sync);

    fx.root().dehydrate_unpinned().unwrap();
    let st = fx.root().state("d/a.bin").unwrap().unwrap();
    assert!(st.in_sync);
    assert_eq!(0, st.on_disk, "nothing written here stays in the folder");
    assert!(fs::metadata(fx.p("d/a.bin")).unwrap().permissions().readonly());
    let aside = fx._tmp.path().join("aside");
    let copies: Vec<_> = walkdir(&aside);
    assert_eq!(1, copies.len(), "{copies:?}");
    assert!(copies[0].ends_with("d\\a.bin"), "{copies:?}");
    assert_eq!(b"my secret edit".to_vec(), fs::read(&copies[0]).unwrap());
    // Drive's version is read again on demand.
    assert!(read_all(&fx.p("d/a.bin")) == content(&a, 0, a.size));
}

fn walkdir(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walkdir(&p));
        } else {
            out.push(p);
        }
    }
    out
}

/// A file still open for writing is not restored: bytes written after the changed copy was set
/// aside and before Drive's version came back would be lost. It is reported and restored at a
/// later sync, once the writer has closed it, with everything it wrote in the copy.
#[test]
fn a_changed_file_still_open_for_writing_is_restored_only_once_closed() {
    use std::io::Write as _;
    let fx = Fixture::with("restore-open", FakeSource::default(), true);
    let a = fref("oa", "1", 300_000);
    let tree = Tree::new([file("a.bin", "oa", "1", a.size, T0)]);
    fx.root().sync(&tree).unwrap();
    assert!(read_all(&fx.p("a.bin")) == content(&a, 0, a.size));
    attrib("-r", &fx.p("a.bin"));
    let mut writer = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(fx.p("a.bin"))
        .unwrap();
    writer.write_all(b"first part").unwrap();
    writer.flush().unwrap();

    let r = fx.root().sync(&tree).unwrap();
    assert!(r.restored.is_empty(), "{r:?}");
    assert_eq!(1, r.kept.len(), "{r:?}");
    assert_eq!(wasl_vfs::KeptKind::Failed, r.kept[0].kind, "{r:?}");
    // The writer goes on; nothing it wrote is gone.
    writer.write_all(b", then the rest").unwrap();
    drop(writer);
    assert_eq!(
        b"first part, then the rest".to_vec(),
        fs::read(fx.p("a.bin")).unwrap()
    );

    let r = fx.root().sync(&tree).unwrap();
    assert!(r.kept.is_empty(), "{r:?}");
    assert_eq!(1, r.restored.len(), "{r:?}");
    assert_eq!(
        b"first part, then the rest".to_vec(),
        fs::read(&r.restored[0].1).unwrap()
    );
    assert_eq!(0, fx.root().state("a.bin").unwrap().unwrap().on_disk);
}

/// Diagnostic, run by hand: which program makes Windows fetch a whole placeholder when another
/// program opens it and reads 16 bytes. Prints every fetch with the process Windows names for it.
/// `cargo test -p wasl-vfs-windows --test cloud_files -- --ignored --nocapture who_fetches`
/// (`WASL_WHO_ONLY=<part of a label>` runs some readers only).
///
/// Found on the development PC (2026-10-22, Defender real-time protection on): every reader but
/// Windows PowerShell has the whole file asked for, in its own name, as soon as it opens the file
/// for reading, before reading anything; opening for attributes only asks for nothing. No sync
/// root setting changes it (partial/progressive, every modifier) and neither does the offline
/// attribute. Windows PowerShell gets exactly the 4 KiB blocks it reads. An on-access scan in the
/// opener's context fits; confirming it needs a Defender exclusion for the test folder.
#[test]
#[ignore = "diagnostic: prints who asks for which ranges"]
fn who_fetches_when_another_program_reads_16_bytes() {
    let fx = Fixture::new("who", FakeSource::default());
    let winps = format!(
        r"{}\System32\WindowsPowerShell\v1.0\powershell.exe",
        std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())
    );
    let ps = |p: &str| vec![p.to_owned(), "-NoProfile".into(), "-Command".into()];
    let py = || vec!["python".to_owned(), "-I".into(), "-c".into()];
    // (label, command, script with {path})
    let ctypes = "import ctypes as c; k=c.windll.kernel32; k.CreateFileW.restype=c.c_void_p; \
                  h=k.CreateFileW(r'{path}',0x80000000,7,None,3,FLAGS,None); b=c.create_string_buffer(16); \
                  n=c.c_ulong(); k.ReadFile(c.c_void_p(h),b,16,c.byref(n),None); k.CloseHandle(c.c_void_p(h))";
    let readers: Vec<(&str, Vec<String>, String)> = vec![
        ("Windows PowerShell OpenRead", ps(&winps), "$f=[IO.File]::OpenRead('{path}'); $b=New-Object byte[] 16; [void]$f.Read($b,0,16); $f.Close()".into()),
        ("PowerShell 7 OpenRead", ps("pwsh"), "$f=[IO.File]::OpenRead('{path}'); $b=New-Object byte[] 16; [void]$f.Read($b,0,16); $f.Close()".into()),
        ("PowerShell 7 FileStream buffer 1", ps("pwsh"), "$f=New-Object IO.FileStream('{path}','Open','Read','ReadWrite',1); $b=New-Object byte[] 16; [void]$f.Read($b,0,16); $f.Close()".into()),
        ("PowerShell 7 RandomAccess", ps("pwsh"), "$f=New-Object IO.FileStream('{path}','Open','Read','ReadWrite',4096,[IO.FileOptions]::RandomAccess); $b=New-Object byte[] 16; [void]$f.Read($b,0,16); $f.Close()".into()),
        ("Python open rb", py(), "f=open(r'{path}','rb'); f.read(16); f.close()".into()),
        ("Python unbuffered", py(), "f=open(r'{path}','rb',buffering=0); f.read(16); f.close()".into()),
        ("Python os.open O_RANDOM", py(), "import os; fd=os.open(r'{path}', os.O_RDONLY|os.O_BINARY|os.O_RANDOM); os.read(fd,16); os.close(fd)".into()),
        ("Python CreateFileW normal", py(), ctypes.replace("FLAGS", "0x80")),
        ("Python CreateFileW random", py(), ctypes.replace("FLAGS", "0x10000000")),
        ("Python CreateFileW sequential", py(), ctypes.replace("FLAGS", "0x08000000")),
        // Open for reading, wait, close without reading: is the whole file asked for on open?
        (
            "Python open, no read",
            py(),
            "import ctypes as c, time; k=c.windll.kernel32; k.CreateFileW.restype=c.c_void_p; \
             h=k.CreateFileW(r'{path}',0x80000000,7,None,3,0x80,None); time.sleep(2); k.CloseHandle(c.c_void_p(h))"
                .into(),
        ),
        (
            "Python open attributes only",
            py(),
            "import ctypes as c, time; k=c.windll.kernel32; k.CreateFileW.restype=c.c_void_p; \
             h=k.CreateFileW(r'{path}',0x80,7,None,3,0x80,None); time.sleep(2); k.CloseHandle(c.c_void_p(h))"
                .into(),
        ),
    ];
    let only = std::env::var("WASL_WHO_ONLY").ok();
    let mut nodes: Vec<Node> = Vec::new();
    for (i, (name, cmd, script)) in readers.into_iter().enumerate() {
        if only.as_deref().is_some_and(|o| !name.contains(o)) {
            continue;
        }
        let rel = format!("big{i}.bin");
        nodes.push(file(&rel, &format!("big{i}"), "1", 64 * MIB, T0));
        fx.root().sync(&Tree::new(nodes.clone())).unwrap();
        let path = fx.p(&rel).display().to_string();
        let before = fx.root().recent_fetches().len();
        let status = Command::new(&cmd[0])
            .args(&cmd[1..])
            .arg(script.replace("{path}", &path))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        std::thread::sleep(Duration::from_secs(3));
        let fetches = fx.root().recent_fetches();
        let fetched: u64 = fetches.iter().skip(before).map(|r| r.len).sum();
        println!(
            "--- {name}: {:?}, {} bytes asked for",
            status.map(|s| s.code()),
            fetched
        );
        for r in fetches.iter().skip(before).take(4) {
            println!(
                "    {:<24} pid {:<6} {:>10} + {:>10}",
                r.process, r.process_id, r.offset, r.len
            );
        }
    }
}
