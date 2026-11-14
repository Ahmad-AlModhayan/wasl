//! Downloads: byte-range resumption into a part file, exports, revision media and arbitrary
//! ranges (port of the download half of `DriveApi.kt`), plus MD5 verification.

use std::io::SeekFrom;
use std::path::Path;

use md5::{Digest, Md5};
use reqwest::header::{ACCEPT_ENCODING, RANGE};
use reqwest::{Method, StatusCode, Url};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::client::{DriveClient, error_for, read_capped, with_retry};
use crate::{DriveError, Result};

/// Progress of a download: bytes in the destination so far, and the total when known.
pub type DownloadProgress<'a> = dyn FnMut(u64, Option<u64>) + Send + 'a;

const REPORT_EVERY: u64 = 1_000_000;

impl DriveClient {
    /// Downloads `file_id` into `dest` (a part file the caller renames), resuming from the bytes
    /// already in `dest`. With `expected_size`, a part file that cannot belong to this version is
    /// dropped and the result's size is verified.
    pub async fn download(
        &self,
        account: &str,
        file_id: &str,
        dest: &Path,
        expected_size: Option<u64>,
        on_progress: impl FnMut(u64, Option<u64>) + Send,
    ) -> Result<()> {
        self.download_flagged(account, file_id, dest, expected_size, on_progress)
            .await
            .map(|_| ())
    }

    /// [`Self::download`], returning whether Drive had flagged the file as malware or spam. Drive
    /// refuses such a file until the request acknowledges the flag, which it accepts only from
    /// the file's owner: the owner's own files sync, and anyone else's still fail with the reason.
    pub async fn download_flagged(
        &self,
        account: &str,
        file_id: &str,
        dest: &Path,
        expected_size: Option<u64>,
        mut on_progress: impl FnMut(u64, Option<u64>) + Send,
    ) -> Result<bool> {
        let media = |acknowledge: bool| {
            let mut params = vec![("alt", Some("media")), ("supportsAllDrives", Some("true"))];
            if acknowledge {
                params.push(("acknowledgeAbuse", Some("true")));
            }
            self.url(&format!("files/{file_id}"), &params)
        };
        match self
            .download_url(
                account,
                media(false)?,
                dest,
                expected_size,
                &mut on_progress,
                true,
            )
            .await
        {
            Err(e) if e.is_flagged_abusive() => self
                .download_url(account, media(true)?, dest, expected_size, &mut on_progress, true)
                .await
                .map(|()| true),
            other => other.map(|()| false),
        }
    }

    /// [`Self::download`], then checks the content against Drive's `md5Checksum`. On a mismatch
    /// the part file is deleted (resuming it could never verify) and [`DriveError::Integrity`]
    /// is returned. Drive reports no checksum for Google docs; pass `None` there.
    pub async fn download_verified(
        &self,
        account: &str,
        file_id: &str,
        dest: &Path,
        expected_size: Option<u64>,
        expected_md5: Option<&str>,
        on_progress: impl FnMut(u64, Option<u64>) + Send,
    ) -> Result<()> {
        self.download(account, file_id, dest, expected_size, on_progress)
            .await?;
        if let Some(expected) = expected_md5 {
            let got = md5_file(dest).await?;
            if !got.eq_ignore_ascii_case(expected) {
                let _ = tokio::fs::remove_file(dest).await;
                return Err(DriveError::Integrity(format!(
                    "checksum mismatch after download of {}: {got} != {expected}",
                    file_name(dest)
                )));
            }
        }
        Ok(())
    }

    /// Exports a Google document as `mime`. Exports are not range-resumable, so a stale `dest` is
    /// deleted first.
    pub async fn export(
        &self,
        account: &str,
        file_id: &str,
        mime: &str,
        dest: &Path,
        mut on_progress: impl FnMut(u64, Option<u64>) + Send,
    ) -> Result<()> {
        if tokio::fs::try_exists(dest).await.unwrap_or(false) {
            remove(dest).await?;
        }
        let url = self.url(&format!("files/{file_id}/export"), &[("mimeType", Some(mime))])?;
        self.download_url(account, url, dest, None, &mut on_progress, false)
            .await
    }

    pub async fn download_revision(
        &self,
        account: &str,
        file_id: &str,
        revision_id: &str,
        dest: &Path,
        mut on_progress: impl FnMut(u64, Option<u64>) + Send,
    ) -> Result<()> {
        let url = self.url(
            &format!("files/{file_id}/revisions/{revision_id}"),
            &[("alt", Some("media"))],
        )?;
        self.download_url(account, url, dest, None, &mut on_progress, true)
            .await
    }

    /// Reads `into.len()` bytes at `offset` (used by on-demand access). Returns the bytes actually
    /// read: fewer at the end of the file, 0 beyond it.
    pub async fn read_range(
        &self,
        account: &str,
        file_id: &str,
        offset: u64,
        into: &mut [u8],
    ) -> Result<usize> {
        if into.is_empty() {
            return Ok(0);
        }
        let url = self.url(
            &format!("files/{file_id}"),
            &[("alt", Some("media")), ("supportsAllDrives", Some("true"))],
        )?;
        let first = with_retry!(self, account, 6, |tok| {
            self.timed(self.read_range_once(&tok, &url, offset, &mut *into))
                .await
        });
        match first {
            // As in `download_flagged`: the owner's own flagged file is still served.
            Err(e) if e.is_flagged_abusive() => {
                let url = self.url(
                    &format!("files/{file_id}"),
                    &[
                        ("alt", Some("media")),
                        ("supportsAllDrives", Some("true")),
                        ("acknowledgeAbuse", Some("true")),
                    ],
                )?;
                with_retry!(self, account, 6, |tok| {
                    self.timed(self.read_range_once(&tok, &url, offset, &mut *into))
                        .await
                })
            }
            other => other,
        }
    }

    async fn read_range_once(&self, token: &str, url: &Url, offset: u64, into: &mut [u8]) -> Result<usize> {
        let last = offset + into.len() as u64 - 1;
        let rb = self
            .request(Method::GET, url.clone(), token)
            .header(ACCEPT_ENCODING, "identity")
            .header(RANGE, format!("bytes={offset}-{last}"));
        let mut resp = self.send(rb).await?;
        if resp.status() == StatusCode::RANGE_NOT_SATISFIABLE {
            return Ok(0);
        }
        if !resp.status().is_success() {
            return Err(response_error(resp).await);
        }
        let mut read = 0;
        while read < into.len() {
            let Some(chunk) = resp.chunk().await.map_err(DriveError::from_reqwest)? else {
                break;
            };
            let n = chunk.len().min(into.len() - read);
            into[read..read + n].copy_from_slice(&chunk[..n]);
            read += n;
        }
        Ok(read)
    }

    async fn download_url(
        &self,
        account: &str,
        url: Url,
        dest: &Path,
        expected_size: Option<u64>,
        on_progress: &mut DownloadProgress<'_>,
        allow_resume: bool,
    ) -> Result<()> {
        with_retry!(self, account, 6, |tok| {
            self.download_once(&tok, &url, dest, expected_size, &mut *on_progress, allow_resume)
                .await
        })?;
        if let Some(expected) = expected_size {
            let got = file_len(dest).await;
            if got != expected {
                if got > expected {
                    // A shorter part is kept and resumed; a longer one never fits.
                    remove(dest).await?;
                }
                return Err(DriveError::Integrity(format!(
                    "size mismatch after download: {got} != {expected}"
                )));
            }
        }
        Ok(())
    }

    async fn download_once(
        &self,
        token: &str,
        url: &Url,
        dest: &Path,
        expected_size: Option<u64>,
        on_progress: &mut DownloadProgress<'_>,
        allow_resume: bool,
    ) -> Result<()> {
        let mut existing = if allow_resume { file_len(dest).await } else { 0 };
        if let Some(expected) = expected_size {
            // Left by an interrupted download of an older, bigger version: resuming it can never work.
            if existing > expected {
                remove(dest).await?;
                existing = 0;
            }
            if existing == expected && existing > 0 {
                return Ok(());
            }
        }
        // File content travels as stored: offsets for resuming, sizes and MD5 stay those of the
        // file (metadata calls are gzip-encoded instead; see `http_client_builder`).
        let mut rb = self
            .request(Method::GET, url.clone(), token)
            .header(ACCEPT_ENCODING, "identity");
        if existing > 0 {
            rb = rb.header(RANGE, format!("bytes={existing}-"));
        }
        let mut resp = self.send(rb).await?;
        let status = resp.status();
        if status == StatusCode::RANGE_NOT_SATISFIABLE {
            // Nothing left to send. That is "complete" only when the size is right; otherwise the
            // part file is stale and every later run would fail on it the same way.
            if let Some(expected) = expected_size
                && existing != expected
            {
                remove(dest).await?;
                return Err(DriveError::Integrity(format!(
                    "stale partial download of {}: {existing} bytes, expected {expected}",
                    file_name(dest)
                )));
            }
            return Ok(());
        }
        if !status.is_success() {
            return Err(response_error(resp).await);
        }
        let append = status == StatusCode::PARTIAL_CONTENT && existing > 0;
        let total = expected_size.or_else(|| {
            resp.content_length()
                .map(|len| len + if append { existing } else { 0 })
        });

        let write_err = |e: std::io::Error| DriveError::Io(format!("cannot write {}: {e}", file_name(dest)));
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(dest)
            .await
            .map_err(write_err)?;
        if append {
            file.seek(SeekFrom::Start(existing)).await.map_err(write_err)?;
        } else {
            file.set_len(0).await.map_err(write_err)?;
        }
        let mut done = if append { existing } else { 0 };
        let mut last_report = 0;
        while let Some(chunk) = resp.chunk().await.map_err(DriveError::from_reqwest)? {
            // A server sending more than the file's size must not fill the disk (security review
            // 2026-10); the part can never verify, so it goes.
            if let Some(expected) = expected_size
                && done + chunk.len() as u64 > expected
            {
                drop(file);
                remove(dest).await?;
                return Err(DriveError::Integrity(format!(
                    "download of {} is longer than the expected {expected} bytes",
                    file_name(dest)
                )));
            }
            self.cfg.limiter.acquire(chunk.len()).await;
            file.write_all(&chunk).await.map_err(write_err)?;
            done += chunk.len() as u64;
            if done - last_report >= REPORT_EVERY {
                last_report = done;
                on_progress(done, total);
            }
        }
        file.flush().await.map_err(write_err)?;
        on_progress(done, total);
        Ok(())
    }
}

/// Maps a non-successful streaming response, reading its body for the error details.
pub(crate) async fn response_error(resp: reqwest::Response) -> DriveError {
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = read_capped(resp)
        .await
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned());
    error_for(status, &headers, body.as_deref())
}

/// Lowercase hex MD5 of a file's content, as Drive's `md5Checksum` reports it.
pub async fn md5_file(path: &Path) -> Result<String> {
    let read_err = |e: std::io::Error| DriveError::Io(format!("cannot read {}: {e}", file_name(path)));
    let mut file = tokio::fs::File::open(path).await.map_err(read_err)?;
    let mut hasher = Md5::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf).await.map_err(read_err)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Length of `path`, 0 when it does not exist (Java's `File.length()`).
pub(crate) async fn file_len(path: &Path) -> u64 {
    tokio::fs::metadata(path).await.map(|m| m.len()).unwrap_or(0)
}

async fn remove(path: &Path) -> Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(DriveError::Io(format!("cannot delete {}: {e}", file_name(path)))),
    }
}

pub(crate) fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}
