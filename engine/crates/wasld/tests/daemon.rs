//! The daemon through its own protocol (SRS FR-E4's second binding): a real socket of this OS, a
//! real engine and database, Drive and Google sign-in mocked.

use std::sync::{Arc, Mutex};
use std::time::Duration;
use wasl_core::RunStatus;
use wasl_core::conditions::Conditions;
use wasl_drive::DriveEndpoints;
use wasl_engine::Trigger;
use wasl_ipc::{Client, ErrorKind, Event, Overall, Paths};
use wasl_store::{Pair, StorageMode};
use wasld::daemon::DaemonConfig;
use wasld::oauth::OAuthConfig;
use wasld::schedule::Timing;
use wasld::tokens::{MemorySecrets, Secrets};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const ME: &str = "me@example.com";

fn json(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.into_bytes(), "application/json; charset=utf-8")
}

fn md5(s: &str) -> String {
    wasl_fs::hash_reader(s.as_bytes()).unwrap().md5
}

/// How long Drive takes to answer an upload whose name starts with `slow`, to hold a run.
const SLOW_UPLOAD: Duration = Duration::from_secs(3);

/// Google: OAuth endpoints, and a Drive with folder `A` holding `remote.txt` that accepts
/// uploads. Records the bearer tokens and upload names it saw.
struct Google {
    server: MockServer,
    uploads: Arc<Mutex<Vec<String>>>,
    bearers: Arc<Mutex<Vec<String>>>,
}

async fn google() -> Google {
    let server = MockServer::builder().start().await;
    let uploads = Arc::new(Mutex::new(Vec::new()));
    let bearers = Arc::new(Mutex::new(Vec::new()));
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(|req: &Request| {
            let body = String::from_utf8_lossy(&req.body);
            if body.contains("grant_type=authorization_code") && body.contains("code=good-code") {
                json(r#"{"access_token":"fresh","refresh_token":"refresh-new","expires_in":3600}"#.into())
            } else if body.contains("grant_type=refresh_token") {
                json(r#"{"access_token":"refreshed","expires_in":3600}"#.into())
            } else {
                ResponseTemplate::new(400).set_body_string(r#"{"error":"invalid_grant"}"#)
            }
        })
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/userinfo"))
        .respond_with(json(format!(r#"{{"email":"{ME}"}}"#)))
        .mount(&server)
        .await;
    let b = bearers.clone();
    Mock::given(method("GET"))
        .and(path("/drive/v3/files/root"))
        .respond_with(move |req: &Request| {
            if let Some(v) = req.headers.get("authorization") {
                b.lock().unwrap().push(v.to_str().unwrap_or("").to_owned());
            }
            json(r#"{"id":"ROOT","name":"My Drive","mimeType":"application/vnd.google-apps.folder"}"#.into())
        })
        .mount(&server)
        .await;
    // The pair's folder, which a first (live) listing checks is still there.
    Mock::given(method("GET"))
        .and(path("/drive/v3/files/A"))
        .respond_with(json(
            r#"{"id":"A","name":"Sync","mimeType":"application/vnd.google-apps.folder","parents":["ROOT"]}"#
                .into(),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/drive/v3/changes/startPageToken"))
        .respond_with(json(r#"{"startPageToken":"7"}"#.into()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/drive/v3/changes"))
        .respond_with(json(r#"{"newStartPageToken":"7","changes":[]}"#.into()))
        .mount(&server)
        .await;
    let remote = "from drive";
    Mock::given(method("GET"))
        .and(path("/drive/v3/files"))
        .respond_with(json(format!(
            r#"{{"files":[
                {{"id":"A","name":"Sync","mimeType":"application/vnd.google-apps.folder","parents":["ROOT"]}},
                {{"id":"B","name":"remote.txt","mimeType":"text/plain","parents":["A"],"size":"{}","md5Checksum":"{}","modifiedTime":"2026-01-02T03:04:05.000Z","version":"2"}}
            ]}}"#,
            remote.len(),
            md5(remote)
        )))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/drive/v3/files/B"))
        .and(query_param("alt", "media"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(remote.as_bytes().to_vec()))
        .mount(&server)
        .await;
    // A view's range read checks the file is still the listed version.
    Mock::given(method("GET"))
        .and(path("/drive/v3/files/B"))
        .and(query_param("fields", "id,version,md5Checksum"))
        .respond_with(json(format!(
            r#"{{"id":"B","version":"2","md5Checksum":"{}"}}"#,
            md5(remote)
        )))
        .mount(&server)
        .await;
    let up = uploads.clone();
    Mock::given(method("POST"))
        .and(path("/upload/drive/v3/files"))
        .respond_with(move |req: &Request| {
            let body = String::from_utf8_lossy(&req.body).into_owned();
            let content = body
                .split("\r\n\r\n")
                .nth(2)
                .and_then(|s| s.split("\r\n--").next())
                .unwrap_or("")
                .to_owned();
            let name = body
                .split("\"name\":\"")
                .nth(1)
                .and_then(|s| s.split('"').next())
                .unwrap_or("file")
                .to_owned();
            let mut u = up.lock().unwrap();
            u.push(name.clone());
            let answer = json(format!(
                r#"{{"id":"U{}","name":"{name}","mimeType":"text/plain","parents":["A"],"size":"{}","md5Checksum":"{}","version":"1"}}"#,
                u.len(),
                content.len(),
                md5(&content)
            ));
            if name.starts_with("slow") {
                answer.set_delay(SLOW_UPLOAD)
            } else {
                answer
            }
        })
        .mount(&server)
        .await;
    Google {
        server,
        uploads,
        bearers,
    }
}

struct Running {
    paths: Paths,
    secrets: Arc<MemorySecrets>,
    vault_keys: Arc<MemorySecrets>,
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    handle: tokio::task::JoinHandle<Result<(), String>>,
}

async fn start(g: &Google, signed_in: bool) -> Running {
    start_with(g, signed_in, None).await
}

async fn start_with(
    g: &Google,
    signed_in: bool,
    view_backend: Option<Arc<dyn wasld::views::Backend>>,
) -> Running {
    start_full(g, signed_in, view_backend, None).await
}

/// With `devices`, the daemon has a device id, so agent changes go to the op-log (ADR-0002).
async fn start_full(
    g: &Google,
    signed_in: bool,
    view_backend: Option<Arc<dyn wasld::views::Backend>>,
    devices: Option<wasl_devices::DeviceConfig>,
) -> Running {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Sync");
    std::fs::create_dir_all(&root).unwrap();
    let paths = Paths::at(dir.path().join("data"));
    let secrets = Arc::new(MemorySecrets::default());
    let vault_keys = Arc::new(MemorySecrets::default());
    if signed_in {
        secrets.set(ME, "refresh-old").unwrap();
    }
    let uri = g.server.uri();
    let cfg = DaemonConfig {
        data_dir: paths.data.clone(),
        oauth: OAuthConfig {
            client_id: "test-client".into(),
            client_secret: "test-secret".into(),
            auth_url: format!("{uri}/auth"),
            token_url: format!("{uri}/token"),
            userinfo_url: format!("{uri}/userinfo"),
            scopes: vec!["openid".into(), "email".into()],
        },
        secrets: secrets.clone(),
        vault_keys: vault_keys.clone(),
        drive_endpoints: Some(DriveEndpoints {
            base: format!("{uri}/drive/v3"),
            upload: format!("{uri}/upload/drive/v3"),
        }),
        open_browser: false,
        device_name: "Test PC".into(),
        sign_in_timeout: Duration::from_secs(10),
        devices,
        view_backend,
        view_refresh: Duration::from_secs(300),
    };
    let timing = Timing {
        tick: Duration::from_millis(100),
        settle: Duration::from_millis(400),
        skip_wait: Duration::from_secs(1),
        bin_purge_every: Duration::from_secs(3600),
    };
    let p = paths.clone();
    let handle = tokio::spawn(async move { wasld::serve(&p, cfg, timing).await });
    // Wait for the socket.
    for _ in 0..100 {
        if Client::connect(&paths).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Running {
        paths,
        secrets,
        vault_keys,
        _dir: dir,
        root,
        handle,
    }
}

fn pair(root: &std::path::Path) -> Pair {
    let mut p = Pair::new("p1", "Sync", root.to_str().unwrap());
    p.account_id = Some(ME.into());
    p.drive_folder_id = Some("A".into());
    p.remote_path = "My Drive/Sync".into();
    p.conditions = Conditions::none();
    p.storage_mode = StorageMode::Files;
    p.instant_sync = false;
    p.interval_minutes = 600;
    p
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pair_runs_end_to_end_through_the_daemon() {
    let g = google().await;
    let d = start(&g, true).await;
    std::fs::write(d.root.join("local.txt"), "from this computer").unwrap();
    let c = Client::connect(&d.paths).await.unwrap();
    assert_eq!(Overall::Idle, c.status().await.unwrap().overall);

    let mut p = pair(&d.root);
    p.status.hold_for_preview = true;
    c.create_pair(p).await.unwrap();
    let preview = c.preview("p1").await.unwrap();
    assert_eq!(
        (1, 1),
        (preview.plan.summary.uploads, preview.plan.summary.downloads)
    );
    let run = c.sync("p1", Trigger::Preview).await.unwrap();
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    assert_eq!(
        "from drive",
        std::fs::read_to_string(d.root.join("remote.txt")).unwrap()
    );
    assert_eq!(vec!["local.txt".to_string()], *g.uploads.lock().unwrap());
    assert!(
        g.bearers.lock().unwrap().iter().all(|b| b == "Bearer refreshed"),
        "the keychain's refresh token gave the access token"
    );

    let status = c.status().await.unwrap();
    assert_eq!(Overall::UpToDate, status.overall);
    assert_eq!(1, status.pairs.len());
    assert_eq!(Some(RunStatus::Success), status.pairs[0].pair.status.last_status);
    let runs = c.runs(Some("p1"), 10).await.unwrap();
    assert!(
        c.run_events(&runs[0].id)
            .await
            .unwrap()
            .iter()
            .any(|e| e.action == "UPLOAD")
    );

    c.set_paused("p1", true, Some("by test".into())).await.unwrap();
    assert_eq!(Overall::Paused, c.status().await.unwrap().overall);
    c.shutdown().await.unwrap();
    assert!(d.handle.await.unwrap().is_ok(), "shutdown ends serve");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_sign_in_while_one_waits_joins_it() {
    // #147 M4: Settings, Problems, the New pair sheet and first run each start a sign-in, from more than one
    // window. Only one may wait at a time; another call gets the same address, and no second browser opens.
    let g = google().await;
    let d = start(&g, false).await;
    let a = Client::connect(&d.paths).await.unwrap();
    let b = Client::connect(&d.paths).await.unwrap();
    let first = a.add_account().await.unwrap();
    let second = b.add_account().await.unwrap();
    assert_eq!(first, second);
    a.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn google_sign_in_completes_through_the_loopback_redirect() {
    let g = google().await;
    let d = start(&g, false).await;
    let c = Client::connect(&d.paths).await.unwrap();
    let mut events = c.subscribe().await.unwrap();
    let url = c.add_account().await.unwrap();
    assert!(url.starts_with(&format!("{}/auth?", g.server.uri())));
    assert!(url.contains("code_challenge_method=S256"));
    let q: std::collections::HashMap<String, String> = url
        .split_once('?')
        .unwrap()
        .1
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.replace("%3A", ":").replace("%2F", "/")))
        .collect();
    let redirect = q["redirect_uri"].clone();
    let state = q["state"].clone();

    // A stray request without our state is ignored; the browser's redirect completes it.
    let http = reqwest::Client::new();
    let stray = http
        .get(format!("{redirect}?code=evil&state=wrong"))
        .send()
        .await
        .unwrap();
    assert_eq!(404, stray.status().as_u16());
    let page = http
        .get(format!("{redirect}?code=good-code&state={state}"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains("Signed in"));

    let added = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match events.recv().await {
                Some(Event::AccountAdded { account }) => return account,
                Some(Event::AccountFailed { message }) => panic!("{message}"),
                Some(_) => continue,
                None => panic!("events ended"),
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(ME, added.email);
    assert_eq!(Some("refresh-new".into()), d.secrets.get(ME));
    assert_eq!(
        vec![ME.to_string()],
        c.accounts()
            .await
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_in_the_folder_syncs_without_asking() {
    let g = google().await;
    let d = start(&g, true).await;
    let c = Client::connect(&d.paths).await.unwrap();
    let mut p = pair(&d.root);
    p.instant_sync = true;
    c.create_pair(p).await.unwrap();
    // The first merge needs the preview; after it the pair runs on its own.
    assert_eq!(
        RunStatus::Success,
        c.sync("p1", Trigger::Preview).await.unwrap().status
    );
    tokio::time::sleep(Duration::from_millis(3500)).await; // the watcher is up

    std::fs::write(d.root.join("new.txt"), "written later").unwrap();
    let uploaded = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if g.uploads.lock().unwrap().iter().any(|n| n == "new.txt") {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    assert!(uploaded.is_ok(), "the watcher's hint started a run");
    let runs = c.runs(Some("p1"), 10).await.unwrap();
    assert_eq!("instant", runs[0].trigger);
}

/// A pair that syncs on change, past its preview, with its watcher up.
async fn syncing_on_change(c: &Client, d: &Running) {
    let mut p = pair(&d.root);
    p.instant_sync = true;
    c.create_pair(p).await.unwrap();
    assert_eq!(
        RunStatus::Success,
        c.sync("p1", Trigger::Preview).await.unwrap().status
    );
    tokio::time::sleep(Duration::from_millis(3500)).await;
}

/// Whether Drive saw an upload of `name` within `wait`.
async fn uploaded(g: &Google, name: &str, wait: Duration) -> bool {
    tokio::time::timeout(wait, async {
        while !g.uploads.lock().unwrap().iter().any(|n| n == name) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .is_ok()
}

async fn instant_runs(c: &Client) -> usize {
    let runs = c.runs(Some("p1"), 20).await.unwrap();
    runs.iter().filter(|r| r.trigger == "instant").count()
}

/// Seen on the Mac (issue #50): a file written while the watcher's run was in flight waited
/// for the next trigger.
#[tokio::test(flavor = "multi_thread")]
async fn a_change_during_a_run_gets_a_follow_up_run() {
    let g = google().await;
    let d = start(&g, true).await;
    let c = Client::connect(&d.paths).await.unwrap();
    syncing_on_change(&c, &d).await;

    std::fs::write(d.root.join("slow.txt"), "held").unwrap();
    assert!(
        uploaded(&g, "slow.txt", Duration::from_secs(15)).await,
        "the watcher started a run"
    );
    // That run is now held on the slow upload, past its scan.
    std::fs::write(d.root.join("during.txt"), "written mid-run").unwrap();

    assert!(
        uploaded(&g, "during.txt", Duration::from_secs(15)).await,
        "the change during the run got a run of its own"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(2, instant_runs(&c).await, "the watcher's run and its follow-up");
}

#[tokio::test(flavor = "multi_thread")]
async fn several_changes_during_a_run_give_one_follow_up_run() {
    let g = google().await;
    let d = start(&g, true).await;
    let c = Client::connect(&d.paths).await.unwrap();
    syncing_on_change(&c, &d).await;

    std::fs::write(d.root.join("slow.txt"), "held").unwrap();
    assert!(
        uploaded(&g, "slow.txt", Duration::from_secs(15)).await,
        "the watcher started a run"
    );
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(d.root.join(name), name).unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    for name in ["a.txt", "b.txt", "c.txt"] {
        assert!(
            uploaded(&g, name, Duration::from_secs(15)).await,
            "{name} went in the follow-up"
        );
    }
    // Past the settle time, so a second follow-up would have started.
    tokio::time::sleep(Duration::from_millis(4500)).await;
    assert_eq!(2, instant_runs(&c).await, "the watcher's run and one follow-up");
    let mut uploads = g.uploads.lock().unwrap().clone();
    uploads.sort();
    assert_eq!(
        vec!["a.txt", "b.txt", "c.txt", "slow.txt"],
        uploads,
        "each file went up once"
    );
}

/// Issue #88: a sync the person started is followed like the watcher's runs, and its own file
/// doesn't run the pair again. The pair syncs on change only once slow.txt is written, so no run
/// of the watcher's own can start ahead of the manual sync and leave it skipped.
#[tokio::test(flavor = "multi_thread")]
async fn a_change_during_a_manual_sync_gets_one_follow_up_run() {
    let g = google().await;
    let d = start(&g, true).await;
    let c = Client::connect(&d.paths).await.unwrap();
    c.create_pair(pair(&d.root)).await.unwrap();
    assert_eq!(
        RunStatus::Success,
        c.sync("p1", Trigger::Preview).await.unwrap().status
    );
    std::fs::write(d.root.join("slow.txt"), "held").unwrap();
    let mut p = c.pair("p1").await.unwrap().unwrap();
    p.instant_sync = true;
    c.update_pair(p).await.unwrap();
    tokio::time::sleep(Duration::from_millis(3500)).await; // the watcher is up

    let manual = {
        let c = Client::connect(&d.paths).await.unwrap();
        tokio::spawn(async move { c.sync("p1", Trigger::Manual).await.unwrap().status })
    };
    let running = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let runs = c.runs(Some("p1"), 10).await.unwrap();
            if runs[0].trigger == "manual" && runs[0].status == RunStatus::Running {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(running.is_ok(), "the manual sync started");
    assert!(
        uploaded(&g, "slow.txt", Duration::from_secs(15)).await,
        "the manual sync reached the slow upload"
    );
    // That sync is now held on the slow upload, past its scan.
    std::fs::write(d.root.join("during.txt"), "written mid-sync").unwrap();

    assert_eq!(RunStatus::Success, manual.await.unwrap());
    assert!(
        uploaded(&g, "during.txt", Duration::from_secs(15)).await,
        "the change during the sync got a run of its own"
    );
    // Past the settle time, so a second follow-up would have started.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let runs = c.runs(Some("p1"), 20).await.unwrap();
    let triggers: Vec<&str> = runs.iter().map(|r| r.trigger.as_str()).collect();
    assert_eq!(vec!["instant", "manual", "preview"], triggers, "one follow-up");
    let mut uploads = g.uploads.lock().unwrap().clone();
    uploads.sort();
    assert_eq!(vec!["during.txt", "slow.txt"], uploads, "each file went up once");
}

/// Issue #88: the scheduler took every change in the 3 s after its run for the run's own writes.
#[tokio::test(flavor = "multi_thread")]
async fn a_change_right_after_a_run_ends_still_runs_the_pair() {
    let g = google().await;
    let d = start(&g, true).await;
    let c = Client::connect(&d.paths).await.unwrap();
    syncing_on_change(&c, &d).await;

    std::fs::write(d.root.join("slow.txt"), "held").unwrap();
    assert!(
        uploaded(&g, "slow.txt", Duration::from_secs(15)).await,
        "the watcher started a run"
    );
    let ended = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let runs = c.runs(Some("p1"), 10).await.unwrap();
            if runs[0].trigger == "instant" && runs[0].ended_at.is_some() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(ended.is_ok(), "the watcher's run ended");
    std::fs::write(d.root.join("after.txt"), "written as the run ended").unwrap();

    assert!(
        uploaded(&g, "after.txt", Duration::from_secs(15)).await,
        "the change got a run of its own"
    );
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        2,
        instant_runs(&c).await,
        "the watcher's run and one for the change"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ignore_explain_names_the_pair_setting_the_planner_skips_by() {
    let g = google().await;
    let d = start(&g, true).await;
    let c = Client::connect(&d.paths).await.unwrap();
    std::fs::create_dir_all(d.root.join(".claude/skills/handoff")).unwrap();
    std::fs::write(d.root.join(".claude/skills/handoff/SKILL.md"), "skill").unwrap();
    std::fs::write(d.root.join("big.bin"), [0u8; 64]).unwrap();
    std::fs::write(d.root.join("small.txt"), "ok").unwrap();
    let mut p = pair(&d.root);
    assert!(!p.sync_hidden, "hidden files are skipped by default");
    p.excluded_subfolders = vec!["Private".into()];
    p.size_limit = 16;
    c.create_pair(p).await.unwrap();

    let hidden = c
        .ignore_explain("p1", ".claude/skills/handoff/SKILL.md")
        .await
        .unwrap();
    assert!(hidden.ignored);
    assert_eq!(Some("hidden files and folders are skipped".into()), hidden.rule);
    assert_eq!(Some("pair settings".into()), hidden.source);
    let private = c.ignore_explain("p1", "Private/notes.txt").await.unwrap();
    assert_eq!(
        (true, Some("the subfolder Private is left out".into())),
        (private.ignored, private.rule)
    );
    let big = c.ignore_explain("p1", "big.bin").await.unwrap();
    assert_eq!(
        (true, Some("larger than the pair's size limit".into())),
        (big.ignored, big.rule)
    );
    let small = c.ignore_explain("p1", "small.txt").await.unwrap();
    assert!(!small.ignored && small.rule.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn ignore_explain_names_the_rule_and_bad_pairs_are_refused() {
    let g = google().await;
    let d = start(&g, true).await;
    let c = Client::connect(&d.paths).await.unwrap();
    std::fs::write(
        d.root.join(".waslignore"),
        "# build output\nbuild/\n*.log\n!keep.log\n",
    )
    .unwrap();
    std::fs::create_dir_all(d.root.join("build")).unwrap();
    c.create_pair(pair(&d.root)).await.unwrap();

    let e = c.ignore_explain("p1", "build/out.o").await.unwrap();
    assert!(e.ignored);
    assert_eq!(Some("build/".into()), e.rule);
    assert_eq!(Some(".waslignore:2".into()), e.source);
    // As in git, a file inside an ignored folder cannot be re-included; one outside can.
    let log = c.ignore_explain("p1", "debug.log").await.unwrap();
    assert_eq!((true, Some(".waslignore:3".into())), (log.ignored, log.source));
    let keep = c.ignore_explain("p1", "keep.log").await.unwrap();
    assert_eq!((false, Some("!keep.log".into())), (keep.ignored, keep.rule));
    let plain = c.ignore_explain("p1", "notes.txt").await.unwrap();
    assert!(!plain.ignored && plain.rule.is_none());

    let mut relative = pair(&d.root);
    relative.id = "p2".into();
    relative.local_root = "Sync".into();
    let err = c.create_pair(relative).await.unwrap_err();
    assert_eq!(Some(ErrorKind::Invalid), err.kind());
    let mut escape = pair(&d.root);
    escape.id = "../x".into();
    assert_eq!(
        Some(ErrorKind::Invalid),
        c.create_pair(escape).await.unwrap_err().kind()
    );
    assert_eq!(
        Some(ErrorKind::NotFound),
        c.preview("missing").await.unwrap_err().kind()
    );
}

/// E2E 2026-10: `wasl pair add` twice on one folder, or on a folder inside a pair, made two pairs
/// on the same files (the phone refuses that). Both clients now get the same answer.
#[tokio::test(flavor = "multi_thread")]
async fn a_pair_on_another_pairs_files_is_refused() {
    let g = google().await;
    let d = start(&g, true).await;
    let c = Client::connect(&d.paths).await.unwrap();
    c.create_pair(pair(&d.root)).await.unwrap();

    for (id, root) in [
        ("same", d.root.clone()),
        ("inside", d.root.join("sub")),
        ("around", d.root.parent().unwrap().to_path_buf()),
    ] {
        let mut p = pair(&root);
        p.id = id.into();
        let e = c.create_pair(p).await.unwrap_err();
        assert_eq!(Some(ErrorKind::Invalid), e.kind(), "{id}");
    }
    let mut beside = pair(&d.root.with_file_name("beside"));
    beside.id = "beside".into();
    c.create_pair(beside.clone()).await.unwrap();
    beside.local_root = d.root.join("deeper").to_str().unwrap().into();
    assert_eq!(
        Some(ErrorKind::Invalid),
        c.update_pair(beside).await.unwrap_err().kind(),
        "moving a pair onto another's files is refused too"
    );
    let mut same = pair(&d.root);
    same.name = "Renamed".into();
    c.update_pair(same).await.unwrap();
}

/// Security review 2026-10: a pair on the daemon's own folder would upload its database, the IPC
/// token and staged files to Drive, and a download could overwrite them.
#[tokio::test(flavor = "multi_thread")]
async fn a_pair_on_the_daemons_own_folder_is_refused() {
    let g = google().await;
    let d = start(&g, true).await;
    let c = Client::connect(&d.paths).await.unwrap();
    for (id, root) in [
        ("data", d.paths.data.clone()),
        ("inside", d.paths.data.join("bin")),
        ("around", d.paths.data.parent().unwrap().to_path_buf()),
    ] {
        let mut p = pair(&root);
        p.id = id.into();
        let e = c.create_pair(p).await.unwrap_err();
        assert_eq!(Some(ErrorKind::Invalid), e.kind(), "{id}");
        assert!(e.to_string().contains("Wasl keeps its own data"), "{id}: {e}");
    }
    let mut p = pair(&d.root);
    c.create_pair(p.clone()).await.unwrap();
    p.local_root = d.paths.data.join("x").to_str().unwrap().into();
    assert_eq!(
        Some(ErrorKind::Invalid),
        c.update_pair(p).await.unwrap_err().kind(),
        "moving a pair onto the data folder is refused too"
    );
}

/// The person in the confirmation tests: answers each dialog by the changeset's label, and
/// records what each dialog was shown. One for the whole test binary (the confirmer is
/// process-wide); tests tell their changesets apart by label.
struct Person {
    seen: Mutex<Vec<wasld::daemon::ConfirmAsk>>,
}

#[async_trait::async_trait]
impl wasld::daemon::Confirmer for Person {
    async fn ask(
        &self,
        ask: wasld::daemon::ConfirmAsk,
    ) -> Result<Option<wasld::daemon::ConfirmAnswer>, String> {
        self.seen.lock().unwrap().push(ask.clone());
        let label = ask.label.clone().unwrap_or_default();
        if label.contains("slow") {
            tokio::time::sleep(Duration::from_millis(1500)).await;
        }
        let mut answer = wasld::daemon::ConfirmAnswer {
            changeset_id: ask.changeset_id.clone(),
            token: ask.token.clone(),
            accept: true,
        };
        if label.contains("decline") {
            answer.accept = false;
        }
        if label.contains("forge") {
            answer.token = "guessed".into();
        }
        if label.contains("silent") {
            return Ok(None);
        }
        Ok(Some(answer))
    }
}

fn person() -> &'static Person {
    static P: std::sync::OnceLock<Arc<Person>> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        let p = Arc::new(Person {
            seen: Mutex::new(Vec::new()),
        });
        wasld::daemon::use_confirmer(p.clone());
        p
    })
}

/// A held changeset whose label tells [`Person`] how to answer.
async fn held(c: &Client, d: &Running, label: &str) -> wasl_store::Changeset {
    c.create_pair(pair(&d.root)).await.unwrap();
    let root = d.root.to_str().unwrap();
    let cs = c
        .agent_begin(root, "claude-code", Some(label), None)
        .await
        .unwrap();
    std::fs::write(d.root.join("new.txt"), "by the agent").unwrap();
    c.agent_end(Some(&cs.id), None, None)
        .await
        .unwrap()
        .expect("the agent changed something")
}

async fn status_after(c: &Client, id: &str, wait: Duration) -> wasl_store::ChangesetStatus {
    let deadline = std::time::Instant::now() + wait;
    loop {
        let s = c.agent_changeset(id).await.unwrap().changeset.status;
        if s != wasl_store::ChangesetStatus::Held || std::time::Instant::now() >= deadline {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Security review 2026-10: the daemon itself refuses an accept from a client that found an agent
/// marker, and records which client accepted.
#[tokio::test(flavor = "multi_thread")]
async fn an_accept_from_inside_an_agent_is_refused_by_the_daemon() {
    person();
    let g = google().await;
    let d = start(&g, true).await;
    let c = Client::connect(&d.paths).await.unwrap();
    let held = held(&c, &d, "marker").await;

    let e = c
        .agent_accept_as(&held.id, Some("cli"), Some("CLAUDECODE"))
        .await
        .unwrap_err();
    assert_eq!(Some(ErrorKind::Permission), e.kind(), "{e}");
    assert!(e.to_string().contains("CLAUDECODE"), "{e}");
    let still = c.agent_changeset(&held.id).await.unwrap();
    assert_eq!(wasl_store::ChangesetStatus::Held, still.changeset.status);
    assert_eq!(None, still.decided_by);
    assert!(
        !person()
            .seen
            .lock()
            .unwrap()
            .iter()
            .any(|a| a.changeset_id == held.id),
        "no dialog for a refused request"
    );

    let asked = c.agent_accept_as(&held.id, Some("cli"), None).await.unwrap();
    assert_eq!(
        wasl_store::ChangesetStatus::Accepted,
        status_after(&c, &held.id, Duration::from_secs(10)).await
    );
    let done = c.agent_changeset(&held.id).await.unwrap();
    assert_eq!(Some("cli".to_owned()), done.decided_by);
    let seen = person().seen.lock().unwrap().clone();
    let shown = seen.iter().find(|a| a.changeset_id == held.id).unwrap();
    assert_eq!(asked.code, shown.code, "the dialog shows the code the client got");
    assert_eq!("claude-code", shown.agent);
    assert_eq!(1, shown.items);
    assert_eq!("cli", shown.asked_by);
}

/// Security review S3-1: a request alone never accepts. The changeset waits until the dialog the
/// daemon started answers with the token it was given; a declined, forged or missing answer
/// leaves it held, and the token never travels over the IPC.
#[tokio::test(flavor = "multi_thread")]
async fn an_accept_needs_the_persons_confirmation_in_the_app() {
    person();
    let g = google().await;
    for label in ["decline", "forge", "silent"] {
        let d = start(&g, true).await;
        let c = Client::connect(&d.paths).await.unwrap();
        let held = held(&c, &d, label).await;
        let asked = c.agent_accept_as(&held.id, None, None).await.unwrap();
        assert_eq!(6, asked.code.len());
        assert!(asked.expires_in_secs > 0);
        // The client's answer carries the code, never the token.
        let token = person()
            .seen
            .lock()
            .unwrap()
            .iter()
            .find(|a| a.changeset_id == held.id)
            .map(|a| a.token.clone());
        if let Some(token) = token {
            assert!(!serde_json::to_string(&asked).unwrap().contains(&token));
        }
        assert_eq!(
            wasl_store::ChangesetStatus::Held,
            status_after(&c, &held.id, Duration::from_millis(800)).await,
            "{label}"
        );
        // Used up, and declined or unanswered: asking again at once opens no dialog and says how
        // long to wait, so an agent asking in a loop cannot keep a dialog on screen.
        let dialogs = || {
            person()
                .seen
                .lock()
                .unwrap()
                .iter()
                .filter(|a| a.changeset_id == held.id)
                .count()
        };
        let shown = dialogs();
        let again = c.agent_accept_as(&held.id, None, None).await.unwrap();
        assert!(
            again
                .retry_in_secs
                .is_some_and(|s| s > 50 && s <= wasld::daemon::COOLDOWN_FIRST.as_secs()),
            "{label}: {again:?}"
        );
        assert!(again.code.is_empty(), "{label}");
        assert_eq!(shown, dialogs(), "{label}: no new dialog");
        // Revert stays open to anyone: it only puts earlier versions back.
        tokio::time::sleep(Duration::from_millis(200)).await;
        c.agent_revert(&held.id).await.unwrap();
        assert!(!d.root.join("new.txt").exists(), "{label}");
    }
}

/// Security review S3-1: one dialog at a time. Asking again for the same changeset answers with
/// the code already shown; another changeset waits its turn.
#[tokio::test(flavor = "multi_thread")]
async fn one_confirmation_at_a_time() {
    person();
    let g = google().await;
    let d = start(&g, true).await;
    let c = Client::connect(&d.paths).await.unwrap();
    let first = held(&c, &d, "slow first").await;
    let asked = c.agent_accept_as(&first.id, Some("desktop"), None).await.unwrap();
    let twice = c.agent_accept_as(&first.id, Some("desktop"), None).await.unwrap();
    assert_eq!(asked.code, twice.code);
    // A second changeset, in a subfolder of the same pair.
    let sub = d.root.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let cs = c
        .agent_begin(sub.to_str().unwrap(), "gemini", Some("second"), None)
        .await
        .unwrap();
    std::fs::write(sub.join("b.txt"), "b").unwrap();
    let second = c.agent_end(Some(&cs.id), None, None).await.unwrap().unwrap();
    let e = c.agent_accept_as(&second.id, None, None).await.unwrap_err();
    assert_eq!(Some(ErrorKind::Busy), e.kind(), "{e}");
    assert_eq!(
        1,
        person()
            .seen
            .lock()
            .unwrap()
            .iter()
            .filter(|a| a.changeset_id == first.id)
            .count(),
        "one dialog for the first"
    );
    assert_eq!(
        wasl_store::ChangesetStatus::Accepted,
        status_after(&c, &first.id, Duration::from_secs(10)).await
    );
    // Its turn now.
    c.agent_accept_as(&second.id, None, None).await.unwrap();
    assert_eq!(
        wasl_store::ChangesetStatus::Accepted,
        status_after(&c, &second.id, Duration::from_secs(10)).await
    );
}

/// Found on real Drive: a pair added from the CLI was synced by the schedule before anyone saw
/// its plan. Every new pair now waits for its preview, whichever client created it.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_pair_waits_for_its_preview_before_anything_moves() {
    let g = google().await;
    let d = start(&g, true).await;
    std::fs::write(d.root.join("local.txt"), "here").unwrap();
    let c = Client::connect(&d.paths).await.unwrap();
    let mut p = pair(&d.root);
    p.instant_sync = true;
    p.interval_minutes = 1;
    p.status.hold_for_preview = false; // the client did not ask for the hold
    c.create_pair(p).await.unwrap();

    std::fs::write(d.root.join("later.txt"), "changed").unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await; // many ticks, past the settle time
    assert!(
        c.runs(Some("p1"), 10).await.unwrap().is_empty(),
        "nothing ran by itself"
    );
    assert!(g.uploads.lock().unwrap().is_empty());
    let st = c.status().await.unwrap();
    assert!(st.pairs[0].pair.status.hold_for_preview);
    assert_eq!(Overall::Waiting, st.overall);

    // A manual sync is refused the same way; the preview's start runs it.
    let manual = c.sync("p1", Trigger::Manual).await.unwrap();
    assert_eq!(RunStatus::Skipped, manual.status);
    assert_eq!(Some(wasl_engine::HOLD_MESSAGE.to_string()), manual.message);
    assert_eq!(
        RunStatus::Success,
        c.sync("p1", Trigger::Preview).await.unwrap().status
    );
    assert!(!c.pair("p1").await.unwrap().unwrap().status.hold_for_preview);
}

/// #94: a pair pointed at another folder started from the old folder's baseline, so its next run
/// read every old file as deleted.
#[tokio::test(flavor = "multi_thread")]
async fn changing_a_pairs_local_folder_holds_it_and_its_first_run_is_a_preview_not_deletions() {
    let g = google().await;
    let d = start(&g, true).await;
    std::fs::write(d.root.join("local.txt"), "from this computer").unwrap();
    let c = Client::connect(&d.paths).await.unwrap();
    c.create_pair(pair(&d.root)).await.unwrap();
    assert_eq!(
        RunStatus::Success,
        c.sync("p1", Trigger::Preview).await.unwrap().status
    );

    // A rename keeps the state: no hold, the same baseline.
    let synced = c.status().await.unwrap().pairs[0].folder_bytes;
    assert!(synced.is_some());
    let mut renamed = c.pair("p1").await.unwrap().unwrap();
    renamed.name = "Renamed".into();
    c.update_pair(renamed).await.unwrap();
    let st = c.status().await.unwrap();
    assert!(
        !st.pairs[0].pair.status.hold_for_preview,
        "a rename keeps its state"
    );
    assert_eq!(synced, st.pairs[0].folder_bytes);

    // Another folder: held, and its preview deletes nothing.
    let other = d.root.parent().unwrap().join("Other");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("other.txt"), "new folder").unwrap();
    let mut moved = c.pair("p1").await.unwrap().unwrap();
    moved.local_root = other.to_str().unwrap().into();
    c.update_pair(moved).await.unwrap();
    let st = c.status().await.unwrap();
    assert!(st.pairs[0].pair.status.hold_for_preview);
    assert_eq!(None, st.pairs[0].folder_bytes, "the old baseline is gone");
    let manual = c.sync("p1", Trigger::Manual).await.unwrap();
    assert_eq!(Some(wasl_engine::HOLD_MESSAGE.to_string()), manual.message);
    let preview = c.preview("p1").await.unwrap().plan.summary;
    assert_eq!(
        (0, 0),
        (preview.delete_local, preview.delete_remote),
        "{preview:?}"
    );
    assert_eq!(1, preview.uploads, "{preview:?}");
    assert!(d.root.join("local.txt").exists(), "the old folder is left alone");
}

/// #91: what the desktop's tables and sidebar show, from the last run and the system.
#[tokio::test(flavor = "multi_thread")]
async fn the_status_carries_folder_size_skipped_count_peers_and_free_space() {
    let g = google().await;
    let d = start(&g, true).await;
    std::fs::write(d.root.join("local.txt"), "from this computer").unwrap();
    let c = Client::connect(&d.paths).await.unwrap();
    c.create_pair(pair(&d.root)).await.unwrap();
    let before = c.status().await.unwrap();
    assert_eq!(
        (None, None),
        (before.pairs[0].folder_bytes, before.pairs[0].skipped)
    );
    assert_eq!(
        RunStatus::Success,
        c.sync("p1", Trigger::Preview).await.unwrap().status
    );

    let st = &c.status().await.unwrap().pairs[0];
    let both = ("from this computer".len() + "from drive".len()) as u64;
    assert_eq!(
        Some(both),
        st.folder_bytes,
        "both files, as the baseline has them"
    );
    assert_eq!(Some(0), st.skipped);
    assert!(st.peers.is_empty(), "a Drive pair syncs with no device");
    assert!(st.free_bytes.is_some_and(|b| b > 0));
}

#[tokio::test(flavor = "multi_thread")]
async fn views_work_through_the_daemon() {
    let g = google().await;
    let backend = Arc::new(wasld::views::MemoryBackend::default());
    let d = start_with(&g, true, Some(backend.clone())).await;
    let c = Client::connect(&d.paths).await.unwrap();
    let local = d.root.parent().unwrap().join("Drive view");
    let view = c
        .add_view(wasl_ipc::NewView {
            name: "Sync on demand".into(),
            account_id: ME.into(),
            drive_folder_id: "A".into(),
            drive_id: None,
            remote_path: "My Drive/Sync".into(),
            local_path: local.display().to_string(),
            cache_limit: None,
        })
        .await
        .unwrap();
    assert_eq!(None, view.last_error);
    assert_eq!(wasld::views::DEFAULT_CACHE_LIMIT, view.cache_limit);
    let info = c.views().await.unwrap();
    assert!(info.supported);
    let st = &info.views[0];
    assert!(st.mounted);
    assert_eq!((1, "from drive".len() as u64), (st.files, st.bytes));
    assert_eq!(
        Some(vec!["remote.txt".to_string()]),
        backend.shown.lock().unwrap().get(&view.id).cloned()
    );

    let file = local.join("remote.txt").display().to_string();
    c.pin_path(&file).await.unwrap();
    assert_eq!(10, c.views().await.unwrap().views[0].pinned_bytes);
    c.unpin_path(&file).await.unwrap();
    assert_eq!(0, c.views().await.unwrap().views[0].pinned_bytes);
    c.refresh_view(&view.id).await.unwrap();
    c.set_view_cache_limit(&view.id, 1 << 20).await.unwrap();
    c.set_view_enabled(&view.id, false).await.unwrap();
    assert!(!c.views().await.unwrap().views[0].mounted);
    c.set_view_enabled(&view.id, true).await.unwrap();
    assert!(c.views().await.unwrap().views[0].mounted);

    // A view is read-only: no pair may sync in or around it.
    let e = c.create_pair(pair(&local.join("inside"))).await.unwrap_err();
    assert_eq!(Some(ErrorKind::Invalid), e.kind());
    assert!(g.uploads.lock().unwrap().is_empty(), "a view never uploads");

    c.remove_view(&view.id, false).await.unwrap();
    assert!(c.views().await.unwrap().views.is_empty());
    let e = c.pin_path(&file).await.unwrap_err();
    assert_eq!(Some(ErrorKind::NotFound), e.kind());
    c.shutdown().await.unwrap();
    assert!(d.handle.await.unwrap().is_ok());
}

/// A vault v2 at the cheapest cost readers accept, as a computer that saw it records it.
fn known_vault(passphrase: &str) -> wasl_store::VaultRecord {
    use wasl_crypto::v2::{KdfParams, VaultKeysV2, keys, vault_json::VaultManifestV2};
    let salt = [9u8; 16];
    let ikm = keys::argon2id(passphrase, &salt, KdfParams::MIN).unwrap();
    let manifest = VaultManifestV2::create(
        "Private",
        &salt,
        KdfParams::MIN,
        &VaultKeysV2::from_ikm(&ikm).key_check(),
        1_760_000_000_000,
    );
    wasl_store::VaultRecord {
        id: "vault-1".into(),
        name: "Private".into(),
        format_version: 2,
        kdf_params_json: "{}".into(),
        key_check: manifest.key_check_bytes().unwrap().to_vec(),
        manifest_json: manifest.to_json(),
        account_id: Some(ME.into()),
        drive_folder_id: Some("V".into()),
        drive_id: None,
        remote_path: "My Drive/Private".into(),
        created_at: 1_760_000_000_000,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn vaults_unlock_and_lock_through_the_daemon() {
    let g = google().await;
    let d = start(&g, true).await;
    wasl_store::Store::open(d.paths.data.join("wasl.db"))
        .unwrap()
        .put_vault(&known_vault("correct horse battery"))
        .unwrap();
    let c = Client::connect(&d.paths).await.unwrap();
    let listed = c.vaults().await.unwrap();
    assert_eq!(1, listed.len());
    assert_eq!(
        ("Private", 2, false),
        (listed[0].name.as_str(), listed[0].version, listed[0].unlocked)
    );

    let wrong = c
        .unlock_vault("vault-1", wasl_ipc::Passphrase::new("not it"))
        .await
        .unwrap_err();
    assert_eq!(Some(ErrorKind::Invalid), wrong.kind());
    assert!(wrong.to_string().contains("Wrong passphrase"), "{wrong}");
    assert_eq!(
        None,
        d.vault_keys.get("vault-1"),
        "nothing kept for a wrong passphrase"
    );

    let open = c
        .unlock_vault("vault-1", wasl_ipc::Passphrase::new("correct horse battery"))
        .await
        .unwrap();
    assert!(open.unlocked);
    let kept = d.vault_keys.get("vault-1").unwrap();
    assert!(
        kept.starts_with("2:") && kept.len() == 2 + 64,
        "the 32-byte Argon2id output, as hex"
    );
    assert!(!kept.contains("correct horse"), "never the passphrase");
    assert!(c.vaults().await.unwrap()[0].unlocked);

    c.lock_vault("vault-1").await.unwrap();
    assert_eq!(None, d.vault_keys.get("vault-1"));
    assert!(!c.vaults().await.unwrap()[0].unlocked);
    let e = c.lock_vault("nope").await.unwrap_err();
    assert_eq!(Some(ErrorKind::NotFound), e.kind());

    // A pair can't name a vault this computer doesn't know.
    let mut p = pair(&d.root);
    p.vault_id = Some("nope".into());
    assert_eq!(
        Some(ErrorKind::NotFound),
        c.create_pair(p).await.unwrap_err().kind()
    );
    // A Drive folder that can't be read (here: gone) can't be checked for a vault: refused, not
    // created as a plain pair. The same when an existing pair moves to it.
    let mut p = pair(&d.root);
    p.drive_folder_id = Some("Z".into());
    assert!(c.create_pair(p).await.is_err());
    assert!(c.pair("p1").await.unwrap().is_none());
    c.create_pair(pair(&d.root)).await.unwrap();
    let mut moved = c.pair("p1").await.unwrap().unwrap();
    moved.drive_folder_id = Some("Z".into());
    assert!(c.update_pair(moved).await.is_err());
    assert_eq!(
        Some("A"),
        c.pair("p1").await.unwrap().unwrap().drive_folder_id.as_deref()
    );
    c.shutdown().await.unwrap();
    assert!(d.handle.await.unwrap().is_ok());
}

/// ADR-0002 Read format through the daemon (#48): an agent's held work is in the op-log under
/// this device, listed with its op count and shown with its operations.
#[tokio::test(flavor = "multi_thread")]
async fn the_op_log_is_read_through_the_daemon() {
    let g = google().await;
    let devices = wasl_devices::DeviceConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        discover: false,
    };
    let d = start_full(&g, true, None, Some(devices)).await;
    let c = Client::connect(&d.paths).await.unwrap();
    let held = held(&c, &d, "op-log").await;

    let page = c.oplog_list(&wasl_store::OplogFilter::default()).await.unwrap();
    assert_eq!(wasl_engine::OPLOG_FORMAT_VERSION, page.version);
    let cs = page
        .changesets
        .iter()
        .find(|cs| cs.id == held.id)
        .expect("listed");
    assert_eq!("agent:claude-code", cs.actor);
    assert_eq!(wasl_store::OplogStatus::Staged, cs.status);
    assert_eq!(Some(1), cs.op_count);
    assert!(cs.origin_device.starts_with("WASL-"), "{}", cs.origin_device);
    let me = c.devices().await.unwrap().devices;
    let me = me.iter().find(|i| i.this_device).expect("this device is listed");
    assert_eq!(me.id, cs.origin_device);
    assert_eq!(
        Some(&me.name),
        cs.origin_device_name.as_ref(),
        "named from the device list"
    );
    assert_eq!(None, cs.operations);
    let other = wasl_store::OplogFilter {
        actor: Some("agent:gemini".into()),
        ..Default::default()
    };
    assert!(c.oplog_list(&other).await.unwrap().changesets.is_empty());
    // Each filter reaches the store through the IPC: a match keeps the changeset, a miss drops it.
    let listed = |f: wasl_store::OplogFilter| {
        let c = &c;
        let id = held.id.clone();
        async move {
            c.oplog_list(&f)
                .await
                .unwrap()
                .changesets
                .iter()
                .any(|cs| cs.id == id)
        }
    };
    let status = |s| wasl_store::OplogFilter {
        status: Some(s),
        ..Default::default()
    };
    assert!(listed(status(wasl_store::OplogStatus::Staged)).await);
    assert!(!listed(status(wasl_store::OplogStatus::Committed)).await);
    let since = |t| wasl_store::OplogFilter {
        since: Some(t),
        ..Default::default()
    };
    assert!(listed(since(held.started_at - 60_000)).await);
    assert!(!listed(since(held.started_at + 3_600_000)).await);
    let pair = |p: &str| wasl_store::OplogFilter {
        pair: Some(p.into()),
        ..Default::default()
    };
    assert!(listed(pair(held.pair().unwrap())).await);
    assert!(!listed(pair("ffff0000-0000-4000-8000-000000000000")).await);
    let limit = |n| wasl_store::OplogFilter {
        limit: Some(n),
        ..Default::default()
    };
    assert!(listed(limit(1)).await, "the newest");
    assert!(!listed(limit(0)).await);

    let shown = c.oplog_show(None, &held.id).await.unwrap();
    let ops = shown.changesets[0].operations.as_ref().unwrap();
    assert_eq!(1, ops.len());
    assert_eq!("create", ops[0].op);
    assert_eq!(
        (&cs.origin_device, &held.id),
        (&ops[0].origin_device, &ops[0].changeset_id)
    );
    assert_eq!(cs.origin_device_name, shown.changesets[0].origin_device_name);
    assert!(ops[0].entity.ends_with("/new.txt"), "{}", ops[0].entity);
    let same = c.oplog_show(Some(&cs.origin_device), &held.id).await.unwrap();
    assert_eq!(shown, same, "a bare id is this device");
    let e = c.oplog_show(Some("WASL-0000"), &held.id).await.unwrap_err();
    assert_eq!(Some(ErrorKind::NotFound), e.kind(), "{e}");
    assert_eq!(
        wasl_store::ChangesetStatus::Held,
        c.agent_changeset(&held.id).await.unwrap().changeset.status,
        "reading decides nothing"
    );
}

/// Security review M1 (#134): an agent holding the IPC token can't get its held changes out by
/// removing its pair and making it again on the same folders. Deleting the pair, or pointing it at
/// other folders, is refused while a changeset is open or held; once the person reverts (or
/// accepts) it, the pair can go.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_changeset_survives_deleting_and_recreating_its_pair() {
    person();
    let g = google().await;
    let d = start(&g, true).await;
    let c = Client::connect(&d.paths).await.unwrap();
    let held = held(&c, &d, "bypass").await;

    // The agent's attempt: remove the pair, make it again, approve its preview, sync.
    let refused = c.delete_pair("p1").await;
    let _ = c.create_pair(pair(&d.root)).await;
    let _ = c.approve_once("p1").await;
    let _ = c.sync("p1", Trigger::Preview).await;
    assert!(
        !uploaded(&g, "new.txt", Duration::from_millis(500)).await,
        "the agent's held file reached Drive without the person's confirmation"
    );
    let e = refused.expect_err("deleting a pair with a held changeset is refused");
    assert_eq!(Some(ErrorKind::Invalid), e.kind(), "{e}");
    assert_eq!(
        wasl_store::ChangesetStatus::Held,
        c.agent_changeset(&held.id).await.unwrap().changeset.status
    );

    // Pointing the pair at another Drive folder is refused the same way.
    let mut moved = pair(&d.root);
    moved.drive_folder_id = Some("B".into());
    let e = c.update_pair(moved).await.expect_err("re-pointing is refused");
    assert_eq!(Some(ErrorKind::Invalid), e.kind(), "{e}");

    // Once the person settles the changeset, the pair can be removed.
    c.agent_revert(&held.id).await.unwrap();
    c.delete_pair("p1").await.unwrap();
}
