//! Serde for hashes as lowercase hex strings.

use crate::Hash;
use serde::{Deserialize, Deserializer, Serializer, de::Error};

pub fn serialize<S: Serializer>(hash: &Hash, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&crate::to_hex(hash))
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Hash, D::Error> {
    let text = <std::borrow::Cow<'de, str>>::deserialize(d)?;
    parse(&text).ok_or_else(|| D::Error::custom("expected 64 hex digits"))
}

/// Parses 64 hex digits (either case).
pub fn parse(text: &str) -> Option<Hash> {
    if text.len() != 64 {
        return None;
    }
    blake3::Hash::from_hex(text).ok().map(|h| *h.as_bytes())
}
