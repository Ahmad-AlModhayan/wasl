//! Chunked transfer between devices (docs/DEVICE_SYNC.md §5, SRS FR-N9, FR-N10).
//!
//! A file version is described by its [`ChunkList`]: the size, the whole-file BLAKE3 and a list of
//! chunks, each named by its own BLAKE3. Files above the maximum chunk size are cut with FastCDC
//! (normalized chunking, [`ChunkParams`]); smaller files are one chunk. The sizes depend on the
//! file's size ([`ChunkParams::for_size`]): 64 KiB average chunks up to 16 MiB, 4 MiB above. The list travels with the
//! file's index entry, as JSON ([`serde`]) or in the compact [wire form](ChunkList::to_bytes).
//!
//! A receiver turns the list into a [`FetchPlan`] with [`plan_fetch`]: every chunk it already has,
//! in any version of any file of the pair (found through a [`ChunkSource`] such as
//! [`ChunkIndex`]), is copied locally; only the rest is asked from the peer. [`assemble`] then
//! writes the file in order, checking every chunk and the whole file. A transfer cut short resumes
//! at chunk granularity through [`Assembly::resume`].
//!
//! # What an edit costs
//!
//! Whether a chunk boundary falls at a position depends only on the 64 bytes before it and on its
//! distance from the start of the current chunk. So an edit to bytes `[e, f)` cannot change any
//! chunk that ends before `e`, and once both versions cut at the same position after `f + 64`
//! every later chunk is the same again. Exactly one chunk changes (the one holding the edit) unless
//! a cut candidate falls among the `f - e + 64` positions whose rolling hash the edit changed. With
//! the default parameters a position is a candidate with probability at most 2^-21, so for an
//! overwrite of 256 KiB that is at most (2^18 + 64) / 2^21 ≈ 12.5 %. When it happens the stray cut
//! starts a chunk the old version never had, and the versions usually meet again at the next
//! strong candidate (one under the strict mask, whose bits contain the relaxed mask's, so every
//! chunker that reaches it cuts there).
//!
//! (These figures are for [`ChunkParams::DEFAULT`]. With [`ChunkParams::MEDIUM`] the candidate
//! probability is 2^-15, so the same argument holds for edits 64 times smaller: an overwrite of a
//! few KiB costs one 64 KiB chunk with probability around 87 % or better.)
//!
//! That is the provable part: one changed chunk, at most `max` bytes, with probability at least
//! 87.5 % for a 256 KiB overwrite. Past it the bound is measured, not proved: over 5,000 seeded
//! edits at 1/256 scale (`edit_cost_distribution`), an overwrite cost one chunk 91 % of the time,
//! two 7 %, three 1.7 %, four or five 0.3 %, and an insertion one chunk 99.6 % of the time; no
//! edit cost more than 1.88 × `max` bytes. There is no worst-case bound: content built against the
//! rolling hash, or long runs without candidates (zeros, where cuts fall every `max` bytes), can
//! keep two versions out of step to the end of the file.

mod assemble;
mod cdc;
mod hex;
mod index;
mod list;
mod params;
mod plan;
mod wire;

pub use assemble::{AssembleError, AssembleStats, Assembly, assemble};
pub use cdc::chunk_reader;
pub use index::{ChunkIndex, ChunkLocation, ChunkSource};
pub use list::{Chunk, ChunkList, ListError, MAX_CHUNK_LEN};
pub use params::{ChunkParams, ParamsError, TIER_LIMIT};
pub use plan::{FetchPlan, Step, plan_fetch};
pub use wire::{WIRE_VERSION, WireError};

/// A BLAKE3 hash: of one chunk, or of a whole file.
pub type Hash = [u8; 32];

/// Lowercase hex of `hash`, as used in JSON and in error messages.
pub fn to_hex(hash: &Hash) -> String {
    blake3::Hash::from(*hash).to_hex().to_string()
}
