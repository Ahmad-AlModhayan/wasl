//! Finding devices on the local network (docs/DEVICE_SYNC.md §7, FR-N5).
//!
//! A device announces only a random [`Token`] chosen at each start and the port of its QUIC
//! endpoint, never its device ID or name: someone listening on the network learns that a Wasl
//! device is there, nothing about whose. Who it is comes out of the TLS handshake, which succeeds
//! only with a key the group trusts.
//!
//! Two ways, used together:
//! - **mDNS** ([`Lan`]): service `_wasl._udp.local.`, instance name = the token in hex, SRV port =
//!   the QUIC port.
//! - **Broadcast probe** ([`respond`], [`probe`]), for networks that drop multicast: the asking
//!   device sends `WASL?` + its token to the broadcast address on [`PROBE_PORT`]; every device
//!   listening there answers `WASL!` + its token + its QUIC port (big-endian) to the sender.
//!   Anything else is ignored, a device does not answer its own probe, and answers are
//!   rate-limited ([`RateLimit`]) so the probe cannot be used to make devices flood a network.

use std::collections::HashMap;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV6};
use std::time::{Duration, Instant};

use mdns_sd::{ScopedIp, ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::net::UdpSocket;

/// The mDNS service type.
pub const SERVICE_TYPE: &str = "_wasl._udp.local.";
/// The UDP port devices listen on for broadcast probes.
pub const PROBE_PORT: u16 = 41_414;

const PROBE_MAGIC: &[u8; 5] = b"WASL?";
const REPLY_MAGIC: &[u8; 5] = b"WASL!";
const TOKEN_LEN: usize = 16;
/// Most addresses kept for one announced device (security review 2026-10: anyone on the network
/// can announce, so what one announcement makes this device dial is bounded).
pub const MAX_FOUND_ADDRS: usize = 8;

/// Why discovery could not start.
#[derive(Debug, thiserror::Error)]
pub enum DiscoverError {
    #[error("mDNS: {0}")]
    Mdns(String),
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
}

fn mdns_error(e: mdns_sd::Error) -> DiscoverError {
    DiscoverError::Mdns(e.to_string())
}

/// A random value a device announces instead of its identity; new at every start.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Token(pub [u8; TOKEN_LEN]);

impl Token {
    pub fn random() -> Self {
        let mut b = [0u8; TOKEN_LEN];
        getrandom::fill(&mut b).expect("the OS random source is available");
        Token(b)
    }

    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn from_hex(s: &str) -> Option<Self> {
        if s.len() != 2 * TOKEN_LEN || !s.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let mut b = [0u8; TOKEN_LEN];
        for (i, out) in b.iter_mut().enumerate() {
            *out = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
        }
        Some(Token(b))
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Token({})", self.to_hex())
    }
}

/// A device seen on the network: its token and where its QUIC endpoint may be reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub token: Token,
    pub addrs: Vec<SocketAddr>,
}

/// mDNS announcing and browsing. Runs its own background thread until [`Lan::shutdown`] or drop.
pub struct Lan {
    daemon: ServiceDaemon,
}

impl Lan {
    pub fn new() -> Result<Self, DiscoverError> {
        Ok(Self {
            daemon: ServiceDaemon::new().map_err(mdns_error)?,
        })
    }

    /// Announces `token` with the QUIC `port` on every interface.
    pub fn announce(&self, token: Token, port: u16) -> Result<(), DiscoverError> {
        let name = token.to_hex();
        let info = ServiceInfo::new(SERVICE_TYPE, &name, &format!("{name}.local."), (), port, None)
            .map_err(mdns_error)?
            .enable_addr_auto();
        self.daemon.register(info).map_err(mdns_error)
    }

    /// Starts looking for other devices.
    pub fn browse(&self) -> Result<Browser, DiscoverError> {
        Ok(Browser {
            events: self.daemon.browse(SERVICE_TYPE).map_err(mdns_error)?,
        })
    }

    pub fn shutdown(self) {
        let _ = self.daemon.shutdown();
    }
}

/// The token in an announced name `<token>._wasl._udp.local.` (service type matched ignoring
/// case, as mDNS does).
fn instance_token(fullname: &str) -> Option<Token> {
    let cut = fullname.len().checked_sub(SERVICE_TYPE.len())?;
    let (instance, suffix) = (fullname.get(..cut)?, fullname.get(cut..)?);
    if !suffix.eq_ignore_ascii_case(SERVICE_TYPE) {
        return None;
    }
    Token::from_hex(instance.strip_suffix('.')?)
}

/// Devices found by mDNS, as they resolve.
pub struct Browser {
    events: mdns_sd::Receiver<ServiceEvent>,
}

impl Browser {
    /// The next device resolved; `None` once discovery has stopped. Includes this device's own
    /// announcement: compare tokens to skip it.
    pub async fn next(&mut self) -> Option<Found> {
        loop {
            if let ServiceEvent::ServiceResolved(service) = self.events.recv_async().await.ok()? {
                // Any host on the network can announce; a name that does not fit is skipped like
                // any other bad entry, never the end of browsing.
                let Some(token) = instance_token(&service.fullname) else {
                    continue;
                };
                let addrs = lan_addrs(service.addresses.iter().map(|ip| socket_addr(ip, service.port)));
                if addrs.is_empty() {
                    continue;
                }
                return Some(Found { token, addrs });
            }
        }
    }
}

/// The addresses worth dialing from an announcement: local-network addresses only (an
/// announcement naming a public address would turn discovery into dialing the internet, FR-N11),
/// no port 0, without repeats, at most [`MAX_FOUND_ADDRS`] (security review 2026-10).
pub fn lan_addrs(addrs: impl IntoIterator<Item = SocketAddr>) -> Vec<SocketAddr> {
    let mut out: Vec<SocketAddr> = addrs
        .into_iter()
        .filter(|a| a.port() != 0 && crate::rendezvous::is_lan_ip(a.ip()))
        .collect();
    out.sort();
    out.dedup();
    out.truncate(MAX_FOUND_ADDRS);
    out
}

fn socket_addr(ip: &ScopedIp, port: u16) -> SocketAddr {
    match ip {
        ScopedIp::V6(v6) if v6.addr().is_unicast_link_local() => {
            SocketAddr::V6(SocketAddrV6::new(*v6.addr(), port, 0, v6.scope_id().index))
        }
        other => SocketAddr::new(other.to_ip_addr(), port),
    }
}

/// A broadcast-probe packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Packet {
    /// "Who is there?" from the device holding this token.
    Probe(Token),
    /// "Here": the answering device's token and QUIC port.
    Reply { token: Token, port: u16 },
}

impl Packet {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Packet::Probe(token) => [&PROBE_MAGIC[..], &token.0].concat(),
            Packet::Reply { token, port } => [&REPLY_MAGIC[..], &token.0, &port.to_be_bytes()].concat(),
        }
    }

    /// The packet in `bytes`, or `None` for anything else (wrong magic or length).
    pub fn decode(bytes: &[u8]) -> Option<Packet> {
        let (magic, rest) = bytes.split_at_checked(PROBE_MAGIC.len())?;
        let token = |b: &[u8]| -> Option<Token> { Some(Token(b.get(..TOKEN_LEN)?.try_into().ok()?)) };
        if magic == PROBE_MAGIC && rest.len() == TOKEN_LEN {
            return Some(Packet::Probe(token(rest)?));
        }
        if magic == REPLY_MAGIC && rest.len() == TOKEN_LEN + 2 {
            let port = u16::from_be_bytes([rest[TOKEN_LEN], rest[TOKEN_LEN + 1]]);
            return Some(Packet::Reply {
                token: token(rest)?,
                port,
            });
        }
        None
    }
}

/// Limits answers to probes: one per sender address per interval, and a total per second.
#[derive(Debug)]
pub struct RateLimit {
    per_source: Duration,
    per_second: u32,
    last: HashMap<IpAddr, Instant>,
    window: Option<Instant>,
    in_window: u32,
}

impl RateLimit {
    pub fn new(per_source: Duration, per_second: u32) -> Self {
        Self {
            per_source,
            per_second,
            last: HashMap::new(),
            window: None,
            in_window: 0,
        }
    }

    /// Whether to answer `from` at `now`; counts the answer if so.
    pub fn allow(&mut self, from: IpAddr, now: Instant) -> bool {
        match self.window {
            Some(start) if now.duration_since(start) < Duration::from_secs(1) => {}
            _ => {
                self.window = Some(now);
                self.in_window = 0;
            }
        }
        if self.in_window >= self.per_second {
            return false;
        }
        if let Some(&at) = self.last.get(&from)
            && now.duration_since(at) < self.per_source
        {
            return false;
        }
        if self.last.len() >= 1024 {
            let per_source = self.per_source;
            self.last.retain(|_, at| now.duration_since(*at) < per_source);
        }
        self.last.insert(from, now);
        self.in_window += 1;
        true
    }
}

impl Default for RateLimit {
    fn default() -> Self {
        Self::new(Duration::from_secs(1), 32)
    }
}

/// Opens the socket probes arrive on: all IPv4 interfaces, `port` (normally [`PROBE_PORT`]).
pub async fn bind_responder(port: u16) -> std::io::Result<UdpSocket> {
    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port))).await?;
    socket.set_broadcast(true)?;
    Ok(socket)
}

/// Answers probes on `socket` with `token` and the QUIC `port`, forever (or until the socket
/// fails). Spawn it.
pub async fn respond(socket: UdpSocket, token: Token, port: u16) -> std::io::Result<()> {
    let reply = Packet::Reply { token, port }.encode();
    let mut limit = RateLimit::default();
    let mut buf = [0u8; 64];
    loop {
        let (n, from) = match socket.recv_from(&mut buf).await {
            Ok(got) => got,
            // Windows reports an ICMP "port unreachable" from an earlier send as a receive error.
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => continue,
            Err(e) => return Err(e),
        };
        match Packet::decode(&buf[..n]) {
            Some(Packet::Probe(theirs)) if theirs != token && limit.allow(from.ip(), Instant::now()) => {
                let _ = socket.send_to(&reply, from).await;
            }
            _ => {}
        }
    }
}

/// The address probes go to by default: the IPv4 limited broadcast on [`PROBE_PORT`].
pub fn broadcast_target() -> SocketAddr {
    SocketAddr::from((Ipv4Addr::BROADCAST, PROBE_PORT))
}

/// Probes `targets` (normally [`broadcast_target`]) as `token` and collects the answers that
/// arrive within `wait`: each device's token and its QUIC address (the sender's IP, the port it
/// announced). Its own token is skipped.
pub async fn probe(token: Token, targets: &[SocketAddr], wait: Duration) -> std::io::Result<Vec<Found>> {
    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))).await?;
    socket.set_broadcast(true)?;
    let packet = Packet::Probe(token).encode();
    for target in targets {
        // One unreachable target (no route, broadcast refused) must not stop the others.
        let _ = socket.send_to(&packet, target).await;
    }
    let mut found: Vec<Found> = Vec::new();
    let deadline = tokio::time::Instant::now() + wait;
    let mut buf = [0u8; 64];
    loop {
        let (n, from) = match tokio::time::timeout_at(deadline, socket.recv_from(&mut buf)).await {
            Err(_) => break,
            Ok(Ok(got)) => got,
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionReset => continue,
            Ok(Err(e)) => return Err(e),
        };
        let Some(Packet::Reply { token: theirs, port }) = Packet::decode(&buf[..n]) else {
            continue;
        };
        if theirs == token || port == 0 || !crate::rendezvous::is_lan_ip(from.ip()) {
            continue;
        }
        let addr = SocketAddr::new(from.ip(), port);
        match found.iter_mut().find(|f| f.token == theirs) {
            Some(f) if !f.addrs.contains(&addr) && f.addrs.len() < MAX_FOUND_ADDRS => f.addrs.push(addr),
            Some(_) => {}
            None => found.push(Found {
                token: theirs,
                addrs: vec![addr],
            }),
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announced_names_are_parsed_without_ever_giving_up() {
        let t = Token::random();
        assert_eq!(Some(t), instance_token(&format!("{}.{SERVICE_TYPE}", t.to_hex())));
        assert_eq!(
            Some(t),
            instance_token(&format!("{}._WASL._UDP.LOCAL.", t.to_hex()))
        );
        for bad in [
            "",
            ".",
            "x",
            "_wasl._udp.local.",
            "zz._wasl._udp.local.",
            "é._other._udp.local.",
        ] {
            assert_eq!(None, instance_token(bad), "{bad}");
        }
    }
    use proptest::prelude::*;

    #[test]
    fn only_a_few_lan_addresses_of_an_announcement_are_dialed() {
        let a = |s: &str| s.parse::<SocketAddr>().unwrap();
        let got = lan_addrs([
            a("8.8.8.8:4000"),
            a("192.168.1.5:4000"),
            a("192.168.1.5:4000"),
            a("[2001:db8::1]:4000"),
            a("10.0.0.2:0"),
            a("[fd00::1]:4000"),
        ]);
        assert_eq!(got, vec![a("192.168.1.5:4000"), a("[fd00::1]:4000")]);
        let many = lan_addrs((1..=50u8).map(|i| SocketAddr::from(([10, 0, 0, i], 4000))));
        assert_eq!(many.len(), MAX_FOUND_ADDRS);
        assert!(lan_addrs([a("1.1.1.1:4000")]).is_empty());
    }

    #[test]
    fn packets_round_trip() {
        let token = Token([7; 16]);
        let probe = Packet::Probe(token);
        assert_eq!(probe.encode().len(), 21);
        assert_eq!(Packet::decode(&probe.encode()), Some(probe));
        let reply = Packet::Reply { token, port: 51_234 };
        assert_eq!(reply.encode().len(), 23);
        assert_eq!(&reply.encode()[21..], &51_234u16.to_be_bytes());
        assert_eq!(Packet::decode(&reply.encode()), Some(reply));
    }

    #[test]
    fn anything_else_is_ignored() {
        let probe = Packet::Probe(Token([1; 16])).encode();
        let reply = Packet::Reply {
            token: Token([1; 16]),
            port: 9,
        }
        .encode();
        assert_eq!(Packet::decode(&probe[..20]), None);
        assert_eq!(Packet::decode(&[&probe[..], &[0]].concat()), None);
        assert_eq!(Packet::decode(&reply[..22]), None);
        assert_eq!(Packet::decode(&[&reply[..], &[0]].concat()), None);
        assert_eq!(Packet::decode(b"WASL"), None);
        assert_eq!(Packet::decode(b""), None);
        let mut other = probe.clone();
        other[4] = b'!';
        assert_eq!(Packet::decode(&other), None);
        let mut lower = probe;
        lower[0] = b'w';
        assert_eq!(Packet::decode(&lower), None);
    }

    proptest! {
        #[test]
        fn decoding_random_bytes_is_total(bytes in proptest::collection::vec(any::<u8>(), 0..40)) {
            if let Some(p) = Packet::decode(&bytes) {
                prop_assert_eq!(p.encode(), bytes);
            }
        }

        #[test]
        fn tokens_round_trip_through_hex(b in any::<[u8; 16]>()) {
            prop_assert_eq!(Token::from_hex(&Token(b).to_hex()), Some(Token(b)));
        }
    }

    #[test]
    fn token_hex_refuses_junk() {
        assert_eq!(Token::from_hex("00"), None);
        assert_eq!(Token::from_hex(&"g".repeat(32)), None);
        assert_eq!(Token::from_hex(&"é".repeat(16)), None);
        assert_ne!(Token::random(), Token::random());
    }

    #[test]
    fn rate_limit_per_source_and_in_total() {
        let mut limit = RateLimit::new(Duration::from_secs(1), 3);
        let t0 = Instant::now();
        let a: IpAddr = "10.0.0.1".parse().unwrap();
        let b: IpAddr = "10.0.0.2".parse().unwrap();
        assert!(limit.allow(a, t0));
        assert!(!limit.allow(a, t0 + Duration::from_millis(500)));
        assert!(limit.allow(b, t0 + Duration::from_millis(500)));
        assert!(limit.allow("10.0.0.3".parse().unwrap(), t0 + Duration::from_millis(600)));
        // Three answers this second: the fourth sender waits for the next one.
        assert!(!limit.allow("10.0.0.4".parse().unwrap(), t0 + Duration::from_millis(700)));
        assert!(limit.allow("10.0.0.4".parse().unwrap(), t0 + Duration::from_millis(1100)));
        assert!(limit.allow(a, t0 + Duration::from_millis(1200)));
    }

    #[test]
    fn rate_limit_memory_is_bounded() {
        let mut limit = RateLimit::new(Duration::from_millis(10), u32::MAX);
        let t0 = Instant::now();
        for i in 0..5000u32 {
            let ip = IpAddr::from(i.to_be_bytes());
            assert!(limit.allow(ip, t0 + Duration::from_millis(u64::from(i))));
        }
        assert!(limit.last.len() <= 1024);
    }

    #[tokio::test]
    async fn a_directed_probe_is_answered() {
        let socket = bind_responder(0).await.unwrap();
        let port = socket.local_addr().unwrap().port();
        let theirs = Token::random();
        tokio::spawn(respond(socket, theirs, 4242));
        let target = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let found = probe(Token::random(), &[target], Duration::from_millis(500))
            .await
            .unwrap();
        assert_eq!(
            found,
            vec![Found {
                token: theirs,
                addrs: vec![SocketAddr::from((Ipv4Addr::LOCALHOST, 4242))],
            }]
        );
        // A device does not answer its own probe.
        let own = probe(theirs, &[target], Duration::from_millis(300))
            .await
            .unwrap();
        assert!(own.is_empty());
    }

    #[tokio::test]
    async fn a_broadcast_probe_is_answered_where_broadcast_works() {
        let socket = bind_responder(0).await.unwrap();
        let port = socket.local_addr().unwrap().port();
        let theirs = Token::random();
        tokio::spawn(respond(socket, theirs, 4243));
        let target = SocketAddr::from((Ipv4Addr::BROADCAST, port));
        let found = probe(Token::random(), &[target], Duration::from_secs(1))
            .await
            .unwrap();
        match found.iter().find(|f| f.token == theirs) {
            Some(f) => assert!(f.addrs.iter().all(|a| a.port() == 4243)),
            None => eprintln!("note: skipped, broadcast does not loop back on this machine"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mdns_finds_an_announced_device_where_multicast_works() {
        let (announcer, browser) = match (Lan::new(), Lan::new()) {
            (Ok(a), Ok(b)) => (a, b),
            (a, b) => {
                eprintln!("note: skipped, mDNS unavailable: {:?}", a.err().or(b.err()));
                return;
            }
        };
        let token = Token::random();
        announcer.announce(token, 4244).unwrap();
        let mut found = browser.browse().unwrap();
        let seen = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(f) = found.next().await {
                if f.token == token {
                    return Some(f);
                }
            }
            None
        })
        .await;
        match seen {
            Ok(Some(f)) => assert!(f.addrs.iter().all(|a| a.port() == 4244), "{f:?}"),
            _ => eprintln!("note: skipped, no multicast on this machine (nothing heard in 5 s)"),
        }
        announcer.shutdown();
        browser.shutdown();
    }
}
