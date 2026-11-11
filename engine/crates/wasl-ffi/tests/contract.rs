//! Contract tests of the binding surface (SRS FR-E4): the same calls Kotlin makes, made from Rust,
//! against a mocked Drive. The SAF test runs where `/proc/self/fd` exists (Linux, Android).

use std::sync::{Arc, Mutex};
use wasl_core::conditions::{Conditions, DeviceState};
use wasl_core::{RunStatus, SyncMode};
use wasl_engine::Trigger;
use wasl_mobile::{EngineInit, Platform, SafBridge, TokenSource, WaslEngine, WaslError};
use wasl_store::{Pair, StorageMode};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

struct Tokens {
    asked: Mutex<u32>,
}

impl TokenSource for Tokens {
    fn access_token(&self, _account: String) -> Result<String, WaslError> {
        *self.asked.lock().unwrap() += 1;
        Ok("tok".into())
    }
    fn invalidate(&self, _account: String) {}
}

struct Device {
    saf: Option<Arc<dyn SafBridge>>,
}

impl Platform for Device {
    fn device_state(&self) -> DeviceState {
        DeviceState::unrestricted()
    }
    fn saf(&self) -> Option<Arc<dyn SafBridge>> {
        self.saf.clone()
    }
    fn vault_access(&self, _vault_id: String) -> Option<wasl_mobile::VaultAccess> {
        None
    }
}

fn json(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.into_bytes(), "application/json; charset=utf-8")
}

fn md5(s: &str) -> String {
    wasl_fs::hash_reader(s.as_bytes()).unwrap().md5
}

/// A Drive with folder `A` holding `remote.txt`, accepting multipart uploads (it answers with the
/// MD5 of the body's content part, as Drive does).
async fn mock_drive(remote_content: &'static str) -> MockServer {
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
        .respond_with(json(r#"{"startPageToken":"7"}"#.into()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/drive/v3/files"))
        .respond_with(json(format!(
            r#"{{"files":[
                {{"id":"A","name":"Sync","mimeType":"application/vnd.google-apps.folder","parents":["ROOT"]}},
                {{"id":"B","name":"remote.txt","mimeType":"text/plain","parents":["A"],"size":"{}","md5Checksum":"{}","modifiedTime":"2026-01-02T03:04:05.000Z","version":"2"}}
            ]}}"#,
            remote_content.len(),
            md5(remote_content)
        )))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/drive/v3/changes"))
        .respond_with(json(r#"{"newStartPageToken":"7","changes":[]}"#.into()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/drive/v3/files/B"))
        .and(query_param("alt", "media"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(remote_content.as_bytes().to_vec()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/drive/v3/files"))
        .respond_with(|req: &Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap_or_default();
            let name = body["name"].as_str().unwrap_or("folder").to_owned();
            json(format!(
                r#"{{"id":"F-{name}","name":"{name}","mimeType":"application/vnd.google-apps.folder","parents":["A"]}}"#
            ))
        })
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/upload/drive/v3/files"))
        .respond_with(|req: &Request| {
            // The second part of the multipart body is the content.
            let body = String::from_utf8_lossy(&req.body).into_owned();
            let content = body.split("\r\n\r\n").nth(2).and_then(|s| s.split("\r\n--").next()).unwrap_or("").to_owned();
            json(format!(
                r#"{{"id":"U{}","name":"local.txt","mimeType":"text/plain","parents":["A"],"size":"{}","md5Checksum":"{}","version":"1"}}"#,
                content.len(),
                content.len(),
                md5(&content)
            ))
        })
        .mount(&server)
        .await;
    server
}

fn init(dir: &std::path::Path, server: &MockServer) -> EngineInit {
    EngineInit {
        db_path: dir.join("wasl.db").to_string_lossy().into_owned(),
        bin_dir: dir.join("bin").to_string_lossy().into_owned(),
        staging_dir: dir.join("staging").to_string_lossy().into_owned(),
        device_name: "Pixel".into(),
        drive_base_url: Some(format!("{}/drive/v3", server.uri())),
        drive_upload_url: Some(format!("{}/upload/drive/v3", server.uri())),
    }
}

fn pair(local_root: &str, mode: StorageMode) -> Pair {
    let mut p = Pair::new("p", "Sync", local_root);
    p.account_id = Some("me@example.com".into());
    p.drive_folder_id = Some("A".into());
    p.remote_path = "My Drive/Sync".into();
    p.conditions = Conditions::none();
    p.storage_mode = mode;
    p
}

#[tokio::test]
async fn the_binding_surface_runs_a_pair_end_to_end() {
    let server = mock_drive("from drive").await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("pair");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("local.txt"), "from device").unwrap();

    let tokens = Arc::new(Tokens { asked: Mutex::new(0) });
    let engine = WaslEngine::new(
        init(dir.path(), &server),
        tokens.clone(),
        Arc::new(Device { saf: None }),
        None,
    )
    .unwrap();
    assert!(!engine.version().is_empty());
    engine
        .create_pair(pair(root.to_str().unwrap(), StorageMode::Files))
        .unwrap();
    assert_eq!(1, engine.pairs().unwrap().len());

    // Preview, then the first-merge stop, then the start from the preview.
    let preview = engine.preview("p".into()).await.unwrap();
    assert_eq!(
        (1, 1),
        (preview.plan.summary.uploads, preview.plan.summary.downloads)
    );
    // A new pair waits for its preview (ADR-0001): a manual run doesn't start it.
    let held = engine.sync("p".into(), Trigger::Manual).await;
    assert_eq!(RunStatus::Skipped, held.status, "{:?}", held.message);
    // As if the preview had been seen: the first-merge stop is next.
    engine
        .set_pair_flags("p".into(), None, None, Some(false))
        .unwrap();
    let stopped = engine.sync("p".into(), Trigger::Manual).await;
    assert_eq!(RunStatus::Blocked, stopped.status);
    let blocked = engine
        .blocked_plan("p".into())
        .unwrap()
        .expect("saved for the Needs-you screen");
    assert_eq!(1, blocked.summary.unwrap().local_files);
    let run = engine.sync("p".into(), Trigger::Preview).await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    assert_eq!(
        "from drive",
        std::fs::read_to_string(root.join("remote.txt")).unwrap()
    );
    assert!(*tokens.asked.lock().unwrap() > 0, "tokens come from the platform");

    // History and status.
    let runs = engine.runs(Some("p".into()), 10).unwrap();
    assert_eq!(
        vec![RunStatus::Success, RunStatus::Blocked],
        runs.iter().map(|r| r.status).collect::<Vec<_>>()
    );
    assert!(
        engine
            .run_events(runs[0].id.clone())
            .unwrap()
            .iter()
            .any(|e| e.action == "UPLOAD")
    );
    assert_eq!(
        Some(RunStatus::Success),
        engine.pair("p".into()).unwrap().unwrap().status.last_status
    );
    assert!(engine.blocked_plan("p".into()).unwrap().is_none());

    // Editing keeps the engine's status; settings persist.
    let mut edited = engine.pair("p".into()).unwrap().unwrap();
    edited.method = SyncMode::UploadOnly;
    edited.status = Default::default();
    engine.update_pair(edited).unwrap();
    let got = engine.pair("p".into()).unwrap().unwrap();
    assert_eq!(
        (SyncMode::UploadOnly, Some(RunStatus::Success)),
        (got.method, got.status.last_status)
    );

    // Another folder from the phone starts the pair over, as on the desktop (#94).
    let mut moved = engine.pair("p".into()).unwrap().unwrap();
    moved.local_root = format!("{}-moved", moved.local_root);
    engine.update_pair(moved).unwrap();
    let got = engine.pair("p".into()).unwrap().unwrap();
    assert!(
        got.status.hold_for_preview,
        "the first run after the move is a preview"
    );
    engine.set_setting("device_name".into(), "جهازي".into()).unwrap();
    assert_eq!(
        Some("جهازي".into()),
        engine.setting("device_name".into()).unwrap()
    );

    engine.delete_pair("p".into()).unwrap();
    assert!(engine.pairs().unwrap().is_empty());
}

/// ADR-0001, ticket #35: the phone's `create_pair` wrote any pair to the store, while the desktop
/// checked it. Now both go through the same rules, and every new pair waits for its preview.
#[tokio::test]
async fn creating_a_pair_checks_the_pair_rules_and_holds_it_for_its_preview() {
    let server = mock_drive("x").await;
    let dir = tempfile::tempdir().unwrap();
    let engine = WaslEngine::new(
        init(dir.path(), &server),
        Arc::new(Tokens { asked: Mutex::new(0) }),
        Arc::new(Device { saf: None }),
        None,
    )
    .unwrap();
    let root = dir.path().join("pair");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    engine
        .create_pair(pair(root.to_str().unwrap(), StorageMode::Files))
        .unwrap();
    assert!(
        engine.pair("p".into()).unwrap().unwrap().status.hold_for_preview,
        "held for its first preview"
    );

    let mut inside = pair(root.join("sub").to_str().unwrap(), StorageMode::Files);
    inside.id = "q".into();
    let err = engine.create_pair(inside).unwrap_err();
    assert!(matches!(err, WaslError::PairRule { .. }), "{err:?}");
    assert!(
        err.to_string().contains("Sync"),
        "names the pair it overlaps: {err}"
    );

    let mut own = pair(dir.path().join("bin").to_str().unwrap(), StorageMode::Files);
    own.id = "r".into();
    assert!(
        matches!(engine.create_pair(own), Err(WaslError::PairRule { .. })),
        "not in Wasl's own folders"
    );
    assert_eq!(1, engine.pairs().unwrap().len(), "nothing refused was saved");
}

#[tokio::test]
async fn a_locked_vault_pair_fails_clearly() {
    let server = mock_drive("x").await;
    let dir = tempfile::tempdir().unwrap();
    let engine = WaslEngine::new(
        init(dir.path(), &server),
        Arc::new(Tokens { asked: Mutex::new(0) }),
        Arc::new(Device { saf: None }),
        None,
    )
    .unwrap();
    let root = dir.path().join("pair");
    std::fs::create_dir_all(&root).unwrap();
    let mut p = pair(root.to_str().unwrap(), StorageMode::Files);
    p.vault_id = Some("v1".into());
    engine.create_pair(p).unwrap();
    // Started from its preview, as every new pair is.
    let r = engine.sync("p".into(), Trigger::Preview).await;
    assert_eq!(
        (RunStatus::Failed, Some(wasl_core::ErrorKind::Locked)),
        (r.status, r.error_kind)
    );
    assert!(r.message.unwrap().contains("Unlock"));
}

/// E2E 2026-10: a phone pair on a vault made on the computer was a plain pair: its preview listed
/// the vault's scrambled files to download, and its uploads would have gone up unencrypted. The
/// phone now refuses it before planning, as the computer does.
#[tokio::test]
async fn a_plain_pair_on_a_vaults_folder_syncs_nothing() {
    const MANIFEST: &str = r#"{"format":"wasl-vault","version":2,"kdf":"argon2id","m":65536,"t":3,"p":1,"salt":"VgfQFrzs4R7trsH4AFPIOw==","keyCheck":"VfMW27+ueHyJey7FZrH/pZQK4aUtTLk8++uip6yrEws=","created":"2026-10-25T21:15:57.071Z","name":"e2e-vault"}"#;
    let server = mock_drive("x").await;
    Mock::given(method("GET"))
        .and(path("/drive/v3/files"))
        .respond_with(json(format!(
            r#"{{"files":[
                {{"id":"A","name":"Sync","mimeType":"application/vnd.google-apps.folder","parents":["ROOT"]}},
                {{"id":"V","name":"vault.json","mimeType":"application/json","parents":["A"],"size":"{}","md5Checksum":"{}","modifiedTime":"2026-01-02T03:04:05.000Z","version":"2"}}
            ]}}"#,
            MANIFEST.len(),
            md5(MANIFEST)
        )))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/drive/v3/files/V"))
        .and(query_param("alt", "media"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(MANIFEST.as_bytes().to_vec()))
        .with_priority(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let engine = WaslEngine::new(
        init(dir.path(), &server),
        Arc::new(Tokens { asked: Mutex::new(0) }),
        Arc::new(Device { saf: None }),
        None,
    )
    .unwrap();
    let root = dir.path().join("pair");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("diary.txt"), "private").unwrap();
    engine
        .create_pair(pair(root.to_str().unwrap(), StorageMode::Files))
        .unwrap();

    let r = engine.sync("p".into(), Trigger::Preview).await;
    assert_eq!(RunStatus::Failed, r.status, "{:?}", r.message);
    assert!(r.message.unwrap_or_default().contains("encrypted vault"));
    let uploads = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|q| q.url.path().starts_with("/upload/"))
        .count();
    assert_eq!(0, uploads, "nothing went up unencrypted");
    assert!(
        !root.join("vault.json").exists(),
        "the vault's files did not come down"
    );
}

#[tokio::test]
async fn a_saf_pair_without_saf_fails_clearly() {
    let server = mock_drive("x").await;
    let dir = tempfile::tempdir().unwrap();
    let engine = WaslEngine::new(
        init(dir.path(), &server),
        Arc::new(Tokens { asked: Mutex::new(0) }),
        Arc::new(Device { saf: None }),
        None,
    )
    .unwrap();
    engine
        .create_pair(pair("content://tree/x", StorageMode::Saf))
        .unwrap();
    // Started from its preview, as every new pair is.
    let r = engine.sync("p".into(), Trigger::Preview).await;
    assert_eq!(RunStatus::Failed, r.status);
    assert!(r.message.unwrap().contains("Storage Access Framework"));
}

/// A SAF bridge over a plain folder that hands out real descriptors, as Kotlin does.
#[cfg(any(target_os = "linux", target_os = "android"))]
mod saf {
    use super::*;
    use std::collections::HashMap;
    use std::os::fd::AsRawFd;
    use wasl_mobile::SafEntry;

    pub struct FolderBridge {
        pub root: std::path::PathBuf,
        pub bin: std::path::PathBuf,
        pub open: Mutex<HashMap<i32, std::fs::File>>,
        pub trees: Mutex<Vec<String>>,
    }

    fn e(err: std::io::Error) -> WaslError {
        WaslError::Io {
            detail: err.to_string(),
        }
    }

    fn entry(name: String, m: &std::fs::Metadata) -> SafEntry {
        let mtime_ms = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as i64);
        SafEntry {
            name,
            is_dir: m.is_dir(),
            size: if m.is_dir() { 0 } else { m.len() },
            mtime_ms,
        }
    }

    impl FolderBridge {
        fn p(&self, tree: &str, rel: &str) -> std::path::PathBuf {
            self.trees.lock().unwrap().push(tree.to_owned());
            if rel.is_empty() {
                self.root.clone()
            } else {
                self.root.join(rel)
            }
        }
    }

    impl SafBridge for FolderBridge {
        fn list(&self, tree: String, dir: String) -> Result<Vec<SafEntry>, WaslError> {
            let mut out = Vec::new();
            for item in std::fs::read_dir(self.p(&tree, &dir)).map_err(e)? {
                let item = item.map_err(e)?;
                out.push(entry(
                    item.file_name().to_string_lossy().into_owned(),
                    &item.metadata().map_err(e)?,
                ));
            }
            Ok(out)
        }
        fn stat(&self, tree: String, rel: String) -> Result<Option<SafEntry>, WaslError> {
            let p = self.p(&tree, &rel);
            Ok(std::fs::metadata(&p)
                .ok()
                .map(|m| entry(rel.rsplit('/').next().unwrap().to_owned(), &m)))
        }
        fn open_read_fd(&self, tree: String, rel: String) -> Result<i32, WaslError> {
            let f = std::fs::File::open(self.p(&tree, &rel)).map_err(e)?;
            let fd = f.as_raw_fd();
            self.open.lock().unwrap().insert(fd, f);
            Ok(fd)
        }
        fn copy_out(&self, tree: String, rel: String, dest: String) -> Result<(), WaslError> {
            std::fs::copy(self.p(&tree, &rel), &dest).map_err(e)?;
            Ok(())
        }
        fn close_fd(&self, fd: i32) {
            self.open.lock().unwrap().remove(&fd);
        }
        fn install(
            &self,
            tree: String,
            staged: String,
            rel: String,
            _mtime: Option<i64>,
        ) -> Result<SafEntry, WaslError> {
            let target = self.p(&tree, &rel);
            std::fs::create_dir_all(target.parent().unwrap()).map_err(e)?;
            std::fs::copy(&staged, &target).map_err(e)?;
            std::fs::remove_file(&staged).map_err(e)?;
            Ok(entry(
                rel.rsplit('/').next().unwrap().to_owned(),
                &std::fs::metadata(&target).map_err(e)?,
            ))
        }
        fn mkdir(&self, tree: String, rel: String) -> Result<(), WaslError> {
            std::fs::create_dir_all(self.p(&tree, &rel)).map_err(e)
        }
        fn remove_file(&self, tree: String, rel: String) -> Result<(), WaslError> {
            std::fs::remove_file(self.p(&tree, &rel)).map_err(e)
        }
        fn remove_dir_if_empty(&self, tree: String, rel: String) -> Result<bool, WaslError> {
            Ok(std::fs::remove_dir(self.p(&tree, &rel)).is_ok())
        }
        fn rename(&self, tree: String, from: String, to: String) -> Result<(), WaslError> {
            std::fs::rename(self.p(&tree, &from), self.p(&tree, &to)).map_err(e)
        }
        fn move_to_bin(&self, tree: String, rel: String) -> Result<String, WaslError> {
            std::fs::create_dir_all(&self.bin).map_err(e)?;
            let loc = format!("b{}", self.open.lock().unwrap().len() + rel.len());
            std::fs::rename(self.p(&tree, &rel), self.bin.join(&loc)).map_err(e)?;
            Ok(loc)
        }
        fn restore_from_bin(&self, tree: String, location: String, rel: String) -> Result<(), WaslError> {
            std::fs::rename(self.bin.join(location), self.p(&tree, &rel)).map_err(e)
        }
        fn purge_from_bin(&self, _tree: String, location: String) -> Result<(), WaslError> {
            let _ = std::fs::remove_file(self.bin.join(location));
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_saf_pair_syncs_through_file_descriptors() {
        let server = mock_drive("from drive").await;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("tree");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/local.txt"), "from device").unwrap();
        let bridge = Arc::new(FolderBridge {
            root: root.clone(),
            bin: dir.path().join("safbin"),
            open: Mutex::new(HashMap::new()),
            trees: Mutex::new(Vec::new()),
        });
        let engine = WaslEngine::new(
            init(dir.path(), &server),
            Arc::new(Tokens { asked: Mutex::new(0) }),
            Arc::new(Device {
                saf: Some(bridge.clone()),
            }),
            None,
        )
        .unwrap();
        engine
            .create_pair(pair(
                "content://com.android.externalstorage.documents/tree/primary%3ASync",
                StorageMode::Saf,
            ))
            .unwrap();
        let run = engine.sync("p".into(), Trigger::Preview).await;
        let errors: Vec<String> = engine
            .run_events(run.run_id.clone())
            .unwrap()
            .into_iter()
            .filter(|e| e.action == "ERROR")
            .map(|e| e.message)
            .collect();
        assert_eq!(RunStatus::Success, run.status, "{:?} {errors:?}", run.message);
        assert_eq!(
            "from drive",
            std::fs::read_to_string(root.join("remote.txt")).unwrap()
        );
        assert!(
            bridge.open.lock().unwrap().is_empty(),
            "every descriptor was closed"
        );
        assert!(
            bridge
                .trees
                .lock()
                .unwrap()
                .iter()
                .all(|t| t.starts_with("content://")),
            "the tree URI is passed through"
        );
        assert!(
            std::fs::read_dir(dir.path().join("staging/p")).map_or(0, |d| d.count()) == 0,
            "staging is cleaned up"
        );
    }
}
