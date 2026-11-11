//! Reaching a device across the internet (docs/DEVICE_SYNC.md §7, FR-N6–N7, FR-N11).
//!
//! From this device's addresses and a peer's (its [`PeerRecord`] or [`Offer`]), [`candidates`]
//! makes the list to try, in order: same LAN → IPv6 → the peer's gateway-mapped port → its STUN
//! address (hole punching). [`race`] dials them all at once (later kinds a little later), from the
//! one QUIC endpoint, and keeps the first connection that completes with the expected device; the
//! rest are dropped. Hole punching is the same race started by both devices at the start time
//! they agreed through the rendezvous: each side's QUIC Initial opens its own NAT toward the other,
//! and the retransmissions get through.
//!
//! [`dial`] is the whole sequence for the device that wants to connect: the peer's record first
//! (direct paths, no start time), then an offer and the peer's answer. [`answer`] is the other
//! side, run while active at each rendezvous poll.
//!
//! **Reach `lan`** (FR-N11) is enforced here, not left to callers: [`gather`] does not ask STUN,
//! [`dial`] and [`answer`] refuse before touching the rendezvous, and [`candidates`] and [`race`]
//! drop every address that is not on a local network, so no packet goes to an internet address.
//!
//! What an attacker can do: steer nothing. Candidates come only from records and offers signed by
//! group devices ([`crate::rendezvous`]), at most [`MAX_CANDIDATES`] of them, and every connection
//! must complete a TLS handshake with the expected device's pinned key, so an address that leads
//! elsewhere (a forged STUN or gateway answer, a stale record) only costs a failed attempt. An
//! attacker on the path can drop packets (no connection; Drive or LAN still work) and can see that
//! two addresses exchange QUIC.
//!
//! A member itself could use the answers to make other devices send packets to addresses it
//! chooses (docs/SECURITY.md S2-3), so that stays small: a punch or a direct attempt goes to at
//! most [`MAX_ANSWER_TARGETS`] addresses ([`limit_targets`]), and an answerer runs one punch per
//! offerer at a time and at most [`MAX_ANSWERS_PER_OFFERER_PER_MINUTE`] a minute
//! ([`AnswerLimits`]). An honest device offers once a minute per peer, well within that.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use tokio::time::Instant;
use wasl_core::Reach;

use crate::identity::{DeviceId, DeviceKey};
use crate::portmap::Mapping;
use crate::quic::{Endpoint, QuicError, QuicLink};
use crate::rendezvous::{
    self, Addrs, Offer, Rendezvous, RendezvousError, ReplayGuard, is_lan_ip, is_public_ip, unix_now,
    unix_now_ms,
};
use crate::stun::StunConfig;

/// Most addresses tried for one peer.
pub const MAX_CANDIDATES: usize = 16;
/// Most offers answered at one poll.
pub const MAX_ANSWERS_PER_POLL: usize = 8;
/// Most addresses one punch or direct attempt sends to (S2-3).
pub const MAX_ANSWER_TARGETS: usize = 4;
/// Most offers answered for one offerer in a minute (S2-3).
pub const MAX_ANSWERS_PER_OFFERER_PER_MINUTE: usize = 4;
/// Most LAN and IPv6 addresses a device publishes.
const MAX_LAN: usize = 8;
const MAX_IPV6: usize = 4;
/// The furthest ahead an agreed start time is waited for.
const MAX_START_WAIT: Duration = Duration::from_secs(60);

/// How an address was learned, which is also the order they are tried in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Via {
    /// A local-network address.
    Lan,
    /// A public IPv6 address.
    Ipv6,
    /// A port the peer's gateway forwards.
    Mapped,
    /// The peer's STUN address, reached by hole punching.
    Punch,
}

impl Via {
    fn tier(self) -> u32 {
        match self {
            Via::Lan => 0,
            Via::Ipv6 => 1,
            Via::Mapped => 2,
            Via::Punch => 3,
        }
    }
}

/// An address to try.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    pub addr: SocketAddr,
    pub via: Via,
}

/// Why a peer could not be reached.
#[derive(Debug, thiserror::Error)]
pub enum PunchError {
    #[error("reach lan does not use the internet")]
    LanOnly,
    #[error("no address to try")]
    NoCandidates,
    #[error("{0} did not answer the offer")]
    NoAnswer(DeviceId),
    #[error("could not reach the device: {0}")]
    Unreachable(String),
    #[error(transparent)]
    Rendezvous(#[from] RendezvousError),
}

/// Timings of the race and the offer exchange.
#[derive(Debug, Clone)]
pub struct PunchConfig {
    /// The delay between kinds of candidates (LAN at once, IPv6 after one, …).
    pub stagger: Duration,
    /// How long one connection attempt may take.
    pub attempt_timeout: Duration,
    /// How long the direct attempts from a peer record may take before offering.
    pub direct_timeout: Duration,
    /// How far ahead of the answer the start time is set: enough for the offerer's next poll.
    pub lead: Duration,
    /// How long the offerer waits for an answer.
    pub answer_wait: Duration,
    /// How often the offerer looks for the answer.
    pub poll: Duration,
    /// Validity of offers and answers (at most [`rendezvous::MAX_TTL_SECS`]).
    pub offer_ttl: Duration,
}

impl Default for PunchConfig {
    fn default() -> Self {
        Self {
            stagger: Duration::from_millis(150),
            attempt_timeout: Duration::from_secs(8),
            direct_timeout: Duration::from_secs(3),
            lead: Duration::from_secs(3),
            answer_wait: Duration::from_secs(30),
            poll: Duration::from_millis(500),
            offer_ttl: Duration::from_secs(120),
        }
    }
}

/// Whether reach `reach` allows sending to `addr`.
pub fn allowed(reach: Reach, addr: SocketAddr) -> bool {
    match reach {
        Reach::Lan => is_lan_ip(addr.ip()),
        Reach::LanAndInternet => true,
    }
}

/// Whether two addresses look like the same network: the same /24 (IPv4) or /64 (IPv6).
fn same_subnet(a: IpAddr, b: IpAddr) -> bool {
    match (a.to_canonical(), b.to_canonical()) {
        (IpAddr::V4(a), IpAddr::V4(b)) => a.octets()[..3] == b.octets()[..3],
        (IpAddr::V6(a), IpAddr::V6(b)) => a.segments()[..4] == b.segments()[..4],
        _ => false,
    }
}

/// The addresses to try for a peer whose addresses are `theirs`, best first, at most
/// [`MAX_CANDIDATES`]. With reach `lan`, only local-network addresses.
pub fn candidates(reach: Reach, ours: &Addrs, theirs: &Addrs) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let mut push = |addr: SocketAddr, via: Via| {
        if allowed(reach, addr) && !out.iter().any(|c| c.addr == addr) && out.len() < MAX_CANDIDATES {
            out.push(Candidate { addr, via });
        }
    };
    let lan: Vec<SocketAddr> = theirs.lan.iter().copied().filter(|a| is_lan_ip(a.ip())).collect();
    let near = |a: &SocketAddr| ours.lan.iter().any(|o| same_subnet(o.ip(), a.ip()));
    for a in lan.iter().filter(|a| near(a)) {
        push(*a, Via::Lan);
    }
    for a in lan.iter().filter(|a| !near(a)) {
        push(*a, Via::Lan);
    }
    if reach == Reach::Lan {
        return out;
    }
    if !ours.ipv6.is_empty() {
        for a in theirs.ipv6.iter().filter(|a| is_public_ip(a.ip())) {
            push(*a, Via::Ipv6);
        }
    }
    if let Some(a) = theirs.mapped.filter(|a| is_public_ip(a.ip())) {
        push(a, Via::Mapped);
    }
    if let Some(a) = theirs.stun.filter(|a| is_public_ip(a.ip())) {
        push(a, Via::Punch);
    }
    out
}

/// At most [`MAX_ANSWER_TARGETS`] of `list` (best first), so another device's record, offer or
/// answer decides where this one sends packets only within that. One address of each internet
/// kind is kept first (a device with many LAN addresses still gets punched), then the best LAN
/// addresses fill the rest; the order stays the order they are tried in.
pub fn limit_targets(list: Vec<Candidate>) -> Vec<Candidate> {
    let mut keep = vec![false; list.len()];
    let mut kinds: Vec<Via> = Vec::new();
    for (i, c) in list.iter().enumerate() {
        if c.via != Via::Lan && !kinds.contains(&c.via) {
            kinds.push(c.via);
            keep[i] = true;
        }
    }
    let mut room = MAX_ANSWER_TARGETS.saturating_sub(kinds.len());
    for (i, c) in list.iter().enumerate() {
        if room == 0 {
            break;
        }
        if c.via == Via::Lan {
            keep[i] = true;
            room -= 1;
        }
    }
    list.into_iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(c, _)| c)
        .take(MAX_ANSWER_TARGETS)
        .collect()
}

/// How often each offerer has been answered (S2-3): one punch at a time, at most
/// [`MAX_ANSWERS_PER_OFFERER_PER_MINUTE`] a minute. Times are unix milliseconds.
#[derive(Debug, Default)]
pub struct AnswerLimits {
    /// Per offerer: until when its current punch runs, and when it was answered this minute.
    offerers: HashMap<DeviceId, (u64, VecDeque<u64>)>,
}

impl AnswerLimits {
    /// Whether an offer from `from` may be answered at `now_ms`.
    pub fn allows(&self, from: DeviceId, now_ms: u64) -> bool {
        self.offerers.get(&from).is_none_or(|(busy_until, answered)| {
            now_ms >= *busy_until
                && answered
                    .iter()
                    .filter(|t| now_ms.saturating_sub(**t) < 60_000)
                    .count()
                    < MAX_ANSWERS_PER_OFFERER_PER_MINUTE
        })
    }

    /// Notes an answer to `from` at `now_ms` whose punch runs until `until_ms`.
    pub fn record(&mut self, from: DeviceId, now_ms: u64, until_ms: u64) {
        // Offerers are members (at most a group's worth); the quiet ones are forgotten anyway.
        self.offerers.retain(|_, (busy_until, answered)| {
            answered.retain(|t| now_ms.saturating_sub(*t) < 60_000);
            *busy_until > now_ms || !answered.is_empty()
        });
        let (busy_until, answered) = self.offerers.entry(from).or_default();
        *busy_until = (*busy_until).max(until_ms);
        answered.push_back(now_ms);
    }
}

/// Something that opens a connection to a given device at an address: the QUIC [`Endpoint`], or
/// a stand-in in tests.
#[async_trait]
pub trait Dial: Sync {
    type Link: Send;
    async fn dial(&self, addr: SocketAddr, peer: DeviceId) -> Result<Self::Link, QuicError>;
}

#[async_trait]
impl Dial for Endpoint {
    type Link = QuicLink;

    async fn dial(&self, addr: SocketAddr, peer: DeviceId) -> Result<QuicLink, QuicError> {
        self.connect(addr, Some(peer)).await
    }
}

/// Dials `peer` at every candidate reach `reach` allows, at `start` (if given), later kinds
/// [`PunchConfig::stagger`] apart; the first connection that completes wins and the other
/// attempts are dropped.
pub async fn race<D: Dial>(
    dialer: &D,
    reach: Reach,
    peer: DeviceId,
    candidates: &[Candidate],
    start: Option<Instant>,
    cfg: &PunchConfig,
) -> Result<(D::Link, Candidate), PunchError> {
    let list: Vec<Candidate> = candidates
        .iter()
        .filter(|c| allowed(reach, c.addr))
        .take(MAX_CANDIDATES)
        .copied()
        .collect();
    if list.is_empty() {
        return Err(PunchError::NoCandidates);
    }
    if let Some(start) = start {
        tokio::time::sleep_until(start.min(Instant::now() + MAX_START_WAIT)).await;
    }
    let mut attempts: FuturesUnordered<_> = list
        .into_iter()
        .map(|c| async move {
            tokio::time::sleep(cfg.stagger * c.via.tier()).await;
            let result = tokio::time::timeout(cfg.attempt_timeout, dialer.dial(c.addr, peer)).await;
            (c, result)
        })
        .collect();
    let mut last = String::from("every attempt timed out");
    while let Some((candidate, result)) = attempts.next().await {
        match result {
            Ok(Ok(link)) => return Ok((link, candidate)),
            Ok(Err(e)) => last = format!("{}: {e}", candidate.addr),
            Err(_) => {}
        }
    }
    Err(PunchError::Unreachable(last))
}

/// This device's interface addresses with the QUIC `port`: private IPv4 and unique-local IPv6 as
/// `lan`, and (reach `lan_and_internet` only) public IPv6 as `ipv6`. Loopback and link-local are
/// left out.
pub fn local_addrs(port: u16, reach: Reach) -> Addrs {
    let mut addrs = Addrs::default();
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return addrs;
    };
    for iface in ifaces {
        if iface.is_loopback() {
            continue;
        }
        let ip = iface.ip();
        let link_local = match ip {
            IpAddr::V4(v4) => v4.is_link_local(),
            IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
        };
        if link_local || ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
            continue;
        }
        let addr = SocketAddr::new(ip, port);
        if is_lan_ip(ip) {
            if addrs.lan.len() < MAX_LAN && !addrs.lan.contains(&addr) {
                addrs.lan.push(addr);
            }
        } else if ip.is_ipv6()
            && reach == Reach::LanAndInternet
            && addrs.ipv6.len() < MAX_IPV6
            && !addrs.ipv6.contains(&addr)
        {
            addrs.ipv6.push(addr);
        }
    }
    addrs
}

/// The addresses to publish for `endpoint`: [`local_addrs`], and with reach `lan_and_internet`
/// the STUN address of the endpoint's socket (first of `stun_servers` that answers) and the
/// gateway mapping, if any. With reach `lan`, no STUN server is contacted.
pub async fn gather(
    endpoint: &Endpoint,
    reach: Reach,
    stun_servers: &[SocketAddr],
    mapping: Option<&Mapping>,
    cfg: &StunConfig,
) -> Addrs {
    let local = endpoint.local_addr().ok();
    let port = local.map(|a| a.port()).unwrap_or(0);
    let mut addrs = local_addrs(port, reach);
    if reach == Reach::Lan {
        return addrs;
    }
    let v4_only = local.is_some_and(|a| a.is_ipv4());
    for server in stun_servers.iter().take(MAX_CANDIDATES) {
        if v4_only && server.is_ipv6() {
            continue;
        }
        match endpoint.stun(*server, cfg).await {
            Ok(found) if is_public_ip(found.ip()) => {
                match found.ip() {
                    IpAddr::V4(_) => addrs.stun = Some(found),
                    IpAddr::V6(_) if addrs.ipv6.len() < MAX_IPV6 && !addrs.ipv6.contains(&found) => {
                        addrs.ipv6.push(found);
                    }
                    IpAddr::V6(_) => {}
                }
                break;
            }
            _ => {}
        }
    }
    addrs.mapped = mapping.map(|m| m.external).filter(|a| is_public_ip(a.ip()));
    addrs
}

/// The [`tokio::time::Instant`] of a unix time in milliseconds (now, if past).
fn instant_at(unix_ms: u64) -> Instant {
    let ahead = Duration::from_millis(unix_ms.saturating_sub(unix_now_ms()));
    Instant::now() + ahead.min(MAX_START_WAIT)
}

/// Connects to `peer` over the internet: direct paths from its record, else an offer through
/// `rv`, the peer's answer, and a race at the agreed start time. `ours` are this device's
/// addresses ([`gather`]); `key` signs the offer. Refused with reach `lan`.
#[allow(clippy::too_many_arguments)]
pub async fn dial<D: Dial>(
    dialer: &D,
    reach: Reach,
    rv: &dyn Rendezvous,
    key: &DeviceKey,
    ours: &Addrs,
    peer: DeviceId,
    trust: &(dyn Fn(&[u8; 32]) -> bool + Sync),
    cfg: &PunchConfig,
) -> Result<(D::Link, Candidate), PunchError> {
    if reach == Reach::Lan {
        return Err(PunchError::LanOnly);
    }
    let me = key.id();
    // A record that does not check is ignored: the offer may still work.
    if let Ok(Some(record)) = rendezvous::fetch_record(rv, peer, trust, unix_now()).await {
        let direct: Vec<Candidate> = limit_targets(
            candidates(reach, ours, &record.addrs)
                .into_iter()
                .filter(|c| c.via != Via::Punch)
                .collect(),
        );
        if !direct.is_empty() {
            let quick = PunchConfig {
                attempt_timeout: cfg.direct_timeout.min(cfg.attempt_timeout),
                ..cfg.clone()
            };
            if let Ok(found) = race(dialer, reach, peer, &direct, None, &quick).await {
                return Ok(found);
            }
        }
    }
    let now = unix_now();
    let start_ms = unix_now_ms() + cfg.lead.as_millis() as u64;
    let offer = Offer::new(key, peer, ours.clone(), start_ms, now, cfg.offer_ttl)?;
    let name = Offer::name(&me, &peer);
    rv.put(&name, offer.to_json()).await?;
    let result = async {
        let deadline = Instant::now() + cfg.answer_wait;
        loop {
            if let Ok(Some(answer)) =
                rendezvous::fetch_answer(rv, peer, me, &offer.nonce, trust, unix_now()).await
            {
                let list = limit_targets(candidates(reach, ours, &answer.addrs));
                return race(dialer, reach, peer, &list, Some(instant_at(answer.start_ms)), cfg).await;
            }
            if Instant::now() + cfg.poll > deadline {
                return Err(PunchError::NoAnswer(peer));
            }
            tokio::time::sleep(cfg.poll).await;
        }
    }
    .await;
    let _ = rv.delete(&name).await;
    result
}

/// Answers the offers waiting for this device in `rv` (new ones only, per `guard`; at most
/// [`MAX_ANSWERS_PER_POLL`], one per peer, within each offerer's `limits`) and races each offerer
/// at the start time it is given, at no more than [`MAX_ANSWER_TARGETS`] of its addresses. Call
/// it at every rendezvous poll while active. Refused with reach `lan`.
#[allow(clippy::too_many_arguments)]
pub async fn answer<D: Dial>(
    dialer: &D,
    reach: Reach,
    rv: &dyn Rendezvous,
    key: &DeviceKey,
    ours: &Addrs,
    trust: &(dyn Fn(&[u8; 32]) -> bool + Sync),
    is_member: &(dyn Fn(&DeviceId) -> bool + Sync),
    guard: &mut ReplayGuard,
    limits: &mut AnswerLimits,
    cfg: &PunchConfig,
) -> Result<Vec<(DeviceId, Result<(D::Link, Candidate), PunchError>)>, PunchError> {
    if reach == Reach::Lan {
        return Err(PunchError::LanOnly);
    }
    let me = key.id();
    let now = unix_now();
    let mut chosen: Vec<Offer> = Vec::new();
    for offer in rendezvous::incoming_offers(rv, me, trust, is_member, now).await? {
        if chosen.len() >= MAX_ANSWERS_PER_POLL {
            break;
        }
        // An offerer whose punch still runs, or that is over its share, waits: its offer is not
        // marked seen, so a later poll may still answer it while it is valid.
        if chosen.iter().any(|o| o.from == offer.from)
            || !limits.allows(offer.from, unix_now_ms())
            || !guard.first_time(&offer, now)
        {
            continue;
        }
        chosen.push(offer);
    }
    let mut races = Vec::new();
    for offer in &chosen {
        let now_ms = unix_now_ms();
        let start_ms = offer.start_ms.max(now_ms + cfg.lead.as_millis() as u64);
        // The punch runs until its last attempt (the slowest kind starts three staggers late)
        // gives up; a start time is waited for no further than MAX_START_WAIT.
        let until_ms = start_ms.min(now_ms + MAX_START_WAIT.as_millis() as u64)
            + (cfg.attempt_timeout + cfg.stagger * 3).as_millis() as u64;
        limits.record(offer.from, now_ms, until_ms);
        let reply = offer.answer(key, ours.clone(), start_ms, unix_now(), cfg.offer_ttl)?;
        let name = Offer::name(&me, &offer.from);
        rv.put(&name, reply.to_json()).await?;
        let list = limit_targets(candidates(reach, ours, &offer.addrs));
        races.push(async move {
            let result = race(dialer, reach, offer.from, &list, Some(instant_at(start_ms)), cfg).await;
            let _ = rv.delete(&name).await;
            (offer.from, result)
        });
    }
    Ok(futures_util::future::join_all(races).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    fn theirs() -> Addrs {
        Addrs {
            lan: vec![addr("10.9.9.9:1"), addr("192.168.1.30:1")],
            ipv6: vec![addr("[2001:db8::30]:1")],
            stun: Some(addr("198.51.100.30:2")),
            mapped: Some(addr("198.51.100.30:3")),
        }
    }

    fn ours() -> Addrs {
        Addrs {
            lan: vec![addr("192.168.1.20:1")],
            ipv6: vec![addr("[2001:db8::20]:1")],
            stun: Some(addr("198.51.100.20:2")),
            mapped: None,
        }
    }

    #[test]
    fn candidates_are_ordered_lan_ipv6_mapped_punch() {
        let list = candidates(Reach::LanAndInternet, &ours(), &theirs());
        let got: Vec<(SocketAddr, Via)> = list.iter().map(|c| (c.addr, c.via)).collect();
        assert_eq!(
            got,
            vec![
                (addr("192.168.1.30:1"), Via::Lan),
                (addr("10.9.9.9:1"), Via::Lan),
                (addr("[2001:db8::30]:1"), Via::Ipv6),
                (addr("198.51.100.30:3"), Via::Mapped),
                (addr("198.51.100.30:2"), Via::Punch),
            ]
        );
        // Without IPv6 of our own, theirs is not tried.
        let no_v6 = Addrs {
            ipv6: vec![],
            ..ours()
        };
        assert!(
            candidates(Reach::LanAndInternet, &no_v6, &theirs())
                .iter()
                .all(|c| c.via != Via::Ipv6)
        );
    }

    #[test]
    fn reach_lan_keeps_only_lan_candidates() {
        let list = candidates(Reach::Lan, &ours(), &theirs());
        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|c| c.via == Via::Lan && is_lan_ip(c.addr.ip())));
    }

    #[test]
    fn candidates_are_capped_and_unique() {
        let many = Addrs {
            lan: (1..=40).map(|p| SocketAddr::from(([10, 0, 0, 1], p))).collect(),
            ipv6: vec![],
            stun: Some(addr("198.51.100.30:2")),
            mapped: Some(addr("198.51.100.30:2")),
        };
        let list = candidates(Reach::LanAndInternet, &ours(), &many);
        assert_eq!(list.len(), MAX_CANDIDATES);
        let dup = Addrs { lan: vec![], ..many };
        assert_eq!(candidates(Reach::LanAndInternet, &ours(), &dup).len(), 1);
    }

    /// Records every address dialed; succeeds at `good`.
    struct Recorder {
        dialed: Mutex<Vec<SocketAddr>>,
        good: Option<SocketAddr>,
        delay: Duration,
    }

    impl Recorder {
        fn new(good: Option<&str>) -> Self {
            Self {
                dialed: Mutex::new(Vec::new()),
                good: good.map(addr),
                delay: Duration::from_millis(10),
            }
        }
    }

    #[async_trait]
    impl Dial for Recorder {
        type Link = SocketAddr;

        async fn dial(&self, addr: SocketAddr, _: DeviceId) -> Result<SocketAddr, QuicError> {
            self.dialed.lock().unwrap().push(addr);
            tokio::time::sleep(self.delay).await;
            if Some(addr) == self.good {
                Ok(addr)
            } else {
                Err(QuicError::Connect("nobody there".into()))
            }
        }
    }

    fn fast() -> PunchConfig {
        PunchConfig {
            stagger: Duration::from_millis(5),
            attempt_timeout: Duration::from_millis(500),
            direct_timeout: Duration::from_millis(200),
            lead: Duration::from_millis(100),
            answer_wait: Duration::from_millis(300),
            poll: Duration::from_millis(20),
            offer_ttl: Duration::from_secs(60),
        }
    }

    fn peer() -> DeviceId {
        DeviceKey::from_seed(&[9; 32]).id()
    }

    #[tokio::test]
    async fn race_with_reach_lan_never_dials_the_internet() {
        let rec = Recorder::new(None);
        let public = [
            Candidate {
                addr: addr("198.51.100.30:2"),
                via: Via::Punch,
            },
            Candidate {
                addr: addr("[2001:db8::30]:1"),
                via: Via::Ipv6,
            },
        ];
        assert!(matches!(
            race(&rec, Reach::Lan, peer(), &public, None, &fast()).await,
            Err(PunchError::NoCandidates)
        ));
        let mixed = [
            public[0],
            Candidate {
                addr: addr("192.168.1.30:1"),
                via: Via::Lan,
            },
        ];
        let _ = race(&rec, Reach::Lan, peer(), &mixed, None, &fast()).await;
        assert_eq!(*rec.dialed.lock().unwrap(), vec![addr("192.168.1.30:1")]);
    }

    #[tokio::test]
    async fn the_first_success_wins() {
        let rec = Recorder::new(Some("198.51.100.30:2"));
        let list = candidates(Reach::LanAndInternet, &ours(), &theirs());
        let (link, c) = race(&rec, Reach::LanAndInternet, peer(), &list, None, &fast())
            .await
            .unwrap();
        assert_eq!(link, addr("198.51.100.30:2"));
        assert_eq!(c.via, Via::Punch);
        assert_eq!(rec.dialed.lock().unwrap().len(), list.len(), "all tried at once");
        let none = Recorder::new(None);
        assert!(matches!(
            race(&none, Reach::LanAndInternet, peer(), &list, None, &fast()).await,
            Err(PunchError::Unreachable(_))
        ));
    }

    #[tokio::test]
    async fn race_waits_for_the_start_time() {
        let rec = Recorder::new(Some("192.168.1.30:1"));
        let list = candidates(Reach::Lan, &ours(), &theirs());
        let begin = Instant::now();
        let start = begin + Duration::from_millis(150);
        race(&rec, Reach::Lan, peer(), &list, Some(start), &fast())
            .await
            .unwrap();
        assert!(begin.elapsed() >= Duration::from_millis(150));
    }

    /// Fails the test on any use.
    struct Untouchable;

    #[async_trait]
    impl Rendezvous for Untouchable {
        async fn put(&self, _: &str, _: Vec<u8>) -> Result<(), RendezvousError> {
            panic!("reach lan wrote to the rendezvous")
        }
        async fn get(&self, _: &str) -> Result<Option<Vec<u8>>, RendezvousError> {
            panic!("reach lan read the rendezvous")
        }
        async fn list(&self, _: &str) -> Result<Vec<String>, RendezvousError> {
            panic!("reach lan listed the rendezvous")
        }
        async fn delete(&self, _: &str) -> Result<(), RendezvousError> {
            panic!("reach lan deleted from the rendezvous")
        }
    }

    #[tokio::test]
    async fn reach_lan_never_touches_the_rendezvous() {
        let key = DeviceKey::from_seed(&[1; 32]);
        let rec = Recorder::new(None);
        let anyone = |_: &[u8; 32]| true;
        assert!(matches!(
            dial(
                &rec,
                Reach::Lan,
                &Untouchable,
                &key,
                &ours(),
                peer(),
                &anyone,
                &fast()
            )
            .await,
            Err(PunchError::LanOnly)
        ));
        let mut guard = ReplayGuard::default();
        assert!(matches!(
            answer(
                &rec,
                Reach::Lan,
                &Untouchable,
                &key,
                &ours(),
                &anyone,
                &|_: &DeviceId| true,
                &mut guard,
                &mut AnswerLimits::default(),
                &fast()
            )
            .await,
            Err(PunchError::LanOnly)
        ));
        assert!(rec.dialed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_unanswered_offer_gives_up_and_is_removed() {
        let rv = rendezvous::MemoryRendezvous::new();
        let key = DeviceKey::from_seed(&[1; 32]);
        let rec = Recorder::new(None);
        let anyone = |_: &[u8; 32]| true;
        assert!(matches!(
            dial(
                &rec,
                Reach::LanAndInternet,
                &rv,
                &key,
                &ours(),
                peer(),
                &anyone,
                &fast()
            )
            .await,
            Err(PunchError::NoAnswer(_))
        ));
        assert!(rv.list("").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn offers_from_untrusted_devices_are_not_answered() {
        let rv = rendezvous::MemoryRendezvous::new();
        let (a, b) = (DeviceKey::from_seed(&[1; 32]), DeviceKey::from_seed(&[2; 32]));
        // Made before the offer: it refuses offers issued before it.
        let mut guard = ReplayGuard::default();
        let offer = Offer::new(
            &a,
            b.id(),
            theirs(),
            unix_now_ms(),
            unix_now(),
            Duration::from_secs(60),
        )
        .unwrap();
        rv.put(&Offer::name(&a.id(), &b.id()), offer.to_json())
            .await
            .unwrap();
        let rec = Recorder::new(None);
        let mut limits = AnswerLimits::default();
        let nobody = |_: &[u8; 32]| false;
        let got = answer(
            &rec,
            Reach::LanAndInternet,
            &rv,
            &b,
            &ours(),
            &nobody,
            &|_: &DeviceId| true,
            &mut guard,
            &mut limits,
            &fast(),
        )
        .await
        .unwrap();
        assert!(got.is_empty());
        assert!(rec.dialed.lock().unwrap().is_empty());
        // Trusted, it is answered once; the replay is not.
        let anyone = |_: &[u8; 32]| true;
        let got = answer(
            &rec,
            Reach::LanAndInternet,
            &rv,
            &b,
            &ours(),
            &anyone,
            &|_: &DeviceId| true,
            &mut guard,
            &mut limits,
            &fast(),
        )
        .await
        .unwrap();
        assert_eq!(got.len(), 1);
        let again = answer(
            &rec,
            Reach::LanAndInternet,
            &rv,
            &b,
            &ours(),
            &anyone,
            &|_: &DeviceId| true,
            &mut guard,
            &mut limits,
            &fast(),
        )
        .await
        .unwrap();
        assert!(again.is_empty());
    }

    #[test]
    fn an_answer_races_few_targets_and_keeps_the_punch() {
        // A member lists as many addresses as a record may hold: the answerer sends to at most
        // MAX_ANSWER_TARGETS of them, and the internet kinds survive many LAN addresses.
        let many = Addrs {
            lan: (1..=8).map(|p| SocketAddr::from(([10, 0, 0, 1], p))).collect(),
            ipv6: vec![addr("[2001:db8::30]:1"), addr("[2001:db8::31]:1")],
            stun: Some(addr("198.51.100.30:2")),
            mapped: Some(addr("198.51.100.30:3")),
        };
        let list = limit_targets(candidates(Reach::LanAndInternet, &ours(), &many));
        assert_eq!(list.len(), MAX_ANSWER_TARGETS);
        let vias: Vec<Via> = list.iter().map(|c| c.via).collect();
        assert_eq!(vias, [Via::Lan, Via::Ipv6, Via::Mapped, Via::Punch]);
        // A short list is left as it is.
        let few = candidates(Reach::LanAndInternet, &ours(), &theirs());
        let kept: Vec<SocketAddr> = limit_targets(few).iter().map(|c| c.addr).collect();
        assert_eq!(
            kept,
            [
                addr("192.168.1.30:1"),
                addr("[2001:db8::30]:1"),
                addr("198.51.100.30:3"),
                addr("198.51.100.30:2")
            ]
        );
    }

    #[test]
    fn an_offerer_gets_one_punch_at_a_time_and_a_few_a_minute() {
        let (a, b) = (peer(), DeviceKey::from_seed(&[8; 32]).id());
        let mut limits = AnswerLimits::default();
        let t = 1_800_000_000_000u64;
        assert!(limits.allows(a, t));
        limits.record(a, t, t + 10_000);
        assert!(!limits.allows(a, t + 5_000), "its punch is still running");
        assert!(limits.allows(b, t + 5_000), "another offerer is not held up");
        let mut now = t + 10_001;
        for _ in 1..MAX_ANSWERS_PER_OFFERER_PER_MINUTE {
            assert!(limits.allows(a, now));
            limits.record(a, now, now + 1);
            now += 2;
        }
        assert!(!limits.allows(a, now), "its share for this minute is used");
        assert!(limits.allows(a, t + 60_001));
    }

    #[tokio::test]
    async fn a_flood_of_offers_from_one_member_is_answered_within_its_limits() {
        // One member writes a new offer before every poll: only the first is answered while its
        // punch runs, and each answer sends to few addresses.
        let rv = rendezvous::MemoryRendezvous::new();
        let (a, b) = (DeviceKey::from_seed(&[1; 32]), DeviceKey::from_seed(&[2; 32]));
        let mut guard = ReplayGuard::new(64);
        let mut limits = AnswerLimits::default();
        let rec = Recorder::new(None);
        let anyone = |_: &[u8; 32]| true;
        let many = Addrs {
            lan: (1..=8).map(|p| SocketAddr::from(([10, 0, 0, 1], p))).collect(),
            ipv6: vec![addr("[2001:db8::30]:1")],
            stun: Some(addr("198.51.100.30:2")),
            mapped: Some(addr("198.51.100.30:3")),
        };
        let slow = PunchConfig {
            attempt_timeout: Duration::from_secs(5),
            ..fast()
        };
        let mut answered = 0;
        for _ in 0..3 {
            let offer = Offer::new(
                &a,
                b.id(),
                many.clone(),
                unix_now_ms(),
                unix_now(),
                Duration::from_secs(60),
            )
            .unwrap();
            rv.put(&Offer::name(&a.id(), &b.id()), offer.to_json())
                .await
                .unwrap();
            answered += answer(
                &rec,
                Reach::LanAndInternet,
                &rv,
                &b,
                &ours(),
                &anyone,
                &|_: &DeviceId| true,
                &mut guard,
                &mut limits,
                &slow,
            )
            .await
            .unwrap()
            .len();
        }
        assert_eq!(answered, 1);
        assert!(rec.dialed.lock().unwrap().len() <= MAX_ANSWER_TARGETS);
    }
}
