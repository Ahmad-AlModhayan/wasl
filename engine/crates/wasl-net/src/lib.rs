//! Wasl device-to-device networking (docs/DEVICE_SYNC.md).
//!
//! Device [`identity`], the [`pairing`] protocol as a pure state machine, the signed [`group`]
//! file, and the [`peer`] requests devices answer for each other: carried by [`quic`] in the
//! [`proto`] wire format, between devices found by [`discover`].
//!
//! Internet reach (docs/DEVICE_SYNC.md §7): [`stun`] learns the public address of the QUIC
//! socket, [`portmap`] asks the gateway for a port (PCP / NAT-PMP), [`rendezvous`] carries signed
//! peer records and offers through shared storage, and [`holepunch`] turns them into ordered
//! candidates and races QUIC connections to them.

pub mod compress;
pub mod discover;
pub mod group;
pub mod holepunch;
pub mod identity;
pub mod pairing;
pub mod peer;
pub mod portmap;
pub mod proto;
pub mod quic;
pub mod rendezvous;
pub mod stun;
pub mod words;

mod base32;

pub use group::{Change, Group, GroupDevice, GroupError, GroupOp, GroupOps, OpId};
pub use identity::{DeviceId, DeviceKey, IdentityError, KeyStore, MemoryKeyStore};
pub use pairing::{
    AbortReason, CodeError, CodeKind, Invitation, JoinCode, Message, PairError, PairedPeer, PairingCode,
    Role, Session, ShortCode, State,
};
pub use peer::{
    IndexPage, LocalLink, Offer, PairReply, PairRound, PeerError, PeerLink, PeerResult, PeerService, ShareId,
};
pub use quic::{Endpoint, QuicError, QuicLink, Trust};
