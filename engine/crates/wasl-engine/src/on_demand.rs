//! Where on-demand reads come from (docs/ON_DEMAND.md §2): any remote's byte ranges, a vault v1
//! file a chunk at a time, and the tree a view shows. A vault v2 file is read by
//! [`crate::VaultV2Remote`] itself, which is a [`Source`] too.

use std::sync::Arc;

use async_trait::async_trait;
use wasl_core::RemoteEntry;
use wasl_crypto::content::{self, CHUNK_BYTES, VaultCipher};
use wasl_drive::{DriveClient, DriveError};
use wasl_vfs::{FileRef, Node, Source, Tree, VfsError};

use crate::remote::Remote;

/// Reads a Drive file's bytes with range requests. [`FileRef::id`] is the Drive file id.
pub struct DriveSource {
    client: Arc<DriveClient>,
    account: String,
}

impl DriveSource {
    pub fn new(client: Arc<DriveClient>, account: impl Into<String>) -> Self {
        Self {
            client,
            account: account.into(),
        }
    }
}

/// After a range read: the file is still the content the caller listed (`expect`: its MD5 or
/// Drive version). Drive's version only grows, so bytes read before this check belong to `expect`
/// if it still holds now; otherwise they may mix two versions and must not be used or cached.
pub(crate) async fn check_still(
    client: &DriveClient,
    account: &str,
    id: &str,
    expect: &str,
) -> wasl_drive::Result<()> {
    let now = client
        .get_file_fields(account, id, "id,version,md5Checksum")
        .await?;
    if now.md5_checksum.as_deref() == Some(expect) || now.version.as_deref() == Some(expect) {
        Ok(())
    } else {
        Err(DriveError::Integrity(format!(
            "{id} changed on Drive while it was being read"
        )))
    }
}

fn vfs_error(e: DriveError) -> VfsError {
    match e {
        DriveError::Integrity(m) => VfsError::Integrity(m),
        e => VfsError::Unavailable(e.to_string()),
    }
}

/// Largest single range request: big enough to stream, small enough to retry cheaply.
const MAX_REQUEST: u64 = 16 << 20;

#[async_trait]
impl Source for DriveSource {
    async fn read(&self, file: &FileRef, offset: u64, len: u64) -> Result<Vec<u8>, VfsError> {
        let end = offset.saturating_add(len).min(file.size);
        let mut out = Vec::with_capacity(end.saturating_sub(offset) as usize);
        let mut at = offset;
        while at < end {
            let want = (end - at).min(MAX_REQUEST) as usize;
            let mut buf = vec![0u8; want];
            let n = self
                .client
                .read_range(&self.account, &file.id, at, &mut buf)
                .await
                .map_err(|e| VfsError::Unavailable(e.to_string()))?;
            if n == 0 {
                // The file is shorter on Drive than the listing said: it changed.
                return Err(VfsError::Integrity(format!(
                    "{} ended at {at} bytes, the listing said {}",
                    file.id, file.size
                )));
            }
            out.extend_from_slice(&buf[..n]);
            at += n as u64;
        }
        check_still(&self.client, &self.account, &file.id, &file.version)
            .await
            .map_err(vfs_error)?;
        Ok(out)
    }
}

/// Reads any remote's files by range ([`Remote::read_range`]).
pub struct RemoteSource(pub Arc<dyn Remote>);

#[async_trait]
impl Source for RemoteSource {
    async fn read(&self, file: &FileRef, offset: u64, len: u64) -> Result<Vec<u8>, VfsError> {
        let want = offset.saturating_add(len).min(file.size).saturating_sub(offset);
        let bytes = self
            .0
            .read_range(&file.id, &file.version, offset, want)
            .await
            .map_err(vfs_error)?;
        if bytes.len() as u64 != want {
            return Err(VfsError::Integrity(format!(
                "{} gave {} bytes at {offset}, the listing promised {want}",
                file.id,
                bytes.len()
            )));
        }
        Ok(bytes)
    }
}

/// A vault v1 file read a 1 MiB chunk at a time (docs/ENCRYPTION.md §5): the encrypted chunks
/// covering a range are fetched by range from the inner remote and each is checked on its own.
/// [`FileRef::id`] is the encrypted file's Drive id and [`FileRef::size`] its plaintext size.
pub struct VaultV1Source {
    inner: Arc<dyn Remote>,
    cipher: Arc<VaultCipher>,
}

impl VaultV1Source {
    pub fn new(inner: Arc<dyn Remote>, cipher: Arc<VaultCipher>) -> Self {
        Self { inner, cipher }
    }
}

#[async_trait]
impl Source for VaultV1Source {
    async fn read(&self, file: &FileRef, offset: u64, len: u64) -> Result<Vec<u8>, VfsError> {
        let end = offset.saturating_add(len).min(file.size);
        if end <= offset {
            return Ok(Vec::new());
        }
        let header = self
            .inner
            .read_range(&file.id, &file.version, 0, content::HEADER_BYTES as u64)
            .await
            .map_err(vfs_error)?;
        let nonce = content::read_header(&header).map_err(|e| VfsError::Integrity(e.to_string()))?;
        let chunk = CHUNK_BYTES as u64;
        let (first, last) = (offset / chunk, (end - 1) / chunk);
        let mut plain = Vec::with_capacity(((last - first + 1) * chunk) as usize);
        for index in first..=last {
            let index = u32::try_from(index).map_err(|_| VfsError::OutOfRange)?;
            let (at, n) = content::chunk_cipher_range(file.size, index)
                .ok_or_else(|| VfsError::Integrity(format!("no chunk {index} in {}", file.id)))?;
            let ct = self
                .inner
                .read_range(&file.id, &file.version, at, n)
                .await
                .map_err(vfs_error)?;
            if ct.len() as u64 != n {
                return Err(VfsError::Integrity(format!(
                    "chunk {index} of {} is {} bytes, the listing promised {n}",
                    file.id,
                    ct.len()
                )));
            }
            let is_last = content::is_last_chunk(file.size, index);
            let p = self
                .cipher
                .decrypt_chunk(&nonce, index, is_last, &ct)
                .map_err(|e| VfsError::Integrity(e.to_string()))?;
            plain.extend_from_slice(&p);
        }
        let skip = (offset - first * chunk) as usize;
        let take = (end - offset) as usize;
        plain
            .get(skip..skip + take)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| VfsError::Integrity(format!("{} decrypted shorter than listed", file.id)))
    }

    fn natural_range(&self, _file: &FileRef, offset: u64, len: u64) -> (u64, u64) {
        let chunk = CHUNK_BYTES as u64;
        let from = offset / chunk * chunk;
        (
            from,
            offset.saturating_add(len).div_ceil(chunk).saturating_mul(chunk) - from,
        )
    }
}

/// The tree a view shows, from a remote's listing: files (with what identifies their content)
/// and folders; Google documents, which have no bytes to read, are left out.
pub fn view_tree(entries: &[RemoteEntry]) -> Tree {
    Tree::new(entries.iter().filter(|e| e.google_mime.is_none()).map(|e| Node {
        path: e.key.clone(),
        is_dir: e.is_dir,
        mtime_ms: e.mtime,
        file: (!e.is_dir).then(|| {
            FileRef {
                id: e.id.clone(),
                version: e
                    .md5
                    .clone()
                    .or_else(|| e.version.map(|v| v.to_string()))
                    .unwrap_or_else(|| format!("{}-{}", e.size.unwrap_or(0), e.mtime)),
                size: e.size.unwrap_or(0),
            }
        }),
    }))
}
