//! Passphrase → keys (ON_DEMAND.md §1.1).
//!
//! ```text
//! ikm      = Argon2id(passphrase UTF-8, salt (16), m KiB, t, p, version 0x13, 32 bytes)
//! kName    = HKDF-SHA256(salt = none, ikm, info = "wasl-v2-name",  64)   AES-256-SIV key
//! kMeta    = HKDF-SHA256(salt = none, ikm, info = "wasl-v2-meta",  32)   AES-256-GCM key
//! kChunk   = HKDF-SHA256(salt = none, ikm, info = "wasl-v2-chunk", 32)   AES-256-GCM key
//! kChunkId = HKDF-SHA256(salt = none, ikm, info = "wasl-v2-cid",   32)   HMAC-SHA256 key
//! kDir     = HKDF-SHA256(salt = none, ikm, info = "wasl-v2-dir",   32)   HMAC-SHA256 key (folder ids)
//! kCdc     = HKDF-SHA256(salt = none, ikm, info = "wasl-v2-cdc",   32)   chunking key (format 2.1)
//! kState   = HKDF-SHA256(salt = none, ikm, info = "wasl-v2-state", 32)   HMAC-SHA256 key (state records)
//! keyCheck = HMAC-SHA256(HKDF-SHA256(salt = none, ikm, "wasl-v2-check", 32), "wasl-vault-check-v2")
//! ```

use crate::error::{CryptoError, Result};
use crate::kdf::hmac_sha256;
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

pub const SALT_BYTES: usize = 16;
pub const IKM_BYTES: usize = 32;
pub const KEY_CHECK_BYTES: usize = 32;
pub const INFO_NAME: &str = "wasl-v2-name";
pub const INFO_META: &str = "wasl-v2-meta";
pub const INFO_CHUNK: &str = "wasl-v2-chunk";
pub const INFO_CHUNK_ID: &str = "wasl-v2-cid";
pub const INFO_DIR: &str = "wasl-v2-dir";
pub const INFO_CHECK: &str = "wasl-v2-check";
/// Format 2.1: the key the FastCDC gear table is drawn from ([`super::chunks::Gear`]).
pub const INFO_CDC: &str = "wasl-v2-cdc";
/// The key authenticating vault state records ([`super::state`]).
pub const INFO_STATE: &str = "wasl-v2-state";
pub const KEY_CHECK_MESSAGE: &str = "wasl-vault-check-v2";

/// Argon2id cost. `m_kib` is in KiB (as Argon2 counts it): the default 65536 is 64 MiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

impl KdfParams {
    /// m = 64 MiB, t = 3, p = 1.
    pub const DEFAULT: KdfParams = KdfParams {
        m_kib: 64 * 1024,
        t: 3,
        p: 1,
    };
    /// The cheapest a reader accepts (19 MiB, t = 1, p = 1): tests use it.
    pub const MIN: KdfParams = KdfParams {
        m_kib: MIN_M_KIB,
        t: 1,
        p: 1,
    };

    /// Refuses costs outside the bounds every reader enforces: `vault.json` sits on Drive where
    /// anyone with write access can edit it, and an unbounded cost would hang every device. The
    /// bounds are a few times what Wasl creates, not what Argon2 allows: a phone must survive
    /// opening a vault someone tampered with (security review 2026-10).
    pub fn check(&self) -> Result<()> {
        let ok = (MIN_M_KIB..=MAX_M_KIB).contains(&self.m_kib)
            && (MIN_T..=MAX_T).contains(&self.t)
            && (MIN_P..=MAX_P).contains(&self.p)
            && u64::from(self.m_kib) * u64::from(self.t) <= MAX_M_T_KIB;
        if ok {
            Ok(())
        } else {
            Err(CryptoError::Integrity(format!(
                "Argon2id parameters m = {} KiB, t = {}, p = {} are outside the allowed range \
                 (m {MIN_M_KIB}..={MAX_M_KIB} KiB, t {MIN_T}..={MAX_T}, p {MIN_P}..={MAX_P}, \
                 m × t at most {MAX_M_T_KIB} KiB)",
                self.m_kib, self.t, self.p
            )))
        }
    }
}

impl Default for KdfParams {
    fn default() -> Self {
        KdfParams::DEFAULT
    }
}

pub const MIN_M_KIB: u32 = 19 * 1024;
/// 256 MiB: four times the default, and still allocatable on a low-end phone.
pub const MAX_M_KIB: u32 = 256 * 1024;
pub const MIN_T: u32 = 1;
pub const MAX_T: u32 = 4;
pub const MIN_P: u32 = 1;
/// Wasl creates p = 1; more lanes only help a reader with that many idle cores.
pub const MAX_P: u32 = 4;
/// Total work m × t in KiB-passes: 512 MiB-passes, under three times the default 64 MiB × 3.
/// Without it the two maxima together would cost 1 GiB-passes, seconds on a slow phone.
pub const MAX_M_T_KIB: u64 = 512 * 1024;

/// The v2 keys. Everything is overwritten with zeros on drop; `Debug` never prints them.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct VaultKeysV2 {
    ikm: [u8; IKM_BYTES],
    name: [u8; 64],
    meta: [u8; 32],
    chunk: [u8; 32],
    chunk_id: [u8; 32],
    dir: [u8; 32],
    cdc: [u8; 32],
    state: [u8; 32],
    check: [u8; KEY_CHECK_BYTES],
}

impl std::fmt::Debug for VaultKeysV2 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VaultKeysV2(<redacted>)")
    }
}

impl VaultKeysV2 {
    /// Expands the Argon2id output into the sub-keys. The `ikm` is what a device stores (wrapped)
    /// to stay unlocked; see [`Self::ikm`].
    pub fn from_ikm(ikm: &[u8; IKM_BYTES]) -> VaultKeysV2 {
        let hk = Hkdf::<Sha256>::new(None, ikm);
        let mut keys = VaultKeysV2 {
            ikm: *ikm,
            name: [0; 64],
            meta: [0; 32],
            chunk: [0; 32],
            chunk_id: [0; 32],
            dir: [0; 32],
            cdc: [0; 32],
            state: [0; 32],
            check: [0; KEY_CHECK_BYTES],
        };
        // HKDF-SHA256 yields up to 8160 bytes, so these cannot fail.
        let expand = |info: &str, out: &mut [u8]| {
            if hk.expand(info.as_bytes(), out).is_err() {
                unreachable!("HKDF output length is far below its limit")
            }
        };
        expand(INFO_NAME, &mut keys.name);
        expand(INFO_META, &mut keys.meta);
        expand(INFO_CHUNK, &mut keys.chunk);
        expand(INFO_CHUNK_ID, &mut keys.chunk_id);
        expand(INFO_DIR, &mut keys.dir);
        // Separate HKDF labels keep every sub-key independent of the others: knowing how a 2.1
        // vault cuts its files (kCdc) or seeing its state records (kState) says nothing about the
        // keys that seal names, manifests and chunks.
        expand(INFO_CDC, &mut keys.cdc);
        expand(INFO_STATE, &mut keys.state);
        let mut check_key = Zeroizing::new([0u8; 32]);
        expand(INFO_CHECK, &mut check_key[..]);
        keys.check = hmac_sha256(&check_key[..], KEY_CHECK_MESSAGE.as_bytes());
        keys
    }

    /// The Argon2id output all other keys come from: the secret a device keeps (wrapped by the
    /// Android Keystore or the OS keychain) instead of the passphrase.
    pub fn ikm(&self) -> &[u8; IKM_BYTES] {
        &self.ikm
    }

    /// The value stored as `keyCheck` in vault.json.
    pub fn key_check(&self) -> [u8; KEY_CHECK_BYTES] {
        self.check
    }

    /// Constant-time comparison with a stored key check.
    pub fn key_check_matches(&self, expected: &[u8]) -> bool {
        if expected.len() != KEY_CHECK_BYTES {
            return false;
        }
        let diff = self
            .check
            .iter()
            .zip(expected)
            .fold(0u8, |acc, (x, y)| acc | (x ^ y));
        std::hint::black_box(diff) == 0
    }

    /// AES-256-SIV key (64 bytes) for names.
    pub fn k_name(&self) -> &[u8; 64] {
        &self.name
    }

    /// AES-256-GCM key for file manifests and the `wasl.m` blob.
    pub fn k_meta(&self) -> &[u8; 32] {
        &self.meta
    }

    /// AES-256-GCM key for chunk objects.
    pub fn k_chunk(&self) -> &[u8; 32] {
        &self.chunk
    }

    /// HMAC-SHA256 key for chunk ids.
    pub fn k_chunk_id(&self) -> &[u8; 32] {
        &self.chunk_id
    }

    /// HMAC-SHA256 key authenticating folder ids (`.wasl-dir`, `wasl.dir`).
    pub fn k_dir(&self) -> &[u8; 32] {
        &self.dir
    }

    /// Format 2.1: the key the chunking gear table is derived from.
    pub fn k_cdc(&self) -> &[u8; 32] {
        &self.cdc
    }

    /// HMAC-SHA256 key authenticating vault state records.
    pub fn k_state(&self) -> &[u8; 32] {
        &self.state
    }
}

/// 16 fresh bytes from the OS random number generator.
pub fn new_salt() -> Result<[u8; SALT_BYTES]> {
    super::random()
}

/// Argon2id(passphrase, salt, params) → keys. Costs `params.m_kib` KiB of memory and about a second
/// at the default on a phone: run it off any UI or async executor thread. The parameters are
/// checked against the reader bounds first.
pub fn derive(passphrase: &str, salt: &[u8], params: KdfParams) -> Result<VaultKeysV2> {
    let ikm = argon2id(passphrase, salt, params)?;
    Ok(VaultKeysV2::from_ikm(&ikm))
}

/// The raw Argon2id step (exposed for test vectors).
pub fn argon2id(passphrase: &str, salt: &[u8], params: KdfParams) -> Result<Zeroizing<[u8; IKM_BYTES]>> {
    params.check()?;
    if passphrase.is_empty() {
        return Err(CryptoError::InvalidInput("passphrase must not be empty".into()));
    }
    if salt.len() != SALT_BYTES {
        return Err(CryptoError::InvalidInput(format!(
            "salt must be {SALT_BYTES} bytes"
        )));
    }
    let p = argon2::Params::new(params.m_kib, params.t, params.p, Some(IKM_BYTES))
        .map_err(|e| CryptoError::InvalidInput(format!("Argon2id parameters: {e}")))?;
    let a = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, p);
    let mut out = Zeroizing::new([0u8; IKM_BYTES]);
    a.hash_password_into(passphrase.as_bytes(), salt, &mut out[..])
        .map_err(|e| CryptoError::InvalidInput(format!("Argon2id: {e}")))?;
    Ok(out)
}
