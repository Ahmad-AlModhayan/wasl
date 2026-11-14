//! Writing a file from local and fetched chunks, checking every one.

use crate::{ChunkList, ChunkLocation, FetchPlan, Hash, Step, to_hex};
use std::io::{self, Read, Write};

/// Why a file could not be assembled. Nothing written so far should be installed; on a
/// [`AssembleError::Chunk`] or [`AssembleError::Fetch`] the chunks before `index` are good and
/// [`Assembly::resume`] picks up from there.
#[derive(Debug, thiserror::Error)]
pub enum AssembleError {
    /// A fetched chunk does not match its name (or has the wrong length).
    #[error("chunk {index} at offset {offset} does not match its hash {expected}")]
    Chunk {
        index: usize,
        offset: u64,
        expected: String,
    },
    /// Every chunk verified but the file as a whole does not: the chunk list itself is wrong.
    #[error("file hash is {got}, expected {expected}")]
    File { expected: String, got: String },
    /// Getting a chunk from the peer failed.
    #[error("fetching chunk {index} ({hash}) failed: {source}")]
    Fetch {
        index: usize,
        hash: String,
        source: io::Error,
    },
    /// Writing the output failed.
    #[error("writing the file failed: {0}")]
    Write(io::Error),
    /// The plan is for a different file than the one being resumed.
    #[error("the fetch plan is for a different file")]
    WrongPlan,
}

/// What an assembly did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AssembleStats {
    /// Chunks copied from local files.
    pub local: usize,
    /// Chunks fetched, including those planned as local whose copy no longer verified.
    pub fetched: usize,
    /// Chunks planned as local that were fetched instead.
    pub stale: usize,
    pub bytes_fetched: u64,
}

/// A file being written chunk by chunk. Starts empty ([`Assembly::new`]) or after the verified
/// part of an interrupted attempt ([`Assembly::resume`]); [`Assembly::run`] writes the rest.
#[derive(Debug, Clone)]
pub struct Assembly {
    target: ChunkList,
    next: usize,
    written: u64,
    hasher: blake3::Hasher,
}

impl Assembly {
    /// Nothing written yet.
    pub fn new(target: &ChunkList) -> Assembly {
        Assembly {
            target: target.clone(),
            next: 0,
            written: 0,
            hasher: blake3::Hasher::new(),
        }
    }

    /// Reads `partial` (the temp file of an earlier attempt, from its start) and keeps the chunks
    /// at its head that match `target`, stopping at the first that is short or wrong. The caller
    /// then cuts the temp file to [`Assembly::verified_bytes`] and appends from there.
    pub fn resume(target: &ChunkList, mut partial: impl Read) -> io::Result<Assembly> {
        let mut at = Assembly::new(target);
        let mut buf = Vec::new();
        for c in &target.chunks {
            buf.clear();
            let got = (&mut partial).take(u64::from(c.len)).read_to_end(&mut buf)?;
            if got != c.len as usize || blake3::hash(&buf).as_bytes() != &c.hash {
                break;
            }
            at.hasher.update(&buf);
            at.written += u64::from(c.len);
            at.next += 1;
        }
        Ok(at)
    }

    /// How many chunks at the head of the file are written and verified.
    pub fn verified_chunks(&self) -> usize {
        self.next
    }

    /// How many bytes at the head of the file are written and verified.
    pub fn verified_bytes(&self) -> u64 {
        self.written
    }

    /// Writes the remaining chunks of `plan`'s target to `out`, in order. A chunk planned as local
    /// is read with `local`; if that fails or the bytes do not match, it is fetched instead. A
    /// fetched chunk comes from `fetched` (called once per chunk written, repeats included) and
    /// must match its hash. After the last chunk the whole-file BLAKE3 must match too.
    pub fn run(
        mut self,
        plan: &FetchPlan,
        mut local: impl FnMut(&ChunkLocation) -> io::Result<Vec<u8>>,
        mut fetched: impl FnMut(&Hash) -> io::Result<Vec<u8>>,
        out: &mut impl Write,
    ) -> Result<AssembleStats, AssembleError> {
        if plan.target() != &self.target {
            return Err(AssembleError::WrongPlan);
        }
        let mut stats = AssembleStats::default();
        for (index, (c, step)) in plan.steps().enumerate().skip(self.next) {
            let ok = |data: &[u8]| data.len() == c.len as usize && blake3::hash(data) == c.hash;
            let from_here = match step {
                Step::Local(at) => local(at).ok().filter(|d| ok(d)),
                Step::Fetch => None,
            };
            let data = match from_here {
                Some(d) => {
                    stats.local += 1;
                    d
                }
                None => {
                    if matches!(step, Step::Local(_)) {
                        stats.stale += 1;
                    }
                    let d = fetched(&c.hash).map_err(|source| AssembleError::Fetch {
                        index,
                        hash: to_hex(&c.hash),
                        source,
                    })?;
                    if !ok(&d) {
                        return Err(AssembleError::Chunk {
                            index,
                            offset: c.offset,
                            expected: to_hex(&c.hash),
                        });
                    }
                    stats.fetched += 1;
                    stats.bytes_fetched += d.len() as u64;
                    d
                }
            };
            out.write_all(&data).map_err(AssembleError::Write)?;
            self.hasher.update(&data);
            self.written += data.len() as u64;
            self.next = index + 1;
        }
        out.flush().map_err(AssembleError::Write)?;
        let got = *self.hasher.finalize().as_bytes();
        if got != self.target.blake3 || self.written != self.target.size {
            return Err(AssembleError::File {
                expected: to_hex(&self.target.blake3),
                got: to_hex(&got),
            });
        }
        Ok(stats)
    }
}

/// Writes `plan`'s target to `out` from the start. See [`Assembly::run`].
pub fn assemble(
    plan: &FetchPlan,
    local: impl FnMut(&ChunkLocation) -> io::Result<Vec<u8>>,
    fetched: impl FnMut(&Hash) -> io::Result<Vec<u8>>,
    out: &mut impl Write,
) -> Result<AssembleStats, AssembleError> {
    Assembly::new(plan.target()).run(plan, local, fetched, out)
}
