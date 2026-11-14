//! The peer requests over real QUIC connections on the loopback interface.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use wasl_chunk::{Chunk, ChunkList, Hash};
use wasl_core::nway::Version;
use wasl_core::vv::VersionVector;
use wasl_net::peer::MAX_CHUNK_BYTES;
use wasl_net::{
    DeviceId, DeviceKey, Endpoint, IndexPage, LocalLink, PeerError, PeerLink, PeerResult, PeerService,
    QuicError, ShareId, Trust,
};

const PAIR: ShareId = ShareId([1; 16]);
const OTHER_PAIR: ShareId = ShareId([2; 16]);
const ENTRIES: u64 = 2500;

/// One shared pair held in memory, answering only the devices it is shared with.
struct Fake {
    shared_with: HashSet<DeviceId>,
    entries: Vec<(u64, String, Version)>,
    lists: HashMap<(String, Hash), ChunkList>,
    chunks: HashMap<Hash, Vec<u8>>,
    asked_by: Mutex<Vec<DeviceId>>,
    notified: Mutex<Vec<(DeviceId, ShareId, u64)>>,
}

fn version(n: u64) -> Version {
    Version {
        is_dir: false,
        deleted: n.is_multiple_of(7),
        hash: Some(format!("{n:064x}")),
        size: n,
        mtime: n as i64,
        vv: VersionVector::new().ticked(n),
        author: n,
    }
}

fn big_chunk() -> Vec<u8> {
    (0..MAX_CHUNK_BYTES).map(|i| (i % 251) as u8).collect()
}

impl Fake {
    fn new(shared_with: &[DeviceId]) -> Self {
        let entries = (1..=ENTRIES)
            .map(|n| (n, format!("dir/file-{n}"), version(n)))
            .collect();
        let chunk = big_chunk();
        let hash = *blake3::hash(&chunk).as_bytes();
        let list = ChunkList {
            size: chunk.len() as u64,
            blake3: hash,
            chunks: vec![Chunk {
                offset: 0,
                len: chunk.len() as u32,
                hash,
            }],
        };
        Self {
            shared_with: shared_with.iter().copied().collect(),
            entries,
            lists: HashMap::from([(("big.bin".to_string(), hash), list)]),
            chunks: HashMap::from([
                (hash, chunk),
                (*blake3::hash(b"small").as_bytes(), b"small".to_vec()),
            ]),
            asked_by: Mutex::new(Vec::new()),
            notified: Mutex::new(Vec::new()),
        }
    }

    fn check(&self, peer: DeviceId, pair: ShareId) -> PeerResult<()> {
        self.asked_by.lock().unwrap().push(peer);
        if pair == PAIR && self.shared_with.contains(&peer) {
            Ok(())
        } else {
            Err(PeerError::NotShared)
        }
    }
}

#[async_trait]
impl PeerService for Fake {
    async fn shared(&self, peer: DeviceId) -> Vec<ShareId> {
        self.asked_by.lock().unwrap().push(peer);
        if self.shared_with.contains(&peer) {
            vec![PAIR]
        } else {
            vec![]
        }
    }

    async fn changes_since(
        &self,
        peer: DeviceId,
        pair: ShareId,
        since: u64,
        limit: usize,
    ) -> PeerResult<IndexPage> {
        self.check(peer, pair)?;
        let after: Vec<_> = self.entries.iter().filter(|(seq, ..)| *seq > since).collect();
        let page: Vec<_> = after.iter().take(limit).collect();
        Ok(IndexPage {
            entries: page.iter().map(|(_, k, v)| (k.clone(), v.clone())).collect(),
            next_since: page.last().map_or(since, |(seq, ..)| *seq),
            more: after.len() > page.len(),
        })
    }

    async fn chunk_list(
        &self,
        peer: DeviceId,
        pair: ShareId,
        key: &str,
        blake3: Hash,
    ) -> PeerResult<ChunkList> {
        self.check(peer, pair)?;
        self.lists
            .get(&(key.to_string(), blake3))
            .cloned()
            .ok_or(PeerError::Gone)
    }

    async fn chunk(&self, peer: DeviceId, pair: ShareId, hash: Hash) -> PeerResult<Vec<u8>> {
        self.check(peer, pair)?;
        self.chunks.get(&hash).cloned().ok_or(PeerError::Gone)
    }

    async fn notify(&self, peer: DeviceId, pair: ShareId, seq: u64) {
        self.asked_by.lock().unwrap().push(peer);
        self.notified.lock().unwrap().push((peer, pair, seq));
    }
}

fn key(n: u8) -> DeviceKey {
    DeviceKey::from_seed(&[n; 32])
}

fn trusting(keys: &[&DeviceKey]) -> Trust {
    let keys: HashSet<[u8; 32]> = keys.iter().map(|k| k.public()).collect();
    Arc::new(move |pk| keys.contains(pk))
}

fn loopback() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

/// A serving endpoint for `key` with the fake service, trusting `trust`.
fn server(key: &DeviceKey, trust: Trust, service: Arc<Fake>) -> Endpoint {
    let endpoint = Endpoint::bind(loopback(), key, trust).unwrap();
    endpoint.serve(service);
    endpoint
}

/// Every [`PeerLink`] method against the fake, the same for any transport.
async fn exercise(link: &dyn PeerLink) {
    assert_eq!(link.shared().await.unwrap(), vec![PAIR]);

    // Paging: everything arrives once, in order, in pages no larger than asked.
    let mut since = 0;
    let mut keys = Vec::new();
    let mut pages = 0;
    loop {
        let page = link.changes_since(PAIR, since, 1000).await.unwrap();
        assert!(page.entries.len() <= 1000);
        pages += 1;
        keys.extend(page.entries.iter().map(|(k, _)| k.clone()));
        since = page.next_since;
        if !page.more {
            break;
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(keys.len() as u64, ENTRIES);
    assert_eq!(keys.last().unwrap(), &format!("dir/file-{ENTRIES}"));
    let page = link.changes_since(PAIR, 10, 1).await.unwrap();
    assert_eq!(page.entries, vec![("dir/file-11".to_string(), version(11))]);
    // A limit above the maximum is lowered to it.
    let page = link.changes_since(PAIR, 0, usize::MAX).await.unwrap();
    assert_eq!(page.entries.len(), wasl_net::peer::MAX_PAGE_ENTRIES);
    assert!(page.more);

    let chunk = big_chunk();
    let hash = *blake3::hash(&chunk).as_bytes();
    let list = link.chunk_list(PAIR, "big.bin", hash).await.unwrap();
    assert_eq!(list.size, MAX_CHUNK_BYTES as u64);
    assert_eq!(list.chunks[0].hash, hash);
    assert_eq!(
        link.chunk_list(PAIR, "big.bin", [0; 32]).await,
        Err(PeerError::Gone)
    );

    let got = link.chunk(PAIR, hash).await.unwrap();
    assert!(got == chunk, "the 16 MiB chunk arrived changed");
    let small = *blake3::hash(b"small").as_bytes();
    assert_eq!(link.chunk(PAIR, small).await.unwrap(), b"small");
    assert_eq!(link.chunk(PAIR, [0; 32]).await, Err(PeerError::Gone));

    link.notify(PAIR, 77).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn every_request_works_over_quic_and_the_server_knows_who_asks() {
    let (a, b) = (key(1), key(2));
    let fake = Arc::new(Fake::new(&[b.id()]));
    let srv = server(&a, trusting(&[&b]), fake.clone());
    let client = Endpoint::bind(loopback(), &b, trusting(&[&a])).unwrap();
    let link = client
        .connect(srv.local_addr().unwrap(), Some(a.id()))
        .await
        .unwrap();
    assert_eq!(link.peer(), a.id());
    assert_eq!(client.id(), b.id());

    exercise(&link).await;

    let asked = fake.asked_by.lock().unwrap().clone();
    assert!(!asked.is_empty());
    assert!(asked.iter().all(|id| *id == b.id()), "{asked:?}");
    assert_eq!(*fake.notified.lock().unwrap(), vec![(b.id(), PAIR, 77)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_local_link_behaves_the_same() {
    let b = key(2);
    let fake = Arc::new(Fake::new(&[b.id()]));
    let link = LocalLink::new(b.id(), key(1).id(), fake);
    exercise(&link).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn requests_run_concurrently_on_one_connection() {
    let (a, b) = (key(1), key(2));
    let srv = server(&a, trusting(&[&b]), Arc::new(Fake::new(&[b.id()])));
    let client = Endpoint::bind(loopback(), &b, trusting(&[&a])).unwrap();
    let link = Arc::new(client.connect(srv.local_addr().unwrap(), None).await.unwrap());
    let tasks: Vec<_> = (0..40)
        .map(|i| {
            let link = link.clone();
            tokio::spawn(async move { link.changes_since(PAIR, i, 5).await.unwrap().entries.len() })
        })
        .collect();
    for t in tasks {
        assert_eq!(t.await.unwrap(), 5);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pair_that_is_not_shared_is_refused() {
    let (a, b, c) = (key(1), key(2), key(3));
    // `c` is trusted by the transport but the pair is shared with `b` only.
    let srv = server(&a, trusting(&[&b, &c]), Arc::new(Fake::new(&[b.id()])));

    let bc = Endpoint::bind(loopback(), &b, trusting(&[&a])).unwrap();
    let link = bc.connect(srv.local_addr().unwrap(), None).await.unwrap();
    assert_eq!(
        link.changes_since(OTHER_PAIR, 0, 10).await,
        Err(PeerError::NotShared)
    );
    assert_eq!(link.chunk(OTHER_PAIR, [0; 32]).await, Err(PeerError::NotShared));
    assert_eq!(
        link.chunk_list(OTHER_PAIR, "x", [0; 32]).await,
        Err(PeerError::NotShared)
    );

    let cc = Endpoint::bind(loopback(), &c, trusting(&[&a])).unwrap();
    let link = cc.connect(srv.local_addr().unwrap(), None).await.unwrap();
    assert_eq!(link.shared().await.unwrap(), vec![]);
    assert_eq!(link.changes_since(PAIR, 0, 10).await, Err(PeerError::NotShared));
    assert_eq!(link.chunk(PAIR, [0; 32]).await, Err(PeerError::NotShared));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_untrusted_client_cannot_connect() {
    let (a, b, stranger) = (key(1), key(2), key(9));
    let fake = Arc::new(Fake::new(&[b.id(), stranger.id()]));
    let srv = server(&a, trusting(&[&b]), fake.clone());
    let client = Endpoint::bind(loopback(), &stranger, trusting(&[&a])).unwrap();
    let result = client.connect(srv.local_addr().unwrap(), None).await;
    // With TLS 1.3 the client can finish its side of the handshake before the server has checked
    // the client's certificate: the refusal then arrives as the connection closing on the first
    // request instead of as a connect error. Either way it fails and the server answered nothing.
    assert!(
        matches!(result, Err(QuicError::Connect(_)) | Err(QuicError::Protocol(_))),
        "{result:?}"
    );
    assert!(fake.asked_by.lock().unwrap().is_empty());

    // The server keeps accepting after the failed handshake.
    let good = Endpoint::bind(loopback(), &b, trusting(&[&a])).unwrap();
    let link = good.connect(srv.local_addr().unwrap(), None).await.unwrap();
    assert_eq!(link.shared().await.unwrap(), vec![PAIR]);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_untrusted_server_is_refused() {
    let (b, impostor) = (key(2), key(9));
    let fake = Arc::new(Fake::new(&[b.id()]));
    let srv = server(&impostor, trusting(&[&b]), fake.clone());
    let client = Endpoint::bind(loopback(), &b, trusting(&[&key(1)])).unwrap();
    let result = client.connect(srv.local_addr().unwrap(), None).await;
    assert!(matches!(result, Err(QuicError::Connect(_))), "{result:?}");
    assert!(fake.asked_by.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn connecting_to_the_wrong_device_fails() {
    let (a, b, c) = (key(1), key(2), key(3));
    let srv = server(&a, trusting(&[&b]), Arc::new(Fake::new(&[b.id()])));
    // `c` would be trusted, but the device at that address is `a`.
    let client = Endpoint::bind(loopback(), &b, trusting(&[&a, &c])).unwrap();
    let addr = srv.local_addr().unwrap();
    assert!(client.connect(addr, Some(c.id())).await.is_err());
    assert!(client.connect(addr, Some(a.id())).await.is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn pairing_trust_accepts_any_key() {
    let (a, b) = (key(1), key(2));
    let anyone: Trust = Arc::new(|_| true);
    let srv = server(&a, anyone.clone(), Arc::new(Fake::new(&[])));
    let client = Endpoint::bind(loopback(), &b, anyone).unwrap();
    let link = client.connect(srv.local_addr().unwrap(), None).await.unwrap();
    assert_eq!(link.peer(), a.id());
    assert_eq!(link.shared().await.unwrap(), vec![]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_closed_endpoint_stops_answering() {
    let (a, b) = (key(1), key(2));
    let srv = server(&a, trusting(&[&b]), Arc::new(Fake::new(&[b.id()])));
    let client = Endpoint::bind(loopback(), &b, trusting(&[&a])).unwrap();
    let link = client.connect(srv.local_addr().unwrap(), None).await.unwrap();
    srv.close();
    assert!(matches!(link.shared().await, Err(PeerError::Connection(_))));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_removed_while_connected_loses_its_connection() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let (a, b) = (key(1), key(2));
    let member = Arc::new(AtomicBool::new(true));
    let b_pk = b.public();
    let flag = member.clone();
    let trust: Trust = Arc::new(move |pk| *pk == b_pk && flag.load(Ordering::SeqCst));
    let srv = server(&a, trust, Arc::new(Fake::new(&[b.id()])));
    let client = Endpoint::bind(loopback(), &b, trusting(&[&a])).unwrap();
    let link = client.connect(srv.local_addr().unwrap(), None).await.unwrap();
    assert_eq!(link.shared().await.unwrap(), vec![PAIR]);

    member.store(false, Ordering::SeqCst);
    assert!(
        link.shared().await.is_err(),
        "the open connection no longer answers"
    );
}
