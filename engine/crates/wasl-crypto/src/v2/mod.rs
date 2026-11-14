//! Vault format v2 (byte-exact spec: `docs/ENCRYPTION.md` "Format v2"; design:
//! `docs/ON_DEMAND.md` §1). v1 ([`crate::content`], [`crate::names`]) stays as it is.
//!
//! v2 cuts every file into content-defined chunks, each its own encrypted Drive object, and keeps a
//! small encrypted manifest per file listing them. An edit uploads only the chunks that changed, and
//! a reader decrypts any byte range by fetching only the chunks covering it.
//!
//! * [`keys`]: passphrase → [`VaultKeysV2`] (Argon2id, then HKDF-SHA256 sub-keys) and the key check.
//! * [`vault_json`]: the v2 `vault.json` ([`VaultManifestV2`]).
//! * [`names`]: AES-256-SIV names bound to their parent folder's [`DirId`] (`<base64url>.w2`).
//! * [`manifest`]: the per-file manifest (`W2M1`) and the compact `wasl.m` listing blob.
//! * [`chunks`]: FastCDC cutting, chunk ids, chunk object names and sealing (`W2C1`).
//! * [`file`]: the streaming encryptor, range reads, whole-file decryption and update plans.
//! * [`layout`]: the Drive folder layout written to / read from a local directory (recovery).
//! * [`format`]: the format revision, 2.0 or 2.1 (keyed chunking and padded chunks).
//! * [`state`]: per-writer vault state records, for detecting a rolled-back folder.
//!
//! Every chunk is bound to its file (`file_id`) and its own keyed content id, not to its index
//! (design decision, ON_DEMAND.md §1.2): an insertion does not change the chunks after it, and the
//! order is authenticated by the manifest's chunk list and the whole file by its BLAKE3.

mod cbor;
pub mod chunks;
pub mod file;
pub mod format;
pub mod keys;
pub mod layout;
pub mod manifest;
pub mod names;
pub mod state;
pub mod vault_json;

pub use chunks::ChunkId;
pub use file::{
    PendingChunk, RangeReader, UpdatePlan, decrypt_file, encrypt_stream, encrypt_update, plan_update,
    read_range,
};
pub use format::Format;
pub use keys::{KdfParams, VaultKeysV2};
pub use manifest::{ChunkRef, FileId, FileManifest, MetaBlob};
pub use names::{DirId, Placement};
pub use vault_json::VaultManifestV2;

use base64::alphabet::{STANDARD, URL_SAFE};
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};

/// base64url without padding, canonical only: padding and non-zero trailing bits are refused, so
/// every byte string has exactly one accepted spelling (a name's Drive spelling is unique).
pub(crate) const B64URL: GeneralPurpose = GeneralPurpose::new(
    &URL_SAFE,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::RequireNone)
        .with_decode_allow_trailing_bits(false),
);

/// Standard base64 with padding, canonical only (vault.json fields, the `wasl.m` blob).
pub(crate) const B64: GeneralPurpose = GeneralPurpose::new(
    &STANDARD,
    GeneralPurposeConfig::new()
        .with_encode_padding(true)
        .with_decode_padding_mode(DecodePaddingMode::RequireCanonical)
        .with_decode_allow_trailing_bits(false),
);

pub(crate) fn integrity(message: impl Into<String>) -> crate::CryptoError {
    crate::CryptoError::Integrity(message.into())
}

pub(crate) fn random<const N: usize>() -> crate::Result<[u8; N]> {
    let mut out = [0u8; N];
    getrandom::fill(&mut out).map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(out)
}

/// Lowercase hex.
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
