//! Deciding which chunks to copy locally and which to ask the peer for.

use crate::{Chunk, ChunkList, ChunkLocation, ChunkSource, Hash};
use std::collections::HashSet;

/// Where one chunk of the target comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Already here: read it from this location.
    Local(ChunkLocation),
    /// Ask the peer for it.
    Fetch,
}

/// How to build a target file: one [`Step`] per chunk, in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchPlan {
    target: ChunkList,
    steps: Vec<Step>,
}

/// Plans `target` against what `have` can find locally.
pub fn plan_fetch(target: &ChunkList, have: &dyn ChunkSource) -> FetchPlan {
    let steps = target
        .chunks
        .iter()
        .map(|c| match have.locate(&c.hash) {
            Some(at) if at.len == c.len => Step::Local(at),
            _ => Step::Fetch,
        })
        .collect();
    FetchPlan {
        target: target.clone(),
        steps,
    }
}

impl FetchPlan {
    /// The file being built.
    pub fn target(&self) -> &ChunkList {
        &self.target
    }

    /// Each chunk of the target with where it comes from, in file order.
    pub fn steps(&self) -> impl Iterator<Item = (&Chunk, &Step)> {
        self.target.chunks.iter().zip(&self.steps)
    }

    /// The chunks to ask the peer for, each once, in the order they are first needed.
    pub fn to_fetch(&self) -> Vec<Chunk> {
        let mut seen = HashSet::new();
        self.steps()
            .filter(|(c, s)| matches!(s, Step::Fetch) && seen.insert(c.hash))
            .map(|(c, _)| *c)
            .collect()
    }

    /// Bytes to transfer: the size of every distinct chunk to fetch.
    pub fn bytes_to_fetch(&self) -> u64 {
        self.to_fetch().iter().map(|c| u64::from(c.len)).sum()
    }

    /// Bytes copied from local files.
    pub fn bytes_local(&self) -> u64 {
        self.steps()
            .filter(|(_, s)| matches!(s, Step::Local(_)))
            .map(|(c, _)| u64::from(c.len))
            .sum()
    }

    /// Whether `hash` is one of the chunks to fetch.
    pub fn needs(&self, hash: &Hash) -> bool {
        self.steps()
            .any(|(c, s)| matches!(s, Step::Fetch) && c.hash == *hash)
    }
}
