//! What the screens show about devices and pairing (sent over the daemon's protocol and the
//! phone's bindings).

use serde::{Deserialize, Serialize};

/// Where a pairing stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum PairingStage {
    Idle,
    /// The code is open (inviter) or the other device is being reached (joiner).
    Waiting,
    /// Both devices proved the code: show the words and ask.
    Confirm,
    Paired,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct PairingStatus {
    pub stage: PairingStage,
    /// This computer made the code.
    pub inviting: bool,
    /// The code to show (inviter, while waiting).
    pub code: Option<String>,
    /// Unix seconds.
    pub expires_at: Option<u64>,
    /// The four words both screens must show.
    pub words: Vec<String>,
    pub peer_name: Option<String>,
    /// The other device's ID (`WASL-XXXX-…`), known from its first message: the inviter shows it
    /// beside the words, so the person can tell which device is asking (security review 2026-10).
    #[serde(default)]
    pub peer_id: Option<String>,
    /// The person here already confirmed; waiting for the other device.
    pub confirmed_here: bool,
    pub message: Option<String>,
}

/// One device of the group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct DeviceInfo {
    /// `WASL-XXXX-…`.
    pub id: String,
    pub name: String,
    pub this_device: bool,
    pub connected: bool,
    /// How the connection reached it, while connected (FR-B7).
    pub path: Option<LinkPath>,
    /// Unix seconds.
    pub added_at: u64,
    /// The device that added it (`WASL-…`); `None` for the device that started the group.
    #[serde(default)]
    pub added_by: Option<String>,
    /// Known on this device (docs/INTRODUCTIONS.md §3.1). A member another device added is new
    /// until its person keeps it here; until then it gets no folder from this device. Absent from
    /// an older daemon, which knows every member.
    #[serde(default = "yes")]
    pub known: bool,
}

fn yes() -> bool {
    true
}

/// Where an offer stands on this device (docs/INTRODUCTIONS.md §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum OfferStatus {
    /// Not answered yet: show it.
    New,
    /// Hidden here.
    Declined,
    /// A pair here syncs it.
    Accepted,
}

/// A folder another device offers this one (Introductions, 2026-10). Data to show, never a
/// command: accepting it is the person's choice, with the usual first-sync preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct OfferInfo {
    /// The share ID (hex): what the pair made on accepting it syncs.
    pub share: String,
    /// The device offering it (`WASL-…`), as the connection authenticated it.
    pub from_device: String,
    /// That device's name in the group.
    pub from_name: String,
    /// The folder's name there (cleaned: no control or bidi characters, at most 128 bytes).
    pub name: String,
    /// Files and bytes there (informational).
    pub files: u64,
    pub bytes: u64,
    /// When this device first saw it, Unix ms.
    pub first_seen: i64,
    pub status: OfferStatus,
}

/// Who a shared pair is served to (docs/INTRODUCTIONS.md §3.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct PairSharing {
    /// The share ID (hex), as share codes show it.
    pub share: String,
    /// Every member known on this device ("All my devices").
    pub all_devices: bool,
    /// The devices it is shared with by name (`WASL-…`); each is offered the pair.
    pub devices: Vec<String>,
}

/// How a connection to a device was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum LinkPath {
    /// On the same local network.
    Lan,
    /// Straight to a public, IPv6 or mapped address (or a typed one).
    Direct,
    /// Through a hole punched in both routers.
    Punched,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct DevicesInfo {
    /// Device sync runs in this daemon.
    pub enabled: bool,
    /// Where other devices reach this one.
    pub listen: Option<String>,
    pub devices: Vec<DeviceInfo>,
}
