//! Which revision of the v2 format a vault is written in (ENCRYPTION.md §11.10).
//!
//! * **2.0** (`vault.json` `"version": 2`): chunks cut with FastCDC's public gear table and sealed
//!   at their exact length (`W2C1`). Someone who sees the Drive folder sees every chunk's size, and
//!   because anyone can compute where FastCDC cuts a known file, the sequence of sizes can confirm
//!   that a vault holds a file the observer also has (security review S4-1).
//! * **2.1** (`vault.json` `"version": 3`): the gear table is derived from the vault's keys, so cut
//!   points are unpredictable without them, and every chunk is padded before sealing (`W2C2`,
//!   padmé buckets, at least 4 KiB), so sizes reveal only a few bits. Keys, names, folder ids and
//!   manifests are exactly 2.0's; that is why it is "2.1".
//!
//! The JSON `version` is a new integer rather than a new key next to `2` because every reader
//! released before 2.1 refuses any `version` other than 2 ("vault format version 3 is not supported
//! here"), while it ignores unknown keys: a flag would let an old device keep writing unkeyed,
//! unpadded chunks into a 2.1 vault without noticing.
//!
//! Readers of either revision open both chunk framings (the magic says which); writers use the
//! vault's own revision, so a 2.0 vault stays readable by devices that predate 2.1.

/// A v2 format revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Format {
    /// Public chunking, exact chunk sizes (`W2C1`).
    V2_0,
    /// Keyed chunking, padded chunks (`W2C2`).
    V2_1,
}

impl Format {
    /// What new vaults are made in.
    pub const LATEST: Format = Format::V2_1;

    /// The `version` written in `vault.json`.
    pub const fn json_version(self) -> i64 {
        match self {
            Format::V2_0 => 2,
            Format::V2_1 => 3,
        }
    }

    /// The revision a `vault.json` `version` names, `None` for one this build does not know.
    pub const fn from_json_version(v: i64) -> Option<Format> {
        match v {
            2 => Some(Format::V2_0),
            3 => Some(Format::V2_1),
            _ => None,
        }
    }

    /// "2.0" or "2.1", for people.
    pub const fn label(self) -> &'static str {
        match self {
            Format::V2_0 => "2.0",
            Format::V2_1 => "2.1",
        }
    }

    /// Whether chunk boundaries come from the vault's keys.
    pub const fn keyed_chunking(self) -> bool {
        matches!(self, Format::V2_1)
    }

    /// Whether chunks are padded before sealing.
    pub const fn padded_chunks(self) -> bool {
        matches!(self, Format::V2_1)
    }
}
