//! On-demand reads from Drive: the hydrator asks Drive for byte ranges only, and the bytes come
//! back right.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use wasl_drive::{DriveClient, DriveClientConfig, DriveEndpoints, DriveError, TokenProvider};
use wasl_engine::DriveSource;
use wasl_vfs::{BLOCK, BlockCache, FileRef, Hydrator, ReadAhead};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

struct Tokens;

#[async_trait]
impl TokenProvider for Tokens {
    async fn access_token(&self, _account: &str) -> Result<String, DriveError> {
        Ok("tok".into())
    }
    async fn invalidate(&self, _account: &str) {}
}

fn content(size: u64) -> Vec<u8> {
    (0..size).map(|i| (i * 7 % 251) as u8).collect()
}

/// Serves `alt=media` with Range support and records the ranges asked for.
struct Ranged {
    body: Vec<u8>,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Respond for Ranged {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let range = req
            .headers
            .get("range")
            .map(|v| v.to_str().unwrap().to_owned())
            .unwrap_or_default();
        self.seen.lock().unwrap().push(range.clone());
        let spec = range.trim_start_matches("bytes=");
        let (a, b) = spec.split_once('-').unwrap();
        let a: usize = a.parse().unwrap();
        if a >= self.body.len() {
            return ResponseTemplate::new(416);
        }
        let b: usize = b.parse::<usize>().unwrap().min(self.body.len() - 1);
        ResponseTemplate::new(206).set_body_bytes(self.body[a..=b].to_vec())
    }
}

#[tokio::test]
async fn a_seek_into_a_drive_file_asks_for_one_block() {
    let server = MockServer::start().await;
    let size = 12 * BLOCK + 345;
    let body = content(size);
    let seen = Arc::new(Mutex::new(Vec::new()));
    Mock::given(method("GET"))
        .and(path("/drive/v3/files/F1"))
        .and(query_param("alt", "media"))
        .respond_with(Ranged {
            body: body.clone(),
            seen: seen.clone(),
        })
        .mount(&server)
        .await;
    // After every range read the source checks the file is still the listed version.
    Mock::given(method("GET"))
        .and(path("/drive/v3/files/F1"))
        .and(query_param("fields", "id,version,md5Checksum"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "F1", "version": "7"})),
        )
        .mount(&server)
        .await;
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
    let dir = tempfile::tempdir().unwrap();
    let cache = Arc::new(BlockCache::open(dir.path(), 1 << 30).unwrap());
    let h = Hydrator::new(
        Arc::new(DriveSource::new(client, "me@example.com")),
        cache,
        ReadAhead::default(),
    );
    let f = FileRef {
        id: "F1".into(),
        version: "7".into(),
        size,
    };

    let at = 9 * BLOCK + 10;
    let got = h.read(&f, at, 1000).await.unwrap();
    assert_eq!(&body[at as usize..at as usize + 1000], &got[..]);
    assert_eq!(
        vec![format!("bytes={}-{}", 9 * BLOCK, 10 * BLOCK - 1)],
        *seen.lock().unwrap(),
        "one block, nothing else"
    );

    // The tail, then the whole file once more from the cache plus what is missing.
    let tail = h.read(&f, size - 100, 500).await.unwrap();
    assert_eq!(&body[(size - 100) as usize..], &tail[..]);
    let all = h.read(&f, 0, size).await.unwrap();
    assert_eq!(body, all);

    // Drive now has another version: a block not yet cached is refused, not mixed in.
    Mock::given(method("GET"))
        .and(path("/drive/v3/files/F1"))
        .and(query_param("fields", "id,version,md5Checksum"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "F1", "version": "8"})),
        )
        .with_priority(1)
        .mount(&server)
        .await;
    let g = FileRef {
        id: "F1".into(),
        version: "7".into(),
        size,
    };
    h.cache().forget(&g);
    let err = h.read(&g, 3 * BLOCK, 10).await.unwrap_err();
    assert!(matches!(err, wasl_vfs::VfsError::Integrity(_)), "{err:?}");
}

#[tokio::test]
async fn a_file_shorter_on_drive_than_listed_is_an_error() {
    let server = MockServer::start().await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    Mock::given(method("GET"))
        .and(path("/drive/v3/files/F2"))
        .respond_with(Ranged {
            body: content(10),
            seen,
        })
        .mount(&server)
        .await;
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
    let dir = tempfile::tempdir().unwrap();
    let h = Hydrator::new(
        Arc::new(DriveSource::new(client, "me@example.com")),
        Arc::new(BlockCache::open(dir.path(), 1 << 30).unwrap()),
        ReadAhead::default(),
    );
    let f = FileRef {
        id: "F2".into(),
        version: "1".into(),
        size: 2 * BLOCK,
    };
    assert!(h.read(&f, 0, 100).await.is_err());
}
