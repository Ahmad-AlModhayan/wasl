//! The Drive v3 REST client: request plumbing, retries and the metadata calls (port of the first
//! half of `DriveApi.kt`). Downloads and uploads live in `download.rs` and `upload.rs`.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use reqwest::header::{CONTENT_TYPE, HeaderMap, RETRY_AFTER};
use reqwest::{Method, RequestBuilder, Response, StatusCode, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::limiter::BandwidthLimiter;
use crate::models::{
    About, ChangeList, DriveFile, DriveList, FILE_FIELDS, FOLDER_MIME, FileList, NewFileMetadata,
    PatchFileMetadata, PatchRevision, Revision, RevisionList, SharedDrive, StartPageToken,
};
use crate::{DriveError, Result};

pub(crate) const JSON_UTF8: &str = "application/json; charset=utf-8";
/// The most a metadata or error response is read into memory: Drive's are kilobytes, and a
/// response that never ends must not exhaust memory (security review 2026-10).
pub(crate) const MAX_RESPONSE_BYTES: usize = 16 << 20;

/// Hands out OAuth access tokens. The OAuth flow itself lives outside this crate.
#[async_trait]
pub trait TokenProvider: Send + Sync {
    /// A bearer token for `account`. Return [`DriveError::Network`] for a transient failure (it is
    /// retried like any other) and [`DriveError::Auth`] with `user_action_required` when only the
    /// user can fix it.
    async fn access_token(&self, account: &str) -> Result<String>;

    /// Called when Drive rejected the token: the next [`TokenProvider::access_token`] must not
    /// return the same one.
    async fn invalidate(&self, account: &str);
}

/// Base URLs, overridable in tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveEndpoints {
    pub base: String,
    pub upload: String,
}

impl Default for DriveEndpoints {
    fn default() -> Self {
        DriveEndpoints {
            base: DriveClient::BASE.into(),
            upload: DriveClient::UPLOAD.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DriveClientConfig {
    pub endpoints: DriveEndpoints,
    /// Ceiling for one metadata request (transfers have none: they report progress instead, and
    /// the sync's stall watchdog covers them). `None` disables it, which tests on paused tokio time
    /// need: a pending timer would be auto-advanced while they wait on real socket I/O.
    pub call_timeout: Option<Duration>,
    pub limiter: BandwidthLimiter,
}

impl Default for DriveClientConfig {
    fn default() -> Self {
        DriveClientConfig {
            endpoints: DriveEndpoints::default(),
            call_timeout: Some(DriveClient::CALL_TIMEOUT),
            limiter: BandwidthLimiter::global().clone(),
        }
    }
}

/// Thin, resumable-aware Google Drive v3 REST client. One instance serves all accounts.
#[derive(Clone)]
pub struct DriveClient {
    pub(crate) http: reqwest::Client,
    pub(crate) tokens: Arc<dyn TokenProvider>,
    pub(crate) cfg: DriveClientConfig,
}

/// Paging and scoping of [`DriveClient::list_page`].
#[derive(Debug, Clone)]
pub struct ListPage<'a> {
    pub q: Option<&'a str>,
    pub page_token: Option<&'a str>,
    /// A shared drive to list (`corpora=drive`); `None` lists the user's corpus.
    pub drive_id: Option<&'a str>,
    pub spaces: &'a str,
    pub page_size: u32,
    pub order_by: Option<&'a str>,
}

impl Default for ListPage<'_> {
    fn default() -> Self {
        ListPage {
            q: None,
            page_token: None,
            drive_id: None,
            spaces: "drive",
            page_size: 1000,
            order_by: None,
        }
    }
}

/// Runs `$body` (an expression ending in `.await` that may use the token `$tok`) with the
/// client's retry policy: one token refresh on an auth failure, exponential backoff (or the
/// server's `Retry-After`) on transient failures, at most `$max` attempts. A macro rather than a
/// closure so the body can borrow progress callbacks and buffers of the calling function.
macro_rules! with_retry {
    ($client:expr, $account:expr, $max:expr, |$tok:ident| $body:expr) => {{
        let mut retry = $crate::client::Retry::new($max);
        loop {
            retry.begin();
            // Bounded like a call: a token endpoint that never answers must not hang the sync.
            let res = match $client.timed($client.tokens.access_token($account)).await {
                Ok($tok) => $body,
                Err(e) => Err(e),
            };
            match res {
                Ok(v) => break Ok(v),
                Err(e) => {
                    if let Err(e) = retry.on_error(&$client, $account, e).await {
                        break Err(e);
                    }
                }
            }
        }
    }};
}
pub(crate) use with_retry;

pub(crate) struct Retry {
    attempt: u32,
    max: u32,
    refreshed: bool,
}

impl Retry {
    pub(crate) fn new(max: u32) -> Self {
        Retry {
            attempt: 0,
            max,
            refreshed: false,
        }
    }

    pub(crate) fn begin(&mut self) {
        self.attempt += 1;
    }

    /// `Ok(())` to try again, `Err` to give up with that error.
    pub(crate) async fn on_error(
        &mut self,
        client: &DriveClient,
        account: &str,
        err: DriveError,
    ) -> Result<()> {
        if let DriveError::Auth {
            user_action_required: false,
            ..
        } = err
            && !self.refreshed
        {
            // parity: the refresh retry still counts as one of the `max` attempts, as in Kotlin.
            self.refreshed = true;
            client.tokens.invalidate(account).await;
            return Ok(());
        }
        if !err.is_transient() || self.attempt >= self.max {
            return Err(err);
        }
        let backoff = match &err {
            DriveError::RateLimited { retry_after_ms, .. } if *retry_after_ms > 0 => *retry_after_ms,
            _ => exp_backoff(self.attempt),
        };
        tokio::time::sleep(Duration::from_millis(backoff)).await;
        Ok(())
    }
}

/// Reads a whole response body of at most [`MAX_RESPONSE_BYTES`]; a bigger one is an error.
pub(crate) async fn read_capped(mut resp: Response) -> Result<Bytes> {
    let too_big = || {
        DriveError::Unknown(format!(
            "Google Drive sent a response over {} MiB",
            MAX_RESPONSE_BYTES >> 20
        ))
    };
    if resp
        .content_length()
        .is_some_and(|n| n > MAX_RESPONSE_BYTES as u64)
    {
        return Err(too_big());
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(DriveError::from_reqwest)? {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(too_big());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(body))
}

/// Whether a bearer token may go to `url`: https, or plain http when `allow_http` (tests and
/// debug builds, whose mocks listen on localhost).
fn scheme_allowed(url: &Url, allow_http: bool) -> bool {
    // Plain http only to this machine: a token sent to loopback never crosses a network (the
    // app's binding tests run a release-built engine against a local mock).
    let loopback = match url.host_str() {
        Some(h) => {
            h.eq_ignore_ascii_case("localhost")
                || h.trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|a| a.is_loopback())
        }
        None => false,
    };
    url.scheme() == "https" || (url.scheme() == "http" && (allow_http || loopback))
}

/// 1 s, 2 s, 4 s ... capped at 64 s, plus up to 1 s of jitter.
fn exp_backoff(attempt: u32) -> u64 {
    let base = 1000u64 << attempt.saturating_sub(1).min(6);
    base.min(64_000) + jitter_ms()
}

/// 0..1000 ms without pulling in a RNG crate: the std hasher is randomly keyed per process.
fn jitter_ms() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    h.finish() % 1000
}

/// A fully read response.
pub(crate) struct Exchange {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Exchange {
    pub(crate) fn error(&self) -> DriveError {
        error_for(
            self.status,
            &self.headers,
            Some(&String::from_utf8_lossy(&self.body)),
        )
    }

    /// The response itself when successful, its mapped error otherwise.
    pub(crate) fn ok(self) -> Result<Self> {
        if self.status.is_success() {
            Ok(self)
        } else {
            Err(self.error())
        }
    }

    pub(crate) fn json<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_slice(&self.body).map_err(DriveError::decode)
    }

    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }
}

pub(crate) fn error_for(status: StatusCode, headers: &HeaderMap, body: Option<&str>) -> DriveError {
    DriveError::from_response(
        status.as_u16(),
        status.canonical_reason().unwrap_or(""),
        headers.get(RETRY_AFTER).and_then(|v| v.to_str().ok()),
        body,
    )
}

impl DriveClient {
    pub const BASE: &'static str = "https://www.googleapis.com/drive/v3";
    pub const UPLOAD: &'static str = "https://www.googleapis.com/upload/drive/v3";
    /// Files up to this size go up in one multipart request; bigger ones use resumable sessions.
    pub const MULTIPART_LIMIT: u64 = 5 * 1024 * 1024;
    /// Resumable chunk size; Drive requires a multiple of 256 KiB.
    pub const CHUNK: usize = 8 * 1024 * 1024;
    /// Ceiling for one metadata request; a connection that trickles bytes would otherwise never end.
    pub const CALL_TIMEOUT: Duration = Duration::from_secs(5 * 60);

    /// A `reqwest` builder with the settings Drive needs: no redirects (a resumable upload answers
    /// `308 Resume Incomplete`, which must not be followed) and rustls. Responses are gzip-encoded
    /// when Google offers it (docs/COMPRESSION.md §3.2): listings and change pages of a large
    /// account are JSON that shrinks about tenfold. Google sends gzip only to a user agent that
    /// says "gzip". File content asks for `identity` (see `download`).
    pub fn http_client_builder() -> reqwest::ClientBuilder {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .gzip(true)
            .user_agent(concat!("wasl/", env!("CARGO_PKG_VERSION"), " (gzip)"))
    }

    /// A client with production settings (the Android app's OkHttp timeouts).
    pub fn new(tokens: Arc<dyn TokenProvider>) -> Result<Self> {
        let http = Self::http_client_builder()
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| DriveError::Unknown(format!("cannot build the HTTP client: {e}")))?;
        Ok(Self::with_http(http, tokens, DriveClientConfig::default()))
    }

    /// A client on a caller-built `reqwest::Client` (build it from [`Self::http_client_builder`]).
    pub fn with_http(http: reqwest::Client, tokens: Arc<dyn TokenProvider>, cfg: DriveClientConfig) -> Self {
        DriveClient { http, tokens, cfg }
    }

    pub fn config(&self) -> &DriveClientConfig {
        &self.cfg
    }

    // ---------------------------------------------------------------- core request plumbing

    pub(crate) fn url(&self, path: &str, params: &[(&str, Option<&str>)]) -> Result<Url> {
        Self::build_url(&format!("{}/{path}", self.cfg.endpoints.base), params)
    }

    pub(crate) fn build_url(base: &str, params: &[(&str, Option<&str>)]) -> Result<Url> {
        let mut u =
            Url::parse(base).map_err(|e| DriveError::Unknown(format!("invalid Drive URL {base}: {e}")))?;
        // Every request carries a bearer token. Tests and debug builds talk to local mocks.
        if !scheme_allowed(&u, cfg!(any(test, debug_assertions))) {
            return Err(DriveError::Unknown(format!(
                "refusing to send a Google token over {}: the Drive URL must be https",
                u.scheme()
            )));
        }
        {
            let mut q = u.query_pairs_mut();
            for (k, v) in params {
                if let Some(v) = v {
                    q.append_pair(k, v);
                }
            }
        }
        if u.query() == Some("") {
            u.set_query(None);
        }
        Ok(u)
    }

    pub(crate) fn request(&self, method: Method, url: Url, token: &str) -> RequestBuilder {
        self.http.request(method, url).bearer_auth(token)
    }

    pub(crate) fn json_body<T: Serialize>(rb: RequestBuilder, body: &T) -> Result<RequestBuilder> {
        let bytes = serde_json::to_vec(body)
            .map_err(|e| DriveError::Unknown(format!("cannot encode request: {e}")))?;
        Ok(rb.header(CONTENT_TYPE, JSON_UTF8).body(bytes))
    }

    /// Sends the request; transport failures are network errors.
    pub(crate) async fn send(&self, rb: RequestBuilder) -> Result<Response> {
        rb.send().await.map_err(DriveError::from_reqwest)
    }

    /// Bounds `fut` by the configured call timeout.
    pub(crate) async fn timed<T>(&self, fut: impl Future<Output = Result<T>>) -> Result<T> {
        match self.cfg.call_timeout {
            None => fut.await,
            Some(limit) => tokio::time::timeout(limit, fut).await.unwrap_or_else(|_| {
                Err(DriveError::Network(format!(
                    "Google Drive did not answer within {}s",
                    limit.as_secs()
                )))
            }),
        }
    }

    /// Sends the request and reads the whole response, without a timeout.
    pub(crate) async fn exchange_untimed(&self, rb: RequestBuilder) -> Result<Exchange> {
        let resp = self.send(rb).await?;
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = read_capped(resp).await?;
        Ok(Exchange {
            status,
            headers,
            body,
        })
    }

    /// Sends the request and reads the whole response within the call timeout.
    pub(crate) async fn exchange(&self, rb: RequestBuilder) -> Result<Exchange> {
        self.timed(self.exchange_untimed(rb)).await
    }

    /// A metadata call: success required, JSON body decoded.
    async fn execute<T: DeserializeOwned>(&self, rb: RequestBuilder) -> Result<T> {
        self.exchange(rb).await?.ok()?.json()
    }

    /// A metadata call whose response body does not matter.
    async fn execute_unit(&self, rb: RequestBuilder) -> Result<()> {
        self.exchange(rb).await?.ok().map(|_| ())
    }

    async fn get_json<T: DeserializeOwned>(
        &self,
        account: &str,
        path: &str,
        params: &[(&str, Option<&str>)],
    ) -> Result<T> {
        let url = self.url(path, params)?;
        with_retry!(self, account, 6, |tok| {
            self.execute(self.request(Method::GET, url.clone(), &tok)).await
        })
    }

    // ---------------------------------------------------------------- metadata

    const ABOUT_FIELDS: &'static str =
        "user(emailAddress,displayName,photoLink),storageQuota(limit,usage,usageInDrive,usageInDriveTrash)";

    pub async fn about(&self, account: &str) -> Result<About> {
        self.get_json(account, "about", &[("fields", Some(Self::ABOUT_FIELDS))])
            .await
    }

    /// Used right after interactive sign-in, before the account email is known. Not retried.
    pub async fn about_with_token(&self, access_token: &str) -> Result<About> {
        let url = self.url("about", &[("fields", Some(Self::ABOUT_FIELDS))])?;
        self.execute(self.request(Method::GET, url, access_token)).await
    }

    /// The id of the My Drive root.
    pub async fn root_id(&self, account: &str) -> Result<String> {
        Ok(self.get_file(account, "root").await?.id)
    }

    pub async fn get_file(&self, account: &str, file_id: &str) -> Result<DriveFile> {
        self.get_file_fields(account, file_id, FILE_FIELDS).await
    }

    pub async fn get_file_fields(&self, account: &str, file_id: &str, fields: &str) -> Result<DriveFile> {
        self.get_json(
            account,
            &format!("files/{file_id}"),
            &[("fields", Some(fields)), ("supportsAllDrives", Some("true"))],
        )
        .await
    }

    pub async fn list_page(&self, account: &str, page: &ListPage<'_>) -> Result<FileList> {
        let page_size = page.page_size.to_string();
        let fields = format!("nextPageToken,incompleteSearch,files({FILE_FIELDS})");
        let shared = page.drive_id.is_some();
        self.get_json(
            account,
            "files",
            &[
                ("q", page.q),
                ("pageToken", page.page_token),
                ("pageSize", Some(&page_size)),
                ("fields", Some(&fields)),
                ("spaces", Some(page.spaces)),
                ("supportsAllDrives", Some("true")),
                (
                    "includeItemsFromAllDrives",
                    Some(if shared { "true" } else { "false" }),
                ),
                ("corpora", Some(if shared { "drive" } else { "user" })),
                ("driveId", page.drive_id),
                ("orderBy", page.order_by),
            ],
        )
        .await
    }

    /// All non-trashed children of `parent_id`, every page.
    pub async fn list_children(
        &self,
        account: &str,
        parent_id: &str,
        drive_id: Option<&str>,
    ) -> Result<Vec<DriveFile>> {
        let q = format!("'{parent_id}' in parents and trashed = false");
        self.list_all(account, Some(&q), drive_id, "drive").await
    }

    /// Everything in the app-data folder.
    pub async fn list_app_data(&self, account: &str) -> Result<Vec<DriveFile>> {
        self.list_all(account, None, None, "appDataFolder").await
    }

    async fn list_all(
        &self,
        account: &str,
        q: Option<&str>,
        drive_id: Option<&str>,
        spaces: &str,
    ) -> Result<Vec<DriveFile>> {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let page = self
                .list_page(
                    account,
                    &ListPage {
                        q,
                        page_token: token.as_deref(),
                        drive_id,
                        spaces,
                        ..ListPage::default()
                    },
                )
                .await?;
            out.extend(page.files);
            match page.next_page_token {
                Some(t) => token = Some(t),
                None => return Ok(out),
            }
        }
    }

    pub async fn list_shared_drives(&self, account: &str) -> Result<Vec<SharedDrive>> {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let page: DriveList = self
                .get_json(
                    account,
                    "drives",
                    &[
                        ("pageSize", Some("100")),
                        ("pageToken", token.as_deref()),
                        ("fields", Some("nextPageToken,drives(id,name)")),
                    ],
                )
                .await?;
            out.extend(page.drives);
            match page.next_page_token {
                Some(t) => token = Some(t),
                None => return Ok(out),
            }
        }
    }

    pub async fn start_page_token(&self, account: &str, drive_id: Option<&str>) -> Result<String> {
        let t: StartPageToken = self
            .get_json(
                account,
                "changes/startPageToken",
                &[("supportsAllDrives", Some("true")), ("driveId", drive_id)],
            )
            .await?;
        Ok(t.start_page_token)
    }

    /// One page of the Changes feed. The caller continues with `next_page_token` and stores
    /// `new_start_page_token` once there is no next page.
    pub async fn changes_page(
        &self,
        account: &str,
        page_token: &str,
        drive_id: Option<&str>,
    ) -> Result<ChangeList> {
        let fields = format!(
            "nextPageToken,newStartPageToken,changes(changeType,fileId,removed,driveId,time,file({FILE_FIELDS}))"
        );
        self.get_json(
            account,
            "changes",
            &[
                ("pageToken", Some(page_token)),
                ("pageSize", Some("1000")),
                ("includeRemoved", Some("true")),
                ("supportsAllDrives", Some("true")),
                ("includeItemsFromAllDrives", Some("true")),
                ("driveId", drive_id),
                ("fields", Some(&fields)),
            ],
        )
        .await
    }

    pub async fn create_folder(&self, account: &str, parent_id: &str, name: &str) -> Result<DriveFile> {
        self.create_folder_with(account, parent_id, name, None).await
    }

    /// [`Self::create_folder`] setting `appProperties` in the same request.
    pub async fn create_folder_with(
        &self,
        account: &str,
        parent_id: &str,
        name: &str,
        app_properties: Option<std::collections::BTreeMap<String, String>>,
    ) -> Result<DriveFile> {
        let meta = NewFileMetadata {
            name: name.into(),
            mime_type: Some(FOLDER_MIME.into()),
            parents: Some(vec![parent_id.into()]),
            modified_time: None,
            app_properties,
        };
        let url = self.url(
            "files",
            &[("fields", Some(FILE_FIELDS)), ("supportsAllDrives", Some("true"))],
        )?;
        with_retry!(self, account, 6, |tok| {
            let rb = Self::json_body(self.request(Method::POST, url.clone(), &tok), &meta)?;
            self.execute(rb).await
        })
    }

    pub async fn patch(
        &self,
        account: &str,
        file_id: &str,
        meta: &PatchFileMetadata,
        add_parents: Option<&str>,
        remove_parents: Option<&str>,
    ) -> Result<DriveFile> {
        let url = self.url(
            &format!("files/{file_id}"),
            &[
                ("fields", Some(FILE_FIELDS)),
                ("supportsAllDrives", Some("true")),
                ("addParents", add_parents),
                ("removeParents", remove_parents),
            ],
        )?;
        with_retry!(self, account, 6, |tok| {
            let rb = Self::json_body(self.request(Method::PATCH, url.clone(), &tok), meta)?;
            self.execute(rb).await
        })
    }

    pub async fn trash(&self, account: &str, file_id: &str) -> Result<()> {
        let meta = PatchFileMetadata {
            trashed: Some(true),
            ..PatchFileMetadata::default()
        };
        self.patch(account, file_id, &meta, None, None).await?;
        Ok(())
    }

    pub async fn untrash(&self, account: &str, file_id: &str) -> Result<()> {
        let meta = PatchFileMetadata {
            trashed: Some(false),
            ..PatchFileMetadata::default()
        };
        self.patch(account, file_id, &meta, None, None).await?;
        Ok(())
    }

    pub async fn rename(&self, account: &str, file_id: &str, name: &str) -> Result<()> {
        let meta = PatchFileMetadata {
            name: Some(name.into()),
            ..PatchFileMetadata::default()
        };
        self.patch(account, file_id, &meta, None, None).await?;
        Ok(())
    }

    /// Moves `file_id` from `from_parent` to `to_parent` (Kotlin `move`).
    pub async fn move_file(
        &self,
        account: &str,
        file_id: &str,
        from_parent: &str,
        to_parent: &str,
    ) -> Result<()> {
        self.patch(
            account,
            file_id,
            &PatchFileMetadata::default(),
            Some(to_parent),
            Some(from_parent),
        )
        .await?;
        Ok(())
    }

    /// Deletes for good, bypassing the bin.
    pub async fn delete_permanently(&self, account: &str, file_id: &str) -> Result<()> {
        let url = self.url(
            &format!("files/{file_id}"),
            &[("supportsAllDrives", Some("true"))],
        )?;
        with_retry!(self, account, 6, |tok| {
            self.execute_unit(self.request(Method::DELETE, url.clone(), &tok))
                .await
        })
    }

    // ---------------------------------------------------------------- revisions

    pub async fn list_revisions(&self, account: &str, file_id: &str) -> Result<Vec<Revision>> {
        let path = format!("files/{file_id}/revisions");
        let fields = "nextPageToken,revisions(id,modifiedTime,size,md5Checksum,keepForever,originalFilename,mimeType,lastModifyingUser(displayName,emailAddress))";
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let page: RevisionList = self
                .get_json(
                    account,
                    &path,
                    &[
                        ("pageSize", Some("200")),
                        ("pageToken", token.as_deref()),
                        ("fields", Some(fields)),
                    ],
                )
                .await?;
            out.extend(page.revisions);
            match page.next_page_token {
                Some(t) => token = Some(t),
                None => return Ok(out),
            }
        }
    }

    pub async fn keep_revision(
        &self,
        account: &str,
        file_id: &str,
        revision_id: &str,
        keep: bool,
    ) -> Result<()> {
        let url = self.url(&format!("files/{file_id}/revisions/{revision_id}"), &[])?;
        let body = PatchRevision { keep_forever: keep };
        with_retry!(self, account, 6, |tok| {
            let rb = Self::json_body(self.request(Method::PATCH, url.clone(), &tok), &body)?;
            self.execute_unit(rb).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_builds_send_tokens_over_https_only() {
        let https = Url::parse("https://www.googleapis.com/drive/v3").unwrap();
        let http = Url::parse("http://drive.example.com/drive/v3").unwrap();
        let local = Url::parse("http://127.0.0.1:8080/drive/v3").unwrap();
        let other = Url::parse("ftp://example.com/").unwrap();
        assert!(scheme_allowed(&https, false));
        assert!(!scheme_allowed(&http, false));
        assert!(scheme_allowed(&local, false), "loopback never crosses a network");
        assert!(scheme_allowed(&http, true));
        assert!(!scheme_allowed(&other, true));
    }
}
