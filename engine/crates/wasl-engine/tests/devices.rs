//! Device sync end to end on real folders: scans, pulls between devices (in process and over
//! QUIC), chunked deltas, moves, deletions, conflicts and a local edit racing a pull.

mod common;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use wasl_core::Route;
use wasl_engine::{Clock, DeviceService, DeviceSync, PullReport};
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

struct Device {
    key: DeviceKey,
    root: PathBuf,
    store: SharedStore,
    sync: DeviceSync,
    service: Arc<DeviceService>,
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

    fn with_folding(n: u8, folding: Option<common::Folding>) -> Device {
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
            None => Arc::new(StdFs::new(&root, &bin)),
        };
        let key = DeviceKey::from_seed(&[n; 32]);
        let clock = Arc::new(TestClock(AtomicI64::new(1_800_000_000_000)));
        let sync = DeviceSync::new(store.clone(), fs.clone(), pair, SHARE, key.id(), clock);
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
            _dir: dir,
        }
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
