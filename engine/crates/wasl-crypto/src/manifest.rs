//! `vault.json` (spec section 3): the only plaintext file in an encrypted Drive folder. It carries
//! what a second device (or the desktop tool) needs to derive the keys from the passphrase, plus a
//! key check so a wrong passphrase is detected before anything is decrypted. No key material.

use crate::error::{CryptoError, Result};
use crate::kdf::{self, KEY_CHECK_BYTES, VaultKeys};
use base64::Engine as _;
use base64::alphabet::STANDARD;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use serde::{Deserialize, Serialize};

/// Standard base64. Decoding accepts missing padding and non-zero trailing bits, like the Java
/// decoder the Android app parses with.
const B64: GeneralPurpose = GeneralPurpose::new(
    &STANDARD,
    GeneralPurposeConfig::new()
        .with_encode_padding(true)
        .with_decode_padding_mode(DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true),
);

/// The manifest as stored. Field order is the serialized order. Unknown keys are ignored when
/// parsing; every listed key is required.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultManifest {
    pub format: String,
    pub version: i64,
    pub kdf: String,
    pub iterations: i64,
    /// Standard base64, 16 bytes.
    pub salt: String,
    /// Standard base64, HMAC-SHA256(keyName, "drivesync-vault-check-v1").
    #[serde(rename = "keyCheck")]
    pub key_check: String,
    /// RFC 3339 instant in UTC, e.g. `2026-09-30T12:34:56.789Z`.
    pub created: String,
    pub name: String,
}

impl VaultManifest {
    pub const FILE_NAME: &'static str = "vault.json";
    pub const FORMAT: &'static str = "drivesync-vault";
    pub const VERSION: i64 = 1;
    pub const KDF: &'static str = "PBKDF2-HMAC-SHA256";

    /// A v1 manifest for a vault with this salt, iteration count and key check, created at
    /// `created_ms` (milliseconds since the Unix epoch).
    pub fn create(
        name: &str,
        salt: &[u8],
        iterations: u32,
        key_check: &[u8],
        created_ms: i64,
    ) -> VaultManifest {
        VaultManifest {
            format: Self::FORMAT.into(),
            version: Self::VERSION,
            kdf: Self::KDF.into(),
            iterations: i64::from(iterations),
            salt: B64.encode(salt),
            key_check: B64.encode(key_check),
            created: format_rfc3339_ms(created_ms),
            name: name.into(),
        }
    }

    /// Parses and validates. Fails with [`CryptoError::Integrity`] and a specific reason for
    /// anything this version cannot use (other format, version or KDF, bad fields).
    pub fn parse(text: &str) -> Result<VaultManifest> {
        let m: VaultManifest = serde_json::from_str(text.trim()).map_err(|e| {
            integrity(format!(
                "vault.json is not a valid Drive Sync vault manifest ({e})"
            ))
        })?;
        if m.format != Self::FORMAT {
            return Err(integrity(format!(
                "vault.json has format \"{}\", expected \"{}\"",
                m.format,
                Self::FORMAT
            )));
        }
        if m.version != Self::VERSION {
            return Err(integrity(format!(
                "vault format version {} is not supported by this app (supports {})",
                m.version,
                Self::VERSION
            )));
        }
        if m.kdf != Self::KDF {
            return Err(integrity(format!(
                "vault KDF \"{}\" is not supported by this app (supports {})",
                m.kdf,
                Self::KDF
            )));
        }
        check_iterations(m.iterations)?;
        let salt = m
            .salt_bytes()
            .map_err(|_| integrity("vault.json salt is not valid base64".into()))?;
        if salt.len() < 8 {
            return Err(integrity(format!(
                "vault.json salt is too short ({} bytes)",
                salt.len()
            )));
        }
        let check = m
            .key_check_bytes()
            .map_err(|_| integrity("vault.json keyCheck is not valid base64".into()))?;
        if check.len() != KEY_CHECK_BYTES {
            return Err(integrity(format!(
                "vault.json keyCheck has the wrong length ({} bytes)",
                check.len()
            )));
        }
        Ok(m)
    }

    /// Pretty-printed JSON, pleasant to paste on another device.
    pub fn to_json(&self) -> String {
        // Serializing plain strings and integers cannot fail.
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn salt_bytes(&self) -> Result<Vec<u8>> {
        B64.decode(&self.salt)
            .map_err(|e| CryptoError::InvalidInput(format!("salt is not valid base64: {e}")))
    }

    pub fn key_check_bytes(&self) -> Result<Vec<u8>> {
        B64.decode(&self.key_check)
            .map_err(|e| CryptoError::InvalidInput(format!("keyCheck is not valid base64: {e}")))
    }

    /// `created` as milliseconds since the Unix epoch, when it is a UTC RFC 3339 instant.
    pub fn created_ms(&self) -> Option<i64> {
        parse_rfc3339_ms(&self.created)
    }

    /// True when both manifests describe the same vault (same salt, iterations and key check).
    pub fn same_vault_as(&self, other: &VaultManifest) -> bool {
        // parity: Kotlin compares the salt as text and the key check as bytes.
        let checks = match (self.key_check_bytes(), other.key_check_bytes()) {
            (Ok(a), Ok(b)) => constant_time_eq(&a, &b),
            _ => false,
        };
        self.salt == other.salt && self.iterations == other.iterations && checks
    }

    /// Derives the keys for `passphrase` (expensive: `iterations` PBKDF2 rounds). `Ok(None)` when
    /// the passphrase does not match the key check. The manifest should come from [`Self::parse`].
    /// The iteration count is bounded here too, so a hand-built manifest cannot stall the caller.
    pub fn verify_passphrase(&self, passphrase: &str) -> Result<Option<VaultKeys>> {
        let iterations = check_iterations(self.iterations)?;
        let keys = kdf::derive(passphrase, &self.salt_bytes()?, iterations)?;
        Ok(kdf::key_check_matches(&keys, &self.key_check_bytes()?).then_some(keys))
    }
}

/// Fewest PBKDF2 iterations a manifest may ask for. The Android app (1.1.4) only ever creates
/// vaults with the 600,000 default; anything below this is too weak to be one of ours.
pub const MIN_ITERATIONS: i64 = 100_000;
/// Most PBKDF2 iterations a manifest may ask for (about 16× the default). `vault.json` sits on
/// Drive where anyone with write access can edit it; without a bound a huge count would hang
/// every device that checks a passphrase against it.
pub const MAX_ITERATIONS: i64 = 10_000_000;

/// The iteration count as `u32`, or an Integrity error when it is outside
/// [`MIN_ITERATIONS`]..=[`MAX_ITERATIONS`].
fn check_iterations(iterations: i64) -> Result<u32> {
    if !(MIN_ITERATIONS..=MAX_ITERATIONS).contains(&iterations) {
        return Err(integrity(format!(
            "vault.json has an invalid iteration count ({iterations}; allowed {MIN_ITERATIONS} to {MAX_ITERATIONS})"
        )));
    }
    u32::try_from(iterations).map_err(|_| integrity(format!("invalid iteration count ({iterations})")))
}

fn integrity(message: String) -> CryptoError {
    CryptoError::Integrity(message)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    std::hint::black_box(diff) == 0
}

// ---------------------------------------------------------------- RFC 3339 (UTC only)

/// `2023-11-14T22:13:20Z`, with `.mmm` only when the milliseconds are not zero (like
/// `java.time.Instant.toString`).
pub(crate) fn format_rfc3339_ms(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let (days, sod) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    let (hh, mm, ss) = (sod / 3600, sod % 3600 / 60, sod % 60);
    let frac = if millis == 0 {
        String::new()
    } else {
        format!(".{millis:03}")
    };
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}{frac}Z")
}

/// Parses `YYYY-MM-DDTHH:MM:SS[.fraction]Z` (also a lowercase `t`/`z`). Offsets other than `Z`
/// are not accepted; `None` for anything else.
pub(crate) fn parse_rfc3339_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 || !s.is_ascii() {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let t = &s[r];
        t.bytes().all(|c| c.is_ascii_digit()).then(|| t.parse().ok())?
    };
    let sep_ok =
        b[4] == b'-' && b[7] == b'-' && (b[10] == b'T' || b[10] == b't') && b[13] == b':' && b[16] == b':';
    if !sep_ok || !matches!(b[b.len() - 1], b'Z' | b'z') {
        return None;
    }
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let rest = &s[19..s.len() - 1];
    let millis = match rest.strip_prefix('.') {
        None if rest.is_empty() => 0,
        Some(f) if !f.is_empty() && f.len() <= 9 && f.bytes().all(|c| c.is_ascii_digit()) => {
            let mut digits = f.to_owned();
            digits.truncate(3);
            while digits.len() < 3 {
                digits.push('0');
            }
            digits.parse::<i64>().ok()?
        }
        _ => return None,
    };
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let days = days_from_civil(y, mo, d);
    Some(((days * 86_400 + h * 3600 + mi * 60 + sec) * 1000) + millis)
}

/// Howard Hinnant's `days_from_civil`: days since 1970-01-01 for a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`].
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_round_trips() {
        for ms in [
            0,
            1,
            999,
            1_700_000_000_000,
            1_759_235_696_789,
            951_782_400_000,
            -1,
        ] {
            assert_eq!(Some(ms), parse_rfc3339_ms(&format_rfc3339_ms(ms)), "{ms}");
        }
        assert_eq!("2023-11-14T22:13:20Z", format_rfc3339_ms(1_700_000_000_000));
        assert_eq!("2000-02-29T00:00:00Z", format_rfc3339_ms(951_782_400_000));
        assert_eq!(
            Some(1_759_235_696_789),
            parse_rfc3339_ms("2025-09-30T12:34:56.789123Z")
        );
        for bad in [
            "",
            "2025-09-30",
            "2025-09-30T12:34:56+02:00",
            "2025-13-01T00:00:00Z",
            "2025-09-30T12:34:56.Z",
            "２０２５-09-30T12:34:56Z",
        ] {
            assert_eq!(None, parse_rfc3339_ms(bad), "{bad}");
        }
    }
}
