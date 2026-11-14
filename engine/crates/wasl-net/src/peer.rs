//! What two devices ask each other once connected (docs/DEVICE_SYNC.md §6), independent of how
//! the bytes travel.
//!
//! A device answers through its [`PeerService`] (the engine implements it over its pairs and
//! index) and asks through a [`PeerLink`] (the QUIC transport implements it; [`LocalLink`] calls a
//! service in-process, for tests and for two engines in one process). Every call names the asking
//! device, so a service only ever answers for pairs it shares with that device.

use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use wasl_chunk::{ChunkList, Hash};
use wasl_core::nway::Version;

use crate::group::{GroupOp, OpId};
use crate::identity::DeviceId;
use crate::pairing::Message as PairMessage;

/// Most entries in one [`IndexPage`].
pub const MAX_PAGE_ENTRIES: usize = 1024;
/// Largest chunk a peer sends (the chunker's maximum).
pub const MAX_CHUNK_BYTES: usize = 16 << 20;

/// Names a pair shared between devices: random, made when the pair is first shared, the same on
/// every device that syncs it. Reveals nothing about the folder.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ShareId(pub [u8; 16]);

impl ShareId {
    pub fn random() -> Self {
        let mut b = [0u8; 16];
        getrandom::fill(&mut b).expect("the OS random source is available");
        ShareId(b)
    }

    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn from_hex(s: &str) -> Option<Self> {
        // Hex digits only: `from_str_radix` alone also takes a leading '+', so one code would
        // have several spellings (robustness review 2026-10).
        if s.len() != 32 || !s.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let mut b = [0u8; 16];
        for (i, out) in b.iter_mut().enumerate() {
            *out = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
        }
        Some(ShareId(b))
    }
}

impl fmt::Debug for ShareId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ShareId({})", self.to_hex())
    }
}

/// One page of "what changed since seq N".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexPage {
    /// Keys and their versions, in the answering device's sequence order.
    pub entries: Vec<(String, Version)>,
    /// Ask with this next time: the highest sequence number covered by this page.
    pub next_since: u64,
    /// More entries follow.
    pub more: bool,
}

/// Most group heads one device sends when asking for the operations it lacks (security review
/// 2026-10: the request stays small; a device with more heads sends some of them and is sent a
/// few operations it already has).
pub const MAX_GROUP_HEADS: usize = 64;

/// Most pairing messages in one round.
pub const MAX_PAIR_MESSAGES: usize = 8;
/// Longest device name sent while pairing, in bytes.
pub const MAX_PAIR_NAME_BYTES: usize = 128;

/// One round of pairing from the joiner (docs/DEVICE_SYNC.md §2): its pairing messages, and its
/// name for the inviter's device list. An empty round asks for whatever the inviter has to say.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairRound {
    pub msgs: Vec<PairMessage>,
    pub name: Option<String>,
}

/// The inviter's answer: its pairing messages and, once both people confirmed and the joiner was
/// added, every operation of the group, for the joiner to adopt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairReply {
    pub msgs: Vec<PairMessage>,
    pub ops: Vec<GroupOp>,
}

/// Most offers one device makes another (docs/INTRODUCTIONS.md §3.3).
pub const MAX_OFFERS: usize = 64;
/// Longest folder name in an offer, in bytes.
pub const MAX_OFFER_NAME_BYTES: usize = 128;

/// A pair a device offers another (Introductions, 2026-10): one it would serve to that device,
/// because the device is on the pair's list. Data to show, never a command: accepting it is the
/// person's choice on the receiving device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub share: ShareId,
    /// The pair's name (at most [`MAX_OFFER_NAME_BYTES`]).
    pub name: String,
    /// Files and bytes in the sender's index of the pair (informational).
    pub files: u64,
    pub bytes: u64,
}

/// `name` cut to at most [`MAX_OFFER_NAME_BYTES`], on a character boundary.
pub fn offer_name(name: &str) -> String {
    let mut end = name.len().min(MAX_OFFER_NAME_BYTES);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_owned()
}

/// Why a peer request failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
pub enum PeerError {
    /// The asking device does not share this pair (or is not trusted).
    #[error("this pair is not shared with that device")]
    NotShared,
    /// The file or chunk is no longer there (changed since the index was sent).
    #[error("no longer available")]
    Gone,
    /// The peer could not be reached or the connection broke.
    #[error("connection failed: {0}")]
    Connection(String),
    /// The peer sent something the protocol does not allow.
    #[error("protocol error: {0}")]
    Protocol(String),
    /// The peer failed to answer (its disk, its store).
    #[error("peer error: {0}")]
    Remote(String),
}

pub type PeerResult<T> = Result<T, PeerError>;

/// What a device answers. Implemented by the engine.
#[async_trait]
pub trait PeerService: Send + Sync {
    /// The pairs this device shares with `peer`.
    async fn shared(&self, peer: DeviceId) -> Vec<ShareId>;

    /// Index entries of `pair` that changed after `since`, at most `limit` of them.
    async fn changes_since(
        &self,
        peer: DeviceId,
        pair: ShareId,
        since: u64,
        limit: usize,
    ) -> PeerResult<IndexPage>;

    /// The chunk list of the file at `key` if its content is still `blake3`.
    async fn chunk_list(
        &self,
        peer: DeviceId,
        pair: ShareId,
        key: &str,
        blake3: Hash,
    ) -> PeerResult<ChunkList>;

    /// The bytes of one chunk of `pair`, checked against `hash` by the caller.
    async fn chunk(&self, peer: DeviceId, pair: ShareId, hash: Hash) -> PeerResult<Vec<u8>>;

    /// `peer` says `pair` changed and its sequence number is now `seq`.
    async fn notify(&self, peer: DeviceId, pair: ShareId, seq: u64);

    /// One pairing round from `peer` (only a pairing endpoint answers; others refuse).
    async fn pair(&self, _peer: DeviceId, _round: PairRound) -> PeerResult<PairReply> {
        Err(PeerError::NotShared)
    }

    /// The group operations `peer` lacks, given the heads of its group (docs/DEVICE_SYNC.md §3):
    /// how additions and removals reach every member (security review 2026-10). Only a member
    /// answers; at most [`crate::group::MAX_OPS`], parents first.
    async fn group_ops(&self, _peer: DeviceId, _heads: Vec<OpId>) -> PeerResult<Vec<GroupOp>> {
        Err(PeerError::NotShared)
    }

    /// `peer` says its group changed: ask it for the operations (above).
    async fn group_changed(&self, _peer: DeviceId) {}

    /// The pairs this device offers `peer` (docs/INTRODUCTIONS.md §3.3), at most [`MAX_OFFERS`].
    /// Only a member answers.
    async fn offers(&self, _peer: DeviceId) -> Vec<Offer> {
        Vec::new()
    }

    /// The service to answer one request that came from `remote`, when it depends on where the
    /// request came from (`None`: this one). A pair kept to the LAN is refused to a connection
    /// from beyond it this way (security review 2026-10, FR-N11).
    fn for_remote(&self, _remote: SocketAddr) -> Option<Arc<dyn PeerService>> {
        None
    }
}

/// How a device asks one connected peer. Implemented by the transport.
#[async_trait]
pub trait PeerLink: Send + Sync {
    /// The device at the other end (its key was checked by the transport).
    fn peer(&self) -> DeviceId;

    async fn shared(&self) -> PeerResult<Vec<ShareId>>;
    async fn changes_since(&self, pair: ShareId, since: u64, limit: usize) -> PeerResult<IndexPage>;
    async fn chunk_list(&self, pair: ShareId, key: &str, blake3: Hash) -> PeerResult<ChunkList>;
    async fn chunk(&self, pair: ShareId, hash: Hash) -> PeerResult<Vec<u8>>;
    async fn notify(&self, pair: ShareId, seq: u64) -> PeerResult<()>;
    async fn pair(&self, round: PairRound) -> PeerResult<PairReply>;
    /// The group operations this device lacks; `heads` are its group's heads.
    async fn group_ops(&self, heads: Vec<OpId>) -> PeerResult<Vec<GroupOp>>;
    /// Tells the peer this device's group changed.
    async fn group_changed(&self) -> PeerResult<()>;
    /// The pairs the peer offers this device. A device too old to know the request reads as
    /// offering nothing.
    async fn offers(&self) -> PeerResult<Vec<Offer>>;
}

/// A link that calls the other device's service directly, as `me`.
pub struct LocalLink {
    me: DeviceId,
    peer: DeviceId,
    service: Arc<dyn PeerService>,
}

impl LocalLink {
    pub fn new(me: DeviceId, peer: DeviceId, service: Arc<dyn PeerService>) -> Self {
        Self { me, peer, service }
    }
}

#[async_trait]
impl PeerLink for LocalLink {
    fn peer(&self) -> DeviceId {
        self.peer
    }

    async fn shared(&self) -> PeerResult<Vec<ShareId>> {
        Ok(self.service.shared(self.me).await)
    }

    async fn changes_since(&self, pair: ShareId, since: u64, limit: usize) -> PeerResult<IndexPage> {
        let page = self
            .service
            .changes_since(self.me, pair, since, limit.min(MAX_PAGE_ENTRIES))
            .await?;
        if page.entries.len() > limit.min(MAX_PAGE_ENTRIES) {
            return Err(PeerError::Protocol("page larger than asked".into()));
        }
        Ok(page)
    }

    async fn chunk_list(&self, pair: ShareId, key: &str, blake3: Hash) -> PeerResult<ChunkList> {
        self.service.chunk_list(self.me, pair, key, blake3).await
    }

    async fn chunk(&self, pair: ShareId, hash: Hash) -> PeerResult<Vec<u8>> {
        let bytes = self.service.chunk(self.me, pair, hash).await?;
        if bytes.len() > MAX_CHUNK_BYTES {
            return Err(PeerError::Protocol("chunk larger than the maximum".into()));
        }
        Ok(bytes)
    }

    async fn notify(&self, pair: ShareId, seq: u64) -> PeerResult<()> {
        self.service.notify(self.me, pair, seq).await;
        Ok(())
    }

    async fn pair(&self, round: PairRound) -> PeerResult<PairReply> {
        self.service.pair(self.me, round).await
    }

    async fn group_ops(&self, heads: Vec<OpId>) -> PeerResult<Vec<GroupOp>> {
        let mut heads = heads;
        heads.truncate(MAX_GROUP_HEADS);
        let ops = self.service.group_ops(self.me, heads).await?;
        if ops.len() > crate::group::MAX_OPS {
            return Err(PeerError::Protocol("too many group operations".into()));
        }
        Ok(ops)
    }

    async fn group_changed(&self) -> PeerResult<()> {
        self.service.group_changed(self.me).await;
        Ok(())
    }

    async fn offers(&self) -> PeerResult<Vec<Offer>> {
        let offers = self.service.offers(self.me).await;
        if offers.len() > MAX_OFFERS || offers.iter().any(|o| o.name.len() > MAX_OFFER_NAME_BYTES) {
            return Err(PeerError::Protocol("offers break the limits".into()));
        }
        Ok(offers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_id_hex_round_trips_and_refuses_junk() {
        let id = ShareId::random();
        assert_eq!(Some(id), ShareId::from_hex(&id.to_hex()));
        assert_eq!(None, ShareId::from_hex("zz"));
        assert_eq!(None, ShareId::from_hex(&"g".repeat(32)));
        assert_eq!(None, ShareId::from_hex(&"é".repeat(16)));
    }

    #[test]
    fn offer_names_are_cut_on_a_character_boundary() {
        assert_eq!("Projects", offer_name("Projects"));
        let long = "م".repeat(100); // 200 bytes
        let cut = offer_name(&long);
        assert_eq!(MAX_OFFER_NAME_BYTES, cut.len());
        assert!(long.starts_with(&cut));
        let odd = format!("a{}", "م".repeat(100));
        assert_eq!(MAX_OFFER_NAME_BYTES - 1, offer_name(&odd).len());
    }
}
