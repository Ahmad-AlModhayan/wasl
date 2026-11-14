//! Whole files: the streaming encryptor, range reads, full decryption and update plans
//! (ON_DEMAND.md §1.3–1.4).

use super::chunks::{self, ChunkId};
use super::format::Format;
use super::integrity;
use super::keys::VaultKeysV2;
use super::manifest::{ChunkRef, FileId, FileManifest, MAX_CHUNKS};
use crate::error::Result;
use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::ops::Range;

/// A chunk the encryptor just cut, before it is sealed. The sink decides whether it needs the
/// object (a chunk the old version already had is on Drive already: skip [`Self::seal`]).
pub struct PendingChunk<'a> {
    keys: &'a VaultKeysV2,
    format: Format,
    file_id: &'a FileId,
    id: ChunkId,
    plain: &'a [u8],
}

impl PendingChunk<'_> {
    pub fn id(&self) -> &ChunkId {
        &self.id
    }

    /// Plaintext length.
    pub fn len(&self) -> u32 {
        self.plain.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.plain.is_empty()
    }

    pub fn plaintext(&self) -> &[u8] {
        self.plain
    }

    /// `.wasl-chunks/<shard>/<name>`.
    pub fn object_path(&self) -> String {
        self.id.object_path()
    }

    /// The object's content (`W2C1`… in 2.0, `W2C2`… in 2.1), with a fresh nonce.
    pub fn seal(&self) -> Result<Vec<u8>> {
        chunks::seal_chunk(self.keys, self.format, self.file_id, &self.id, self.plain)
    }
}

/// Encrypts a new file read from `reader` (manifest version 1): cuts it, hands every distinct chunk
/// to `sink` in order (a chunk repeated inside the file is handed over once: it is one object),
/// and returns the manifest to seal and upload after the chunks (ON_DEMAND.md §1.3). At most 8 MiB
/// is held in memory. For a new version of an existing file use [`encrypt_update`]. `format` is
/// the vault's ([`crate::v2::VaultManifestV2::format`]): it decides how the file is cut and how
/// its chunks are sealed.
pub fn encrypt_stream(
    keys: &VaultKeysV2,
    format: Format,
    file_id: FileId,
    mtime_ms: i64,
    reader: impl Read,
    sink: impl FnMut(PendingChunk<'_>) -> Result<()>,
) -> Result<FileManifest> {
    encrypt_version(keys, format, file_id, 1, mtime_ms, reader, sink)
}

/// Encrypts a new version of the file `previous` describes: same file id (so unchanged chunks
/// keep their ids and need no upload: check [`PendingChunk::id`] against the old list, or use
/// [`plan_update`] afterwards), version one higher.
pub fn encrypt_update(
    keys: &VaultKeysV2,
    format: Format,
    previous: &FileManifest,
    mtime_ms: i64,
    reader: impl Read,
    sink: impl FnMut(PendingChunk<'_>) -> Result<()>,
) -> Result<FileManifest> {
    let version = previous
        .version
        .checked_add(1)
        .ok_or_else(|| crate::CryptoError::InvalidInput("file version counter is exhausted".into()))?;
    encrypt_version(keys, format, previous.file_id, version, mtime_ms, reader, sink)
}

fn encrypt_version(
    keys: &VaultKeysV2,
    format: Format,
    file_id: FileId,
    version: u64,
    mtime_ms: i64,
    reader: impl Read,
    mut sink: impl FnMut(PendingChunk<'_>) -> Result<()>,
) -> Result<FileManifest> {
    let mut list = Vec::new();
    let mut seen = HashSet::new();
    let (size, blake3) = chunks::cut_as(keys, format, reader, |plain| {
        if list.len() >= MAX_CHUNKS {
            return Err(crate::CryptoError::InvalidInput(
                "file is too large for one vault v2 manifest".into(),
            ));
        }
        let id = ChunkId::compute(keys, &file_id, plain);
        list.push(ChunkRef {
            id,
            len: plain.len() as u32,
        });
        if seen.insert(id) {
            sink(PendingChunk {
                keys,
                format,
                file_id: &file_id,
                id,
                plain,
            })?;
        }
        Ok(())
    })?;
    let m = FileManifest {
        file_id,
        size,
        mtime_ms,
        blake3,
        chunks: list,
        version,
    };
    m.validate()?;
    Ok(m)
}

/// Reads parts of one file: finds the chunks covering a range and fetches only those.
pub struct RangeReader<'a> {
    keys: &'a VaultKeysV2,
    manifest: &'a FileManifest,
    offsets: Vec<u64>,
}

impl<'a> RangeReader<'a> {
    pub fn new(keys: &'a VaultKeysV2, manifest: &'a FileManifest) -> RangeReader<'a> {
        RangeReader {
            keys,
            manifest,
            offsets: manifest.offsets(),
        }
    }

    /// The indexes of the chunks holding bytes `[offset, offset + len)` (clamped to the file).
    pub fn covering(&self, offset: u64, len: u64) -> Range<usize> {
        let size = self.manifest.size;
        let end = offset.saturating_add(len).min(size);
        if offset >= end {
            return 0..0;
        }
        let starts = &self.offsets[..self.manifest.chunks.len()];
        let first = starts.partition_point(|&o| o <= offset) - 1;
        let last = starts.partition_point(|&o| o < end);
        first..last
    }

    /// Chunk `index`'s plaintext offset and length.
    pub fn chunk_span(&self, index: usize) -> Option<(u64, u32)> {
        let c = self.manifest.chunks.get(index)?;
        Some((self.offsets[index], c.len))
    }

    /// Fetches and opens chunk `index` (verifying its tag, length and id).
    pub fn read_chunk(
        &self,
        index: usize,
        fetch: &mut impl FnMut(&ChunkId) -> io::Result<Vec<u8>>,
    ) -> Result<Vec<u8>> {
        let c = self
            .manifest
            .chunks
            .get(index)
            .ok_or_else(|| integrity(format!("no chunk {index} in this file")))?;
        let sealed = fetch(&c.id)?;
        chunks::open_chunk(self.keys, &self.manifest.file_id, &c.id, Some(c.len), &sealed)
    }

    /// The plaintext bytes `[offset, offset + len)`, shorter at the end of the file, fetching only
    /// the covering chunks.
    pub fn read(
        &self,
        offset: u64,
        len: u64,
        mut fetch: impl FnMut(&ChunkId) -> io::Result<Vec<u8>>,
    ) -> Result<Vec<u8>> {
        let range = self.covering(offset, len);
        let end = offset.saturating_add(len).min(self.manifest.size);
        let mut out = Vec::with_capacity(end.saturating_sub(offset) as usize);
        for i in range {
            let plain = self.read_chunk(i, &mut fetch)?;
            let start = self.offsets[i];
            let from = offset.saturating_sub(start) as usize;
            let to = ((end - start) as usize).min(plain.len());
            out.extend_from_slice(&plain[from..to]);
        }
        Ok(out)
    }
}

/// [`RangeReader::read`] in one call.
pub fn read_range(
    keys: &VaultKeysV2,
    manifest: &FileManifest,
    offset: u64,
    len: u64,
    fetch: impl FnMut(&ChunkId) -> io::Result<Vec<u8>>,
) -> Result<Vec<u8>> {
    RangeReader::new(keys, manifest).read(offset, len, fetch)
}

/// Decrypts a whole file into `out`, checking every chunk and then the size and whole-file
/// BLAKE3 (so a manifest whose chunks are reordered or swapped cannot produce a different file).
/// Returns the plaintext size. Unverified bytes may have been written when it fails: write to a
/// temporary file and keep it only on success.
pub fn decrypt_file(
    keys: &VaultKeysV2,
    manifest: &FileManifest,
    mut fetch: impl FnMut(&ChunkId) -> io::Result<Vec<u8>>,
    mut out: impl Write,
) -> Result<u64> {
    manifest.validate()?;
    let mut hasher = blake3::Hasher::new();
    let mut total = 0u64;
    for c in &manifest.chunks {
        let sealed = fetch(&c.id)?;
        let plain = chunks::open_chunk(keys, &manifest.file_id, &c.id, Some(c.len), &sealed)?;
        hasher.update(&plain);
        out.write_all(&plain)?;
        total += plain.len() as u64;
    }
    out.flush()?;
    if total != manifest.size || hasher.finalize().as_bytes() != &manifest.blake3 {
        return Err(integrity(
            "the decrypted file does not match its manifest (size or BLAKE3)",
        ));
    }
    Ok(total)
}

/// What replacing one version of a file with another costs on Drive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdatePlan {
    /// Chunks the new version has and the old lacks: upload these first (each once).
    pub upload: Vec<ChunkRef>,
    /// Chunks only the old version had: delete these after the new manifest is in place.
    pub delete: Vec<ChunkId>,
}

impl UpdatePlan {
    /// Plaintext bytes to upload (each sealed object is 32 bytes more, and padded in 2.1).
    pub fn upload_bytes(&self) -> u64 {
        self.upload.iter().map(|c| u64::from(c.len)).sum()
    }
}

/// The chunks to upload and delete to go from `old` to `new` (ON_DEMAND.md §1.3: upload, then
/// replace the manifest, then delete). Each id appears once, in list order.
pub fn plan_update(old: &FileManifest, new: &FileManifest) -> UpdatePlan {
    let old_ids: HashSet<ChunkId> = old.chunks.iter().map(|c| c.id).collect();
    let new_ids: HashSet<ChunkId> = new.chunks.iter().map(|c| c.id).collect();
    let mut seen = HashSet::new();
    let upload = new
        .chunks
        .iter()
        .filter(|c| !old_ids.contains(&c.id) && seen.insert(c.id))
        .copied()
        .collect();
    let mut seen = HashSet::new();
    let delete = old
        .chunks
        .iter()
        .filter(|c| !new_ids.contains(&c.id) && seen.insert(c.id))
        .map(|c| c.id)
        .collect();
    UpdatePlan { upload, delete }
}
