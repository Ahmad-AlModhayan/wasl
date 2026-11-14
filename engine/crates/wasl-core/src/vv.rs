//! Version vectors (SRS FR-C4, docs/DEVICE_SYNC.md §4): which participant changed a file how many
//! times. Comparing two vectors tells whether one version descends from the other or both changed
//! independently.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A participant's short id: the first 8 bytes of its device ID. `0` is Drive, for changes made
/// outside Wasl.
pub type ShortId = u64;

/// The participant that stands for "changed on Drive by something other than Wasl".
pub const DRIVE: ShortId = 0;

/// How two versions relate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Causality {
    Equal,
    /// This version descends from the other (it saw every change the other saw, and more).
    Dominates,
    /// The other descends from this one.
    DominatedBy,
    /// Both changed independently.
    Concurrent,
}

/// Counters per participant. Absent participants count as 0; a zero is never stored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VersionVector(BTreeMap<ShortId, u64>);

impl VersionVector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, id: ShortId) -> u64 {
        self.0.get(&id).copied().unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn entries(&self) -> impl Iterator<Item = (ShortId, u64)> + '_ {
        self.0.iter().map(|(k, v)| (*k, *v))
    }

    /// One more change by `id`.
    pub fn tick(&mut self, id: ShortId) {
        *self.0.entry(id).or_insert(0) += 1;
    }

    pub fn ticked(&self, id: ShortId) -> Self {
        let mut v = self.clone();
        v.tick(id);
        v
    }

    /// Element-wise maximum: a version that has seen everything both have.
    pub fn merge(&mut self, other: &VersionVector) {
        for (k, v) in &other.0 {
            let e = self.0.entry(*k).or_insert(0);
            if *v > *e {
                *e = *v;
            }
        }
    }

    pub fn merged(&self, other: &VersionVector) -> Self {
        let mut v = self.clone();
        v.merge(other);
        v
    }

    pub fn compare(&self, other: &VersionVector) -> Causality {
        let mut greater = false;
        let mut less = false;
        for k in self.0.keys().chain(other.0.keys()) {
            let (a, b) = (self.get(*k), other.get(*k));
            greater |= a > b;
            less |= a < b;
            if greater && less {
                return Causality::Concurrent;
            }
        }
        match (greater, less) {
            (false, false) => Causality::Equal,
            (true, false) => Causality::Dominates,
            (false, true) => Causality::DominatedBy,
            (true, true) => Causality::Concurrent,
        }
    }

    /// `self ≥ other` everywhere.
    pub fn descends_from(&self, other: &VersionVector) -> bool {
        matches!(self.compare(other), Causality::Equal | Causality::Dominates)
    }

    /// Compact bytes for Drive's `appProperties` (FR-C4): per entry, the participant as its
    /// position in `members` plus one (a varint), or `0` followed by its full id for a participant
    /// not in the list; then the counter (a varint).
    pub fn encode(&self, members: &[ShortId]) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.0.len() * 3);
        for (id, n) in &self.0 {
            match members.iter().position(|m| m == id) {
                Some(i) => put_varint(&mut out, i as u64 + 1),
                None => {
                    put_varint(&mut out, 0);
                    put_varint(&mut out, *id);
                }
            }
            put_varint(&mut out, *n);
        }
        out
    }

    /// The inverse of [`encode`](Self::encode). Any malformed input is `None`.
    pub fn decode(bytes: &[u8], members: &[ShortId]) -> Option<Self> {
        let mut v = BTreeMap::new();
        let mut rest = bytes;
        while !rest.is_empty() {
            let tag = take_varint(&mut rest)?;
            let id = if tag == 0 {
                take_varint(&mut rest)?
            } else {
                *members.get(usize::try_from(tag - 1).ok()?)?
            };
            let n = take_varint(&mut rest)?;
            if n == 0 || v.insert(id, n).is_some() {
                return None;
            }
        }
        Some(VersionVector(v))
    }
}

impl FromIterator<(ShortId, u64)> for VersionVector {
    fn from_iter<T: IntoIterator<Item = (ShortId, u64)>>(iter: T) -> Self {
        VersionVector(iter.into_iter().filter(|(_, n)| *n > 0).collect())
    }
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn take_varint(bytes: &mut &[u8]) -> Option<u64> {
    let mut v: u64 = 0;
    for shift in (0..64).step_by(7) {
        let (&b, rest) = bytes.split_first()?;
        *bytes = rest;
        // The tenth byte holds bit 63 only: anything above would be dropped by the shift.
        if shift == 63 && b > 1 {
            return None;
        }
        v |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vv(e: &[(u64, u64)]) -> VersionVector {
        e.iter().copied().collect()
    }

    /// #145 L6: the tenth byte of a u64 varint holds one bit; more is rejected, not dropped.
    #[test]
    fn a_varint_wider_than_64_bits_is_rejected() {
        let max = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01];
        assert_eq!(Some(u64::MAX), take_varint(&mut &max[..]));
        let wide = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f];
        assert_eq!(None, take_varint(&mut &wide[..]));
        let two = [0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x02];
        assert_eq!(None, take_varint(&mut &two[..]));
    }

    #[test]
    fn compare_covers_the_four_cases() {
        assert_eq!(Causality::Equal, vv(&[(1, 2)]).compare(&vv(&[(1, 2)])));
        assert_eq!(
            Causality::Dominates,
            vv(&[(1, 2), (2, 1)]).compare(&vv(&[(1, 2)]))
        );
        assert_eq!(Causality::DominatedBy, vv(&[(1, 1)]).compare(&vv(&[(1, 2)])));
        assert_eq!(Causality::Concurrent, vv(&[(1, 2)]).compare(&vv(&[(2, 1)])));
        assert_eq!(Causality::Equal, VersionVector::new().compare(&vv(&[(5, 0)])));
    }

    #[test]
    fn merge_dominates_both() {
        let a = vv(&[(1, 3), (2, 1)]);
        let b = vv(&[(2, 4), (7, 1)]);
        let m = a.merged(&b);
        assert!(m.descends_from(&a) && m.descends_from(&b));
        assert_eq!(vv(&[(1, 3), (2, 4), (7, 1)]), m);
    }

    #[test]
    fn encoding_round_trips_and_is_small() {
        let members = [0xAAAA_BBBB_CCCC_DDDD, 0x1111, 42];
        let v = vv(&[(0xAAAA_BBBB_CCCC_DDDD, 300), (42, 1), (9_999_999_999, 2)]);
        let bytes = v.encode(&members);
        assert_eq!(Some(v), VersionVector::decode(&bytes, &members));
        // Three members with small counters fit in a few bytes.
        let small = vv(&[(0xAAAA_BBBB_CCCC_DDDD, 3), (0x1111, 1), (42, 9)]).encode(&members);
        assert_eq!(6, small.len());
    }

    #[test]
    fn malformed_encodings_are_rejected() {
        let members = [1, 2];
        assert_eq!(None, VersionVector::decode(&[0x80], &members), "truncated varint");
        assert_eq!(
            None,
            VersionVector::decode(&[9, 1], &members),
            "unknown member index"
        );
        assert_eq!(None, VersionVector::decode(&[1, 0], &members), "zero counter");
        assert_eq!(
            None,
            VersionVector::decode(&[1, 1, 1, 2], &members),
            "duplicate participant"
        );
        assert_eq!(
            None,
            VersionVector::decode(&[0xff; 11], &members),
            "overlong varint"
        );
    }
}
