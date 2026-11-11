//! The compact binary form of a chunk list, for the device protocol.
//!
//! ```text
//! u8        version (1)
//! u64 LE    file size
//! [u8; 32]  file BLAKE3
//! u32 LE    chunk count n
//! n × { u32 LE length, [u8; 32] BLAKE3 }
//! ```
//!
//! Offsets are not sent: chunks are contiguous, so each starts where the previous one ended. The
//! encoding is exact; trailing bytes are an error. About 36 bytes per chunk, so a 1 GiB file costs
//! some 9 KiB.

use crate::{Chunk, ChunkList, Hash, ListError};

/// The format version [`ChunkList::to_bytes`] writes and [`ChunkList::from_bytes`] reads.
pub const WIRE_VERSION: u8 = 1;

const HEADER: usize = 1 + 8 + 32 + 4;
const ENTRY: usize = 4 + 32;

/// Why bytes are not a chunk list.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WireError {
    #[error("chunk list format version {0} is not supported")]
    Version(u8),
    #[error("chunk list is {got} bytes, expected {expected}")]
    Length { got: usize, expected: usize },
    #[error(transparent)]
    List(#[from] ListError),
}

impl ChunkList {
    /// The wire form (see the module docs).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER + ENTRY * self.chunks.len());
        out.push(WIRE_VERSION);
        out.extend_from_slice(&self.size.to_le_bytes());
        out.extend_from_slice(&self.blake3);
        out.extend_from_slice(&(self.chunks.len() as u32).to_le_bytes());
        for c in &self.chunks {
            out.extend_from_slice(&c.len.to_le_bytes());
            out.extend_from_slice(&c.hash);
        }
        out
    }

    /// Reads the wire form. Rejects an unknown version, a wrong length and a list that does not
    /// cover the stated size ([`ChunkList::check`]); never panics on any input.
    pub fn from_bytes(bytes: &[u8]) -> Result<ChunkList, WireError> {
        let Some((&version, rest)) = bytes.split_first() else {
            return Err(WireError::Length {
                got: 0,
                expected: HEADER,
            });
        };
        if version != WIRE_VERSION {
            return Err(WireError::Version(version));
        }
        if bytes.len() < HEADER {
            return Err(WireError::Length {
                got: bytes.len(),
                expected: HEADER,
            });
        }
        let (size, rest) = rest.split_at(8);
        let (blake3, rest) = rest.split_at(32);
        let (count, mut rest) = rest.split_at(4);
        let size = u64::from_le_bytes(size.try_into().expect("8 bytes"));
        let blake3: Hash = blake3.try_into().expect("32 bytes");
        let count = u32::from_le_bytes(count.try_into().expect("4 bytes")) as usize;
        // Check the length before allocating anything sized by `count`.
        let expected = (count as u64)
            .checked_mul(ENTRY as u64)
            .and_then(|n| n.checked_add(HEADER as u64));
        if expected != Some(bytes.len() as u64) {
            return Err(WireError::Length {
                got: bytes.len(),
                expected: expected.map_or(usize::MAX, |n| n.min(usize::MAX as u64) as usize),
            });
        }
        let mut chunks = Vec::with_capacity(count);
        let mut offset = 0u64;
        while !rest.is_empty() {
            let (entry, next) = rest.split_at(ENTRY);
            rest = next;
            let len = u32::from_le_bytes(entry[..4].try_into().expect("4 bytes"));
            let hash: Hash = entry[4..].try_into().expect("32 bytes");
            chunks.push(Chunk { offset, len, hash });
            // An overflow is caught by `check` below, which sees the wrapped offset as a gap.
            offset = offset.wrapping_add(u64::from(len));
        }
        let list = ChunkList { size, blake3, chunks };
        list.check()?;
        Ok(list)
    }
}
