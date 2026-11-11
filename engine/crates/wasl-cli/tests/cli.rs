//! The real `wasl` binary against a fake daemon on a real socket of this OS.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use wasl_core::{PlanSummary, RunStatus, SyncMode};
use wasl_engine::{Phase, Progress, RunOutcome};
use wasl_ipc::{
    Account, DaemonStatus, DriveFolder, ErrorKind, Event, Handler, IgnoreExplanation, Overall, PairState,
    Paths, RemoteError, Request, Server, ok,
};
use wasl_store::Pair;

const PHOTOS: &str = "aaaa1111-0000-4000-8000-000000000001";
const WORK: &str = "bbbb2222-0000-4000-8000-000000000002";
const MUSIC: &str = "cccc0001-0000-4000-8000-000000000003";
const VIDEOS: &str = "cccc0002-0000-4000-8000-000000000004";
const BROKEN: &str = "dddd4444-0000-4000-8000-000000000005";

#[derive(Default)]
struct Fake {
    events: OnceLock<tokio::sync::broadcast::Sender<Event>>,
    created: Mutex<Option<Pair>>,
    resolved: Mutex<Vec<(String, String, bool)>>,
    paused: Mutex<Vec<(String, bool)>>,
    explained: Mutex<Vec<(String, String)>>,
    added_view: Mutex<Option<wasl_ipc::NewView>>,
    pins: Mutex<Vec<(String, bool)>>,
    removed_views: Mutex<Vec<(String, bool)>>,
    /// Vault requests as (op, vault id or folder, passphrase).
    vault_calls: Mutex<Vec<(String, String, String)>>,
    /// Introductions requests, as text (docs/INTRODUCTIONS.md).
    introductions: Mutex<Vec<String>>,
    /// Op-log requests, as JSON.
    oplog_asked: Mutex<Vec<serde_json::Value>>,
    /// When set, the device list can't be read.
    devices_fail: std::sync::atomic::AtomicBool,
}

const DEV: &str = "WASL-0123-4567-89AB-CDEF-GHJK-MNPQ-RSTV";

/// One staged changeset in the Work pair, in the read format (`show` adds its operations).
fn oplog_page(with_ops: bool) -> wasl_engine::OplogPage {
    use wasl_engine::{OplogEntry, OplogOpEntry, rfc3339};
    let t0 = 1_790_000_000_000;
    let operations = vec![
        OplogOpEntry {
            origin_device: DEV.into(),
            changeset_id: "1790000000000-1".into(),
            seq: 1,
            entity: format!("file://{WORK}/notes/plan.md"),
            op: "create".into(),
            prev: None,
            next: Some(serde_json::json!({"kind": "file", "size": 12, "mtime_ms": t0})),
            reverts: None,
            outcome: None,
            at: rfc3339(t0 + 1).unwrap(),
        },
        OplogOpEntry {
            origin_device: DEV.into(),
            changeset_id: "1790000000000-1".into(),
            seq: 2,
            entity: format!("file://{WORK}/notes/old.md"),
            op: "delete".into(),
            prev: Some(serde_json::json!({"kind": "file", "size": 3, "mtime_ms": t0})),
            next: None,
            reverts: None,
            outcome: Some(wasl_store::OpOutcome::Applied),
            at: rfc3339(t0 + 2).unwrap(),
        },
    ];
    wasl_engine::OplogPage {
        version: wasl_engine::OPLOG_FORMAT_VERSION,
        changesets: vec![OplogEntry {
            origin_device: DEV.into(),
            origin_device_name: Some("Studio".into()),
            id: "1790000000000-1".into(),
            actor: "agent:claude-code".into(),
            label: Some("tidy the notes".into()),
            scope: format!("file://{WORK}/notes/"),
            status: wasl_store::OplogStatus::Staged,
            opened_at: rfc3339(t0).unwrap(),
            closed_at: Some(rfc3339(t0 + 5).unwrap()),
            decided_at: None,
            decided_by: None,
            decided_actor: None,
            imported_at: None,
            op_count: (!with_ops).then_some(2),
            operations: with_ops.then_some(operations),
        }],
    }
}

const VAULT: &str = "ffff6666-0000-4000-8000-000000000007";

fn vault_info(unlocked: bool) -> wasl_ipc::VaultInfo {
    wasl_ipc::VaultInfo {
        id: VAULT.into(),
        name: "Private".into(),
        version: 2,
        format: "2.1".into(),
        unlocked,
        account_id: Some("acc1".into()),
        drive_folder_id: Some("drive:My Drive/Private".into()),
        drive_id: None,
        remote_path: "My Drive/Private".into(),
        created_at: 1,
    }
}

const VIEW: &str = "eeee5555-0000-4000-8000-000000000006";

fn view() -> wasl_ipc::View {
    wasl_ipc::View {
        id: VIEW.into(),
        name: "Archive".into(),
        account_id: "acc1".into(),
        drive_folder_id: "drive:My Drive/Archive".into(),
        drive_id: None,
        remote_path: "My Drive/Archive".into(),
        local_path: "/home/me/Archive".into(),
        vault_id: None,
        cache_limit: 20 << 30,
        enabled: true,
        created_at: 1,
        last_refresh_at: Some(1_760_000_000_000),
        last_error: None,
    }
}

fn pair(id: &str, name: &str) -> Pair {
    let mut p = Pair::new(id, name, &format!("/home/me/{name}"));
    p.account_id = Some("acc1".into());
    p.remote_path = format!("My Drive/{name}");
    p.status.last_success_at = Some(1_760_000_000_000);
    p.status.last_status = Some(RunStatus::Success);
    p
}

fn pairs() -> Vec<Pair> {
    vec![
        pair(PHOTOS, "Photos"),
        pair(WORK, "Work"),
        pair(MUSIC, "Music"),
        pair(VIDEOS, "Videos"),
        pair(BROKEN, "Broken"),
    ]
}

fn outcome(status: RunStatus, message: Option<&str>) -> RunOutcome {
    RunOutcome {
        run_id: "run1".into(),
        status,
        message: message.map(Into::into),
        summary: Some(PlanSummary {
            uploads: 2,
            ..PlanSummary::default()
        }),
        error_kind: None,
        blocker: None,
    }
}

fn progress(pair_id: &str) -> Progress {
    Progress {
        pair_id: pair_id.into(),
        run_id: "run1".into(),
        phase: Phase::Transferring,
        total_actions: 4,
        done_actions: 1,
        bytes_total: 0,
        bytes_done: 0,
        checked: 0,
        errors: 0,
        current_file: None,
    }
}

#[async_trait::async_trait]
impl Handler for Fake {
    async fn handle(&self, request: Request) -> Result<serde_json::Value, RemoteError> {
        match request {
            Request::Status => ok(DaemonStatus {
                agent_changes: 0,
                version: "test".into(),
                protocol: wasl_ipc::PROTOCOL_VERSION,
                accounts: vec![account()],
                pairs: pairs()
                    .into_iter()
                    .map(|pair| PairState {
                        pair,
                        running: false,
                        progress: None,
                        open_conflicts: 0,
                        blocked: false,
                        folder_bytes: None,
                        skipped: None,
                        peers: Vec::new(),
                        free_bytes: None,
                    })
                    .collect(),
                overall: Overall::UpToDate,
            }),
            Request::Pairs => ok(pairs()),
            Request::Accounts => ok(vec![account()]),
            Request::Devices if self.devices_fail.load(std::sync::atomic::Ordering::SeqCst) => {
                Err(RemoteError {
                    kind: ErrorKind::Other,
                    message: "the device list can't be read".into(),
                })
            }
            Request::Devices => ok(wasl_ipc::DevicesInfo {
                enabled: true,
                listen: Some("0.0.0.0:51820".into()),
                devices: vec![
                    wasl_ipc::DeviceInfo {
                        id: "WASL-AAAA".into(),
                        name: "Laptop".into(),
                        this_device: true,
                        connected: false,
                        path: None,
                        added_at: 1,
                        added_by: None,
                        known: true,
                    },
                    wasl_ipc::DeviceInfo {
                        id: "WASL-BBBB".into(),
                        name: "هاتفي".into(),
                        this_device: false,
                        connected: true,
                        path: Some(wasl_ipc::LinkPath::Lan),
                        added_at: 2,
                        added_by: Some("WASL-AAAA".into()),
                        known: true,
                    },
                    wasl_ipc::DeviceInfo {
                        id: "WASL-CCCC".into(),
                        name: "Work laptop".into(),
                        this_device: false,
                        connected: false,
                        path: None,
                        added_at: 3,
                        added_by: Some("WASL-BBBB".into()),
                        known: false,
                    },
                ],
            }),
            Request::Offers => ok(vec![wasl_ipc::OfferInfo {
                share: "00112233445566778899aabbccddeeff".into(),
                from_device: "WASL-BBBB".into(),
                from_name: "هاتفي".into(),
                name: "Projects".into(),
                files: 1240,
                bytes: 2_100_000_000,
                first_seen: 1,
                status: wasl_ipc::OfferStatus::New,
            }]),
            Request::KeepDevice { id } => {
                self.introductions.lock().unwrap().push(format!("keep {id}"));
                ok(())
            }
            Request::DeclineOffer { share, from } => {
                self.introductions
                    .lock()
                    .unwrap()
                    .push(format!("decline {share} {from}"));
                ok(())
            }
            Request::AcceptOffer {
                share,
                from,
                local_root,
                name,
            } => {
                self.introductions
                    .lock()
                    .unwrap()
                    .push(format!("accept {share} {from} {name:?}"));
                let mut p = pair("new-pair", "Projects");
                p.local_root = local_root;
                ok(p)
            }
            Request::SharePair { id, .. } => {
                self.introductions.lock().unwrap().push(format!("share {id}"));
                ok("00112233445566778899aabbccddeeff")
            }
            Request::ShareWith { id, all, devices } => {
                self.introductions
                    .lock()
                    .unwrap()
                    .push(format!("with {id} {all} {}", devices.join(",")));
                ok(())
            }
            Request::Setting { .. } => ok(None::<String>),
            Request::Vaults => ok(vec![vault_info(false)]),
            Request::UnlockVault { id, passphrase } => {
                let p = passphrase.expose().to_owned();
                self.vault_calls
                    .lock()
                    .unwrap()
                    .push(("unlock".into(), id, p.clone()));
                if p == "correct horse battery" {
                    ok(vault_info(true))
                } else {
                    Err(RemoteError {
                        kind: ErrorKind::Invalid,
                        message: "Wrong passphrase for this vault.".into(),
                    })
                }
            }
            Request::UnlockVaultFolder { folder, passphrase } => {
                self.vault_calls.lock().unwrap().push((
                    "unlock_folder".into(),
                    folder.folder_id,
                    passphrase.expose().to_owned(),
                ));
                ok(vault_info(true))
            }
            Request::CreateVault { vault } => {
                self.vault_calls.lock().unwrap().push((
                    "create".into(),
                    format!("{:?}|{}|{}", vault.folder_id, vault.name, vault.remote_path),
                    vault.passphrase.expose().to_owned(),
                ));
                ok(vault_info(true))
            }
            Request::LockVault { id } => {
                self.vault_calls
                    .lock()
                    .unwrap()
                    .push(("lock".into(), id, String::new()));
                ok(())
            }
            Request::ResolveDrivePath {
                account,
                path,
                create,
            } => {
                self.resolved
                    .lock()
                    .unwrap()
                    .push((account, path.clone(), create));
                ok(DriveFolder {
                    id: format!("drive:{path}"),
                    name: path,
                    drive_id: None,
                })
            }
            Request::CreatePair { pair } => {
                *self.created.lock().unwrap() = Some(pair);
                ok(())
            }
            Request::SetPaused { id, paused, .. } => {
                self.paused.lock().unwrap().push((id, paused));
                ok(())
            }
            Request::Sync { id, .. } if id == BROKEN => ok(outcome(RunStatus::Failed, Some("Drive said no"))),
            Request::Sync { id, .. } => {
                if let Some(tx) = self.events.get() {
                    let _ = tx.send(Event::Progress {
                        progress: progress(&id),
                    });
                }
                tokio::time::sleep(Duration::from_millis(300)).await;
                ok(outcome(RunStatus::Success, None))
            }
            Request::Views => ok(wasl_ipc::ViewsInfo {
                supported: true,
                views: vec![wasl_ipc::ViewState {
                    view: view(),
                    mounted: true,
                    files: 12,
                    bytes: 3 << 30,
                    cache_used: 300 << 20,
                    pinned_bytes: 100 << 20,
                    local_changes: vec![wasl_ipc::LocalChange {
                        path: "notes.txt".into(),
                        reason: "only here: not in the listing".into(),
                        kind: Some(wasl_ipc::LocalChangeKind::OnlyHere),
                    }],
                    last_error_kind: None,
                }],
            }),
            Request::AddView { view: new } => {
                *self.added_view.lock().unwrap() = Some(new.clone());
                let mut v = view();
                v.name = new.name;
                v.local_path = new.local_path;
                v.remote_path = new.remote_path;
                v.cache_limit = new.cache_limit.unwrap_or(20 << 30);
                if v.name == "Vaulted" {
                    v.last_error = Some("This folder is an encrypted vault.".into());
                }
                ok(v)
            }
            Request::RemoveView { id, keep_files } => {
                self.removed_views.lock().unwrap().push((id, keep_files));
                ok(())
            }
            Request::PinPath { path } | Request::UnpinPath { path } if path.contains("outside") => {
                Err(RemoteError {
                    kind: ErrorKind::NotFound,
                    message: format!("{path} isn't in a view. See: wasl view list"),
                })
            }
            Request::PinPath { path } => {
                self.pins.lock().unwrap().push((path, true));
                ok(())
            }
            Request::UnpinPath { path } => {
                self.pins.lock().unwrap().push((path, false));
                ok(())
            }
            Request::IgnoreExplain { pair_id, path } => {
                self.explained.lock().unwrap().push((pair_id, path.clone()));
                ok(if path.starts_with("node_modules/") {
                    IgnoreExplanation {
                        ignored: true,
                        rule: Some("node_modules/".into()),
                        source: Some(".waslignore:3".into()),
                        note: None,
                    }
                } else {
                    IgnoreExplanation {
                        ignored: false,
                        rule: None,
                        source: None,
                        note: Some("The size limit was checked against the file on this device only.".into()),
                    }
                })
            }
            Request::OplogList { .. } => {
                self.oplog_asked
                    .lock()
                    .unwrap()
                    .push(serde_json::to_value(&request).unwrap());
                ok(oplog_page(false))
            }
            Request::OplogShow {
                ref origin_device,
                ref id,
            } => {
                self.oplog_asked
                    .lock()
                    .unwrap()
                    .push(serde_json::to_value(&request).unwrap());
                if id != "1790000000000-1" || origin_device.as_deref().is_some_and(|d| d != DEV) {
                    return Err(RemoteError {
                        kind: ErrorKind::NotFound,
                        message: format!("changeset {id}"),
                    });
                }
                ok(oplog_page(true))
            }
            _ => Err(RemoteError {
                kind: ErrorKind::Unsupported,
                message: "not in this test".into(),
            }),
        }
    }
}

fn account() -> Account {
    Account {
        id: "acc1".into(),
        email: "me@example.com".into(),
        auth_error: None,
    }
}

struct Daemon {
    dir: tempfile::TempDir,
    fake: Arc<Fake>,
}

fn start() -> Daemon {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(Fake::default());
    let server = Server::bind(&Paths::at(dir.path()), fake.clone(), "test").unwrap();
    let _ = fake.events.set(server.events());
    tokio::spawn(server.serve());
    Daemon { dir, fake }
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

async fn wasl(home: &Path, lang: &str, args: &[&str]) -> Run {
    wasl_with_input(home, lang, args, None).await
}

/// Runs `wasl` with `input` on its standard input (a passphrase for --passphrase-stdin).
async fn wasl_with_input(home: &Path, lang: &str, args: &[&str], input: Option<&str>) -> Run {
    let input = input.map(str::to_owned);
    let home: PathBuf = home.to_owned();
    let lang = lang.to_owned();
    let args: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
    tokio::task::spawn_blocking(move || {
        let mut child = Command::new(env!("CARGO_BIN_EXE_wasl"))
            .args(&args)
            .env("WASL_HOME", &home)
            .env("WASL_LANG", &lang)
            .env("NO_COLOR", "1")
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(text) = input {
            use std::io::Write as _;
            child.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
        }
        let out = child.wait_with_output().unwrap();
        Run {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8(out.stdout).unwrap(),
            stderr: String::from_utf8(out.stderr).unwrap(),
        }
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn status_shows_the_daemon_and_each_pair() {
    let d = start();
    let r = wasl(d.dir.path(), "en", &["status"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(r.stdout.contains("Wasl (wasld test): Synced"), "{}", r.stdout);
    let photos = r.stdout.lines().find(|l| l.starts_with("Photos")).unwrap();
    assert!(photos.contains("/home/me/Photos ↔ My Drive/Photos"), "{photos}");
    assert!(photos.contains("Synced"), "{photos}");
    // Local time, to the minute.
    assert!(photos.contains("2025-10-"), "{photos}");
    assert!(r.stdout.contains("LAST SYNC"));
}

#[tokio::test(flavor = "multi_thread")]
async fn status_json_is_machine_readable() {
    let d = start();
    let r = wasl(d.dir.path(), "ar", &["status", "--json"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    let v: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
    assert_eq!("test", v["version"]);
    assert_eq!("up_to_date", v["overall"]);
    assert_eq!(5, v["pairs"].as_array().unwrap().len());
    assert_eq!("Photos", v["pairs"][0]["pair"]["name"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn pair_list_shows_every_pair() {
    let d = start();
    let r = wasl(d.dir.path(), "en", &["pair", "list"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    let line = r.stdout.lines().find(|l| l.contains("Photos")).unwrap();
    assert!(line.starts_with("aaaa1111"), "{line}");
    assert!(line.contains("Two-way") && line.contains("My Drive/Photos") && line.contains("On"));
    assert_eq!(6, r.stdout.lines().count(), "{}", r.stdout);
}

#[tokio::test(flavor = "multi_thread")]
async fn pair_add_creates_the_pair_it_was_asked_for() {
    let d = start();
    let local = tempfile::tempdir().unwrap();
    let folder = local.path().join("Camera");
    std::fs::create_dir(&folder).unwrap();
    let r = wasl(
        d.dir.path(),
        "en",
        &[
            "pair",
            "add",
            folder.to_str().unwrap(),
            "Backups/Camera",
            "--method",
            "upload-only",
            "--name",
            "Phone camera",
        ],
    )
    .await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(r.stdout.contains("Added Phone camera"), "{}", r.stdout);
    let p = d.fake.created.lock().unwrap().clone().unwrap();
    assert_eq!("Phone camera", p.name);
    assert_eq!(folder.display().to_string(), p.local_root);
    assert_eq!(Some("drive:My Drive/Backups/Camera".into()), p.drive_folder_id);
    assert_eq!("My Drive/Backups/Camera", p.remote_path);
    assert_eq!(SyncMode::UploadOnly, p.method);
    assert_eq!(Some("acc1".into()), p.account_id);
    assert_eq!(36, p.id.len());
    assert_eq!(
        vec![("acc1".to_owned(), "My Drive/Backups/Camera".to_owned(), false)],
        *d.fake.resolved.lock().unwrap()
    );

    // Defaults: the folder's name, two-way, and the folder is created on request.
    let r = wasl(
        d.dir.path(),
        "en",
        &[
            "pair",
            "add",
            folder.to_str().unwrap(),
            "My Drive/New",
            "--account",
            "ME@example.com",
            "--create-drive-folder",
        ],
    )
    .await;
    assert_eq!(0, r.code, "{}", r.stderr);
    let p = d.fake.created.lock().unwrap().clone().unwrap();
    assert_eq!("Camera", p.name);
    assert_eq!(SyncMode::TwoWay, p.method);
    assert!(d.fake.resolved.lock().unwrap()[1].2);
    assert!(!p.honor_gitignore && p.presets.is_empty() && !p.sync_hidden);

    // E2E 2026-10: the computer offered five of the seven methods, and no way to honour
    // .gitignore, pick a preset or sync hidden files (PRD P0 ignore rules).
    let r = wasl(
        d.dir.path(),
        "en",
        &[
            "pair",
            "add",
            folder.to_str().unwrap(),
            "My Drive/Code",
            "--method",
            "upload-then-delete",
            "--gitignore",
            "--preset",
            "node",
            "--preset",
            "os-junk",
            "--hidden",
        ],
    )
    .await;
    assert_eq!(0, r.code, "{}", r.stderr);
    let p = d.fake.created.lock().unwrap().clone().unwrap();
    assert_eq!(SyncMode::UploadThenDelete, p.method);
    assert!(p.honor_gitignore && p.sync_hidden);
    assert_eq!(vec!["node".to_owned(), "os-junk".to_owned()], p.presets);
    let r = wasl(
        d.dir.path(),
        "en",
        &[
            "pair",
            "add",
            folder.to_str().unwrap(),
            "X",
            "--method",
            "download-then-delete",
            "--preset",
            "nodejs",
        ],
    )
    .await;
    assert_eq!(2, r.code, "{}", r.stderr);
    assert!(r.stderr.contains("No preset is called nodejs"), "{}", r.stderr);

    // A folder that does not exist, an unknown account.
    let r = wasl(d.dir.path(), "en", &["pair", "add", "/no/such/folder/x", "X"]).await;
    assert_eq!(4, r.code);
    let r = wasl(
        d.dir.path(),
        "en",
        &[
            "pair",
            "add",
            folder.to_str().unwrap(),
            "X",
            "--account",
            "you@example.com",
        ],
    )
    .await;
    assert_eq!(4, r.code);
    assert!(r.stderr.contains("you@example.com"), "{}", r.stderr);
}

#[tokio::test(flavor = "multi_thread")]
async fn pairs_are_named_by_prefix_or_name_and_ambiguity_is_refused() {
    let d = start();
    let r = wasl(d.dir.path(), "en", &["pause", "aaaa"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert_eq!("Paused Photos.\n", r.stdout.replace("\r\n", "\n"));
    let r = wasl(d.dir.path(), "en", &["resume", "WORK"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(r.stdout.contains("Resumed Work."));
    assert_eq!(
        vec![(PHOTOS.to_owned(), true), (WORK.to_owned(), false)],
        *d.fake.paused.lock().unwrap()
    );

    let r = wasl(d.dir.path(), "en", &["pause", "cccc"]).await;
    assert_eq!(2, r.code);
    assert!(r.stderr.contains("more than one"), "{}", r.stderr);
    assert!(
        r.stderr.contains(MUSIC) && r.stderr.contains(VIDEOS),
        "{}",
        r.stderr
    );

    let r = wasl(d.dir.path(), "en", &["pause", "nope"]).await;
    assert_eq!(4, r.code);
    assert!(
        r.stderr.contains("No folder pair matches \"nope\""),
        "{}",
        r.stderr
    );
    assert_eq!(2, d.fake.paused.lock().unwrap().len());
}

#[tokio::test(flavor = "multi_thread")]
async fn pause_and_resume_all() {
    let d = start();
    let r = wasl(d.dir.path(), "en", &["pause", "--all"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    let r = wasl(d.dir.path(), "en", &["resume", "--all"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    let calls = d.fake.paused.lock().unwrap().clone();
    assert_eq!(10, calls.len());
    assert!(calls[..5].iter().all(|(_, p)| *p));
    assert!(calls[5..].iter().all(|(_, p)| !*p));
    // Neither a pair nor --all is a usage error.
    let r = wasl(d.dir.path(), "en", &["pause"]).await;
    assert_eq!(2, r.code);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_sync_exits_non_zero() {
    let d = start();
    let r = wasl(d.dir.path(), "en", &["sync", "broken"]).await;
    assert_eq!(1, r.code, "{}{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("Broken: sync failed: Drive said no"),
        "{}",
        r.stdout
    );
    let r = wasl(d.dir.path(), "en", &["sync", "broken", "--wait"]).await;
    assert_eq!(1, r.code);
}

#[tokio::test(flavor = "multi_thread")]
async fn sync_wait_shows_progress_until_the_end() {
    let d = start();
    let r = wasl(d.dir.path(), "en", &["sync", "Photos", "--wait"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(
        r.stdout.contains("Photos: Transferring 25% · 1/4 files"),
        "{}",
        r.stdout
    );
    assert!(
        r.stdout.contains("Photos: synced (2 up, 0 down, 0 deleted)"),
        "{}",
        r.stdout
    );

    // Without --wait it returns once the run has started.
    let r = wasl(d.dir.path(), "en", &["sync", "photos"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(r.stdout.contains("Sync started for Photos."), "{}", r.stdout);
}

#[tokio::test(flavor = "multi_thread")]
async fn ignore_explains_the_rule() {
    let d = start();
    let r = wasl(
        d.dir.path(),
        "en",
        &[
            "ignore",
            "--explain",
            "photos",
            "/home/me/Photos/node_modules/x.js",
        ],
    )
    .await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(r.stdout.contains("node_modules/x.js is ignored."), "{}", r.stdout);
    assert!(r.stdout.contains("Rule: node_modules/"));
    assert!(r.stdout.contains("From: .waslignore:3"));
    assert_eq!(
        (PHOTOS.to_owned(), "node_modules/x.js".to_owned()),
        d.fake.explained.lock().unwrap()[0]
    );

    let r = wasl(d.dir.path(), "en", &["ignore", "photos", "a/b.jpg"]).await;
    assert_eq!(2, r.code);
}

#[tokio::test(flavor = "multi_thread")]
async fn with_a_note_explain_does_not_say_the_file_syncs() {
    let d = start();
    let r = wasl(d.dir.path(), "en", &["ignore", "--explain", "photos", "a/b.jpg"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(
        r.stdout.contains("a/b.jpg is not ignored on this device.") && !r.stdout.contains("it syncs"),
        "a note means Drive may still decide: {}",
        r.stdout
    );
    assert!(
        r.stdout
            .contains("  The size limit was checked against the file on this device only."),
        "the engine's note is shown: {}",
        r.stdout
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_daemon_it_says_how_to_start_one() {
    let empty = tempfile::tempdir().unwrap();
    for args in [
        &["status"][..],
        &["pair", "list"],
        &["daemon", "status"],
        &["sync"],
    ] {
        let r = wasl(empty.path(), "en", args).await;
        assert_eq!(3, r.code, "{args:?}");
        assert!(
            r.stderr
                .contains("wasld is not running. Start it with: wasl daemon start"),
            "{}",
            r.stderr
        );
    }
    let r = wasl(empty.path(), "ar", &["status"]).await;
    assert_eq!(3, r.code);
    assert!(
        r.stderr
            .contains("خطأ: wasld لا يعمل. شغّله بالأمر: wasl daemon start"),
        "{}",
        r.stderr
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn arabic_output() {
    let d = start();
    let r = wasl(d.dir.path(), "ar", &["status"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(
        r.stdout.contains("وصل (wasld test): تمت المزامنة"),
        "{}",
        r.stdout
    );
    assert!(r.stdout.contains("آخر مزامنة"));
    assert!(r.stdout.contains("2025-10-"), "Western digits: {}", r.stdout);

    let r = wasl(d.dir.path(), "ar", &["pause", "nope"]).await;
    assert_eq!(4, r.code);
    assert!(r.stderr.contains("لا يوجد ربط يطابق \"nope\""), "{}", r.stderr);

    let r = wasl(d.dir.path(), "ar", &["pause", "photos"]).await;
    assert!(r.stdout.contains("Photos: متوقف مؤقتًا."), "{}", r.stdout);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_old_pin_command_points_to_views() {
    let d = start();
    let r = wasl(d.dir.path(), "en", &["pin", "list"]).await;
    assert_eq!(2, r.code);
    assert!(r.stderr.contains("wasl view pin"), "{}", r.stderr);
}

#[tokio::test(flavor = "multi_thread")]
async fn views_are_listed_added_pinned_and_removed() {
    let d = start();
    let r = wasl(d.dir.path(), "en", &["view", "list"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    let line = r.stdout.lines().find(|l| l.contains("Archive")).unwrap();
    assert!(line.starts_with("eeee5555"), "{line}");
    assert!(
        line.contains("12 files") && line.contains("/home/me/Archive"),
        "{line}"
    );
    assert!(line.contains("300.0 MB of 20.0 GB (100.0 MB kept)"), "{line}");
    assert!(
        r.stdout.contains("notes.txt"),
        "local changes are reported: {}",
        r.stdout
    );

    let r = wasl(d.dir.path(), "ar", &["view", "list"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(r.stdout.contains("على هذا الحاسوب"), "{}", r.stdout);

    let r = wasl(d.dir.path(), "en", &["view", "list", "--json"]).await;
    let v: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
    assert_eq!("Archive", v["views"][0]["view"]["name"]);

    let local = tempfile::tempdir().unwrap();
    let folder = local.path().join("Archive here");
    let r = wasl(
        d.dir.path(),
        "en",
        &[
            "view",
            "add",
            "Archive",
            folder.to_str().unwrap(),
            "--cache-limit",
            "5G",
        ],
    )
    .await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(
        r.stdout.contains("Archive shows My Drive/Archive"),
        "{}",
        r.stdout
    );
    let added = d.fake.added_view.lock().unwrap().clone().unwrap();
    assert_eq!("Archive", added.name);
    assert_eq!("drive:My Drive/Archive", added.drive_folder_id);
    assert_eq!(folder.display().to_string(), added.local_path);
    assert_eq!(Some(5 << 30), added.cache_limit);
    assert_eq!("acc1", added.account_id);
    // The Drive folder must exist: a view never creates one.
    assert!(!d.fake.resolved.lock().unwrap()[0].2);

    let r = wasl(
        d.dir.path(),
        "en",
        &[
            "view",
            "add",
            "Vault",
            folder.to_str().unwrap(),
            "--name",
            "Vaulted",
        ],
    )
    .await;
    assert_eq!(1, r.code);
    assert!(r.stdout.contains("encrypted vault"), "{}", r.stdout);
    let r = wasl(
        d.dir.path(),
        "en",
        &["view", "add", "X", "/y", "--cache-limit", "lots"],
    )
    .await;
    assert_eq!(2, r.code);

    let pinned = local.path().join("a.jpg");
    let r = wasl(d.dir.path(), "en", &["view", "pin", pinned.to_str().unwrap()]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(r.stdout.contains("stays on this computer"), "{}", r.stdout);
    let r = wasl(d.dir.path(), "en", &["view", "unpin", pinned.to_str().unwrap()]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert_eq!(
        vec![
            (pinned.display().to_string(), true),
            (pinned.display().to_string(), false)
        ],
        *d.fake.pins.lock().unwrap()
    );
    let r = wasl(d.dir.path(), "en", &["view", "pin", "/outside/x"]).await;
    assert_eq!(4, r.code);

    let r = wasl(
        d.dir.path(),
        "en",
        &["view", "remove", "archive", "--keep-files", "--yes"],
    )
    .await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(r.stdout.contains("Removed the view Archive"), "{}", r.stdout);
    assert_eq!(
        vec![(VIEW.to_owned(), true)],
        *d.fake.removed_views.lock().unwrap()
    );
    let r = wasl(d.dir.path(), "en", &["view", "remove", "nope", "--yes"]).await;
    assert_eq!(4, r.code);
}

#[tokio::test(flavor = "multi_thread")]
async fn devices_are_listed_in_both_languages() {
    let d = start();
    let r = wasl(d.dir.path(), "en", &["device", "list"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(
        r.stdout.contains("Laptop") && r.stdout.contains("This computer"),
        "{}",
        r.stdout
    );
    assert!(
        r.stdout.contains("هاتفي") && r.stdout.contains("connected (same network)"),
        "{}",
        r.stdout
    );
    assert!(r.stdout.contains("port 51820"), "{}", r.stdout);
    let r = wasl(d.dir.path(), "ar", &["device", "list"]).await;
    assert!(
        r.stdout.contains("هذا الحاسوب") && r.stdout.contains("متصل"),
        "{}",
        r.stdout
    );
    let r = wasl(d.dir.path(), "en", &["device", "remove", "nobody", "--yes"]).await;
    assert_eq!(4, r.code, "{}", r.stderr);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_device_is_shown_with_who_added_it_and_can_be_kept() {
    let d = start();
    let r = wasl(d.dir.path(), "en", &["device", "list"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(r.stdout.contains("new: not kept here yet"), "{}", r.stdout);
    assert!(
        r.stdout.contains("هاتفي added Work laptop to your devices"),
        "{}",
        r.stdout
    );
    let r = wasl(d.dir.path(), "en", &["device", "keep", "Work laptop"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert_eq!(
        vec!["keep WASL-CCCC".to_string()],
        *d.fake.introductions.lock().unwrap()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn offers_are_listed_accepted_and_declined() {
    let d = start();
    let r = wasl(d.dir.path(), "en", &["device", "offers"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(
        r.stdout.contains("Projects") && r.stdout.contains("هاتفي") && r.stdout.contains("1240 files"),
        "{}",
        r.stdout
    );
    let folder = d.dir.path().join("Projects here");
    let r = wasl(
        d.dir.path(),
        "en",
        &["device", "accept", "0011", folder.to_str().unwrap()],
    )
    .await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(r.stdout.contains("first preview"), "{}", r.stdout);
    let r = wasl(d.dir.path(), "en", &["device", "decline", "Projects"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    let r = wasl(d.dir.path(), "en", &["device", "decline", "nothing"]).await;
    assert_eq!(4, r.code, "{}", r.stderr);
    assert_eq!(
        vec![
            "accept 00112233445566778899aabbccddeeff WASL-BBBB None".to_string(),
            "decline 00112233445566778899aabbccddeeff WASL-BBBB".to_string()
        ],
        *d.fake.introductions.lock().unwrap()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pair_is_shared_with_chosen_devices() {
    let d = start();
    let r = wasl(
        d.dir.path(),
        "en",
        &["pair", "share", "Photos", "--with", "هاتفي"],
    )
    .await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(r.stdout.contains("offered to: هاتفي"), "{}", r.stdout);
    let r = wasl(
        d.dir.path(),
        "en",
        &["pair", "share", "Photos", "--with", "nobody"],
    )
    .await;
    assert_eq!(4, r.code, "an unknown device shares nothing");
    let calls = d.fake.introductions.lock().unwrap().clone();
    assert_eq!(2, calls.len(), "{calls:?}");
    assert!(calls[0].starts_with("share "));
    assert!(calls[1].ends_with(" false WASL-BBBB"), "{calls:?}");
    // Without --with: as before, a share code.
    let r = wasl(d.dir.path(), "en", &["pair", "share", "Photos"]).await;
    assert!(r.stdout.contains("--code 00112233"), "{}", r.stdout);
}

#[tokio::test(flavor = "multi_thread")]
async fn daemon_status_names_the_version() {
    let d = start();
    let r = wasl(d.dir.path(), "en", &["daemon", "status"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(r.stdout.contains("wasld is running (version test)."));
}

#[tokio::test(flavor = "multi_thread")]
async fn vaults_are_listed_unlocked_made_and_locked() {
    let d = start();
    let r = wasl(d.dir.path(), "en", &["vault", "list"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    let line = r.stdout.lines().find(|l| l.contains("Private")).unwrap();
    assert!(
        line.starts_with("ffff6666") && line.contains("v2") && line.contains("Locked"),
        "{line}"
    );
    let r = wasl(d.dir.path(), "ar", &["vault", "list"]).await;
    assert!(r.stdout.contains("مقفلة"), "{}", r.stdout);

    // The passphrase comes from standard input (or a prompt without echo), never an argument.
    let r = wasl_with_input(
        d.dir.path(),
        "en",
        &["vault", "unlock", "private", "--passphrase-stdin"],
        Some("correct horse battery\n"),
    )
    .await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(
        r.stdout.contains("Private is unlocked on this computer"),
        "{}",
        r.stdout
    );
    assert!(!r.stdout.contains("correct horse") && !r.stderr.contains("correct horse"));
    let r = wasl_with_input(
        d.dir.path(),
        "ar",
        &["vault", "unlock", "ffff", "--passphrase-stdin"],
        Some("wrong one\r\n"),
    )
    .await;
    assert_eq!(5, r.code, "a wrong passphrase: {}", r.stderr);
    assert!(r.stderr.contains("عبارة المرور غير صحيحة"), "{}", r.stderr);
    let r = wasl_with_input(
        d.dir.path(),
        "en",
        &["vault", "unlock", "Private", "--passphrase-stdin"],
        Some("\n"),
    )
    .await;
    assert_eq!(2, r.code, "an empty passphrase: {}", r.stderr);
    // Without a terminal and without --passphrase-stdin there is nowhere to ask.
    let r = wasl(d.dir.path(), "en", &["vault", "unlock", "Private"]).await;
    assert_eq!(2, r.code);
    assert!(r.stderr.contains("--passphrase-stdin"), "{}", r.stderr);

    // A Drive folder this computer doesn't know as a vault yet.
    let r = wasl_with_input(
        d.dir.path(),
        "en",
        &["vault", "unlock", "My Drive/Old vault", "--passphrase-stdin"],
        Some("correct horse battery\n"),
    )
    .await;
    assert_eq!(0, r.code, "{}", r.stderr);

    let r = wasl_with_input(
        d.dir.path(),
        "en",
        &["vault", "create", "Taxes", "--passphrase-stdin"],
        Some("short\n"),
    )
    .await;
    assert_eq!(2, r.code, "{}", r.stderr);
    assert!(r.stderr.contains("at least 8 characters"), "{}", r.stderr);
    let r = wasl_with_input(
        d.dir.path(),
        "en",
        &["vault", "create", "Taxes", "--passphrase-stdin"],
        Some("a long enough passphrase\n"),
    )
    .await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(
        r.stdout.contains("Made the vault Private in My Drive/Taxes"),
        "{}",
        r.stdout
    );
    let r = wasl(d.dir.path(), "en", &["vault", "create", "My Drive"]).await;
    assert_eq!(2, r.code, "{}", r.stderr);

    let r = wasl(d.dir.path(), "en", &["vault", "lock", "Private"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(
        r.stdout.contains("Private is locked on this computer"),
        "{}",
        r.stdout
    );
    let r = wasl(d.dir.path(), "en", &["vault", "lock", "Nope"]).await;
    assert_eq!(4, r.code);

    let calls = d.fake.vault_calls.lock().unwrap().clone();
    let call = |op: &str, what: &str, pass: &str| (op.to_string(), what.to_string(), pass.to_string());
    assert_eq!(
        vec![
            call("unlock", VAULT, "correct horse battery"),
            call("unlock", VAULT, "wrong one"),
            call(
                "unlock_folder",
                "drive:My Drive/Old vault",
                "correct horse battery"
            ),
            call(
                "create",
                "Some(\"drive:My Drive/Taxes\")|Taxes|My Drive/Taxes",
                "a long enough passphrase"
            ),
            call("lock", VAULT, ""),
        ],
        calls
    );
    // Only the vault's own folder was made; its parent had to exist.
    let resolved = d.fake.resolved.lock().unwrap().clone();
    assert!(
        resolved.contains(&("acc1".into(), "My Drive".into(), false)),
        "{resolved:?}"
    );
    assert!(
        resolved.contains(&("acc1".into(), "My Drive/Taxes".into(), true)),
        "{resolved:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn oplog_list_shows_each_changeset_and_passes_the_filters() {
    let d = start();
    let r = wasl(d.dir.path(), "en", &["oplog", "list"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    let mut lines = r.stdout.lines();
    let head = lines.next().unwrap();
    for col in ["ID", "ACTOR", "PAIR", "SCOPE", "STARTED", "STATE", "CHANGES"] {
        assert!(head.contains(col), "{head}");
    }
    let row = lines.next().unwrap();
    assert!(
        row.starts_with(&format!("{DEV}:1790000000000-1")),
        "another device's changeset names its device, since a bare id means this one: {row}"
    );
    for part in ["agent:claude-code", "Work", "/notes", "staged", " 2"] {
        assert!(row.contains(part), "{part} in {row}");
    }

    let r = wasl(
        d.dir.path(),
        "en",
        &[
            "oplog",
            "list",
            "--actor",
            "agent:gemini",
            "--status",
            "committed",
            "--since",
            "2026-10-01T00:00:00Z",
            "--pair",
            "work",
        ],
    )
    .await;
    assert_eq!(0, r.code, "{}", r.stderr);
    let asked = d.fake.oplog_asked.lock().unwrap().last().cloned().unwrap();
    assert_eq!("agent:gemini", asked["actor"]);
    assert_eq!("committed", asked["status"]);
    assert_eq!(1_790_812_800_000_i64, asked["since"]);
    assert_eq!(WORK, asked["pair"], "the pair is named by name and sent by id");
    assert_eq!(100, asked["limit"], "the newest 100 by default");
    let r = wasl(d.dir.path(), "en", &["oplog", "list", "--limit", "5"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    let asked = d.fake.oplog_asked.lock().unwrap().last().cloned().unwrap();
    assert_eq!(5, asked["limit"]);
    assert!(!r.stderr.contains("--limit"), "{}", r.stderr);
    // As many as asked for: a note on stderr that there may be more; JSON keeps its shape.
    let r = wasl(d.dir.path(), "en", &["oplog", "list", "--limit", "1"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(
        r.stderr.contains("Showing the newest 1; use --limit for more."),
        "{}",
        r.stderr
    );
    let r = wasl(d.dir.path(), "en", &["oplog", "list", "--limit", "1", "--json"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(!r.stderr.contains("--limit"), "{}", r.stderr);

    let r = wasl(d.dir.path(), "en", &["oplog", "list", "--status", "held"]).await;
    assert_eq!(2, r.code);
    assert!(r.stderr.contains("partly_reverted"), "{}", r.stderr);
    let r = wasl(d.dir.path(), "en", &["oplog", "list", "--since", "soon"]).await;
    assert_eq!(2, r.code);
    for back in ["--since=-7d", "--since=-1m"] {
        let r = wasl(d.dir.path(), "en", &["oplog", "list", back]).await;
        assert_eq!(2, r.code, "a count below zero is in the future: {back}");
        assert!(r.stderr.contains("Can't read the time"), "{}", r.stderr);
    }
    for (arg, back) in [("30m", 1_800_000_i64), ("24h", 86_400_000), ("7d", 604_800_000)] {
        let before = chrono::Utc::now().timestamp_millis();
        let r = wasl(d.dir.path(), "en", &["oplog", "list", "--since", arg]).await;
        let after = chrono::Utc::now().timestamp_millis();
        assert_eq!(0, r.code, "{}", r.stderr);
        let asked = d.fake.oplog_asked.lock().unwrap().last().cloned().unwrap();
        let since = asked["since"].as_i64().unwrap();
        assert!(before - back <= since && since <= after - back, "{arg}: {since}");
    }
    let r = wasl(d.dir.path(), "en", &["oplog", "list", "--since", "2026-10-01"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    let asked = d.fake.oplog_asked.lock().unwrap().last().cloned().unwrap();
    let midnight = chrono::NaiveDate::from_ymd_opt(2026, 10, 1)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_local_timezone(chrono::Local)
        .earliest()
        .unwrap()
        .timestamp_millis();
    assert_eq!(midnight, asked["since"], "a date is local midnight");

    let r = wasl(d.dir.path(), "ar", &["oplog", "list"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(r.stdout.contains("الفاعل"), "{}", r.stdout);
}

#[tokio::test(flavor = "multi_thread")]
async fn oplog_list_takes_an_unknown_pair_as_an_id_and_refuses_an_ambiguous_one() {
    let d = start();
    let r = wasl(d.dir.path(), "en", &["oplog", "list", "--pair", "Gone"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert!(
        r.stderr.contains("No pair named Gone; matching it as a pair id."),
        "{}",
        r.stderr
    );
    let asked = d.fake.oplog_asked.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        "Gone", asked["pair"],
        "a deleted pair's changesets stay until pruned"
    );

    let asks = d.fake.oplog_asked.lock().unwrap().len();
    let r = wasl(d.dir.path(), "en", &["oplog", "list", "--pair", "cccc"]).await;
    assert_eq!(2, r.code, "{}", r.stdout);
    assert!(
        r.stderr.contains(MUSIC) && r.stderr.contains(VIDEOS),
        "{}",
        r.stderr
    );
    assert_eq!(asks, d.fake.oplog_asked.lock().unwrap().len(), "nothing is asked");
}

#[tokio::test(flavor = "multi_thread")]
async fn oplog_show_names_a_changeset_with_or_without_its_device() {
    let d = start();
    let r = wasl(d.dir.path(), "en", &["oplog", "show", "1790000000000-1"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    let mut lines = r.stdout.lines();
    let head = lines.next().unwrap();
    assert!(
        head.starts_with(&format!("{DEV}:1790000000000-1  agent:claude-code")),
        "{head}"
    );
    assert_eq!("  tidy the notes", lines.next().unwrap());
    assert!(r.stdout.contains("SEQ"), "{}", r.stdout);
    let delete = r.stdout.lines().find(|l| l.contains("old.md")).unwrap();
    assert!(
        delete.starts_with("2 ") && delete.contains("delete") && delete.contains("applied"),
        "{delete}"
    );
    let asked = d.fake.oplog_asked.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        serde_json::Value::Null,
        asked["origin_device"],
        "a bare id is this device"
    );

    let r = wasl(
        d.dir.path(),
        "en",
        &["oplog", "show", &format!("{DEV}:1790000000000-1")],
    )
    .await;
    assert_eq!(0, r.code, "{}", r.stderr);
    let asked = d.fake.oplog_asked.lock().unwrap().last().cloned().unwrap();
    assert_eq!(DEV, asked["origin_device"]);
    assert_eq!("1790000000000-1", asked["id"]);

    let r = wasl(
        d.dir.path(),
        "en",
        &["oplog", "show", "WASL-OTHER:1790000000000-1"],
    )
    .await;
    assert_eq!(4, r.code, "{}", r.stdout);
}

/// The read format is a contract with Raqeeb, Qasd and Miqyas (ADR-0002): removing or renaming a
/// field must raise `version`, so this list changes only with it.
const CHANGESET_FIELDS: &[&str] = &[
    "origin_device",
    "origin_device_name",
    "id",
    "actor",
    "label",
    "scope",
    "status",
    "opened_at",
    "closed_at",
    "decided_at",
    "decided_by",
    "decided_actor",
    "imported_at",
];
const OPERATION_FIELDS: &[&str] = &[
    "seq",
    "origin_device",
    "changeset_id",
    "entity",
    "op",
    "prev",
    "next",
    "reverts",
    "outcome",
    "at",
];

fn keys(v: &serde_json::Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
    k.sort();
    k
}

fn sorted(fields: &[&str], extra: &str) -> Vec<String> {
    let mut k: Vec<String> = fields.iter().chain([&extra]).map(|s| (*s).to_owned()).collect();
    k.sort();
    k
}

#[tokio::test(flavor = "multi_thread")]
async fn oplog_list_fails_when_the_device_list_cannot_be_read() {
    let d = start();
    d.fake
        .devices_fail
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let r = wasl(d.dir.path(), "en", &["oplog", "list"]).await;
    assert_ne!(0, r.code);
    assert_eq!("", r.stdout, "no list whose ids may name the wrong device");
    assert!(r.stderr.contains("the device list can't be read"), "{}", r.stderr);
}

/// The exact text of `list` and `show` for the fake's changeset: a change to either is seen here
/// first. The start time is local, in the zone `wasl` and this test share (chrono reads `TZ` on
/// Unix and the system zone on Windows), so it is worked out here rather than written down.
#[tokio::test(flavor = "multi_thread")]
async fn oplog_list_and_show_print_exactly_this_text() {
    let d = start();
    let started = chrono::DateTime::from_timestamp_millis(1_790_000_000_000)
        .unwrap()
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%d %H:%M")
        .to_string();
    let r = wasl(d.dir.path(), "en", &["oplog", "list"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert_eq!(GOLDEN_LIST.replace("{STARTED}", &started), r.stdout);
    let r = wasl(d.dir.path(), "en", &["oplog", "show", "1790000000000-1"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert_eq!(GOLDEN_SHOW.replace("{WORK}", WORK), r.stdout);
}

/// The exact JSON of `list --json` and `show --json` for the fake's changeset.
#[tokio::test(flavor = "multi_thread")]
async fn oplog_json_prints_exactly_this_object() {
    let d = start();
    let r = wasl(d.dir.path(), "en", &["oplog", "list", "--json"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert_eq!(GOLDEN_LIST_JSON.replace("{WORK}", WORK), r.stdout);
    let r = wasl(
        d.dir.path(),
        "en",
        &["oplog", "show", "1790000000000-1", "--json"],
    )
    .await;
    assert_eq!(0, r.code, "{}", r.stderr);
    assert_eq!(GOLDEN_SHOW_JSON.replace("{WORK}", WORK), r.stdout);
}

const GOLDEN_LIST: &str = "\
ID                                                       ACTOR              PAIR  SCOPE   STARTED           STATE   CHANGES
WASL-0123-4567-89AB-CDEF-GHJK-MNPQ-RSTV:1790000000000-1  agent:claude-code  Work  /notes  {STARTED}  staged  2
";
const GOLDEN_SHOW: &str = "\
WASL-0123-4567-89AB-CDEF-GHJK-MNPQ-RSTV:1790000000000-1  agent:claude-code  file://{WORK}/notes/  staged
  tidy the notes
SEQ  CHANGE  ENTITY                                                     RESULT
1    create  file://{WORK}/notes/plan.md
2    delete  file://{WORK}/notes/old.md   applied
";
const GOLDEN_LIST_JSON: &str = r#"{
  "version": 1,
  "changesets": [
    {
      "origin_device": "WASL-0123-4567-89AB-CDEF-GHJK-MNPQ-RSTV",
      "origin_device_name": "Studio",
      "id": "1790000000000-1",
      "actor": "agent:claude-code",
      "label": "tidy the notes",
      "scope": "file://{WORK}/notes/",
      "status": "staged",
      "opened_at": "2026-09-21T14:13:20.000Z",
      "closed_at": "2026-09-21T14:13:20.005Z",
      "decided_at": null,
      "decided_by": null,
      "decided_actor": null,
      "imported_at": null,
      "op_count": 2
    }
  ]
}
"#;
const GOLDEN_SHOW_JSON: &str = r#"{
  "version": 1,
  "changesets": [
    {
      "origin_device": "WASL-0123-4567-89AB-CDEF-GHJK-MNPQ-RSTV",
      "origin_device_name": "Studio",
      "id": "1790000000000-1",
      "actor": "agent:claude-code",
      "label": "tidy the notes",
      "scope": "file://{WORK}/notes/",
      "status": "staged",
      "opened_at": "2026-09-21T14:13:20.000Z",
      "closed_at": "2026-09-21T14:13:20.005Z",
      "decided_at": null,
      "decided_by": null,
      "decided_actor": null,
      "imported_at": null,
      "operations": [
        {
          "origin_device": "WASL-0123-4567-89AB-CDEF-GHJK-MNPQ-RSTV",
          "changeset_id": "1790000000000-1",
          "seq": 1,
          "entity": "file://{WORK}/notes/plan.md",
          "op": "create",
          "prev": null,
          "next": {
            "kind": "file",
            "mtime_ms": 1790000000000,
            "size": 12
          },
          "reverts": null,
          "outcome": null,
          "at": "2026-09-21T14:13:20.001Z"
        },
        {
          "origin_device": "WASL-0123-4567-89AB-CDEF-GHJK-MNPQ-RSTV",
          "changeset_id": "1790000000000-1",
          "seq": 2,
          "entity": "file://{WORK}/notes/old.md",
          "op": "delete",
          "prev": {
            "kind": "file",
            "mtime_ms": 1790000000000,
            "size": 3
          },
          "next": null,
          "reverts": null,
          "outcome": "applied",
          "at": "2026-09-21T14:13:20.002Z"
        }
      ]
    }
  ]
}
"#;

#[tokio::test(flavor = "multi_thread")]
async fn oplog_json_keeps_every_field_of_version_1() {
    let d = start();
    let r = wasl(d.dir.path(), "en", &["oplog", "list", "--json"]).await;
    assert_eq!(0, r.code, "{}", r.stderr);
    let v: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
    assert_eq!(vec!["changesets", "version"], keys(&v));
    assert_eq!(1, v["version"]);
    let cs = &v["changesets"][0];
    assert_eq!(sorted(CHANGESET_FIELDS, "op_count"), keys(cs));
    assert_eq!(DEV, cs["origin_device"]);
    assert_eq!("Studio", cs["origin_device_name"]);
    assert_eq!("staged", cs["status"]);
    assert_eq!("2026-09-21T14:13:20.000Z", cs["opened_at"]);
    assert_eq!(serde_json::Value::Null, cs["decided_actor"]);
    assert_eq!(2, cs["op_count"]);

    let r = wasl(
        d.dir.path(),
        "en",
        &["oplog", "show", "1790000000000-1", "--json"],
    )
    .await;
    assert_eq!(0, r.code, "{}", r.stderr);
    let v: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
    assert_eq!(1, v["version"]);
    let cs = &v["changesets"][0];
    assert_eq!(sorted(CHANGESET_FIELDS, "operations"), keys(cs));
    let op = &cs["operations"][0];
    assert_eq!(OPERATION_FIELDS.len(), keys(op).len());
    assert_eq!(sorted(&OPERATION_FIELDS[1..], OPERATION_FIELDS[0]), keys(op));
    assert_eq!(format!("file://{WORK}/notes/plan.md"), op["entity"]);
    assert_eq!(DEV, op["origin_device"]);
    assert_eq!("1790000000000-1", op["changeset_id"]);
    assert_eq!(12, op["next"]["size"], "states are JSON objects, not strings");
    assert_eq!(serde_json::Value::Null, op["prev"]);
    assert_eq!("applied", cs["operations"][1]["outcome"]);
}
