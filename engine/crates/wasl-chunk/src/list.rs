//! A file version's chunk list.

use crate::Hash;
use serde::{Deserialize, Serialize};

/// One piece of a file: `len` bytes at `offset`, named by their BLAKE3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Chunk {
    pub offset: u64,
    pub len: u32,
    #[serde(with = "crate::hex")]
    pub hash: Hash,
}

impl Chunk {
    /// The offset just past the chunk.
    pub fn end(&self) -> u64 {
        self.offset + u64::from(self.len)
    }
}

/// What a file version is made of. The chunks cover the file in order, without gaps or overlap;
/// an empty file has none. Built by [`chunk_reader`](crate::chunk_reader), carried with the file's
/// index entry, and checked with [`ChunkList::check`] whenever it comes from elsewhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkList {
    pub size: u64,
    /// BLAKE3 of the whole file.
    #[serde(with = "crate::hex")]
    pub blake3: Hash,
    pub chunks: Vec<Chunk>,
}

/// Longest chunk any list may hold: FastCDC's maximum, the most the chunker ever cuts (and what
/// a peer may send for one chunk).
pub const MAX_CHUNK_LEN: u32 = 16 << 20;

/// Why a [`ChunkList`] does not describe a file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ListError {
    #[error("chunk {index} is empty")]
    EmptyChunk { index: usize },
    #[error("chunk {index} is {len} bytes, more than the chunker ever cuts")]
    LongChunk { index: usize, len: u32 },
    #[error("chunk {index} starts at {offset}, expected {expected}")]
    Gap {
        index: usize,
        offset: u64,
        expected: u64,
    },
    #[error("chunks cover {covered} bytes of a {size}-byte file")]
    Size { covered: u64, size: u64 },
}

impl ChunkList {
    /// Checks that the chunks are non-empty, at most [`MAX_CHUNK_LEN`] each, and cover exactly
    /// `size` bytes in order. Says nothing about the hashes; those are checked against the bytes
    /// on assembly.
    pub fn check(&self) -> Result<(), ListError> {
        let mut at = 0u64;
        for (index, c) in self.chunks.iter().enumerate() {
            if c.len == 0 {
                return Err(ListError::EmptyChunk { index });
            }
            // Security review 2026-10: a list from a peer naming a 4 GiB chunk would have this
            // device read or buffer that much for one chunk.
            if c.len > MAX_CHUNK_LEN {
                return Err(ListError::LongChunk { index, len: c.len });
            }
            if c.offset != at {
                return Err(ListError::Gap {
                    index,
                    offset: c.offset,
                    expected: at,
                });
            }
            at = c.offset.checked_add(u64::from(c.len)).ok_or(ListError::Size {
                covered: u64::MAX,
                size: self.size,
            })?;
        }
        if at != self.size {
            return Err(ListError::Size {
                covered: at,
                size: self.size,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(lens: &[u32]) -> ChunkList {
        let mut offset = 0;
        let chunks = lens
            .iter()
            .map(|&len| {
                let c = Chunk {
                    offset,
                    len,
                    hash: [0; 32],
                };
                offset += u64::from(len);
                c
            })
            .collect();
        ChunkList {
            size: offset,
            blake3: [0; 32],
            chunks,
        }
    }

    #[test]
    fn a_chunk_longer_than_the_chunker_cuts_is_refused() {
        assert!(list(&[MAX_CHUNK_LEN, 1]).check().is_ok());
        assert_eq!(
            list(&[1, MAX_CHUNK_LEN + 1]).check(),
            Err(ListError::LongChunk {
                index: 1,
                len: MAX_CHUNK_LEN + 1
            })
        );
        assert!(list(&[u32::MAX]).check().is_err());
    }
}
