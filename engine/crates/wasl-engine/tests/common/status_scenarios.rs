//! Folder situations seeded into a database, and what a person and an agent must read of each
//! (issues #29, #31, #32). The daemon's tests and the phone binding's tests both include this file
//! (`#[path = "../../wasl-engine/tests/common/status_scenarios.rs"] mod scenarios;`), seed each
//! situation the same way, and check the status they get against the same expectation, so the two
//! can't drift apart: same state, same key, same words.
//!
//! A scenario seeds the database before an engine or daemon opens it (one process holds the store).

#![allow(dead_code)]

use wasl_proto::{ErrorKind, FolderState, FolderStatus, OverallState, RunStatus, WaitReason};
use wasl_store::{Pair, Store};

pub const ACCOUNT: &str = "me@example.com";
pub const PAIR: &str = "p1";
pub const NAME: &str = "Sync";

/// What a status must say.
pub struct Expect {
    pub state: FolderState,
    pub wait: Option<WaitReason>,
    /// The headline's catalogue key.
    pub key: &'static str,
    /// The keys of what needs the person, most urgent first.
    pub needs_you: &'static [&'static str],
    /// The whole app's state, worked out from this one folder.
    pub overall: OverallState,
}

pub struct Scenario {
    pub name: &'static str,
    pub seed: fn(&Store, &str),
    pub expect: Expect,
}

/// The pair every scenario starts from: set up and synced once.
pub fn pair(root: &str) -> Pair {
    let mut p = Pair::new(PAIR, NAME, root);
    p.account_id = Some(ACCOUNT.into());
    p.drive_folder_id = Some("A".into());
    p.remote_path = "My Drive/Sync".into();
    p.instant_sync = false;
    p.interval_minutes = 600;
    p.status.last_status = Some(RunStatus::Success);
    p.status.last_success_at = Some(1_790_000_000_000);
    // Ran just now, so no schedule takes it up while a test reads it.
    p.status.last_run_at = Some(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64),
    );
    p
}

fn seed_with(store: &Store, root: &str, change: impl FnOnce(&mut Pair)) {
    let mut p = pair(root);
    change(&mut p);
    store.set_account_email(ACCOUNT, ACCOUNT).unwrap();
    store.create_pair(&p).unwrap();
}

/// The number the system itself prints for "no space left on the device".
fn no_space() -> String {
    let code = if cfg!(windows) { 112 } else { 28 };
    format!("No space left on the device (os error {code})")
}

pub fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "first preview",
            seed: |s, root| seed_with(s, root, |p| p.status.hold_for_preview = true),
            expect: Expect {
                state: FolderState::NeedsYou,
                wait: None,
                key: "folder.needs_you",
                needs_you: &["folder.first_preview"],
                overall: OverallState::NeedsYou,
            },
        },
        Scenario {
            name: "partial run",
            seed: |s, root| {
                seed_with(s, root, |p| {
                    p.status.last_status = Some(RunStatus::Partial);
                    p.status.last_error =
                        Some("2 items couldn't sync. First: a.txt is open in another program".into());
                })
            },
            expect: Expect {
                state: FolderState::PartlySynced,
                wait: None,
                key: "run.partial",
                needs_you: &[],
                overall: OverallState::PartlySynced,
            },
        },
        Scenario {
            name: "a failure that may pass",
            seed: |s, root| {
                seed_with(s, root, |p| {
                    p.status.last_status = Some(RunStatus::Failed);
                    p.status.last_error_kind = Some(ErrorKind::Network);
                    p.status.last_error = Some("dns error".into());
                    p.status.consecutive_failures = 1;
                })
            },
            expect: Expect {
                state: FolderState::Waiting,
                wait: Some(WaitReason::NoNetwork),
                key: "folder.waiting_no_network",
                needs_you: &[],
                overall: OverallState::Waiting,
            },
        },
        Scenario {
            name: "a lasting failure",
            seed: |s, root| {
                seed_with(s, root, |p| {
                    p.status.last_status = Some(RunStatus::Failed);
                    p.status.last_error_kind = Some(ErrorKind::Network);
                    p.status.last_error = Some("dns error".into());
                    p.status.consecutive_failures = 20;
                })
            },
            expect: Expect {
                state: FolderState::Failed,
                wait: None,
                key: "error.network",
                needs_you: &["error.network"],
                overall: OverallState::NeedsYou,
            },
        },
        Scenario {
            name: "a full disk",
            seed: |s, root| {
                seed_with(s, root, |p| {
                    p.status.last_status = Some(RunStatus::Failed);
                    p.status.last_error_kind = Some(ErrorKind::Io);
                    p.status.last_error = Some(no_space());
                    p.status.consecutive_failures = 1;
                })
            },
            expect: Expect {
                state: FolderState::NeedsYou,
                wait: None,
                key: "error.disk_full",
                needs_you: &["error.disk_full"],
                overall: OverallState::NeedsYou,
            },
        },
        Scenario {
            name: "signed out",
            seed: |s, root| {
                seed_with(s, root, |_| {});
                s.set_account_auth_error(ACCOUNT, Some("invalid_grant")).unwrap();
            },
            expect: Expect {
                state: FolderState::NeedsYou,
                wait: None,
                key: "folder.needs_you",
                needs_you: &["error.auth"],
                overall: OverallState::NeedsYou,
            },
        },
    ]
}

/// Says what is wrong with `status` against `expect`, or nothing. Words are checked in English:
/// the keys are what an agent reads, the words what a person does.
pub fn mismatch(status: &FolderStatus, expect: &Expect) -> Option<String> {
    let keys: Vec<&str> = status.needs_you.iter().map(|r| r.key.as_str()).collect();
    let got = (status.state, status.wait, status.words.key.as_str(), keys);
    let want = (expect.state, expect.wait, expect.key, expect.needs_you.to_vec());
    (got != want).then(|| format!("got {got:?}, wanted {want:?}"))
}
