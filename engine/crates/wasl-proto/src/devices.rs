//! What the screens show about devices and pairing (sent over the daemon's protocol and the
//! phone's bindings).

use crate::words::{self, Lang, Rendered, Values};
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
    /// The code to show as a QR (inviter, while waiting).
    pub code: Option<String>,
    /// The same invitation as the 8 characters a person can type (`XXXX-XXXX`), shown beside the
    /// QR (inviter, while waiting).
    #[serde(default)]
    pub short_code: Option<String>,
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
    /// Why a pairing failed in plain English; for screens that keep no wording of their own it is
    /// worded in the catalogue (`failure`).
    pub message: Option<String>,
    /// The catalogue key of the failure (`pair.wrong_code`, `pair.code_expired`,
    /// `pair.device_removed`, ...), set with `message` when the stage is `Failed`. Scripts and
    /// agents read this, not the sentence.
    #[serde(default)]
    pub failure: Option<String>,
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

/// Which side of a refused device connection has to update Wasl: the two share no protocol
/// version (wasl#34), so one of them is too old for the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum UpdateSide {
    /// This device speaks only versions older than the other device's.
    This,
    /// The other device speaks only versions older than this device's.
    Peer,
}

impl UpdateSide {
    /// The catalogue entry that words it.
    pub fn key(self) -> &'static str {
        match self {
            UpdateSide::This => "device.this_needs_update",
            UpdateSide::Peer => "device.peer_needs_update",
        }
    }
}

/// A device that couldn't be connected because one of the two needs an update: a typed reason
/// the apps and agents read, with the words for a person beside it (docs/STATUS_WORDS.md,
/// `device.this_needs_update` and `device.peer_needs_update`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct DeviceNeedsUpdate {
    /// Who is behind.
    pub behind: UpdateSide,
    /// The other device's ID (`WASL-XXXX-…`).
    pub device: String,
    /// Its name in the group, where this device has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The words, in the language asked for (English until [`DeviceNeedsUpdate::in_lang`]): the
    /// key is stable and the device is in its values, so a script never parses the sentence.
    pub words: Rendered,
}

impl DeviceNeedsUpdate {
    /// The refusal for `device` (named `name` when known), worded in `lang`. The words name the
    /// device by its name, or by its ID when this device has no name for it.
    pub fn new(behind: UpdateSide, device: impl Into<String>, name: Option<String>, lang: Lang) -> Self {
        let device = device.into();
        let shown = name.clone().unwrap_or_else(|| device.clone());
        let words = words::render(
            behind.key(),
            &Values::default().device(shown),
            lang,
            None,
            &[],
            &mut |_| {},
        );
        DeviceNeedsUpdate {
            behind,
            device,
            name,
            words,
        }
    }

    /// The same refusal in another language: its side, device and name stay.
    pub fn in_lang(&self, lang: Lang) -> Self {
        DeviceNeedsUpdate {
            words: self.words.in_lang(lang),
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "WASL-AAAA-BBBB-CCCC-DDDD-EEEE-FFFF-GGGG";

    #[test]
    fn a_device_that_is_behind_is_worded_with_its_name_and_a_fix_for_this_device_only() {
        let this = DeviceNeedsUpdate::new(UpdateSide::This, ID, Some("Pixel".into()), Lang::En);
        assert_eq!("device.this_needs_update", this.words.key);
        assert_eq!(
            "This device needs an update to sync with Pixel. Update Wasl on this device.",
            this.words.text
        );
        assert_eq!(Some(crate::words::Fix::UpdateWasl), this.words.fix);
        assert_eq!(Some("Pixel".into()), this.words.values.device);

        let peer = DeviceNeedsUpdate::new(UpdateSide::Peer, ID, Some("Pixel".into()), Lang::En);
        assert_eq!("device.peer_needs_update", peer.words.key);
        assert_eq!(
            "Pixel needs an update to sync with this device. Update Wasl on Pixel.",
            peer.words.text
        );
        assert_eq!(None, peer.words.fix, "the update happens on the other device");
        assert_eq!((ID, Some("Pixel".to_owned())), (peer.device.as_str(), peer.name));
    }

    #[test]
    fn a_device_with_no_name_here_is_worded_by_its_id() {
        let n = DeviceNeedsUpdate::new(UpdateSide::Peer, ID, None, Lang::En);
        assert!(n.words.text.starts_with(ID), "{}", n.words.text);
        assert_eq!(Some(ID.to_owned()), n.words.values.device);
    }

    #[test]
    fn the_same_refusal_is_worded_in_arabic_with_the_same_key_and_values() {
        let en = DeviceNeedsUpdate::new(UpdateSide::This, ID, Some("Pixel".into()), Lang::En);
        let ar = en.in_lang(Lang::Ar);
        assert_eq!(
            "يحتاج هذا الجهاز إلى تحديث للمزامنة مع Pixel. حدّث وصل على هذا الجهاز.",
            ar.words.text
        );
        assert_eq!(
            (en.behind, &en.device, &en.words.key, &en.words.values),
            (ar.behind, &ar.device, &ar.words.key, &ar.words.values)
        );
    }
}
