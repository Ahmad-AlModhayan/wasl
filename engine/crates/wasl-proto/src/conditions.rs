//! When a pair may run (SRS FR-P4). The platform reports the device state; the decision is here, so
//! every platform applies the same rules (FR-E3). Ported from 1.1.4 `Conditions.blockReason`.

use serde::{Deserialize, Serialize};

/// A pair's run conditions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct Conditions {
    pub wifi_only: bool,
    pub allow_metered: bool,
    pub allow_roaming: bool,
    pub require_charging: bool,
    /// Below this battery level (and not charging) the pair waits. 0 disables the rule.
    pub min_battery_percent: u8,
    /// Wi-Fi names the pair may sync on; empty means any.
    pub ssid_allow: Vec<String>,
    /// Wi-Fi names the pair never syncs on.
    pub ssid_deny: Vec<String>,
    pub require_vpn: bool,
}

impl Default for Conditions {
    /// The 1.1.4 defaults.
    fn default() -> Self {
        Conditions {
            wifi_only: true,
            allow_metered: false,
            allow_roaming: false,
            require_charging: false,
            min_battery_percent: 15,
            ssid_allow: Vec::new(),
            ssid_deny: Vec::new(),
            require_vpn: false,
        }
    }
}

impl Conditions {
    /// No restrictions (desktop default).
    pub fn none() -> Conditions {
        Conditions {
            wifi_only: false,
            allow_metered: true,
            allow_roaming: true,
            require_charging: false,
            min_battery_percent: 0,
            ssid_allow: Vec::new(),
            ssid_deny: Vec::new(),
            require_vpn: false,
        }
    }
}

/// What the platform knows about the device right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct DeviceState {
    pub online: bool,
    /// Wi-Fi or Ethernet.
    pub wifi: bool,
    pub metered: bool,
    pub vpn: bool,
    pub roaming: bool,
    pub ssid: Option<String>,
    pub charging: bool,
    /// 0..=100.
    pub battery_percent: u8,
}

impl DeviceState {
    /// A desktop on mains power and an unmetered network.
    pub fn unrestricted() -> DeviceState {
        DeviceState {
            online: true,
            wifi: true,
            metered: false,
            vpn: false,
            roaming: false,
            ssid: None,
            charging: true,
            battery_percent: 100,
        }
    }
}

/// Why a pair is waiting. The apps turn this into words in the user's language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum Blocker {
    Offline,
    WaitingForWifi,
    MeteredConnection,
    Roaming,
    WaitingForVpn,
    WaitingForCharger,
    BatteryBelow { percent: u8 },
    WifiNotAllowed,
    WifiDenied,
}

/// The first condition that stops the pair, or `None` when it may run. A manual "Sync now" only
/// needs a connection (FR-P4); automation does not get that exemption.
pub fn blocker(c: &Conditions, d: &DeviceState, manual: bool) -> Option<Blocker> {
    if !d.online {
        return Some(Blocker::Offline);
    }
    if manual {
        return None;
    }
    if c.wifi_only && !d.wifi {
        return Some(Blocker::WaitingForWifi);
    }
    if !c.allow_metered && d.metered && !d.wifi {
        return Some(Blocker::MeteredConnection);
    }
    if !c.allow_roaming && d.roaming {
        return Some(Blocker::Roaming);
    }
    if c.require_vpn && !d.vpn {
        return Some(Blocker::WaitingForVpn);
    }
    if c.require_charging && !d.charging {
        return Some(Blocker::WaitingForCharger);
    }
    if !d.charging && d.battery_percent < c.min_battery_percent {
        return Some(Blocker::BatteryBelow {
            percent: c.min_battery_percent,
        });
    }
    let allow: Vec<&str> = c
        .ssid_allow
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    let deny: Vec<&str> = c
        .ssid_deny
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if d.wifi && (!allow.is_empty() || !deny.is_empty()) {
        let ssid = d.ssid.as_deref();
        if !allow.is_empty()
            && !ssid.is_some_and(|s| allow.iter().any(|a| a.to_lowercase() == s.to_lowercase()))
        {
            return Some(Blocker::WifiNotAllowed);
        }
        if ssid.is_some_and(|s| deny.iter().any(|a| a.to_lowercase() == s.to_lowercase())) {
            return Some(Blocker::WifiDenied);
        }
    }
    None
}
