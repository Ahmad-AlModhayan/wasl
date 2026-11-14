//! The engine over the real Drive client and node cache, against a mocked Drive REST API: the crawl,
//! a verified download, a multipart upload and a trash, with the cache and sessions in SQLite.

use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use wasl_core::RunStatus;
use wasl_core::conditions::{Conditions, DeviceState};
use wasl_drive::{DriveClient, DriveClientConfig, DriveEndpoints, DriveError, DriveNodeCache, TokenProvider};
use wasl_engine::{DriveRemote, Engine, EngineConfig, Environment, Remote, SystemClock, Trigger};
use wasl_fs::{LocalFs, StdFs};
use wasl_store::{Pair, SharedStore, Store};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

struct Tokens;

#[async_trait]
impl TokenProvider for Tokens {
    async fn access_token(&self, _account: &str) -> Result<String, DriveError> {
        Ok("tok".into())
    }
    async fn invalidate(&self, _account: &str) {}
}

struct Env {
    root: std::path::PathBuf,
    bin: std::path::PathBuf,
    remote: Arc<DriveRemote>,
}

impl Environment for Env {
    fn local_fs(&self, _p: &Pair) -> wasl_engine::Result<Arc<dyn LocalFs>> {
        Ok(Arc::new(StdFs::new(&self.root, &self.bin)))
    }
    fn remote(&self, _p: &Pair) -> wasl_engine::Result<Arc<dyn Remote>> {
        Ok(self.remote.clone())
    }
    fn device_state(&self) -> DeviceState {
        DeviceState::unrestricted()
    }
}

fn md5(s: &str) -> String {
    wasl_fs::hash_reader(s.as_bytes()).unwrap().md5
}

fn json(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.into_bytes(), "application/json; charset=utf-8")
}

const ACCOUNT: &str = "me@example.com";

#[tokio::test]
async fn a_pair_syncs_through_the_drive_api() {
    let server = MockServer::builder().start().await;
    let remote_content = "hello from drive";
    let remote_md5 = md5(remote_content);
    let local_content = "hello from the device";
    let local_md5 = md5(local_content);

    // The crawl: the root, the changes token taken first, one page of files, no changes since.
    Mock::given(method("GET"))
        .and(path("/drive/v3/files/root"))
        .respond_with(json(
            r#"{"id":"ROOT","name":"My Drive","mimeType":"application/vnd.google-apps.folder"}"#.into(),
        ))
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
        .respond_with(json(r#"{"startPageToken":"100"}"#.into()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/drive/v3/files"))
        .respond_with(json(format!(
            r#"{{"files":[
                {{"id":"A","name":"Sync","mimeType":"application/vnd.google-apps.folder","parents":["ROOT"],"modifiedTime":"2026-01-01T00:00:00.000Z"}},
                {{"id":"B","name":"remote.txt","mimeType":"text/plain","parents":["A"],"size":"{}","md5Checksum":"{remote_md5}","modifiedTime":"2026-01-02T03:04:05.000Z","version":"3"}}
            ]}}"#,
            remote_content.len()
        )))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/drive/v3/changes"))
        .respond_with(json(r#"{"newStartPageToken":"100","changes":[]}"#.into()))
        .mount(&server)
        .await;
    // The download.
    Mock::given(method("GET"))
        .and(path("/drive/v3/files/B"))
        .and(query_param("alt", "media"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(remote_content.as_bytes().to_vec()))
        .mount(&server)
        .await;
    // The upload answers with the MD5 of what the device sent.
    let uploaded: Arc<Mutex<Vec<String>>> = Arc::default();
    {
        let uploaded = uploaded.clone();
        let local_md5 = local_md5.clone();
        Mock::given(method("POST"))
            .and(path("/upload/drive/v3/files"))
            .and(query_param("uploadType", "multipart"))
            .respond_with(move |req: &Request| {
                uploaded.lock().unwrap().push(String::from_utf8_lossy(&req.body).into_owned());
                json(format!(
                    r#"{{"id":"C","name":"local.txt","mimeType":"text/plain","parents":["A"],"size":"{}","md5Checksum":"{local_md5}","modifiedTime":"2026-01-03T00:00:00.000Z","version":"1"}}"#,
                    local_content.len()
                ))
            })
            .mount(&server)
            .await;
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("pair");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("local.txt"), local_content).unwrap();

    let store = SharedStore::new(Store::open(dir.path().join("wasl.db")).unwrap());
    let http = DriveClient::http_client_builder().build().unwrap();
    let client = Arc::new(DriveClient::with_http(
        http,
        Arc::new(Tokens),
        DriveClientConfig {
            endpoints: DriveEndpoints {
                base: format!("{}/drive/v3", server.uri()),
                upload: format!("{}/upload/drive/v3", server.uri()),
            },
            ..DriveClientConfig::default()
        },
    ));
    let cache = Arc::new(DriveNodeCache::new(client.clone(), Arc::new(store.clone())));
    let remote = Arc::new(DriveRemote {
        account: ACCOUNT.into(),
        client,
        cache,
        sessions: Arc::new(store.clone()),
        flagged: Default::default(),
    });
    let mut pair = Pair::new("p", "Sync", root.to_str().unwrap());
    pair.account_id = Some(ACCOUNT.into());
    pair.drive_folder_id = Some("A".into());
    pair.conditions = Conditions::none();
    store.lock().create_pair(&pair).unwrap();
    let env = Arc::new(Env {
        root: root.clone(),
        bin: dir.path().join("bin"),
        remote,
    });
    let engine = Engine::new(
        store.clone(),
        env,
        Arc::new(SystemClock),
        EngineConfig::default(),
        None,
    );

    // Both sides have a file: the first sync waits for the preview; starting from it syncs both ways.
    assert_eq!(RunStatus::Blocked, engine.sync("p", Trigger::Manual).await.status);
    let run = engine.sync("p", Trigger::Preview).await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    assert_eq!(
        remote_content,
        std::fs::read_to_string(root.join("remote.txt")).unwrap()
    );
    let bodies = uploaded.lock().unwrap();
    assert_eq!(1, bodies.len());
    assert!(
        bodies[0].contains(local_content) && bodies[0].contains(r#""parents":["A"]"#),
        "{}",
        bodies[0]
    );

    // The Drive cache and the baseline are in SQLite now.
    let s = store.lock();
    assert!(
        s.drive_nodes(ACCOUNT).unwrap().iter().any(|n| n.id == "C"),
        "the upload was remembered"
    );
    let base = s.load_baseline("p").unwrap();
    assert_eq!(Some(local_md5), base["local.txt"].remote_md5);
    assert_eq!(Some(remote_md5), base["remote.txt"].local_hash);
}

#[tokio::test]
async fn a_download_that_does_not_match_drives_md5_is_rejected() {
    let server = MockServer::builder().start().await;
    Mock::given(method("GET"))
        .and(path("/drive/v3/files/root"))
        .respond_with(json(
            r#"{"id":"ROOT","name":"My Drive","mimeType":"application/vnd.google-apps.folder"}"#.into(),
        ))
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
        .respond_with(json(r#"{"startPageToken":"1"}"#.into()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/drive/v3/files"))
        .respond_with(json(r#"{"files":[
            {"id":"A","name":"Sync","mimeType":"application/vnd.google-apps.folder","parents":["ROOT"]},
            {"id":"B","name":"x.bin","mimeType":"application/octet-stream","parents":["A"],"size":"4","md5Checksum":"00000000000000000000000000000000","version":"1"}
        ]}"#.into()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/drive/v3/changes"))
        .respond_with(json(r#"{"newStartPageToken":"1","changes":[]}"#.into()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/drive/v3/files/B"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"evil".to_vec()))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("pair");
    std::fs::create_dir_all(&root).unwrap();
    let store = SharedStore::new(Store::open_in_memory().unwrap());
    let client = Arc::new(DriveClient::with_http(
        DriveClient::http_client_builder().build().unwrap(),
        Arc::new(Tokens),
        DriveClientConfig {
            endpoints: DriveEndpoints {
                base: format!("{}/drive/v3", server.uri()),
                upload: format!("{}/upload/drive/v3", server.uri()),
            },
            ..DriveClientConfig::default()
        },
    ));
    let cache = Arc::new(DriveNodeCache::new(client.clone(), Arc::new(store.clone())));
    let remote = Arc::new(DriveRemote {
        account: ACCOUNT.into(),
        client,
        cache,
        sessions: Arc::new(store.clone()),
        flagged: Default::default(),
    });
    let mut pair = Pair::new("p", "Sync", root.to_str().unwrap());
    pair.account_id = Some(ACCOUNT.into());
    pair.drive_folder_id = Some("A".into());
    pair.conditions = Conditions::none();
    store.lock().create_pair(&pair).unwrap();
    let env = Arc::new(Env {
        root: root.clone(),
        bin: dir.path().join("bin"),
        remote,
    });
    let engine = Engine::new(
        store.clone(),
        env,
        Arc::new(SystemClock),
        EngineConfig::default(),
        None,
    );

    let run = engine.sync("p", Trigger::Manual).await;
    assert_eq!(RunStatus::Partial, run.status, "{:?}", run.message);
    assert!(
        !root.join("x.bin").exists(),
        "unverified content never lands in the folder"
    );
    assert!(
        std::fs::read_dir(&root).unwrap().next().is_none(),
        "no part file left behind"
    );
    assert!(
        store.lock().load_baseline("p").unwrap().is_empty(),
        "nothing recorded as synced"
    );
}

/// Drive's scan flags ordinary files as malware (a vite.config.ts, 2026-10) and refuses them unless
/// the request acknowledges it. The user's own file syncs, and the log says what happened.
#[tokio::test]
async fn a_file_drive_flags_as_malware_syncs_with_a_note() {
    let server = MockServer::builder().start().await;
    let content = "export default {}";
    let content_md5 = md5(content);
    Mock::given(method("GET"))
        .and(path("/drive/v3/files/root"))
        .respond_with(json(
            r#"{"id":"ROOT","name":"My Drive","mimeType":"application/vnd.google-apps.folder"}"#.into(),
        ))
        .mount(&server)
        .await;
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
        .respond_with(json(r#"{"startPageToken":"1"}"#.into()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/drive/v3/files"))
        .respond_with(json(format!(
            r#"{{"files":[
            {{"id":"A","name":"Sync","mimeType":"application/vnd.google-apps.folder","parents":["ROOT"]}},
            {{"id":"B","name":"vite.config.ts","mimeType":"text/plain","parents":["A"],"size":"{}","md5Checksum":"{content_md5}","version":"1"}}
        ]}}"#,
            content.len()
        )))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/drive/v3/changes"))
        .respond_with(json(r#"{"newStartPageToken":"1","changes":[]}"#.into()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/drive/v3/files/B"))
        .and(query_param("acknowledgeAbuse", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(content.as_bytes().to_vec()))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/drive/v3/files/B"))
        .respond_with(
            ResponseTemplate::new(403).set_body_raw(
                r#"{"error":{"code":403,"message":"This file has been identified as malware or spam and cannot be downloaded.","errors":[{"reason":"cannotDownloadAbusiveFile"}]}}"#,
                "application/json",
            ),
        )
        .with_priority(2)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("pair");
    std::fs::create_dir_all(&root).unwrap();
    let store = SharedStore::new(Store::open_in_memory().unwrap());
    let client = Arc::new(DriveClient::with_http(
        DriveClient::http_client_builder().build().unwrap(),
        Arc::new(Tokens),
        DriveClientConfig {
            endpoints: DriveEndpoints {
                base: format!("{}/drive/v3", server.uri()),
                upload: format!("{}/upload/drive/v3", server.uri()),
            },
            ..DriveClientConfig::default()
        },
    ));
    let cache = Arc::new(DriveNodeCache::new(client.clone(), Arc::new(store.clone())));
    let remote = Arc::new(DriveRemote {
        account: ACCOUNT.into(),
        client,
        cache,
        sessions: Arc::new(store.clone()),
        flagged: Default::default(),
    });
    let mut pair = Pair::new("p", "Sync", root.to_str().unwrap());
    pair.account_id = Some(ACCOUNT.into());
    pair.drive_folder_id = Some("A".into());
    pair.conditions = Conditions::none();
    store.lock().create_pair(&pair).unwrap();
    let env = Arc::new(Env {
        root: root.clone(),
        bin: dir.path().join("bin"),
        remote,
    });
    let engine = Engine::new(
        store.clone(),
        env,
        Arc::new(SystemClock),
        EngineConfig::default(),
        None,
    );

    let run = engine.sync("p", Trigger::Manual).await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    assert_eq!(
        content,
        std::fs::read_to_string(root.join("vite.config.ts")).unwrap()
    );
    let events = store.lock().run_events(&run.run_id).unwrap();
    assert!(
        events.iter().any(|e| e.level == wasl_store::EventLevel::Warn
            && e.key.as_deref() == Some("vite.config.ts")
            && e.message.contains("flagged this file as malware")),
        "{events:?}"
    );
}

/// "Delete forever" removes a file from Drive only while it is in Drive's bin: a bin row that names
/// a live file (an edited import, a file restored in Drive since) never deletes it.
#[tokio::test]
async fn a_permanent_delete_spares_a_file_that_is_not_in_drives_bin() {
    let server = MockServer::builder().start().await;
    for (id, trashed) in [("live", false), ("binned", true)] {
        Mock::given(method("GET"))
            .and(path(format!("/drive/v3/files/{id}")))
            .respond_with(json(format!(
                r#"{{"id":"{id}","name":"{id}.txt","mimeType":"text/plain","trashed":{trashed}}}"#
            )))
            .mount(&server)
            .await;
    }
    let deleted = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = deleted.clone();
    Mock::given(method("DELETE"))
        .respond_with(move |req: &Request| {
            seen.lock().unwrap().push(req.url.path().to_owned());
            ResponseTemplate::new(204)
        })
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let store = SharedStore::new(Store::open(dir.path().join("wasl.db")).unwrap());
    let client = Arc::new(DriveClient::with_http(
        DriveClient::http_client_builder().build().unwrap(),
        Arc::new(Tokens),
        DriveClientConfig {
            endpoints: DriveEndpoints {
                base: format!("{}/drive/v3", server.uri()),
                upload: format!("{}/upload/drive/v3", server.uri()),
            },
            ..DriveClientConfig::default()
        },
    ));
    let remote = DriveRemote {
        account: ACCOUNT.into(),
        client: client.clone(),
        cache: Arc::new(DriveNodeCache::new(client, Arc::new(store.clone()))),
        sessions: Arc::new(store.clone()),
        flagged: Default::default(),
    };

    let refused = remote.delete_permanently("live").await;
    assert!(refused.is_err(), "a live file is left alone");
    remote.delete_permanently("binned").await.unwrap();
    remote.delete_permanently("gone").await.unwrap();
    assert_eq!(
        vec!["/drive/v3/files/binned".to_string()],
        *deleted.lock().unwrap()
    );
}
