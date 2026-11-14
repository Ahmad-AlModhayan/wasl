//! Uploads: one multipart request for small files, the resumable protocol with persisted
//! sessions for big ones (port of the upload half of `DriveApi.kt`).

use std::io::SeekFrom;
use std::path::Path;

use bytes::Bytes;
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE};
use reqwest::{Body, Method, StatusCode, Url};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::mpsc;

use crate::client::{DriveClient, JSON_UTF8, with_retry};
use crate::download::file_name;
use crate::models::{DriveFile, FILE_FIELDS, NewFileMetadata, PatchFileMetadata};
use crate::session::SessionStore;
use crate::time::to_rfc3339;
use crate::{DriveError, Result};

/// Progress of an upload: bytes acknowledged (or streamed) so far, and the file size.
pub type UploadProgress<'a> = dyn FnMut(u64, u64) + Send + 'a;

/// Where a resumable session URI is persisted, and under which key. The store sees the key
/// namespaced by account ([`session_store_key`]), and a stored URI is only used when it points at
/// the configured upload host.
#[derive(Clone, Copy)]
pub struct SessionRef<'a> {
    pub store: &'a dyn SessionStore,
    pub key: &'a str,
}

/// What to upload and where.
#[derive(Clone, Copy)]
pub struct UploadRequest<'a> {
    pub src: &'a Path,
    pub name: &'a str,
    /// Parent of a new file; ignored when `existing_file_id` is set.
    pub parent_id: Option<&'a str>,
    /// Upload as new content of this file instead of creating one.
    pub existing_file_id: Option<&'a str>,
    /// Epoch milliseconds to set as Drive's `modifiedTime`.
    pub modified_time_ms: Option<i64>,
    pub mime_type: &'a str,
    /// Persistence for resumable sessions, so an interrupted big upload continues.
    pub session: Option<SessionRef<'a>>,
    /// Drive `appProperties` to set in the same write.
    pub app_properties: Option<&'a std::collections::BTreeMap<String, String>>,
}

impl<'a> UploadRequest<'a> {
    pub fn new(src: &'a Path, name: &'a str) -> Self {
        UploadRequest {
            src,
            name,
            parent_id: None,
            existing_file_id: None,
            modified_time_ms: None,
            mime_type: "application/octet-stream",
            session: None,
            app_properties: None,
        }
    }
}

enum ChunkResult {
    More(u64),
    Done(Box<DriveFile>),
}

enum ResumeStatus {
    Complete(Box<DriveFile>),
    Partial(u64),
    Expired,
}

/// Size of the slices streamed (and throttled) at a time.
const SLICE: usize = 256 * 1024;

impl DriveClient {
    /// Uploads `req.src` as a new file, or as new content of `req.existing_file_id`. Files up to
    /// [`DriveClient::MULTIPART_LIMIT`] use a single multipart request; bigger ones use the
    /// resumable protocol, persisting the session in `req.session` before any data is sent.
    pub async fn upload(
        &self,
        account: &str,
        req: UploadRequest<'_>,
        mut on_progress: impl FnMut(u64, u64) + Send,
    ) -> Result<DriveFile> {
        let size = tokio::fs::metadata(req.src)
            .await
            .map_err(|e| DriveError::Io(format!("cannot read {}: {e}", file_name(req.src))))?
            .len();
        let modified_time = req.modified_time_ms.map(to_rfc3339);
        let meta = match req.existing_file_id {
            None => serde_json::to_string(&NewFileMetadata {
                name: req.name.into(),
                mime_type: None,
                parents: req.parent_id.map(|p| vec![p.to_string()]),
                modified_time,
                app_properties: req.app_properties.cloned(),
            }),
            Some(_) => serde_json::to_string(&PatchFileMetadata {
                name: Some(req.name.into()),
                trashed: None,
                modified_time,
                app_properties: req.app_properties.cloned(),
            }),
        }
        .map_err(|e| DriveError::Unknown(format!("cannot encode metadata: {e}")))?;

        if size <= Self::MULTIPART_LIMIT {
            self.upload_multipart(account, &req, &meta, &mut on_progress)
                .await
        } else {
            self.upload_resumable(account, &req, &meta, size, &mut on_progress)
                .await
        }
    }

    fn upload_url(&self, existing_file_id: Option<&str>, upload_type: &str) -> Result<Url> {
        let base = match existing_file_id {
            None => format!("{}/files", self.cfg.endpoints.upload),
            Some(id) => format!("{}/files/{id}", self.cfg.endpoints.upload),
        };
        Self::build_url(
            &base,
            &[
                ("uploadType", Some(upload_type)),
                ("supportsAllDrives", Some("true")),
                ("fields", Some(FILE_FIELDS)),
            ],
        )
    }

    fn create_or_update(existing_file_id: Option<&str>) -> Method {
        if existing_file_id.is_none() {
            Method::POST
        } else {
            Method::PATCH
        }
    }

    // ---------------------------------------------------------------- multipart

    async fn upload_multipart(
        &self,
        account: &str,
        req: &UploadRequest<'_>,
        meta: &str,
        on_progress: &mut UploadProgress<'_>,
    ) -> Result<DriveFile> {
        let url = self.upload_url(req.existing_file_id, "multipart")?;
        with_retry!(self, account, 6, |tok| {
            self.multipart_once(&tok, &url, req, meta, &mut *on_progress)
                .await
        })
    }

    /// Streams `multipart/related` (metadata part, then media part) from the file, throttled and
    /// with progress, while the request is in flight.
    async fn multipart_once(
        &self,
        token: &str,
        url: &Url,
        req: &UploadRequest<'_>,
        meta: &str,
        on_progress: &mut UploadProgress<'_>,
    ) -> Result<DriveFile> {
        let read_err = |e: std::io::Error| DriveError::Io(format!("cannot read {}: {e}", file_name(req.src)));
        let mut file = tokio::fs::File::open(req.src).await.map_err(read_err)?;
        let size = file.metadata().await.map_err(read_err)?.len();

        let boundary = boundary();
        let head = format!(
            "--{boundary}\r\nContent-Type: {JSON_UTF8}\r\n\r\n{meta}\r\n--{boundary}\r\nContent-Type: {}\r\n\r\n",
            req.mime_type
        );
        let tail = format!("\r\n--{boundary}--\r\n");
        let length = head.len() as u64 + size + tail.len() as u64;

        let (tx, rx) = mpsc::channel::<std::io::Result<Bytes>>(2);
        let stream =
            futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|item| (item, rx)) });
        let rb = self
            .request(Self::create_or_update(req.existing_file_id), url.clone(), token)
            .header(CONTENT_TYPE, format!("multipart/related; boundary={boundary}"))
            .header(CONTENT_LENGTH, length)
            .body(Body::wrap_stream(stream));

        let feed = async {
            // A send error means the request already ended; its outcome is what matters.
            if tx.send(Ok(Bytes::from(head))).await.is_err() {
                return Ok(());
            }
            let mut buf = vec![0u8; SLICE];
            let mut done = 0u64;
            while done < size {
                let want = SLICE.min((size - done) as usize);
                let n = match file.read(&mut buf[..want]).await {
                    Ok(0) => {
                        let e = std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "the file shrank while it was uploaded",
                        );
                        let err = read_err(std::io::Error::new(e.kind(), e.to_string()));
                        let _ = tx.send(Err(e)).await;
                        return Err(err);
                    }
                    Ok(n) => n,
                    Err(e) => {
                        let err = read_err(std::io::Error::new(e.kind(), e.to_string()));
                        let _ = tx.send(Err(e)).await;
                        return Err(err);
                    }
                };
                self.cfg.limiter.acquire(n).await;
                if tx.send(Ok(Bytes::copy_from_slice(&buf[..n]))).await.is_err() {
                    return Ok(());
                }
                done += n as u64;
                on_progress(done, size);
            }
            let _ = tx.send(Ok(Bytes::from(tail))).await;
            Ok(())
        };
        let (sent, fed) = tokio::join!(self.exchange_untimed(rb), feed);
        // A local read failure explains a failed request better than the aborted transfer does.
        fed?;
        sent?.ok()?.json()
    }

    // ---------------------------------------------------------------- resumable

    async fn upload_resumable(
        &self,
        account: &str,
        req: &UploadRequest<'_>,
        meta: &str,
        size: u64,
        on_progress: &mut UploadProgress<'_>,
    ) -> Result<DriveFile> {
        // Sessions are stored per account: a session URI is only ever resumed with the token of
        // the account that opened it.
        let store_key = req.session.map(|s| session_store_key(account, s.key));
        let forget = async || -> Result<()> {
            if let (Some(s), Some(key)) = (req.session, store_key.as_deref()) {
                s.store.remove(key).await?;
            }
            Ok(())
        };
        let stored = match (req.session, store_key.as_deref()) {
            (Some(s), Some(key)) => s.store.get(key).await?,
            _ => None,
        };
        let mut session_url = None;
        if let Some(uri) = stored {
            match self.trusted_session_url(&uri) {
                // A tampered or corrupted entry: the bearer token never goes there. Start over.
                None => forget().await?,
                Some(url) => session_url = Some(url),
            }
        }
        let mut offset = 0u64;

        if let Some(url) = session_url.clone() {
            match self.query_resumable(account, &url, size).await? {
                ResumeStatus::Complete(f) => {
                    forget().await?;
                    return Ok(*f);
                }
                ResumeStatus::Partial(next) => offset = next,
                ResumeStatus::Expired => {
                    session_url = None;
                    forget().await?;
                }
            }
        }
        let session_url = match session_url {
            Some(u) => u,
            None => {
                let u = self.start_session(account, req, meta, size).await?;
                // Persisted before any data is sent, so an interruption resumes this session.
                if let (Some(s), Some(key)) = (req.session, store_key.as_deref()) {
                    s.store.put(key, u.as_str()).await?;
                }
                offset = 0;
                u
            }
        };

        let read_err = |e: std::io::Error| DriveError::Io(format!("cannot read {}: {e}", file_name(req.src)));
        let mut file = tokio::fs::File::open(req.src).await.map_err(read_err)?;
        while offset < size {
            let len = (Self::CHUNK as u64).min(size - offset) as usize;
            let mut buf = vec![0u8; len];
            file.seek(SeekFrom::Start(offset)).await.map_err(read_err)?;
            file.read_exact(&mut buf).await.map_err(read_err)?;
            let end = offset + len as u64 - 1;
            let body = Bytes::from(buf);
            let mut throttled = 0;
            while throttled < len {
                let slice = SLICE.min(len - throttled);
                self.cfg.limiter.acquire(slice).await;
                throttled += slice;
            }
            let chunk_start = offset;
            let result = with_retry!(self, account, 8, |tok| {
                self.put_chunk(
                    &tok,
                    &session_url,
                    body.clone(),
                    chunk_start,
                    end,
                    size,
                    req.mime_type,
                )
                .await
            })?;
            match result {
                ChunkResult::Done(f) => {
                    forget().await?;
                    on_progress(size, size);
                    return Ok(*f);
                }
                ChunkResult::More(next) => {
                    offset = next;
                    on_progress(offset, size);
                }
            }
        }
        // Fully acknowledged without a final 200: ask for the session's status.
        match self.query_resumable(account, &session_url, size).await? {
            ResumeStatus::Complete(f) => {
                forget().await?;
                Ok(*f)
            }
            _ => Err(DriveError::Http {
                code: 0,
                reason: None,
                message: "resumable upload did not complete".into(),
            }),
        }
    }

    async fn start_session(
        &self,
        account: &str,
        req: &UploadRequest<'_>,
        meta: &str,
        size: u64,
    ) -> Result<Url> {
        let url = self.upload_url(req.existing_file_id, "resumable")?;
        with_retry!(self, account, 6, |tok| {
            let rb = self
                .request(Self::create_or_update(req.existing_file_id), url.clone(), &tok)
                .header("X-Upload-Content-Type", req.mime_type)
                .header("X-Upload-Content-Length", size)
                .header(CONTENT_TYPE, JSON_UTF8)
                .body(meta.to_string());
            self.start_session_once(rb).await
        })
    }

    async fn start_session_once(&self, rb: reqwest::RequestBuilder) -> Result<Url> {
        let ex = self.exchange(rb).await?.ok()?;
        let Some(location) = ex.header("Location") else {
            return Err(DriveError::Http {
                code: ex.status.as_u16(),
                reason: None,
                message: "resumable session without Location".into(),
            });
        };
        // Never follow a session URI off the upload host: every chunk carries the bearer token.
        self.trusted_session_url(location)
            .ok_or_else(|| DriveError::Http {
                code: ex.status.as_u16(),
                reason: Some("untrustedSessionUri".into()),
                message: "resumable session Location is not on the Drive upload host".into(),
            })
    }

    /// `uri` as a URL when it is safe to send the bearer token to: same scheme, host and port as
    /// the configured upload endpoint (https in production). `None` otherwise.
    pub(crate) fn trusted_session_url(&self, uri: &str) -> Option<Url> {
        let base = Url::parse(&self.cfg.endpoints.upload).ok()?;
        let url = Url::parse(uri).ok()?;
        let trusted = url.scheme() == base.scheme()
            && url.host_str().is_some()
            && url.host_str() == base.host_str()
            && url.port_or_known_default() == base.port_or_known_default()
            && url.username().is_empty()
            && url.password().is_none();
        trusted.then_some(url)
    }

    #[allow(clippy::too_many_arguments)]
    async fn put_chunk(
        &self,
        token: &str,
        session: &Url,
        body: Bytes,
        start: u64,
        end: u64,
        size: u64,
        mime_type: &str,
    ) -> Result<ChunkResult> {
        let rb = self
            .request(Method::PUT, session.clone(), token)
            .header("Content-Range", format!("bytes {start}-{end}/{size}"))
            .header(CONTENT_TYPE, mime_type)
            .body(body);
        let ex = self.exchange_untimed(rb).await?;
        match ex.status {
            StatusCode::PERMANENT_REDIRECT => Ok(ChunkResult::More(
                parse_range_end(ex.header("Range")).unwrap_or(end + 1),
            )),
            s if s.is_success() => Ok(ChunkResult::Done(Box::new(ex.json()?))),
            StatusCode::NOT_FOUND | StatusCode::GONE => {
                Err(DriveError::NotFound("upload session expired".into()))
            }
            _ => Err(ex.error()),
        }
    }

    async fn query_resumable(&self, account: &str, url: &Url, size: u64) -> Result<ResumeStatus> {
        with_retry!(self, account, 6, |tok| {
            let rb = self
                .request(Method::PUT, url.clone(), &tok)
                .header("Content-Range", format!("bytes */{size}"))
                // An empty body is sent without a length, and Google refuses that with 411.
                .header(CONTENT_LENGTH, 0)
                .body(Vec::new());
            self.query_resumable_once(rb).await
        })
    }

    async fn query_resumable_once(&self, rb: reqwest::RequestBuilder) -> Result<ResumeStatus> {
        let ex = self.exchange(rb).await?;
        match ex.status {
            StatusCode::PERMANENT_REDIRECT => Ok(ResumeStatus::Partial(
                parse_range_end(ex.header("Range")).unwrap_or(0),
            )),
            s if s.is_success() => Ok(ResumeStatus::Complete(Box::new(ex.json()?))),
            StatusCode::NOT_FOUND | StatusCode::GONE | StatusCode::BAD_REQUEST => Ok(ResumeStatus::Expired),
            _ => Err(ex.error()),
        }
    }
}

/// The [`SessionStore`] key of an upload: the caller's key, namespaced by account.
pub fn session_store_key(account: &str, key: &str) -> String {
    format!("{account}|{key}")
}

/// `"bytes=0-524287"` → 524288, the next offset to send.
fn parse_range_end(range: Option<&str>) -> Option<u64> {
    let (_, end) = range?.split_once('-')?;
    end.trim().parse::<u64>().ok().map(|e| e + 1)
}

fn boundary() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    format!("wasl-{:016x}", h.finish())
}

#[cfg(test)]
mod tests {
    use super::parse_range_end;

    struct NoTokens;

    #[async_trait::async_trait]
    impl crate::TokenProvider for NoTokens {
        async fn access_token(&self, _: &str) -> crate::Result<String> {
            Ok(String::new())
        }
        async fn invalidate(&self, _: &str) {}
    }

    #[test]
    fn only_the_production_upload_host_over_https_is_trusted() {
        let api = crate::DriveClient::with_http(
            reqwest::Client::new(),
            std::sync::Arc::new(NoTokens),
            crate::DriveClientConfig::default(),
        );
        let ok = "https://www.googleapis.com/upload/drive/v3/files?uploadType=resumable&upload_id=x";
        assert!(api.trusted_session_url(ok).is_some());
        assert!(
            api.trusted_session_url("https://www.googleapis.com:443/upload/x")
                .is_some()
        );
        for bad in [
            "http://www.googleapis.com/upload/drive/v3/files?upload_id=x",
            "https://evil.example/upload/drive/v3/files?upload_id=x",
            "https://www.googleapis.com.evil.example/upload",
            "https://www.googleapis.com:8443/upload",
            "https://user:pw@www.googleapis.com/upload",
            "file:///etc/passwd",
            "not a url",
            "",
        ] {
            assert!(api.trusted_session_url(bad).is_none(), "{bad} was trusted");
        }
    }

    #[test]
    fn parses_the_acknowledged_range() {
        assert_eq!(Some(524_288), parse_range_end(Some("bytes=0-524287")));
        assert_eq!(None, parse_range_end(Some("bytes=0-")));
        assert_eq!(None, parse_range_end(None));
    }
}
