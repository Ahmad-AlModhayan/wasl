//! Devices talking to each other over QUIC (docs/DEVICE_SYNC.md §6, FR-N4).
//!
//! Each device presents a self-signed X.509 certificate wrapping its Ed25519 device key, and both
//! sides present one (mutual TLS 1.3, ALPN `wasl/1`). There are no certificate authorities, names
//! or validity dates: a certificate is accepted when the key inside it is one the caller's
//! [`Trust`] accepts (the group's devices, or any key while pairing), and the TLS handshake
//! signature is still verified so the peer proves it holds that key. The verified key gives the
//! peer's [`DeviceId`], which the server passes to every [`PeerService`] call and which
//! [`QuicLink::peer`] reports.
//!
//! TLS session resumption is off on both sides: a resumed session would skip the certificate
//! check, so a device removed from the group could come back on an old ticket.
//!
//! One connection per peer. Its first stream carries the [`Hello`] exchange; every request after
//! that gets its own bidirectional stream ([`crate::proto`]). A failed handshake or a broken
//! connection ends only that connection; the accept loop keeps going.
//!
//! The endpoint's UDP socket also carries this device's STUN requests ([`Endpoint::stun`]), so the
//! public address STUN reports is the one peers reach QUIC at (see [`crate::stun`]). The QUIC bit
//! is not greased, so QUIC packets never look like STUN (RFC 9443).

use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{Connection, RecvStream, SendStream, VarInt};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{CertificateError, DigitallySignedStruct, DistinguishedName, SignatureScheme};
use wasl_chunk::{ChunkList, Hash};
use zeroize::Zeroizing;

use crate::compress::{self, Packed};
use crate::group::{GroupOp, MAX_OPS, OpId};
use crate::identity::{DeviceId, DeviceKey, parse_public_key};
use crate::peer::{
    IndexPage, MAX_GROUP_HEADS, MAX_OFFERS, MAX_PAGE_ENTRIES, Offer, PairReply, PairRound, PeerError,
    PeerLink, PeerResult, PeerService, ShareId,
};
use crate::proto::{
    self, Hello, MAX_FRAME_BYTES, MAX_REQUEST_BYTES, PROTOCOL_VERSION, ProtoError, Request, Response,
};
use crate::stun::{self, EndpointWire, Mux, MuxSocket, StunConfig, StunError};

/// The TLS application protocol name.
pub const ALPN: &[u8] = b"wasl/1";
/// Requests one peer may have open at once on a connection.
pub const MAX_STREAMS: u32 = 32;

/// The name a client puts in its TLS hello. Nobody checks it; certificates carry keys, not names.
const SERVER_NAME: &str = "wasl";
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const KEEP_ALIVE: Duration = Duration::from_secs(10);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest wait for one answer; a full-size chunk over a slow link fits well within it.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const CLOSE_VERSION: u32 = 1;
const CLOSE_PROTOCOL: u32 = 2;
const CLOSE_UNTRUSTED: u32 = 3;

/// PKCS#8 v1 for an Ed25519 key: this prefix, then the 32-byte seed (RFC 8410).
const ED25519_PKCS8_PREFIX: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];
/// SubjectPublicKeyInfo for an Ed25519 key: this prefix, then the 32-byte public key (RFC 8410).
const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// Decides whether a peer's Ed25519 public key may connect, e.g. `move |pk| group.trusts_key(pk)`.
pub type Trust = Arc<dyn Fn(&[u8; 32]) -> bool + Send + Sync>;

/// Why the transport could not start or reach a peer.
#[derive(Debug, thiserror::Error)]
pub enum QuicError {
    #[error("could not open the socket: {0}")]
    Bind(#[source] std::io::Error),
    #[error("TLS setup failed: {0}")]
    Tls(String),
    #[error("could not connect: {0}")]
    Connect(String),
    #[error("reached {found}, not {expected}")]
    WrongPeer { expected: DeviceId, found: DeviceId },
    #[error("the peer speaks protocol version {0}, this device speaks {PROTOCOL_VERSION}")]
    Version(u8),
    #[error(transparent)]
    Protocol(#[from] ProtoError),
}

impl From<QuicError> for PeerError {
    fn from(e: QuicError) -> Self {
        match e {
            QuicError::Protocol(p) => p.into(),
            QuicError::Version(_) => PeerError::Protocol(e.to_string()),
            other => PeerError::Connection(other.to_string()),
        }
    }
}

/// A device's QUIC endpoint: serves its [`PeerService`] and connects to peers.
///
/// Must be created inside a Tokio runtime.
pub struct Endpoint {
    endpoint: quinn::Endpoint,
    socket: Arc<dyn quinn::AsyncUdpSocket>,
    mux: Arc<Mux>,
    id: DeviceId,
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
    trust: Trust,
    /// The most work this device does to compress a chunk it sends (lowered on battery).
    compression_cap: Arc<AtomicU8>,
    /// Compressions running for peers; see [`compress::MAX_PARALLEL`].
    compress_slots: Arc<tokio::sync::Semaphore>,
}

impl Endpoint {
    /// Opens a UDP socket at `addr` (port 0 picks one) for the device `key`, accepting peers that
    /// `trust` accepts.
    pub fn bind(addr: SocketAddr, key: &DeviceKey, trust: Trust) -> Result<Self, QuicError> {
        let (cert, private) = certificate(key)?;
        let server = server_config(cert.clone(), private.clone_key(), Verifier::new(trust.clone()))?;
        let runtime: Arc<dyn quinn::Runtime> = Arc::new(quinn::TokioRuntime);
        let udp = std::net::UdpSocket::bind(addr).map_err(QuicError::Bind)?;
        let inner = runtime.wrap_udp_socket(udp).map_err(QuicError::Bind)?;
        let (socket, mux) = MuxSocket::new(inner.clone());
        let mut config = quinn::EndpointConfig::default();
        config.grease_quic_bit(false);
        let endpoint = quinn::Endpoint::new_with_abstract_socket(config, Some(server), socket, runtime)
            .map_err(QuicError::Bind)?;
        Ok(Self {
            endpoint,
            socket: inner,
            mux,
            id: key.id(),
            cert,
            key: private,
            trust,
            compression_cap: Arc::new(AtomicU8::new(compress::MAX_LEVEL)),
            compress_slots: Arc::new(tokio::sync::Semaphore::new(compress::MAX_PARALLEL)),
        })
    }

    /// Caps the zstd level this device uses for chunks it sends, whatever peers ask for (0 sends
    /// raw bytes). A phone on battery sets [`compress::LEVEL_LAN`].
    pub fn set_compression_cap(&self, level: u8) {
        self.compression_cap
            .store(level.min(compress::MAX_LEVEL), Ordering::Relaxed);
    }

    /// This device's ID.
    pub fn id(&self) -> DeviceId {
        self.id
    }

    /// The address the socket is bound to.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.endpoint.local_addr()
    }

    /// Answers peers with `service` until [`Endpoint::close`]. Spawns the accept loop and returns.
    pub fn serve(&self, service: Arc<dyn PeerService>) {
        let endpoint = self.endpoint.clone();
        let trust = self.trust.clone();
        let cap = self.compression_cap.clone();
        let slots = self.compress_slots.clone();
        tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                tokio::spawn(serve_connection(
                    incoming,
                    service.clone(),
                    trust.clone(),
                    cap.clone(),
                    slots.clone(),
                ));
            }
        });
    }

    /// Connects to the device at `addr`. With `expected`, any other device is refused during the
    /// handshake, before this device shows its own certificate.
    pub async fn connect(&self, addr: SocketAddr, expected: Option<DeviceId>) -> Result<QuicLink, QuicError> {
        self.dial(addr, expected, self.trust.clone()).await
    }

    /// Connects to the device at `addr`, accepting only a peer `trust` accepts instead of the
    /// endpoint's own trust. The peer's certificate is checked before this device shows its own,
    /// so a peer `trust` refuses never sees who this is (security review 2026-10: discovery dials
    /// with members only, even while a pairing code lets anyone in).
    pub async fn connect_trusting(&self, addr: SocketAddr, trust: Trust) -> Result<QuicLink, QuicError> {
        self.dial(addr, None, trust).await
    }

    async fn dial(
        &self,
        addr: SocketAddr,
        expected: Option<DeviceId>,
        trust: Trust,
    ) -> Result<QuicLink, QuicError> {
        let trust = match expected {
            None => trust,
            Some(id) => {
                Arc::new(move |pk: &[u8; 32]| DeviceId::from_public_key(pk) == id && trust(pk)) as Trust
            }
        };
        let config = client_config(self.cert.clone(), self.key.clone_key(), Verifier::new(trust))?;
        let conn = self
            .endpoint
            .connect_with(config, addr, SERVER_NAME)
            .map_err(|e| QuicError::Connect(e.to_string()))?
            .await
            .map_err(|e| QuicError::Connect(e.to_string()))?;
        let peer =
            peer_id(&conn).ok_or_else(|| QuicError::Connect("the peer sent no usable certificate".into()))?;
        if let Some(expected) = expected.filter(|e| *e != peer) {
            conn.close(VarInt::from_u32(CLOSE_PROTOCOL), b"wrong device");
            return Err(QuicError::WrongPeer {
                expected,
                found: peer,
            });
        }
        match tokio::time::timeout(HELLO_TIMEOUT, hello(&conn)).await {
            Ok(Ok(())) => Ok(QuicLink {
                conn,
                peer,
                level: Arc::new(AtomicU8::new(compress::LEVEL_INTERNET)),
                stats: Arc::new(WireStats::default()),
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                conn.close(VarInt::from_u32(CLOSE_PROTOCOL), b"no hello");
                Err(QuicError::Connect("the peer did not answer the hello".into()))
            }
        }
    }

    /// Asks the STUN `server` for the public address of this endpoint's socket: where peers on
    /// the internet may reach it (behind an endpoint-independent NAT). Sent on the QUIC socket
    /// itself; see [`crate::stun`].
    pub async fn stun(&self, server: SocketAddr, cfg: &StunConfig) -> Result<SocketAddr, StunError> {
        let tx = stun::new_transaction();
        let mut wire = EndpointWire::new(self.socket.clone(), &self.mux, tx)?;
        stun::transact(&mut wire, server, tx, cfg).await
    }

    /// Closes every connection and stops serving.
    pub fn close(&self) {
        self.endpoint.close(VarInt::from_u32(0), b"closing");
    }
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Endpoint")
            .field("id", &self.id)
            .field("addr", &self.endpoint.local_addr().ok())
            .finish_non_exhaustive()
    }
}

/// A connection to one peer, asking it through [`PeerLink`].
#[derive(Debug, Clone)]
pub struct QuicLink {
    conn: Connection,
    peer: DeviceId,
    /// The zstd level asked for chunks (0: raw only). Set from the path: see [`compress`].
    level: Arc<AtomicU8>,
    stats: Arc<WireStats>,
}

/// Chunk bytes received on a link: as used, and as they travelled.
#[derive(Debug, Default)]
pub struct WireStats {
    raw: AtomicU64,
    wire: AtomicU64,
}

impl QuicLink {
    /// Sets the zstd level asked for chunks from now on (0: raw bytes only).
    pub fn set_compression(&self, level: u8) {
        self.level
            .store(level.min(compress::MAX_LEVEL), Ordering::Relaxed);
    }

    /// Chunk bytes received so far: (raw, on the wire).
    pub fn chunk_bytes(&self) -> (u64, u64) {
        (
            self.stats.raw.load(Ordering::Relaxed),
            self.stats.wire.load(Ordering::Relaxed),
        )
    }

    /// The peer's address.
    pub fn remote_addr(&self) -> SocketAddr {
        self.conn.remote_address()
    }

    /// Closes the connection.
    pub fn close(&self) {
        self.conn.close(VarInt::from_u32(0), b"closing");
    }

    /// One request and its answer, bounded: a peer whose connection stays alive (keep-alives keep it
    /// from idling out) but that never answers must not hold a sync forever (perf audit 2026-10-29).
    async fn request(&self, req: &Request) -> PeerResult<Response> {
        tokio::time::timeout(REQUEST_TIMEOUT, self.exchange(req))
            .await
            .map_err(|_| PeerError::Connection(format!("no answer in {} s", REQUEST_TIMEOUT.as_secs())))?
    }

    async fn exchange(&self, req: &Request) -> PeerResult<Response> {
        let (mut send, mut recv) = self
            .conn
            .open_bi()
            .await
            .map_err(|e| PeerError::Connection(e.to_string()))?;
        proto::write_frame(&mut send, &proto::encode(req)).await?;
        send.finish().map_err(|e| PeerError::Connection(e.to_string()))?;
        let body = proto::read_frame(&mut recv, MAX_FRAME_BYTES).await?;
        match proto::decode_response(&body)? {
            Response::Error(e) => Err(e),
            resp => Ok(resp),
        }
    }
}

fn unexpected(resp: &Response) -> PeerError {
    let name = match resp {
        Response::Shared(_) => "Shared",
        Response::Page(_) => "Page",
        Response::ChunkList(_) => "ChunkList",
        Response::Chunk(_) => "Chunk",
        Response::ChunkZstd { .. } => "ChunkZstd",
        Response::Notified => "Notified",
        Response::Pair(_) => "Pair",
        Response::Group(_) => "Group",
        Response::Offers(_) => "Offers",
        Response::Error(_) => "Error",
    };
    PeerError::Protocol(format!("unexpected {name} response"))
}

#[async_trait]
impl PeerLink for QuicLink {
    fn peer(&self) -> DeviceId {
        self.peer
    }

    async fn shared(&self) -> PeerResult<Vec<ShareId>> {
        match self.request(&Request::Shared).await? {
            Response::Shared(pairs) => Ok(pairs),
            other => Err(unexpected(&other)),
        }
    }

    async fn changes_since(&self, pair: ShareId, since: u64, limit: usize) -> PeerResult<IndexPage> {
        let limit = limit.min(MAX_PAGE_ENTRIES);
        let req = Request::ChangesSince {
            pair,
            since,
            limit: limit as u32,
        };
        match self.request(&req).await? {
            Response::Page(page) if page.entries.len() > limit => {
                Err(PeerError::Protocol("page larger than asked".into()))
            }
            Response::Page(page) => Ok(page),
            other => Err(unexpected(&other)),
        }
    }

    async fn chunk_list(&self, pair: ShareId, key: &str, blake3: Hash) -> PeerResult<ChunkList> {
        let req = Request::ChunkList {
            pair,
            key: key.to_string(),
            blake3,
        };
        if let Err(e) = req.check() {
            return Err(e.into());
        }
        match self.request(&req).await? {
            Response::ChunkList(list) if list.blake3 != blake3 => {
                Err(PeerError::Protocol("chunk list for other content".into()))
            }
            Response::ChunkList(list) => Ok(list),
            other => Err(unexpected(&other)),
        }
    }

    async fn chunk(&self, pair: ShareId, hash: Hash) -> PeerResult<Vec<u8>> {
        let zstd = self.level.load(Ordering::Relaxed);
        let (raw, wire) = match self.request(&Request::Chunk { pair, hash, zstd }).await? {
            Response::Chunk(bytes) => {
                let n = bytes.len();
                (bytes, n)
            }
            Response::ChunkZstd { len, data } if zstd > 0 => {
                let n = data.len();
                let raw = tokio::task::spawn_blocking(move || compress::unpack(len, &data))
                    .await
                    .map_err(|e| PeerError::Connection(e.to_string()))?
                    .map_err(PeerError::Protocol)?;
                (raw, n)
            }
            other => return Err(unexpected(&other)),
        };
        self.stats.raw.fetch_add(raw.len() as u64, Ordering::Relaxed);
        self.stats.wire.fetch_add(wire as u64, Ordering::Relaxed);
        Ok(raw)
    }

    async fn notify(&self, pair: ShareId, seq: u64) -> PeerResult<()> {
        match self.request(&Request::Notify { pair, seq }).await? {
            Response::Notified => Ok(()),
            other => Err(unexpected(&other)),
        }
    }

    async fn pair(&self, round: PairRound) -> PeerResult<PairReply> {
        match self.request(&Request::Pair(round)).await? {
            Response::Pair(reply) => Ok(reply),
            other => Err(unexpected(&other)),
        }
    }

    async fn group_ops(&self, heads: Vec<OpId>) -> PeerResult<Vec<GroupOp>> {
        let mut heads = heads;
        heads.truncate(MAX_GROUP_HEADS);
        match self.request(&Request::Group { heads }).await? {
            Response::Group(ops) if ops.len() <= MAX_OPS => Ok(ops),
            other => Err(unexpected(&other)),
        }
    }

    async fn group_changed(&self) -> PeerResult<()> {
        match self.request(&Request::GroupChanged).await? {
            Response::Notified => Ok(()),
            other => Err(unexpected(&other)),
        }
    }

    async fn offers(&self) -> PeerResult<Vec<Offer>> {
        match self.request(&Request::Offers).await {
            Ok(Response::Offers(offers)) => Ok(offers),
            Ok(other) => Err(unexpected(&other)),
            // A device from before offers cannot decode the request and says so with a protocol
            // error (docs/INTRODUCTIONS.md §5); so does an answer over the limits. Both stay
            // errors: read as "offers nothing", they made the caller drop every offer kept from
            // that device, so a declined offer came back as new (review of #28). A device from
            // before offers never made one, so nothing it offered is lost.
            Err(e) => Err(e),
        }
    }
}

/// The client side of the hello: says our version, hears theirs.
async fn hello(conn: &Connection) -> Result<(), QuicError> {
    let broken = |e: &dyn fmt::Display| QuicError::Connect(e.to_string());
    let (mut send, mut recv) = conn.open_bi().await.map_err(|e| broken(&e))?;
    proto::write_frame(
        &mut send,
        &proto::encode(&Hello {
            version: PROTOCOL_VERSION,
        }),
    )
    .await?;
    send.finish().map_err(|e| broken(&e))?;
    let reply: Hello = match proto::read_frame(&mut recv, MAX_REQUEST_BYTES).await {
        Ok(body) => proto::decode(&body)?,
        // A peer that refused our certificate closes the connection instead of answering.
        Err(ProtoError::Io(_) | ProtoError::Closed) => {
            let reason = conn.close_reason().map(|r| r.to_string());
            return Err(QuicError::Connect(
                reason.unwrap_or_else(|| "the peer closed the connection".into()),
            ));
        }
        Err(e) => return Err(e.into()),
    };
    if reply.version != PROTOCOL_VERSION {
        conn.close(VarInt::from_u32(CLOSE_VERSION), b"unsupported protocol version");
        return Err(QuicError::Version(reply.version));
    }
    Ok(())
}

/// The server side of one connection: the hello, then one task per request stream.
/// Trust is asked again before every request, so a device removed from the group loses an open
/// connection at its next request, not only at its next handshake.
async fn serve_connection(
    incoming: quinn::Incoming,
    service: Arc<dyn PeerService>,
    trust: Trust,
    cap: Arc<AtomicU8>,
    slots: Arc<tokio::sync::Semaphore>,
) {
    let Ok(conn) = incoming.await else {
        return;
    };
    let Some(pk) = peer_key(&conn) else {
        conn.close(VarInt::from_u32(CLOSE_PROTOCOL), b"no certificate");
        return;
    };
    let peer = DeviceId::from_public_key(&pk);
    match tokio::time::timeout(HELLO_TIMEOUT, answer_hello(&conn)).await {
        Ok(Ok(true)) => {}
        Ok(Ok(false)) => {
            // Leave the peer time to read our version before closing.
            let _ = tokio::time::timeout(HELLO_TIMEOUT, conn.closed()).await;
            conn.close(VarInt::from_u32(CLOSE_VERSION), b"unsupported protocol version");
            return;
        }
        _ => {
            conn.close(VarInt::from_u32(CLOSE_PROTOCOL), b"bad hello");
            return;
        }
    }
    while let Ok((send, recv)) = conn.accept_bi().await {
        if !trust(&pk) {
            conn.close(VarInt::from_u32(CLOSE_UNTRUSTED), b"no longer trusted");
            return;
        }
        let level_cap = cap.load(Ordering::Relaxed);
        let squeeze = Squeeze {
            level_cap,
            slots: slots.clone(),
        };
        // Where the request comes from now (QUIC lets an address change mid-connection).
        let answering = service
            .for_remote(conn.remote_address())
            .unwrap_or_else(|| service.clone());
        tokio::spawn(serve_stream(peer, answering, send, recv, squeeze));
    }
}

/// Answers the client's hello with our version. `false` when the versions differ.
async fn answer_hello(conn: &Connection) -> Result<bool, QuicError> {
    let (mut send, mut recv) = conn
        .accept_bi()
        .await
        .map_err(|e| QuicError::Connect(e.to_string()))?;
    let theirs: Hello = proto::decode(&proto::read_frame(&mut recv, MAX_REQUEST_BYTES).await?)?;
    proto::write_frame(
        &mut send,
        &proto::encode(&Hello {
            version: PROTOCOL_VERSION,
        }),
    )
    .await?;
    send.finish().map_err(|e| QuicError::Connect(e.to_string()))?;
    Ok(theirs.version == PROTOCOL_VERSION)
}

async fn serve_stream(
    peer: DeviceId,
    service: Arc<dyn PeerService>,
    mut send: SendStream,
    mut recv: RecvStream,
    squeeze: Squeeze,
) {
    let request = proto::read_frame(&mut recv, MAX_REQUEST_BYTES)
        .await
        .and_then(|body| proto::decode_request(&body));
    let response = match request {
        Ok(req) => answer(service.as_ref(), peer, req, &squeeze).await,
        Err(e) => {
            let _ = recv.stop(VarInt::from_u32(CLOSE_PROTOCOL));
            Response::Error(e.into())
        }
    };
    if proto::write_frame(&mut send, &proto::encode(&response))
        .await
        .is_ok()
    {
        let _ = send.finish();
    }
}

/// What this device spends compressing chunks for one request: the level cap, and the shared
/// slots for compressions running at once.
struct Squeeze {
    level_cap: u8,
    slots: Arc<tokio::sync::Semaphore>,
}

async fn answer(service: &dyn PeerService, peer: DeviceId, req: Request, squeeze: &Squeeze) -> Response {
    let result = match req {
        Request::Shared => Ok(Response::Shared(service.shared(peer).await)),
        Request::ChangesSince { pair, since, limit } => {
            let limit = (limit as usize).min(MAX_PAGE_ENTRIES);
            match service.changes_since(peer, pair, since, limit).await {
                Ok(page) if page.entries.len() > limit => Err(PeerError::Remote(
                    "the index returned a page larger than asked".into(),
                )),
                other => other.map(Response::Page),
            }
        }
        Request::ChunkList { pair, key, blake3 } => service
            .chunk_list(peer, pair, &key, blake3)
            .await
            .map(Response::ChunkList),
        Request::Chunk { pair, hash, zstd } => match service.chunk(peer, pair, hash).await {
            Ok(bytes) => {
                // The asker's wish within this device's budget, off the async threads. When every
                // compression slot is busy the chunk goes raw rather than queueing CPU work a peer
                // asked for.
                let level = zstd.min(squeeze.level_cap);
                let slot = (level > 0)
                    .then(|| squeeze.slots.clone().try_acquire_owned().ok())
                    .flatten();
                let packed = match slot {
                    None => Ok(Packed::Raw(bytes)),
                    Some(slot) => tokio::task::spawn_blocking(move || {
                        let packed = compress::pack(bytes, level);
                        drop(slot);
                        packed
                    })
                    .await
                    .map_err(|e| PeerError::Remote(e.to_string())),
                };
                packed.map(|p| match p {
                    Packed::Raw(bytes) => Response::Chunk(bytes),
                    Packed::Zstd { len, data } => Response::ChunkZstd { len, data },
                })
            }
            Err(e) => Err(e),
        },
        Request::Notify { pair, seq } => {
            service.notify(peer, pair, seq).await;
            Ok(Response::Notified)
        }
        Request::Pair(round) => service.pair(peer, round).await.map(Response::Pair),
        Request::Group { heads } => service.group_ops(peer, heads).await.map(Response::Group),
        Request::GroupChanged => {
            service.group_changed(peer).await;
            Ok(Response::Notified)
        }
        Request::Offers => {
            let mut offers = service.offers(peer).await;
            offers.truncate(MAX_OFFERS);
            Ok(Response::Offers(offers))
        }
    };
    let response = result.unwrap_or_else(Response::Error);
    match response.check() {
        Ok(()) => response,
        Err(e) => Response::Error(PeerError::Remote(format!(
            "this device produced an invalid answer: {e}"
        ))),
    }
}

/// The device ID behind the certificate the peer proved during the handshake.
fn peer_id(conn: &Connection) -> Option<DeviceId> {
    peer_key(conn).map(|pk| DeviceId::from_public_key(&pk))
}

/// The key the peer proved during the handshake.
fn peer_key(conn: &Connection) -> Option<[u8; 32]> {
    let certs = conn
        .peer_identity()?
        .downcast::<Vec<CertificateDer<'static>>>()
        .ok()?;
    certs.first().and_then(certificate_key)
}

/// The self-signed certificate and PKCS#8 key for a device key.
fn certificate(key: &DeviceKey) -> Result<(CertificateDer<'static>, PrivateKeyDer<'static>), QuicError> {
    let tls = |e: rcgen::Error| QuicError::Tls(e.to_string());
    let mut pkcs8 = Zeroizing::new(Vec::with_capacity(ED25519_PKCS8_PREFIX.len() + 32));
    pkcs8.extend_from_slice(&ED25519_PKCS8_PREFIX);
    pkcs8.extend_from_slice(key.seed().as_ref());
    let der = PrivatePkcs8KeyDer::from(pkcs8.to_vec());
    let pair = rcgen::KeyPair::from_pkcs8_der_and_sign_algo(&der, &rcgen::PKCS_ED25519).map_err(tls)?;
    let mut params = rcgen::CertificateParams::new(vec![SERVER_NAME.to_string()]).map_err(tls)?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, SERVER_NAME);
    let cert = params.self_signed(&pair).map_err(tls)?;
    Ok((cert.der().clone(), PrivateKeyDer::Pkcs8(der)))
}

/// The Ed25519 public key in a certificate, if that is what it holds.
fn certificate_key(cert: &CertificateDer<'_>) -> Option<[u8; 32]> {
    let parsed = webpki::EndEntityCert::try_from(cert).ok()?;
    let spki = parsed.subject_public_key_info();
    let key = spki.as_ref().strip_prefix(&ED25519_SPKI_PREFIX[..])?;
    key.try_into().ok()
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn transport() -> Arc<quinn::TransportConfig> {
    let mut t = quinn::TransportConfig::default();
    t.max_concurrent_bidi_streams(VarInt::from_u32(MAX_STREAMS))
        .max_concurrent_uni_streams(VarInt::from_u32(0))
        .keep_alive_interval(Some(KEEP_ALIVE))
        .max_idle_timeout(Some(
            IDLE_TIMEOUT.try_into().expect("idle timeout fits a QUIC varint"),
        ));
    Arc::new(t)
}

fn tls_error(e: rustls::Error) -> QuicError {
    QuicError::Tls(e.to_string())
}

fn server_config(
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
    verifier: Arc<Verifier>,
) -> Result<quinn::ServerConfig, QuicError> {
    let mut tls = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(tls_error)?
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![cert], key)
        .map_err(tls_error)?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    tls.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
    tls.send_tls13_tickets = 0;
    let crypto = QuicServerConfig::try_from(tls).map_err(|e| QuicError::Tls(e.to_string()))?;
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    config.transport_config(transport());
    Ok(config)
}

fn client_config(
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
    verifier: Arc<Verifier>,
) -> Result<quinn::ClientConfig, QuicError> {
    let mut tls = rustls::ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(tls_error)?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(vec![cert], key)
        .map_err(tls_error)?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    tls.resumption = rustls::client::Resumption::disabled();
    let crypto = QuicClientConfig::try_from(tls).map_err(|e| QuicError::Tls(e.to_string()))?;
    let mut config = quinn::ClientConfig::new(Arc::new(crypto));
    config.transport_config(transport());
    Ok(config)
}

/// Checks a peer's certificate on either side: an Ed25519 key that [`Trust`] accepts, and a
/// handshake signature made with it.
struct Verifier {
    trust: Trust,
    algorithms: WebPkiSupportedAlgorithms,
}

impl Verifier {
    fn new(trust: Trust) -> Arc<Self> {
        Arc::new(Self {
            trust,
            algorithms: rustls::crypto::ring::default_provider().signature_verification_algorithms,
        })
    }

    fn check(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
    ) -> Result<(), rustls::Error> {
        let bad = || rustls::Error::InvalidCertificate(CertificateError::BadEncoding);
        if !intermediates.is_empty() {
            return Err(bad());
        }
        let pk = certificate_key(end_entity).ok_or_else(bad)?;
        parse_public_key(&pk).map_err(|_| bad())?;
        if !(self.trust)(&pk) {
            return Err(rustls::Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ));
        }
        Ok(())
    }

    fn tls13(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.algorithms)
    }
}

impl fmt::Debug for Verifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Verifier")
    }
}

/// TLS 1.2 is never negotiated (both configs allow 1.3 only); refuse it if asked anyway.
fn no_tls12() -> Result<HandshakeSignatureValid, rustls::Error> {
    Err(rustls::Error::PeerIncompatible(
        rustls::PeerIncompatible::Tls12NotOffered,
    ))
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        self.check(end_entity, intermediates)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        no_tls12()
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.tls13(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
}

impl ClientCertVerifier for Verifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        self.check(end_entity, intermediates)?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        no_tls12()
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.tls13(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    #[async_trait]
    impl PeerService for Echo {
        async fn shared(&self, _: DeviceId) -> Vec<ShareId> {
            vec![ShareId([9; 16])]
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

    fn anyone() -> Trust {
        Arc::new(|_| true)
    }

    async fn pair() -> (Endpoint, Endpoint, QuicLink) {
        let local = "127.0.0.1:0".parse().unwrap();
        let server = Endpoint::bind(local, &DeviceKey::from_seed(&[1; 32]), anyone()).unwrap();
        server.serve(Arc::new(Echo));
        let client = Endpoint::bind(local, &DeviceKey::from_seed(&[2; 32]), anyone()).unwrap();
        let link = client.connect(server.local_addr().unwrap(), None).await.unwrap();
        (server, client, link)
    }

    /// Sends raw bytes on a fresh stream and reads the one response.
    async fn raw(link: &QuicLink, bytes: &[u8]) -> Response {
        let (mut send, mut recv) = link.conn.open_bi().await.unwrap();
        // The server may stop reading before taking everything; that is the point.
        let _ = send.write_all(bytes).await;
        let _ = send.finish();
        let body = proto::read_frame(&mut recv, MAX_FRAME_BYTES).await.unwrap();
        proto::decode_response(&body).unwrap()
    }

    #[test]
    fn the_certificate_carries_the_device_key() {
        let key = DeviceKey::from_seed(&[5; 32]);
        let (cert, _) = certificate(&key).unwrap();
        assert_eq!(certificate_key(&cert), Some(key.public()));
        assert_eq!(certificate_key(&CertificateDer::from(vec![0x30, 0x00])), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_huge_frame_is_refused_and_the_connection_still_serves() {
        let (_server, _client, link) = pair().await;
        let resp = raw(&link, &u32::MAX.to_be_bytes()).await;
        assert!(
            matches!(resp, Response::Error(PeerError::Protocol(_))),
            "{resp:?}"
        );
        let mut over = ((MAX_REQUEST_BYTES + 1) as u32).to_be_bytes().to_vec();
        over.extend(vec![0u8; 1000]);
        let resp = raw(&link, &over).await;
        assert!(
            matches!(resp, Response::Error(PeerError::Protocol(_))),
            "{resp:?}"
        );
        assert_eq!(link.shared().await.unwrap(), vec![ShareId([9; 16])]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn garbage_closes_only_its_stream() {
        let (_server, _client, link) = pair().await;
        for garbage in [
            &[0, 0, 0, 3, 0xff, 0xff, 0xff][..],
            &[0, 0, 0, 1, 0x00],
            &[0, 0, 0, 9, 1, 2],
        ] {
            let resp = raw(&link, garbage).await;
            assert!(
                matches!(resp, Response::Error(PeerError::Protocol(_))),
                "{resp:?}"
            );
        }
        // An unknown request variant.
        let mut unknown = Vec::new();
        ciborium::into_writer(&"Format", &mut unknown).unwrap();
        let mut frame = (unknown.len() as u32).to_be_bytes().to_vec();
        frame.extend(unknown);
        assert!(matches!(
            raw(&link, &frame).await,
            Response::Error(PeerError::Protocol(_))
        ));
        assert_eq!(link.shared().await.unwrap(), vec![ShareId([9; 16])]);
    }

    /// What a device from before offers knows of requests (enough for this test).
    #[derive(serde::Deserialize)]
    enum OldRequest {
        Shared,
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_device_from_before_offers_answers_with_a_protocol_error_and_keeps_serving() {
        // The server side as a 0.3.1 device runs it: a request it cannot decode is answered with
        // a protocol error on its stream (`serve_stream`), and the connection goes on.
        let local = "127.0.0.1:0".parse().unwrap();
        let server = Endpoint::bind(local, &DeviceKey::from_seed(&[1; 32]), anyone()).unwrap();
        let accept = server.endpoint.clone();
        tokio::spawn(async move {
            let conn = accept.accept().await.unwrap().await.unwrap();
            assert!(answer_hello(&conn).await.unwrap());
            while let Ok((mut send, mut recv)) = conn.accept_bi().await {
                let body = proto::read_frame(&mut recv, MAX_REQUEST_BYTES).await.unwrap();
                let resp = match proto::decode::<OldRequest>(&body) {
                    Ok(OldRequest::Shared) => Response::Shared(vec![ShareId([9; 16])]),
                    Err(e) => Response::Error(e.into()),
                };
                proto::write_frame(&mut send, &proto::encode(&resp))
                    .await
                    .unwrap();
                let _ = send.finish();
            }
        });
        let client = Endpoint::bind(local, &DeviceKey::from_seed(&[2; 32]), anyone()).unwrap();
        let link = client.connect(server.local_addr().unwrap(), None).await.unwrap();
        assert!(
            matches!(link.offers().await, Err(PeerError::Protocol(_))),
            "an error, not \"offers nothing\": the caller keeps what it knew"
        );
        assert_eq!(
            Ok(vec![ShareId([9; 16])]),
            link.shared().await,
            "the link still serves"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn offers_travel_and_are_cut_to_the_limit() {
        struct Offering;
        #[async_trait]
        impl PeerService for Offering {
            async fn shared(&self, _: DeviceId) -> Vec<ShareId> {
                Vec::new()
            }
            async fn changes_since(
                &self,
                _: DeviceId,
                _: ShareId,
                _: u64,
                _: usize,
            ) -> PeerResult<IndexPage> {
                Err(PeerError::NotShared)
            }
            async fn chunk_list(&self, _: DeviceId, _: ShareId, _: &str, _: Hash) -> PeerResult<ChunkList> {
                Err(PeerError::NotShared)
            }
            async fn chunk(&self, _: DeviceId, _: ShareId, _: Hash) -> PeerResult<Vec<u8>> {
                Err(PeerError::NotShared)
            }
            async fn notify(&self, _: DeviceId, _: ShareId, _: u64) {}
            async fn offers(&self, peer: DeviceId) -> Vec<Offer> {
                (0..100u8)
                    .map(|i| Offer {
                        share: ShareId([i; 16]),
                        name: format!("{peer}"),
                        files: u64::from(i),
                        bytes: 7,
                    })
                    .collect()
            }
        }
        let local = "127.0.0.1:0".parse().unwrap();
        let server = Endpoint::bind(local, &DeviceKey::from_seed(&[1; 32]), anyone()).unwrap();
        server.serve(Arc::new(Offering));
        let key = DeviceKey::from_seed(&[2; 32]);
        let client = Endpoint::bind(local, &key, anyone()).unwrap();
        let link = client.connect(server.local_addr().unwrap(), None).await.unwrap();
        let offers = link.offers().await.unwrap();
        assert_eq!(MAX_OFFERS, offers.len());
        assert_eq!(
            key.id().to_string(),
            offers[0].name,
            "answered for the asking device"
        );
        // A service that answers nothing (the default) offers nothing.
        let (_s, _c, echo) = pair().await;
        assert_eq!(Ok(Vec::new()), echo.offers().await);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn another_protocol_version_is_turned_away_politely() {
        let local = "127.0.0.1:0".parse().unwrap();
        let server = Endpoint::bind(local, &DeviceKey::from_seed(&[1; 32]), anyone()).unwrap();
        server.serve(Arc::new(Echo));
        let client = Endpoint::bind(local, &DeviceKey::from_seed(&[2; 32]), anyone()).unwrap();
        let config = client_config(
            client.cert.clone(),
            client.key.clone_key(),
            Verifier::new(anyone()),
        )
        .unwrap();
        let conn = client
            .endpoint
            .connect_with(config, server.local_addr().unwrap(), SERVER_NAME)
            .unwrap()
            .await
            .unwrap();
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        proto::write_frame(&mut send, &proto::encode(&Hello { version: 2 }))
            .await
            .unwrap();
        send.finish().unwrap();
        let reply: Hello = proto::decode(&proto::read_frame(&mut recv, 100).await.unwrap()).unwrap();
        assert_eq!(reply.version, PROTOCOL_VERSION);
        conn.close(VarInt::from_u32(CLOSE_VERSION), b"");
    }
}
