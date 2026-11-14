//! Shared doubles for the Drive tests (the Kotlin `FakeTokens`, `MockWebServer` helpers, `fileJson`).
#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use wasl_drive::{DriveClient, DriveClientConfig, DriveEndpoints, DriveError, TokenProvider};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

pub const ACCOUNT: &str = "me@example.com";

/// Token provider double: hands out "tok" until invalidated, then "tok2".
#[derive(Default)]
pub struct FakeTokens {
    state: Mutex<TokenState>,
}

struct TokenState {
    token: String,
    requested: Vec<String>,
    invalidations: usize,
    /// Errors the next token requests return, in order (a flaky network, an unanswered sign-in).
    fail_next: VecDeque<DriveError>,
}

impl Default for TokenState {
    fn default() -> Self {
        TokenState {
            token: "tok".into(),
            requested: Vec::new(),
            invalidations: 0,
            fail_next: VecDeque::new(),
        }
    }
}

impl FakeTokens {
    pub fn requested(&self) -> Vec<String> {
        self.state.lock().unwrap().requested.clone()
    }
    pub fn invalidations(&self) -> usize {
        self.state.lock().unwrap().invalidations
    }
    pub fn fail_next(&self, e: DriveError) {
        self.state.lock().unwrap().fail_next.push_back(e);
    }
}

#[async_trait]
impl TokenProvider for FakeTokens {
    async fn access_token(&self, account: &str) -> Result<String, DriveError> {
        let mut s = self.state.lock().unwrap();
        s.requested.push(account.to_string());
        if let Some(e) = s.fail_next.pop_front() {
            return Err(e);
        }
        Ok(s.token.clone())
    }

    async fn invalidate(&self, _account: &str) {
        let mut s = self.state.lock().unwrap();
        s.invalidations += 1;
        s.token = "tok2".into();
    }
}

/// Answers every request with the next enqueued response, like `MockWebServer.enqueue`.
struct Queue(Arc<Mutex<VecDeque<ResponseTemplate>>>);

impl Respond for Queue {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        self.0
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| ResponseTemplate::new(418).set_body_string("no response enqueued"))
    }
}

/// `MockWebServer` stand-in: a FIFO of responses plus the recorded requests, taken in order.
pub struct MockDrive {
    pub server: MockServer,
    queue: Arc<Mutex<VecDeque<ResponseTemplate>>>,
    taken: Mutex<usize>,
}

impl MockDrive {
    pub async fn start() -> Self {
        // A fresh server per test, not one from wiremock's shared pool: a request still in flight
        // from a finished test could otherwise land on a recycled server and use up this test's
        // queued response.
        let server = MockServer::builder().start().await;
        let queue = Arc::new(Mutex::new(VecDeque::new()));
        Mock::given(any())
            .respond_with(Queue(queue.clone()))
            .mount(&server)
            .await;
        MockDrive {
            server,
            queue,
            taken: Mutex::new(0),
        }
    }

    pub fn enqueue(&self, r: ResponseTemplate) {
        self.queue.lock().unwrap().push_back(r);
    }

    pub async fn request_count(&self) -> usize {
        self.server.received_requests().await.unwrap().len()
    }

    /// The next recorded request (fails the test when there is none).
    pub async fn take(&self) -> Request {
        let all = self.server.received_requests().await.unwrap();
        let mut taken = self.taken.lock().unwrap();
        let r = all.get(*taken).cloned().expect("no request was recorded");
        *taken += 1;
        r
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.server.uri(), path)
    }

    pub fn endpoints(&self) -> DriveEndpoints {
        DriveEndpoints {
            base: self.url("/drive/v3"),
            upload: self.url("/upload/drive/v3"),
        }
    }

    /// A client against this server. No call timeout and no idle-pool timer: under paused tokio
    /// time any pending timer would be auto-advanced while the test waits on real socket I/O.
    pub fn client(&self, tokens: Arc<FakeTokens>) -> DriveClient {
        let http = DriveClient::http_client_builder()
            .pool_idle_timeout(None)
            .build()
            .unwrap();
        DriveClient::with_http(
            http,
            tokens,
            DriveClientConfig {
                endpoints: self.endpoints(),
                call_timeout: None,
                ..DriveClientConfig::default()
            },
        )
    }
}

pub fn json_response(body: &str) -> ResponseTemplate {
    json_response_code(body, 200)
}

pub fn json_response_code(body: &str, code: u16) -> ResponseTemplate {
    ResponseTemplate::new(code).set_body_raw(body.as_bytes().to_vec(), "application/json; charset=utf-8")
}

pub fn api_error(code: u16, reason: &str, message: &str) -> ResponseTemplate {
    json_response_code(
        &format!(
            r#"{{"error":{{"code":{code},"message":"{message}","errors":[{{"domain":"global","reason":"{reason}","message":"{message}"}}]}}}}"#
        ),
        code,
    )
}

pub fn file_json(id: &str, name: &str, mime_type: &str, parent: Option<&str>, extra: &str) -> String {
    let parents = match parent {
        None => String::new(),
        Some(p) => format!(r#","parents":["{p}"]"#),
    };
    format!(r#"{{"id":"{id}","name":"{name}","mimeType":"{mime_type}"{parents}{extra}}}"#)
}

/// `fileJson(id)` with the Kotlin defaults.
pub fn fj(id: &str) -> String {
    file_json(id, "n", "text/plain", None, "")
}

pub fn query(req: &Request, key: &str) -> Option<String> {
    req.url
        .query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

pub fn header(req: &Request, name: &str) -> Option<String> {
    req.headers.get(name).map(|v| v.to_str().unwrap().to_string())
}

pub fn path(req: &Request) -> String {
    req.url.path().to_string()
}

pub fn body_text(req: &Request) -> String {
    String::from_utf8_lossy(&req.body).into_owned()
}
