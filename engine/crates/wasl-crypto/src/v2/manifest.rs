//! The per-file manifest and the `wasl.m` listing blob (ON_DEMAND.md §1.2).
//!
//! ```text
//! placement = parent dir id (16) || the file's encrypted name as SIV bytes      (names::Placement)
//! manifest  = "W2M1" || nonce (12, random)
//!             || AES-256-GCM(kMeta, cbor, aad = "wasl-v2-manifest" || placement)
//! cbor      = map(6) { "size": uint, "mtime": int, "blake3": bytes(32),
//!                      "chunks": [ [bytes(32) chunk_id, uint len], ... ], "file_id": bytes(16),
//!                      "version": uint }
//! wasl.m    = base64(nonce (12) || AES-256-GCM(kMeta, size (u64 BE) || mtime (i64 BE) || blake3 (32),
//!                                              aad = "wasl-v2-meta" || placement))
//! ```
//!
//! Binding to the placement means a manifest (or blob) swapped with another file's, or moved to
//! another folder or name, does not open: a rename or move re-seals the manifest. `version` starts
//! at 1 and grows by one with every write of the file ([`crate::v2::file::encrypt_update`]); a
//! reader that remembers the highest version it saw refuses an older, genuine manifest put back
//! ([`FileManifest::check_not_older_than`]). Nothing stops a rollback to a version a device never
//! saw superseded.
//!
//! The CBOR is deterministic (RFC 8949 §4.2.1): shortest heads, definite lengths, map keys in that
//! order (bytewise order of their encodings). `mtime` is milliseconds since the Unix epoch. The
//! decoder accepts exactly this shape: chunk lengths 1..=4 MiB summing to `size`, at most
//! [`MAX_CHUNKS`] chunks, at most [`MAX_MANIFEST_BYTES`] of manifest.

use super::cbor::{self, ARRAY, MAP, Reader};
use super::chunks::{CHUNK_ID_BYTES, ChunkId, MAX_CHUNK_BYTES};
use super::keys::VaultKeysV2;
use super::names::Placement;
use super::{B64, integrity};
use crate::error::Result;
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine as _;

pub const MAGIC: [u8; 4] = *b"W2M1";
pub const NONCE_BYTES: usize = 12;
pub const TAG_BYTES: usize = 16;
pub const AAD: &str = "wasl-v2-manifest";
pub const META_AAD: &str = "wasl-v2-meta";
pub const FILE_ID_BYTES: usize = 16;
pub const BLAKE3_BYTES: usize = 32;
/// The most chunks one manifest may list (about 1 TiB at the 1 MiB average).
pub const MAX_CHUNKS: usize = 1 << 20;
/// The largest manifest object a reader accepts (sealed bytes).
pub const MAX_MANIFEST_BYTES: usize = 64 << 20;
/// The Drive appProperties key of the listing blob.
pub const META_PROPERTY: &str = "wasl.m";
/// Drive's limit on one appProperties entry: key + value, UTF-8 bytes.
pub const MAX_PROPERTY_BYTES: usize = 124;
const META_PLAIN_BYTES: usize = 8 + 8 + BLAKE3_BYTES;

/// A file's random 16-byte id, fixed for the file's life (edits keep it, so unchanged chunks keep
/// their ids). Bound into every chunk's id and AAD.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(pub [u8; FILE_ID_BYTES]);

impl FileId {
    pub fn random() -> Result<FileId> {
        super::random().map(FileId)
    }

    pub fn as_bytes(&self) -> &[u8; FILE_ID_BYTES] {
        &self.0
    }
}

impl std::fmt::Debug for FileId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FileId({})", super::hex(&self.0))
    }
}

/// One entry of a manifest's chunk list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkRef {
    pub id: ChunkId,
    /// Plaintext length, 1..=4 MiB.
    pub len: u32,
}

/// A file's manifest: what it is and the chunks it is made of, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileManifest {
    pub file_id: FileId,
    pub size: u64,
    /// Milliseconds since the Unix epoch.
    pub mtime_ms: i64,
    /// BLAKE3 of the whole plaintext.
    pub blake3: [u8; BLAKE3_BYTES],
    pub chunks: Vec<ChunkRef>,
    /// 1 for a new file, one more with every write.
    pub version: u64,
}

impl FileManifest {
    /// Refuses a manifest older than the highest version this device has seen of the file
    /// (rollback by someone who can write to the Drive folder).
    pub fn check_not_older_than(&self, highest_seen: u64) -> Result<()> {
        if self.version < highest_seen {
            return Err(integrity(format!(
                "file manifest version {} is older than version {highest_seen} seen before (rolled back)",
                self.version
            )));
        }
        Ok(())
    }

    /// Checks the bounds the decoder enforces (chunk count and sizes, their sum).
    pub fn validate(&self) -> Result<()> {
        if self.version == 0 {
            return Err(integrity("file manifest version must be at least 1"));
        }
        if self.chunks.len() > MAX_CHUNKS {
            return Err(integrity(format!(
                "file manifest lists {} chunks (at most {MAX_CHUNKS})",
                self.chunks.len()
            )));
        }
        let mut sum: u64 = 0;
        for c in &self.chunks {
            if c.len == 0 || c.len as usize > MAX_CHUNK_BYTES {
                return Err(integrity(format!("file manifest has a chunk of {} bytes", c.len)));
            }
            sum += u64::from(c.len);
        }
        if sum != self.size {
            return Err(integrity(format!(
                "file manifest chunks add up to {sum} bytes, not the file's {}",
                self.size
            )));
        }
        Ok(())
    }

    /// The deterministic CBOR plaintext.
    pub fn to_cbor(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 + self.chunks.len() * (CHUNK_ID_BYTES + 8));
        cbor::head(&mut out, MAP, 6);
        cbor::text(&mut out, "size");
        cbor::head(&mut out, cbor::UINT, self.size);
        cbor::text(&mut out, "mtime");
        cbor::int(&mut out, self.mtime_ms);
        cbor::text(&mut out, "blake3");
        cbor::bytes(&mut out, &self.blake3);
        cbor::text(&mut out, "chunks");
        cbor::head(&mut out, ARRAY, self.chunks.len() as u64);
        for c in &self.chunks {
            cbor::head(&mut out, ARRAY, 2);
            cbor::bytes(&mut out, c.id.as_bytes());
            cbor::head(&mut out, cbor::UINT, u64::from(c.len));
        }
        cbor::text(&mut out, "file_id");
        cbor::bytes(&mut out, self.file_id.as_bytes());
        cbor::text(&mut out, "version");
        cbor::head(&mut out, cbor::UINT, self.version);
        out
    }

    /// Parses the CBOR plaintext, exactly the shape [`Self::to_cbor`] writes, then validates it.
    pub fn from_cbor(data: &[u8]) -> Result<FileManifest> {
        let m = parse_cbor(data).ok_or_else(|| integrity("file manifest content is malformed"))?;
        m.validate()?;
        Ok(m)
    }

    /// Seals with a fresh random nonce for the file at `at`.
    pub fn seal(&self, keys: &VaultKeysV2, at: &Placement) -> Result<Vec<u8>> {
        Ok(self.seal_with_nonce(keys, at, &super::random()?))
    }

    /// Seals with a chosen nonce. Only for known-answer tests: reusing a nonce breaks AES-GCM.
    pub fn seal_with_nonce(&self, keys: &VaultKeysV2, at: &Placement, nonce: &[u8; NONCE_BYTES]) -> Vec<u8> {
        let ct = gcm(keys)
            .encrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: &self.to_cbor(),
                    aad: &bound_aad(AAD, at),
                },
            )
            .unwrap_or_default();
        let mut out = Vec::with_capacity(MAGIC.len() + NONCE_BYTES + ct.len());
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(nonce);
        out.extend_from_slice(&ct);
        out
    }

    /// Opens the manifest object of the file at `at`. Anything modified, truncated, foreign, moved
    /// from another file or folder, or out of bounds fails with [`crate::CryptoError::Integrity`].
    pub fn open(keys: &VaultKeysV2, at: &Placement, sealed: &[u8]) -> Result<FileManifest> {
        if sealed.len() > MAX_MANIFEST_BYTES {
            return Err(integrity(format!(
                "file manifest is {} bytes (at most {MAX_MANIFEST_BYTES})",
                sealed.len()
            )));
        }
        let rest = sealed
            .strip_prefix(&MAGIC[..])
            .ok_or_else(|| integrity("not a vault v2 file manifest (unknown format or version)"))?;
        if rest.len() < NONCE_BYTES + TAG_BYTES {
            return Err(integrity("file manifest is truncated"));
        }
        let (nonce, ct) = rest.split_at(NONCE_BYTES);
        let plain = gcm(keys)
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ct,
                    aad: &bound_aad(AAD, at),
                },
            )
            .map_err(|_| {
                integrity(
                    "file manifest does not verify: modified, truncated, another file's or another vault's",
                )
            })?;
        FileManifest::from_cbor(&plain)
    }

    /// The listing blob for this manifest.
    pub fn meta(&self) -> MetaBlob {
        MetaBlob {
            size: self.size,
            mtime_ms: self.mtime_ms,
            blake3: self.blake3,
        }
    }

    /// The plaintext offset of every chunk, plus the file size at the end (`chunks.len() + 1`
    /// values).
    pub fn offsets(&self) -> Vec<u64> {
        let mut v = Vec::with_capacity(self.chunks.len() + 1);
        let mut at = 0u64;
        v.push(0);
        for c in &self.chunks {
            at += u64::from(c.len);
            v.push(at);
        }
        v
    }
}

fn parse_cbor(data: &[u8]) -> Option<FileManifest> {
    let mut r = Reader::new(data);
    if r.expect(MAP)? != 6 {
        return None;
    }
    r.key("size")?;
    let size = r.uint()?;
    r.key("mtime")?;
    let mtime_ms = r.int()?;
    r.key("blake3")?;
    let blake3 = r.fixed::<BLAKE3_BYTES>()?;
    r.key("chunks")?;
    let n = usize::try_from(r.expect(ARRAY)?).ok()?;
    if n > MAX_CHUNKS {
        return None;
    }
    // Each entry takes at least 37 bytes, so `n` is bounded by the input before allocating.
    if n > data.len() / (CHUNK_ID_BYTES + 5) {
        return None;
    }
    let mut chunks = Vec::with_capacity(n);
    for _ in 0..n {
        if r.expect(ARRAY)? != 2 {
            return None;
        }
        let id = ChunkId(r.fixed::<CHUNK_ID_BYTES>()?);
        let len = u32::try_from(r.uint()?).ok()?;
        chunks.push(ChunkRef { id, len });
    }
    r.key("file_id")?;
    let file_id = FileId(r.fixed::<FILE_ID_BYTES>()?);
    r.key("version")?;
    let version = r.uint()?;
    r.at_end().then_some(FileManifest {
        file_id,
        size,
        mtime_ms,
        blake3,
        chunks,
        version,
    })
}

/// label || parent dir id || name SIV bytes.
fn bound_aad(label: &str, at: &Placement) -> Vec<u8> {
    let mut v = label.as_bytes().to_vec();
    v.extend_from_slice(&at.to_bytes());
    v
}

fn gcm(keys: &VaultKeysV2) -> Aes256Gcm {
    Aes256Gcm::new(keys.k_meta().into())
}

/// What a listing learns from `appProperties["wasl.m"]` without downloading the manifest. Bound to
/// the file's placement like the manifest; the manifest stays authoritative (the blob carries no
/// version, so an older blob of the same file could be put back).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetaBlob {
    pub size: u64,
    pub mtime_ms: i64,
    pub blake3: [u8; BLAKE3_BYTES],
}

impl MetaBlob {
    /// The property value (standard base64, 104 characters) with a fresh nonce.
    pub fn encode(&self, keys: &VaultKeysV2, at: &Placement) -> Result<String> {
        Ok(self.encode_with_nonce(keys, at, &super::random()?))
    }

    /// With a chosen nonce: known-answer tests only.
    pub fn encode_with_nonce(&self, keys: &VaultKeysV2, at: &Placement, nonce: &[u8; NONCE_BYTES]) -> String {
        let mut plain = [0u8; META_PLAIN_BYTES];
        plain[..8].copy_from_slice(&self.size.to_be_bytes());
        plain[8..16].copy_from_slice(&self.mtime_ms.to_be_bytes());
        plain[16..].copy_from_slice(&self.blake3);
        let ct = gcm(keys)
            .encrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: &plain[..],
                    aad: &bound_aad(META_AAD, at),
                },
            )
            .unwrap_or_default();
        let mut raw = nonce.to_vec();
        raw.extend_from_slice(&ct);
        let value = B64.encode(raw);
        debug_assert!(META_PROPERTY.len() + value.len() <= MAX_PROPERTY_BYTES);
        value
    }

    /// Decodes a property value; anything else than a value [`Self::encode`] made with these keys
    /// fails with [`crate::CryptoError::Integrity`].
    pub fn decode(keys: &VaultKeysV2, at: &Placement, value: &str) -> Result<MetaBlob> {
        if META_PROPERTY.len() + value.len() > MAX_PROPERTY_BYTES {
            return Err(integrity("wasl.m property is too long"));
        }
        let raw = B64
            .decode(value)
            .map_err(|_| integrity("wasl.m property is not base64"))?;
        if raw.len() != NONCE_BYTES + META_PLAIN_BYTES + TAG_BYTES {
            return Err(integrity("wasl.m property has the wrong length"));
        }
        let (nonce, ct) = raw.split_at(NONCE_BYTES);
        let plain = gcm(keys)
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ct,
                    aad: &bound_aad(META_AAD, at),
                },
            )
            .map_err(|_| integrity("wasl.m property does not verify"))?;
        let mut blake3 = [0u8; BLAKE3_BYTES];
        blake3.copy_from_slice(&plain[16..]);
        Ok(MetaBlob {
            size: u64::from_be_bytes(plain[..8].try_into().unwrap_or_default()),
            mtime_ms: i64::from_be_bytes(plain[8..16].try_into().unwrap_or_default()),
            blake3,
        })
    }
}
