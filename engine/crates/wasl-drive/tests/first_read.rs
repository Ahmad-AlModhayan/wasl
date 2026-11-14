//! The first listing of an account does not wait for the full crawl (found on a real 17 GB Drive:
//! the first sync of a small pair waited 84 s for the crawl). The pair's folder tree is read live
//! while the crawl runs in the background; afterwards the cache answers.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wasl_drive::{
    AccountState, DriveClient, DriveClientConfig, DriveEndpoints, DriveError, DriveNodeCache, FOLDER_MIME,
    MemoryNodeStore, TokenProvider,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const ACCOUNT: &str = "me@example.com";

struct Tokens;

#[async_trait::async_trait]
impl TokenProvider for Tokens {
    async fn access_token(&self, _account: &str) -> Result<String, DriveError> {
        Ok("tok".into())
    }
    async fn invalidate(&self, _account: &str) {}
}

fn json(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.into_bytes(), "application/json; charset=utf-8")
}

fn file(id: &str, name: &str, mime: &str, parent: &str, modified: &str) -> String {
    format!(
        r#"{{"id":"{id}","name":"{name}","mimeType":"{mime}","parents":["{parent}"],"modifiedTime":"{modified}","size":"3","md5Checksum":"x","version":"1"}}"#
    )
}

const T1: &str = "2026-01-01T00:00:00.000Z";
const T2: &str = "2026-02-01T00:00:00.000Z";

/// A Drive whose pair folder A holds a.txt, two "dup.txt" (the newer wins), a shortcut and a
/// folder B with b.txt; plus 3 folders elsewhere. The full crawl answers after `crawl_delay`, the
/// change list after half a second.
/// Records the `q` of every files.list call.
async fn drive(crawl_delay: Duration) -> (MockServer, Arc<Mutex<Vec<String>>>) {
    let server = MockServer::builder().start().await;
    let queries = Arc::new(Mutex::new(Vec::new()));
    Mock::given(method("GET"))
        .and(path("/drive/v3/files/root"))
        .respond_with(json(
            r#"{"id":"ROOT","name":"My Drive","mimeType":"application/vnd.google-apps.folder"}"#.into(),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/drive/v3/files/A"))
        .respond_with(json(file("A", "Pair", FOLDER_MIME, "ROOT", T1)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/drive/v3/files/GONE"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_string(r#"{"error":{"code":404,"message":"File not found"}}"#),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/drive/v3/changes/startPageToken"))
        .respond_with(json(r#"{"startPageToken":"7"}"#.into()))
        .mount(&server)
        .await;
    // Slow, so the background read is still applying the change list well after its crawl is
    // stored: a listing then must use the cache, not read live (#85).
    Mock::given(method("GET"))
        .and(path("/drive/v3/changes"))
        .respond_with(
            json(r#"{"newStartPageToken":"7","changes":[]}"#.into()).set_delay(Duration::from_millis(500)),
        )
        .mount(&server)
        .await;
    let in_a = format!(
        "[{},{},{},{},{}]",
        file("a", "a.txt", "text/plain", "A", T1),
        file("d1", "dup.txt", "text/plain", "A", T1),
        file("d2", "dup.txt", "text/plain", "A", T2),
        file("s", "link", "application/vnd.google-apps.shortcut", "A", T1),
        file("B", "B", FOLDER_MIME, "A", T1),
    );
    let in_b = format!("[{}]", file("b", "b.txt", "text/plain", "B", T1));
    let everything = format!(
        "[{},{},{},{},{},{},{},{},{},{}]",
        file("A", "Pair", FOLDER_MIME, "ROOT", T1),
        file("a", "a.txt", "text/plain", "A", T1),
        file("d1", "dup.txt", "text/plain", "A", T1),
        file("d2", "dup.txt", "text/plain", "A", T2),
        file("s", "link", "application/vnd.google-apps.shortcut", "A", T1),
        file("B", "B", FOLDER_MIME, "A", T1),
        file("b", "b.txt", "text/plain", "B", T1),
        file("x", "X", FOLDER_MIME, "ROOT", T1),
        file("y", "Y", FOLDER_MIME, "ROOT", T1),
        file("z", "Z", FOLDER_MIME, "ROOT", T1),
    );
    let q = queries.clone();
    Mock::given(method("GET"))
        .and(path("/drive/v3/files"))
        .respond_with(move |req: &Request| {
            let query = req
                .url
                .query_pairs()
                .find(|(k, _)| k == "q")
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default();
            q.lock().unwrap().push(query.clone());
            let body = if query.contains("'A' in parents") {
                in_a.clone()
            } else if query.contains("'B' in parents") {
                in_b.clone()
            } else if query.contains("in parents") {
                "[]".into()
            } else {
                return json(format!(r#"{{"files":{everything}}}"#)).set_delay(crawl_delay);
            };
            json(format!(r#"{{"files":{body}}}"#))
        })
        .mount(&server)
        .await;

    (server, queries)
}

fn cache(server: &MockServer) -> (Arc<DriveNodeCache>, Arc<MemoryNodeStore>) {
    let http = DriveClient::http_client_builder()
        .pool_idle_timeout(None)
        .build()
        .unwrap();
    let api = Arc::new(DriveClient::with_http(
        http,
        Arc::new(Tokens),
        DriveClientConfig {
            endpoints: DriveEndpoints {
                base: format!("{}/drive/v3", server.uri()),
                upload: format!("{}/upload/drive/v3", server.uri()),
            },
            call_timeout: None,
            ..DriveClientConfig::default()
        },
    ));
    let store = Arc::new(MemoryNodeStore::new());
    store.set_account(AccountState::new(ACCOUNT));
    (Arc::new(DriveNodeCache::new(api, store.clone())), store)
}

fn keys(l: &wasl_drive::SubtreeListing) -> Vec<String> {
    let mut k: Vec<String> = l.entries.iter().map(|e| e.key.clone()).collect();
    k.sort();
    k
}

#[tokio::test(flavor = "multi_thread")]
async fn the_first_listing_reads_the_pair_live_while_the_full_crawl_runs() {
    let (server, queries) = drive(Duration::from_secs(3)).await;
    let (cache, store) = cache(&server);

    let started = Instant::now();
    let live = cache.list_subtree(ACCOUNT, "A", None).await.unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "did not wait for the crawl: {:?}",
        started.elapsed()
    );
    assert_eq!(vec!["B", "B/b.txt", "a.txt", "dup.txt"], keys(&live));
    assert_eq!(
        vec!["dup.txt".to_string()],
        live.duplicates,
        "the older same-named file loses"
    );
    let dup = live.entries.iter().find(|e| e.key == "dup.txt").unwrap();
    assert_eq!("d2", dup.id, "the newer one wins");
    assert_eq!(vec!["link".to_string()], live.shortcuts);

    // The crawl finishes in the background; then the cache answers, with no per-folder queries.
    let done = tokio::time::timeout(Duration::from_secs(10), async {
        while store
            .account_state(ACCOUNT)
            .and_then(|a| a.changes_token)
            .is_none()
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(done.is_ok(), "the background crawl completed");
    let asked = queries.lock().unwrap().len();
    let cached = cache.list_subtree(ACCOUNT, "A", None).await.unwrap();
    assert_eq!(keys(&live), keys(&cached), "live and cached listings agree");
    assert_eq!(live.duplicates, cached.duplicates);
    assert_eq!(live.shortcuts, cached.shortcuts);
    let after: Vec<String> = queries.lock().unwrap()[asked..].to_vec();
    assert!(after.iter().all(|q| !q.contains("in parents")), "{after:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pair_folder_that_is_gone_is_reported_on_the_live_path_too() {
    let (server, _) = drive(Duration::from_secs(3)).await;
    let (cache, _) = cache(&server);
    let e = cache.list_subtree(ACCOUNT, "GONE", None).await.unwrap_err();
    assert!(matches!(e, DriveError::NotFound(_)), "{e:?}");
}

/// Whether a folder is a vault, or empty, needs only its own items. Before the account's first
/// read that used to walk the whole folder live, which on a big folder outlasted the vault check
/// before a new pair ("Can't reach Google Drive", 2026-10).
#[tokio::test(flavor = "multi_thread")]
async fn the_top_level_is_one_folder_read_live_and_agrees_with_the_cache() {
    let (server, queries) = drive(Duration::from_secs(3)).await;
    let (cache, store) = cache(&server);

    let live = cache.list_top_level(ACCOUNT, "A", None).await.unwrap();
    assert_eq!(
        vec!["B", "a.txt", "dup.txt"],
        keys(&live),
        "B's own b.txt is not listed"
    );
    assert_eq!(vec!["dup.txt".to_string()], live.duplicates);
    assert_eq!(vec!["link".to_string()], live.shortcuts);
    let folders: Vec<String> = queries
        .lock()
        .unwrap()
        .iter()
        .filter(|q| q.contains("in parents"))
        .cloned()
        .collect();
    assert_eq!(1, folders.len(), "one folder read: {folders:?}");
    assert!(folders[0].contains("'A' in parents"), "{folders:?}");

    let e = cache.list_top_level(ACCOUNT, "GONE", None).await.unwrap_err();
    assert!(matches!(e, DriveError::NotFound(_)), "{e:?}");

    // Once the crawl is done the cache answers the same, with no per-folder queries.
    let done = tokio::time::timeout(Duration::from_secs(10), async {
        while store
            .account_state(ACCOUNT)
            .and_then(|a| a.changes_token)
            .is_none()
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(done.is_ok(), "the background crawl completed");
    let asked = queries.lock().unwrap().len();
    let cached = cache.list_top_level(ACCOUNT, "A", None).await.unwrap();
    assert_eq!(keys(&live), keys(&cached));
    assert_eq!(live.duplicates, cached.duplicates);
    assert_eq!(live.shortcuts, cached.shortcuts);
    let after: Vec<String> = queries.lock().unwrap()[asked..].to_vec();
    assert!(after.iter().all(|q| !q.contains("in parents")), "{after:?}");
}
