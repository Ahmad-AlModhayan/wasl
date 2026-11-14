//! Where chunks the receiver already has can be read from.

use crate::{ChunkList, Hash};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// `len` bytes at `offset` of the local file `file`. What `file` names is up to the caller (a
/// pair-relative path, a row id); this crate only hands it back.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChunkLocation {
    pub file: String,
    pub offset: u64,
    pub len: u32,
}

/// Finds a chunk by hash among the files the receiver already has. The engine backs this with
/// the pair's chunk table in SQLite; [`ChunkIndex`] is the in-memory one.
///
/// A location may be stale (the file changed since it was indexed); [`assemble`](crate::assemble)
/// checks every chunk it reads, so a stale answer costs a fetch, never a wrong file. A back end
/// that fails to look up should answer `None` for the same reason.
pub trait ChunkSource {
    fn locate(&self, hash: &Hash) -> Option<ChunkLocation>;
}

/// Chunk hash → the places it occurs, built from chunk lists.
#[derive(Debug, Clone, Default)]
pub struct ChunkIndex {
    by_hash: HashMap<Hash, Vec<ChunkLocation>>,
}

impl ChunkIndex {
    pub fn new() -> ChunkIndex {
        ChunkIndex::default()
    }

    /// An index over `(file, list)` pairs.
    pub fn from_lists<'a, K: Into<String>>(
        lists: impl IntoIterator<Item = (K, &'a ChunkList)>,
    ) -> ChunkIndex {
        let mut index = ChunkIndex::new();
        for (file, list) in lists {
            index.add(file, list);
        }
        index
    }

    /// Records every chunk of `list` as readable from `file`. Locations already known are kept
    /// first, so the earliest file added is the one read from.
    pub fn add(&mut self, file: impl Into<String>, list: &ChunkList) {
        let file = file.into();
        for c in &list.chunks {
            let at = ChunkLocation {
                file: file.clone(),
                offset: c.offset,
                len: c.len,
            };
            let places = self.by_hash.entry(c.hash).or_default();
            if !places.contains(&at) {
                places.push(at);
            }
        }
    }

    /// Forgets every location in `file` (it was deleted or is about to change). Chunks that also
    /// occur elsewhere stay findable there.
    pub fn remove_file(&mut self, file: &str) {
        self.by_hash.retain(|_, places| {
            places.retain(|p| p.file != file);
            !places.is_empty()
        });
    }

    /// Every known location of `hash`, earliest added first.
    pub fn locations(&self, hash: &Hash) -> &[ChunkLocation] {
        self.by_hash.get(hash).map_or(&[], Vec::as_slice)
    }

    /// The number of distinct chunks known.
    pub fn len(&self) -> usize {
        self.by_hash.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_hash.is_empty()
    }
}

impl ChunkSource for ChunkIndex {
    fn locate(&self, hash: &Hash) -> Option<ChunkLocation> {
        self.locations(hash).first().cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Chunk;

    fn list(hashes: &[u8]) -> ChunkList {
        let chunks = hashes
            .iter()
            .enumerate()
            .map(|(i, &h)| Chunk {
                offset: i as u64 * 10,
                len: 10,
                hash: [h; 32],
            })
            .collect::<Vec<_>>();
        ChunkList {
            size: chunks.len() as u64 * 10,
            blake3: [0; 32],
            chunks,
        }
    }

    #[test]
    fn earliest_file_wins_and_removal_falls_back() {
        let a = list(&[1, 2]);
        let b = list(&[3, 2]);
        let mut index = ChunkIndex::from_lists([("a", &a), ("b", &b)]);
        assert_eq!(index.len(), 3);
        let two = index.locate(&[2; 32]).unwrap();
        assert_eq!((two.file.as_str(), two.offset), ("a", 10));
        index.remove_file("a");
        let two = index.locate(&[2; 32]).unwrap();
        assert_eq!((two.file.as_str(), two.offset), ("b", 10));
        assert!(index.locate(&[1; 32]).is_none());
        index.add("b", &b);
        assert_eq!(index.locations(&[2; 32]).len(), 1);
    }
}
