//! Streaming file content encryption (spec section 5).
//!
//! ```text
//! output  = "DSV1" || fileNonce (8 random bytes) || chunk_0 || chunk_1 || ... || chunk_(n-1)
//! chunk_i = AES-256-GCM(keyEnc, iv = fileNonce || i, plain_i, aad = "drivesync-chunk-v1" || i || lastFlag)
//! ```
//!
//! `i` is a big-endian uint32, `plain_i` is 1 MiB for every chunk but the last (which may be
//! shorter or empty), `lastFlag` is one byte, 1 for the final chunk. Every chunk is raw ciphertext
//! followed by its 16-byte tag. There is always at least one chunk. Decryption verifies every tag,
//! so modified, truncated or extended streams fail with [`CryptoError::Integrity`]. The working
//! set is one chunk buffer plus a one-byte look-ahead, so multi-gigabyte files stream.

use crate::error::{CryptoError, Result};
use crate::kdf::VaultKeys;
use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce, Tag};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

pub const CHUNK_BYTES: usize = 1 << 20;
pub const TAG_BYTES: usize = 16;
pub const FILE_NONCE_BYTES: usize = 8;
pub const HEADER_BYTES: usize = 4 + FILE_NONCE_BYTES;
pub const MAGIC: [u8; 4] = *b"DSV1";
pub const CHUNK_AAD: &str = "drivesync-chunk-v1";
pub const MAX_CHUNK_INDEX: u64 = 0xFFFF_FFFF;

const BUFFER_BYTES: usize = 256 * 1024;

pub type FileNonce = [u8; FILE_NONCE_BYTES];
type NonceSource = Box<dyn Fn() -> io::Result<FileNonce> + Send + Sync>;

/// Encrypts and decrypts file contents with one vault's `keyEnc`.
pub struct VaultCipher {
    aes: Aes256Gcm,
    nonce_source: NonceSource,
}

impl std::fmt::Debug for VaultCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VaultCipher(<redacted>)")
    }
}

impl VaultCipher {
    /// A cipher whose file nonces come from the OS random number generator.
    pub fn new(keys: &VaultKeys) -> VaultCipher {
        Self::with_nonce_source(keys, random_file_nonce)
    }

    /// A cipher with a chosen file-nonce source. Only for known-answer tests: reusing a nonce with
    /// the same key breaks AES-GCM.
    pub fn with_nonce_source(
        keys: &VaultKeys,
        source: impl Fn() -> io::Result<FileNonce> + Send + Sync + 'static,
    ) -> VaultCipher {
        VaultCipher {
            aes: Aes256Gcm::new(keys.key_enc().into()),
            nonce_source: Box::new(source),
        }
    }

    /// Encrypts `src` into `dst` (replaced). On failure `dst` is removed. Returns the file nonce.
    pub fn encrypt_file(&self, src: &Path, dst: &Path) -> Result<FileNonce> {
        let run = || -> Result<FileNonce> {
            let input = BufReader::with_capacity(BUFFER_BYTES, std::fs::File::open(src)?);
            let mut output = BufWriter::with_capacity(BUFFER_BYTES, std::fs::File::create(dst)?);
            let nonce = self.encrypt(input, &mut output)?;
            output.into_inner().map_err(|e| e.into_error())?.sync_all()?;
            Ok(nonce)
        };
        run().inspect_err(|_| {
            let _ = std::fs::remove_file(dst);
        })
    }

    /// Decrypts `src` into `dst` (replaced). On failure `dst` is removed, so no unverified
    /// plaintext is left behind. Returns the plaintext size.
    pub fn decrypt_file(&self, src: &Path, dst: &Path) -> Result<u64> {
        let run = || -> Result<u64> {
            let input = BufReader::with_capacity(BUFFER_BYTES, std::fs::File::open(src)?);
            let mut output = BufWriter::with_capacity(BUFFER_BYTES, std::fs::File::create(dst)?);
            let n = self.decrypt(input, &mut output)?;
            output.into_inner().map_err(|e| e.into_error())?.sync_all()?;
            Ok(n)
        };
        run().inspect_err(|_| {
            let _ = std::fs::remove_file(dst);
        })
    }

    /// Encrypts `input` to `output`; neither is closed. Returns the file nonce.
    pub fn encrypt(&self, mut input: impl Read, mut output: impl Write) -> Result<FileNonce> {
        let file_nonce = (self.nonce_source)()?;
        output.write_all(&MAGIC)?;
        output.write_all(&file_nonce)?;
        let mut buf = vec![0u8; CHUNK_BYTES];
        let mut index: u64 = 0;
        let mut n = read_fully(&mut input, &mut buf)?;
        loop {
            let next = if n == CHUNK_BYTES {
                read_byte(&mut input)?
            } else {
                None
            };
            let last = next.is_none();
            let index32 =
                u32::try_from(index).map_err(|_| io::Error::other("file too large for the vault format"))?;
            let tag = self
                .aes
                .encrypt_in_place_detached(
                    Nonce::from_slice(&chunk_iv(&file_nonce, index32)),
                    &chunk_aad(index32, last),
                    &mut buf[..n],
                )
                .map_err(|_| io::Error::other("chunk encryption failed"))?;
            output.write_all(&buf[..n])?;
            output.write_all(&tag)?;
            let Some(first) = next else { break };
            buf[0] = first;
            n = 1 + read_fully(&mut input, &mut buf[1..])?;
            index += 1;
        }
        output.flush()?;
        Ok(file_nonce)
    }

    /// Decrypts `input` to `output`; neither is closed. Fails with [`CryptoError::Integrity`] for
    /// anything that does not verify. Returns the plaintext size.
    pub fn decrypt(&self, mut input: impl Read, mut output: impl Write) -> Result<u64> {
        let mut header = [0u8; HEADER_BYTES];
        if read_fully(&mut input, &mut header)? != HEADER_BYTES {
            return Err(integrity("not an encrypted vault file (shorter than the header)"));
        }
        if header[..MAGIC.len()] != MAGIC {
            return Err(integrity(
                "not an encrypted vault file (unknown format or version)",
            ));
        }
        let mut file_nonce: FileNonce = [0; FILE_NONCE_BYTES];
        file_nonce.copy_from_slice(&header[MAGIC.len()..]);
        let mut buf = vec![0u8; CHUNK_BYTES + TAG_BYTES];
        let mut index: u64 = 0;
        let mut total: u64 = 0;
        let mut n = read_fully(&mut input, &mut buf)?;
        loop {
            let next = if n == buf.len() {
                read_byte(&mut input)?
            } else {
                None
            };
            let last = next.is_none();
            if n < TAG_BYTES {
                return Err(integrity(&if index == 0 {
                    "encrypted file has no data chunk (truncated)".to_owned()
                } else {
                    format!("encrypted file is truncated after chunk {}", index - 1)
                }));
            }
            // A stream longer than the format allows is not something this key produced.
            let index32 = u32::try_from(index)
                .map_err(|_| integrity("encrypted file has more chunks than the format allows"))?;
            let (ct, tag) = buf[..n].split_at_mut(n - TAG_BYTES);
            let tag = Tag::clone_from_slice(tag);
            self.aes
                .decrypt_in_place_detached(
                    Nonce::from_slice(&chunk_iv(&file_nonce, index32)),
                    &chunk_aad(index32, last),
                    ct,
                    &tag,
                )
                .map_err(|_| {
                    integrity(&if last {
                        format!(
                            "authentication failed on the final chunk ({index}): the file was modified or truncated"
                        )
                    } else {
                        format!("authentication failed on chunk {index}: the file was modified")
                    })
                })?;
            output.write_all(ct)?;
            total += ct.len() as u64;
            let Some(first) = next else { break };
            buf[0] = first;
            n = 1 + read_fully(&mut input, &mut buf[1..])?;
            index += 1;
        }
        output.flush()?;
        Ok(total)
    }
}

impl VaultCipher {
    /// Decrypts one chunk on its own, for random access (files on demand). `ciphertext` is the
    /// chunk's bytes including its 16-byte tag, as laid out by [`chunk_cipher_range`]; `last` must
    /// be true exactly for the file's final chunk (see [`is_last_chunk`]). A chunk from another
    /// file, another position, or with the wrong last flag fails with [`CryptoError::Integrity`].
    pub fn decrypt_chunk(
        &self,
        file_nonce: &FileNonce,
        index: u32,
        last: bool,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>> {
        if ciphertext.len() < TAG_BYTES || ciphertext.len() > CHUNK_BYTES + TAG_BYTES {
            return Err(integrity(&format!(
                "chunk {index} has an impossible size ({} bytes)",
                ciphertext.len()
            )));
        }
        if !last && ciphertext.len() != CHUNK_BYTES + TAG_BYTES {
            return Err(integrity(&format!("chunk {index} is truncated")));
        }
        let (ct, tag) = ciphertext.split_at(ciphertext.len() - TAG_BYTES);
        let mut plain = ct.to_vec();
        self.aes
            .decrypt_in_place_detached(
                Nonce::from_slice(&chunk_iv(file_nonce, index)),
                &chunk_aad(index, last),
                &mut plain,
                Tag::from_slice(tag),
            )
            .map_err(|_| {
                integrity(&format!(
                    "authentication failed on chunk {index}: the file was modified, or the chunk belongs elsewhere"
                ))
            })?;
        Ok(plain)
    }
}

/// The file nonce from the first [`HEADER_BYTES`] of an encrypted file (`bytes` may be longer).
/// Fails with [`CryptoError::Integrity`] when the header is short or the magic is not `DSV1`.
pub fn read_header(bytes: &[u8]) -> Result<FileNonce> {
    if bytes.len() < HEADER_BYTES {
        return Err(integrity("not an encrypted vault file (shorter than the header)"));
    }
    if bytes[..MAGIC.len()] != MAGIC {
        return Err(integrity(
            "not an encrypted vault file (unknown format or version)",
        ));
    }
    let mut nonce = [0u8; FILE_NONCE_BYTES];
    nonce.copy_from_slice(&bytes[MAGIC.len()..HEADER_BYTES]);
    Ok(nonce)
}

/// Where chunk `index` of a file of `plain_size` plaintext bytes sits in the ciphertext:
/// `(offset, len)`, the length including the tag. `None` when the file has no such chunk.
pub fn chunk_cipher_range(plain_size: u64, index: u32) -> Option<(u64, u64)> {
    let (plain_offset, plain_len) = chunk_plain_range(plain_size, index)?;
    let offset = HEADER_BYTES as u64 + plain_offset + TAG_BYTES as u64 * u64::from(index);
    Some((offset, plain_len + TAG_BYTES as u64))
}

/// The plaintext bytes chunk `index` holds: `(offset, len)`. `None` when there is no such chunk.
pub fn chunk_plain_range(plain_size: u64, index: u32) -> Option<(u64, u64)> {
    let index = u64::from(index);
    if index >= chunk_count(plain_size) {
        return None;
    }
    let offset = index * CHUNK_BYTES as u64;
    Some((offset, (plain_size - offset).min(CHUNK_BYTES as u64)))
}

/// Whether chunk `index` is the final chunk of a file of `plain_size` bytes.
pub fn is_last_chunk(plain_size: u64, index: u32) -> bool {
    u64::from(index) + 1 == chunk_count(plain_size)
}

/// The chunk holding plaintext byte `offset` (the last chunk for `offset >= plain_size`).
pub fn chunk_index_of(plain_size: u64, offset: u64) -> u32 {
    let last = chunk_count(plain_size) - 1;
    u32::try_from((offset / CHUNK_BYTES as u64).min(last)).unwrap_or(u32::MAX)
}

fn integrity(message: &str) -> CryptoError {
    CryptoError::Integrity(message.to_owned())
}

/// 8 fresh bytes from the OS random number generator.
pub fn random_file_nonce() -> io::Result<FileNonce> {
    let mut nonce = [0u8; FILE_NONCE_BYTES];
    getrandom::fill(&mut nonce).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(nonce)
}

/// Number of chunks for a plaintext of `plain_size` bytes (at least one).
pub fn chunk_count(plain_size: u64) -> u64 {
    plain_size.div_ceil(CHUNK_BYTES as u64).max(1)
}

/// Exact ciphertext size for a plaintext of `plain_size` bytes: 12 + len + 16 × chunks.
pub fn encrypted_size(plain_size: u64) -> u64 {
    HEADER_BYTES as u64 + plain_size + TAG_BYTES as u64 * chunk_count(plain_size)
}

/// The plaintext size of a ciphertext of `cipher_size` bytes, or `None` when no encryption
/// produces that size (header only, or a final chunk shorter than its tag).
pub fn plaintext_size(cipher_size: u64) -> Option<u64> {
    let body = cipher_size.checked_sub(HEADER_BYTES as u64)?;
    let full = (CHUNK_BYTES + TAG_BYTES) as u64;
    let (whole, rest) = (body / full, body % full);
    let chunks = match rest {
        0 if whole > 0 => whole,
        r if r >= TAG_BYTES as u64 => whole + 1,
        _ => return None,
    };
    Some(body - TAG_BYTES as u64 * chunks)
}

/// The file nonce from an encrypted file's header, or `None` when the file does not start with a
/// valid header.
pub fn read_file_nonce(path: &Path) -> Option<FileNonce> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut header = [0u8; HEADER_BYTES];
    if read_fully(&mut file, &mut header).ok()? != HEADER_BYTES || header[..MAGIC.len()] != MAGIC {
        return None;
    }
    let mut nonce = [0u8; FILE_NONCE_BYTES];
    nonce.copy_from_slice(&header[MAGIC.len()..]);
    Some(nonce)
}

/// fileNonce || uint32 index.
pub fn chunk_iv(file_nonce: &FileNonce, index: u32) -> [u8; 12] {
    let mut iv = [0u8; 12];
    iv[..FILE_NONCE_BYTES].copy_from_slice(file_nonce);
    iv[FILE_NONCE_BYTES..].copy_from_slice(&index.to_be_bytes());
    iv
}

/// "drivesync-chunk-v1" || uint32 index || lastFlag.
pub fn chunk_aad(index: u32, last: bool) -> Vec<u8> {
    let mut aad = Vec::with_capacity(CHUNK_AAD.len() + 5);
    aad.extend_from_slice(CHUNK_AAD.as_bytes());
    aad.extend_from_slice(&index.to_be_bytes());
    aad.push(u8::from(last));
    aad
}

/// Reads until `buf` is full or the stream ends; returns the bytes read.
fn read_fully(input: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match input.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(r) => total += r,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}

fn read_byte(input: &mut impl Read) -> io::Result<Option<u8>> {
    let mut b = [0u8; 1];
    Ok((read_fully(input, &mut b)? == 1).then_some(b[0]))
}
