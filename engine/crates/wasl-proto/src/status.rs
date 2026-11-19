//! One status per folder, decided once (issue #29).
//!
//! The engine works out each synced folder's state here, with one rule set, and the daemon, the
//! command line (`wasl status --json`) and later the phone binding hand the result out. Every
//! sentence in it is a catalogue entry (`words`): a key, its values, the fix and the words in the
//! caller's language, so an agent reads exactly what an app draws and no app words a status itself.
//!
//! What a person sees, in the order the rules look:
//!
//! 1. A folder turned off is **off**.
//! 2. Something that waits for the person is **needs you**: a sign-in that ended, the first preview
//!    a new folder waits for, a stopped plan that waits for a yes, files changed in two places, a
//!    full local disk. A folder held for its first preview is "needs you", not "waiting".
//! 3. A folder syncing now is **syncing**.
//! 4. A paused folder is **waiting** because it is paused, whatever its last run said.
//! 5. The last run says the rest. A run that failed for a lasting reason (signed out, the Drive
//!    folder gone, a full disk) is **needs you** at once. One that couldn't reach Google (or Google
//!    was busy) is **waiting** while the engine retries it (`retry`), and **needs you** once the
//!    retry policy gives up; any other failure is **failed**. A run that moved some files and not
//!    others is **partly synced**, never "synced" and never "failed".
//! 6. An offline device, or conditions that hold the next run, are **waiting**.
//! 7. Otherwise the folder is **synced**.
//!
//! The whole-app state ([`overall_status`]) is worked out from the folder statuses by the same rules.

use crate::conditions::Blocker;
use crate::retry::{self, RetryInfo};
use crate::run::{ErrorKind, RunStatus};
use crate::store::{Pair, RunRecord, RunStats};
use crate::words::{self, Lang, Platform, Rendered, SignInCause, Values};
use serde::{Deserialize, Serialize};

/// How many of a folder's latest runs a status carries.
pub const LAST_RUNS: usize = 3;

/// A folder's state: one closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum FolderState {
    Synced,
    Syncing,
    /// Nothing is wrong and nothing is asked of the person; [`FolderStatus::wait`] says why.
    Waiting,
    /// The person has something to do; [`FolderStatus::needs_you`] holds each item.
    NeedsYou,
    /// The last run moved some files and not others.
    PartlySynced,
    /// The last run failed and Wasl can't say what the person should do about it.
    Failed,
    Off,
}

/// Why a [`FolderState::Waiting`] folder waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum WaitReason {
    /// The device is offline, or the last run couldn't reach Google Drive.
    NoNetwork,
    /// The folder's own conditions hold the next run (Wi-Fi, charging, battery, VPN), or its first
    /// sync hasn't come round yet.
    Schedule,
    /// The person paused it.
    Paused,
    /// Google was busy or had a problem; the run tries again on its own.
    Retrying,
    /// A folder that syncs only between devices waits for a device that is offline
    /// ([`FolderStatus::peers`] says which).
    Device,
}

/// Numbers a screen draws beside the words.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct FolderCounts {
    /// Files changed in two places and waiting for the person.
    pub conflicts: u32,
    /// Items the last run did not move (a partial run); 0 otherwise.
    pub not_moved: u32,
    /// Files the last run left alone on purpose; None before the first finished run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<u32>,
}

/// One of the folder's latest runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct LastRun {
    pub run_id: String,
    pub status: RunStatus,
    pub trigger: String,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    pub stats: RunStats,
    /// What a partial or failed run says, from the catalogue; absent for a run that went well.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<Rendered>,
}

/// A folder's status: the state, why, what needs the person, and the numbers and runs behind it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct FolderStatus {
    pub pair_id: String,
    /// The folder's name, as the words use it.
    pub name: String,
    pub state: FolderState,
    /// Why, for a waiting folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<WaitReason>,
    /// The condition holding the next run, for a folder waiting on one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocker: Option<Blocker>,
    /// For a folder whose last run failed for a passing reason: which try failed, how many the engine
    /// makes and when the next one is (`retry`). Absent otherwise; the apps keep no count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryInfo>,
    /// The headline: one catalogue entry for the state.
    pub words: Rendered,
    /// Each thing that needs the person, most urgent first. A failed folder holds its one failure
    /// here too, so the count of items and the overall state agree.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs_you: Vec<Rendered>,
    pub counts: FolderCounts,
    /// When the last run that moved everything finished, milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_synced_at: Option<i64>,
    /// The latest runs, newest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub last_runs: Vec<LastRun>,
    /// The other devices the folder is shared with, and where each stands now. Empty for a folder
    /// that isn't shared. Added by [`with_peers`], never by [`folder_status`] (which knows no devices).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peers: Vec<FolderPeer>,
}

impl FolderStatus {
    /// The same status in another language: every sentence is worded again from its key and values.
    /// The keys, values, fixes and Details are the ones it already has.
    pub fn in_lang(&self, lang: Lang) -> FolderStatus {
        let mut s = self.clone();
        s.words = self.words.in_lang(lang);
        s.needs_you = self.needs_you.iter().map(|r| r.in_lang(lang)).collect();
        for run in &mut s.last_runs {
            run.words = run.words.as_ref().map(|r| r.in_lang(lang));
        }
        s
    }
}

/// What the engine knows about one folder when it decides.
pub struct Facts<'a> {
    pub pair: &'a Pair,
    /// A run is in flight.
    pub running: bool,
    pub open_conflicts: u32,
    /// The pair's Google account must sign in again.
    pub signed_out: bool,
    /// Why, when the engine knows; none reads as Google no longer accepting the sign-in.
    pub sign_in_cause: Option<SignInCause>,
    /// What holds the next run on this device now (offline, Wi-Fi only, …).
    pub blocker: Option<Blocker>,
    /// Free bytes on the drive holding the folder; None when it can't be told.
    pub free_bytes: Option<u64>,
    /// Files the last finished run left alone on purpose.
    pub skipped: Option<u32>,
    /// The folder's latest runs, newest first.
    pub runs: &'a [RunRecord],
}

/// Decides one folder's status in `lang`. A cause the catalogue can't name goes to `log` once.
pub fn folder_status(facts: &Facts, lang: Lang, log: &mut dyn FnMut(String)) -> FolderStatus {
    folder_status_for(Platform::current(), facts, lang, log)
}

/// [`folder_status`] for a run whose I/O errors came from `platform`.
pub fn folder_status_for(
    platform: Platform,
    facts: &Facts,
    lang: Lang,
    log: &mut dyn FnMut(String),
) -> FolderStatus {
    let pair = facts.pair;
    let st = &pair.status;
    let values = Values::default().folder(pair.name.clone());
    let roots = [pair.local_root.as_str()];
    let secrets: Vec<&str> = pair.account_id.as_deref().into_iter().collect();
    let mut say = |key: &str, values: &Values, raw: Option<&str>| {
        words::render_with(key, values, lang, raw, &roots, &secrets, &mut *log)
    };

    let mut status = FolderStatus {
        pair_id: pair.id.clone(),
        name: pair.name.clone(),
        state: FolderState::Synced,
        wait: None,
        blocker: None,
        retry: None,
        words: say("folder.synced", &values, None),
        needs_you: Vec::new(),
        counts: FolderCounts {
            conflicts: facts.open_conflicts,
            not_moved: 0,
            skipped: facts.skipped,
        },
        last_synced_at: st.last_success_at,
        last_runs: Vec::new(),
        peers: Vec::new(),
    };
    let raw = st.last_error.as_deref();
    let ran = st.last_status;
    let kind = st.last_error_kind.unwrap_or(ErrorKind::Other);
    // The last run's cause as the catalogue names it (a number the system itself printed can make an
    // I/O error a full disk).
    let failed_key =
        (ran == Some(RunStatus::Failed)).then(|| words::classify_for(platform, kind, raw.unwrap_or("")));
    let partial_count = if ran == Some(RunStatus::Partial) {
        raw.and_then(words::partial_count)
    } else {
        None
    };
    if ran == Some(RunStatus::Partial) {
        status.counts.not_moved = partial_count
            .map(|n| n.min(u64::from(u32::MAX)) as u32)
            .or_else(|| facts.runs.first().map(|r| r.stats.errors).filter(|n| *n > 0))
            .unwrap_or(0);
    }
    status.last_runs = facts
        .runs
        .iter()
        .take(LAST_RUNS)
        .enumerate()
        .map(|(i, r)| {
            let newest = i == 0;
            let words = match r.status {
                RunStatus::Partial => {
                    let n = (r.stats.errors > 0).then_some(u64::from(r.stats.errors));
                    Some(partial_words(
                        &mut say,
                        &values,
                        n,
                        if newest { raw } else { None },
                    ))
                }
                // The newest run's cause is kept with the folder. An older failure is worded from
                // the kind its own run kept; a run stored before runs kept it (issue #57) reads as
                // the fallback, quietly.
                RunStatus::Failed => {
                    let (key, cause) = if newest {
                        (failed_key.unwrap_or(words::FALLBACK), raw)
                    } else {
                        let cause = r.message.as_deref();
                        let key = r.error_kind.map_or(words::FALLBACK, |k| {
                            words::classify_for(platform, k, cause.unwrap_or(""))
                        });
                        (key, cause)
                    };
                    Some(words::render_with(
                        key,
                        &values,
                        lang,
                        cause,
                        &roots,
                        &secrets,
                        &mut |_| {},
                    ))
                }
                _ => None,
            };
            LastRun {
                run_id: r.id.clone(),
                status: r.status,
                trigger: r.trigger.clone(),
                started_at: r.started_at,
                ended_at: r.ended_at,
                stats: r.stats.clone(),
                words,
            }
        })
        .collect();

    if !pair.enabled {
        status.state = FolderState::Off;
        status.words = say("folder.off", &values, None);
        return status;
    }

    // What waits for the person whatever the last run said.
    let mut needs: Vec<Rendered> = Vec::new();
    if facts.signed_out {
        needs.push(say(
            facts.sign_in_cause.unwrap_or(SignInCause::Ended).folder_key(),
            &values,
            raw.filter(|_| kind == ErrorKind::Auth),
        ));
    }
    if st.hold_for_preview && !st.approve_once {
        needs.push(say("folder.first_preview", &values, None));
    }
    if st.pending_approval {
        needs.push(say("folder.needs_approval", &values, None));
    }
    if facts.open_conflicts > 0 {
        let v = values.clone().count(u64::from(facts.open_conflicts));
        needs.push(say("folder.conflicts", &v, None));
    }
    if facts.free_bytes == Some(0) {
        needs.push(say("error.disk_full", &values, None));
    }
    if !needs.is_empty() {
        status.state = FolderState::NeedsYou;
        status.words = say("folder.needs_you", &values, None);
        status.needs_you = needs;
        return status;
    }
    if facts.running {
        status.state = FolderState::Syncing;
        status.words = say("folder.syncing", &values, None);
        return status;
    }

    // A paused folder reads paused, whatever its last run said: the person stopped it.
    if pair.paused {
        status.state = FolderState::Waiting;
        status.wait = Some(WaitReason::Paused);
        status.words = say("folder.paused", &values, None);
        return status;
    }

    // The last run says the rest.
    match ran {
        Some(RunStatus::Failed) => {
            let key = failed_key.unwrap_or(words::FALLBACK);
            let transient = kind.transient();
            // A network failure while the device is offline is the offline wait, however many runs
            // it has missed: nothing here is the person's to fix.
            let offline_now = kind == ErrorKind::Network && facts.blocker == Some(Blocker::Offline);
            let retrying = retry::plan(st);
            if transient && (retrying.is_some() || offline_now) {
                status.state = FolderState::Waiting;
                status.retry = retrying;
                if kind == ErrorKind::Network {
                    status.wait = Some(WaitReason::NoNetwork);
                    status.words = say("folder.waiting_no_network", &values, raw);
                } else {
                    status.wait = Some(WaitReason::Retrying);
                    status.words = say(key, &values, raw);
                }
                return status;
            }
            // Wasl has stopped trying (a passing failure that kept failing) or never will (a lasting
            // one): the person has something to do, and the fix is in the entry.
            let item = say(key, &values, raw);
            status.state = if transient || FIXABLE.contains(&key) {
                FolderState::NeedsYou
            } else {
                FolderState::Failed
            };
            status.words = item.clone();
            status.needs_you = vec![item];
            return status;
        }
        Some(RunStatus::Partial) => {
            status.state = FolderState::PartlySynced;
            status.words = partial_words(&mut say, &values, partial_count, raw);
            return status;
        }
        _ => {}
    }

    if let Some(b) = facts.blocker.clone() {
        let offline = b == Blocker::Offline;
        status.state = FolderState::Waiting;
        status.wait = Some(if offline {
            WaitReason::NoNetwork
        } else {
            WaitReason::Schedule
        });
        status.words = say(
            if offline {
                "folder.waiting_no_network"
            } else {
                "folder.waiting_schedule"
            },
            &values,
            None,
        );
        status.blocker = Some(b);
    } else if ran != Some(RunStatus::Success) && st.last_success_at.is_none() {
        // Never synced yet, and nothing holds it: its first sync hasn't come round.
        status.state = FolderState::Waiting;
        status.wait = Some(WaitReason::Schedule);
        status.words = say("folder.waiting_schedule", &values, None);
    }
    status
}

/// The lasting failures a person can do something about; the rest read "failed". A passing failure
/// that the retry policy gave up on needs the person too.
const FIXABLE: &[&str] = &[
    "error.auth",
    "error.permission",
    "error.quota",
    "error.not_found",
    "error.folder_missing",
    "error.locked",
    "error.disk_full",
    "error.disk_quota",
    "error.name_refused",
];

/// A partial run's words: the count when the message holds one, the first cause as Details.
fn partial_words(
    say: &mut impl FnMut(&str, &Values, Option<&str>) -> Rendered,
    values: &Values,
    count: Option<u64>,
    raw: Option<&str>,
) -> Rendered {
    let cause = raw.map(|r| r.split_once(" First: ").map_or(r, |(_, c)| c));
    match count {
        Some(n) => say("run.partial", &values.clone().count(n), cause),
        None => say("run.partial_some", values, cause),
    }
}

/// The whole app's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum OverallState {
    /// No folders yet.
    Idle,
    Synced,
    Syncing,
    Paused,
    Waiting,
    PartlySynced,
    NeedsYou,
}

/// The whole app's status, worked out from the folder statuses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct OverallStatus {
    pub state: OverallState,
    /// The short status words (`overall.*`).
    pub words: Rendered,
    /// The needs-you items across all folders, and what else waits for the person (agent changes
    /// held for a decision, an account that must sign in with no folder on it).
    pub needs_you: u32,
    /// "N things need you", when there are any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_you_words: Option<Rendered>,
}

impl OverallStatus {
    /// The same status in another language.
    pub fn in_lang(&self, lang: Lang) -> OverallStatus {
        let mut s = self.clone();
        s.words = self.words.in_lang(lang);
        s.needs_you_words = self.needs_you_words.as_ref().map(|r| r.in_lang(lang));
        s
    }
}

/// Works the whole app's state out of `folders`. `other_items` counts what waits for the person
/// outside any folder (agent changes held, an account to sign in again with no folder showing it).
pub fn overall_status(folders: &[FolderStatus], other_items: u32, lang: Lang) -> OverallStatus {
    use FolderState as F;
    let needs_you = folders.iter().map(|f| f.needs_you.len() as u32).sum::<u32>() + other_items;
    let any = |pred: &dyn Fn(&FolderStatus) -> bool| folders.iter().any(pred);
    let active: Vec<&FolderStatus> = folders.iter().filter(|f| f.state != F::Off).collect();
    let paused = |f: &FolderStatus| f.state == F::Waiting && f.wait == Some(WaitReason::Paused);
    let state = if folders.is_empty() && needs_you == 0 {
        OverallState::Idle
    } else if needs_you > 0 || any(&|f| matches!(f.state, F::NeedsYou | F::Failed)) {
        OverallState::NeedsYou
    } else if any(&|f| f.state == F::Syncing) {
        OverallState::Syncing
    } else if active.iter().all(|f| paused(f)) {
        OverallState::Paused
    } else if any(&|f| f.state == F::PartlySynced) {
        OverallState::PartlySynced
    } else if active.iter().any(|f| f.state == F::Waiting && !paused(f)) {
        OverallState::Waiting
    } else {
        OverallState::Synced
    };
    let key = match state {
        OverallState::Idle => "overall.idle",
        OverallState::Synced => "overall.synced",
        OverallState::Syncing => "overall.syncing",
        OverallState::Paused => "overall.paused",
        OverallState::Waiting => "overall.waiting",
        OverallState::PartlySynced => "overall.partly_synced",
        OverallState::NeedsYou => "overall.needs_you",
    };
    let words = words::render(key, &Values::default(), lang, None, &[], &mut |_| {});
    let needs_you_words = (needs_you > 0).then(|| {
        words::render(
            "status.items_need_you",
            &Values::default().count(u64::from(needs_you)),
            lang,
            None,
            &[],
            &mut |_| {},
        )
    });
    OverallStatus {
        state,
        words,
        needs_you,
        needs_you_words,
    }
}

// ---------------------------------------------------------------- devices (issue #32)

/// An offline device reads "last seen" until it has been out of reach this long (30 days), and
/// "can't reach" after; a device never reached from here reads "can't reach" at once. Milliseconds.
pub const CANT_REACH_AFTER_MS: i64 = 30 * 24 * 3600 * 1000;

/// A device's state: one closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum DevicePresence {
    /// Connected now (this device is always online to itself).
    Online,
    /// Connected now, and a folder is syncing with it.
    Syncing,
    /// Offline, but seen recently: [`DeviceStatus::last_seen_at`] says when.
    LastSeen,
    /// Never reached from here, or not for more than [`CANT_REACH_AFTER_MS`].
    CantReach,
}

/// One device in "my devices", its state decided once (docs/STATUS_WORDS.md, `device.*`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct DeviceStatus {
    /// `WASL-XXXX-…`.
    pub id: String,
    pub name: String,
    pub this_device: bool,
    pub state: DevicePresence,
    /// One catalogue entry for the state: the key is stable, the time is in its values.
    pub words: Rendered,
    /// When this device last saw it, Unix milliseconds. For a device online now, when it was
    /// decided. None for a device never reached from here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<i64>,
}

impl DeviceStatus {
    /// The same status in another language: the key, values and state stay.
    pub fn in_lang(&self, lang: Lang) -> DeviceStatus {
        DeviceStatus {
            words: self.words.in_lang(lang),
            ..self.clone()
        }
    }
}

/// What the engine knows about one device when it decides.
pub struct DeviceFacts<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub this_device: bool,
    /// A connection to it is open now.
    pub connected: bool,
    /// A folder is syncing with it now.
    pub syncing: bool,
    /// When it was last connected, Unix milliseconds; None when never.
    pub last_seen_at: Option<i64>,
    /// Now, Unix milliseconds.
    pub now: i64,
}

/// Decides one device's status in `lang`.
pub fn device_status(facts: &DeviceFacts, lang: Lang) -> DeviceStatus {
    let (state, key, count, last_seen_at) = if facts.this_device || facts.connected {
        let state = if facts.syncing {
            DevicePresence::Syncing
        } else {
            DevicePresence::Online
        };
        let key = if facts.syncing {
            "device.syncing"
        } else {
            "device.online"
        };
        (state, key, None, Some(facts.now))
    } else {
        match facts.last_seen_at {
            None => (DevicePresence::CantReach, "device.cant_reach", None, None),
            Some(seen) => {
                let age = (facts.now - seen).max(0);
                let (min, hour, day) = (60_000, 3_600_000, 86_400_000);
                let (key, count) = if age > CANT_REACH_AFTER_MS {
                    ("device.cant_reach", None)
                } else if age < min {
                    ("device.last_seen_now", None)
                } else if age < hour {
                    ("device.last_seen_minutes", Some(age / min))
                } else if age < day {
                    ("device.last_seen_hours", Some(age / hour))
                } else {
                    ("device.last_seen_days", Some(age / day))
                };
                let state = if key == "device.cant_reach" {
                    DevicePresence::CantReach
                } else {
                    DevicePresence::LastSeen
                };
                (state, key, count, Some(seen))
            }
        }
    };
    let mut values = Values::default().device(facts.name);
    if let Some(n) = count {
        values = values.count(n as u64);
    }
    DeviceStatus {
        id: facts.id.to_owned(),
        name: facts.name.to_owned(),
        this_device: facts.this_device,
        state,
        words: words::render(key, &values, lang, None, &[], &mut |_| {}),
        last_seen_at,
    }
}

/// A device a folder is shared with, and where it stands. Only the state: the words with the time
/// are the device's own ([`DeviceStatus`]), so a folder's status doesn't change every minute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct FolderPeer {
    pub device_id: String,
    pub name: String,
    pub state: DevicePresence,
}

/// Adds the devices a folder is shared with to its status. A folder that syncs only between
/// devices (`only_devices`: no Drive copy keeps it current) and would read synced waits instead
/// for a device that is offline, and says which one: the one last seen recently first, then one
/// that can't be reached. A folder that also syncs with Drive stays synced: its Drive copy is
/// current, and the device catches up when it is back. Either way [`FolderStatus::peers`] lists
/// every device, so a screen can show each one's state.
pub fn with_peers(
    mut status: FolderStatus,
    peers: Vec<FolderPeer>,
    only_devices: bool,
    lang: Lang,
) -> FolderStatus {
    status.peers = peers;
    if status.state == FolderState::Synced && only_devices {
        let named = [DevicePresence::LastSeen, DevicePresence::CantReach]
            .iter()
            .find_map(|s| status.peers.iter().find(|p| p.state == *s));
        if let Some(p) = named {
            let key = if p.state == DevicePresence::CantReach {
                "folder.cant_reach_device"
            } else {
                "folder.waiting_device"
            };
            let values = Values::default()
                .folder(status.name.clone())
                .device(p.name.clone());
            status.words = words::render(key, &values, lang, None, &[], &mut |_| {});
            status.state = FolderState::Waiting;
            status.wait = Some(WaitReason::Device);
        }
    }
    status
}

impl Rendered {
    /// The same entry, values and Details in `lang`.
    pub fn in_lang(&self, lang: Lang) -> Rendered {
        let mut r = words::render(&self.key, &self.values, lang, None, &[], &mut |_| {});
        r.details = self.details.clone();
        // A key this build doesn't know keeps its own key and text rather than the fallback's.
        if words::entry(&self.key).is_none() {
            return self.clone();
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::{ErrorKind, RunStatus};
    use crate::words::Fix;

    fn pair() -> Pair {
        let mut p = Pair::new("p1", "Photos", "/home/me/Photos");
        p.account_id = Some("me@example.com".into());
        p.status.last_status = Some(RunStatus::Success);
        p.status.last_success_at = Some(1_000);
        p
    }

    fn facts(pair: &Pair) -> Facts<'_> {
        Facts {
            pair,
            running: false,
            open_conflicts: 0,
            signed_out: false,
            sign_in_cause: None,
            blocker: None,
            free_bytes: Some(10_000_000),
            skipped: None,
            runs: &[],
        }
    }

    fn decide(f: &Facts) -> FolderStatus {
        folder_status_for(Platform::Linux, f, Lang::En, &mut |_| {})
    }

    fn keys(s: &FolderStatus) -> Vec<&str> {
        s.needs_you.iter().map(|r| r.key.as_str()).collect()
    }

    #[test]
    fn a_folder_that_synced_cleanly_is_synced() {
        let p = pair();
        let s = decide(&facts(&p));
        assert_eq!(FolderState::Synced, s.state);
        assert_eq!("folder.synced", s.words.key);
        assert_eq!("Photos is up to date.", s.words.text);
        assert!(s.needs_you.is_empty());
        assert_eq!(Some(1_000), s.last_synced_at);
    }

    #[test]
    fn a_folder_turned_off_is_off() {
        let mut p = pair();
        p.enabled = false;
        p.status.hold_for_preview = true;
        let s = decide(&facts(&p));
        assert_eq!(FolderState::Off, s.state);
        assert_eq!("folder.off", s.words.key);
    }

    #[test]
    fn a_folder_waiting_for_its_first_preview_needs_you() {
        let mut p = pair();
        p.status = Default::default();
        p.status.hold_for_preview = true;
        let s = decide(&facts(&p));
        assert_eq!(FolderState::NeedsYou, s.state);
        assert_eq!(vec!["folder.first_preview"], keys(&s));
        assert_eq!(Some(Fix::Review), s.needs_you[0].fix);
        assert_eq!(Some("Photos"), s.needs_you[0].values.folder.as_deref());
    }

    #[test]
    fn a_partial_run_is_partly_synced_never_synced_or_failed() {
        let mut p = pair();
        p.status.last_status = Some(RunStatus::Partial);
        p.status.last_error = Some("3 items couldn't sync. First: Drive refused a name".into());
        let s = decide(&facts(&p));
        assert_eq!(FolderState::PartlySynced, s.state);
        assert_eq!("run.partial", s.words.key);
        assert_eq!(3, s.counts.not_moved);
        assert_eq!(Some(3), s.words.values.count);
        assert!(s.needs_you.is_empty(), "nothing is asked of the person");
    }

    #[test]
    fn a_partial_run_that_names_no_count_still_reads_partly_synced() {
        let mut p = pair();
        p.status.last_status = Some(RunStatus::Partial);
        p.status.last_error = Some("one file was refused".into());
        let s = decide(&facts(&p));
        assert_eq!(FolderState::PartlySynced, s.state);
        assert_eq!("run.partial_some", s.words.key);
    }

    #[test]
    fn a_full_local_disk_needs_you_with_the_storage_fix() {
        let mut p = pair();
        p.status.last_status = Some(RunStatus::Failed);
        p.status.last_error_kind = Some(ErrorKind::Io);
        p.status.last_error = Some("No space left on device (os error 28)".into());
        let s = decide(&facts(&p));
        assert_eq!(FolderState::NeedsYou, s.state);
        assert_eq!(vec!["error.disk_full"], keys(&s));
        assert_eq!(Some(Fix::OpenStorageSettings), s.needs_you[0].fix);
    }

    #[test]
    fn no_free_bytes_on_the_drive_is_a_full_disk_too() {
        let p = pair();
        let mut f = facts(&p);
        f.free_bytes = Some(0);
        let s = decide(&f);
        assert_eq!(FolderState::NeedsYou, s.state);
        assert_eq!(vec!["error.disk_full"], keys(&s));
    }

    #[test]
    fn a_signed_out_account_needs_you_with_sign_in_again() {
        let p = pair();
        let mut f = facts(&p);
        f.signed_out = true;
        let s = decide(&f);
        assert_eq!(FolderState::NeedsYou, s.state);
        assert_eq!(vec!["error.auth"], keys(&s));
        assert_eq!(Some(Fix::SignInAgain), s.needs_you[0].fix);
    }

    #[test]
    fn a_sign_in_saved_on_this_device_that_cannot_be_read_says_so() {
        let p = pair();
        let mut f = facts(&p);
        f.signed_out = true;
        f.sign_in_cause = Some(SignInCause::Unreadable);
        let s = decide(&f);
        assert_eq!(FolderState::NeedsYou, s.state);
        assert_eq!(vec!["error.sign_in_unreadable"], keys(&s));
        assert_eq!(Some(Fix::SignInAgain), s.needs_you[0].fix);
        assert!(s.needs_you[0].text.contains("Photos"), "{}", s.needs_you[0].text);
        // Google ending the session is the other cause; no cause given reads as that.
        f.sign_in_cause = Some(SignInCause::Ended);
        assert_eq!(vec!["error.auth"], keys(&decide(&f)));
        f.sign_in_cause = None;
        assert_eq!(vec!["error.auth"], keys(&decide(&f)));
    }

    #[test]
    fn a_missing_folder_needs_you_at_once_with_its_own_words() {
        let mut p = pair();
        p.status.last_status = Some(RunStatus::Failed);
        p.status.last_error_kind = Some(ErrorKind::FolderMissing);
        p.status.last_error = Some("gone (os error 3)".into());
        p.status.consecutive_failures = 1;
        let s = decide(&facts(&p));
        assert_eq!(FolderState::NeedsYou, s.state);
        assert_eq!(vec!["error.folder_missing"], keys(&s));
        assert_eq!(Some(Fix::OpenFolder), s.needs_you[0].fix);
        assert_eq!(None, s.retry);
        assert_eq!(Some("Photos"), s.needs_you[0].values.folder.as_deref());
    }

    #[test]
    fn a_signed_out_account_and_a_failed_auth_run_are_one_item() {
        let mut p = pair();
        p.status.last_status = Some(RunStatus::Failed);
        p.status.last_error_kind = Some(ErrorKind::Auth);
        p.status.last_error = Some("invalid_grant".into());
        let mut f = facts(&p);
        f.signed_out = true;
        assert_eq!(vec!["error.auth"], keys(&decide(&f)));
    }

    #[test]
    fn a_paused_folder_is_waiting_because_paused() {
        let mut p = pair();
        p.paused = true;
        let s = decide(&facts(&p));
        assert_eq!(FolderState::Waiting, s.state);
        assert_eq!(Some(WaitReason::Paused), s.wait);
        assert_eq!("folder.paused", s.words.key);
    }

    #[test]
    fn a_paused_folder_reads_paused_even_after_a_failed_run() {
        let mut p = pair();
        p.paused = true;
        p.status.last_status = Some(RunStatus::Failed);
        p.status.last_error_kind = Some(ErrorKind::Other);
        let s = decide(&facts(&p));
        assert_eq!(
            (FolderState::Waiting, Some(WaitReason::Paused)),
            (s.state, s.wait)
        );
    }

    #[test]
    fn many_missed_runs_while_offline_still_wait_for_the_network() {
        let mut p = pair();
        p.status.last_status = Some(RunStatus::Failed);
        p.status.last_error_kind = Some(ErrorKind::Network);
        p.status.consecutive_failures = retry::MAX_TRIES + 5;
        let mut f = facts(&p);
        f.blocker = Some(Blocker::Offline);
        let s = decide(&f);
        assert_eq!(
            (FolderState::Waiting, Some(WaitReason::NoNetwork)),
            (s.state, s.wait)
        );
        assert!(s.needs_you.is_empty());
        // Online, the same run history is a failure the person is told about.
        let s = decide(&facts(&p));
        assert_eq!(FolderState::NeedsYou, s.state);
    }

    #[test]
    fn an_offline_device_is_waiting_for_the_network() {
        let p = pair();
        let mut f = facts(&p);
        f.blocker = Some(Blocker::Offline);
        let s = decide(&f);
        assert_eq!(FolderState::Waiting, s.state);
        assert_eq!(Some(WaitReason::NoNetwork), s.wait);
        assert_eq!(Some(Blocker::Offline), s.blocker);
        assert_eq!("folder.waiting_no_network", s.words.key);
    }

    #[test]
    fn a_run_that_couldnt_reach_google_waits_for_the_network_until_the_policy_gives_up() {
        let mut p = pair();
        p.status.last_run_at = Some(5_000);
        p.status.last_status = Some(RunStatus::Failed);
        p.status.last_error_kind = Some(ErrorKind::Network);
        p.status.last_error = Some("connection refused".into());
        p.status.consecutive_failures = 1;
        let s = decide(&facts(&p));
        assert_eq!(FolderState::Waiting, s.state);
        assert_eq!(Some(WaitReason::NoNetwork), s.wait);
        assert_eq!(Some("connection refused"), s.words.details.as_deref());
        assert!(s.needs_you.is_empty(), "a passing failure is no problem");
        assert_eq!(
            Some(RetryInfo {
                attempt: 1,
                max_tries: retry::MAX_TRIES,
                next_at: Some(5_000 + 30_000),
            }),
            s.retry
        );

        p.status.consecutive_failures = retry::MAX_TRIES - 1;
        assert_eq!(FolderState::Waiting, decide(&facts(&p)).state, "one try left");

        p.status.consecutive_failures = retry::MAX_TRIES;
        let s = decide(&facts(&p));
        assert_eq!(FolderState::NeedsYou, s.state, "the policy gave up");
        assert_eq!(vec!["error.network"], keys(&s));
        assert_eq!(Some(crate::words::Fix::Retry), s.needs_you[0].fix);
        assert_eq!(None, s.retry);
    }

    #[test]
    fn a_lasting_failure_needs_you_at_once_without_a_retry() {
        for (kind, key) in [
            (ErrorKind::NotFound, "error.not_found"),
            (ErrorKind::Permission, "error.permission"),
            (ErrorKind::Quota, "error.quota"),
            (ErrorKind::Locked, "error.locked"),
        ] {
            let mut p = pair();
            p.status.last_status = Some(RunStatus::Failed);
            p.status.last_error_kind = Some(kind);
            p.status.consecutive_failures = 1;
            let s = decide(&facts(&p));
            assert_eq!(FolderState::NeedsYou, s.state, "{kind:?}");
            assert_eq!(vec![key], keys(&s), "{kind:?}");
            assert_eq!(None, s.retry, "{kind:?}");
        }
    }

    #[test]
    fn conditions_that_hold_the_next_run_are_the_schedule() {
        let p = pair();
        let mut f = facts(&p);
        f.blocker = Some(Blocker::WaitingForVpn);
        let s = decide(&f);
        assert_eq!(FolderState::Waiting, s.state);
        assert_eq!(Some(WaitReason::Schedule), s.wait);
        assert_eq!(Some(Blocker::WaitingForVpn), s.blocker);
    }

    #[test]
    fn google_being_busy_is_waiting_to_retry() {
        let mut p = pair();
        p.status.last_status = Some(RunStatus::Failed);
        p.status.last_error_kind = Some(ErrorKind::RateLimit);
        p.status.consecutive_failures = 1;
        let s = decide(&facts(&p));
        assert_eq!(
            (FolderState::Waiting, Some(WaitReason::Retrying)),
            (s.state, s.wait)
        );
        assert_eq!("error.rate_limit", s.words.key);
        assert!(s.needs_you.is_empty());
        assert_eq!(Some(1), s.retry.map(|r| r.attempt));
        p.status.consecutive_failures = retry::MAX_TRIES;
        let s = decide(&facts(&p));
        assert_eq!(FolderState::NeedsYou, s.state);
        assert_eq!(vec!["error.rate_limit"], keys(&s));
    }

    #[test]
    fn a_run_that_failed_for_a_reason_wasl_cant_fix_is_failed() {
        let mut p = pair();
        p.status.last_status = Some(RunStatus::Failed);
        p.status.last_error_kind = Some(ErrorKind::Other);
        p.status.last_error = Some("boom at /home/me/Photos/a.txt".into());
        let mut logged = Vec::new();
        let s = folder_status_for(Platform::Linux, &facts(&p), Lang::En, &mut |l| logged.push(l));
        assert_eq!(FolderState::Failed, s.state);
        assert_eq!("error.unknown", s.words.key);
        assert_eq!(
            vec!["error.unknown"],
            keys(&s),
            "a failed folder holds its one failure"
        );
        assert_eq!(
            Some("boom at Photos/a.txt"),
            s.words.details.as_deref(),
            "paths are cut"
        );
        assert_eq!(1, logged.len(), "an unknown error is logged");
    }

    #[test]
    fn open_conflicts_and_a_stopped_plan_each_need_you() {
        let mut p = pair();
        p.status.pending_approval = true;
        let mut f = facts(&p);
        f.open_conflicts = 2;
        let s = decide(&f);
        assert_eq!(FolderState::NeedsYou, s.state);
        assert_eq!(vec!["folder.needs_approval", "folder.conflicts"], keys(&s));
        assert_eq!(Some(2), s.needs_you[1].values.count);
        assert_eq!(2, s.counts.conflicts);
        assert!(s.needs_you[1].text.contains("2 files"));
    }

    #[test]
    fn a_running_folder_is_syncing_unless_the_person_is_needed() {
        let mut p = pair();
        p.status.last_status = Some(RunStatus::Failed);
        p.status.last_error_kind = Some(ErrorKind::Other);
        let mut f = facts(&p);
        f.running = true;
        assert_eq!(
            FolderState::Syncing,
            decide(&f).state,
            "a failure being retried is not news"
        );
        f.open_conflicts = 1;
        assert_eq!(FolderState::NeedsYou, decide(&f).state);
    }

    #[test]
    fn a_folder_that_never_synced_waits_for_its_schedule() {
        let mut p = pair();
        p.status = Default::default();
        let s = decide(&facts(&p));
        assert_eq!(
            (FolderState::Waiting, Some(WaitReason::Schedule)),
            (s.state, s.wait)
        );
    }

    #[test]
    fn the_latest_runs_carry_their_words() {
        let p = {
            let mut p = pair();
            p.status.last_status = Some(RunStatus::Partial);
            p.status.last_error = Some("2 items couldn't sync. First: x".into());
            p
        };
        let run = |id: &str, status, errors| RunRecord {
            id: id.into(),
            pair_id: "p1".into(),
            trigger: "manual".into(),
            status,
            stats: RunStats {
                errors,
                ..Default::default()
            },
            message: None,
            error_kind: None,
            started_at: 10,
            ended_at: Some(20),
        };
        let runs = [run("r2", RunStatus::Partial, 2), run("r1", RunStatus::Success, 0)];
        let mut f = facts(&p);
        f.runs = &runs;
        let s = decide(&f);
        assert_eq!(2, s.last_runs.len());
        assert_eq!("run.partial", s.last_runs[0].words.as_ref().unwrap().key);
        assert!(s.last_runs[1].words.is_none());
    }

    /// An older failed run is worded from the kind its own run kept (issue #57); one stored before
    /// runs kept it reads as the generic sentence.
    #[test]
    fn an_older_failed_run_is_worded_from_its_own_kind() {
        let p = {
            let mut p = pair();
            p.status.last_status = Some(RunStatus::Failed);
            p.status.last_error = Some("no route to host".into());
            p.status.last_error_kind = Some(ErrorKind::Network);
            p
        };
        let run = |id: &str, kind: Option<ErrorKind>| RunRecord {
            id: id.into(),
            pair_id: "p1".into(),
            trigger: "manual".into(),
            status: RunStatus::Failed,
            stats: RunStats::default(),
            message: Some("raw cause".into()),
            error_kind: kind,
            started_at: 10,
            ended_at: Some(20),
        };
        let runs = [
            run("r4", Some(ErrorKind::Network)),
            run("r3", Some(ErrorKind::Auth)),
            run("r2", Some(ErrorKind::Quota)),
            run("r1", None),
        ];
        let mut f = facts(&p);
        f.runs = &runs;
        let status = decide(&f);
        let keys: Vec<&str> = status
            .last_runs
            .iter()
            .map(|r| r.words.as_ref().unwrap().key.as_str())
            .collect();
        assert_eq!(
            vec!["error.network", "error.auth", "error.quota", words::FALLBACK],
            keys
        );
    }

    #[test]
    fn the_same_status_in_arabic_has_the_same_keys_and_arabic_words() {
        let mut p = pair();
        p.status.last_status = Some(RunStatus::Partial);
        p.status.last_error = Some("2 items couldn't sync. First: x".into());
        let en = decide(&facts(&p));
        let ar = folder_status_for(Platform::Linux, &facts(&p), Lang::Ar, &mut |_| {});
        assert_eq!(en.words.key, ar.words.key);
        assert_ne!(en.words.text, ar.words.text);
        assert!(ar.words.text.contains("عنصران"), "{}", ar.words.text);
        assert_eq!(
            ar,
            en.in_lang(Lang::Ar),
            "re-wording an English status gives the Arabic one"
        );
    }

    fn st(state: FolderState, wait: Option<WaitReason>, items: usize) -> FolderStatus {
        let p = pair();
        let mut s = decide(&facts(&p));
        s.state = state;
        s.wait = wait;
        s.needs_you = (0..items)
            .map(|_| {
                words::render(
                    "folder.conflicts",
                    &Values::default(),
                    Lang::En,
                    None,
                    &[],
                    &mut |_| {},
                )
            })
            .collect();
        s
    }

    #[test]
    fn the_overall_state_comes_from_the_folder_statuses() {
        use FolderState::*;
        let o = |v: &[FolderStatus], other| overall_status(v, other, Lang::En);
        assert_eq!(OverallState::Idle, o(&[], 0).state);
        assert_eq!(OverallState::Synced, o(&[st(Synced, None, 0)], 0).state);
        assert_eq!(
            OverallState::Syncing,
            o(&[st(Synced, None, 0), st(Syncing, None, 0)], 0).state
        );
        assert_eq!(
            OverallState::PartlySynced,
            o(
                &[
                    st(Waiting, Some(WaitReason::NoNetwork), 0),
                    st(PartlySynced, None, 0)
                ],
                0
            )
            .state
        );
        assert_eq!(
            OverallState::Waiting,
            o(
                &[st(Synced, None, 0), st(Waiting, Some(WaitReason::NoNetwork), 0)],
                0
            )
            .state
        );
        assert_eq!(
            OverallState::Synced,
            o(
                &[st(Synced, None, 0), st(Waiting, Some(WaitReason::Paused), 0)],
                0
            )
            .state,
            "one paused folder among synced ones doesn't make the app wait"
        );
        assert_eq!(
            OverallState::Paused,
            o(&[st(Waiting, Some(WaitReason::Paused), 0), st(Off, None, 0)], 0).state
        );
        assert_eq!(
            OverallState::Paused,
            o(&[st(Off, None, 0)], 0).state,
            "every folder turned off: nothing is syncing, the person stopped it"
        );
        assert_eq!(OverallState::NeedsYou, o(&[st(NeedsYou, None, 1)], 0).state);
        assert_eq!(
            OverallState::NeedsYou,
            o(&[st(Synced, None, 0)], 1).state,
            "an agent change waits"
        );
    }

    #[test]
    fn the_needs_you_count_is_the_number_of_needs_you_items() {
        use FolderState::*;
        let folders = [st(NeedsYou, None, 2), st(Synced, None, 0), st(Failed, None, 1)];
        let o = overall_status(&folders, 1, Lang::En);
        assert_eq!(4, o.needs_you);
        assert_eq!(
            folders.iter().map(|f| f.needs_you.len() as u32).sum::<u32>() + 1,
            o.needs_you
        );
        assert_eq!("4 things need you", o.needs_you_words.as_ref().unwrap().text);
        assert_eq!("overall.needs_you", o.words.key);
        assert_eq!("Needs you", o.words.text);
        let none = overall_status(&[st(Synced, None, 0)], 0, Lang::En);
        assert!(none.needs_you_words.is_none());
        let ar = overall_status(&folders, 1, Lang::Ar);
        assert_eq!(o.words.key, ar.words.key);
        assert_eq!(o.in_lang(Lang::Ar), ar);
    }

    #[test]
    fn a_failed_folder_makes_the_app_need_you() {
        let o = overall_status(&[st(FolderState::Failed, None, 1)], 0, Lang::En);
        assert_eq!(OverallState::NeedsYou, o.state);
    }

    #[test]
    fn a_status_reads_back_from_json_and_older_fields_are_optional() {
        let p = pair();
        let s = decide(&facts(&p));
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(s, serde_json::from_str::<FolderStatus>(&json).unwrap());
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!("synced", v["state"]);
        assert_eq!("folder.synced", v["words"]["key"]);
        assert!(
            v.get("retry").is_none(),
            "absent unless the folder is being retried"
        );
    }

    #[test]
    fn a_retried_folder_carries_stable_retry_keys() {
        let mut p = pair();
        p.status.last_run_at = Some(5_000);
        p.status.last_status = Some(RunStatus::Failed);
        p.status.last_error_kind = Some(ErrorKind::Server);
        p.status.consecutive_failures = 2;
        let v = serde_json::to_value(decide(&facts(&p))).unwrap();
        assert_eq!("waiting", v["state"]);
        assert_eq!("retrying", v["wait"]);
        assert_eq!(2, v["retry"]["attempt"]);
        assert_eq!(retry::MAX_TRIES, v["retry"]["max_tries"]);
        assert_eq!(5_000 + 120_000, v["retry"]["next_at"]);
        assert_eq!(0, v["needs_you"].as_array().map_or(0, Vec::len));
    }

    // ---- devices (issue #32)

    const NOW: i64 = 1_800_000_000_000;
    const MIN: i64 = 60_000;
    const HOUR: i64 = 60 * MIN;
    const DAY: i64 = 24 * HOUR;

    fn device(connected: bool, syncing: bool, seen_ago: Option<i64>, lang: Lang) -> DeviceStatus {
        device_status(
            &DeviceFacts {
                id: "WASL-AAAA",
                name: "Laptop",
                this_device: false,
                connected,
                syncing,
                last_seen_at: seen_ago.map(|a| NOW - a),
                now: NOW,
            },
            lang,
        )
    }

    #[test]
    fn a_device_is_online_syncing_last_seen_or_unreachable() {
        use DevicePresence::*;
        let cases = [
            (
                device(true, false, None, Lang::En),
                Online,
                "device.online",
                "Online",
            ),
            (
                device(true, true, None, Lang::En),
                Syncing,
                "device.syncing",
                "Syncing",
            ),
            (
                device(false, false, Some(20_000), Lang::En),
                LastSeen,
                "device.last_seen_now",
                "Last seen a moment ago",
            ),
            (
                device(false, false, Some(5 * MIN), Lang::En),
                LastSeen,
                "device.last_seen_minutes",
                "Last seen 5 minutes ago",
            ),
            (
                device(false, false, Some(3 * HOUR + 5 * MIN), Lang::En),
                LastSeen,
                "device.last_seen_hours",
                "Last seen 3 hours ago",
            ),
            (
                device(false, false, Some(3 * DAY), Lang::En),
                LastSeen,
                "device.last_seen_days",
                "Last seen 3 days ago",
            ),
            (
                device(false, false, Some(31 * DAY), Lang::En),
                CantReach,
                "device.cant_reach",
                "Can't reach",
            ),
            (
                device(false, false, None, Lang::En),
                CantReach,
                "device.cant_reach",
                "Can't reach",
            ),
        ];
        for (got, state, key, text) in cases {
            assert_eq!(
                (state, key, text),
                (got.state, got.words.key.as_str(), got.words.text.as_str()),
                "{got:?}"
            );
            assert_eq!(Some("Laptop"), got.words.values.device.as_deref());
        }
    }

    #[test]
    fn last_seen_carries_its_time_as_a_value_and_as_a_timestamp() {
        let d = device(false, false, Some(5 * MIN), Lang::En);
        assert_eq!(Some(5), d.words.values.count);
        assert_eq!(Some(NOW - 5 * MIN), d.last_seen_at);
        let on = device(true, false, Some(5 * MIN), Lang::En);
        assert_eq!(Some(NOW), on.last_seen_at, "online now means seen now");
        let never = device(false, false, None, Lang::En);
        assert_eq!(None, never.last_seen_at);
    }

    #[test]
    fn an_offline_device_stops_being_last_seen_after_thirty_days() {
        let edge = device(false, false, Some(CANT_REACH_AFTER_MS), Lang::En);
        assert_eq!(DevicePresence::LastSeen, edge.state);
        assert_eq!(Some(30), edge.words.values.count);
        let past = device(false, false, Some(CANT_REACH_AFTER_MS + 1), Lang::En);
        assert_eq!(DevicePresence::CantReach, past.state);
        assert_eq!(
            Some(NOW - CANT_REACH_AFTER_MS - 1),
            past.last_seen_at,
            "the time stays for an agent"
        );
        // A clock that moved back never makes a device "seen in the future".
        assert_eq!(
            "device.last_seen_now",
            device(false, false, Some(-5 * MIN), Lang::En).words.key
        );
    }

    #[test]
    fn last_seen_plurals_read_right_in_english() {
        let t = |ago| device(false, false, Some(ago), Lang::En).words.text;
        assert_eq!("Last seen 1 minute ago", t(MIN));
        assert_eq!("Last seen 2 minutes ago", t(2 * MIN + 30_000));
        assert_eq!("Last seen 1 hour ago", t(HOUR));
        assert_eq!("Last seen 23 hours ago", t(23 * HOUR + 59 * MIN));
        assert_eq!("Last seen 1 day ago", t(DAY));
        assert_eq!("Last seen 2 days ago", t(2 * DAY));
    }

    #[test]
    fn last_seen_plurals_and_time_words_read_right_in_arabic() {
        let t = |ago| device(false, false, Some(ago), Lang::Ar).words.text;
        assert_eq!("آخر ظهور قبل لحظات", t(10_000));
        assert_eq!("آخر ظهور قبل دقيقة واحدة", t(MIN));
        assert_eq!("آخر ظهور قبل دقيقتين", t(2 * MIN));
        assert_eq!("آخر ظهور قبل 5 دقائق", t(5 * MIN));
        assert_eq!("آخر ظهور قبل 11 دقيقة", t(11 * MIN));
        assert_eq!("آخر ظهور قبل 59 دقيقة", t(59 * MIN));
        assert_eq!("آخر ظهور قبل ساعة واحدة", t(HOUR));
        assert_eq!("آخر ظهور قبل ساعتين", t(2 * HOUR));
        assert_eq!("آخر ظهور قبل 3 ساعات", t(3 * HOUR));
        assert_eq!("آخر ظهور قبل 12 ساعة", t(12 * HOUR));
        assert_eq!("آخر ظهور قبل يوم واحد", t(DAY));
        assert_eq!("آخر ظهور قبل يومين", t(2 * DAY));
        assert_eq!("آخر ظهور قبل 3 أيام", t(3 * DAY));
        assert_eq!("آخر ظهور قبل 11 يومًا", t(11 * DAY));
        assert_eq!("آخر ظهور قبل 30 يومًا", t(30 * DAY));
        assert_eq!("متصل", device(true, false, None, Lang::Ar).words.text);
        assert_eq!("تجري المزامنة", device(true, true, None, Lang::Ar).words.text);
        assert_eq!(
            "تعذّر الوصول إليه",
            device(false, false, None, Lang::Ar).words.text
        );
    }

    #[test]
    fn the_same_device_in_arabic_has_the_same_state_key_and_values() {
        let en = device(false, false, Some(3 * DAY), Lang::En);
        let ar = device(false, false, Some(3 * DAY), Lang::Ar);
        assert_eq!(
            (en.state, &en.words.key, &en.words.values),
            (ar.state, &ar.words.key, &ar.words.values)
        );
        assert_ne!(en.words.text, ar.words.text);
        assert_eq!(ar, en.in_lang(Lang::Ar));
    }

    fn peer(name: &str, state: DevicePresence) -> FolderPeer {
        FolderPeer {
            device_id: format!("WASL-{name}"),
            name: name.into(),
            state,
        }
    }

    #[test]
    fn a_devices_only_folder_waits_for_the_offline_device_and_names_it() {
        let p = pair();
        let base = decide(&facts(&p));
        let s = with_peers(
            base,
            vec![
                peer("Desktop", DevicePresence::Online),
                peer("Laptop", DevicePresence::LastSeen),
            ],
            true,
            Lang::En,
        );
        assert_eq!(
            (FolderState::Waiting, Some(WaitReason::Device)),
            (s.state, s.wait)
        );
        assert_eq!("folder.waiting_device", s.words.key);
        assert_eq!("Photos will sync when Laptop is back online.", s.words.text);
        assert_eq!(Some("Laptop"), s.words.values.device.as_deref());
        assert_eq!(2, s.peers.len(), "every device is listed");
        let ar = s.in_lang(Lang::Ar);
        assert_eq!("folder.waiting_device", ar.words.key);
        assert_eq!(Some("Laptop"), ar.words.values.device.as_deref());
        assert_eq!("ستتم مزامنة Photos عندما يعود Laptop إلى الاتصال.", ar.words.text);
        let o = overall_status(&[s], 0, Lang::En);
        assert_eq!(OverallState::Waiting, o.state);
        assert_eq!(0, o.needs_you);
    }

    #[test]
    fn a_device_that_cant_be_reached_is_named_as_such() {
        let p = pair();
        let s = with_peers(
            decide(&facts(&p)),
            vec![peer("Old phone", DevicePresence::CantReach)],
            true,
            Lang::En,
        );
        assert_eq!(
            (FolderState::Waiting, Some(WaitReason::Device)),
            (s.state, s.wait)
        );
        assert_eq!("folder.cant_reach_device", s.words.key);
        assert_eq!(
            "Wasl can't reach Old phone, so Photos isn't synced there.",
            s.words.text
        );
        // A device last seen is waited for before one that can't be reached.
        let both = with_peers(
            decide(&facts(&p)),
            vec![
                peer("Old phone", DevicePresence::CantReach),
                peer("Laptop", DevicePresence::LastSeen),
            ],
            true,
            Lang::En,
        );
        assert_eq!("folder.waiting_device", both.words.key);
        assert_eq!(Some("Laptop"), both.words.values.device.as_deref());
    }

    #[test]
    fn devices_never_change_a_folder_that_has_other_news_or_a_drive_copy() {
        let p = pair();
        let offline = || vec![peer("Laptop", DevicePresence::LastSeen)];
        // Drive keeps the folder current: still synced, the device is listed.
        let drive = with_peers(decide(&facts(&p)), offline(), false, Lang::En);
        assert_eq!((FolderState::Synced, None), (drive.state, drive.wait));
        assert_eq!(1, drive.peers.len());
        // Online devices change nothing.
        let on = with_peers(
            decide(&facts(&p)),
            vec![peer("Laptop", DevicePresence::Syncing)],
            true,
            Lang::En,
        );
        assert_eq!(FolderState::Synced, on.state);
        // A folder that needs the person keeps saying so.
        let mut held = pair();
        held.status.hold_for_preview = true;
        let needs = with_peers(decide(&facts(&held)), offline(), true, Lang::En);
        assert_eq!(FolderState::NeedsYou, needs.state);
        assert_eq!("folder.needs_you", needs.words.key);
    }

    #[test]
    fn a_status_with_peers_reads_back_from_json_and_an_older_one_has_none() {
        let p = pair();
        let s = with_peers(
            decide(&facts(&p)),
            vec![peer("Laptop", DevicePresence::Online)],
            false,
            Lang::En,
        );
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(s, serde_json::from_str::<FolderStatus>(&json).unwrap());
        let mut v = serde_json::to_value(&s).unwrap();
        assert_eq!("online", v["peers"][0]["state"]);
        v.as_object_mut().unwrap().remove("peers");
        assert!(
            serde_json::from_value::<FolderStatus>(v)
                .unwrap()
                .peers
                .is_empty()
        );
    }
}
