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
//! 5. The last run says the rest. A run that failed for a reason the person can fix is **needs you**;
//!    one that couldn't reach Google (or Google was busy) is **waiting** until it has kept failing;
//!    any other failure is **failed**. A run that moved some files and not others is **partly
//!    synced**, never "synced" and never "failed".
//! 6. An offline device, or conditions that hold the next run, are **waiting**.
//! 7. Otherwise the folder is **synced**.
//!
//! The whole-app state ([`overall_status`]) is worked out from the folder statuses by the same rules.

use crate::conditions::Blocker;
use crate::run::{ErrorKind, RunStatus};
use crate::store::{Pair, RunRecord, RunStats};
use crate::words::{self, Lang, Platform, Rendered, Values};
use serde::{Deserialize, Serialize};

/// A folder keeps failing for a reason that retrying fixes (no network, Google busy) this many runs
/// in a row before it reads "failed" instead of "waiting".
pub const RETRIES_BEFORE_FAILED: u32 = 3;

/// How many of a folder's latest runs a status carries.
pub const LAST_RUNS: usize = 3;

/// A folder's state: one closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
}

/// Numbers a screen draws beside the words.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
        words: say("folder.synced", &values, None),
        needs_you: Vec::new(),
        counts: FolderCounts {
            conflicts: facts.open_conflicts,
            not_moved: 0,
            skipped: facts.skipped,
        },
        last_synced_at: st.last_success_at,
        last_runs: Vec::new(),
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
                // Only the newest run's cause is kept with the folder; an older failure is worded by
                // the fallback, quietly, until runs keep their own entry.
                RunStatus::Failed => {
                    let (key, cause) = if newest {
                        (failed_key.unwrap_or(words::FALLBACK), raw)
                    } else {
                        (words::FALLBACK, r.message.as_deref())
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
            "error.auth",
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
            let transient = matches!(
                kind,
                ErrorKind::Network | ErrorKind::RateLimit | ErrorKind::Server
            );
            // A network failure while the device is offline is the offline wait, however many runs
            // it has missed: nothing here is the person's to fix.
            let offline_now = kind == ErrorKind::Network && facts.blocker == Some(Blocker::Offline);
            if transient && (st.consecutive_failures < RETRIES_BEFORE_FAILED || offline_now) {
                status.state = FolderState::Waiting;
                if kind == ErrorKind::Network {
                    status.wait = Some(WaitReason::NoNetwork);
                    status.words = say("folder.waiting_no_network", &values, raw);
                } else {
                    status.wait = Some(WaitReason::Retrying);
                    status.words = say(key, &values, raw);
                }
                return status;
            }
            let item = say(key, &values, raw);
            status.state = if FIXABLE.contains(&key) {
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

/// The failures a person can do something about; the rest read "failed".
const FIXABLE: &[&str] = &[
    "error.auth",
    "error.permission",
    "error.quota",
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
        p.status.consecutive_failures = RETRIES_BEFORE_FAILED + 5;
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
        assert_eq!(FolderState::Failed, s.state);
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
    fn a_run_that_couldnt_reach_google_waits_for_the_network_until_it_keeps_failing() {
        let mut p = pair();
        p.status.last_status = Some(RunStatus::Failed);
        p.status.last_error_kind = Some(ErrorKind::Network);
        p.status.last_error = Some("connection refused".into());
        p.status.consecutive_failures = 1;
        let s = decide(&facts(&p));
        assert_eq!(FolderState::Waiting, s.state);
        assert_eq!(Some(WaitReason::NoNetwork), s.wait);
        assert_eq!(Some("connection refused"), s.words.details.as_deref());

        p.status.consecutive_failures = RETRIES_BEFORE_FAILED;
        let s = decide(&facts(&p));
        assert_eq!(FolderState::Failed, s.state);
        assert_eq!(vec!["error.network"], keys(&s));
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
    }
}
