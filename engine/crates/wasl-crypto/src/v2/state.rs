//! Vault state records (ENCRYPTION.md §11.11, ON_DEMAND.md §1.6): rollback detection for a whole
//! vault folder (security review S4-4).
//!
//! Per-file manifest versions ([`super::FileManifest::check_not_older_than`]) catch an older
//! manifest put back for a file a device has seen, but not a whole folder put back to an earlier,
//! consistent state, nor a deleted file brought back. Every writing device therefore keeps one
//! small authenticated record at the vault root:
//!
//! ```text
//! name    = ".wasl-state." || hex(writer)              (writer: 8 bytes, random, kept per device)
//! record  = "W2S1" || writer (8) || counter (u64 BE) || time_ms (i64 BE) || digest (32) || tag (32)
//! tag     = HMAC-SHA256(kState, "wasl-v2-state" || writer || counter || time_ms || digest)
//! digest  = SHA-256("wasl-v2-state-set" || u64 BE n || for each live file, sorted by
//!                    (placement, listing blob): u32 BE len || placement || u32 BE len || blob)
//! ```
//!
//! Only its own device writes a record, so its counter only grows and concurrent writers never
//! race on one object. A reader remembers the highest counter it has seen per writer and the
//! highest sum of all counters; a record that went back, or a record that vanished (the sum went
//! down), means the folder is older than one this device already saw.
//!
//! The digest covers what a listing shows of every live file: its placement (folder id and
//! encrypted name) and its `wasl.m` blob, which is sealed with a fresh nonce on every write, so it
//! changes with every version and cannot be forged. It needs no manifest download. A listing that
//! matches the newest record is a state some writer published; one that does not was either
//! changed by a writer that has not published yet (it is mid-run, or crashed) or tampered with
//! (a file put back, removed or rolled back while the records were kept). Readers report that,
//! they cannot tell the two apart.

use super::integrity;
use crate::error::Result;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

pub const MAGIC: [u8; 4] = *b"W2S1";
/// Record objects at the vault root are named this, then the writer's id in hex.
pub const NAME_PREFIX: &str = ".wasl-state.";
pub const TAG_LABEL: &str = "wasl-v2-state";
pub const SET_LABEL: &str = "wasl-v2-state-set";
pub const WRITER_BYTES: usize = 8;
pub const DIGEST_BYTES: usize = 32;
const TAG_BYTES: usize = 32;
/// A record's exact size.
pub const RECORD_BYTES: usize = MAGIC.len() + WRITER_BYTES + 8 + 8 + DIGEST_BYTES + TAG_BYTES;

/// A writing device's id within one vault: random, kept by the device.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WriterId(pub [u8; WRITER_BYTES]);

impl WriterId {
    pub fn random() -> Result<WriterId> {
        Ok(WriterId(super::random()?))
    }

    pub fn from_u64(v: u64) -> WriterId {
        WriterId(v.to_be_bytes())
    }

    pub fn to_u64(self) -> u64 {
        u64::from_be_bytes(self.0)
    }

    /// `.wasl-state.<16 hex>`.
    pub fn object_name(&self) -> String {
        format!("{NAME_PREFIX}{}", super::hex(&self.0))
    }

    /// The writer a record object's name belongs to: exactly 16 lowercase hex digits after the
    /// prefix, `None` otherwise.
    pub fn from_object_name(name: &str) -> Option<WriterId> {
        let hex = name.strip_prefix(NAME_PREFIX)?;
        if hex.len() != WRITER_BYTES * 2 || !hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return None;
        }
        let mut out = [0u8; WRITER_BYTES];
        for (i, pair) in hex.as_bytes().chunks(2).enumerate() {
            let s = std::str::from_utf8(pair).ok()?;
            out[i] = u8::from_str_radix(s, 16).ok()?;
        }
        Some(WriterId(out))
    }
}

impl std::fmt::Debug for WriterId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WriterId({})", super::hex(&self.0))
    }
}

/// One writer's record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateRecord {
    pub writer: WriterId,
    /// Grows by at least one with every record the writer publishes.
    pub counter: u64,
    /// When it was published (the writer's clock): orders records of different writers, for the
    /// digest check only.
    pub time_ms: i64,
    /// [`set_digest`] of the vault's live files as the writer saw them after its write.
    pub digest: [u8; DIGEST_BYTES],
}

type HmacSha256 = Hmac<Sha256>;

fn mac(k_state: &[u8; 32]) -> HmacSha256 {
    match HmacSha256::new_from_slice(k_state) {
        Ok(m) => m,
        Err(_) => unreachable!("HMAC takes keys of any length"),
    }
}

impl StateRecord {
    fn body(&self) -> [u8; RECORD_BYTES - TAG_BYTES] {
        let mut b = [0u8; RECORD_BYTES - TAG_BYTES];
        b[..4].copy_from_slice(&MAGIC);
        b[4..12].copy_from_slice(&self.writer.0);
        b[12..20].copy_from_slice(&self.counter.to_be_bytes());
        b[20..28].copy_from_slice(&self.time_ms.to_be_bytes());
        b[28..60].copy_from_slice(&self.digest);
        b
    }

    fn tagger(k_state: &[u8; 32], body: &[u8]) -> HmacSha256 {
        let mut m = mac(k_state);
        m.update(TAG_LABEL.as_bytes());
        // Everything after the magic: writer, counter, time, digest.
        m.update(&body[MAGIC.len()..]);
        m
    }

    /// The object's content (deterministic: no nonce, nothing secret in it).
    pub fn seal(&self, k_state: &[u8; 32]) -> Vec<u8> {
        let body = self.body();
        let tag = Self::tagger(k_state, &body).finalize().into_bytes();
        let mut out = Vec::with_capacity(RECORD_BYTES);
        out.extend_from_slice(&body);
        out.extend_from_slice(&tag);
        out
    }

    /// Verifies a record object named `name`: exact size, magic, a tag made with this vault's key
    /// (compared in constant time) and the writer in the name equal to the one inside, so a record
    /// cannot be copied over another writer's.
    pub fn open(k_state: &[u8; 32], name: &str, bytes: &[u8]) -> Result<StateRecord> {
        let bad = || integrity(format!("vault state record {name} does not verify"));
        let named = WriterId::from_object_name(name).ok_or_else(bad)?;
        if bytes.len() != RECORD_BYTES || bytes[..4] != MAGIC {
            return Err(bad());
        }
        let (body, tag) = bytes.split_at(RECORD_BYTES - TAG_BYTES);
        Self::tagger(k_state, body).verify_slice(tag).map_err(|_| bad())?;
        let mut writer = [0u8; WRITER_BYTES];
        writer.copy_from_slice(&body[4..12]);
        let mut counter = [0u8; 8];
        counter.copy_from_slice(&body[12..20]);
        let mut time = [0u8; 8];
        time.copy_from_slice(&body[20..28]);
        let mut digest = [0u8; DIGEST_BYTES];
        digest.copy_from_slice(&body[28..60]);
        let record = StateRecord {
            writer: WriterId(writer),
            counter: u64::from_be_bytes(counter),
            time_ms: i64::from_be_bytes(time),
            digest,
        };
        if record.writer != named {
            return Err(bad());
        }
        Ok(record)
    }
}

/// The digest of a vault's live files: `(placement bytes, wasl.m blob)` per file, in any order
/// (they are sorted here; an empty blob for a file listed without one).
pub fn set_digest<'a>(files: impl IntoIterator<Item = (&'a [u8], &'a str)>) -> [u8; DIGEST_BYTES] {
    let mut list: Vec<(&[u8], &str)> = files.into_iter().collect();
    list.sort_unstable();
    let mut h = Sha256::new();
    h.update(SET_LABEL.as_bytes());
    h.update((list.len() as u64).to_be_bytes());
    for (placement, blob) in list {
        // Length prefixes keep the encoding unambiguous.
        h.update((placement.len() as u32).to_be_bytes());
        h.update(placement);
        h.update((blob.len() as u32).to_be_bytes());
        h.update(blob.as_bytes());
    }
    h.finalize().into()
}
