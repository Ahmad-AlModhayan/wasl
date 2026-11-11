//! Client-side encryption for Wasl vaults, formats v1 and v2 (byte-exact spec:
//! `docs/ENCRYPTION.md`; v2 design: `docs/ON_DEMAND.md` §1).
//!
//! * [`kdf`]: passphrase → [`VaultKeys`] (PBKDF2-HMAC-SHA256) and the key check.
//! * [`manifest`]: `vault.json`, the only plaintext file in an encrypted Drive folder.
//! * [`names`]: deterministic file/folder name encryption (`<base64url>.dsv`).
//! * [`content`]: streaming, chunked AES-256-GCM file contents (`DSV1` framing), with random
//!   access to single chunks for files on demand.
//! * [`v2`]: vault format v2: Argon2id keys, AES-SIV names, content-defined chunks as separate
//!   objects, per-file manifests, range reads.
//! * [`recover`]: decrypting a whole downloaded vault folder, v1 or v2 (FR-V3).
//!
//! Built on primitive crates only (`aes-gcm`, `aes-siv`, `pbkdf2`, `argon2`, `hkdf`, `hmac`,
//! `sha2`, `blake3`, `fastcdc`, `base64`, `getrandom`, `zeroize`); the format is Wasl's own and interoperates with the Android app and
//! `tools/decrypt_vault.py`. Nothing here panics on hostile input: names, manifests and
//! ciphertext come from Drive, where anyone with access can write.

pub mod content;
mod error;
pub mod kdf;
pub mod manifest;
pub mod names;
pub mod recover;
pub mod v2;

pub use content::VaultCipher;
pub use error::{CryptoError, Result};
pub use kdf::VaultKeys;
pub use manifest::VaultManifest;

/// Lowercase hex SHA-256 of `data`. Not part of the format: callers use it to derive stable,
/// opaque file names (for example temp files keyed by an upload session).
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}
