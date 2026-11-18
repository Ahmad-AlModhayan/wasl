//! Device sync end to end on real folders: scans, pulls between devices (in process and over
//! QUIC), chunked deltas, moves, deletions, conflicts and a local edit racing a pull.

mod common;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use wasl_core::Route;
use wasl_engine::fault::{FaultPlan, Point};
use wasl_engine::{Clock, DeviceService, DeviceSync, MAX_LOCAL_VERSIONS, PullReport};
use wasl_fs::{LocalFs, StdFs};
use wasl_net::{DeviceId, DeviceKey, Endpoint, LocalLink, PeerLink, PeerService, ShareId, Trust};
use wasl_store::{Pair, SharedStore, Store};

const SHARE: ShareId = ShareId([7; 16]);
const MB: usize = 1 << 20;

struct TestClock(AtomicI64);

impl Clock for TestClock {
    fn now_ms(&self) -> i64 {
        self.0.fetch_add(1000, Ordering::SeqCst)
    }
}

/// How a test device's folder treats names.
enum Disk {
    Plain,
    Windows,
    Refusing,
}

struct Device {
    key: DeviceKey,
    root: PathBuf,
    store: SharedStore,
    sync: DeviceSync,
    service: Arc<DeviceService>,
    fs: Arc<dyn LocalFs>,
    pair: Pair,
    clock: Arc<TestClock>,
    _dir: tempfile::TempDir,
}

impl Device {
    fn new(n: u8) -> Device {
        Device::with_folding(n, None)
    }

    /// A device whose folder folds case and accents, as Windows, macOS and Android do.
    fn folding(n: u8) -> Device {
        Device::with_folding(n, Some(common::Folding { android: true }))
    }

    /// A device whose folder refuses names as Windows does (the host must create the awkward names).
    fn windows(n: u8) -> Device {
        Device::build(n, None, Disk::Windows)
    }

    /// A device whose disk enforces Windows' rules but tells Wasl it takes any name, so a name
    /// the mapper leaves alone is refused by the disk (what a mapper that missed a rule would see).
    fn refusing(n: u8) -> Device {
        Device::build(n, None, Disk::Refusing)
    }

    fn with_folding(n: u8, folding: Option<common::Folding>) -> Device {
        Device::build(n, folding, Disk::Plain)
    }

    fn build(n: u8, folding: Option<common::Folding>, disk: Disk) -> Device {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("folder");
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let store = SharedStore::new(Store::open_in_memory().unwrap());
        let mut pair = Pair::new("p", "Shared", root.to_str().unwrap());
        pair.route = Route::Devices;
        store.lock().create_pair(&pair).unwrap();
        store.lock().set_share_id("p", &SHARE.to_hex()).unwrap();
        // The test devices (seeds 1 to 9) know each other (docs/INTRODUCTIONS.md §3.1).
        for seed in 1..=9u8 {
            let id = DeviceKey::from_seed(&[seed; 32]).id().to_string();
            store.lock().set_device_known(&id, true).unwrap();
        }
        let fs: Arc<dyn LocalFs> = match folding {
            Some(f) => common::folded_fs(&root, &bin, f),
            None => match disk {
                Disk::Windows => common::windows_names_fs(Arc::new(StdFs::new(&root, &bin))),
                Disk::Refusing => common::refusing_windows_names_fs(Arc::new(StdFs::new(&root, &bin))),
                Disk::Plain => Arc::new(StdFs::new(&root, &bin)),
            },
        };
        let key = DeviceKey::from_seed(&[n; 32]);
        let clock = Arc::new(TestClock(AtomicI64::new(1_800_000_000_000)));
        let sync = DeviceSync::new(
            store.clone(),
            fs.clone(),
            pair.clone(),
            SHARE,
            key.id(),
            clock.clone(),
        );
        let open = fs.clone();
        let service = Arc::new(DeviceService::new(
            store.clone(),
            move |_| Ok(open.clone()),
            |_, _, _| {},
        ));
        Device {
            key,
            root,
            store,
            sync,
            service,
            fs,
            pair,
            clock,
            _dir: dir,
        }
    }

    /// The process dies and starts again: a new `DeviceSync` over the same store and folder,
    /// stopping where `fault` says (none: runs to the end).
    fn restart(&mut self, fault: Option<Arc<FaultPlan>>) {
        let mut sync = DeviceSync::new(
            self.store.clone(),
            self.fs.clone(),
            self.pair.clone(),
            SHARE,
            self.key.id(),
            self.clock.clone(),
        );
        if let Some(f) = fault {
            sync = sync.with_fault(f);
        }
        self.sync = sync;
    }

    fn id(&self) -> DeviceId {
        self.key.id()
    }

    fn write(&self, rel: &str, bytes: &[u8]) {
        let p = self.root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    fn link_to(&self, other: &Device) -> LocalLink {
        LocalLink::new(
            self.id(),
            other.id(),
            other.service.clone() as Arc<dyn PeerService>,
        )
    }

    async fn pull_from(&self, other: &Device) -> PullReport {
        self.sync.scan().await.unwrap();
        self.sync.pull(&self.link_to(other)).await.unwrap()
    }

    /// Every file under the folder, by relative path.
    fn files(&self) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        walk(&self.root, &self.root, &mut out);
        out
    }

    fn bin_count(&self) -> usize {
        self.store.lock().bin_entries(Some("p")).unwrap().len()
    }
}

fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let e = e.unwrap();
        let p = e.path();
        if e.file_type().unwrap().is_dir() {
            walk(root, &p, out);
        } else {
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            out.insert(rel, std::fs::read(&p).unwrap());
        }
    }
}

/// Bytes that chunk well (no long runs).
fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_device_receives_everything_with_times_kept() {
    let (a, b) = (Device::new(1), Device::new(2));
    a.write("notes.txt", b"hello");
    a.write("مستندات/تقرير.txt", "نص عربي".as_bytes());
    a.write("deep/er/still/x.bin", &noise(3 * MB, 1));
    std::fs::create_dir_all(a.root.join("empty")).unwrap();
    let mtime = filetime::FileTime::from_unix_time(1_700_000_000, 0);
    filetime::set_file_mtime(a.root.join("notes.txt"), mtime).unwrap();
    a.sync.scan().await.unwrap();

    let r = b.pull_from(&a).await;
    assert_eq!(0, r.deferred);
    assert_eq!(a.files(), b.files());
    assert!(b.root.join("empty").is_dir());
    let got = filetime::FileTime::from_last_modification_time(
        &std::fs::metadata(b.root.join("notes.txt")).unwrap(),
    );
    assert_eq!(mtime.unix_seconds(), got.unix_seconds());

    // Nothing more to do the second time, either way.
    let again = b.pull_from(&a).await;
    assert_eq!((0, 0), (again.taken, again.bytes_fetched as usize));
    let back = a.pull_from(&b).await;
    assert_eq!(0, back.bytes_fetched);
    assert_eq!(a.files(), b.files());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_edit_in_a_large_file_sends_only_the_changed_chunks() {
    let (a, b) = (Device::new(1), Device::new(2));
    let mut big = noise(48 * MB, 2);
    a.write("video.mov", &big);
    a.sync.scan().await.unwrap();
    let first = b.pull_from(&a).await;
    assert_eq!(48 * MB as u64, first.bytes_fetched);

    for byte in &mut big[20 * MB..20 * MB + 4096] {
        *byte ^= 0xff;
    }
    a.write("video.mov", &big);
    a.sync.scan().await.unwrap();
    let edit = b.pull_from(&a).await;
    assert_eq!(a.files(), b.files());
    assert!(edit.bytes_fetched > 0);
    assert!(
        edit.bytes_fetched <= 2 * 16 * MB as u64,
        "fetched {} bytes for a 4 KiB edit",
        edit.bytes_fetched
    );
    assert!(
        edit.bytes_local >= 16 * MB as u64,
        "the rest came from the old file"
    );
    assert_eq!(1, b.bin_count(), "the old version is in the bin");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_moved_file_costs_no_transfer_and_a_deletion_goes_to_the_bin() {
    let (a, b) = (Device::new(1), Device::new(2));
    a.write("old/place.bin", &noise(2 * MB, 3));
    a.write("gone.txt", b"bye");
    a.sync.scan().await.unwrap();
    b.pull_from(&a).await;

    std::fs::create_dir_all(a.root.join("new")).unwrap();
    std::fs::rename(a.root.join("old/place.bin"), a.root.join("new/place.bin")).unwrap();
    std::fs::remove_file(a.root.join("gone.txt")).unwrap();
    a.sync.scan().await.unwrap();
    let r = b.pull_from(&a).await;
    assert_eq!(0, r.bytes_fetched, "the content was already here");
    assert_eq!(a.files(), b.files());
    assert!(!b.root.join("gone.txt").exists());
    assert_eq!(
        2,
        b.bin_count(),
        "the deleted file and the moved file's old place"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_edits_keep_both_and_converge() {
    let (a, b) = (Device::new(1), Device::new(2));
    a.write("plan.txt", b"base");
    a.sync.scan().await.unwrap();
    b.pull_from(&a).await;

    a.write("plan.txt", b"edited on A");
    b.write("plan.txt", b"edited on B, longer");
    a.sync.scan().await.unwrap();
    b.sync.scan().await.unwrap();
    b.pull_from(&a).await;
    a.pull_from(&b).await;
    b.pull_from(&a).await;

    let (fa, fb) = (a.files(), b.files());
    assert_eq!(fa, fb);
    assert_eq!(2, fa.len(), "{fa:?}");
    let contents: Vec<&[u8]> = fa.values().map(Vec::as_slice).collect();
    assert!(contents.contains(&&b"edited on A"[..]));
    assert!(contents.contains(&&b"edited on B, longer"[..]));
    assert!(fa.keys().any(|k| k.contains("conflict")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_local_edit_not_yet_scanned_is_never_overwritten() {
    let (a, b) = (Device::new(1), Device::new(2));
    a.write("doc.txt", b"v1");
    a.sync.scan().await.unwrap();
    b.pull_from(&a).await;

    a.write("doc.txt", b"v2 from A");
    a.sync.scan().await.unwrap();
    // B edits after its last scan and pulls without scanning: the change waits.
    b.write("doc.txt", b"B's unsaved work");
    let r = b.sync.pull(&b.link_to(&a)).await.unwrap();
    assert_eq!(1, r.deferred);
    assert_eq!(
        b"B's unsaved work".to_vec(),
        std::fs::read(b.root.join("doc.txt")).unwrap()
    );

    // After a scan it is an ordinary conflict: B's edit is newer and wins; A, which holds the
    // losing content, keeps it as a copy, and that reaches B too.
    b.pull_from(&a).await;
    a.pull_from(&b).await;
    b.pull_from(&a).await;
    assert_eq!(a.files(), b.files());
    let files = b.files();
    let contents: Vec<&[u8]> = files.values().map(Vec::as_slice).collect();
    assert!(contents.contains(&&b"v2 from A"[..]));
    assert!(contents.contains(&&b"B's unsaved work"[..]));
}

#[tokio::test(flavor = "multi_thread")]
async fn three_devices_in_a_chain_converge() {
    let (a, b, c) = (Device::new(1), Device::new(2), Device::new(3));
    a.write("a.txt", b"from a");
    b.write("b.txt", b"from b");
    c.write("c/c.txt", b"from c");
    for _ in 0..2 {
        b.pull_from(&a).await;
        c.pull_from(&b).await;
        a.pull_from(&c).await;
        b.pull_from(&c).await;
    }
    assert_eq!(a.files(), b.files());
    assert_eq!(b.files(), c.files());
    assert_eq!(3, a.files().len());

    // A deletion travels the chain and is not brought back by C, which still had the file.
    std::fs::remove_file(a.root.join("b.txt")).unwrap();
    a.sync.scan().await.unwrap();
    for _ in 0..2 {
        c.pull_from(&a).await;
        b.pull_from(&c).await;
        a.pull_from(&b).await;
    }
    for d in [&a, &b, &c] {
        assert!(!d.root.join("b.txt").exists());
        assert_eq!(2, d.files().len());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn devices_sync_over_quic() {
    let (a, b) = (Device::new(1), Device::new(2));
    a.write("over/the/wire.bin", &noise(5 * MB, 4));
    a.sync.scan().await.unwrap();

    let only = |k: &DeviceKey| -> Trust {
        let pk = k.public();
        Arc::new(move |p: &[u8; 32]| *p == pk)
    };
    let loopback: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let server = Endpoint::bind(loopback, &a.key, only(&b.key)).unwrap();
    server.serve(a.service.clone());
    let client = Endpoint::bind(loopback, &b.key, only(&a.key)).unwrap();
    let link = client
        .connect(server.local_addr().unwrap(), Some(a.id()))
        .await
        .unwrap();
    assert_eq!(vec![SHARE], link.shared().await.unwrap());

    b.sync.scan().await.unwrap();
    let r = b.sync.pull(&link).await.unwrap();
    assert_eq!(5 * MB as u64, r.bytes_fetched);
    assert_eq!(a.files(), b.files());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pair_that_syncs_only_with_drive_is_not_offered() {
    let (a, b) = (Device::new(1), Device::new(2));
    let mut p = a.store.lock().get_pair("p").unwrap().unwrap();
    p.route = Route::Drive;
    a.store.lock().update_pair(&p).unwrap();
    assert!(b.link_to(&a).shared().await.unwrap().is_empty());
    b.sync.scan().await.unwrap();
    assert!(b.sync.pull(&b.link_to(&a)).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn ignored_and_internal_paths_from_a_peer_are_never_written() {
    let (a, b) = (Device::new(1), Device::new(2));
    // B ignores *.tmp through a rule file; A does not.
    b.write(".waslignore", b"*.tmp\n");
    a.write("keep.txt", b"k");
    a.write("scratch.tmp", b"t");
    a.write("half.bin.wasl-part", b"p");
    a.sync.scan().await.unwrap();
    let r = b.pull_from(&a).await;
    assert!(b.root.join("keep.txt").exists());
    assert!(!b.root.join("scratch.tmp").exists(), "ignored here");
    assert!(!b.root.join("half.bin.wasl-part").exists());
    assert_eq!(0, r.deferred, "rejections do not hold the next pull back");
    assert!(r.rejected >= 1);
    // Nothing of the spool is left in the folder.
    assert!(b.files().keys().all(|k| !k.contains("wasl-part")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pull_before_any_scan_is_refused() {
    let (a, b) = (Device::new(1), Device::new(2));
    a.write("x.txt", b"x");
    a.sync.scan().await.unwrap();
    assert!(b.sync.pull(&b.link_to(&a)).await.is_err());
    assert!(!b.root.join("x.txt").exists());
}

/// An agent's open or held changes on one device (docs/AGENT_CHANGES.md) are not offered to
/// peers, and a peer's change to such a file waits; accepting releases them.
#[tokio::test(flavor = "multi_thread")]
async fn an_agents_changes_stay_on_the_device_until_accepted() {
    use wasl_store::{AgentEntry, Changeset, ChangesetStatus, EarlierVersion, EntryKind, ManifestRow};
    let (pc, mac) = (Device::new(1), Device::new(2));
    pc.write("a.txt", b"a1");
    pc.write("b.txt", b"b1");
    pc.sync.scan().await.unwrap();
    mac.pull_from(&pc).await;

    let manifest: Vec<ManifestRow> = ["a.txt", "b.txt"]
        .iter()
        .map(|r| {
            let m = std::fs::metadata(pc.root.join(r)).unwrap();
            let t = filetime::FileTime::from_last_modification_time(&m);
            ManifestRow {
                rel: (*r).into(),
                is_dir: false,
                size: m.len(),
                mtime: t.unix_seconds() * 1000 + i64::from(t.nanoseconds() / 1_000_000),
                synced: true,
            }
        })
        .collect();
    let cs = Changeset {
        id: "cs1".into(),
        pair_id: Some("p".into()),
        actor: "agent:claude-code".into(),
        label: None,
        scope: String::new(),
        session: None,
        status: ChangesetStatus::Open,
        started_at: 1,
        ended_at: None,
        decided_at: None,
    };
    pc.store.lock().create_changeset(&cs, &manifest).unwrap();
    pc.write("a.txt", b"agent");
    pc.write("c.txt", b"new by agent");
    std::fs::remove_file(pc.root.join("b.txt")).unwrap();
    // The Mac edits the file the agent deleted: it waits on the PC too.
    mac.write("b.txt", b"b2 from the mac");
    mac.sync.scan().await.unwrap();

    pc.sync.scan().await.unwrap();
    mac.pull_from(&pc).await;
    let r = pc.pull_from(&mac).await;
    assert_eq!(1, r.deferred);
    assert_eq!(Some(&b"a1".to_vec()), mac.files().get("a.txt"));
    assert!(!mac.files().contains_key("c.txt"));
    assert!(!pc.root.join("b.txt").exists());

    // Ended and held: still nothing goes.
    let entry = |rel: &str, kind| AgentEntry {
        rel: rel.into(),
        key: rel.into(),
        kind,
        is_dir: false,
        before_size: None,
        before_mtime: None,
        after_size: None,
        after_mtime: None,
        source: EarlierVersion::None,
        location: None,
        outcome: None,
    };
    let entries = vec![
        entry("a.txt", EntryKind::Changed),
        entry("b.txt", EntryKind::Deleted),
        entry("c.txt", EntryKind::Added),
    ];
    pc.store.lock().end_changeset("cs1", &entries, 2).unwrap();
    mac.pull_from(&pc).await;
    assert_eq!(Some(&b"a1".to_vec()), mac.files().get("a.txt"));

    // Accepted: the agent's work goes to the Mac.
    pc.store
        .lock()
        .decide_changeset("cs1", ChangesetStatus::Accepted, 3)
        .unwrap();
    pc.sync.scan().await.unwrap();
    mac.pull_from(&pc).await;
    let got = mac.files();
    assert_eq!(Some(&b"agent".to_vec()), got.get("a.txt"));
    assert_eq!(Some(&b"new by agent".to_vec()), got.get("c.txt"));
}

/// Text-like bytes: compress well, and differ by seed so chunks are not shared by accident.
fn prose(len: usize, seed: u64) -> Vec<u8> {
    let words = [
        "sync", "folder", "wasl", "device", "chunk", "drive", "kahf", "labs", "note", "edit",
    ];
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut out = Vec::with_capacity(len + 16);
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(words[(x % 10) as usize].as_bytes());
        out.push(if x.is_multiple_of(13) { b'\n' } else { b' ' });
    }
    out.truncate(len);
    out
}

/// Two devices over real QUIC on loopback; the client pulls from the server.
async fn quic_pair(a: &Device, b: &Device) -> (Endpoint, Endpoint, wasl_net::QuicLink) {
    let only = |k: &DeviceKey| -> Trust {
        let pk = k.public();
        Arc::new(move |p: &[u8; 32]| *p == pk)
    };
    let loopback: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let server = Endpoint::bind(loopback, &a.key, only(&b.key)).unwrap();
    server.serve(a.service.clone());
    let client = Endpoint::bind(loopback, &b.key, only(&a.key)).unwrap();
    let link = client
        .connect(server.local_addr().unwrap(), Some(a.id()))
        .await
        .unwrap();
    (server, client, link)
}

/// A QUIC link that asks for one chunk at a time. A sender answers raw when all its compression
/// slots are busy (by design: peers cannot queue CPU work on it), so with several chunks in flight
/// how much travels compressed depends on how loaded the machine is (#85). One at a time, a slot
/// is always free.
struct OneChunkAtATime {
    link: wasl_net::QuicLink,
    turn: tokio::sync::Mutex<()>,
}

#[async_trait::async_trait]
impl PeerLink for OneChunkAtATime {
    fn peer(&self) -> DeviceId {
        self.link.peer()
    }
    async fn shared(&self) -> wasl_net::PeerResult<Vec<ShareId>> {
        self.link.shared().await
    }
    async fn changes_since(
        &self,
        pair: ShareId,
        since: u64,
        limit: usize,
    ) -> wasl_net::PeerResult<wasl_net::IndexPage> {
        self.link.changes_since(pair, since, limit).await
    }
    async fn chunk_list(
        &self,
        pair: ShareId,
        key: &str,
        blake3: wasl_chunk::Hash,
    ) -> wasl_net::PeerResult<wasl_chunk::ChunkList> {
        self.link.chunk_list(pair, key, blake3).await
    }
    async fn chunk(&self, pair: ShareId, hash: wasl_chunk::Hash) -> wasl_net::PeerResult<Vec<u8>> {
        let _turn = self.turn.lock().await;
        self.link.chunk(pair, hash).await
    }
    async fn notify(&self, pair: ShareId, seq: u64) -> wasl_net::PeerResult<()> {
        self.link.notify(pair, seq).await
    }
    async fn pair(&self, round: wasl_net::PairRound) -> wasl_net::PeerResult<wasl_net::PairReply> {
        self.link.pair(round).await
    }
    async fn group_ops(&self, heads: Vec<wasl_net::OpId>) -> wasl_net::PeerResult<Vec<wasl_net::GroupOp>> {
        self.link.group_ops(heads).await
    }
    async fn group_changed(&self) -> wasl_net::PeerResult<()> {
        self.link.group_changed().await
    }
    async fn offers(&self) -> wasl_net::PeerResult<Vec<wasl_net::Offer>> {
        self.link.offers().await
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn text_travels_compressed_media_raw_and_the_files_are_identical() {
    let (a, b) = (Device::new(1), Device::new(2));
    a.write("notes/book.md", &prose(3 * MB, 1));
    a.write("big/log.txt", &prose(20 * MB, 2));
    a.write("photos/raw.jpg", &noise(2 * MB, 3));
    a.sync.scan().await.unwrap();
    let (_s, _c, link) = quic_pair(&a, &b).await;
    link.set_compression(wasl_net::compress::LEVEL_INTERNET);

    b.sync.scan().await.unwrap();
    b.sync
        .pull(&OneChunkAtATime {
            link: link.clone(),
            turn: Default::default(),
        })
        .await
        .unwrap();
    assert_eq!(a.files(), b.files());
    let (raw, wire) = link.chunk_bytes();
    assert_eq!((25 * MB) as u64, raw);
    // The text shrinks several times; the noise (a stand-in for a photo) goes as it is.
    assert!(wire < raw / 2, "{wire} on the wire for {raw}");
    assert!(wire > (2 * MB) as u64, "the incompressible file is sent raw");
}

#[tokio::test(flavor = "multi_thread")]
async fn raw_is_sent_when_asked_or_when_the_sender_is_on_a_budget() {
    let (a, b) = (Device::new(1), Device::new(2));
    a.write("doc.txt", &prose(MB, 4));
    a.sync.scan().await.unwrap();
    let (server, _c, link) = quic_pair(&a, &b).await;
    // The sender has no budget for compression at all: everything goes raw.
    server.set_compression_cap(0);
    link.set_compression(wasl_net::compress::LEVEL_METERED);
    b.sync.scan().await.unwrap();
    b.sync.pull(&link).await.unwrap();
    assert_eq!(a.files(), b.files());
    let (raw, wire) = link.chunk_bytes();
    assert_eq!(raw, wire);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_edit_in_a_medium_file_sends_about_one_small_chunk() {
    let (a, b) = (Device::new(1), Device::new(2));
    let mut doc = prose(8 * MB, 5);
    a.write("report.md", &doc);
    a.sync.scan().await.unwrap();
    b.pull_from(&a).await;

    // One paragraph rewritten in the middle of an 8 MiB document.
    for (i, byte) in b"A new paragraph, written today.".iter().enumerate() {
        doc[4 * MB + i] = *byte;
    }
    a.write("report.md", &doc);
    a.sync.scan().await.unwrap();
    let r = b.pull_from(&a).await;
    assert_eq!(a.files(), b.files());
    // Before the medium tier this cost the whole 8 MiB; a 256 KiB chunk is the most one edit
    // usually touches, two in rare cases.
    assert!(
        r.bytes_fetched <= 2 * 256 * 1024,
        "{} bytes fetched",
        r.bytes_fetched
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn files_cut_before_the_medium_tier_are_cut_again_once_without_a_new_version() {
    let a = Device::new(1);
    let doc = prose(6 * MB, 6);
    a.write("old.md", &doc);
    a.sync.scan().await.unwrap();
    // As an older Wasl left it: the 6 MiB file as one chunk, and no tier recorded.
    let one = wasl_chunk::chunk_reader(&doc[..], &wasl_chunk::ChunkParams::DEFAULT).unwrap();
    assert_eq!(1, one.chunks.len());
    let rows: Vec<([u8; 32], u64, u64)> = one
        .chunks
        .iter()
        .map(|c| (c.hash, c.offset, u64::from(c.len)))
        .collect();
    a.store
        .lock()
        .put_chunk_list("p", "old.md", &one.blake3, &one.to_bytes(), &rows)
        .unwrap();
    a.store.lock().remove_setting("chunk_tier:p").unwrap();
    let seq = a.store.lock().load_index("p").unwrap().unwrap().seq;

    let r = a.sync.scan().await.unwrap();
    assert_eq!(0, r.changed, "same content: no new version");
    assert_eq!(seq, a.store.lock().load_index("p").unwrap().unwrap().seq);
    let (_, bytes) = a
        .store
        .lock()
        .chunk_list_by_content("p", &one.blake3)
        .unwrap()
        .unwrap();
    let list = wasl_chunk::ChunkList::from_bytes(&bytes).unwrap();
    assert!(
        list.chunks.len() > 20,
        "cut again into {} chunks",
        list.chunks.len()
    );
    // Only once.
    assert_eq!(0, a.sync.scan().await.unwrap().hashed);
}

/// Measures compression and the medium tier on a real folder (docs/COMPRESSION.md §4):
/// `WASL_MEASURE_TREE=<folder> cargo test -p wasl-engine --test devices measure -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs WASL_MEASURE_TREE"]
async fn measure_a_real_tree() {
    let Some(src) = std::env::var_os("WASL_MEASURE_TREE") else {
        return;
    };
    let a = Device::new(1);
    copy_tree(Path::new(&src), &a.root);
    let t = std::time::Instant::now();
    a.sync.scan().await.unwrap();
    println!("scan: {:.1} s", t.elapsed().as_secs_f64());

    println!("first sync of the whole tree to a new device, over QUIC on loopback:");
    for (name, level) in [
        ("raw", 0u8),
        ("LAN (1)", 1),
        ("internet (3)", 3),
        ("metered (9)", 9),
    ] {
        let b = Device::new(2);
        let (_s, _c, link) = quic_pair(&a, &b).await;
        link.set_compression(level);
        b.sync.scan().await.unwrap();
        let t = std::time::Instant::now();
        let report = b.sync.pull(&link).await.unwrap();
        let secs = t.elapsed().as_secs_f64();
        let (raw, wire) = link.chunk_bytes();
        assert_eq!((0, 0), (report.deferred, report.rejected), "{report:?}");
        same_where_synced(&a, &b);
        println!(
            "  {name:<13} {:>8.1} MB used, {:>8.1} MB on the wire ({:>5.1} %), {secs:.1} s",
            raw as f64 / 1e6,
            wire as f64 / 1e6,
            100.0 * wire as f64 / raw.max(1) as f64
        );
    }

    // Small edits to medium text files: what the medium tier sends against the whole file.
    let b = Device::new(2);
    b.pull_from(&a).await;
    let mut edited: Vec<(String, u64)> = Vec::new();
    for (rel, bytes) in a.files() {
        let text = ["md", "rs", "ts", "kt", "json", "dart", "py", "html", "css", "sql"]
            .iter()
            .any(|e| rel.ends_with(&format!(".{e}")));
        if text && bytes.len() > 256 * 1024 && edited.len() < 50 {
            let mut v = bytes.clone();
            let at = v.len() / 2;
            v.splice(at..at, b"/* edited */".iter().copied());
            a.write(&rel, &v);
            edited.push((rel, v.len() as u64));
        }
    }
    if edited.is_empty() {
        println!("no text file above 256 KiB to edit");
        return;
    }
    a.sync.scan().await.unwrap();
    let r = b.pull_from(&a).await;
    let whole: u64 = edited.iter().map(|(_, n)| n).sum();
    same_where_synced(&a, &b);
    println!(
        "{} edits in text files of 256 KiB–16 MiB: {:.2} MB fetched with the medium tier, {:.2} MB as whole files ({:.1} %)",
        edited.len(),
        r.bytes_fetched as f64 / 1e6,
        whole as f64 / 1e6,
        100.0 * r.bytes_fetched as f64 / whole as f64
    );
}

/// Every file the receiver has is the sender's, byte for byte, and everything the receiver lacks is
/// something the pair's rules leave out (hidden files and folders in a default pair).
fn same_where_synced(a: &Device, b: &Device) {
    let (fa, fb) = (a.files(), b.files());
    for (k, v) in &fb {
        assert_eq!(Some(v), fa.get(k), "{k} differs");
    }
    let missing: Vec<&String> = fa.keys().filter(|k| !fb.contains_key(*k)).collect();
    let unexplained: Vec<&&String> = missing
        .iter()
        .filter(|k| !k.split('/').any(|part| part.starts_with('.')))
        .collect();
    assert!(
        unexplained.is_empty(),
        "not synced: {:?}",
        &unexplained[..unexplained.len().min(10)]
    );
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        let ft = e.file_type().unwrap();
        if ft.is_dir() {
            copy_tree(&src, &dst);
        } else if ft.is_file() {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

/// A peer whose index never ends: every page says more follows.
struct Endless;

#[async_trait::async_trait]
impl PeerService for Endless {
    async fn shared(&self, _: DeviceId) -> Vec<ShareId> {
        vec![SHARE]
    }
    async fn changes_since(
        &self,
        _: DeviceId,
        _: ShareId,
        since: u64,
        limit: usize,
    ) -> wasl_net::PeerResult<wasl_net::IndexPage> {
        let mut vv = wasl_core::vv::VersionVector::new();
        vv.tick(9);
        let gone = wasl_core::nway::Version {
            is_dir: false,
            deleted: true,
            hash: None,
            size: 0,
            mtime: 0,
            vv,
            author: 9,
        };
        Ok(wasl_net::IndexPage {
            entries: (1..=limit as u64)
                .map(|i| (format!("gone-{}", since + i), gone.clone()))
                .collect(),
            next_since: since + limit as u64,
            more: true,
        })
    }
    async fn chunk_list(
        &self,
        _: DeviceId,
        _: ShareId,
        _: &str,
        _: wasl_chunk::Hash,
    ) -> wasl_net::PeerResult<wasl_chunk::ChunkList> {
        Err(wasl_net::PeerError::NotShared)
    }
    async fn chunk(&self, _: DeviceId, _: ShareId, _: wasl_chunk::Hash) -> wasl_net::PeerResult<Vec<u8>> {
        Err(wasl_net::PeerError::NotShared)
    }
    async fn notify(&self, _: DeviceId, _: ShareId, _: u64) {}
}

#[tokio::test(flavor = "multi_thread")]
async fn a_peer_that_never_stops_paging_is_read_a_bounded_amount_per_pull() {
    assert_eq!(200_000, wasl_engine::MAX_PULL_ENTRIES);
    let a = Device::new(1);
    let fs: Arc<dyn LocalFs> = Arc::new(StdFs::new(&a.root, a._dir.path().join("bin")));
    let pair = a.store.lock().get_pair("p").unwrap().unwrap();
    let clock = Arc::new(TestClock(AtomicI64::new(1_800_000_000_000)));
    let sync =
        DeviceSync::new(a.store.clone(), fs, pair, SHARE, a.id(), clock).with_pull_limit(3000, usize::MAX);
    sync.scan().await.unwrap();
    let peer = DeviceKey::from_seed(&[9; 32]).id();
    let link = LocalLink::new(a.id(), peer, Arc::new(Endless));
    let r = tokio::time::timeout(std::time::Duration::from_secs(60), sync.pull(&link))
        .await
        .expect("the pull ends")
        .unwrap();
    assert!(r.truncated);
    // Three whole pages; the next pull goes on from there.
    let since = |s: &SharedStore| s.lock().peer_since("p", &peer.to_string()).unwrap();
    assert_eq!(3 * 1024, since(&a.store));
    let r = sync.pull(&link).await.unwrap();
    assert!(r.truncated);
    assert_eq!(6 * 1024, since(&a.store));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pair_kept_to_the_lan_is_never_served_beyond_it() {
    let a = Device::new(1);
    a.write("x.txt", b"x");
    a.sync.scan().await.unwrap();
    let asker = DeviceKey::from_seed(&[2; 32]).id();
    let far = a.service.from_remote("203.0.113.5:4000".parse().unwrap());
    let near = a.service.from_remote("192.168.1.9:4000".parse().unwrap());
    // Reach "LAN" (the default): only a LAN address is answered.
    assert!(far.shared(asker).await.is_empty());
    assert_eq!(
        Err(wasl_net::PeerError::NotShared),
        far.changes_since(asker, SHARE, 0, 10).await.map(|_| ())
    );
    assert_eq!(vec![SHARE], near.shared(asker).await);
    assert!(near.changes_since(asker, SHARE, 0, 10).await.is_ok());
    // Reach "LAN and internet": anywhere.
    let mut pair = a.store.lock().get_pair("p").unwrap().unwrap();
    pair.reach = wasl_core::Reach::LanAndInternet;
    a.store.lock().update_pair(&pair).unwrap();
    assert_eq!(vec![SHARE], far.shared(asker).await);
}

// ------------------------------------------------------------- introductions (2026-10)

fn stranger(seed: u8) -> DeviceId {
    DeviceKey::from_seed(&[seed; 32]).id()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_new_here_gets_nothing_until_kept() {
    // docs/INTRODUCTIONS.md §3.1: a pair shared with all devices is served only to members known
    // on this device.
    let a = Device::new(1);
    a.write("x.txt", b"x");
    a.sync.scan().await.unwrap();
    let new = stranger(20);
    a.store.lock().note_device(&new.to_string(), false).unwrap();
    assert!(a.service.shared(new).await.is_empty());
    assert_eq!(
        Err(wasl_net::PeerError::NotShared),
        a.service.changes_since(new, SHARE, 0, 10).await.map(|_| ())
    );
    assert_eq!(
        Err(wasl_net::PeerError::NotShared),
        a.service.chunk(new, SHARE, [0; 32]).await.map(|_| ())
    );
    // No note at all is no better than new.
    let unnoted = stranger(21);
    assert!(a.service.shared(unnoted).await.is_empty());

    a.store.lock().set_device_known(&new.to_string(), true).unwrap();
    assert_eq!(vec![SHARE], a.service.shared(new).await);
    assert!(a.service.changes_since(new, SHARE, 0, 10).await.is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pair_shared_with_chosen_devices_is_served_only_to_them() {
    let (a, b, c) = (Device::new(1), Device::new(2), Device::new(3));
    a.write("plan.txt", b"for B");
    a.sync.scan().await.unwrap();
    a.store
        .lock()
        .set_share_scope("p", false, &[b.id().to_string()])
        .unwrap();
    // C is known here, but not on the list.
    assert!(a.service.shared(c.id()).await.is_empty());
    assert!(c.sync.scan().await.is_ok());
    assert!(c.sync.pull(&c.link_to(&a)).await.is_err());
    assert!(!c.root.join("plan.txt").exists());
    // B is on it.
    assert_eq!(vec![SHARE], a.service.shared(b.id()).await);
    b.pull_from(&a).await;
    assert_eq!(b"for B".to_vec(), std::fs::read(b.root.join("plan.txt")).unwrap());
    // Off the list: stopped at the next request.
    a.store.lock().set_share_scope("p", false, &[]).unwrap();
    assert_eq!(
        Err(wasl_net::PeerError::NotShared),
        a.service.changes_since(b.id(), SHARE, 0, 10).await.map(|_| ())
    );
    // A device on the list is served even while new here.
    let new = stranger(20);
    a.store.lock().note_device(&new.to_string(), false).unwrap();
    a.store
        .lock()
        .set_share_scope("p", false, &[new.to_string()])
        .unwrap();
    assert_eq!(vec![SHARE], a.service.shared(new).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn offers_are_the_pairs_that_list_the_asker() {
    let a = Device::new(1);
    a.write("one.txt", b"12345");
    a.write("sub/two.txt", b"678");
    a.sync.scan().await.unwrap();
    let (b, c) = (stranger(2), stranger(3));
    assert!(
        a.service.offers(b).await.is_empty(),
        "all devices: offered to nobody"
    );
    a.store
        .lock()
        .set_share_scope("p", true, &[b.to_string()])
        .unwrap();
    let offers = a.service.offers(b).await;
    assert_eq!(1, offers.len());
    assert_eq!(
        (SHARE, "Shared", 2, 8),
        (
            offers[0].share,
            offers[0].name.as_str(),
            offers[0].files,
            offers[0].bytes
        )
    );
    assert!(a.service.offers(c).await.is_empty());
    // A pair that would not be served is not offered either.
    let mut pair = a.store.lock().get_pair("p").unwrap().unwrap();
    pair.enabled = false;
    a.store.lock().update_pair(&pair).unwrap();
    assert!(a.service.offers(b).await.is_empty());
}

impl Device {
    /// Writes `rel` with a fixed modification time, so which edit wins a conflict is known.
    fn write_at(&self, rel: &str, bytes: &[u8], secs: i64) {
        self.write(rel, bytes);
        let t = filetime::FileTime::from_unix_time(1_800_000_000 + secs, 0);
        filetime::set_file_mtime(self.root.join(rel), t).unwrap();
    }

    /// Whether some file in the folder holds exactly `bytes`.
    fn holds(&self, bytes: &[u8]) -> bool {
        self.files().values().any(|v| v == bytes)
    }
}

/// A edits `f.txt` to `on_a`, B to `on_b` a minute later, so A loses, keeps a conflict copy,
/// and both devices exchange the result.
async fn a_loses_a_conflict(a: &Device, b: &Device, on_a: &[u8], on_b: &[u8], at: i64) -> PullReport {
    a.write_at("f.txt", on_a, at);
    b.write_at("f.txt", on_b, at + 60);
    a.sync.scan().await.unwrap();
    b.sync.scan().await.unwrap();
    let r = a.pull_from(b).await;
    b.pull_from(a).await;
    r
}

#[tokio::test(flavor = "multi_thread")]
async fn a_conflict_copy_made_again_after_the_first_was_deleted_is_kept() {
    // #141 H2: the second copy took the first one's name and a vector that the first copy's
    // tombstone dominates, so the losing content never reached B.
    let (a, b) = (Device::new(1), Device::new(2));
    a.write_at("f.txt", b"base", 0);
    a.sync.scan().await.unwrap();
    b.pull_from(&a).await;
    a_loses_a_conflict(&a, &b, b"lost once", b"won once", 100).await;
    let copy = b
        .files()
        .into_keys()
        .find(|k| k.contains("conflict"))
        .expect("a copy");
    std::fs::remove_file(b.root.join(&copy)).unwrap();
    b.sync.scan().await.unwrap();
    a.pull_from(&b).await;
    assert!(!a.holds(b"lost once"), "the reviewed copy is deleted on A too");

    // A loses again with the same content (it went back to it), so the copy name repeats.
    a_loses_a_conflict(&a, &b, b"lost once", b"won twice", 300).await;
    a.pull_from(&b).await;
    assert!(
        a.holds(b"lost once"),
        "A keeps the losing content: {:?}",
        a.files().keys()
    );
    assert!(
        b.holds(b"lost once"),
        "the copy reaches B: {:?}",
        b.files().keys()
    );
    assert_eq!(a.files(), b.files());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_conflict_lost_by_the_same_device_gets_its_own_copy() {
    // #141 H3: both copies were named after the author only, so the second conflict waited for
    // the first copy to go, and the key never synced again.
    let (a, b) = (Device::new(1), Device::new(2));
    a.write_at("f.txt", b"base", 0);
    a.sync.scan().await.unwrap();
    b.pull_from(&a).await;
    a_loses_a_conflict(&a, &b, b"lost once", b"won once", 100).await;
    let r = a_loses_a_conflict(&a, &b, b"lost twice", b"won twice", 300).await;
    assert_eq!((1, 0), (r.conflicts, r.deferred), "{r:?}");
    for d in [&a, &b] {
        assert!(d.holds(b"lost once") && d.holds(b"lost twice") && d.holds(b"won twice"));
    }
    assert_eq!(a.files(), b.files());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rename_to_the_same_name_is_followed_in_place_on_a_folding_folder() {
    // #173 (ADR-0004): `Foo.txt` renamed to `foo.txt` (or NFD to NFC) on one device reached a
    // folder that folds names as a new file it could not place beside the old one, then a delete:
    // the file went to the bin and came back as a download, or as `foo (1).txt` on Android.
    for (old, new) in [("Foo.txt", "foo.txt"), ("\u{627}\u{654}.txt", "\u{623}.txt")] {
        let (a, b) = (Device::new(1), Device::folding(2));
        a.write(old, &noise(300_000, 5));
        a.sync.scan().await.unwrap();
        b.pull_from(&a).await;
        std::fs::rename(a.root.join(old), a.root.join(new)).unwrap();
        a.sync.scan().await.unwrap();
        let r = b.pull_from(&a).await;
        assert_eq!((0, 0), (r.deferred, r.bytes_fetched), "{old} -> {new}: {r:?}");
        assert_eq!(
            (0, 0),
            (r.received, r.deleted),
            "{old} -> {new}: a rename, counted as neither"
        );
        assert_eq!(
            vec![new.to_owned()],
            b.files().into_keys().collect::<Vec<_>>(),
            "{old} -> {new}"
        );
        assert_eq!(0, b.bin_count(), "{old} -> {new}: nothing is binned");
        a.pull_from(&b).await;
        assert_eq!(a.files(), b.files());
    }
}

// Only a folder that keeps both spellings apart can hold them (Linux).
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn a_name_held_twice_by_a_peer_is_a_clash_until_one_goes() {
    // #173 (ADR-0004 rule 2): `Notes.txt` and `notes.txt` are one name, as are the folders `Docs`
    // and `docs`. Every key with such a name, and everything under it, stays out until one goes.
    let (a, b) = (Device::new(1), Device::folding(2));
    a.write("Notes.txt", b"upper");
    a.write("notes.txt", b"lower");
    a.write("Docs/a.txt", b"a");
    a.write("docs/b.txt", b"b");
    a.write("plain.txt", b"p");
    a.sync.scan().await.unwrap();
    let r = b.pull_from(&a).await;
    assert_eq!(vec!["Docs", "Notes.txt", "docs", "notes.txt"], r.clashes, "{r:?}");
    assert_eq!(6, r.deferred, "{r:?}");
    assert_eq!(
        vec!["plain.txt".to_owned()],
        b.files().into_keys().collect::<Vec<_>>()
    );
    std::fs::remove_file(a.root.join("notes.txt")).unwrap();
    std::fs::remove_dir_all(a.root.join("docs")).unwrap();
    a.sync.scan().await.unwrap();
    let r = b.pull_from(&a).await;
    assert_eq!((0, 0), (r.clashes.len(), r.deferred), "{r:?}");
    assert_eq!(a.files(), b.files());
}

#[tokio::test(flavor = "multi_thread")]
async fn one_name_made_on_two_devices_is_a_clash_on_both_until_one_is_renamed() {
    // ADR-0004 rule 2, on both sides: `report.txt` made on A and `Report.txt` on B are one name.
    let (a, b) = (Device::new(1), Device::folding(2));
    a.write("report.txt", b"from A");
    b.write("Report.txt", b"from B");
    a.sync.scan().await.unwrap();
    let both = vec!["Report.txt".to_owned(), "report.txt".to_owned()];
    let r = b.pull_from(&a).await;
    assert_eq!((&both, 1), (&r.clashes, r.deferred), "{r:?}");
    let r = a.pull_from(&b).await;
    assert_eq!((&both, 1), (&r.clashes, r.deferred), "{r:?}");
    assert!(a.holds(b"from A") && !a.holds(b"from B"));
    assert!(b.holds(b"from B") && !b.holds(b"from A"));
    assert_eq!((0, 0), (a.bin_count(), b.bin_count()));

    std::fs::rename(a.root.join("report.txt"), a.root.join("report-a.txt")).unwrap();
    a.sync.scan().await.unwrap();
    let r = b.pull_from(&a).await;
    assert!(r.clashes.is_empty(), "{r:?}");
    a.pull_from(&b).await;
    assert_eq!(a.files(), b.files());
    assert_eq!(2, a.files().len());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pull_counts_only_what_it_changed_here() {
    // #148 review: a tombstone for a file already gone here is neither a download nor a delete.
    let (a, b) = (Device::new(1), Device::new(2));
    a.write("x.txt", b"x");
    a.write("y.txt", b"y");
    a.sync.scan().await.unwrap();
    let r = b.pull_from(&a).await;
    assert_eq!((2, 0), (r.received, r.deleted), "{r:?}");
    std::fs::remove_file(a.root.join("x.txt")).unwrap();
    std::fs::remove_file(b.root.join("x.txt")).unwrap();
    a.sync.scan().await.unwrap();
    let r = b.pull_from(&a).await;
    assert_eq!((0, 0), (r.received, r.deleted), "{r:?}");
    std::fs::remove_file(a.root.join("y.txt")).unwrap();
    a.sync.scan().await.unwrap();
    let r = b.pull_from(&a).await;
    assert_eq!((0, 1), (r.received, r.deleted), "{r:?}");
}

// ---------------------------------------------------------------- the apply layer (wasl#17)

/// A link that runs `hook` once, when the first chunk is asked for: after the pull has planned and
/// begun to build, before it installs (a person editing the folder meanwhile).
struct HookLink {
    inner: LocalLink,
    hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

#[async_trait::async_trait]
impl PeerLink for HookLink {
    fn peer(&self) -> DeviceId {
        self.inner.peer()
    }
    async fn shared(&self) -> wasl_net::PeerResult<Vec<ShareId>> {
        self.inner.shared().await
    }
    async fn changes_since(
        &self,
        p: ShareId,
        since: u64,
        limit: usize,
    ) -> wasl_net::PeerResult<wasl_net::IndexPage> {
        self.inner.changes_since(p, since, limit).await
    }
    async fn chunk_list(
        &self,
        p: ShareId,
        key: &str,
        b: wasl_chunk::Hash,
    ) -> wasl_net::PeerResult<wasl_chunk::ChunkList> {
        self.inner.chunk_list(p, key, b).await
    }
    async fn chunk(&self, p: ShareId, h: wasl_chunk::Hash) -> wasl_net::PeerResult<Vec<u8>> {
        if let Some(hook) = self.hook.lock().unwrap().take() {
            hook();
        }
        self.inner.chunk(p, h).await
    }
    async fn notify(&self, p: ShareId, seq: u64) -> wasl_net::PeerResult<()> {
        self.inner.notify(p, seq).await
    }
    async fn pair(&self, r: wasl_net::PairRound) -> wasl_net::PeerResult<wasl_net::PairReply> {
        self.inner.pair(r).await
    }
    async fn group_ops(&self, h: Vec<wasl_net::OpId>) -> wasl_net::PeerResult<Vec<wasl_net::GroupOp>> {
        self.inner.group_ops(h).await
    }
    async fn group_changed(&self) -> wasl_net::PeerResult<()> {
        self.inner.group_changed().await
    }
    async fn offers(&self) -> wasl_net::PeerResult<Vec<wasl_net::Offer>> {
        self.inner.offers().await
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn device_pull_and_drive_share_one_bin_policy_and_its_version_cap() {
    // The same file replaced again and again from the device side: only the newest
    // `MAX_LOCAL_VERSIONS` old copies stay in the bin, as on the Drive side.
    let (a, b) = (Device::new(1), Device::new(2));
    a.write("doc.txt", &noise(50_000, 1));
    a.sync.scan().await.unwrap();
    b.pull_from(&a).await;
    let rounds = MAX_LOCAL_VERSIONS as u64 + 3;
    for n in 0..rounds {
        a.write("doc.txt", &noise(50_000 + n as usize, 100 + n));
        a.sync.scan().await.unwrap();
        let r = b.pull_from(&a).await;
        assert_eq!(1, r.received, "round {n}: {r:?}");
    }
    assert_eq!(a.files(), b.files());
    let binned = b.store.lock().bin_entries(Some("p")).unwrap();
    assert_eq!(MAX_LOCAL_VERSIONS as usize, binned.len(), "the cap holds");
    assert!(
        binned
            .iter()
            .all(|e| e.entry.key == "doc.txt" && e.entry.reason == "OVERWRITTEN")
    );
    // What the cap dropped is gone from the bin folder too.
    let on_disk = std::fs::read_dir(b._dir.path().join("bin")).unwrap().count();
    assert_eq!(MAX_LOCAL_VERSIONS as usize, on_disk);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_edited_here_after_the_pull_was_planned_is_left_alone_and_reported() {
    let (a, b) = (Device::new(1), Device::new(2));
    a.write("doc.txt", &noise(300_000, 1));
    a.sync.scan().await.unwrap();
    b.pull_from(&a).await;
    a.write("doc.txt", &noise(300_000, 2));
    a.sync.scan().await.unwrap();
    b.sync.scan().await.unwrap();
    let root = b.root.clone();
    let link = HookLink {
        inner: b.link_to(&a),
        hook: Mutex::new(Some(Box::new(move || {
            std::fs::write(root.join("doc.txt"), b"edited while the pull ran").unwrap();
        }))),
    };
    let r = b.sync.pull(&link).await.unwrap();
    assert_eq!(vec!["doc.txt".to_owned()], r.changed_since_planned);
    assert_eq!((1, 0), (r.deferred, r.received), "{r:?}");
    assert_eq!(
        b"edited while the pull ran".to_vec(),
        std::fs::read(b.root.join("doc.txt")).unwrap()
    );
    assert_eq!(0, b.bin_count());
    // The next scan records the edit; it is then an ordinary conflict, and nothing is lost.
    b.pull_from(&a).await;
    a.pull_from(&b).await;
    b.pull_from(&a).await;
    let contents: Vec<Vec<u8>> = b.files().into_values().collect();
    assert!(contents.contains(&b"edited while the pull ran".to_vec()));
    assert!(contents.contains(&noise(300_000, 2)));
}

/// Pulls on B once to count the points of the apply path it passes, then once per point, stopped
/// there; B restarts and the devices sync again; `check` sees both devices.
async fn for_each_stop_of_a_pull(
    make: impl Fn() -> (Device, Device),
    mut check: impl FnMut(&str, &Device, &Device, &BTreeMap<String, Vec<u8>>),
) {
    let (a, mut b) = make();
    let plan = FaultPlan::counting();
    b.restart(Some(plan.clone()));
    b.sync.scan().await.unwrap();
    b.sync.pull(&b.link_to(&a)).await.unwrap();
    let points: Vec<Point> = plan.seen();
    assert!(!points.is_empty(), "a pull passes no point of the apply path");
    for (i, point) in points.iter().enumerate() {
        let (a, mut b) = make();
        let plan = FaultPlan::stop_at(i);
        b.restart(Some(plan.clone()));
        b.sync.scan().await.unwrap();
        let stopped = b.sync.pull(&b.link_to(&a)).await;
        assert!(stopped.is_err(), "{point:?} (#{i}) did not stop the pull");
        let at_stop = b.files();
        // The process starts again; both devices keep syncing.
        b.restart(None);
        for _ in 0..2 {
            b.pull_from(&a).await;
            a.pull_from(&b).await;
        }
        check(&format!("{point:?} (#{i})"), &a, &b, &at_stop);
    }
}

/// Two devices that share `doc.txt`; `edit` then changes it on A and A scans.
fn two_devices_then(edit: impl FnOnce(&Device)) -> (Device, Device) {
    let (a, b) = (Device::new(1), Device::new(2));
    a.write("doc.txt", &noise(40_000, 1));
    a.write("keep.txt", b"untouched");
    let rt = tokio::runtime::Handle::current();
    tokio::task::block_in_place(|| {
        rt.block_on(async {
            a.sync.scan().await.unwrap();
            b.pull_from(&a).await;
            edit(&a);
            a.sync.scan().await.unwrap();
        })
    });
    (a, b)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pull_stopped_anywhere_while_replacing_a_file_loses_nothing() {
    let (old, new) = (noise(40_000, 1), noise(40_001, 2));
    let make = || two_devices_then(|a| a.write("doc.txt", &noise(40_001, 2)));
    for_each_stop_of_a_pull(make, |at, a, b, at_stop| {
        let held = at_stop.get("doc.txt");
        assert!(
            held == Some(&old) || held == Some(&new),
            "{at}: at the stop the path held {:?} bytes",
            held.map(Vec::len)
        );
        // After the restart both devices hold the new content, and the old one is not lost.
        assert_eq!(Some(&new), b.files().get("doc.txt"), "{at}");
        assert_eq!(
            Some(&new),
            a.files().get("doc.txt"),
            "{at}: the other device is as it was"
        );
        assert_eq!(Some(&b"untouched".to_vec()), b.files().get("keep.txt"), "{at}");
        let in_bin = std::fs::read_dir(b._dir.path().join("bin"))
            .map(|rd| {
                rd.flatten()
                    .any(|f| std::fs::read(f.path()).is_ok_and(|c| c == old))
            })
            .unwrap_or(false);
        let kept_as_a_file = b.files().values().any(|c| c == &old);
        assert!(in_bin || kept_as_a_file, "{at}: the old content is gone");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pull_stopped_anywhere_while_deleting_a_file_loses_nothing() {
    let old = noise(40_000, 1);
    let make = || two_devices_then(|a| std::fs::remove_file(a.root.join("doc.txt")).unwrap());
    for_each_stop_of_a_pull(make, |at, a, b, _| {
        assert_eq!(None, b.files().get("doc.txt"), "{at}");
        assert_eq!(None, a.files().get("doc.txt"), "{at}");
        // Nothing else was touched on either device, and the deleted content is in B's bin.
        assert_eq!(Some(&b"untouched".to_vec()), b.files().get("keep.txt"), "{at}");
        assert_eq!(Some(&b"untouched".to_vec()), a.files().get("keep.txt"), "{at}");
        let kept = std::fs::read_dir(b._dir.path().join("bin"))
            .unwrap()
            .flatten()
            .any(|f| std::fs::read(f.path()).is_ok_and(|c| c == old));
        assert!(kept, "{at}: the deleted file's content is not in the bin");
    })
    .await;
}

// ---------------------------------------------------------------- folder renamed by case or accents (wasl#19)

/// Two spellings of one folder name: by case, and composed versus decomposed accents.
const FOLDER_RENAMES: [(&str, &str); 2] = [("Docs", "docs"), ("caf\u{e9}", "cafe\u{301}")];

/// A and B share `old/a.txt` and `old/sub/b.txt`; A then renames the folder to `new` and scans.
fn folder_renamed_on_a(old: &str, new: &str, b: Device) -> (Device, Device) {
    let a = Device::new(1);
    a.write(&format!("{old}/a.txt"), &noise(300_000, 5));
    a.write(&format!("{old}/sub/b.txt"), &noise(20_000, 6));
    a.write("other.txt", b"other");
    let rt = tokio::runtime::Handle::current();
    tokio::task::block_in_place(|| {
        rt.block_on(async {
            a.sync.scan().await.unwrap();
            b.pull_from(&a).await;
            std::fs::rename(a.root.join(old), a.root.join(new)).unwrap();
            a.sync.scan().await.unwrap();
        })
    });
    (a, b)
}

fn names_of(d: &Device) -> Vec<String> {
    d.files().into_keys().collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_folder_renamed_only_by_case_or_accents_follows_to_the_other_device() {
    for (old, new) in FOLDER_RENAMES {
        for folding in [false, true] {
            let b = if folding {
                Device::folding(2)
            } else {
                Device::new(2)
            };
            let (a, b) = folder_renamed_on_a(old, new, b);
            let r = b.pull_from(&a).await;
            let at = format!("{old} -> {new} (folding: {folding}): {r:?}");
            assert_eq!(0, r.deferred, "{at}");
            assert_eq!(
                (0, 0, 0),
                (r.received, r.deleted, r.bytes_fetched as usize),
                "{at}"
            );
            assert_eq!(
                vec![
                    format!("{new}/a.txt"),
                    format!("{new}/sub/b.txt"),
                    "other.txt".to_owned()
                ],
                names_of(&b),
                "{at}"
            );
            assert_eq!(0, b.bin_count(), "{at}: nothing is binned");
            // Not deferred again, and nothing goes back to A because of it.
            let again = b.pull_from(&a).await;
            assert_eq!(0, again.deferred, "{at}");
            let back = a.pull_from(&b).await;
            assert_eq!((0, 0), (back.received, back.deleted), "{at}: {back:?}");
            assert_eq!(0, a.bin_count(), "{at}");
            assert_eq!(a.files(), b.files(), "{at}");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_deleted_while_its_folder_was_renamed_does_not_come_back() {
    for (old, new) in FOLDER_RENAMES {
        let (a, b) = (Device::new(1), Device::new(2));
        a.write(&format!("{old}/a.txt"), &noise(30_000, 5));
        a.write(&format!("{old}/c.txt"), &noise(30_000, 6));
        a.sync.scan().await.unwrap();
        b.pull_from(&a).await;
        std::fs::remove_file(a.root.join(old).join("c.txt")).unwrap();
        std::fs::rename(a.root.join(old), a.root.join(new)).unwrap();
        a.sync.scan().await.unwrap();
        for _ in 0..2 {
            b.pull_from(&a).await;
            a.pull_from(&b).await;
        }
        let at = format!("{old} -> {new}");
        assert_eq!(vec![format!("{new}/a.txt")], names_of(&b), "{at}");
        assert_eq!(vec![format!("{new}/a.txt")], names_of(&a), "{at}");
    }
}

/// Every folder under `d`'s folder, by relative path.
fn dirs(d: &Device) -> Vec<String> {
    fn walk_dirs(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                out.push(p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
                walk_dirs(root, &p, out);
            }
        }
    }
    let mut out = Vec::new();
    walk_dirs(&d.root, &d.root, &mut out);
    out.sort();
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn a_folder_rename_stopped_anywhere_ends_under_one_of_its_names_with_all_its_files() {
    for (old, new) in FOLDER_RENAMES {
        let make = || folder_renamed_on_a(old, new, Device::new(2));
        let all = |d: &Device, dir: &str| {
            let names = names_of(d);
            names.contains(&format!("{dir}/a.txt")) && names.contains(&format!("{dir}/sub/b.txt"))
        };
        // The folders, not only the files: exactly the folder and its subfolder, under one name.
        let tree = |dir: &str| vec![dir.to_owned(), format!("{dir}/sub")];
        let (a, mut b) = make();
        let plan = FaultPlan::counting();
        b.restart(Some(plan.clone()));
        b.sync.scan().await.unwrap();
        b.sync.pull(&b.link_to(&a)).await.unwrap();
        let points = plan.seen();
        assert!(
            points.contains(&Point::BeforeRename),
            "{old} -> {new}: {points:?}"
        );
        // The rename is the only change to a file or folder here, so it is stopped before and after.
        assert!(points.contains(&Point::AfterRename), "{old} -> {new}: {points:?}");
        for (i, point) in points.iter().enumerate() {
            let (a, mut b) = make();
            let plan = FaultPlan::stop_at(i);
            b.restart(Some(plan.clone()));
            b.sync.scan().await.unwrap();
            let at = format!("{old} -> {new}, {point:?} (#{i})");
            assert!(b.sync.pull(&b.link_to(&a)).await.is_err(), "{at}");
            // At the stop the folder is under one of its names, with everything in it, and no
            // second folder or leftover half exists.
            assert!(all(&b, old) || all(&b, new), "{at}: {:?}", names_of(&b));
            let d = dirs(&b);
            assert!(d == tree(old) || d == tree(new), "{at}: folders {d:?}");
            assert_eq!(a.files().len(), b.files().len(), "{at}: no file lost or doubled");
            b.restart(None);
            for _ in 0..3 {
                b.pull_from(&a).await;
                a.pull_from(&b).await;
            }
            assert!(
                all(&b, new),
                "{at}: the next runs finish the rename: {:?}",
                names_of(&b)
            );
            assert_eq!(tree(new), dirs(&b), "{at}: the folders after the restart");
            assert_eq!(a.files(), b.files(), "{at}");
            assert_eq!(noise(300_000, 5), b.files()[&format!("{new}/a.txt")], "{at}");
            assert_eq!(0, b.bin_count() + a.bin_count(), "{at}: a rename bins nothing");
        }
    }
}

// ---------------------------------------------------------------- names the disk refuses (wasl#18)

// Device A is a Linux or macOS folder, which holds names a Windows folder refuses; creating them
// needs such a disk, so the test runs there. The mapping itself is tested on every OS in wasl-core.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn names_a_windows_disk_refuses_arrive_mapped_and_go_back_under_the_original() {
    let awkward = ["CON", "aux.txt", "report.", "notes ", "ab:c.txt"];
    let (a, b) = (Device::new(1), Device::windows(2));
    for n in awkward {
        a.write(n, format!("from A: {n}").as_bytes());
    }
    a.write("plain.txt", b"plain");
    a.sync.scan().await.unwrap();
    let r = b.pull_from(&a).await;
    assert_eq!(
        (0, 0, awkward.len() + 1),
        (r.deferred, r.rejected, r.received),
        "{r:?}"
    );
    let local = b.files();
    assert_eq!(awkward.len() + 1, local.len(), "{:?}", names_of(&b));
    for (name, body) in &local {
        assert_eq!(
            None,
            wasl_core::names::refusal(name, wasl_core::names::NameRules::drive(true)),
            "{name}"
        );
        assert!(
            body == b"plain" || awkward.iter().any(|n| body == format!("from A: {n}").as_bytes()),
            "{name}"
        );
    }
    // Stable: nothing is fetched or deferred again.
    let again = b.pull_from(&a).await;
    assert_eq!(
        (0, 0, 0),
        (again.deferred, again.received, again.bytes_fetched as usize)
    );
    // Edited here, the files go back to A under their original names, not as new ones.
    for name in local.keys() {
        if name != "plain.txt" {
            b.write(name, b"edited on B");
        }
    }
    b.sync.scan().await.unwrap();
    let back = a.pull_from(&b).await;
    assert_eq!(0, back.deferred, "{back:?}");
    let files = a.files();
    assert_eq!(awkward.len() + 1, files.len(), "{:?}", names_of(&a));
    for n in awkward {
        assert_eq!(
            Some(&b"edited on B".to_vec()),
            files.get(n),
            "{n}: {:?}",
            names_of(&a)
        );
    }
}

// ---------------------------------------------------------------- names and the apply layer (wasl#41)

/// A link to a peer that spells one key another way: `from` as the peer holds it is told to this
/// device as `to` (a name its own disk could not have made), and asked for again as `from`.
struct TamperLink {
    inner: LocalLink,
    from: &'static str,
    to: &'static str,
}

#[async_trait::async_trait]
impl PeerLink for TamperLink {
    fn peer(&self) -> DeviceId {
        self.inner.peer()
    }
    async fn shared(&self) -> wasl_net::PeerResult<Vec<ShareId>> {
        self.inner.shared().await
    }
    async fn changes_since(
        &self,
        p: ShareId,
        since: u64,
        limit: usize,
    ) -> wasl_net::PeerResult<wasl_net::IndexPage> {
        let mut page = self.inner.changes_since(p, since, limit).await?;
        for (k, _) in &mut page.entries {
            if k == self.from {
                *k = self.to.to_owned();
            }
        }
        Ok(page)
    }
    async fn chunk_list(
        &self,
        p: ShareId,
        key: &str,
        b: wasl_chunk::Hash,
    ) -> wasl_net::PeerResult<wasl_chunk::ChunkList> {
        let key = if key == self.to { self.from } else { key };
        self.inner.chunk_list(p, key, b).await
    }
    async fn chunk(&self, p: ShareId, h: wasl_chunk::Hash) -> wasl_net::PeerResult<Vec<u8>> {
        self.inner.chunk(p, h).await
    }
    async fn notify(&self, p: ShareId, seq: u64) -> wasl_net::PeerResult<()> {
        self.inner.notify(p, seq).await
    }
    async fn pair(&self, r: wasl_net::PairRound) -> wasl_net::PeerResult<wasl_net::PairReply> {
        self.inner.pair(r).await
    }
    async fn group_ops(&self, h: Vec<wasl_net::OpId>) -> wasl_net::PeerResult<Vec<wasl_net::GroupOp>> {
        self.inner.group_ops(h).await
    }
    async fn group_changed(&self) -> wasl_net::PeerResult<()> {
        self.inner.group_changed().await
    }
    async fn offers(&self) -> wasl_net::PeerResult<Vec<wasl_net::Offer>> {
        self.inner.offers().await
    }
}

/// The name a Windows disk gives `key` on this device's side of a pull.
fn windows_name(key: &str) -> String {
    key.split('/')
        .map(|p| wasl_core::names::to_local_name_with(p, wasl_core::names::NameRules::device(true)))
        .collect::<Vec<_>>()
        .join("/")
}

/// `b` pulls from `a` through a link that tells one of `a`'s keys under another spelling. Names a
/// Windows host refuses cannot be made in `a`'s own folder there, so the peer's key is what the
/// test changes, on every OS alike.
async fn pull_tampered(b: &Device, a: &Device, from: &'static str, to: &'static str) -> PullReport {
    b.sync.scan().await.unwrap();
    let link = TamperLink {
        inner: b.link_to(a),
        from,
        to,
    };
    b.sync.pull(&link).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_peer_key_with_a_drive_like_prefix_skips_only_that_file() {
    // The key reaches this device from a peer whose disk holds `a:b.txt`; ours may not.
    for windows in [false, true] {
        let a = Device::new(1);
        a.write("plain.txt", b"plain");
        a.write("a_b.txt", b"colon");
        a.sync.scan().await.unwrap();
        let b = if windows {
            Device::windows(2)
        } else {
            Device::new(2)
        };
        b.sync.scan().await.unwrap();
        let link = TamperLink {
            inner: b.link_to(&a),
            from: "a_b.txt",
            to: "a:b.txt",
        };
        let r = b
            .sync
            .pull(&link)
            .await
            .unwrap_or_else(|e| panic!("windows: {windows}: one odd name must not fail the pull: {e}"));
        let at = format!("windows: {windows}: {r:?}");
        assert_eq!(0, r.deferred, "{at}");
        assert_eq!(Some(&b"plain".to_vec()), b.files().get("plain.txt"), "{at}");
        // A Windows disk (and a Windows host's own folder) holds it under a look-alike name; any
        // other disk has no way to and refuses it.
        if windows || cfg!(windows) {
            assert!(r.refused.is_empty(), "{at}");
            assert_eq!(
                Some(&b"colon".to_vec()),
                b.files().get(&windows_name("a:b.txt")),
                "{at}: {:?}",
                names_of(&b)
            );
        } else {
            assert_eq!(vec!["a:b.txt".to_owned()], r.refused, "{at}");
            assert_eq!(1, r.rejected, "{at}");
            assert_eq!(vec!["plain.txt".to_owned()], names_of(&b), "{at}");
        }
        // Skipped for good: the next pull does not read it again.
        let again = b.sync.pull(&link).await.unwrap();
        assert_eq!(
            (0, 0, 0),
            (again.deferred, again.received, again.rejected),
            "{at}: {again:?}"
        );
        assert!(again.refused.is_empty(), "{at}: {again:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn names_a_windows_disk_maps_to_one_name_are_a_clash_until_one_is_renamed() {
    // `REPORT~1.DOC` looks like a short name, so a Windows disk keeps it as `REPORT～1.DOC`,
    // which the peer holds as a different file.
    let (a, b) = (Device::new(1), Device::windows(2));
    a.write("REPORT_1.DOC", b"tilde");
    a.write("REPORT\u{ff5e}1.DOC", b"wide tilde");
    a.write("plain.txt", b"plain");
    a.sync.scan().await.unwrap();
    let r = pull_tampered(&b, &a, "REPORT_1.DOC", "REPORT~1.DOC").await;
    assert_eq!(
        vec!["REPORT~1.DOC".to_owned(), "REPORT\u{ff5e}1.DOC".to_owned()],
        r.clashes,
        "{r:?}"
    );
    assert_eq!(2, r.deferred, "{r:?}");
    assert_eq!(
        vec!["plain.txt".to_owned()],
        names_of(&b),
        "neither of the pair is written: {r:?}"
    );
    // The clash is read again until one is renamed on the peer.
    let still = pull_tampered(&b, &a, "REPORT_1.DOC", "REPORT~1.DOC").await;
    assert_eq!(2, still.deferred, "{still:?}");
    std::fs::rename(a.root.join("REPORT\u{ff5e}1.DOC"), a.root.join("report2.doc")).unwrap();
    a.sync.scan().await.unwrap();
    let r = pull_tampered(&b, &a, "REPORT_1.DOC", "REPORT~1.DOC").await;
    assert_eq!((0, 0), (r.deferred, r.clashes.len()), "{r:?}");
    assert_eq!(3, b.files().len(), "{:?}", names_of(&b));
    assert_eq!(
        Some(&b"tilde".to_vec()),
        b.files().get(&windows_name("REPORT~1.DOC"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_file_under_the_look_alike_of_a_deleted_name_is_not_the_deleted_one() {
    let (a, b) = (Device::new(1), Device::windows(2));
    a.write("REPORT_1.DOC", b"old");
    a.sync.scan().await.unwrap();
    pull_tampered(&b, &a, "REPORT_1.DOC", "REPORT~1.DOC").await;
    assert_eq!(1, b.files().len(), "{:?}", names_of(&b));
    std::fs::remove_file(a.root.join("REPORT_1.DOC")).unwrap();
    a.sync.scan().await.unwrap();
    pull_tampered(&b, &a, "REPORT_1.DOC", "REPORT~1.DOC").await;
    assert!(b.files().is_empty(), "{:?}", names_of(&b));
    // Made on B under the name its disk used to give the deleted file: a new file, found under
    // its own name, not the deleted one's key coming back to life.
    let own = windows_name("REPORT~1.DOC");
    b.write(&own, b"new");
    b.sync.scan().await.unwrap();
    let page = a.link_to(&b).changes_since(SHARE, 0, 100).await.unwrap();
    let state: BTreeMap<&str, bool> = page
        .entries
        .iter()
        .map(|(k, v)| (k.as_str(), v.deleted))
        .collect();
    assert_eq!(Some(&false), state.get(own.as_str()), "{state:?}");
    assert_eq!(
        Some(&true),
        state.get("REPORT~1.DOC"),
        "the deleted key stays deleted: {state:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_the_disk_refuses_is_skipped_and_named_and_not_tried_again() {
    let (a, b) = (Device::new(1), Device::refusing(2));
    a.write("REPORT_1.DOC", b"refused");
    a.write("fine.txt", b"fine");
    a.sync.scan().await.unwrap();
    let r = pull_tampered(&b, &a, "REPORT_1.DOC", "REPORT~1.DOC").await;
    assert_eq!(vec!["REPORT~1.DOC".to_owned()], r.refused, "{r:?}");
    assert_eq!((0, 1), (r.deferred, r.received), "{r:?}");
    assert_eq!(vec!["fine.txt".to_owned()], names_of(&b));
    let again = pull_tampered(&b, &a, "REPORT_1.DOC", "REPORT~1.DOC").await;
    assert_eq!((0, 0), (again.deferred, again.received), "{again:?}");
}
