//! Chunking speed. Run in release:
//! `cargo test -p wasl-chunk --release --test throughput -- --ignored --nocapture`

mod common;

use common::Synth;
use std::io::{self, Read};
use std::time::Instant;
use wasl_chunk::{ChunkParams, chunk_reader};

/// `total` bytes cycling over one in-memory block, so the source costs next to nothing.
struct Cycle {
    block: Vec<u8>,
    at: usize,
    left: u64,
}

impl Read for Cycle {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = buf
            .len()
            .min(self.block.len() - self.at)
            .min(self.left.min(usize::MAX as u64) as usize);
        buf[..n].copy_from_slice(&self.block[self.at..self.at + n]);
        self.at = (self.at + n) % self.block.len();
        self.left -= n as u64;
        Ok(n)
    }
}

#[test]
#[ignore = "slow; run in release"]
fn chunking_throughput_1gib() {
    const GIB: u64 = 1 << 30;
    // An odd block length so repeats do not line up with chunk boundaries.
    let block = Synth::plain(50, 64 * (1 << 20) + 4099).bytes();

    let start = Instant::now();
    let mut hashed = blake3::Hasher::new();
    let mut src = Cycle {
        block: block.clone(),
        at: 0,
        left: GIB,
    };
    io::copy(&mut src, &mut hashed).unwrap();
    let blake3_only = start.elapsed();

    let start = Instant::now();
    let src = Cycle {
        block,
        at: 0,
        left: GIB,
    };
    let list = chunk_reader(src, &ChunkParams::DEFAULT).unwrap();
    let took = start.elapsed();
    assert_eq!(list.size, GIB);

    let mbs = |d: std::time::Duration| GIB as f64 / 1e6 / d.as_secs_f64();
    eprintln!(
        "1 GiB: {} chunks, chunk list {:.0} MB/s ({:.2} s); one BLAKE3 pass alone {:.0} MB/s",
        list.chunks.len(),
        mbs(took),
        took.as_secs_f64(),
        mbs(blake3_only)
    );
}
