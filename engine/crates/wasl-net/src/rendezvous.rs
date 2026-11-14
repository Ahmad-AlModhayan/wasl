//! Meeting through shared storage (docs/DEVICE_SYNC.md §7): signed peer records and connection
//! offers, kept in a folder every device of the group can read and write (the daemon uses the
//! Drive app-data folder; see [`Rendezvous`]).
//!
//! - `peers/<id>.json`: a [`PeerRecord`], where a device may be reached (LAN and IPv6 addresses,
//!   the public address STUN reported, a gateway-mapped port), valid for at most
//!   [`MAX_TTL_SECS`].
//! - `offers/<from>-<to>.json`: an [`Offer`] to connect, with the offerer's addresses, a start time
//!   and a random nonce. The other device answers with an [`Offer`] of its own (`answer: true`,
//!   same nonce, the start time both use) at `offers/<to>-<from>.json`.
//!
//! - `group/<group>-<id>.json`: the group operations device `<id>` holds ([`publish_group`]), so
//!   devices that are never online together still learn of each other's additions and removals
//!   (docs/SECURITY.md S2-1). `<group>` is the first half of the group's genesis ID in hex. Each
//!   device writes only its own file (Drive has no compare-and-swap, so files shared between
//!   writers would lose news), and reads only its members' files ([`fetch_group`]), by name, so
//!   files planted under strangers' or removed devices' names are never opened. The operations
//!   carry their own signatures and are checked one by one by [`crate::group::GroupOps::insert`]
//!   as if a member had sent them; the file needs no signature of its own.
//!
//! Records and offers are canonical JSON (like the group file, [`crate::group::canonical_json`]) signed with the
//! device key over the canonical JSON without `signature`. A record or offer is used only when its
//! key hashes to the device ID it claims, that ID is the one expected (file name, recipient), the
//! caller's trust predicate (group membership) accepts the key, the signature checks, it is within
//! its validity (with [`CLOCK_SKEW_SECS`] tolerance) and it is no larger than
//! [`MAX_RECORD_BYTES`], with at most [`MAX_ADDRS`] addresses, each of the kind its list says.
//!
//! What someone with access to the folder (the storage provider, an attacker holding the account)
//! can do: read where the devices are (addresses, not identities beyond the device IDs already in
//! the group file), delete or withhold records and offers (no internet path; LAN still works), and
//! replay a still-valid signed offer, which makes the answerer try the offerer's own signed
//! addresses again (answerers remember nonces, [`ReplayGuard`]). It cannot forge a record or offer,
//! so it cannot point a device at an address no group member signed.

use std::collections::{BTreeMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::group::{GroupOp, OpId, canonical_json, hex};
use crate::identity::{DeviceId, DeviceKey, parse_public_key, verify_signature};

/// Longest validity of a record or offer.
pub const MAX_TTL_SECS: u64 = 600;
/// How far another device's clock may be off.
pub const CLOCK_SKEW_SECS: u64 = 120;
/// Largest record or offer read.
pub const MAX_RECORD_BYTES: usize = 4096;
/// Most addresses in one record or offer, all kinds together.
pub const MAX_ADDRS: usize = 16;
/// Most offer files looked at in one poll.
pub const MAX_OFFERS_SCANNED: usize = 64;
/// Where peer records live.
pub const PEERS_PREFIX: &str = "peers/";
/// Where offers live.
pub const OFFERS_PREFIX: &str = "offers/";
/// Where group files live.
pub const GROUP_PREFIX: &str = "group/";
/// Largest group file written or read: about a thousand operations of ordinary size.
pub const MAX_GROUP_FILE_BYTES: usize = 1 << 20;

/// The most bytes a store should ever hold or hand back under `name`.
pub fn max_bytes(name: &str) -> usize {
    if name.starts_with(GROUP_PREFIX) {
        MAX_GROUP_FILE_BYTES
    } else {
        MAX_RECORD_BYTES
    }
}

/// Why a record or offer was refused, or storage failed.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RendezvousError {
    #[error("record is larger than {MAX_RECORD_BYTES} bytes")]
    TooLarge,
    #[error("record is not valid: {0}")]
    Malformed(String),
    #[error("record is for another device")]
    WrongDevice,
    #[error("record key does not match its device ID")]
    WrongKey,
    #[error("record signer is not trusted")]
    Untrusted,
    #[error("record signature does not verify")]
    BadSignature,
    #[error("record has expired")]
    Expired,
    #[error("record validity is not acceptable")]
    BadTime,
    #[error("record has more than {MAX_ADDRS} addresses")]
    TooManyAddrs,
    #[error("record has an address not allowed in its place: {0}")]
    BadAddr(SocketAddr),
    #[error("rendezvous storage: {0}")]
    Storage(String),
}

/// Whether `ip` is only reachable on a local network: private and link-local IPv4, unique-local
/// and link-local IPv6, loopback.
pub fn is_lan_ip(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => v4.is_private() || v4.is_loopback() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            v6.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

/// Whether `ip` is a unicast address someone could send to at all.
fn is_unicast(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => !(v4.is_unspecified() || v4.is_multicast() || v4.is_broadcast()),
        IpAddr::V6(v6) => !(v6.is_unspecified() || v6.is_multicast()),
    }
}

/// Whether `ip` is a unicast address beyond the local network.
pub fn is_public_ip(ip: IpAddr) -> bool {
    is_unicast(ip) && !is_lan_ip(ip)
}

/// Where a device may be reached. Each list holds only its kind of address.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Addrs {
    /// Local-network addresses ([`is_lan_ip`]), IPv4 or IPv6 (not link-local IPv6: no scope).
    pub lan: Vec<SocketAddr>,
    /// Public IPv6 addresses.
    pub ipv6: Vec<SocketAddr>,
    /// The public address STUN reported for the QUIC socket.
    pub stun: Option<SocketAddr>,
    /// A port the gateway forwards (PCP / NAT-PMP).
    pub mapped: Option<SocketAddr>,
}

impl Addrs {
    /// How many addresses, all kinds together.
    pub fn len(&self) -> usize {
        self.lan.len()
            + self.ipv6.len()
            + usize::from(self.stun.is_some())
            + usize::from(self.mapped.is_some())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Checks the count and that every address is of its list's kind.
    pub fn check(&self) -> Result<(), RendezvousError> {
        if self.len() > MAX_ADDRS {
            return Err(RendezvousError::TooManyAddrs);
        }
        let usable = |a: &SocketAddr| a.port() != 0 && is_unicast(a.ip());
        for a in &self.lan {
            let link_local_v6 =
                matches!(a.ip().to_canonical(), IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80);
            if !usable(a) || !is_lan_ip(a.ip()) || link_local_v6 {
                return Err(RendezvousError::BadAddr(*a));
            }
        }
        for a in &self.ipv6 {
            if !usable(a) || !a.is_ipv6() || a.ip().to_canonical().is_ipv4() || !is_public_ip(a.ip()) {
                return Err(RendezvousError::BadAddr(*a));
            }
        }
        for a in self.stun.iter().chain(&self.mapped) {
            if !usable(a) || !is_public_ip(a.ip()) {
                return Err(RendezvousError::BadAddr(*a));
            }
        }
        Ok(())
    }
}

/// Where a device may be reached: `peers/<id>.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerRecord {
    pub device: DeviceId,
    #[serde(with = "hex")]
    pub key: [u8; 32],
    pub addrs: Addrs,
    /// Unix seconds.
    pub issued_at: u64,
    /// Unix seconds; at most [`MAX_TTL_SECS`] after `issued_at`.
    pub expires_at: u64,
    #[serde(with = "hex")]
    pub signature: [u8; 64],
}

impl PeerRecord {
    /// A record for `key`'s device, signed, valid for `ttl` (at most [`MAX_TTL_SECS`]) from `now`
    /// (unix seconds).
    pub fn new(key: &DeviceKey, addrs: Addrs, now: u64, ttl: Duration) -> Result<Self, RendezvousError> {
        addrs.check()?;
        let mut record = Self {
            device: key.id(),
            key: key.public(),
            addrs,
            issued_at: now,
            expires_at: now + ttl.as_secs().clamp(1, MAX_TTL_SECS),
            signature: [0; 64],
        };
        record.signature = key.sign(&signed_bytes(&record));
        Ok(record)
    }

    /// The file name of `id`'s record.
    pub fn name(id: &DeviceId) -> String {
        format!("{PEERS_PREFIX}{id}.json")
    }

    /// Canonical JSON, signature included.
    pub fn to_json(&self) -> Vec<u8> {
        canonical(self)
    }

    /// Parses and checks a record that should be `expected`'s; see the module docs.
    pub fn verify(
        bytes: &[u8],
        expected: DeviceId,
        trust: &dyn Fn(&[u8; 32]) -> bool,
        now: u64,
    ) -> Result<Self, RendezvousError> {
        let record: Self = parse(bytes)?;
        if record.device != expected {
            return Err(RendezvousError::WrongDevice);
        }
        check_signer(record.device, &record.key, trust)?;
        if !verify_signature(&record.key, &signed_bytes(&record), &record.signature) {
            return Err(RendezvousError::BadSignature);
        }
        check_validity(record.issued_at, record.expires_at, now)?;
        record.addrs.check()?;
        Ok(record)
    }
}

/// An offer to connect, or the answer to one: `offers/<from>-<to>.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Offer {
    pub from: DeviceId,
    #[serde(with = "hex")]
    pub key: [u8; 32],
    pub to: DeviceId,
    /// Where `from` may be reached.
    pub addrs: Addrs,
    /// When both send (unix milliseconds). In an offer, the earliest; the answer's is the one used.
    pub start_ms: u64,
    #[serde(with = "hex")]
    pub nonce: [u8; 16],
    /// An answer to the offer with the same nonce.
    pub answer: bool,
    /// Unix seconds.
    pub issued_at: u64,
    /// Unix seconds; at most [`MAX_TTL_SECS`] after `issued_at`.
    pub expires_at: u64,
    #[serde(with = "hex")]
    pub signature: [u8; 64],
}

impl Offer {
    /// A signed offer from `key`'s device to `to`, starting no earlier than `start_ms`, valid for
    /// `ttl` from `now` (unix seconds).
    pub fn new(
        key: &DeviceKey,
        to: DeviceId,
        addrs: Addrs,
        start_ms: u64,
        now: u64,
        ttl: Duration,
    ) -> Result<Self, RendezvousError> {
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).expect("the OS random source is available");
        Self::signed(key, to, addrs, start_ms, nonce, false, now, ttl)
    }

    /// The answer to this offer from `key`'s device (its recipient): same nonce, the start time
    /// both will use.
    pub fn answer(
        &self,
        key: &DeviceKey,
        addrs: Addrs,
        start_ms: u64,
        now: u64,
        ttl: Duration,
    ) -> Result<Self, RendezvousError> {
        if key.id() != self.to {
            return Err(RendezvousError::WrongDevice);
        }
        Self::signed(key, self.from, addrs, start_ms, self.nonce, true, now, ttl)
    }

    #[allow(clippy::too_many_arguments)]
    fn signed(
        key: &DeviceKey,
        to: DeviceId,
        addrs: Addrs,
        start_ms: u64,
        nonce: [u8; 16],
        answer: bool,
        now: u64,
        ttl: Duration,
    ) -> Result<Self, RendezvousError> {
        addrs.check()?;
        let mut offer = Self {
            from: key.id(),
            key: key.public(),
            to,
            addrs,
            start_ms,
            nonce,
            answer,
            issued_at: now,
            expires_at: now + ttl.as_secs().clamp(1, MAX_TTL_SECS),
            signature: [0; 64],
        };
        check_start(offer.start_ms, offer.issued_at, offer.expires_at)?;
        offer.signature = key.sign(&signed_bytes(&offer));
        Ok(offer)
    }

    /// The file name of an offer (or answer) from `from` to `to`.
    pub fn name(from: &DeviceId, to: &DeviceId) -> String {
        format!("{OFFERS_PREFIX}{from}-{to}.json")
    }

    /// Canonical JSON, signature included.
    pub fn to_json(&self) -> Vec<u8> {
        canonical(self)
    }

    /// Parses and checks an offer or answer that should be from `from` to `me`; see the module
    /// docs. `now` in unix seconds.
    pub fn verify(
        bytes: &[u8],
        from: DeviceId,
        me: DeviceId,
        trust: &dyn Fn(&[u8; 32]) -> bool,
        now: u64,
    ) -> Result<Self, RendezvousError> {
        let offer: Self = parse(bytes)?;
        if offer.from != from || offer.to != me || offer.from == offer.to {
            return Err(RendezvousError::WrongDevice);
        }
        check_signer(offer.from, &offer.key, trust)?;
        if !verify_signature(&offer.key, &signed_bytes(&offer), &offer.signature) {
            return Err(RendezvousError::BadSignature);
        }
        check_validity(offer.issued_at, offer.expires_at, now)?;
        check_start(offer.start_ms, offer.issued_at, offer.expires_at)?;
        offer.addrs.check()?;
        Ok(offer)
    }
}

fn parse<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, RendezvousError> {
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(RendezvousError::TooLarge);
    }
    serde_json::from_slice(bytes).map_err(|e| RendezvousError::Malformed(e.to_string()))
}

fn check_signer(
    id: DeviceId,
    key: &[u8; 32],
    trust: &dyn Fn(&[u8; 32]) -> bool,
) -> Result<(), RendezvousError> {
    if DeviceId::from_public_key(key) != id || parse_public_key(key).is_err() {
        return Err(RendezvousError::WrongKey);
    }
    if !trust(key) {
        return Err(RendezvousError::Untrusted);
    }
    Ok(())
}

fn check_validity(issued_at: u64, expires_at: u64, now: u64) -> Result<(), RendezvousError> {
    if expires_at <= issued_at || expires_at - issued_at > MAX_TTL_SECS {
        return Err(RendezvousError::BadTime);
    }
    if issued_at > now.saturating_add(CLOCK_SKEW_SECS) {
        return Err(RendezvousError::BadTime);
    }
    if now > expires_at.saturating_add(CLOCK_SKEW_SECS) {
        return Err(RendezvousError::Expired);
    }
    Ok(())
}

/// The start time must fall within the offer's validity (give or take the skew).
fn check_start(start_ms: u64, issued_at: u64, expires_at: u64) -> Result<(), RendezvousError> {
    let earliest = issued_at.saturating_sub(CLOCK_SKEW_SECS).saturating_mul(1000);
    let latest = expires_at.saturating_mul(1000);
    if start_ms < earliest || start_ms > latest {
        return Err(RendezvousError::BadTime);
    }
    Ok(())
}

fn canonical<T: Serialize>(value: &T) -> Vec<u8> {
    let value = serde_json::to_value(value).expect("records are always JSON");
    canonical_json(&value).into_bytes()
}

/// The bytes a signature covers: canonical JSON without `signature`.
fn signed_bytes<T: Serialize>(value: &T) -> Vec<u8> {
    let mut value = serde_json::to_value(value).expect("records are always JSON");
    if let Value::Object(map) = &mut value {
        map.remove("signature");
    }
    canonical_json(&value).into_bytes()
}

/// Now, in unix seconds.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Now, in unix milliseconds.
pub fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Shared storage for records and offers: flat names like `peers/<id>.json`. The daemon
/// implements it over the Drive app-data folder; [`MemoryRendezvous`] is for tests.
/// Implementations should refuse to download more than [`MAX_RECORD_BYTES`] for one name.
#[async_trait]
pub trait Rendezvous: Send + Sync {
    /// Writes `name`, replacing it.
    async fn put(&self, name: &str, bytes: Vec<u8>) -> Result<(), RendezvousError>;
    /// Reads `name`; `None` if absent.
    async fn get(&self, name: &str) -> Result<Option<Vec<u8>>, RendezvousError>;
    /// The names starting with `prefix`.
    async fn list(&self, prefix: &str) -> Result<Vec<String>, RendezvousError>;
    /// Deletes `name`; absent is fine.
    async fn delete(&self, name: &str) -> Result<(), RendezvousError>;
}

/// A [`Rendezvous`] in memory.
#[derive(Debug, Default)]
pub struct MemoryRendezvous {
    files: Mutex<BTreeMap<String, Vec<u8>>>,
}

impl MemoryRendezvous {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl Rendezvous for MemoryRendezvous {
    async fn put(&self, name: &str, bytes: Vec<u8>) -> Result<(), RendezvousError> {
        self.files.lock().expect("lock").insert(name.to_string(), bytes);
        Ok(())
    }

    async fn get(&self, name: &str) -> Result<Option<Vec<u8>>, RendezvousError> {
        Ok(self.files.lock().expect("lock").get(name).cloned())
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>, RendezvousError> {
        Ok(self
            .files
            .lock()
            .expect("lock")
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect())
    }

    async fn delete(&self, name: &str) -> Result<(), RendezvousError> {
        self.files.lock().expect("lock").remove(name);
        Ok(())
    }
}

/// Publishes `record` at its name.
pub async fn publish(rv: &dyn Rendezvous, record: &PeerRecord) -> Result<(), RendezvousError> {
    rv.put(&PeerRecord::name(&record.device), record.to_json()).await
}

/// Reads and checks `peer`'s record; `None` if it has none.
pub async fn fetch_record(
    rv: &dyn Rendezvous,
    peer: DeviceId,
    trust: &(dyn Fn(&[u8; 32]) -> bool + Sync),
    now: u64,
) -> Result<Option<PeerRecord>, RendezvousError> {
    match rv.get(&PeerRecord::name(&peer)).await? {
        Some(bytes) => PeerRecord::verify(&bytes, peer, trust, now).map(Some),
        None => Ok(None),
    }
}

/// The valid offers (not answers) waiting for `me` from devices `is_member` accepts, at most
/// [`MAX_OFFERS_SCANNED`] files looked at. Invalid ones are skipped. Names from anyone else are
/// passed over before the limit is counted (security review 2026-10), so files planted under
/// strangers' names cannot crowd out a member's offer.
pub async fn incoming_offers(
    rv: &dyn Rendezvous,
    me: DeviceId,
    trust: &(dyn Fn(&[u8; 32]) -> bool + Sync),
    is_member: &(dyn Fn(&DeviceId) -> bool + Sync),
    now: u64,
) -> Result<Vec<Offer>, RendezvousError> {
    let suffix = format!("-{me}.json");
    let mut out = Vec::new();
    let names = rv.list(OFFERS_PREFIX).await?;
    let from_members = names.iter().filter_map(|name| {
        let from = name
            .strip_prefix(OFFERS_PREFIX)
            .and_then(|n| n.strip_suffix(&suffix))
            .and_then(|f| DeviceId::parse(f).ok())?;
        is_member(&from).then_some((name, from))
    });
    for (name, from) in from_members.take(MAX_OFFERS_SCANNED) {
        let Ok(Some(bytes)) = rv.get(name).await else {
            continue;
        };
        if let Ok(offer) = Offer::verify(&bytes, from, me, trust, now)
            && !offer.answer
        {
            out.push(offer);
        }
    }
    Ok(out)
}

/// `peer`'s answer to our offer `nonce`, if it is there and valid.
pub async fn fetch_answer(
    rv: &dyn Rendezvous,
    peer: DeviceId,
    me: DeviceId,
    nonce: &[u8; 16],
    trust: &(dyn Fn(&[u8; 32]) -> bool + Sync),
    now: u64,
) -> Result<Option<Offer>, RendezvousError> {
    let Some(bytes) = rv.get(&Offer::name(&peer, &me)).await? else {
        return Ok(None);
    };
    match Offer::verify(&bytes, peer, me, trust, now) {
        Ok(answer) if answer.answer && &answer.nonce == nonce => Ok(Some(answer)),
        _ => Ok(None),
    }
}

/// The group's tag in file names: the first half of its genesis ID, in hex.
pub fn group_tag(genesis: &OpId) -> String {
    genesis.0[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// The name of the file holding `device`'s group operations.
pub fn group_file_name(genesis: &OpId, device: &DeviceId) -> String {
    format!("{GROUP_PREFIX}{}-{device}.json", group_tag(genesis))
}

/// The group tag and device of a group file name; `None` unless the name is exactly what
/// [`group_file_name`] writes.
pub fn parse_group_file_name(name: &str) -> Option<(String, DeviceId)> {
    let rest = name.strip_prefix(GROUP_PREFIX)?.strip_suffix(".json")?;
    let (tag, device) = rest.split_once('-')?;
    let device = DeviceId::parse(device).ok()?;
    let canonical = tag.len() == 32 && tag.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'));
    (canonical && name == format!("{GROUP_PREFIX}{tag}-{device}.json")).then(|| (tag.to_string(), device))
}

/// `ops` (parents first) as a group file of at most `limit` bytes: the newest that fit, still
/// parents first. A device that reads it already holds the oldest, or gets them from a member.
pub fn group_file(ops: &[GroupOp], limit: usize) -> Vec<u8> {
    let encoded: Vec<Vec<u8>> = ops.iter().map(GroupOp::to_json).collect();
    let mut size = 2;
    let mut first = encoded.len();
    for (i, e) in encoded.iter().enumerate().rev() {
        // A comma before every entry but the newest.
        let more = e.len() + usize::from(first < encoded.len());
        if size + more > limit {
            break;
        }
        size += more;
        first = i;
    }
    let mut out = Vec::with_capacity(size);
    out.push(b'[');
    for (n, e) in encoded[first..].iter().enumerate() {
        if n > 0 {
            out.push(b',');
        }
        out.extend_from_slice(e);
    }
    out.push(b']');
    out
}

/// The operations in a group file, bounded: at most [`MAX_GROUP_FILE_BYTES`] and
/// [`crate::group::MAX_OPS`] entries. Entries that are not operations are skipped. Nothing here
/// is trusted yet.
pub fn parse_group_file(bytes: &[u8]) -> Result<Vec<GroupOp>, RendezvousError> {
    if bytes.len() > MAX_GROUP_FILE_BYTES {
        return Err(RendezvousError::TooLarge);
    }
    let entries: Vec<Value> =
        serde_json::from_slice(bytes).map_err(|e| RendezvousError::Malformed(e.to_string()))?;
    Ok(entries
        .into_iter()
        .take(crate::group::MAX_OPS)
        .filter_map(|v| serde_json::from_value::<GroupOp>(v).ok())
        .filter(|op| op.to_json().len() <= crate::group::MAX_OP_BYTES)
        .collect())
}

/// Writes `me`'s group file: `ops` (parents first), the newest that fit.
pub async fn publish_group(
    rv: &dyn Rendezvous,
    genesis: OpId,
    me: DeviceId,
    ops: &[GroupOp],
) -> Result<(), RendezvousError> {
    rv.put(
        &group_file_name(&genesis, &me),
        group_file(ops, MAX_GROUP_FILE_BYTES),
    )
    .await
}

/// The group files of `members` (but `me`) that are there, each as the operations it holds. Only
/// these names are opened; a file that cannot be read or parsed is skipped.
pub async fn fetch_group(
    rv: &dyn Rendezvous,
    genesis: OpId,
    me: DeviceId,
    members: &[DeviceId],
) -> Result<Vec<(DeviceId, Vec<GroupOp>)>, RendezvousError> {
    let present = rv.list(GROUP_PREFIX).await?;
    let mut out = Vec::new();
    for m in members
        .iter()
        .filter(|m| **m != me)
        .take(crate::group::MAX_DEVICES)
    {
        let name = group_file_name(&genesis, m);
        if !present.contains(&name) {
            continue;
        }
        if let Ok(Some(bytes)) = rv.get(&name).await
            && let Ok(ops) = parse_group_file(&bytes)
        {
            out.push((*m, ops));
        }
    }
    Ok(out)
}

/// The offers an answerer has already answered, so a replayed offer is not answered again.
#[derive(Debug)]
pub struct ReplayGuard {
    seen: VecDeque<(DeviceId, [u8; 16], u64)>,
    capacity: usize,
    /// Offers issued before this (unix seconds) are refused.
    not_before: u64,
}

impl Default for ReplayGuard {
    /// Refuses offers issued before now: the nonces seen before a restart are not kept, so an
    /// offer from then could otherwise be answered again (security review 2026-10). A member
    /// whose offer is lost this way offers again at its next dial.
    fn default() -> Self {
        Self::new(256).not_before(unix_now())
    }
}

impl ReplayGuard {
    pub fn new(capacity: usize) -> Self {
        Self {
            seen: VecDeque::new(),
            capacity: capacity.max(1),
            not_before: 0,
        }
    }

    /// Refuses offers issued before `issued` (unix seconds).
    pub fn not_before(mut self, issued: u64) -> Self {
        self.not_before = issued;
        self
    }

    /// Whether `offer` is new; remembers it if so (until it expires, or until `capacity` newer
    /// ones push it out, by when it has expired anyway at the poll rates in use).
    pub fn first_time(&mut self, offer: &Offer, now: u64) -> bool {
        if offer.issued_at < self.not_before {
            return false;
        }
        self.seen
            .retain(|(_, _, expires)| expires.saturating_add(CLOCK_SKEW_SECS) >= now);
        if self
            .seen
            .iter()
            .any(|(from, nonce, _)| *from == offer.from && *nonce == offer.nonce)
        {
            return false;
        }
        if self.seen.len() >= self.capacity {
            self.seen.pop_front();
        }
        self.seen.push_back((offer.from, offer.nonce, offer.expires_at));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;

    fn key(n: u8) -> DeviceKey {
        DeviceKey::from_seed(&[n; 32])
    }

    fn addrs() -> Addrs {
        Addrs {
            lan: vec![
                "192.168.1.20:4433".parse().unwrap(),
                "[fd00::20]:4433".parse().unwrap(),
            ],
            ipv6: vec!["[2001:db8::20]:4433".parse().unwrap()],
            stun: Some("198.51.100.7:40001".parse().unwrap()),
            mapped: Some("198.51.100.7:40000".parse().unwrap()),
        }
    }

    fn anyone(_: &[u8; 32]) -> bool {
        true
    }

    fn any_device(_: &DeviceId) -> bool {
        true
    }

    #[test]
    fn a_record_round_trips() {
        let k = key(1);
        let r = PeerRecord::new(&k, addrs(), NOW, Duration::from_secs(300)).unwrap();
        let json = r.to_json();
        assert_eq!(PeerRecord::verify(&json, k.id(), &anyone, NOW + 10).unwrap(), r);
        assert!(PeerRecord::name(&k.id()).starts_with("peers/WASL-"));
        // Canonical: keys sorted, no whitespace.
        let text = String::from_utf8(json).unwrap();
        assert!(text.starts_with("{\"addrs\":{\"ipv6\":"), "{text}");
        assert!(!text.contains(' '));
    }

    #[test]
    fn a_record_ttl_is_capped() {
        let r = PeerRecord::new(&key(1), Addrs::default(), NOW, Duration::from_secs(86_400)).unwrap();
        assert_eq!(r.expires_at - r.issued_at, MAX_TTL_SECS);
    }

    #[test]
    fn records_are_refused_when_they_should_be() {
        let k = key(1);
        let r = PeerRecord::new(&k, addrs(), NOW, Duration::from_secs(300)).unwrap();
        let json = r.to_json();
        // Another device's name, an untrusted signer, the past, the future.
        assert_eq!(
            PeerRecord::verify(&json, key(2).id(), &anyone, NOW),
            Err(RendezvousError::WrongDevice)
        );
        assert_eq!(
            PeerRecord::verify(&json, k.id(), &|_| false, NOW),
            Err(RendezvousError::Untrusted)
        );
        assert_eq!(
            PeerRecord::verify(&json, k.id(), &anyone, NOW + 300 + CLOCK_SKEW_SECS + 1),
            Err(RendezvousError::Expired)
        );
        assert_eq!(
            PeerRecord::verify(&json, k.id(), &anyone, NOW - CLOCK_SKEW_SECS - 1),
            Err(RendezvousError::BadTime)
        );
        assert!(PeerRecord::verify(&json, k.id(), &anyone, NOW - CLOCK_SKEW_SECS).is_ok());
        // A changed address breaks the signature.
        let mut forged = r.clone();
        forged.addrs.stun = Some("203.0.113.66:1".parse().unwrap());
        assert_eq!(
            PeerRecord::verify(&forged.to_json(), k.id(), &anyone, NOW),
            Err(RendezvousError::BadSignature)
        );
        // Another device's key under this device's ID.
        let mut swapped = r.clone();
        swapped.key = key(2).public();
        assert_eq!(
            PeerRecord::verify(&swapped.to_json(), k.id(), &anyone, NOW),
            Err(RendezvousError::WrongKey)
        );
        // A signed record with a too-long life.
        let mut long = r.clone();
        long.expires_at = long.issued_at + MAX_TTL_SECS + 1;
        long.signature = k.sign(&signed_bytes(&long));
        assert_eq!(
            PeerRecord::verify(&long.to_json(), k.id(), &anyone, NOW),
            Err(RendezvousError::BadTime)
        );
        // Unknown fields, garbage, too large.
        let mut extra: Value = serde_json::from_slice(&json).unwrap();
        extra["relay"] = Value::String("evil".into());
        assert!(matches!(
            PeerRecord::verify(extra.to_string().as_bytes(), k.id(), &anyone, NOW),
            Err(RendezvousError::Malformed(_))
        ));
        assert!(matches!(
            PeerRecord::verify(b"[]", k.id(), &anyone, NOW),
            Err(RendezvousError::Malformed(_))
        ));
        assert_eq!(
            PeerRecord::verify(&vec![b' '; MAX_RECORD_BYTES + 1], k.id(), &anyone, NOW),
            Err(RendezvousError::TooLarge)
        );
    }

    #[test]
    fn signed_records_with_bad_addresses_are_refused() {
        let k = key(1);
        let sign = |addrs: Addrs| {
            let mut r = PeerRecord {
                device: k.id(),
                key: k.public(),
                addrs,
                issued_at: NOW,
                expires_at: NOW + 60,
                signature: [0; 64],
            };
            r.signature = k.sign(&signed_bytes(&r));
            PeerRecord::verify(&r.to_json(), k.id(), &anyone, NOW)
        };
        let too_many = Addrs {
            lan: (1..=MAX_ADDRS as u16 + 1)
                .map(|p| SocketAddr::from(([10, 0, 0, 1], p)))
                .collect(),
            ..Addrs::default()
        };
        assert_eq!(sign(too_many), Err(RendezvousError::TooManyAddrs));
        let bad = [
            Addrs {
                lan: vec!["8.8.8.8:1".parse().unwrap()],
                ..Addrs::default()
            },
            Addrs {
                lan: vec!["[fe80::1]:1".parse().unwrap()],
                ..Addrs::default()
            },
            Addrs {
                ipv6: vec!["1.2.3.4:1".parse().unwrap()],
                ..Addrs::default()
            },
            Addrs {
                ipv6: vec!["[fd00::1]:1".parse().unwrap()],
                ..Addrs::default()
            },
            Addrs {
                stun: Some("192.168.1.1:1".parse().unwrap()),
                ..Addrs::default()
            },
            Addrs {
                mapped: Some("255.255.255.255:1".parse().unwrap()),
                ..Addrs::default()
            },
            Addrs {
                stun: Some("224.0.0.251:5353".parse().unwrap()),
                ..Addrs::default()
            },
            Addrs {
                stun: Some("1.2.3.4:0".parse().unwrap()),
                ..Addrs::default()
            },
        ];
        for addrs in bad {
            assert!(
                matches!(sign(addrs.clone()), Err(RendezvousError::BadAddr(_))),
                "{addrs:?}"
            );
            assert!(PeerRecord::new(&k, addrs, NOW, Duration::from_secs(60)).is_err());
        }
    }

    #[test]
    fn an_offer_and_its_answer() {
        let (a, b) = (key(1), key(2));
        let start = NOW * 1000 + 3000;
        let offer = Offer::new(&a, b.id(), addrs(), start, NOW, Duration::from_secs(60)).unwrap();
        let name = Offer::name(&a.id(), &b.id());
        assert!(name.starts_with("offers/WASL-") && name.ends_with(&format!("-{}.json", b.id())));
        let got = Offer::verify(&offer.to_json(), a.id(), b.id(), &anyone, NOW).unwrap();
        assert_eq!(got, offer);
        // Only for its recipient, only from its sender.
        assert_eq!(
            Offer::verify(&offer.to_json(), a.id(), key(3).id(), &anyone, NOW),
            Err(RendezvousError::WrongDevice)
        );
        assert_eq!(
            Offer::verify(&offer.to_json(), key(3).id(), b.id(), &anyone, NOW),
            Err(RendezvousError::WrongDevice)
        );
        let answer = got
            .answer(&b, Addrs::default(), start + 1000, NOW, Duration::from_secs(60))
            .unwrap();
        assert!(answer.answer);
        assert_eq!(answer.nonce, offer.nonce);
        assert_eq!(
            Offer::verify(&answer.to_json(), b.id(), a.id(), &anyone, NOW).unwrap(),
            answer
        );
        // Only the recipient answers.
        assert_eq!(
            got.answer(&key(3), Addrs::default(), start, NOW, Duration::from_secs(60)),
            Err(RendezvousError::WrongDevice)
        );
        // A start time outside the validity.
        assert_eq!(
            Offer::new(
                &a,
                b.id(),
                Addrs::default(),
                (NOW + 3600) * 1000,
                NOW,
                Duration::from_secs(60)
            ),
            Err(RendezvousError::BadTime)
        );
    }

    #[test]
    fn the_replay_guard_answers_once() {
        let offer = Offer::new(
            &key(1),
            key(2).id(),
            Addrs::default(),
            NOW * 1000,
            NOW,
            Duration::from_secs(60),
        )
        .unwrap();
        let mut guard = ReplayGuard::new(2);
        assert!(guard.first_time(&offer, NOW));
        assert!(!guard.first_time(&offer, NOW + 1));
        let other = Offer::new(
            &key(1),
            key(2).id(),
            Addrs::default(),
            NOW * 1000,
            NOW,
            Duration::from_secs(60),
        )
        .unwrap();
        assert!(guard.first_time(&other, NOW));
        // Expired entries are forgotten (the offer itself would be refused as expired).
        assert!(guard.first_time(&offer, NOW + 60 + CLOCK_SKEW_SECS + 1));
    }

    #[test]
    fn an_offer_from_before_a_restart_is_not_answered_again() {
        let offer = Offer::new(
            &key(1),
            key(2).id(),
            Addrs::default(),
            NOW * 1000,
            NOW,
            Duration::from_secs(600),
        )
        .unwrap();
        // Answered before the restart; the new guard has forgotten its nonce.
        assert!(ReplayGuard::new(8).not_before(NOW).first_time(&offer, NOW));
        let mut restarted = ReplayGuard::new(8).not_before(NOW + 30);
        assert!(!restarted.first_time(&offer, NOW + 31));
        let fresh = Offer::new(
            &key(1),
            key(2).id(),
            Addrs::default(),
            (NOW + 40) * 1000,
            NOW + 40,
            Duration::from_secs(60),
        )
        .unwrap();
        assert!(restarted.first_time(&fresh, NOW + 41));
        assert!(ReplayGuard::default().not_before > 0);
    }

    #[tokio::test]
    async fn offers_from_strangers_never_crowd_out_a_member() {
        let rv = MemoryRendezvous::new();
        let (a, b) = (key(1), key(2));
        // More planted offers than one poll looks at, all named before the member's.
        for n in 0..(MAX_OFFERS_SCANNED as u8 + 4) {
            let stranger = DeviceKey::from_seed(&[100u8.wrapping_add(n); 32]);
            let o = Offer::new(
                &stranger,
                b.id(),
                addrs(),
                NOW * 1000,
                NOW,
                Duration::from_secs(60),
            )
            .unwrap();
            rv.put(&Offer::name(&stranger.id(), &b.id()), o.to_json())
                .await
                .unwrap();
        }
        let good = Offer::new(&a, b.id(), addrs(), NOW * 1000, NOW, Duration::from_secs(60)).unwrap();
        rv.put(&Offer::name(&a.id(), &b.id()), good.to_json())
            .await
            .unwrap();
        let member = a.id();
        let got = incoming_offers(&rv, b.id(), &anyone, &|d: &DeviceId| *d == member, NOW)
            .await
            .unwrap();
        assert_eq!(got, vec![good]);
    }

    #[tokio::test]
    async fn storage_helpers() {
        let rv = MemoryRendezvous::new();
        let (a, b, c) = (key(1), key(2), key(3));
        let record = PeerRecord::new(&a, addrs(), NOW, Duration::from_secs(60)).unwrap();
        publish(&rv, &record).await.unwrap();
        assert_eq!(
            fetch_record(&rv, a.id(), &anyone, NOW).await.unwrap(),
            Some(record)
        );
        assert_eq!(fetch_record(&rv, b.id(), &anyone, NOW).await.unwrap(), None);
        // Offers to b: one good, one tampered, one to someone else, one junk name.
        let good = Offer::new(&a, b.id(), addrs(), NOW * 1000, NOW, Duration::from_secs(60)).unwrap();
        rv.put(&Offer::name(&a.id(), &b.id()), good.to_json())
            .await
            .unwrap();
        let mut bad = Offer::new(&c, b.id(), addrs(), NOW * 1000, NOW, Duration::from_secs(60)).unwrap();
        bad.addrs.stun = Some("203.0.113.66:1".parse().unwrap());
        rv.put(&Offer::name(&c.id(), &b.id()), bad.to_json())
            .await
            .unwrap();
        let other = Offer::new(&a, c.id(), addrs(), NOW * 1000, NOW, Duration::from_secs(60)).unwrap();
        rv.put(&Offer::name(&a.id(), &c.id()), other.to_json())
            .await
            .unwrap();
        rv.put(&format!("offers/nonsense-{}.json", b.id()), b"{}".to_vec())
            .await
            .unwrap();
        assert_eq!(
            incoming_offers(&rv, b.id(), &anyone, &any_device, NOW)
                .await
                .unwrap(),
            vec![good.clone()]
        );
        // The answer is found only with the right nonce.
        let answer = good
            .answer(&b, addrs(), NOW * 1000, NOW, Duration::from_secs(60))
            .unwrap();
        rv.put(&Offer::name(&b.id(), &a.id()), answer.to_json())
            .await
            .unwrap();
        assert_eq!(
            fetch_answer(&rv, b.id(), a.id(), &good.nonce, &anyone, NOW)
                .await
                .unwrap(),
            Some(answer)
        );
        assert_eq!(
            fetch_answer(&rv, b.id(), a.id(), &[0; 16], &anyone, NOW)
                .await
                .unwrap(),
            None
        );
        // An answer is not an incoming offer.
        assert!(
            incoming_offers(&rv, a.id(), &anyone, &any_device, NOW)
                .await
                .unwrap()
                .is_empty()
        );
        rv.delete(&Offer::name(&b.id(), &a.id())).await.unwrap();
        assert_eq!(rv.list("offers/").await.unwrap().len(), 4);
    }

    #[test]
    fn address_kinds() {
        for lan in [
            "10.1.2.3",
            "172.16.0.1",
            "192.168.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "fd12::1",
            "fe80::1",
            "::1",
            "::ffff:192.168.1.1",
        ] {
            assert!(is_lan_ip(lan.parse().unwrap()), "{lan}");
        }
        for public in ["8.8.8.8", "198.51.100.7", "2001:db8::1", "100.64.0.1"] {
            assert!(is_public_ip(public.parse().unwrap()), "{public}");
        }
        for neither in ["0.0.0.0", "224.0.0.1", "255.255.255.255", "::", "ff02::1"] {
            assert!(!is_public_ip(neither.parse().unwrap()), "{neither}");
        }
    }

    /// A group of A and B, with C added and then removed by A.
    fn group_with_a_removal() -> (crate::group::GroupOps, DeviceKey, DeviceKey, DeviceKey) {
        let (a, b, c) = (key(1), key(2), key(3));
        let (mut g, _) = crate::group::GroupOps::create(&a, "Laptop", NOW).unwrap();
        for (k, name) in [(&b, "Phone"), (&c, "Tablet")] {
            let op = g.add_device(k.public(), name, NOW, &a).unwrap();
            g.insert(op).unwrap();
        }
        let rm = g.remove_device(c.id(), &a).unwrap();
        g.insert(rm).unwrap();
        (g, a, b, c)
    }

    #[test]
    fn group_file_names_are_whitelisted() {
        let (g, a, _, _) = group_with_a_removal();
        let name = group_file_name(&g.genesis(), &a.id());
        assert!(
            name.starts_with(GROUP_PREFIX) && name.ends_with(".json"),
            "{name}"
        );
        assert!(name.len() < 200);
        assert_eq!(
            parse_group_file_name(&name),
            Some((group_tag(&g.genesis()), a.id()))
        );
        for bad in [
            "group/x.json".to_string(),
            name.replace(".json", ".txt"),
            name.replace("group/", "peers/"),
            name.to_lowercase(),
            format!("{name}/../x.json"),
        ] {
            assert_eq!(parse_group_file_name(&bad), None, "{bad}");
        }
    }

    #[test]
    fn a_group_file_round_trips_and_is_bounded() {
        let (g, ..) = group_with_a_removal();
        let ops: Vec<crate::group::GroupOp> = g.ops().into_iter().cloned().collect();
        let bytes = group_file(&ops, MAX_GROUP_FILE_BYTES);
        assert_eq!(parse_group_file(&bytes).unwrap(), ops);
        // Too little room: the newest operations are kept, still in order.
        let one = ops.last().unwrap().to_json().len() + 2;
        assert_eq!(
            parse_group_file(&group_file(&ops, one)).unwrap(),
            vec![ops.last().unwrap().clone()]
        );
        assert_eq!(
            parse_group_file(&vec![b' '; MAX_GROUP_FILE_BYTES + 1]),
            Err(RendezvousError::TooLarge)
        );
        assert!(matches!(
            parse_group_file(b"{}"),
            Err(RendezvousError::Malformed(_))
        ));
        // Junk entries are skipped, the rest kept.
        let mut text = String::from_utf8(bytes).unwrap();
        text.insert_str(1, "{\"nonsense\":1},");
        assert_eq!(parse_group_file(text.as_bytes()).unwrap(), ops);
    }

    #[tokio::test]
    async fn group_files_are_read_only_from_members() {
        let rv = MemoryRendezvous::new();
        let (g, a, b, c) = group_with_a_removal();
        let ops: Vec<crate::group::GroupOp> = g.ops().into_iter().cloned().collect();
        publish_group(&rv, g.genesis(), a.id(), &ops).await.unwrap();
        // C (removed) and a stranger publish too; B asks only for its members' files.
        publish_group(&rv, g.genesis(), c.id(), &ops[..1]).await.unwrap();
        rv.put(&group_file_name(&g.genesis(), &key(9).id()), b"[]".to_vec())
            .await
            .unwrap();
        // Another group's file in the same account is not this one's.
        rv.put(
            &group_file_name(&crate::group::OpId([5; 32]), &a.id()),
            b"[]".to_vec(),
        )
        .await
        .unwrap();
        let members = [a.id(), b.id()];
        let got = fetch_group(&rv, g.genesis(), b.id(), &members).await.unwrap();
        assert_eq!(got, vec![(a.id(), ops.clone())]);
        // B's own file is never read back; nothing else is either.
        publish_group(&rv, g.genesis(), b.id(), &ops).await.unwrap();
        let got = fetch_group(&rv, g.genesis(), b.id(), &members).await.unwrap();
        assert_eq!(got.len(), 1);
        assert!(
            fetch_group(&rv, g.genesis(), b.id(), &[b.id()])
                .await
                .unwrap()
                .is_empty()
        );
    }
}
