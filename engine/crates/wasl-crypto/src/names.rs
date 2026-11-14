//! Deterministic file and folder name encryption (spec section 4).
//!
//! ```text
//! nonce     = HMAC-SHA256(keyName, "name-nonce" || 0x00 || parentToken || 0x00 || nameUtf8)[0..12)
//! encrypted = base64url-no-padding(nonce || AES-256-GCM(keyEnc, nonce, nameUtf8, aad = "drivesync-name-v1")) + ".dsv"
//! ```
//!
//! The same plaintext name in the same parent always maps to the same ciphertext, so a re-created
//! folder keeps its Drive name. Decryption needs only the keys because the nonce is embedded.

use crate::error::{CryptoError, Result};
use crate::kdf::{VaultKeys, hmac_sha256};
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine as _;
use base64::alphabet::URL_SAFE;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};

pub const SUFFIX: &str = ".dsv";
pub const NONCE_BYTES: usize = 12;
pub const TAG_BYTES: usize = 16;
pub const AAD: &str = "drivesync-name-v1";
pub const NONCE_LABEL: &str = "name-nonce";

/// base64url without padding. Decoding tolerates non-zero trailing bits like Java's decoder and
/// Python's `urlsafe_b64decode` do, so every implementation accepts the same set of names.
const B64URL: GeneralPurpose = GeneralPurpose::new(
    &URL_SAFE,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::RequireNone)
        .with_decode_allow_trailing_bits(true),
);

/// True when `name` has the shape of a vault name (suffix only; the content is not checked).
pub fn is_vault_name(name: &str) -> bool {
    name.len() > SUFFIX.len() && name.ends_with(SUFFIX)
}

/// Encrypts `name` for the folder whose Drive id is `parent_token` ("" when unknown).
pub fn encrypt_name(keys: &VaultKeys, parent_token: &str, name: &str) -> Result<String> {
    if name.is_empty() {
        return Err(CryptoError::InvalidInput("name must not be empty".into()));
    }
    if name.contains('/') {
        return Err(CryptoError::InvalidInput("name must not contain '/'".into()));
    }
    // parity: like Kotlin, "." and ".." are accepted here although decrypt_name refuses them; the
    // engine never produces such names (they are not valid relative path segments).
    let plain = name.as_bytes();
    let nonce = name_nonce(keys, parent_token, plain);
    let ct = cipher(keys)
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plain,
                aad: AAD.as_bytes(),
            },
        )
        .map_err(|_| CryptoError::InvalidInput("name is too long to encrypt".into()))?;
    let mut raw = Vec::with_capacity(NONCE_BYTES + ct.len());
    raw.extend_from_slice(&nonce);
    raw.extend_from_slice(&ct);
    Ok(B64URL.encode(raw) + SUFFIX)
}

/// The plaintext name, or `None` when `encrypted_name` is not a valid name for these keys (foreign
/// file, tampered, wrong vault). Never panics on hostile input.
pub fn decrypt_name(keys: &VaultKeys, encrypted_name: &str) -> Option<String> {
    if !is_vault_name(encrypted_name) {
        return None;
    }
    let body = &encrypted_name[..encrypted_name.len() - SUFFIX.len()];
    if body.contains('=') {
        return None;
    }
    let raw = B64URL.decode(body).ok()?;
    if raw.len() < NONCE_BYTES + TAG_BYTES {
        return None;
    }
    let (nonce, ct) = raw.split_at(NONCE_BYTES);
    let plain = cipher(keys)
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ct,
                aad: AAD.as_bytes(),
            },
        )
        .ok()?;
    let name = String::from_utf8(plain).ok()?;
    // parity: Kotlin refuses exactly these; tools/decrypt_vault.py also refuses names containing
    // NUL. A NUL name never reaches disk here: the local file layer rejects it.
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        return None;
    }
    Some(name)
}

fn cipher(keys: &VaultKeys) -> Aes256Gcm {
    Aes256Gcm::new(keys.key_enc().into())
}

fn name_nonce(keys: &VaultKeys, parent_token: &str, name_utf8: &[u8]) -> [u8; NONCE_BYTES] {
    let label = NONCE_LABEL.as_bytes();
    let parent = parent_token.as_bytes();
    let mut msg = Vec::with_capacity(label.len() + 1 + parent.len() + 1 + name_utf8.len());
    msg.extend_from_slice(label);
    msg.push(0);
    msg.extend_from_slice(parent);
    msg.push(0);
    msg.extend_from_slice(name_utf8);
    let mac = hmac_sha256(keys.key_name(), &msg);
    let mut nonce = [0u8; NONCE_BYTES];
    nonce.copy_from_slice(&mac[..NONCE_BYTES]);
    nonce
}
