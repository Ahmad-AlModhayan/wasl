//! Internet reach on the loopback interface: STUN on the QUIC socket, and two endpoints meeting
//! through an in-memory rendezvous (record path and offer/answer path).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use wasl_chunk::{ChunkList, Hash};
use wasl_core::Reach;
use wasl_net::holepunch::{self, PunchConfig, Via};
use wasl_net::rendezvous::{self, Addrs, MemoryRendezvous, PeerRecord, Rendezvous, ReplayGuard};
use wasl_net::stun::{self, StunConfig};
use wasl_net::{
    DeviceId, DeviceKey, Endpoint, IndexPage, PeerError, PeerLink, PeerResult, PeerService, ShareId, Trust,
};

struct Echo;

#[async_trait]
impl PeerService for Echo {
    async fn shared(&self, _: DeviceId) -> Vec<ShareId> {
        vec![ShareId([7; 16])]
    }
    async fn changes_since(&self, _: DeviceId, _: ShareId, _: u64, _: usize) -> PeerResult<IndexPage> {
        Err(PeerError::NotShared)
    }
    async fn chunk_list(&self, _: DeviceId, _: ShareId, _: &str, _: Hash) -> PeerResult<ChunkList> {
        Err(PeerError::NotShared)
    }
    async fn chunk(&self, _: DeviceId, _: ShareId, _: Hash) -> PeerResult<Vec<u8>> {
        Err(PeerError::NotShared)
    }
    async fn notify(&self, _: DeviceId, _: ShareId, _: u64) {}
}

fn key(n: u8) -> DeviceKey {
    DeviceKey::from_seed(&[n; 32])
}

fn trusting(keys: &[&DeviceKey]) -> Trust {
    let pks: Vec<[u8; 32]> = keys.iter().map(|k| k.public()).collect();
    Arc::new(move |pk| pks.contains(pk))
}

fn device(k: &DeviceKey, trust: Trust) -> Endpoint {
    let ep = Endpoint::bind("127.0.0.1:0".parse().unwrap(), k, trust).unwrap();
    ep.serve(Arc::new(Echo));
    ep
}

fn lan_addrs(ep: &Endpoint) -> Addrs {
    Addrs {
        lan: vec![ep.local_addr().unwrap()],
        ..Addrs::default()
    }
}

fn fast() -> PunchConfig {
    PunchConfig {
        lead: Duration::from_millis(300),
        answer_wait: Duration::from_secs(10),
        poll: Duration::from_millis(50),
        ..PunchConfig::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn stun_runs_on_the_quic_socket_and_quic_still_works() {
    let responder = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let stun_addr = responder.local_addr().unwrap();
    tokio::spawn(stun::respond(responder));
    let (a, b) = (key(1), key(2));
    let trust = trusting(&[&a, &b]);
    let server = device(&a, trust.clone());
    let client = device(&b, trust);
    // The address STUN reports is the QUIC socket's own.
    let mapped = server.stun(stun_addr, &StunConfig::default()).await.unwrap();
    assert_eq!(mapped, server.local_addr().unwrap());
    // Several at once, while a connection is open and working.
    let link = client
        .connect(server.local_addr().unwrap(), Some(a.id()))
        .await
        .unwrap();
    let cfg = StunConfig::default();
    let (r1, r2, shared) = tokio::join!(
        server.stun(stun_addr, &cfg),
        client.stun(stun_addr, &cfg),
        link.shared()
    );
    assert_eq!(r1.unwrap(), server.local_addr().unwrap());
    assert_eq!(r2.unwrap(), client.local_addr().unwrap());
    assert_eq!(shared.unwrap(), vec![ShareId([7; 16])]);
    // Stray STUN messages reaching the QUIC socket are ignored, and QUIC goes on.
    let stray = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let tx = stun::new_transaction();
    for junk in [
        stun::encode_request(&tx),
        stun::encode_success(&tx, "6.6.6.6:6".parse().unwrap()),
    ] {
        stray.send_to(&junk, server.local_addr().unwrap()).await.unwrap();
    }
    let mut buf = [0u8; 64];
    assert!(
        tokio::time::timeout(Duration::from_millis(200), stray.recv_from(&mut buf))
            .await
            .is_err(),
        "the endpoint never answers STUN"
    );
    assert_eq!(link.shared().await.unwrap(), vec![ShareId([7; 16])]);
}

#[tokio::test(flavor = "multi_thread")]
async fn gather_asks_stun_only_with_internet_reach() {
    // A responder that counts what it hears.
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let server = socket.local_addr().unwrap();
    let heard = Arc::new(AtomicUsize::new(0));
    let count = heard.clone();
    tokio::spawn(async move {
        let mut buf = [0u8; 1500];
        while let Ok((n, from)) = socket.recv_from(&mut buf).await {
            count.fetch_add(1, Ordering::SeqCst);
            if let Ok(stun::Message::BindingRequest { tx }) = stun::decode(&buf[..n]) {
                let _ = socket.send_to(&stun::encode_success(&tx, from), from).await;
            }
        }
    });
    let k = key(1);
    let ep = device(&k, trusting(&[&k]));
    let lan = holepunch::gather(&ep, Reach::Lan, &[server], None, &StunConfig::default()).await;
    assert!(lan.stun.is_none() && lan.ipv6.is_empty() && lan.mapped.is_none());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        heard.load(Ordering::SeqCst),
        0,
        "reach lan contacted the STUN server"
    );
    // With internet reach it asks (a loopback answer is not public, so it is not published).
    let net = holepunch::gather(
        &ep,
        Reach::LanAndInternet,
        &[server],
        None,
        &StunConfig::default(),
    )
    .await;
    assert!(net.stun.is_none());
    assert!(heard.load(Ordering::SeqCst) >= 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_published_record_gives_a_direct_connection() {
    let (a, b) = (key(1), key(2));
    let trust = trusting(&[&a, &b]);
    let ea = device(&a, trust.clone());
    let eb = device(&b, trust.clone());
    let rv = MemoryRendezvous::new();
    let record = PeerRecord::new(
        &b,
        lan_addrs(&eb),
        rendezvous::unix_now(),
        Duration::from_secs(300),
    )
    .unwrap();
    rendezvous::publish(&rv, &record).await.unwrap();
    let (link, via) = holepunch::dial(
        &ea,
        Reach::LanAndInternet,
        &rv,
        &a,
        &lan_addrs(&ea),
        b.id(),
        trust.as_ref(),
        &fast(),
    )
    .await
    .unwrap();
    assert_eq!(via.via, Via::Lan);
    assert_eq!(link.peer(), b.id());
    assert_eq!(link.remote_addr(), eb.local_addr().unwrap());
    assert!(rv.list("offers/").await.unwrap().is_empty(), "no offer needed");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_offer_and_its_answer_connect_both_ways() {
    let (a, b) = (key(1), key(2));
    let trust = trusting(&[&a, &b]);
    let ea = device(&a, trust.clone());
    let eb = device(&b, trust.clone());
    let rv = Arc::new(MemoryRendezvous::new());
    // b polls the rendezvous while active.
    let b_id = b.id();
    let answerer = {
        let (rv, trust, b_addrs) = (rv.clone(), trust.clone(), lan_addrs(&eb));
        tokio::spawn(async move {
            let mut guard = ReplayGuard::new(256);
            loop {
                let got = holepunch::answer(
                    &eb,
                    Reach::LanAndInternet,
                    rv.as_ref(),
                    &b,
                    &b_addrs,
                    trust.as_ref(),
                    &|_: &wasl_net::DeviceId| true,
                    &mut guard,
                    &mut holepunch::AnswerLimits::default(),
                    &fast(),
                )
                .await
                .unwrap();
                if let Some((from, result)) = got.into_iter().next() {
                    let (link, _) = result.unwrap();
                    return (from, link.peer(), eb);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
    };
    let (link, via) = holepunch::dial(
        &ea,
        Reach::LanAndInternet,
        rv.as_ref(),
        &a,
        &lan_addrs(&ea),
        b_id,
        trust.as_ref(),
        &fast(),
    )
    .await
    .unwrap();
    assert_eq!(via.via, Via::Lan);
    assert_eq!(link.peer(), b_id);
    assert_eq!(link.shared().await.unwrap(), vec![ShareId([7; 16])]);
    let (from, peer, _eb) = answerer.await.unwrap();
    assert_eq!(from, a.id());
    assert_eq!(peer, a.id());
    // Both sides cleaned up.
    assert!(rv.list("offers/").await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_record_signed_by_an_outsider_is_not_dialed() {
    let (a, b, outsider) = (key(1), key(2), key(3));
    let trust = trusting(&[&a, &b]);
    let ea = device(&a, trust.clone());
    // Where the outsider wants a's packets to go.
    let trap = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let rv = MemoryRendezvous::new();
    // The outsider publishes a record under b's name pointing at the trap.
    let mut record = PeerRecord::new(
        &outsider,
        Addrs {
            lan: vec![trap.local_addr().unwrap()],
            ..Addrs::default()
        },
        rendezvous::unix_now(),
        Duration::from_secs(300),
    )
    .unwrap();
    record.device = b.id();
    rv.put(&PeerRecord::name(&b.id()), record.to_json())
        .await
        .unwrap();
    let cfg = PunchConfig {
        answer_wait: Duration::from_millis(200),
        ..fast()
    };
    let result = holepunch::dial(
        &ea,
        Reach::LanAndInternet,
        &rv,
        &a,
        &lan_addrs(&ea),
        b.id(),
        trust.as_ref(),
        &cfg,
    )
    .await;
    assert!(
        matches!(result, Err(holepunch::PunchError::NoAnswer(_))),
        "{result:?}"
    );
    let mut buf = [0u8; 1500];
    assert!(
        tokio::time::timeout(Duration::from_millis(100), trap.recv_from(&mut buf))
            .await
            .is_err(),
        "a sent packets to an address from an untrusted record"
    );
}
