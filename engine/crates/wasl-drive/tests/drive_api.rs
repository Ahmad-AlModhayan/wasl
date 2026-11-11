//! Port of `DriveApiTest.kt`. MockWebServer becomes wiremock (a FIFO responder, see `common`), and
//! `runTest` + `currentTime` becomes paused tokio time + `Instant::elapsed`: every backoff and
//! `Retry-After` wait happens in virtual time.

mod common;

use std::collections::HashMap;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use common::*;
use serde_json::Value;
use tempfile::TempDir;
use tokio::time::Instant;
use wasl_drive::{
    DriveClient, DriveError, FILE_FIELDS, FOLDER_MIME, ListPage, PatchFileMetadata, SessionRef, SessionStore,
    UploadRequest, session_store_key, to_rfc3339,
};
use wiremock::ResponseTemplate;

const CHUNK: u64 = DriveClient::CHUNK as u64;
const MULTIPART_LIMIT: u64 = DriveClient::MULTIPART_LIMIT;
/// 2025-01-02T03:04:05Z
const MTIME: i64 = 1_735_787_045_000;
/// What the store sees for the caller key "k": sessions are namespaced by account.
const K: &str = "me@example.com|k";

struct Env {
    server: MockDrive,
    tokens: Arc<FakeTokens>,
    api: DriveClient,
    tmp: TempDir,
    start: Instant,
}

async fn env() -> Env {
    let server = MockDrive::start().await;
    let tokens = Arc::new(FakeTokens::default());
    let api = server.client(tokens.clone());
    Env {
        server,
        tokens,
        api,
        tmp: TempDir::new().unwrap(),
        start: Instant::now(),
    }
}

impl Env {
    /// Virtual time since the test started (Kotlin `currentTime`).
    fn elapsed_ms(&self) -> u128 {
        self.start.elapsed().as_millis()
    }
    fn file(&self, name: &str) -> PathBuf {
        self.tmp.path().join(name)
    }
    fn file_with(&self, name: &str, text: &str) -> PathBuf {
        let p = self.file(name);
        std::fs::write(&p, text).unwrap();
        p
    }
    /// Sparse file of `size` bytes with recognisable marker bytes at the given offsets.
    fn big_file(&self, name: &str, size: u64, markers: &[(u64, u8)]) -> PathBuf {
        let p = self.file(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.set_len(size).unwrap();
        for (offset, b) in markers {
            f.seek(SeekFrom::Start(*offset)).unwrap();
            f.write_all(&[*b]).unwrap();
        }
        p
    }
    fn session_response(&self, path: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).insert_header("Location", self.server.url(path).as_str())
    }
}

fn read_text(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

fn len(p: &Path) -> u64 {
    std::fs::metadata(p).unwrap().len()
}

fn meta_line(body: &str) -> Value {
    let line = body.lines().find(|l| l.starts_with('{')).unwrap();
    serde_json::from_str(line).unwrap()
}

#[derive(Default)]
struct FakeSessionStore {
    map: Mutex<HashMap<String, String>>,
    ops: Mutex<Vec<String>>,
}

impl FakeSessionStore {
    fn with(key: &str, uri: &str) -> Self {
        let s = FakeSessionStore::default();
        s.map.lock().unwrap().insert(key.into(), uri.into());
        s
    }
    fn ops(&self) -> Vec<String> {
        self.ops.lock().unwrap().clone()
    }
    fn map(&self) -> HashMap<String, String> {
        self.map.lock().unwrap().clone()
    }
}

#[async_trait]
impl SessionStore for FakeSessionStore {
    async fn get(&self, key: &str) -> Result<Option<String>, DriveError> {
        self.ops.lock().unwrap().push(format!("get:{key}"));
        Ok(self.map.lock().unwrap().get(key).cloned())
    }
    async fn put(&self, key: &str, uri: &str) -> Result<(), DriveError> {
        self.ops.lock().unwrap().push(format!("put:{key}"));
        self.map.lock().unwrap().insert(key.into(), uri.into());
        Ok(())
    }
    async fn remove(&self, key: &str) -> Result<(), DriveError> {
        self.ops.lock().unwrap().push(format!("remove:{key}"));
        self.map.lock().unwrap().remove(key);
        Ok(())
    }
}

fn no_progress(_: u64, _: Option<u64>) {}
fn no_upload_progress(_: u64, _: u64) {}

// ================================================================ plumbing

#[tokio::test(start_paused = true)]
async fn get_file_sends_bearer_token_and_field_selection() {
    let t = env().await;
    t.server.enqueue(json_response(&file_json(
        "abc",
        "hello.txt",
        "text/plain",
        Some("root"),
        r#","size":"12","md5Checksum":"aa","modifiedTime":"2026-01-02T03:04:05.000Z","version":"7","trashed":false"#,
    )));
    let f = t.api.get_file(ACCOUNT, "abc").await.unwrap();
    assert_eq!("abc", f.id);
    assert_eq!("hello.txt", f.name);
    assert_eq!(Some(12), f.size_u64());
    assert_eq!(Some(7), f.version_u64());
    assert_eq!(Some("aa"), f.md5_checksum.as_deref());
    assert_eq!(Some(vec!["root".to_string()]), f.parents);
    assert!(!f.is_folder());
    assert!(!f.is_shortcut());
    assert!(!f.is_google_doc());

    let req = t.server.take().await;
    assert_eq!("GET", req.method.as_str());
    assert_eq!(Some("Bearer tok".into()), header(&req, "Authorization"));
    assert_eq!("/drive/v3/files/abc", path(&req));
    assert_eq!(Some(FILE_FIELDS.into()), query(&req, "fields"));
    assert_eq!(Some("true".into()), query(&req, "supportsAllDrives"));
    assert_eq!(vec![ACCOUNT.to_string()], t.tokens.requested());
}

#[tokio::test(start_paused = true)]
async fn root_id_resolves_the_root_alias() {
    let t = env().await;
    t.server.enqueue(json_response(&file_json(
        "0AB",
        "My Drive",
        FOLDER_MIME,
        None,
        "",
    )));
    assert_eq!("0AB", t.api.root_id(ACCOUNT).await.unwrap());
    assert_eq!("/drive/v3/files/root", path(&t.server.take().await));
}

#[tokio::test(start_paused = true)]
async fn about_parses_user_and_quota() {
    let t = env().await;
    t.server.enqueue(json_response(
        r#"{"user":{"emailAddress":"a@b.c","displayName":"A","photoLink":"http://p"},"storageQuota":{"limit":"100","usage":"5","usageInDrive":"4","usageInDriveTrash":"1"}}"#,
    ));
    let about = t.api.about(ACCOUNT).await.unwrap();
    let user = about.user.unwrap();
    assert_eq!(Some("a@b.c"), user.email_address.as_deref());
    assert_eq!(Some("A"), user.display_name.as_deref());
    let quota = about.storage_quota.unwrap();
    assert_eq!(Some("100"), quota.limit.as_deref());
    assert_eq!(Some("5"), quota.usage.as_deref());
    let req = t.server.take().await;
    assert_eq!("/drive/v3/about", path(&req));
    assert!(query(&req, "fields").unwrap().contains("storageQuota"));
}

// ================================================================ listing

#[tokio::test(start_paused = true)]
async fn list_children_paginates_and_sends_query_parameters() {
    let t = env().await;
    t.server.enqueue(json_response(&format!(
        r#"{{"nextPageToken":"t2","files":[{}]}}"#,
        file_json("1", "a", "text/plain", None, "")
    )));
    t.server.enqueue(json_response(&format!(
        r#"{{"files":[{}]}}"#,
        file_json("2", "b", "text/plain", None, "")
    )));

    let files = t.api.list_children(ACCOUNT, "parent1", None).await.unwrap();
    assert_eq!(
        vec!["1", "2"],
        files.iter().map(|f| f.id.as_str()).collect::<Vec<_>>()
    );

    let r1 = t.server.take().await;
    assert_eq!("/drive/v3/files", path(&r1));
    assert_eq!(
        Some("'parent1' in parents and trashed = false".into()),
        query(&r1, "q")
    );
    assert_eq!(Some("1000".into()), query(&r1, "pageSize"));
    assert_eq!(
        Some(format!("nextPageToken,incompleteSearch,files({FILE_FIELDS})")),
        query(&r1, "fields")
    );
    assert_eq!(Some("drive".into()), query(&r1, "spaces"));
    assert_eq!(Some("true".into()), query(&r1, "supportsAllDrives"));
    assert_eq!(Some("false".into()), query(&r1, "includeItemsFromAllDrives"));
    assert_eq!(Some("user".into()), query(&r1, "corpora"));
    assert_eq!(None, query(&r1, "driveId"));
    assert_eq!(None, query(&r1, "pageToken"));
    assert_eq!(None, query(&r1, "orderBy"));

    let r2 = t.server.take().await;
    assert_eq!(Some("t2".into()), query(&r2, "pageToken"));
    assert_eq!(
        Some("'parent1' in parents and trashed = false".into()),
        query(&r2, "q")
    );
}

#[tokio::test(start_paused = true)]
async fn list_page_shared_drive_sets_corpora_and_drive_id() {
    let t = env().await;
    t.server
        .enqueue(json_response(r#"{"files":[],"incompleteSearch":true}"#));
    let page = t
        .api
        .list_page(
            ACCOUNT,
            &ListPage {
                q: None,
                page_token: Some("p9"),
                drive_id: Some("D1"),
                page_size: 50,
                order_by: Some("name"),
                ..ListPage::default()
            },
        )
        .await
        .unwrap();
    assert!(page.files.is_empty());
    assert_eq!(None, page.next_page_token);
    assert!(page.incomplete_search);
    let u = t.server.take().await;
    assert_eq!(Some("drive".into()), query(&u, "corpora"));
    assert_eq!(Some("D1".into()), query(&u, "driveId"));
    assert_eq!(Some("true".into()), query(&u, "includeItemsFromAllDrives"));
    assert_eq!(Some("true".into()), query(&u, "supportsAllDrives"));
    assert_eq!(Some("50".into()), query(&u, "pageSize"));
    assert_eq!(Some("name".into()), query(&u, "orderBy"));
    assert_eq!(Some("p9".into()), query(&u, "pageToken"));
    assert_eq!(None, query(&u, "q"));
}

#[tokio::test(start_paused = true)]
async fn list_app_data_uses_the_app_data_folder_space() {
    let t = env().await;
    t.server.enqueue(json_response(&format!(
        r#"{{"files":[{}]}}"#,
        file_json("cfg", "config.json", "text/plain", None, "")
    )));
    let files = t.api.list_app_data(ACCOUNT).await.unwrap();
    assert_eq!(
        vec!["cfg"],
        files.iter().map(|f| f.id.as_str()).collect::<Vec<_>>()
    );
    let u = t.server.take().await;
    assert_eq!(Some("appDataFolder".into()), query(&u, "spaces"));
    assert_eq!(None, query(&u, "q"));
    assert_eq!(Some("user".into()), query(&u, "corpora"));
}

#[tokio::test(start_paused = true)]
async fn list_shared_drives_paginates() {
    let t = env().await;
    t.server.enqueue(json_response(
        r#"{"nextPageToken":"x","drives":[{"id":"D1","name":"Team"}]}"#,
    ));
    t.server
        .enqueue(json_response(r#"{"drives":[{"id":"D2","name":"Other"}]}"#));
    let drives = t.api.list_shared_drives(ACCOUNT).await.unwrap();
    assert_eq!(
        vec![("D1", "Team"), ("D2", "Other")],
        drives
            .iter()
            .map(|d| (d.id.as_str(), d.name.as_str()))
            .collect::<Vec<_>>()
    );
    let r1 = t.server.take().await;
    assert_eq!("/drive/v3/drives", path(&r1));
    assert_eq!(Some("100".into()), query(&r1, "pageSize"));
    assert_eq!(None, query(&r1, "pageToken"));
    assert_eq!(Some("x".into()), query(&t.server.take().await, "pageToken"));
}

// ================================================================ changes

#[tokio::test(start_paused = true)]
async fn start_page_token_parses_and_supports_shared_drives() {
    let t = env().await;
    t.server.enqueue(json_response(r#"{"startPageToken":"123"}"#));
    assert_eq!("123", t.api.start_page_token(ACCOUNT, None).await.unwrap());
    let u = t.server.take().await;
    assert_eq!("/drive/v3/changes/startPageToken", path(&u));
    assert_eq!(Some("true".into()), query(&u, "supportsAllDrives"));
    assert_eq!(None, query(&u, "driveId"));

    t.server.enqueue(json_response(r#"{"startPageToken":"7"}"#));
    assert_eq!("7", t.api.start_page_token(ACCOUNT, Some("D1")).await.unwrap());
    assert_eq!(Some("D1".into()), query(&t.server.take().await, "driveId"));
}

#[tokio::test(start_paused = true)]
async fn changes_page_parses_removed_items_and_new_start_page_token() {
    let t = env().await;
    let f2 = file_json(
        "f2",
        "n",
        "text/plain",
        Some("root"),
        r#","size":"12","md5Checksum":"abc","modifiedTime":"2026-01-02T03:04:05.000Z","version":"7","trashed":false"#,
    );
    t.server.enqueue(json_response(&format!(
        r#"{{"newStartPageToken":"99","changes":[
            {{"changeType":"file","fileId":"f1","removed":true,"time":"2026-01-01T00:00:00.000Z"}},
            {{"changeType":"file","fileId":"f2","removed":false,"file":{f2}}},
            {{"changeType":"drive","driveId":"D","time":"2026-01-01T00:00:00.000Z"}}
        ]}}"#
    )));
    let page = t.api.changes_page(ACCOUNT, "5", None).await.unwrap();
    assert_eq!(Some("99"), page.new_start_page_token.as_deref());
    assert_eq!(None, page.next_page_token);
    assert_eq!(3, page.changes.len());
    let (c1, c2, c3) = (&page.changes[0], &page.changes[1], &page.changes[2]);
    assert!(c1.removed);
    assert_eq!(Some("f1"), c1.file_id.as_deref());
    assert!(c1.file.is_none());
    assert!(!c2.removed);
    let file2 = c2.file.as_ref().unwrap();
    assert_eq!("f2", file2.id);
    assert_eq!(Some("abc"), file2.md5_checksum.as_deref());
    assert_eq!(Some(12), file2.size_u64());
    assert_eq!(Some("drive"), c3.change_type.as_deref());
    assert_eq!(Some("D"), c3.drive_id.as_deref());
    assert_eq!(None, c3.file_id);

    let u = t.server.take().await;
    assert_eq!("/drive/v3/changes", path(&u));
    assert_eq!(Some("5".into()), query(&u, "pageToken"));
    assert_eq!(Some("true".into()), query(&u, "includeRemoved"));
    assert_eq!(Some("1000".into()), query(&u, "pageSize"));
    assert_eq!(Some("true".into()), query(&u, "supportsAllDrives"));
    assert_eq!(Some("true".into()), query(&u, "includeItemsFromAllDrives"));
    assert_eq!(None, query(&u, "driveId"));
    let fields = query(&u, "fields").unwrap();
    assert!(fields.contains("newStartPageToken"));
    assert!(fields.contains(&format!("file({FILE_FIELDS})")));
}

#[tokio::test(start_paused = true)]
async fn changes_page_with_next_page_token_is_returned_for_the_caller_to_continue() {
    let t = env().await;
    t.server
        .enqueue(json_response(r#"{"nextPageToken":"6","changes":[]}"#));
    let page = t.api.changes_page(ACCOUNT, "5", Some("D1")).await.unwrap();
    assert_eq!(Some("6"), page.next_page_token.as_deref());
    assert_eq!(None, page.new_start_page_token);
    assert_eq!(Some("D1".into()), query(&t.server.take().await, "driveId"));
}

// ================================================================ error mapping and retries

#[tokio::test(start_paused = true)]
async fn unauthorized_invalidates_the_token_and_retries_once_without_backoff() {
    let t = env().await;
    t.server
        .enqueue(api_error(401, "authError", "Invalid Credentials"));
    t.server.enqueue(json_response(&fj("abc")));
    assert_eq!("abc", t.api.get_file(ACCOUNT, "abc").await.unwrap().id);
    assert_eq!(1, t.tokens.invalidations());
    assert_eq!(2, t.server.request_count().await);
    assert_eq!(
        Some("Bearer tok".into()),
        header(&t.server.take().await, "Authorization")
    );
    assert_eq!(
        Some("Bearer tok2".into()),
        header(&t.server.take().await, "Authorization")
    );
    assert_eq!(0, t.elapsed_ms(), "the auth retry is immediate");
}

#[tokio::test(start_paused = true)]
async fn unauthorized_twice_fails_with_auth_after_a_single_refresh() {
    let t = env().await;
    t.server
        .enqueue(api_error(401, "authError", "Invalid Credentials"));
    t.server
        .enqueue(api_error(401, "authError", "Invalid Credentials"));
    let e = t.api.get_file(ACCOUNT, "abc").await.unwrap_err();
    assert!(matches!(e, DriveError::Auth { .. }), "{e:?}");
    assert!(e.to_string().contains("Invalid Credentials"));
    assert_eq!(1, t.tokens.invalidations());
    assert_eq!(2, t.server.request_count().await);
}

#[tokio::test(start_paused = true)]
async fn a_network_error_while_getting_the_token_is_retried_like_any_network_error() {
    let t = env().await;
    t.tokens.fail_next(DriveError::Network(
        "Network error while contacting Google.".into(),
    ));
    t.server.enqueue(json_response(&fj("abc")));

    assert_eq!("abc", t.api.get_file(ACCOUNT, "abc").await.unwrap().id);
    assert_eq!(1, t.server.request_count().await);
}

#[tokio::test(start_paused = true)]
async fn rate_limited403_is_retried_after_exponential_backoff() {
    let t = env().await;
    t.server
        .enqueue(api_error(403, "userRateLimitExceeded", "Rate Limit Exceeded"));
    t.server.enqueue(json_response(&fj("abc")));
    assert_eq!("abc", t.api.get_file(ACCOUNT, "abc").await.unwrap().id);
    assert_eq!(2, t.server.request_count().await);
    let ms = t.elapsed_ms();
    assert!(
        (1000..=1999).contains(&ms),
        "first backoff is 1s plus up to 1s jitter, got {ms}"
    );
    assert_eq!(0, t.tokens.invalidations());
}

#[tokio::test(start_paused = true)]
async fn too_many_requests429_waits_exactly_retry_after() {
    let t = env().await;
    t.server
        .enqueue(api_error(429, "rateLimitExceeded", "Too many").insert_header("Retry-After", "5"));
    t.server.enqueue(json_response(&fj("abc")));
    assert_eq!("abc", t.api.get_file(ACCOUNT, "abc").await.unwrap().id);
    assert_eq!(2, t.server.request_count().await);
    assert_eq!(5000, t.elapsed_ms());
}

#[tokio::test(start_paused = true)]
async fn storage_quota403_is_not_retried() {
    let t = env().await;
    t.server.enqueue(api_error(
        403,
        "storageQuotaExceeded",
        "The user's Drive storage quota has been exceeded.",
    ));
    let e = t.api.get_file(ACCOUNT, "abc").await.unwrap_err();
    assert!(matches!(e, DriveError::StorageQuota(_)), "{e:?}");
    assert!(e.to_string().contains("storage quota"));
    assert_eq!(1, t.server.request_count().await);
    assert_eq!(0, t.elapsed_ms());
}

#[tokio::test(start_paused = true)]
async fn not_found404_is_not_retried() {
    let t = env().await;
    t.server
        .enqueue(api_error(404, "notFound", "File not found: abc."));
    let e = t.api.get_file(ACCOUNT, "abc").await.unwrap_err();
    assert!(matches!(e, DriveError::NotFound(_)), "{e:?}");
    assert_eq!("File not found: abc.", e.to_string());
    assert_eq!(1, t.server.request_count().await);
}

#[tokio::test(start_paused = true)]
async fn server_error500_is_retried_then_succeeds() {
    let t = env().await;
    t.server.enqueue(api_error(500, "backendError", "Backend Error"));
    t.server.enqueue(json_response(&fj("abc")));
    assert_eq!("abc", t.api.get_file(ACCOUNT, "abc").await.unwrap().id);
    assert_eq!(2, t.server.request_count().await);
    assert!((1000..=1999).contains(&t.elapsed_ms()));
}

#[tokio::test(start_paused = true)]
async fn non_json_server_error_is_still_transient() {
    let t = env().await;
    t.server
        .enqueue(ResponseTemplate::new(502).set_body_string("<html>Bad Gateway</html>"));
    t.server.enqueue(json_response(&fj("abc")));
    assert_eq!("abc", t.api.get_file(ACCOUNT, "abc").await.unwrap().id);
    assert_eq!(2, t.server.request_count().await);
}

#[tokio::test(start_paused = true)]
async fn transient_errors_give_up_after_six_attempts() {
    let t = env().await;
    for _ in 0..6 {
        t.server.enqueue(api_error(503, "backendError", "Backend Error"));
    }
    let e = t.api.get_file(ACCOUNT, "abc").await.unwrap_err();
    assert!(matches!(e, DriveError::Http { code: 503, .. }), "{e:?}");
    assert_eq!(6, t.server.request_count().await);
    // 1+2+4+8+16 seconds of backoff (each with < 1s jitter), all in virtual time
    let ms = t.elapsed_ms();
    assert!((31_000..=35_999).contains(&ms), "backoff total was {ms}");
}

#[tokio::test(start_paused = true)]
async fn forbidden_with_unknown_reason_is_not_retried() {
    let t = env().await;
    t.server.enqueue(api_error(403, "domainPolicy", "Blocked"));
    let e = t.api.get_file(ACCOUNT, "abc").await.unwrap_err();
    match e {
        DriveError::Http { code, reason, .. } => {
            assert_eq!(403, code);
            assert_eq!(Some("domainPolicy".into()), reason);
        }
        other => panic!("expected Http, got {other:?}"),
    }
    assert_eq!(1, t.server.request_count().await);
}

// ================================================================ downloads

#[tokio::test(start_paused = true)]
async fn download_writes_the_file_and_reports_progress() {
    let t = env().await;
    t.server
        .enqueue(ResponseTemplate::new(200).set_body_string("hello world"));
    let dest = t.file("a.part");
    let mut progress = Vec::new();
    t.api
        .download(ACCOUNT, "f1", &dest, Some(11), |d, total| {
            progress.push((d, total))
        })
        .await
        .unwrap();
    assert_eq!("hello world", read_text(&dest));
    let req = t.server.take().await;
    assert_eq!("/drive/v3/files/f1", path(&req));
    assert_eq!(Some("media".into()), query(&req, "alt"));
    assert_eq!(Some("true".into()), query(&req, "supportsAllDrives"));
    assert_eq!(None, header(&req, "Range"));
    assert_eq!(Some("Bearer tok".into()), header(&req, "Authorization"));
    assert_eq!(Some(&(11, Some(11))), progress.last());
}

#[tokio::test(start_paused = true)]
async fn a_file_drive_flags_as_malware_is_downloaded_with_the_flag_acknowledged() {
    // Drive's scan flags ordinary files (a vite.config.ts, 2026-10) and refuses them until the
    // request acknowledges it, which Drive allows the owner.
    let t = env().await;
    t.server.enqueue(api_error(
        403,
        "cannotDownloadAbusiveFile",
        "This file has been identified as malware or spam and cannot be downloaded.",
    ));
    t.server
        .enqueue(ResponseTemplate::new(200).set_body_string("hello world"));
    let dest = t.file("a.part");
    let flagged = t
        .api
        .download_flagged(ACCOUNT, "f1", &dest, Some(11), no_progress)
        .await
        .unwrap();
    assert!(flagged);
    assert_eq!("hello world", read_text(&dest));
    assert_eq!(None, query(&t.server.take().await, "acknowledgeAbuse"));
    assert_eq!(
        Some("true".into()),
        query(&t.server.take().await, "acknowledgeAbuse")
    );

    // A file that is not the user's stays refused, with a reason that says what happened.
    t.server
        .enqueue(api_error(403, "cannotDownloadAbusiveFile", "flagged"));
    t.server
        .enqueue(api_error(403, "cannotDownloadAbusiveFile", "flagged"));
    let e = t
        .api
        .download(ACCOUNT, "f2", &t.file("b.part"), Some(11), no_progress)
        .await
        .unwrap_err();
    assert!(e.is_flagged_abusive());
    assert!(e.user_hint().contains("malware"));
    assert_eq!(
        4,
        t.server.request_count().await,
        "one acknowledged retry, no more"
    );

    // An ordinary download is not flagged.
    t.server
        .enqueue(ResponseTemplate::new(200).set_body_string("hello world"));
    assert!(
        !t.api
            .download_flagged(ACCOUNT, "f3", &t.file("c.part"), Some(11), no_progress)
            .await
            .unwrap()
    );
}

#[tokio::test(start_paused = true)]
async fn metadata_is_asked_gzip_encoded_and_file_content_as_stored() {
    use std::io::Write as _;
    let t = env().await;
    let json = file_json(
        "abc",
        "notes.md",
        "text/markdown",
        Some("root"),
        r#","size":"12","md5Checksum":"aa","modifiedTime":"2026-01-02T03:04:05.000Z","version":"7","trashed":false"#,
    );
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(json.as_bytes()).unwrap();
    t.server.enqueue(
        ResponseTemplate::new(200)
            .insert_header("Content-Type", "application/json")
            .insert_header("Content-Encoding", "gzip")
            .set_body_bytes(gz.finish().unwrap()),
    );
    let f = t.api.get_file(ACCOUNT, "abc").await.unwrap();
    assert_eq!("notes.md", f.name);
    let req = t.server.take().await;
    assert!(header(&req, "Accept-Encoding").is_some_and(|v| v.contains("gzip")));
    assert!(header(&req, "User-Agent").is_some_and(|v| v.contains("gzip")));

    // File bytes are asked for as stored, so resume offsets and MD5 stay the file's own.
    t.server
        .enqueue(ResponseTemplate::new(200).set_body_string("hello world"));
    let dest = t.file("b.part");
    t.api
        .download(ACCOUNT, "f1", &dest, Some(11), no_progress)
        .await
        .unwrap();
    assert_eq!("hello world", read_text(&dest));
    assert_eq!(
        Some("identity".into()),
        header(&t.server.take().await, "Accept-Encoding")
    );
}

#[tokio::test(start_paused = true)]
async fn download_without_expected_size_uses_content_length_for_progress() {
    let t = env().await;
    t.server
        .enqueue(ResponseTemplate::new(200).set_body_string("hello world"));
    let dest = t.file("a.part");
    let mut progress = Vec::new();
    t.api
        .download(ACCOUNT, "f1", &dest, None, |d, total| progress.push((d, total)))
        .await
        .unwrap();
    assert_eq!(Some(&(11, Some(11))), progress.last());
}

#[tokio::test(start_paused = true)]
async fn download_size_mismatch_throws_integrity_without_retrying() {
    let t = env().await;
    t.server
        .enqueue(ResponseTemplate::new(200).set_body_string("hello world"));
    let dest = t.file("a.part");
    let e = t
        .api
        .download(ACCOUNT, "f1", &dest, Some(20), no_progress)
        .await
        .unwrap_err();
    assert!(matches!(e, DriveError::Integrity(_)), "{e:?}");
    assert!(e.to_string().contains("11 != 20"));
    assert_eq!(1, t.server.request_count().await);
    assert_eq!(11, len(&dest), "partial data is kept for a later resume");
}

#[tokio::test(start_paused = true)]
async fn download_resumes_from_an_existing_part_file() {
    let t = env().await;
    let dest = t.file_with("a.part", "hello ");
    t.server.enqueue(
        ResponseTemplate::new(206)
            .insert_header("Content-Range", "bytes 6-10/11")
            .set_body_string("world"),
    );
    let mut progress = Vec::new();
    t.api
        .download(ACCOUNT, "f1", &dest, Some(11), |d, total| {
            progress.push((d, total))
        })
        .await
        .unwrap();
    assert_eq!(Some("bytes=6-".into()), header(&t.server.take().await, "Range"));
    assert_eq!("hello world", read_text(&dest));
    assert_eq!(Some(&(11, Some(11))), progress.last());
}

#[tokio::test(start_paused = true)]
async fn download_resume_without_expected_size_adds_existing_bytes_to_the_total() {
    let t = env().await;
    let dest = t.file_with("a.part", "hello ");
    t.server
        .enqueue(ResponseTemplate::new(206).set_body_string("world"));
    let mut progress = Vec::new();
    t.api
        .download(ACCOUNT, "f1", &dest, None, |d, total| progress.push((d, total)))
        .await
        .unwrap();
    assert_eq!("hello world", read_text(&dest));
    assert_eq!(Some(&(11, Some(11))), progress.last());
}

#[tokio::test(start_paused = true)]
async fn download_when_the_part_file_is_already_complete_makes_no_request() {
    let t = env().await;
    let dest = t.file_with("a.part", "hello world");
    t.api
        .download(ACCOUNT, "f1", &dest, Some(11), no_progress)
        .await
        .unwrap();
    assert_eq!(0, t.server.request_count().await);
    assert_eq!("hello world", read_text(&dest));
}

#[tokio::test(start_paused = true)]
async fn download_when_the_server_ignores_the_range_restarts_from_scratch() {
    let t = env().await;
    let dest = t.file_with("a.part", "hello ");
    t.server
        .enqueue(ResponseTemplate::new(200).set_body_string("hello world"));
    t.api
        .download(ACCOUNT, "f1", &dest, Some(11), no_progress)
        .await
        .unwrap();
    assert_eq!(Some("bytes=6-".into()), header(&t.server.take().await, "Range"));
    assert_eq!("hello world", read_text(&dest));
}

#[tokio::test(start_paused = true)]
async fn download_range_not_satisfiable_is_treated_as_complete() {
    let t = env().await;
    let dest = t.file_with("a.part", "hello world");
    t.server.enqueue(ResponseTemplate::new(416));
    t.api
        .download(ACCOUNT, "f1", &dest, None, no_progress)
        .await
        .unwrap();
    assert_eq!(Some("bytes=11-".into()), header(&t.server.take().await, "Range"));
    assert_eq!("hello world", read_text(&dest));
}

#[tokio::test(start_paused = true)]
async fn download_a_part_file_larger_than_the_file_is_dropped_and_the_download_starts_over() {
    let t = env().await;
    // Left by an interrupted download of an older, bigger version of the file.
    let dest = t.file_with("a.part", "an older, longer version");
    t.server
        .enqueue(ResponseTemplate::new(200).set_body_string("hello world"));

    t.api
        .download(ACCOUNT, "f1", &dest, Some(11), no_progress)
        .await
        .unwrap();

    assert_eq!(
        None,
        header(&t.server.take().await, "Range"),
        "a stale part file must not be resumed"
    );
    assert_eq!("hello world", read_text(&dest));
}

#[tokio::test(start_paused = true)]
async fn download_range_not_satisfiable_but_the_size_is_wrong_drops_the_part_file() {
    let t = env().await;
    // Otherwise every later run resumes the same stale bytes and fails the same way.
    let dest = t.file_with("a.part", "hello");
    t.server.enqueue(ResponseTemplate::new(416));

    let e = t
        .api
        .download(ACCOUNT, "f1", &dest, Some(11), no_progress)
        .await
        .unwrap_err();
    assert!(matches!(e, DriveError::Integrity(_)), "{e:?}");

    assert!(!dest.exists(), "the stale part file is kept and fails every run");
}

#[tokio::test(start_paused = true)]
async fn download_server_error_is_retried_and_resumes_from_what_was_written() {
    let t = env().await;
    let dest = t.file("a.part");
    t.server.enqueue(api_error(503, "backendError", "error"));
    t.server
        .enqueue(ResponseTemplate::new(200).set_body_string("hello world"));
    t.api
        .download(ACCOUNT, "f1", &dest, Some(11), no_progress)
        .await
        .unwrap();
    assert_eq!("hello world", read_text(&dest));
    assert_eq!(2, t.server.request_count().await);
}

#[tokio::test(start_paused = true)]
async fn download_revision_targets_the_revision_media() {
    let t = env().await;
    t.server.enqueue(ResponseTemplate::new(200).set_body_string("v1"));
    let dest = t.file("rev.part");
    t.api
        .download_revision(ACCOUNT, "f1", "r1", &dest, no_progress)
        .await
        .unwrap();
    assert_eq!("v1", read_text(&dest));
    let u = t.server.take().await;
    assert_eq!("/drive/v3/files/f1/revisions/r1", path(&u));
    assert_eq!(Some("media".into()), query(&u, "alt"));
}

#[tokio::test(start_paused = true)]
async fn export_deletes_the_stale_destination_and_never_resumes() {
    let t = env().await;
    let dest = t.file_with("doc.part", "old");
    t.server
        .enqueue(ResponseTemplate::new(200).set_body_string("new-doc"));
    t.api
        .export(ACCOUNT, "f1", "application/pdf", &dest, no_progress)
        .await
        .unwrap();
    let req = t.server.take().await;
    assert_eq!("/drive/v3/files/f1/export", path(&req));
    assert_eq!(Some("application/pdf".into()), query(&req, "mimeType"));
    assert_eq!(None, header(&req, "Range"));
    assert_eq!("new-doc", read_text(&dest));
}

#[tokio::test(start_paused = true)]
async fn read_range_sends_the_range_header_and_returns_the_bytes_read() {
    let t = env().await;
    t.server
        .enqueue(ResponseTemplate::new(206).set_body_string("cdefg"));
    let mut buf = [0u8; 10];
    let n = t.api.read_range(ACCOUNT, "f1", 2, &mut buf[..5]).await.unwrap();
    assert_eq!(5, n);
    assert_eq!(b"cdefg", &buf[..n]);
    let req = t.server.take().await;
    assert_eq!(Some("bytes=2-6".into()), header(&req, "Range"));
    assert_eq!(Some("media".into()), query(&req, "alt"));
}

#[tokio::test(start_paused = true)]
async fn read_range_beyond_the_end_returns_zero() {
    let t = env().await;
    t.server.enqueue(ResponseTemplate::new(416));
    let mut buf = [0u8; 5];
    assert_eq!(0, t.api.read_range(ACCOUNT, "f1", 100, &mut buf).await.unwrap());
}

// ================================================================ uploads: multipart

#[tokio::test(start_paused = true)]
async fn upload_small_file_uses_a_single_multipart_related_request() {
    let t = env().await;
    let src = t.file_with("photo.jpg", "0123456789");
    t.server.enqueue(json_response(&file_json(
        "new1",
        "photo.jpg",
        "image/jpeg",
        None,
        r#","md5Checksum":"x""#,
    )));
    let mut progress = Vec::new();

    let file = t
        .api
        .upload(
            ACCOUNT,
            UploadRequest {
                parent_id: Some("parent1"),
                modified_time_ms: Some(MTIME),
                mime_type: "image/jpeg",
                ..UploadRequest::new(&src, "photo.jpg")
            },
            |d, total| progress.push((d, total)),
        )
        .await
        .unwrap();
    assert_eq!("new1", file.id);

    let req = t.server.take().await;
    assert_eq!("POST", req.method.as_str());
    assert_eq!("/upload/drive/v3/files", path(&req));
    assert_eq!(Some("multipart".into()), query(&req, "uploadType"));
    assert_eq!(Some("true".into()), query(&req, "supportsAllDrives"));
    assert_eq!(Some(FILE_FIELDS.into()), query(&req, "fields"));
    assert_eq!(Some("Bearer tok".into()), header(&req, "Authorization"));
    assert!(
        header(&req, "Content-Type")
            .unwrap()
            .starts_with("multipart/related; boundary=")
    );

    let body = body_text(&req);
    assert!(body.contains("Content-Type: application/json; charset=utf-8"));
    assert!(body.contains("Content-Type: image/jpeg"));
    assert!(
        body.find("application/json").unwrap() < body.find("image/jpeg").unwrap(),
        "metadata part comes first"
    );
    assert!(body.contains("\r\n\r\n0123456789\r\n"));
    let meta = meta_line(&body);
    assert_eq!("photo.jpg", meta["name"]);
    assert_eq!(serde_json::json!(["parent1"]), meta["parents"]);
    assert_eq!("2025-01-02T03:04:05Z", meta["modifiedTime"]);
    assert!(
        meta.get("mimeType").is_none(),
        "the MIME type travels in the media part, not the metadata"
    );
    assert_eq!(Some(&(10, 10)), progress.last());
}

#[tokio::test(start_paused = true)]
async fn upload_small_file_to_an_existing_file_patches_without_parents() {
    let t = env().await;
    let src = t.file_with("photo.jpg", "0123456789");
    t.server.enqueue(json_response(&file_json(
        "e1",
        "photo.jpg",
        "text/plain",
        None,
        "",
    )));
    t.api
        .upload(
            ACCOUNT,
            UploadRequest {
                existing_file_id: Some("e1"),
                ..UploadRequest::new(&src, "photo.jpg")
            },
            no_upload_progress,
        )
        .await
        .unwrap();
    let req = t.server.take().await;
    assert_eq!("PATCH", req.method.as_str());
    assert_eq!("/upload/drive/v3/files/e1", path(&req));
    assert_eq!(Some("multipart".into()), query(&req, "uploadType"));
    let meta = meta_line(&body_text(&req));
    assert_eq!("photo.jpg", meta["name"]);
    assert!(meta.get("parents").is_none());
    assert!(meta.get("modifiedTime").is_none());
}

#[tokio::test(start_paused = true)]
async fn upload_small_file_server_error_is_retried_with_the_full_body() {
    let t = env().await;
    let src = t.file_with("photo.jpg", "0123456789");
    t.server.enqueue(api_error(500, "backendError", "error"));
    t.server.enqueue(json_response(&fj("new1")));
    let file = t
        .api
        .upload(
            ACCOUNT,
            UploadRequest {
                parent_id: Some("parent1"),
                ..UploadRequest::new(&src, "photo.jpg")
            },
            no_upload_progress,
        )
        .await
        .unwrap();
    assert_eq!("new1", file.id);
    assert!(body_text(&t.server.take().await).contains("0123456789"));
    assert!(body_text(&t.server.take().await).contains("0123456789"));
}

// ================================================================ uploads: resumable

#[tokio::test(start_paused = true)]
async fn upload_large_file_starts_a_session_and_sends_chunks_with_content_range() {
    let t = env().await;
    let size = CHUNK + 1000;
    let src = t.big_file("big.bin", size, &[(0, b'A'), (CHUNK, b'B'), (size - 1, b'Z')]);
    let store = FakeSessionStore::default();
    let key = "pair|big.bin|md5";
    t.server.enqueue(t.session_response("/upload/session/s1"));
    t.server
        .enqueue(ResponseTemplate::new(308).insert_header("Range", format!("bytes=0-{}", CHUNK - 1)));
    t.server.enqueue(json_response(&file_json(
        "big1",
        "big.bin",
        "text/plain",
        None,
        "",
    )));
    let mut progress = Vec::new();

    let file = t
        .api
        .upload(
            ACCOUNT,
            UploadRequest {
                parent_id: Some("parent1"),
                session: Some(SessionRef { store: &store, key }),
                ..UploadRequest::new(&src, "big.bin")
            },
            |d, total| progress.push((d, total)),
        )
        .await
        .unwrap();
    assert_eq!("big1", file.id);

    let start = t.server.take().await;
    assert_eq!("POST", start.method.as_str());
    assert_eq!("/upload/drive/v3/files", path(&start));
    assert_eq!(Some("resumable".into()), query(&start, "uploadType"));
    assert_eq!(Some("true".into()), query(&start, "supportsAllDrives"));
    assert_eq!(Some(size.to_string()), header(&start, "X-Upload-Content-Length"));
    assert_eq!(
        Some("application/octet-stream".into()),
        header(&start, "X-Upload-Content-Type")
    );
    assert_eq!(Some("Bearer tok".into()), header(&start, "Authorization"));
    assert!(
        header(&start, "Content-Type")
            .unwrap()
            .starts_with("application/json")
    );
    let meta: Value = serde_json::from_slice(&start.body).unwrap();
    assert_eq!("big.bin", meta["name"]);
    assert_eq!(serde_json::json!(["parent1"]), meta["parents"]);
    assert!(meta.get("modifiedTime").is_none());

    let c1 = t.server.take().await;
    assert_eq!("PUT", c1.method.as_str());
    assert_eq!("/upload/session/s1", path(&c1));
    assert_eq!(
        Some(format!("bytes 0-{}/{size}", CHUNK - 1)),
        header(&c1, "Content-Range")
    );
    assert_eq!(Some("Bearer tok".into()), header(&c1, "Authorization"));
    assert_eq!(CHUNK as usize, c1.body.len());
    assert_eq!(b'A', c1.body[0]);

    let c2 = t.server.take().await;
    assert_eq!("PUT", c2.method.as_str());
    assert_eq!(
        Some(format!("bytes {CHUNK}-{}/{size}", size - 1)),
        header(&c2, "Content-Range")
    );
    assert_eq!(1000, c2.body.len());
    assert_eq!(b'B', c2.body[0]);
    assert_eq!(b'Z', c2.body[999]);

    assert_eq!(
        vec![
            format!("get:{ACCOUNT}|{key}"),
            format!("put:{ACCOUNT}|{key}"),
            format!("remove:{ACCOUNT}|{key}")
        ],
        store.ops()
    );
    assert!(store.map().is_empty());
    assert_eq!(vec![(CHUNK, size), (size, size)], progress);
}

#[tokio::test(start_paused = true)]
async fn upload_large_file_persists_the_session_uri_before_sending_data() {
    let t = env().await;
    let size = MULTIPART_LIMIT + 1;
    let src = t.big_file("big.bin", size, &[]);
    let store = FakeSessionStore::default();
    t.server.enqueue(t.session_response("/upload/session/s1"));
    t.server.enqueue(api_error(404, "notFound", "Not Found")); // chunk PUT: session vanished
    let e = t
        .api
        .upload(
            ACCOUNT,
            UploadRequest {
                parent_id: Some("parent1"),
                session: Some(SessionRef {
                    store: &store,
                    key: "k",
                }),
                ..UploadRequest::new(&src, "big.bin")
            },
            no_upload_progress,
        )
        .await
        .unwrap_err();
    assert!(matches!(e, DriveError::NotFound(_)), "{e:?}");
    assert_eq!("upload session expired", e.to_string());
    assert_eq!(Some(&t.server.url("/upload/session/s1")), store.map().get(K));
    assert_eq!(vec!["get:me@example.com|k", "put:me@example.com|k"], store.ops());
}

#[tokio::test(start_paused = true)]
async fn upload_large_file_when_the_server_acknowledges_less_continues_from_the_acknowledged_offset() {
    let t = env().await;
    let size = MULTIPART_LIMIT + 1; // a single chunk normally
    let ack: u64 = 1_048_576;
    let src = t.big_file("big.bin", size, &[(ack, b'M'), (size - 1, b'Z')]);
    t.server.enqueue(t.session_response("/upload/session/s1"));
    t.server
        .enqueue(ResponseTemplate::new(308).insert_header("Range", format!("bytes=0-{}", ack - 1)));
    t.server.enqueue(json_response(&fj("big1")));

    let file = t
        .api
        .upload(
            ACCOUNT,
            UploadRequest {
                parent_id: Some("parent1"),
                ..UploadRequest::new(&src, "big.bin")
            },
            no_upload_progress,
        )
        .await
        .unwrap();
    assert_eq!("big1", file.id);

    t.server.take().await; // session start
    let c1 = t.server.take().await;
    assert_eq!(
        Some(format!("bytes 0-{}/{size}", size - 1)),
        header(&c1, "Content-Range")
    );
    assert_eq!(size as usize, c1.body.len());
    let c2 = t.server.take().await;
    assert_eq!(
        Some(format!("bytes {ack}-{}/{size}", size - 1)),
        header(&c2, "Content-Range")
    );
    assert_eq!((size - ack) as usize, c2.body.len());
    assert_eq!(b'M', c2.body[0]);
}

#[tokio::test(start_paused = true)]
async fn upload_large_file_resumes_an_existing_session_from_the_server_reported_offset() {
    let t = env().await;
    let size = MULTIPART_LIMIT + 1;
    let offset: u64 = 2_097_152;
    let src = t.big_file("big.bin", size, &[(offset, b'M')]);
    let store = FakeSessionStore::with(K, &t.server.url("/upload/session/old"));
    t.server
        .enqueue(ResponseTemplate::new(308).insert_header("Range", format!("bytes=0-{}", offset - 1)));
    t.server.enqueue(json_response(&fj("big1")));

    let file = t
        .api
        .upload(
            ACCOUNT,
            UploadRequest {
                parent_id: Some("parent1"),
                session: Some(SessionRef {
                    store: &store,
                    key: "k",
                }),
                ..UploadRequest::new(&src, "big.bin")
            },
            no_upload_progress,
        )
        .await
        .unwrap();
    assert_eq!("big1", file.id);

    let q = t.server.take().await;
    assert_eq!("PUT", q.method.as_str());
    assert_eq!("/upload/session/old", path(&q));
    assert_eq!(Some(format!("bytes */{size}")), header(&q, "Content-Range"));
    assert_eq!(0, q.body.len());
    // Google answers a resume query without it with 411 Length Required, so an interrupted
    // upload could never finish (E2E 2026-10).
    assert_eq!(Some("0".into()), header(&q, "Content-Length"));
    assert_eq!(Some("Bearer tok".into()), header(&q, "Authorization"));

    let chunk = t.server.take().await;
    assert_eq!("/upload/session/old", path(&chunk));
    assert_eq!(
        Some(format!("bytes {offset}-{}/{size}", size - 1)),
        header(&chunk, "Content-Range")
    );
    assert_eq!((size - offset) as usize, chunk.body.len());
    assert_eq!(b'M', chunk.body[0]);

    assert_eq!(2, t.server.request_count().await, "no new session was started");
    assert_eq!(
        vec!["get:me@example.com|k", "remove:me@example.com|k"],
        store.ops()
    );
    assert!(store.map().is_empty());
}

#[tokio::test(start_paused = true)]
async fn upload_large_file_expired_session_is_forgotten_and_a_new_one_started() {
    let t = env().await;
    let size = MULTIPART_LIMIT + 1;
    let src = t.big_file("big.bin", size, &[]);
    let store = FakeSessionStore::with(K, &t.server.url("/upload/session/old"));
    t.server.enqueue(ResponseTemplate::new(404)); // status query: gone
    t.server.enqueue(t.session_response("/upload/session/new"));
    t.server.enqueue(json_response(&fj("big1")));

    let file = t
        .api
        .upload(
            ACCOUNT,
            UploadRequest {
                parent_id: Some("parent1"),
                session: Some(SessionRef {
                    store: &store,
                    key: "k",
                }),
                ..UploadRequest::new(&src, "big.bin")
            },
            no_upload_progress,
        )
        .await
        .unwrap();
    assert_eq!("big1", file.id);

    assert_eq!("/upload/session/old", path(&t.server.take().await));
    let start = t.server.take().await;
    assert_eq!("POST", start.method.as_str());
    assert_eq!("/upload/drive/v3/files", path(&start));
    let chunk = t.server.take().await;
    assert_eq!("/upload/session/new", path(&chunk));
    assert_eq!(
        Some(format!("bytes 0-{}/{size}", size - 1)),
        header(&chunk, "Content-Range")
    );
    assert_eq!(
        vec![
            "get:me@example.com|k",
            "remove:me@example.com|k",
            "put:me@example.com|k",
            "remove:me@example.com|k"
        ],
        store.ops()
    );
}

#[tokio::test(start_paused = true)]
async fn upload_large_file_existing_session_already_complete_returns_without_sending_data() {
    let t = env().await;
    let size = MULTIPART_LIMIT + 1;
    let src = t.big_file("big.bin", size, &[]);
    let store = FakeSessionStore::with(K, &t.server.url("/upload/session/old"));
    t.server.enqueue(json_response(&fj("done1")));
    let file = t
        .api
        .upload(
            ACCOUNT,
            UploadRequest {
                parent_id: Some("parent1"),
                session: Some(SessionRef {
                    store: &store,
                    key: "k",
                }),
                ..UploadRequest::new(&src, "big.bin")
            },
            no_upload_progress,
        )
        .await
        .unwrap();
    assert_eq!("done1", file.id);
    assert_eq!(1, t.server.request_count().await);
    assert_eq!(
        Some(format!("bytes */{size}")),
        header(&t.server.take().await, "Content-Range")
    );
    assert_eq!(
        vec!["get:me@example.com|k", "remove:me@example.com|k"],
        store.ops()
    );
}

#[tokio::test(start_paused = true)]
async fn upload_large_file_to_an_existing_file_starts_the_patch_session() {
    let t = env().await;
    let size = MULTIPART_LIMIT + 1;
    let src = t.big_file("big.bin", size, &[]);
    t.server.enqueue(t.session_response("/upload/session/s1"));
    t.server.enqueue(json_response(&fj("e1")));
    t.api
        .upload(
            ACCOUNT,
            UploadRequest {
                existing_file_id: Some("e1"),
                modified_time_ms: Some(MTIME),
                ..UploadRequest::new(&src, "big.bin")
            },
            no_upload_progress,
        )
        .await
        .unwrap();
    let start = t.server.take().await;
    assert_eq!("PATCH", start.method.as_str());
    assert_eq!("/upload/drive/v3/files/e1", path(&start));
    assert_eq!(Some("resumable".into()), query(&start, "uploadType"));
    let meta: Value = serde_json::from_slice(&start.body).unwrap();
    assert_eq!("big.bin", meta["name"]);
    assert_eq!("2025-01-02T03:04:05Z", meta["modifiedTime"]);
    assert!(meta.get("parents").is_none());
    assert_eq!(
        Some(format!("bytes 0-{}/{size}", size - 1)),
        header(&t.server.take().await, "Content-Range")
    );
}

#[tokio::test(start_paused = true)]
async fn upload_large_file_session_start_without_location_fails() {
    let t = env().await;
    let src = t.big_file("big.bin", MULTIPART_LIMIT + 1, &[]);
    t.server.enqueue(ResponseTemplate::new(200));
    let e = t
        .api
        .upload(
            ACCOUNT,
            UploadRequest {
                parent_id: Some("parent1"),
                ..UploadRequest::new(&src, "big.bin")
            },
            no_upload_progress,
        )
        .await
        .unwrap_err();
    assert!(matches!(e, DriveError::Http { .. }), "{e:?}");
    assert!(e.to_string().contains("Location"));
}

#[tokio::test(start_paused = true)]
async fn upload_large_file_transient_chunk_failure_is_retried_on_the_same_session() {
    let t = env().await;
    let size = MULTIPART_LIMIT + 1;
    let src = t.big_file("big.bin", size, &[]);
    t.server.enqueue(t.session_response("/upload/session/s1"));
    t.server.enqueue(api_error(503, "backendError", "error"));
    t.server.enqueue(json_response(&fj("big1")));
    let file = t
        .api
        .upload(
            ACCOUNT,
            UploadRequest {
                parent_id: Some("parent1"),
                ..UploadRequest::new(&src, "big.bin")
            },
            no_upload_progress,
        )
        .await
        .unwrap();
    assert_eq!("big1", file.id);
    t.server.take().await;
    assert_eq!("/upload/session/s1", path(&t.server.take().await));
    let retry = t.server.take().await;
    assert_eq!("/upload/session/s1", path(&retry));
    assert_eq!(
        Some(format!("bytes 0-{}/{size}", size - 1)),
        header(&retry, "Content-Range")
    );
    assert!(t.elapsed_ms() >= 1000);
}

// ================================================================ revisions

#[tokio::test(start_paused = true)]
async fn list_revisions_paginates_and_parses() {
    let t = env().await;
    t.server.enqueue(json_response(
        r#"{"nextPageToken":"n","revisions":[{"id":"1","modifiedTime":"2026-01-02T03:04:05.000Z","size":"5","md5Checksum":"x","keepForever":true,"originalFilename":"a.txt","mimeType":"text/plain","lastModifyingUser":{"displayName":"A","emailAddress":"a@b.c"}}]}"#,
    ));
    t.server.enqueue(json_response(r#"{"revisions":[{"id":"2"}]}"#));
    let revs = t.api.list_revisions(ACCOUNT, "f1").await.unwrap();
    assert_eq!(
        vec!["1", "2"],
        revs.iter().map(|r| r.id.as_str()).collect::<Vec<_>>()
    );
    assert!(revs[0].keep_forever);
    assert_eq!(Some("5"), revs[0].size.as_deref());
    assert_eq!(Some("x"), revs[0].md5_checksum.as_deref());
    assert_eq!(Some("a.txt"), revs[0].original_filename.as_deref());
    assert_eq!(
        Some("A"),
        revs[0]
            .last_modifying_user
            .as_ref()
            .unwrap()
            .display_name
            .as_deref()
    );
    assert!(!revs[1].keep_forever);
    assert!(revs[1].last_modifying_user.is_none());

    let r1 = t.server.take().await;
    assert_eq!("/drive/v3/files/f1/revisions", path(&r1));
    assert_eq!(Some("200".into()), query(&r1, "pageSize"));
    assert_eq!(None, query(&r1, "pageToken"));
    assert!(
        query(&r1, "fields")
            .unwrap()
            .contains("revisions(id,modifiedTime,size,md5Checksum,keepForever")
    );
    assert_eq!(Some("n".into()), query(&t.server.take().await, "pageToken"));
}

#[tokio::test(start_paused = true)]
async fn keep_revision_patches_keep_forever() {
    let t = env().await;
    t.server.enqueue(json_response("{}"));
    t.api.keep_revision(ACCOUNT, "f1", "r9", true).await.unwrap();
    let req = t.server.take().await;
    assert_eq!("PATCH", req.method.as_str());
    assert_eq!("/drive/v3/files/f1/revisions/r9", path(&req));
    assert!(
        header(&req, "Content-Type")
            .unwrap()
            .starts_with("application/json")
    );
    assert_eq!(r#"{"keepForever":true}"#, body_text(&req));

    t.server.enqueue(json_response("{}"));
    t.api.keep_revision(ACCOUNT, "f1", "r9", false).await.unwrap();
    assert_eq!(r#"{"keepForever":false}"#, body_text(&t.server.take().await));
}

// ================================================================ folders and metadata patches

#[tokio::test(start_paused = true)]
async fn create_folder_posts_folder_metadata() {
    let t = env().await;
    t.server.enqueue(json_response(&file_json(
        "d1",
        "New",
        FOLDER_MIME,
        Some("p1"),
        "",
    )));
    let f = t.api.create_folder(ACCOUNT, "p1", "New").await.unwrap();
    assert_eq!("d1", f.id);
    assert!(f.is_folder());
    let req = t.server.take().await;
    assert_eq!("POST", req.method.as_str());
    assert_eq!("/drive/v3/files", path(&req));
    assert_eq!(Some(FILE_FIELDS.into()), query(&req, "fields"));
    assert_eq!(Some("true".into()), query(&req, "supportsAllDrives"));
    assert!(
        header(&req, "Content-Type")
            .unwrap()
            .starts_with("application/json")
    );
    let meta: Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!("New", meta["name"]);
    assert_eq!(FOLDER_MIME, meta["mimeType"]);
    assert_eq!(serde_json::json!(["p1"]), meta["parents"]);
    assert!(meta.get("modifiedTime").is_none());
}

#[tokio::test(start_paused = true)]
async fn trash_untrash_rename_send_minimal_patch_bodies() {
    let t = env().await;
    for _ in 0..3 {
        t.server.enqueue(json_response(&fj("f1")));
    }

    t.api.trash(ACCOUNT, "f1").await.unwrap();
    let tr = t.server.take().await;
    assert_eq!("PATCH", tr.method.as_str());
    assert_eq!("/drive/v3/files/f1", path(&tr));
    assert_eq!(Some(FILE_FIELDS.into()), query(&tr, "fields"));
    assert_eq!(Some("true".into()), query(&tr, "supportsAllDrives"));
    assert_eq!(None, query(&tr, "addParents"));
    assert_eq!(None, query(&tr, "removeParents"));
    assert_eq!(r#"{"trashed":true}"#, body_text(&tr));

    t.api.untrash(ACCOUNT, "f1").await.unwrap();
    assert_eq!(r#"{"trashed":false}"#, body_text(&t.server.take().await));

    t.api.rename(ACCOUNT, "f1", "renamed.txt").await.unwrap();
    assert_eq!(r#"{"name":"renamed.txt"}"#, body_text(&t.server.take().await));
}

#[tokio::test(start_paused = true)]
async fn move_uses_add_parents_and_remove_parents() {
    let t = env().await;
    t.server.enqueue(json_response(&fj("f1")));
    t.api.move_file(ACCOUNT, "f1", "from", "to").await.unwrap();
    let req = t.server.take().await;
    assert_eq!("PATCH", req.method.as_str());
    assert_eq!(Some("to".into()), query(&req, "addParents"));
    assert_eq!(Some("from".into()), query(&req, "removeParents"));
    assert_eq!("{}", body_text(&req));
}

#[tokio::test(start_paused = true)]
async fn patch_with_modified_time_sends_rfc3339() {
    let t = env().await;
    t.server.enqueue(json_response(&fj("f1")));
    t.api
        .patch(
            ACCOUNT,
            "f1",
            &PatchFileMetadata {
                modified_time: Some(to_rfc3339(MTIME)),
                ..PatchFileMetadata::default()
            },
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        r#"{"modifiedTime":"2025-01-02T03:04:05Z"}"#,
        body_text(&t.server.take().await)
    );
}

#[tokio::test(start_paused = true)]
async fn delete_permanently_sends_delete() {
    let t = env().await;
    t.server.enqueue(ResponseTemplate::new(204));
    t.api.delete_permanently(ACCOUNT, "f1").await.unwrap();
    let req = t.server.take().await;
    assert_eq!("DELETE", req.method.as_str());
    assert_eq!("/drive/v3/files/f1", path(&req));
    assert_eq!(Some("true".into()), query(&req, "supportsAllDrives"));
}

// ================================================================ Rust-only additions

// ---- session URI trust: the bearer token only ever goes to the configured upload host

fn big_upload<'a>(src: &'a Path, store: &'a FakeSessionStore) -> UploadRequest<'a> {
    UploadRequest {
        parent_id: Some("parent1"),
        session: Some(SessionRef { store, key: "k" }),
        ..UploadRequest::new(src, "big.bin")
    }
}

#[tokio::test(start_paused = true)]
async fn upload_large_file_stored_session_on_another_host_is_discarded_without_a_request() {
    let t = env().await;
    let evil = MockDrive::start().await;
    let src = t.big_file("big.bin", MULTIPART_LIMIT + 1, &[]);
    let store = FakeSessionStore::with(K, &evil.url("/upload/session/stolen"));
    t.server.enqueue(t.session_response("/upload/session/new"));
    t.server.enqueue(json_response(&fj("big1")));

    let file = t
        .api
        .upload(ACCOUNT, big_upload(&src, &store), no_upload_progress)
        .await
        .unwrap();

    assert_eq!("big1", file.id);
    assert_eq!(0, evil.request_count().await, "the token went to a foreign host");
    let start = t.server.take().await;
    assert_eq!("POST", start.method.as_str(), "a new session is started instead");
    assert_eq!("/upload/drive/v3/files", path(&start));
    assert_eq!("/upload/session/new", path(&t.server.take().await));
    assert_eq!(
        vec![
            format!("get:{K}"),
            format!("remove:{K}"),
            format!("put:{K}"),
            format!("remove:{K}")
        ],
        store.ops()
    );
}

#[tokio::test(start_paused = true)]
async fn upload_large_file_session_location_on_another_host_is_rejected() {
    let t = env().await;
    let evil = MockDrive::start().await;
    let src = t.big_file("big.bin", MULTIPART_LIMIT + 1, &[]);
    let store = FakeSessionStore::default();
    t.server.enqueue(
        ResponseTemplate::new(200).insert_header("Location", evil.url("/upload/session/s1").as_str()),
    );

    let e = t
        .api
        .upload(ACCOUNT, big_upload(&src, &store), no_upload_progress)
        .await
        .unwrap_err();

    match &e {
        DriveError::Http { reason, .. } => assert_eq!(Some("untrustedSessionUri"), reason.as_deref()),
        other => panic!("expected Http, got {other:?}"),
    }
    assert!(!e.is_transient());
    assert_eq!(0, evil.request_count().await, "the token went to a foreign host");
    assert_eq!(1, t.server.request_count().await);
    assert_eq!(
        vec![format!("get:{K}")],
        store.ops(),
        "an untrusted session is never persisted"
    );
}

#[tokio::test(start_paused = true)]
async fn upload_large_file_never_resumes_a_session_of_another_account() {
    let t = env().await;
    let src = t.big_file("big.bin", MULTIPART_LIMIT + 1, &[]);
    let other = session_store_key("other@example.com", "k");
    let store = FakeSessionStore::with(&other, &t.server.url("/upload/session/theirs"));
    t.server.enqueue(t.session_response("/upload/session/mine"));
    t.server.enqueue(json_response(&fj("big1")));

    t.api
        .upload(ACCOUNT, big_upload(&src, &store), no_upload_progress)
        .await
        .unwrap();

    let start = t.server.take().await;
    assert_eq!("POST", start.method.as_str());
    assert_eq!("/upload/drive/v3/files", path(&start));
    assert_eq!("/upload/session/mine", path(&t.server.take().await));
    assert!(
        store.map().contains_key(&other),
        "the other account's session is left alone"
    );
}

/// The production call timeout must surface as a transient network error, not hang a sync.
#[tokio::test(start_paused = true)]
async fn a_call_that_exceeds_the_call_timeout_is_a_network_error() {
    let server = MockDrive::start().await;
    let tokens = Arc::new(FakeTokens::default());
    for _ in 0..6 {
        server.enqueue(json_response(&fj("abc")).set_delay(Duration::from_secs(600)));
    }
    let http = DriveClient::http_client_builder()
        .pool_idle_timeout(None)
        .build()
        .unwrap();
    let api = DriveClient::with_http(
        http,
        tokens,
        wasl_drive::DriveClientConfig {
            endpoints: server.endpoints(),
            call_timeout: Some(Duration::from_secs(1)),
            ..Default::default()
        },
    );
    let e = api.get_file(ACCOUNT, "abc").await.unwrap_err();
    assert!(matches!(e, DriveError::Network(_)), "{e:?}");
}

#[tokio::test(start_paused = true)]
async fn download_verified_rejects_a_checksum_mismatch_and_drops_the_part_file() {
    let t = env().await;
    t.server
        .enqueue(ResponseTemplate::new(200).set_body_string("hello world"));
    let dest = t.file("a.part");
    let e = t
        .api
        .download_verified(
            ACCOUNT,
            "f1",
            &dest,
            Some(11),
            Some("00000000000000000000000000000000"),
            no_progress,
        )
        .await
        .unwrap_err();
    assert!(matches!(e, DriveError::Integrity(_)), "{e:?}");
    assert!(!dest.exists());

    t.server
        .enqueue(ResponseTemplate::new(200).set_body_string("hello world"));
    t.api
        .download_verified(
            ACCOUNT,
            "f1",
            &dest,
            Some(11),
            Some("5EB63BBBE01EEED093CB22BB8F5ACDC3"),
            no_progress,
        )
        .await
        .unwrap();
    assert_eq!("hello world", read_text(&dest));
}

/// Futures of the public API must be `Send` so the engine can spawn them on a multi-thread runtime.
#[test]
fn public_futures_are_send() {
    fn assert_send<T: Send>(_: &T) {}
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let server = MockDrive::start().await;
        let api = server.client(Arc::new(FakeTokens::default()));
        let p = Path::new("x");
        let store = FakeSessionStore::default();
        let mut buf = [0u8; 4];
        assert_send(&api.get_file(ACCOUNT, "a"));
        assert_send(&api.list_children(ACCOUNT, "a", None));
        assert_send(&api.download(ACCOUNT, "a", p, None, no_progress));
        assert_send(&api.read_range(ACCOUNT, "a", 0, &mut buf));
        assert_send(&api.upload(
            ACCOUNT,
            UploadRequest {
                session: Some(SessionRef {
                    store: &store,
                    key: "k",
                }),
                ..UploadRequest::new(p, "x")
            },
            no_upload_progress,
        ));
    });
}

// ================================================================ security review 2026-10

/// A client with a call timeout against `base` (`upload` beside it).
fn timed_client(tokens: Arc<dyn wasl_drive::TokenProvider>, base: &str, limit: Duration) -> DriveClient {
    let http = DriveClient::http_client_builder()
        .pool_idle_timeout(None)
        .build()
        .unwrap();
    DriveClient::with_http(
        http,
        tokens,
        wasl_drive::DriveClientConfig {
            endpoints: wasl_drive::DriveEndpoints {
                base: format!("{base}/drive/v3"),
                upload: format!("{base}/upload/drive/v3"),
            },
            call_timeout: Some(limit),
            ..Default::default()
        },
    )
}

/// Stored errors end up in the database and the UI: file names (in `q`), page tokens and upload
/// session ids (in the query) must never be in them.
#[tokio::test(start_paused = true)]
async fn network_errors_do_not_carry_the_request_url() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        // Accept and hang up: every request fails in transport.
        while let Ok((sock, _)) = listener.accept().await {
            drop(sock);
        }
    });
    let api = timed_client(
        Arc::new(FakeTokens::default()),
        &format!("http://{addr}"),
        Duration::from_secs(30),
    );
    let e = api
        .list_page(
            ACCOUNT,
            &ListPage {
                q: Some("name = 'secret-name.txt'"),
                page_token: Some("SECRET-PAGE-TOKEN"),
                ..ListPage::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(e, DriveError::Network(_)), "{e:?}");
    let text = format!("{e} {e:?}");
    assert!(!text.contains("secret-name"), "{text}");
    assert!(!text.contains("SECRET-PAGE-TOKEN"), "{text}");
}

#[tokio::test(start_paused = true)]
async fn a_metadata_response_over_16_mib_is_refused() {
    let t = env().await;
    let pad = "x".repeat(16 << 20);
    t.server.enqueue(json_response(&file_json(
        "f1",
        "n",
        "text/plain",
        None,
        &format!(r#","pad":"{pad}""#),
    )));
    let e = t.api.get_file(ACCOUNT, "f1").await.unwrap_err();
    assert!(!e.is_transient(), "{e:?}");
    assert_eq!(1, t.server.request_count().await);
}

#[tokio::test(start_paused = true)]
async fn a_download_stops_once_it_passes_the_expected_size() {
    let t = env().await;
    t.server
        .enqueue(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; 3_000_000]));
    let dest = t.file("a.part");
    let mut most = 0;
    let e = t
        .api
        .download(ACCOUNT, "f1", &dest, Some(10), |d, _| most = most.max(d))
        .await
        .unwrap_err();
    assert!(matches!(e, DriveError::Integrity(_)), "{e:?}");
    assert!(most <= 10, "{most} bytes reported");
    assert!(!dest.exists(), "a part that can never verify is dropped");
}

/// Tokens that never come (a token endpoint that does not answer).
struct StuckTokens;

#[async_trait]
impl wasl_drive::TokenProvider for StuckTokens {
    async fn access_token(&self, _account: &str) -> Result<String, DriveError> {
        std::future::pending().await
    }
    async fn invalidate(&self, _account: &str) {}
}

#[tokio::test(start_paused = true)]
async fn a_token_request_that_never_answers_is_bounded_by_the_call_timeout() {
    let server = MockDrive::start().await;
    let api = timed_client(
        Arc::new(StuckTokens),
        &server.server.uri(),
        Duration::from_secs(30),
    );
    let got = tokio::time::timeout(Duration::from_secs(3600), api.get_file(ACCOUNT, "abc")).await;
    let e = got.expect("the token request was not bounded").unwrap_err();
    assert!(matches!(e, DriveError::Network(_)), "{e:?}");
    assert_eq!(0, server.request_count().await);
}
