//! Seeded synthetic files, generated as they are read (nothing is kept on disk).

#![allow(dead_code)]

use std::io::{self, Read};

pub fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// `len` bytes of stream `seed` starting at its byte `from`.
#[derive(Debug, Clone, Copy)]
pub struct Seg {
    pub len: u64,
    pub seed: u64,
    pub from: u64,
}

/// A file made of pieces of pseudo-random streams. Byte `i` of stream `s` is a pure function of
/// `(s, i)`, so an edited version is just a different list of pieces.
#[derive(Debug, Clone)]
pub struct Synth {
    segs: Vec<Seg>,
    seg: usize,
    at: u64,
}

impl Synth {
    pub fn new(segs: Vec<Seg>) -> Synth {
        Synth { segs, seg: 0, at: 0 }
    }

    /// `len` bytes of stream `seed`.
    pub fn plain(seed: u64, len: u64) -> Synth {
        Synth::new(vec![Seg { len, seed, from: 0 }])
    }

    /// Stream `seed` of `len` bytes with `n` bytes at `at` replaced by stream `other`.
    pub fn overwrite(seed: u64, len: u64, at: u64, n: u64, other: u64) -> Synth {
        Synth::new(vec![
            Seg {
                len: at,
                seed,
                from: 0,
            },
            Seg {
                len: n,
                seed: other,
                from: 0,
            },
            Seg {
                len: len - at - n,
                seed,
                from: at + n,
            },
        ])
    }

    /// Stream `seed` of `len` bytes with `n` bytes of stream `other` inserted at `at`.
    pub fn insert(seed: u64, len: u64, at: u64, n: u64, other: u64) -> Synth {
        Synth::new(vec![
            Seg {
                len: at,
                seed,
                from: 0,
            },
            Seg {
                len: n,
                seed: other,
                from: 0,
            },
            Seg {
                len: len - at,
                seed,
                from: at,
            },
        ])
    }

    pub fn bytes(mut self) -> Vec<u8> {
        let mut out = Vec::new();
        self.read_to_end(&mut out).unwrap();
        out
    }
}

impl Read for Synth {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut n = 0;
        while n < buf.len() {
            let Some(seg) = self.segs.get(self.seg) else { break };
            if self.at == seg.len {
                self.seg += 1;
                self.at = 0;
                continue;
            }
            let src = seg.from + self.at;
            let word = mix(seg.seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ (src / 8)).to_le_bytes();
            let k = (src % 8) as usize;
            let take = (8 - k).min((seg.len - self.at) as usize).min(buf.len() - n);
            buf[n..n + take].copy_from_slice(&word[k..k + take]);
            n += take;
            self.at += take as u64;
        }
        Ok(n)
    }
}

/// Hands out at most a few bytes per read, to shake out buffering mistakes.
pub struct Trickle<R> {
    pub inner: R,
    pub step: usize,
}

impl<R: Read> Read for Trickle<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.step = self.step % 13 + 1;
        let n = self.step.min(buf.len());
        self.inner.read(&mut buf[..n])
    }
}
