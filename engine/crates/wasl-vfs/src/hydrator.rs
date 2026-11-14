//! Serving byte ranges: the cache first, the source for the rest (contiguous missing blocks in one
//! request), and read-ahead when a reader goes through a file in order.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::cache::{BLOCK, BlockCache, BlockKey};
use crate::{FileRef, Result, Source, VfsError};

/// How far ahead a sequential reader is served.
#[derive(Debug, Clone, Copy)]
pub struct ReadAhead {
    /// Blocks fetched beyond a read once the reader has read this many blocks in order.
    pub blocks: u64,
    pub after: u64,
}

impl Default for ReadAhead {
    fn default() -> Self {
        Self { blocks: 4, after: 2 }
    }
}

/// Byte ranges of on-demand files, through the cache.
pub struct Hydrator {
    source: Arc<dyn Source>,
    cache: Arc<BlockCache>,
    ahead: ReadAhead,
    /// Per file: where the last read ended, and how many blocks were read in order.
    streams: Mutex<HashMap<String, (u64, u64)>>,
}

impl Hydrator {
    pub fn new(source: Arc<dyn Source>, cache: Arc<BlockCache>, ahead: ReadAhead) -> Self {
        Self {
            source,
            cache,
            ahead,
            streams: Mutex::new(HashMap::new()),
        }
    }

    pub fn cache(&self) -> &BlockCache {
        &self.cache
    }

    /// Bytes `offset..offset+len` of `f` (fewer at the end of the file; none past it).
    pub async fn read(&self, f: &FileRef, offset: u64, len: u64) -> Result<Vec<u8>> {
        if offset > f.size {
            return Err(VfsError::OutOfRange);
        }
        let end = (offset + len).min(f.size);
        if end == offset {
            return Ok(Vec::new());
        }
        self.cache.refresh_pin(f);
        let first = offset / BLOCK;
        let last = (end - 1) / BLOCK;
        let ahead = self.ahead_for(f, offset, end, last);
        let blocks = self.blocks(f, first, last.max(ahead).min(last_block(f))).await?;
        let mut out = Vec::with_capacity((end - offset) as usize);
        for (i, b) in blocks.iter().enumerate().take((last - first + 1) as usize) {
            let start_in = if i == 0 { (offset % BLOCK) as usize } else { 0 };
            let block_start = (first + i as u64) * BLOCK;
            let stop = ((end - block_start).min(b.len() as u64)) as usize;
            if start_in > stop {
                return Err(VfsError::Integrity(
                    "a block is shorter than the file says".into(),
                ));
            }
            out.extend_from_slice(&b[start_in..stop]);
        }
        if out.len() as u64 != end - offset {
            return Err(VfsError::Integrity(
                "the source returned fewer bytes than the file has".into(),
            ));
        }
        Ok(out)
    }

    /// Fetches every block of the file and pins it: it stays complete on the device (FR-O5).
    pub async fn pin(&self, f: &FileRef) -> Result<()> {
        self.cache.set_pinned(f, true)?;
        if f.size > 0 {
            self.blocks(f, 0, last_block(f)).await?;
        }
        Ok(())
    }

    /// Lets the file's blocks be evicted again.
    pub fn unpin(&self, f: &FileRef) -> Result<()> {
        Ok(self.cache.set_pinned(f, false)?)
    }

    /// Whether every block of the file is in the cache.
    pub fn complete(&self, f: &FileRef) -> bool {
        f.size == 0 || (0..=last_block(f)).all(|i| self.cache.has(&BlockKey::new(f, i)))
    }

    /// The last block to fetch for this read: further when the reader is going in order.
    fn ahead_for(&self, f: &FileRef, offset: u64, end: u64, last: u64) -> u64 {
        let mut streams = self.streams.lock().unwrap_or_else(|p| p.into_inner());
        let key = format!("{}\0{}", f.id, f.version);
        let run = match streams.get(&key) {
            Some((prev_end, run)) if *prev_end == offset => run + 1,
            _ => 0,
        };
        streams.insert(key, (end, run));
        if run >= self.ahead.after {
            last + self.ahead.blocks
        } else {
            last
        }
    }

    /// Blocks `first..=last` of `f`, from the cache or fetched (contiguous gaps in one request).
    async fn blocks(&self, f: &FileRef, first: u64, last: u64) -> Result<Vec<Vec<u8>>> {
        let mut out: Vec<Option<Vec<u8>>> = (first..=last)
            .map(|i| self.cache.get(&BlockKey::new(f, i)))
            .collect();
        let mut i = 0usize;
        while i < out.len() {
            if out[i].is_some() {
                i += 1;
                continue;
            }
            let mut j = i;
            while j + 1 < out.len() && out[j + 1].is_none() {
                j += 1;
            }
            let from = (first + i as u64) * BLOCK;
            let to = ((first + j as u64 + 1) * BLOCK).min(f.size);
            let (nat_from, nat_len) = self.source.natural_range(f, from, to - from);
            let nat_from = nat_from.min(from);
            let nat_to = (nat_from + nat_len).max(to).min(f.size);
            // Fetch whole blocks around the natural range, so every block stored is complete.
            let fetch_from = nat_from / BLOCK * BLOCK;
            let fetch_to = nat_to.div_ceil(BLOCK).saturating_mul(BLOCK).min(f.size);
            let bytes = self.source.read(f, fetch_from, fetch_to - fetch_from).await?;
            if bytes.len() as u64 != fetch_to - fetch_from {
                return Err(VfsError::Integrity(format!(
                    "asked for {} bytes, got {}",
                    fetch_to - fetch_from,
                    bytes.len()
                )));
            }
            for (n, piece) in bytes.chunks(BLOCK as usize).enumerate() {
                let index = fetch_from / BLOCK + n as u64;
                self.cache.put(BlockKey::new(f, index), piece)?;
                if index >= first && index <= last {
                    out[(index - first) as usize] = Some(piece.to_vec());
                }
            }
            i = j + 1;
        }
        Ok(out.into_iter().map(|b| b.expect("filled")).collect())
    }
}

fn last_block(f: &FileRef) -> u64 {
    f.size.saturating_sub(1) / BLOCK
}
