//! Cutting a file into chunks.
//!
//! FastCDC (Xia et al., 2020 variant) with normalization level 1, through the `fastcdc` crate's
//! cut function and our own buffering, so a file of any size is read once with at most
//! `max + READ_AHEAD` bytes in memory. The cut points equal `fastcdc::v2020::FastCDC::new` run over
//! the whole file (a test checks this).

use crate::{Chunk, ChunkList, ChunkParams};
use fastcdc::v2020;
use std::io::{self, Read};

/// How far past one maximum-size chunk the buffer reads, so it is not refilled after every chunk.
const READ_AHEAD: usize = 4 << 20;

/// The first read, before the full window is allocated.
const SMALL: usize = 64 << 10;

/// Reads everything `r` yields and returns its chunk list. A file of at most `params.max()` bytes
/// is one chunk (none when empty); a larger one is cut with FastCDC.
pub fn chunk_reader(mut r: impl Read, params: &ChunkParams) -> io::Result<ChunkList> {
    let min = params.min() as usize;
    let avg = params.avg() as usize;
    let max = params.max() as usize;
    let cap = max + READ_AHEAD;

    // Most files are small: read a little first and take the whole window only when needed.
    let mut buf = Vec::new();
    let mut eof = fill(&mut r, &mut buf, SMALL.min(cap))?;
    if !eof {
        buf.reserve_exact(cap - buf.len());
        eof = fill(&mut r, &mut buf, cap)?;
    }
    if eof && buf.len() <= max {
        let hash = *blake3::hash(&buf).as_bytes();
        let chunks = if buf.is_empty() {
            Vec::new()
        } else {
            vec![Chunk {
                offset: 0,
                len: buf.len() as u32,
                hash,
            }]
        };
        return Ok(ChunkList {
            size: buf.len() as u64,
            blake3: hash,
            chunks,
        });
    }

    let (mask_s, mask_l) = masks(params);
    let mut file = blake3::Hasher::new();
    let mut chunks = Vec::new();
    let mut offset = 0u64;
    let mut start = 0usize;
    loop {
        // A cut needs a full maximum-size window unless the input has ended.
        if !eof && buf.len() - start < max {
            buf.copy_within(start.., 0);
            buf.truncate(buf.len() - start);
            start = 0;
            eof = fill(&mut r, &mut buf, cap)?;
        }
        let avail = &buf[start..];
        if avail.is_empty() {
            break;
        }
        let (_, len) = v2020::cut(avail, min, avg, max, mask_s, mask_l, mask_s << 1, mask_l << 1);
        let data = &avail[..len];
        file.update(data);
        chunks.push(Chunk {
            offset,
            len: len as u32,
            hash: *blake3::hash(data).as_bytes(),
        });
        offset += len as u64;
        start += len;
    }
    Ok(ChunkList {
        size: offset,
        blake3: *file.finalize().as_bytes(),
        chunks,
    })
}

/// The strict and relaxed masks for level-1 normalization, as `v2020::FastCDC::new` picks them.
fn masks(params: &ChunkParams) -> (u64, u64) {
    let bits = v2020::logarithm2(params.avg()) as usize;
    (v2020::MASKS[bits + 1], v2020::MASKS[bits - 1])
}

/// Reads into `buf` until it holds `cap` bytes or the input ends. Returns whether it ended.
fn fill(r: &mut impl Read, buf: &mut Vec<u8>, cap: usize) -> io::Result<bool> {
    let want = cap - buf.len();
    let got = (&mut *r).take(want as u64).read_to_end(buf)?;
    Ok(got < want)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(len: usize, mut seed: u64) -> Vec<u8> {
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            seed = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = seed;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^= z >> 31;
            out.extend_from_slice(&z.to_le_bytes());
        }
        out.truncate(len);
        out
    }

    #[test]
    fn cuts_match_fastcdc_over_the_whole_buffer() {
        let p = ChunkParams::new(1024, 4096, 16384).unwrap();
        // Long enough that the buffer is refilled and compacted many times.
        let data = noise(READ_AHEAD * 3 + 12345, 7);
        let ours = chunk_reader(&data[..], &p).unwrap();
        let theirs: Vec<(u64, u32)> = v2020::FastCDC::new(&data, 1024, 4096, 16384)
            .map(|c| (c.offset as u64, c.length as u32))
            .collect();
        let ours: Vec<(u64, u32)> = ours.chunks.iter().map(|c| (c.offset, c.len)).collect();
        assert_eq!(ours, theirs);
    }

    #[test]
    fn masks_match_fastcdc() {
        let p = ChunkParams::DEFAULT;
        let (s, l) = masks(&p);
        // 4 MiB average, level 1: the 8 MiB and 2 MiB entries.
        assert_eq!(s, v2020::MASKS[23]);
        assert_eq!(l, v2020::MASKS[21]);
        // The relaxed mask's bits are a subset of the strict one's, which the bound in the crate
        // docs leans on: a strict candidate is a candidate either side of the average.
        assert_eq!(l & !s, 0);
    }
}
