//! STUN Binding (RFC 8489): learning the public address a NAT gives this device's QUIC socket
//! (docs/DEVICE_SYNC.md §7).
//!
//! Only the Binding method, without authentication: a 20-byte request, and a response carrying
//! `XOR-MAPPED-ADDRESS` (or the old `MAPPED-ADDRESS`). Requests are retransmitted with a doubling
//! timeout, bounded by [`StunConfig`].
//!
//! The address only helps hole punching if it is the mapping of the QUIC socket itself, so the
//! [`crate::quic::Endpoint`] runs STUN on its own socket ([`crate::quic::Endpoint::stun`]): a
//! wrapper around quinn's socket ([`MuxSocket`]) takes out of the receive path the STUN responses
//! whose transaction ID this device is waiting for, and hands everything else to QUIC (RFC 7983
//! demultiplexing, made exact by the 96-bit random transaction ID). The endpoint never answers STUN
//! requests. [`binding`] does the same over a plain [`UdpSocket`], and [`respond`] is a small
//! rate-limited STUN server (tests, the NAT lab, or a self-hosted server).
//!
//! What an attacker on the path can do: forge or change a response (STUN without
//! MESSAGE-INTEGRITY is unauthenticated), so this device publishes a wrong public address in its
//! signed record. Peers then send QUIC handshakes to that address, which fail the pinned-key check:
//! the connection is lost (as the attacker could do by dropping packets anyway), never redirected
//! to someone else. It cannot make the endpoint deliver anything but a matching response to the
//! STUN client, nor make it answer anyone. Lengths in every message are checked against the
//! datagram before use, and messages over [`MAX_MESSAGE_BYTES`] are ignored.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::io::{self, IoSliceMut};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, ready};
use std::time::Duration;

use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::discover::RateLimit;

/// The fixed value in every STUN header (RFC 8489 §5).
pub const MAGIC_COOKIE: u32 = 0x2112_A442;
/// The STUN header length.
pub const HEADER_LEN: usize = 20;
/// Largest STUN message read; anything longer is not for us.
pub const MAX_MESSAGE_BYTES: usize = 1280;
/// Public STUN servers used when none are configured (`host:port`, resolved when used).
pub const DEFAULT_SERVERS: &[&str] = &[
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302",
    "stun.cloudflare.com:3478",
];
/// Transactions an endpoint may have open at once.
pub const MAX_PENDING: usize = 16;

const BINDING_REQUEST: u16 = 0x0001;
const BINDING_SUCCESS: u16 = 0x0101;
const BINDING_ERROR: u16 = 0x0111;
const ATTR_MAPPED_ADDRESS: u16 = 0x0001;
const ATTR_ERROR_CODE: u16 = 0x0009;
const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;
/// Comprehension-required attributes a server may add that a Binding client can ignore:
/// USERNAME, MESSAGE-INTEGRITY, UNKNOWN-ATTRIBUTES, REALM, NONCE, MESSAGE-INTEGRITY-SHA256,
/// PASSWORD-ALGORITHM, USERHASH.
const IGNORABLE_REQUIRED: &[u16] = &[0x0006, 0x0008, 0x000A, 0x0014, 0x0015, 0x001C, 0x001D, 0x001E];
const MAX_ATTRIBUTES: usize = 32;
const FAMILY_V4: u8 = 0x01;
const FAMILY_V6: u8 = 0x02;

/// A STUN transaction ID: 96 random bits.
pub type TransactionId = [u8; 12];

/// A fresh random transaction ID.
pub fn new_transaction() -> TransactionId {
    let mut tx = [0u8; 12];
    getrandom::fill(&mut tx).expect("the OS random source is available");
    tx
}

/// Why a STUN exchange failed.
#[derive(Debug, thiserror::Error)]
pub enum StunError {
    #[error("not a STUN message")]
    NotStun,
    #[error("malformed STUN message: {0}")]
    Malformed(&'static str),
    #[error("unsupported STUN message type {0:#06x}")]
    Unsupported(u16),
    #[error("the STUN server answered without an address")]
    NoAddress,
    #[error("the STUN server refused the request (error {0})")]
    Refused(u16),
    #[error("no answer from the STUN server")]
    Timeout,
    #[error("too many STUN requests at once")]
    Busy,
    #[error("i/o: {0}")]
    Io(#[from] io::Error),
}

/// A decoded STUN message (Binding only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    BindingRequest {
        tx: TransactionId,
    },
    /// `mapped` is the reflexive address, `None` when the server sent none.
    BindingSuccess {
        tx: TransactionId,
        mapped: Option<SocketAddr>,
    },
    /// `code` is the ERROR-CODE (class × 100 + number), 0 if absent.
    BindingError {
        tx: TransactionId,
        code: u16,
    },
}

impl Message {
    pub fn tx(&self) -> TransactionId {
        match self {
            Message::BindingRequest { tx }
            | Message::BindingSuccess { tx, .. }
            | Message::BindingError { tx, .. } => *tx,
        }
    }
}

fn header(kind: u16, body_len: usize, tx: &TransactionId) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + body_len);
    out.extend_from_slice(&kind.to_be_bytes());
    out.extend_from_slice(&(body_len as u16).to_be_bytes());
    out.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
    out.extend_from_slice(tx);
    out
}

/// A Binding request: the bare 20-byte header.
pub fn encode_request(tx: &TransactionId) -> Vec<u8> {
    header(BINDING_REQUEST, 0, tx)
}

/// A Binding success response telling the client its address `mapped` (XOR-MAPPED-ADDRESS).
pub fn encode_success(tx: &TransactionId, mapped: SocketAddr) -> Vec<u8> {
    let value = xor_address(tx, mapped);
    let mut out = header(BINDING_SUCCESS, 4 + value.len(), tx);
    out.extend_from_slice(&ATTR_XOR_MAPPED_ADDRESS.to_be_bytes());
    out.extend_from_slice(&(value.len() as u16).to_be_bytes());
    out.extend_from_slice(&value);
    out
}

/// A Binding error response with `code` (300–699).
pub fn encode_error(tx: &TransactionId, code: u16) -> Vec<u8> {
    let mut out = header(BINDING_ERROR, 8, tx);
    out.extend_from_slice(&ATTR_ERROR_CODE.to_be_bytes());
    out.extend_from_slice(&4u16.to_be_bytes());
    out.extend_from_slice(&[0, 0, (code / 100) as u8 & 0x07, (code % 100) as u8]);
    out
}

/// The XOR-MAPPED-ADDRESS value for `addr` (RFC 8489 §14.2).
fn xor_address(tx: &TransactionId, addr: SocketAddr) -> Vec<u8> {
    let port = addr.port() ^ (MAGIC_COOKIE >> 16) as u16;
    let mut out = vec![0];
    match addr.ip() {
        IpAddr::V4(ip) => {
            out.push(FAMILY_V4);
            out.extend_from_slice(&port.to_be_bytes());
            out.extend_from_slice(&(u32::from(ip) ^ MAGIC_COOKIE).to_be_bytes());
        }
        IpAddr::V6(ip) => {
            out.push(FAMILY_V6);
            out.extend_from_slice(&port.to_be_bytes());
            let key = xor_key(tx);
            out.extend(ip.octets().iter().zip(key).map(|(a, k)| a ^ k));
        }
    }
    out
}

/// The 16 bytes an IPv6 address is XORed with: the cookie, then the transaction ID.
fn xor_key(tx: &TransactionId) -> [u8; 16] {
    let mut key = [0u8; 16];
    key[..4].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
    key[4..].copy_from_slice(tx);
    key
}

/// Whether `packet` has the shape of a STUN message: top two bits zero, a body length matching
/// the datagram and a multiple of 4, and the magic cookie (RFC 8489 §5, RFC 7983).
pub fn looks_like_stun(packet: &[u8]) -> bool {
    packet.len() >= HEADER_LEN
        && packet.len() <= MAX_MESSAGE_BYTES
        && packet[0] & 0xC0 == 0
        && usize::from(u16::from_be_bytes([packet[2], packet[3]])) == packet.len() - HEADER_LEN
        && packet.len().is_multiple_of(4)
        && packet[4..8] == MAGIC_COOKIE.to_be_bytes()
}

/// Decodes a Binding request or response. Every attribute length is checked against the
/// datagram; a success response with a comprehension-required attribute this client does not
/// know is refused (RFC 8489 §7.3.3).
pub fn decode(packet: &[u8]) -> Result<Message, StunError> {
    if !looks_like_stun(packet) {
        return Err(StunError::NotStun);
    }
    let kind = u16::from_be_bytes([packet[0], packet[1]]);
    let tx: TransactionId = packet[8..20].try_into().expect("12 bytes");
    let mut body = &packet[HEADER_LEN..];
    let mut xor_mapped = None;
    let mut mapped = None;
    let mut code = 0;
    let mut count = 0;
    while !body.is_empty() {
        count += 1;
        if count > MAX_ATTRIBUTES {
            return Err(StunError::Malformed("too many attributes"));
        }
        if body.len() < 4 {
            return Err(StunError::Malformed("truncated attribute header"));
        }
        let attr = u16::from_be_bytes([body[0], body[1]]);
        let len = usize::from(u16::from_be_bytes([body[2], body[3]]));
        let padded = len.div_ceil(4) * 4;
        if body.len() < 4 + padded {
            return Err(StunError::Malformed("attribute longer than the message"));
        }
        let value = &body[4..4 + len];
        match attr {
            ATTR_XOR_MAPPED_ADDRESS => xor_mapped = Some(parse_address(value, Some(&tx))?),
            ATTR_MAPPED_ADDRESS => mapped = Some(parse_address(value, None)?),
            ATTR_ERROR_CODE => {
                if value.len() < 4 {
                    return Err(StunError::Malformed("short ERROR-CODE"));
                }
                code = u16::from(value[2] & 0x07) * 100 + u16::from(value[3]);
            }
            a if a < 0x8000 && kind == BINDING_SUCCESS && !IGNORABLE_REQUIRED.contains(&a) => {
                return Err(StunError::Malformed("unknown comprehension-required attribute"));
            }
            _ => {}
        }
        body = &body[4 + padded..];
    }
    match kind {
        BINDING_REQUEST => Ok(Message::BindingRequest { tx }),
        BINDING_SUCCESS => Ok(Message::BindingSuccess {
            tx,
            mapped: xor_mapped.or(mapped),
        }),
        BINDING_ERROR => Ok(Message::BindingError { tx, code }),
        other => Err(StunError::Unsupported(other)),
    }
}

/// A (XOR-)MAPPED-ADDRESS value; `tx` set means XORed.
fn parse_address(value: &[u8], tx: Option<&TransactionId>) -> Result<SocketAddr, StunError> {
    if value.len() < 4 {
        return Err(StunError::Malformed("short address"));
    }
    let mut port = u16::from_be_bytes([value[2], value[3]]);
    if tx.is_some() {
        port ^= (MAGIC_COOKIE >> 16) as u16;
    }
    let ip = match (value[1], value.len()) {
        (FAMILY_V4, 8) => {
            let mut raw = u32::from_be_bytes(value[4..8].try_into().expect("4 bytes"));
            if tx.is_some() {
                raw ^= MAGIC_COOKIE;
            }
            IpAddr::V4(Ipv4Addr::from(raw))
        }
        (FAMILY_V6, 20) => {
            let mut raw: [u8; 16] = value[4..20].try_into().expect("16 bytes");
            if let Some(tx) = tx {
                for (b, k) in raw.iter_mut().zip(xor_key(tx)) {
                    *b ^= k;
                }
            }
            IpAddr::V6(Ipv6Addr::from(raw))
        }
        _ => return Err(StunError::Malformed("bad address family or length")),
    };
    Ok(SocketAddr::new(ip, port))
}

/// How hard a Binding request is tried.
#[derive(Debug, Clone)]
pub struct StunConfig {
    /// The first retransmission timeout; doubled after each send (RFC 8489 §6.2.1).
    pub rto: Duration,
    /// Requests sent at most (the first one included).
    pub sends: u32,
    /// The whole exchange gives up after this.
    pub timeout: Duration,
}

impl Default for StunConfig {
    fn default() -> Self {
        Self {
            rto: Duration::from_millis(500),
            sends: 4,
            timeout: Duration::from_millis(7500),
        }
    }
}

/// Resolves `host:port` server names, skipping the ones that fail. Each name gives at most two
/// addresses (one per family).
pub async fn resolve(servers: &[String]) -> Vec<SocketAddr> {
    let mut out = Vec::new();
    for server in servers {
        let Ok(addrs) = tokio::net::lookup_host(server.as_str()).await else {
            continue;
        };
        let mut v4 = false;
        let mut v6 = false;
        for addr in addrs {
            let seen = if addr.is_ipv4() { &mut v4 } else { &mut v6 };
            if !*seen && !out.contains(&addr) {
                *seen = true;
                out.push(addr);
            }
        }
    }
    out
}

/// One way to move STUN datagrams: a plain socket, or the QUIC endpoint's.
pub(crate) trait Wire {
    fn send(&self, to: SocketAddr, bytes: &[u8]) -> impl Future<Output = io::Result<()>> + Send;
    /// The next candidate response (source address, bytes), or `None` at `until`.
    fn recv(&mut self, until: Instant) -> impl Future<Output = Option<(SocketAddr, Vec<u8>)>> + Send;
}

/// Runs one Binding transaction with retransmissions: the mapped address `server` reports.
pub(crate) async fn transact<W: Wire>(
    wire: &mut W,
    server: SocketAddr,
    tx: TransactionId,
    cfg: &StunConfig,
) -> Result<SocketAddr, StunError> {
    let request = encode_request(&tx);
    let start = Instant::now();
    let give_up = start + cfg.timeout;
    let mut rto = cfg.rto;
    let mut next_send = start;
    for sent in 0..cfg.sends.max(1) {
        if Instant::now() >= give_up {
            break;
        }
        wire.send(server, &request).await?;
        next_send += rto;
        rto = rto.saturating_mul(2);
        let until = if sent + 1 == cfg.sends.max(1) {
            give_up
        } else {
            next_send.min(give_up)
        };
        while let Some((from, bytes)) = wire.recv(until).await {
            if canonical(from) != canonical(server) {
                continue;
            }
            match decode(&bytes) {
                Ok(Message::BindingSuccess { tx: t, mapped }) if t == tx => {
                    return mapped
                        .map(|m| SocketAddr::new(m.ip().to_canonical(), m.port()))
                        .ok_or(StunError::NoAddress);
                }
                Ok(Message::BindingError { tx: t, code }) if t == tx => return Err(StunError::Refused(code)),
                _ => {}
            }
        }
    }
    Err(StunError::Timeout)
}

fn canonical(addr: SocketAddr) -> SocketAddr {
    SocketAddr::new(addr.ip().to_canonical(), addr.port())
}

struct PlainWire<'a>(&'a UdpSocket);

impl Wire for PlainWire<'_> {
    async fn send(&self, to: SocketAddr, bytes: &[u8]) -> io::Result<()> {
        self.0.send_to(bytes, to).await.map(|_| ())
    }

    async fn recv(&mut self, until: Instant) -> Option<(SocketAddr, Vec<u8>)> {
        let mut buf = [0u8; MAX_MESSAGE_BYTES + 1];
        loop {
            match tokio::time::timeout_at(until, self.0.recv_from(&mut buf)).await {
                Err(_) => return None,
                Ok(Ok((n, from))) if n <= MAX_MESSAGE_BYTES => return Some((from, buf[..n].to_vec())),
                Ok(Ok(_)) => continue,
                // Windows reports an ICMP "port unreachable" from an earlier send as a receive error.
                Ok(Err(e)) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                Ok(Err(_)) => return None,
            }
        }
    }
}

/// Asks `server` for this socket's public address. For NAT-type checks; the address the QUIC
/// endpoint is reachable at comes from [`crate::quic::Endpoint::stun`].
pub async fn binding(
    socket: &UdpSocket,
    server: SocketAddr,
    cfg: &StunConfig,
) -> Result<SocketAddr, StunError> {
    transact(&mut PlainWire(socket), server, new_transaction(), cfg).await
}

/// A tiny STUN server: answers Binding requests on `socket` with the sender's address, forever
/// (or until the socket fails). Answers are rate-limited per source and in total; anything else
/// is ignored. Spawn it.
pub async fn respond(socket: UdpSocket) -> io::Result<()> {
    let mut limit = RateLimit::new(Duration::from_millis(50), 512);
    let mut buf = [0u8; MAX_MESSAGE_BYTES + 1];
    loop {
        let (n, from) = match socket.recv_from(&mut buf).await {
            Ok(got) => got,
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
            Err(e) => return Err(e),
        };
        if n > MAX_MESSAGE_BYTES {
            continue;
        }
        if let Ok(Message::BindingRequest { tx }) = decode(&buf[..n])
            && limit.allow(from.ip(), std::time::Instant::now())
        {
            let _ = socket.send_to(&encode_success(&tx, canonical(from)), from).await;
        }
    }
}

type Delivery = (SocketAddr, Vec<u8>);

/// The STUN transactions an endpoint is waiting on, by transaction ID.
#[derive(Default)]
pub(crate) struct Mux {
    pending: Mutex<HashMap<TransactionId, mpsc::Sender<Delivery>>>,
}

impl fmt::Debug for Mux {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Mux")
    }
}

impl Mux {
    fn register(
        self: &Arc<Self>,
        tx: TransactionId,
    ) -> Result<(Registration, mpsc::Receiver<Delivery>), StunError> {
        let mut pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
        if pending.len() >= MAX_PENDING {
            return Err(StunError::Busy);
        }
        let (send, recv) = mpsc::channel(4);
        pending.insert(tx, send);
        Ok((
            Registration {
                mux: self.clone(),
                tx,
            },
            recv,
        ))
    }

    /// Takes `packet` if it is a STUN message for a waiting transaction.
    fn offer(&self, from: SocketAddr, packet: &[u8]) -> bool {
        if !looks_like_stun(packet) {
            return false;
        }
        let tx: TransactionId = packet[8..20].try_into().expect("12 bytes");
        let pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
        match pending.get(&tx) {
            Some(sender) => {
                // A full queue drops the extra copy; one answer is all that is needed.
                let _ = sender.try_send((canonical(from), packet.to_vec()));
                true
            }
            None => false,
        }
    }
}

/// Removes its transaction from the [`Mux`] when dropped.
pub(crate) struct Registration {
    mux: Arc<Mux>,
    tx: TransactionId,
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.mux.pending.lock() {
            pending.remove(&self.tx);
        }
    }
}

/// quinn's UDP socket with STUN responses for waiting transactions taken out of the receive path.
#[derive(Debug)]
pub(crate) struct MuxSocket {
    inner: Arc<dyn AsyncUdpSocket>,
    mux: Arc<Mux>,
}

impl MuxSocket {
    pub(crate) fn new(inner: Arc<dyn AsyncUdpSocket>) -> (Arc<Self>, Arc<Mux>) {
        let mux = Arc::new(Mux::default());
        (
            Arc::new(Self {
                inner,
                mux: mux.clone(),
            }),
            mux,
        )
    }
}

impl AsyncUdpSocket for MuxSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        self.inner.clone().create_io_poller()
    }

    fn try_send(&self, transmit: &Transmit) -> io::Result<()> {
        self.inner.try_send(transmit)
    }

    fn poll_recv(
        &self,
        cx: &mut Context,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        loop {
            let n = ready!(self.inner.poll_recv(cx, bufs, meta))?;
            let mut kept = 0;
            for i in 0..n {
                let m = meta[i];
                // A buffer holding several coalesced datagrams (GRO) is QUIC's.
                let single = m.stride == 0 || m.len <= m.stride;
                if single && m.len <= bufs[i].len() && self.mux.offer(m.addr, &bufs[i][..m.len]) {
                    continue;
                }
                if kept != i {
                    let (head, tail) = bufs.split_at_mut(i);
                    let len = m.len.min(head[kept].len()).min(tail[0].len());
                    head[kept][..len].copy_from_slice(&tail[0][..len]);
                    meta[kept] = RecvMeta { len, ..m };
                }
                kept += 1;
            }
            if kept > 0 || n == 0 {
                return Poll::Ready(Ok(kept));
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }

    fn max_transmit_segments(&self) -> usize {
        self.inner.max_transmit_segments()
    }

    fn max_receive_segments(&self) -> usize {
        self.inner.max_receive_segments()
    }

    fn may_fragment(&self) -> bool {
        self.inner.may_fragment()
    }
}

/// STUN over the QUIC endpoint's socket.
pub(crate) struct EndpointWire {
    socket: Arc<dyn AsyncUdpSocket>,
    recv: mpsc::Receiver<Delivery>,
    _registration: Registration,
}

impl EndpointWire {
    pub(crate) fn new(
        socket: Arc<dyn AsyncUdpSocket>,
        mux: &Arc<Mux>,
        tx: TransactionId,
    ) -> Result<Self, StunError> {
        let (registration, recv) = mux.register(tx)?;
        Ok(Self {
            socket,
            recv,
            _registration: registration,
        })
    }
}

impl Wire for EndpointWire {
    async fn send(&self, to: SocketAddr, bytes: &[u8]) -> io::Result<()> {
        send_raw(&self.socket, to, bytes).await
    }

    async fn recv(&mut self, until: Instant) -> Option<(SocketAddr, Vec<u8>)> {
        tokio::time::timeout_at(until, self.recv.recv())
            .await
            .ok()
            .flatten()
    }
}

/// Sends one datagram on quinn's socket, waiting while it is not writable.
pub(crate) async fn send_raw(
    socket: &Arc<dyn AsyncUdpSocket>,
    to: SocketAddr,
    bytes: &[u8],
) -> io::Result<()> {
    let destination = match (socket.local_addr()?, to) {
        (SocketAddr::V6(_), SocketAddr::V4(v4)) => {
            SocketAddr::new(IpAddr::V6(v4.ip().to_ipv6_mapped()), v4.port())
        }
        (SocketAddr::V4(_), SocketAddr::V6(v6)) => match v6.ip().to_ipv4_mapped() {
            Some(v4) => SocketAddr::new(IpAddr::V4(v4), v6.port()),
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "IPv6 address on an IPv4 socket",
                ));
            }
        },
        _ => to,
    };
    let transmit = Transmit {
        destination,
        ecn: None,
        contents: bytes,
        segment_size: None,
        src_ip: None,
    };
    let mut poller = socket.clone().create_io_poller();
    loop {
        match socket.try_send(&transmit) {
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::future::poll_fn(|cx| poller.as_mut().poll_writable(cx)).await?;
            }
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TX: TransactionId = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];

    #[test]
    fn request_is_the_bare_header() {
        let req = encode_request(&TX);
        assert_eq!(req.len(), 20);
        assert_eq!(&req[..4], &[0x00, 0x01, 0x00, 0x00]);
        assert_eq!(&req[4..8], &[0x21, 0x12, 0xA4, 0x42]);
        assert_eq!(decode(&req).unwrap(), Message::BindingRequest { tx: TX });
    }

    #[test]
    fn rfc5769_ipv4_response_vector() {
        // RFC 5769 §2.2 sample IPv4 response: only the parts this client reads, with SOFTWARE
        // (comprehension-optional) kept and MESSAGE-INTEGRITY/FINGERPRINT dropped.
        let tx = [
            0xb7, 0xe7, 0xa7, 0x01, 0xbc, 0x34, 0xd6, 0x86, 0xfa, 0x87, 0xdf, 0xae,
        ];
        let mut msg = vec![0x01, 0x01, 0x00, 0x00, 0x21, 0x12, 0xa4, 0x42];
        msg.extend_from_slice(&tx);
        // SOFTWARE "test vector " (11 bytes + 1 pad).
        msg.extend_from_slice(&[0x80, 0x22, 0x00, 0x0b]);
        msg.extend_from_slice(b"test vector ");
        // XOR-MAPPED-ADDRESS 192.0.2.1:32853.
        msg.extend_from_slice(&[
            0x00, 0x20, 0x00, 0x08, 0x00, 0x01, 0xa1, 0x47, 0xe1, 0x12, 0xa6, 0x43,
        ]);
        let body = (msg.len() - 20) as u16;
        msg[2..4].copy_from_slice(&body.to_be_bytes());
        assert_eq!(
            decode(&msg).unwrap(),
            Message::BindingSuccess {
                tx,
                mapped: Some("192.0.2.1:32853".parse().unwrap())
            }
        );
    }

    #[test]
    fn rfc5769_ipv6_xor_address() {
        // RFC 5769 §2.3: 2001:db8:1234:5678:11:2233:4455:6677 port 32853.
        let tx = [
            0xb7, 0xe7, 0xa7, 0x01, 0xbc, 0x34, 0xd6, 0x86, 0xfa, 0x87, 0xdf, 0xae,
        ];
        let value = [
            0x00, 0x02, 0xa1, 0x47, 0x01, 0x13, 0xa9, 0xfa, 0xa5, 0xd3, 0xf1, 0x79, 0xbc, 0x25, 0xf4, 0xb5,
            0xbe, 0xd2, 0xb9, 0xd9,
        ];
        let addr: SocketAddr = "[2001:db8:1234:5678:11:2233:4455:6677]:32853".parse().unwrap();
        assert_eq!(parse_address(&value, Some(&tx)).unwrap(), addr);
        assert_eq!(xor_address(&tx, addr), value);
    }

    #[test]
    fn success_round_trips_both_families() {
        for addr in ["203.0.113.9:4500", "[2001:db8::1]:65535"] {
            let addr: SocketAddr = addr.parse().unwrap();
            let msg = encode_success(&TX, addr);
            assert!(looks_like_stun(&msg));
            assert_eq!(
                decode(&msg).unwrap(),
                Message::BindingSuccess {
                    tx: TX,
                    mapped: Some(addr)
                }
            );
        }
    }

    #[test]
    fn error_round_trips() {
        assert_eq!(
            decode(&encode_error(&TX, 420)).unwrap(),
            Message::BindingError { tx: TX, code: 420 }
        );
    }

    #[test]
    fn old_mapped_address_is_used_without_xor() {
        let mut msg = header(BINDING_SUCCESS, 12, &TX);
        msg.extend_from_slice(&[0x00, 0x01, 0x00, 0x08, 0x00, 0x01, 0x12, 0x34, 10, 0, 0, 1]);
        assert_eq!(
            decode(&msg).unwrap(),
            Message::BindingSuccess {
                tx: TX,
                mapped: Some("10.0.0.1:4660".parse().unwrap())
            }
        );
    }

    #[test]
    fn malformed_messages_are_refused() {
        let good = encode_success(&TX, "1.2.3.4:5".parse().unwrap());
        // Wrong cookie, wrong length field, odd length, top bits set, truncated.
        let mut bad_cookie = good.clone();
        bad_cookie[4] ^= 1;
        let mut bad_len = good.clone();
        bad_len[3] += 4;
        let mut top_bits = good.clone();
        top_bits[0] |= 0x40;
        for bad in [&bad_cookie[..], &bad_len, &top_bits, &good[..19], &[]] {
            assert!(matches!(decode(bad), Err(StunError::NotStun)), "{bad:?}");
        }
        // An attribute running past the end.
        let mut over = header(BINDING_SUCCESS, 8, &TX);
        over.extend_from_slice(&[0x00, 0x20, 0x00, 0x40, 0, 0, 0, 0]);
        assert!(matches!(decode(&over), Err(StunError::Malformed(_))));
        // A bad family.
        let mut family = header(BINDING_SUCCESS, 12, &TX);
        family.extend_from_slice(&[0x00, 0x20, 0x00, 0x08, 0x00, 0x03, 0, 0, 0, 0, 0, 0]);
        assert!(matches!(decode(&family), Err(StunError::Malformed(_))));
        // An unknown comprehension-required attribute in a success response.
        let mut unknown = header(BINDING_SUCCESS, 8, &TX);
        unknown.extend_from_slice(&[0x00, 0x7f, 0x00, 0x04, 0, 0, 0, 0]);
        assert!(matches!(decode(&unknown), Err(StunError::Malformed(_))));
        // Too many attributes.
        let mut many = header(BINDING_SUCCESS, 4 * (MAX_ATTRIBUTES + 1), &TX);
        for _ in 0..=MAX_ATTRIBUTES {
            many.extend_from_slice(&[0x80, 0x22, 0x00, 0x00]);
        }
        assert!(matches!(decode(&many), Err(StunError::Malformed(_))));
        // Indications are not Binding requests or responses.
        let indication = header(0x0011, 0, &TX);
        assert!(matches!(decode(&indication), Err(StunError::Unsupported(0x0011))));
    }

    #[test]
    fn quic_packets_do_not_look_like_stun() {
        // Long header (Initial) and short header packets have the top bits set.
        assert!(!looks_like_stun(&[0xc3; 1200]));
        assert!(!looks_like_stun(&[0x41; 40]));
    }

    #[tokio::test]
    async fn binding_against_an_in_process_responder() {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let server_addr = server.local_addr().unwrap();
        tokio::spawn(respond(server));
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mapped = binding(&client, server_addr, &StunConfig::default())
            .await
            .unwrap();
        assert_eq!(mapped, client.local_addr().unwrap());
    }

    #[tokio::test]
    async fn binding_retransmits_after_a_lost_request() {
        // A server that ignores the first request and answers the second.
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let server_addr = server.local_addr().unwrap();
        let seen = tokio::spawn(async move {
            let mut buf = [0u8; 100];
            let mut count = 0;
            loop {
                let (n, from) = server.recv_from(&mut buf).await.unwrap();
                count += 1;
                if count == 2 {
                    let tx = decode(&buf[..n]).unwrap().tx();
                    server.send_to(&encode_success(&tx, from), from).await.unwrap();
                    return count;
                }
            }
        });
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let cfg = StunConfig {
            rto: Duration::from_millis(50),
            ..StunConfig::default()
        };
        let mapped = binding(&client, server_addr, &cfg).await.unwrap();
        assert_eq!(mapped, client.local_addr().unwrap());
        assert_eq!(seen.await.unwrap(), 2);
    }

    #[tokio::test]
    async fn binding_ignores_strangers_and_wrong_transactions_then_times_out() {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let server_addr = server.local_addr().unwrap();
        let stranger = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let client_addr = client.local_addr().unwrap();
        let sends = tokio::spawn(async move {
            let mut buf = [0u8; 100];
            let mut count = 0u32;
            while let Ok(Ok((n, from))) =
                tokio::time::timeout(Duration::from_millis(600), server.recv_from(&mut buf)).await
            {
                count += 1;
                let tx = decode(&buf[..n]).unwrap().tx();
                // The right transaction from the wrong address, and the wrong transaction.
                stranger
                    .send_to(&encode_success(&tx, "6.6.6.6:6".parse().unwrap()), client_addr)
                    .await
                    .unwrap();
                server
                    .send_to(&encode_success(&[0; 12], "6.6.6.6:6".parse().unwrap()), from)
                    .await
                    .unwrap();
            }
            count
        });
        let cfg = StunConfig {
            rto: Duration::from_millis(20),
            sends: 3,
            timeout: Duration::from_millis(300),
        };
        assert!(matches!(
            binding(&client, server_addr, &cfg).await,
            Err(StunError::Timeout)
        ));
        assert_eq!(sends.await.unwrap(), 3, "bounded retransmissions");
    }

    #[tokio::test]
    async fn an_error_response_is_reported() {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let server_addr = server.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 100];
            let (n, from) = server.recv_from(&mut buf).await.unwrap();
            let tx = decode(&buf[..n]).unwrap().tx();
            server.send_to(&encode_error(&tx, 500), from).await.unwrap();
        });
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        assert!(matches!(
            binding(&client, server_addr, &StunConfig::default()).await,
            Err(StunError::Refused(500))
        ));
    }

    #[tokio::test]
    async fn the_responder_ignores_everything_but_requests() {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let server_addr = server.local_addr().unwrap();
        tokio::spawn(respond(server));
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        for junk in [
            &b"hello"[..],
            &encode_success(&TX, "1.2.3.4:5".parse().unwrap()),
            &[0u8; 2000],
        ] {
            client.send_to(junk, server_addr).await.unwrap();
        }
        let mut buf = [0u8; 100];
        assert!(
            tokio::time::timeout(Duration::from_millis(200), client.recv_from(&mut buf))
                .await
                .is_err()
        );
    }
}
