//! The v2 `vault.json` (ON_DEMAND.md §1.1): the only plaintext file of a v2 vault folder.
//!
//! ```json
//! { "format": "wasl-vault", "version": 2, "kdf": "argon2id", "m": 65536, "t": 3, "p": 1,
//!   "salt": "<base64, 16 bytes>", "keyCheck": "<base64, 32 bytes>",
//!   "created": "2026-10-21T12:34:56.789Z", "name": "Photos" }
//! ```
//!
//! `m` is in KiB. `version` is 2 for format 2.0 and 3 for format 2.1 ([`Format`]). Unknown keys
//! are ignored; another format, version or kdf is refused, and so are Argon2id costs outside the
//! reader bounds ([`KdfParams::check`]).

use super::format::Format;
use super::keys::{self, KEY_CHECK_BYTES, KdfParams, SALT_BYTES, VaultKeysV2};
use super::{B64, integrity};
use crate::error::Result;
use crate::manifest::{format_rfc3339_ms, parse_rfc3339_ms};
use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// The manifest as stored. Field order is the serialized order; every listed key is required.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultManifestV2 {
    pub format: String,
    pub version: i64,
    pub kdf: String,
    /// Argon2id memory in KiB.
    pub m: i64,
    pub t: i64,
    pub p: i64,
    /// Standard base64, 16 bytes.
    pub salt: String,
    /// Standard base64, 32 bytes.
    #[serde(rename = "keyCheck")]
    pub key_check: String,
    /// RFC 3339 instant in UTC.
    pub created: String,
    pub name: String,
}

impl VaultManifestV2 {
    pub const FILE_NAME: &'static str = "vault.json";
    pub const FORMAT: &'static str = "wasl-vault";
    /// The `version` of format 2.0; 2.1 writes [`Format::json_version`] of [`Format::V2_1`].
    pub const VERSION: i64 = 2;
    pub const KDF: &'static str = "argon2id";

    /// A new vault's `vault.json`, in the newest format ([`Format::LATEST`], 2.1).
    pub fn create(
        name: &str,
        salt: &[u8; SALT_BYTES],
        params: KdfParams,
        key_check: &[u8; KEY_CHECK_BYTES],
        created_ms: i64,
    ) -> VaultManifestV2 {
        Self::create_as(Format::LATEST, name, salt, params, key_check, created_ms)
    }

    /// A `vault.json` of a chosen format revision (tests and compatibility checks make 2.0 ones).
    pub fn create_as(
        format: Format,
        name: &str,
        salt: &[u8; SALT_BYTES],
        params: KdfParams,
        key_check: &[u8; KEY_CHECK_BYTES],
        created_ms: i64,
    ) -> VaultManifestV2 {
        VaultManifestV2 {
            format: Self::FORMAT.into(),
            version: format.json_version(),
            kdf: Self::KDF.into(),
            m: params.m_kib.into(),
            t: params.t.into(),
            p: params.p.into(),
            salt: B64.encode(salt),
            key_check: B64.encode(key_check),
            created: format_rfc3339_ms(created_ms),
            name: name.into(),
        }
    }

    /// Parses and validates; [`crate::CryptoError::Integrity`] with a reason otherwise.
    pub fn parse(text: &str) -> Result<VaultManifestV2> {
        let m: VaultManifestV2 = serde_json::from_str(text.trim())
            .map_err(|e| integrity(format!("vault.json is not a valid Wasl vault manifest ({e})")))?;
        if m.format != Self::FORMAT {
            return Err(integrity(format!(
                "vault.json has format \"{}\", expected \"{}\"",
                m.format,
                Self::FORMAT
            )));
        }
        if Format::from_json_version(m.version).is_none() {
            return Err(integrity(format!(
                "vault format version {} is not supported here (supports 2 and 3, formats 2.0 and 2.1)",
                m.version
            )));
        }
        if m.kdf != Self::KDF {
            return Err(integrity(format!(
                "vault KDF \"{}\" is not supported here (supports {})",
                m.kdf,
                Self::KDF
            )));
        }
        m.params()?;
        m.salt_bytes()?;
        m.key_check_bytes()?;
        Ok(m)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// The format revision: how files are cut and chunks sealed. Only meaningful on a parsed
    /// manifest (2.0 for anything else, which [`Self::parse`] never returns).
    pub fn format(&self) -> Format {
        Format::from_json_version(self.version).unwrap_or(Format::V2_0)
    }

    /// The Argon2id cost, checked against the reader bounds.
    pub fn params(&self) -> Result<KdfParams> {
        let conv = |v: i64| u32::try_from(v).unwrap_or(u32::MAX);
        let p = KdfParams {
            m_kib: conv(self.m),
            t: conv(self.t),
            p: conv(self.p),
        };
        p.check().map(|()| p)
    }

    pub fn salt_bytes(&self) -> Result<[u8; SALT_BYTES]> {
        B64.decode(&self.salt)
            .ok()
            .and_then(|v| <[u8; SALT_BYTES]>::try_from(v).ok())
            .ok_or_else(|| integrity("vault.json salt is not 16 bytes of base64"))
    }

    pub fn key_check_bytes(&self) -> Result<[u8; KEY_CHECK_BYTES]> {
        B64.decode(&self.key_check)
            .ok()
            .and_then(|v| <[u8; KEY_CHECK_BYTES]>::try_from(v).ok())
            .ok_or_else(|| integrity("vault.json keyCheck is not 32 bytes of base64"))
    }

    pub fn created_ms(&self) -> Option<i64> {
        parse_rfc3339_ms(&self.created)
    }

    /// Same salt, cost, key check and format revision. The revision counts: a `vault.json` edited
    /// on Drive from 2.1 back to 2.0 is refused, so nobody can make writers fall back to public
    /// chunking and exact sizes.
    pub fn same_vault_as(&self, other: &VaultManifestV2) -> bool {
        self.version == other.version
            && self.salt_bytes().ok() == other.salt_bytes().ok()
            && self.salt_bytes().is_ok()
            && (self.m, self.t, self.p) == (other.m, other.t, other.p)
            && self.key_check_bytes().ok() == other.key_check_bytes().ok()
    }

    /// Derives the keys (expensive). `Ok(None)` when the passphrase does not match the key check.
    pub fn verify_passphrase(&self, passphrase: &str) -> Result<Option<VaultKeysV2>> {
        let keys = keys::derive(passphrase, &self.salt_bytes()?, self.params()?)?;
        Ok(keys.key_check_matches(&self.key_check_bytes()?).then_some(keys))
    }
}
