//! Chunks (ON_DEMAND.md §1.2, ENCRYPTION.md §11.7 and §11.10).
//!
//! ```text
//! cut      = FastCDC 2020, normalization level 1, min 256 KiB, avg 1 MiB, max 4 MiB;
//!            a file of at most 4 MiB is one chunk (none when empty).
//!            2.0: FastCDC's public gear table. 2.1: a gear table derived from kCdc (Gear::keyed)
//! chunk_id = HMAC-SHA256(kChunkId, file_id || BLAKE3(plaintext))
//! object   = ".wasl-chunks/" || hex(chunk_id[0]) || "/" || base64url(chunk_id)
//! content  2.0: "W2C1" || nonce (12, random) || AES-256-GCM(kChunk, plaintext,
//!                                              aad = "wasl-v2-chunk" || file_id || chunk_id)
//!          2.1: "W2C2" || nonce (12, random) || AES-256-GCM(kChunk, body,
//!                                              aad = "wasl-v2.1-chunk" || file_id || chunk_id)
//!               body = u32 BE len || plaintext || zeros, |body| = padme(max(4 + len, 4096))
//! ```
//!
//! The 2.0 cut points are those of `wasl_chunk::chunk_reader` with these sizes (a test checks it);
//! the cutting here also hands out each chunk's bytes, reading the input once.

use super::format::Format;
use super::manifest::{FileId, NONCE_BYTES, TAG_BYTES};
use super::{B64URL, integrity, keys::VaultKeysV2};
use crate::error::Result;
use crate::kdf::hmac_sha256;
use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce, Tag};
use base64::Engine as _;
use fastcdc::v2020;
use std::io::{self, Read};
use zeroize::Zeroizing;

pub const MIN_CHUNK_BYTES: usize = 256 << 10;
pub const AVG_CHUNK_BYTES: usize = 1 << 20;
pub const MAX_CHUNK_BYTES: usize = 4 << 20;
pub const CHUNK_ID_BYTES: usize = 32;
pub const MAGIC: [u8; 4] = *b"W2C1";
pub const AAD_LABEL: &str = "wasl-v2-chunk";
/// Format 2.1's padded chunk framing.
pub const MAGIC_PADDED: [u8; 4] = *b"W2C2";
/// A different label from 2.0's, so a body can never be read under the other framing's rules.
pub const AAD_LABEL_PADDED: &str = "wasl-v2.1-chunk";
/// HKDF info turning kCdc into the 2.1 gear table.
pub const GEAR_INFO: &str = "wasl-v2-gear";
/// The smallest padded body: a file of a few bytes looks like one of 4 KiB.
pub const MIN_PADDED_BYTES: usize = 4096;
/// The length prefix inside a padded body.
const LEN_BYTES: usize = 4;
/// The Drive folder (under the vault root) holding every chunk, in 256 shard folders.
pub const CHUNKS_DIR: &str = ".wasl-chunks";
/// Bytes a sealed chunk adds to its plaintext: magic, nonce, tag.
pub const OVERHEAD: usize = MAGIC.len() + NONCE_BYTES + TAG_BYTES;
/// The largest chunk object a reader accepts: a full chunk, padded (2.1) and sealed. 2.0's
/// largest (`OVERHEAD + MAX_CHUNK_BYTES`) is smaller.
pub const MAX_SEALED_BYTES: usize = OVERHEAD + padded_len(MAX_CHUNK_BYTES);

/// Padmé (Nikitin et al., "Reducing Metadata Leakage from Encrypted Files and Communication with
/// PURBs", PETS 2019): rounds `len` up so that only its top `floor(log2(floor(log2 len))) + 1`
/// bits can be non-zero. An observer learns O(log log len) bits of the length; the cost is at
/// most 11.1 %, and under 3.2 % from 64 KiB up (every chunk but a file's last is 256 KiB or more).
pub const fn padme(len: u64) -> u64 {
    if len < 2 {
        return len;
    }
    let e = 63 - len.leading_zeros(); // floor(log2 len), at least 1
    let s = 32 - e.leading_zeros(); // floor(log2 e) + 1
    let last_bits = e - s;
    let mask = (1u64 << last_bits) - 1;
    (len + mask) & !mask
}

/// The padded body length of a 2.1 chunk of `len` plaintext bytes: the length prefix and the
/// plaintext, at least [`MIN_PADDED_BYTES`], rounded up with [`padme`].
pub const fn padded_len(len: usize) -> usize {
    let raw = LEN_BYTES + len;
    let raw = if raw < MIN_PADDED_BYTES {
        MIN_PADDED_BYTES
    } else {
        raw
    };
    padme(raw as u64) as usize
}

/// A FastCDC gear table and its left-shifted twin (FastCDC 2020 rolls two bytes per step).
pub struct Gear {
    gear: Zeroizing<[u64; 256]>,
    gear_ls: Zeroizing<[u64; 256]>,
}

impl Gear {
    /// Format 2.1's table: `HKDF-SHA256(salt = none, ikm = kCdc, info = "wasl-v2-gear", 2048)`
    /// read as 256 big-endian u64s; the shifted table is each value shifted left by one bit, as
    /// FastCDC's own is. A whole table drawn from a PRF, not FastCDC's XOR-with-a-seed option
    /// (one 64-bit value XORed into every entry keeps the table's structure). Without kCdc nobody
    /// can tell where a file is cut. The table is secret: it is wiped when dropped.
    pub fn keyed(keys: &VaultKeysV2) -> Gear {
        let mut bytes = Zeroizing::new([0u8; 256 * 8]);
        let hk = hkdf::Hkdf::<sha2::Sha256>::new(None, keys.k_cdc());
        if hk.expand(GEAR_INFO.as_bytes(), &mut bytes[..]).is_err() {
            unreachable!("2048 bytes is far below HKDF-SHA256's limit")
        }
        let mut gear = Zeroizing::new([0u64; 256]);
        let mut gear_ls = Zeroizing::new([0u64; 256]);
        for (i, word) in bytes.as_chunks::<8>().0.iter().enumerate() {
            gear[i] = u64::from_be_bytes(*word);
            gear_ls[i] = gear[i] << 1;
        }
        Gear { gear, gear_ls }
    }

    /// The first `n` table values (known-answer tests).
    pub fn head(&self, n: usize) -> Vec<u64> {
        self.gear.iter().take(n).copied().collect()
    }
}

/// A chunk's keyed content id.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChunkId(pub [u8; CHUNK_ID_BYTES]);

impl ChunkId {
    /// HMAC-SHA256(kChunkId, file_id || BLAKE3(plaintext)).
    pub fn compute(keys: &VaultKeysV2, file_id: &FileId, plaintext: &[u8]) -> ChunkId {
        Self::from_hash(keys, file_id, blake3::hash(plaintext).as_bytes())
    }

    /// The same, from the plaintext's BLAKE3.
    pub fn from_hash(keys: &VaultKeysV2, file_id: &FileId, blake3: &[u8; 32]) -> ChunkId {
        let mut msg = [0u8; 16 + 32];
        msg[..16].copy_from_slice(file_id.as_bytes());
        msg[16..].copy_from_slice(blake3);
        ChunkId(hmac_sha256(keys.k_chunk_id(), &msg))
    }

    pub fn as_bytes(&self) -> &[u8; CHUNK_ID_BYTES] {
        &self.0
    }

    /// The shard folder: lowercase hex of the first byte (`00`..`ff`).
    pub fn shard(&self) -> String {
        format!("{:02x}", self.0[0])
    }

    /// The object's name inside its shard: base64url without padding (43 characters).
    pub fn object_name(&self) -> String {
        B64URL.encode(self.0)
    }

    /// `.wasl-chunks/<shard>/<name>`, `/`-separated, relative to the vault root.
    pub fn object_path(&self) -> String {
        format!("{CHUNKS_DIR}/{}/{}", self.shard(), self.object_name())
    }

    /// Parses an object name (canonical base64url of 32 bytes), `None` otherwise.
    pub fn from_object_name(name: &str) -> Option<ChunkId> {
        Some(ChunkId(B64URL.decode(name).ok()?.try_into().ok()?))
    }
}

impl std::fmt::Debug for ChunkId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ChunkId({})", self.object_name())
    }
}

fn aad(file_id: &FileId, id: &ChunkId) -> Vec<u8> {
    let mut v = Vec::with_capacity(AAD_LABEL.len() + 16 + CHUNK_ID_BYTES);
    v.extend_from_slice(AAD_LABEL.as_bytes());
    v.extend_from_slice(file_id.as_bytes());
    v.extend_from_slice(id.as_bytes());
    v
}

fn gcm(keys: &VaultKeysV2) -> Aes256Gcm {
    Aes256Gcm::new(keys.k_chunk().into())
}

fn aad_padded(file_id: &FileId, id: &ChunkId) -> Vec<u8> {
    let mut v = Vec::with_capacity(AAD_LABEL_PADDED.len() + 16 + CHUNK_ID_BYTES);
    v.extend_from_slice(AAD_LABEL_PADDED.as_bytes());
    v.extend_from_slice(file_id.as_bytes());
    v.extend_from_slice(id.as_bytes());
    v
}

/// Seals one chunk with a fresh nonce, in the framing of `format`.
pub fn seal_chunk(
    keys: &VaultKeysV2,
    format: Format,
    file_id: &FileId,
    id: &ChunkId,
    plain: &[u8],
) -> Result<Vec<u8>> {
    let nonce = super::random()?;
    Ok(if format.padded_chunks() {
        seal_padded_chunk_with_nonce(keys, file_id, id, plain, &nonce)
    } else {
        seal_chunk_with_nonce(keys, file_id, id, plain, &nonce)
    })
}

/// Format 2.1 (`W2C2`) with a chosen nonce: known-answer tests only.
pub fn seal_padded_chunk_with_nonce(
    keys: &VaultKeysV2,
    file_id: &FileId,
    id: &ChunkId,
    plain: &[u8],
    nonce: &[u8; NONCE_BYTES],
) -> Vec<u8> {
    let body_len = padded_len(plain.len());
    let mut out = Vec::with_capacity(OVERHEAD + body_len);
    out.extend_from_slice(&MAGIC_PADDED);
    out.extend_from_slice(nonce);
    let start = out.len();
    // A chunk is at most 4 MiB, so its length always fits the u32 prefix.
    out.extend_from_slice(&(plain.len() as u32).to_be_bytes());
    out.extend_from_slice(plain);
    out.resize(start + body_len, 0);
    match gcm(keys).encrypt_in_place_detached(
        Nonce::from_slice(nonce),
        &aad_padded(file_id, id),
        &mut out[start..],
    ) {
        Ok(tag) => out.extend_from_slice(&tag),
        // Only for inputs beyond 64 GiB, which no chunk is.
        Err(_) => out.clear(),
    }
    out
}

/// Format 2.0 (`W2C1`) with a chosen nonce: known-answer tests only.
pub fn seal_chunk_with_nonce(
    keys: &VaultKeysV2,
    file_id: &FileId,
    id: &ChunkId,
    plain: &[u8],
    nonce: &[u8; NONCE_BYTES],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(OVERHEAD + plain.len());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(nonce);
    let start = out.len();
    out.extend_from_slice(plain);
    match gcm(keys).encrypt_in_place_detached(Nonce::from_slice(nonce), &aad(file_id, id), &mut out[start..])
    {
        Ok(tag) => out.extend_from_slice(&tag),
        // Only for inputs beyond 64 GiB, which no chunk is.
        Err(_) => out.clear(),
    }
    out
}

/// Opens a chunk object of file `file_id` expected to be chunk `id` (of `expected_len` bytes when
/// given). Refuses anything modified, truncated, from another file or another position, and checks
/// that the plaintext really has this id. Both framings are accepted (the magic says which): each
/// has its own AAD label, so neither can be passed off as the other.
pub fn open_chunk(
    keys: &VaultKeysV2,
    file_id: &FileId,
    id: &ChunkId,
    expected_len: Option<u32>,
    sealed: &[u8],
) -> Result<Vec<u8>> {
    if sealed.len() < OVERHEAD || sealed.len() > MAX_SEALED_BYTES {
        return Err(integrity(format!(
            "chunk {} has an impossible size",
            id.object_name()
        )));
    }
    let (magic, rest) = sealed.split_at(MAGIC.len());
    let padded = if magic == MAGIC {
        false
    } else if magic == MAGIC_PADDED {
        true
    } else {
        return Err(integrity(format!(
            "chunk {} is not a vault v2 chunk",
            id.object_name()
        )));
    };
    let (nonce, body) = rest.split_at(NONCE_BYTES);
    let (ct, tag) = body.split_at(body.len() - TAG_BYTES);
    let mut plain = ct.to_vec();
    let aad = if padded {
        aad_padded(file_id, id)
    } else {
        aad(file_id, id)
    };
    gcm(keys)
        .decrypt_in_place_detached(Nonce::from_slice(nonce), &aad, &mut plain, Tag::from_slice(tag))
        .map_err(|_| {
            integrity(format!(
                "chunk {} does not verify: modified, truncated, or not this file's",
                id.object_name()
            ))
        })?;
    if padded {
        plain = unpad(plain).ok_or_else(|| {
            integrity(format!(
                "chunk {} is not padded as format 2.1 pads",
                id.object_name()
            ))
        })?;
    }
    if expected_len.is_some_and(|l| l as usize != plain.len())
        || ChunkId::compute(keys, file_id, &plain) != *id
    {
        return Err(integrity(format!(
            "chunk {} does not match its manifest",
            id.object_name()
        )));
    }
    Ok(plain)
}

/// The plaintext of a verified 2.1 body, `None` unless the padding is exactly what a writer makes
/// (the canonical length, all zeros): a body has one accepted form.
fn unpad(mut body: Vec<u8>) -> Option<Vec<u8>> {
    let prefix: [u8; LEN_BYTES] = body.get(..LEN_BYTES)?.try_into().ok()?;
    let len = u32::from_be_bytes(prefix) as usize;
    if len > MAX_CHUNK_BYTES || padded_len(len) != body.len() {
        return None;
    }
    if body[LEN_BYTES + len..].iter().any(|&b| b != 0) {
        return None;
    }
    body.truncate(LEN_BYTES + len);
    body.drain(..LEN_BYTES);
    Some(body)
}

/// Cuts everything `r` yields into chunks as format 2.0 does (FastCDC's public gear table),
/// calling `f` with each chunk's bytes in order. Returns the total size and the whole input's
/// BLAKE3. Holds at most 8 MiB in memory.
pub fn cut(r: impl Read, f: impl FnMut(&[u8]) -> Result<()>) -> Result<(u64, [u8; 32])> {
    cut_with(None, r, f)
}

/// [`cut`] the way `format` cuts: 2.1 with the vault's keyed gear table ([`Gear::keyed`]).
pub fn cut_as(
    keys: &VaultKeysV2,
    format: Format,
    r: impl Read,
    f: impl FnMut(&[u8]) -> Result<()>,
) -> Result<(u64, [u8; 32])> {
    if format.keyed_chunking() {
        cut_with(Some(&Gear::keyed(keys)), r, f)
    } else {
        cut_with(None, r, f)
    }
}

fn cut_with(
    gear: Option<&Gear>,
    mut r: impl Read,
    mut f: impl FnMut(&[u8]) -> Result<()>,
) -> Result<(u64, [u8; 32])> {
    const READ_AHEAD: usize = 4 << 20;
    let (min, avg, max) = (MIN_CHUNK_BYTES, AVG_CHUNK_BYTES, MAX_CHUNK_BYTES);
    let cap = max + READ_AHEAD;
    let mut buf = Vec::new();
    let mut eof = fill(&mut r, &mut buf, (64 << 10).min(cap))?;
    if !eof {
        buf.reserve_exact(cap - buf.len());
        eof = fill(&mut r, &mut buf, cap)?;
    }
    let mut whole = blake3::Hasher::new();
    if eof && buf.len() <= max {
        whole.update(&buf);
        if !buf.is_empty() {
            f(&buf)?;
        }
        return Ok((buf.len() as u64, *whole.finalize().as_bytes()));
    }
    let bits = v2020::logarithm2(avg as u32) as usize;
    let (mask_s, mask_l) = (v2020::MASKS[bits + 1], v2020::MASKS[bits - 1]);
    let mut total = 0u64;
    let mut start = 0usize;
    loop {
        if !eof && buf.len() - start < max {
            buf.copy_within(start.., 0);
            buf.truncate(buf.len() - start);
            start = 0;
            eof = fill(&mut r, &mut buf, cap)?;
        }
        let avail = &buf[start..];
        if avail.is_empty() {
            break;
        }
        let (_, len) = match gear {
            None => v2020::cut(avail, min, avg, max, mask_s, mask_l, mask_s << 1, mask_l << 1),
            Some(g) => v2020::cut_gear(
                avail,
                min,
                avg,
                max,
                mask_s,
                mask_l,
                mask_s << 1,
                mask_l << 1,
                &g.gear,
                &g.gear_ls,
            ),
        };
        let data = &avail[..len];
        whole.update(data);
        f(data)?;
        total += len as u64;
        start += len;
    }
    Ok((total, *whole.finalize().as_bytes()))
}

fn fill(r: &mut impl Read, buf: &mut Vec<u8>, cap: usize) -> io::Result<bool> {
    let want = cap - buf.len();
    let got = (&mut *r).take(want as u64).read_to_end(buf)?;
    Ok(got < want)
}
