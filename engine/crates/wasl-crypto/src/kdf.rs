//! Passphrase → keys (spec section 2).
//!
//! ```text
//! master   = PBKDF2-HMAC-SHA256(passphrase UTF-8, salt, iterations, 64 bytes)
//! keyEnc   = master[0..32)     AES-256-GCM key for names and contents
//! keyName  = master[32..64)    HMAC-SHA256 key for name nonces and the key check
//! keyCheck = HMAC-SHA256(keyName, "drivesync-vault-check-v1")
//! ```
//!
//! `keyCheck` lets a device reject a wrong passphrase before touching any file, without storing
//! anything derived from `keyEnc`.

use crate::error::{CryptoError, Result};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

pub const DEFAULT_ITERATIONS: u32 = 600_000;
pub const SALT_BYTES: usize = 16;
pub const KEY_BYTES: usize = 32;
pub const MASTER_BYTES: usize = 64;
pub const KEY_CHECK_BYTES: usize = 32;
pub const KEY_CHECK_MESSAGE: &str = "drivesync-vault-check-v1";

type HmacSha256 = Hmac<Sha256>;

/// The two 256-bit keys derived from a vault passphrase. Both are overwritten with zeros when the
/// value is dropped (or on [`Zeroize::zeroize`]); `Debug` never prints them.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct VaultKeys {
    key_enc: [u8; KEY_BYTES],
    key_name: [u8; KEY_BYTES],
}

impl VaultKeys {
    pub fn new(key_enc: [u8; KEY_BYTES], key_name: [u8; KEY_BYTES]) -> VaultKeys {
        VaultKeys { key_enc, key_name }
    }

    /// Splits the 64-byte PBKDF2 output: keyEnc = master[0..32), keyName = master[32..64).
    pub fn from_master(master: &[u8]) -> Result<VaultKeys> {
        if master.len() != MASTER_BYTES {
            return Err(CryptoError::InvalidInput(format!(
                "master key must be {MASTER_BYTES} bytes"
            )));
        }
        let mut keys = VaultKeys::new([0; KEY_BYTES], [0; KEY_BYTES]);
        keys.key_enc.copy_from_slice(&master[..KEY_BYTES]);
        keys.key_name.copy_from_slice(&master[KEY_BYTES..]);
        Ok(keys)
    }

    /// AES-256-GCM key for names and contents.
    pub fn key_enc(&self) -> &[u8; KEY_BYTES] {
        &self.key_enc
    }

    /// HMAC-SHA256 key for name nonces and the key check.
    pub fn key_name(&self) -> &[u8; KEY_BYTES] {
        &self.key_name
    }
}

impl std::fmt::Debug for VaultKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VaultKeys(<redacted>)")
    }
}

/// 16 fresh bytes from the OS random number generator.
pub fn new_salt() -> Result<[u8; SALT_BYTES]> {
    let mut salt = [0u8; SALT_BYTES];
    getrandom::fill(&mut salt).map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(salt)
}

/// Derives the vault keys. Costs `iterations` PBKDF2 rounds (about a second at the default on a
/// phone): run it off any UI or async executor thread.
pub fn derive(passphrase: &str, salt: &[u8], iterations: u32) -> Result<VaultKeys> {
    if passphrase.is_empty() {
        return Err(CryptoError::InvalidInput("passphrase must not be empty".into()));
    }
    if salt.is_empty() {
        return Err(CryptoError::InvalidInput("salt must not be empty".into()));
    }
    let master = Zeroizing::new(pbkdf2_hmac_sha256(
        passphrase.as_bytes(),
        salt,
        iterations,
        MASTER_BYTES,
    )?);
    VaultKeys::from_master(&master)
}

/// HMAC-SHA256(keyName, "drivesync-vault-check-v1").
pub fn key_check(keys: &VaultKeys) -> [u8; KEY_CHECK_BYTES] {
    hmac_sha256(keys.key_name(), KEY_CHECK_MESSAGE.as_bytes())
}

/// Constant-time comparison of the computed key check against a stored one.
pub fn key_check_matches(keys: &VaultKeys, expected: &[u8]) -> bool {
    let mut mac = new_mac(keys.key_name());
    mac.update(KEY_CHECK_MESSAGE.as_bytes());
    // `verify_slice` compares in constant time and refuses a wrong length.
    mac.verify_slice(expected).is_ok()
}

pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = new_mac(key);
    mac.update(data);
    mac.finalize().into_bytes().into()
}

fn new_mac(key: &[u8]) -> HmacSha256 {
    // HMAC accepts keys of any length, so this cannot fail.
    match <HmacSha256 as Mac>::new_from_slice(key) {
        Ok(mac) => mac,
        Err(_) => unreachable!("HMAC takes keys of any length"),
    }
}

/// RFC 8018 section 5.2 with PRF = HMAC-SHA256. The passphrase is always its raw bytes (UTF-8 for
/// text), so the result is the same on every platform.
pub fn pbkdf2_hmac_sha256(password: &[u8], salt: &[u8], iterations: u32, dk_len: usize) -> Result<Vec<u8>> {
    if password.is_empty() {
        return Err(CryptoError::InvalidInput("password must not be empty".into()));
    }
    if iterations < 1 {
        return Err(CryptoError::InvalidInput("iterations must be >= 1".into()));
    }
    if dk_len < 1 {
        return Err(CryptoError::InvalidInput("dkLen must be >= 1".into()));
    }
    let mut out = vec![0u8; dk_len];
    pbkdf2::pbkdf2_hmac::<Sha256>(password, salt, iterations, &mut out);
    Ok(out)
}
