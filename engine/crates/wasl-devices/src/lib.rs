//! This device in a Wasl group (docs/DEVICE_SYNC.md): its key and group, the endpoint other
//! devices reach it on, LAN discovery, pairing a new device, internet reach and the shared group
//! files through Drive's app-data folder, and running pairs with the connected devices. Shared by the desktop daemon
//! and the phone; each supplies where the key is kept and how a pair's folder is opened.

#[cfg(feature = "ffi")]
uniffi::setup_scaffolding!();

pub mod board;
pub mod devices;
pub mod pairing;
pub mod reach;
pub mod status;

pub use board::BoardTiming;
pub use devices::{
    DeviceConfig, Devices, MAX_OFFERS_KEPT, MAX_OFFERS_PER_DEVICE, NO_LONGER_OFFERED, NO_PEER, NOT_IN_GROUP,
    NOT_SHARED, OpenFolder, clean_name,
};
pub use pairing::Pairing;
pub use reach::{DriveRendezvous, ReachPolicy, ReachTiming};
pub use status::{
    DeviceInfo, DevicesInfo, LinkPath, OfferInfo, OfferStatus, PairSharing, PairingStage, PairingStatus,
};
