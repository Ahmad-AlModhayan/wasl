//! Shared by the robustness tests of several crates (`tests/robustness.rs`, included with
//! `#[path]`; docs/SECURITY.md S5-2). Two things:
//!
//! * [`mutate`]: a small deterministic mutator. The tests encode a valid value, then flip, set,
//!   insert, delete, duplicate or truncate bytes and splice in tokens that parsers find hard (CBOR
//!   heads announcing 2^64 bytes, indefinite lengths, deep nesting, huge JSON numbers, unpaired
//!   surrogates). Mutating real encodings reaches far deeper into a parser than random bytes do.
//!   The same `seed` always gives the same output, so a failure names its reproducer.
//! * [`bounded`]: runs a parser on its own thread and fails the test if it panics or takes longer
//!   than a limit (a hang is reported instead of stalling the suite).
//!
//! Plain Rust (no test framework), so crates without `proptest` use it with a seed loop.

#![allow(dead_code)]

use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Generous for a debug build on a busy machine; every parser here answers in microseconds.
pub const PER_CASE: Duration = Duration::from_secs(5);

/// SplitMix64: tiny, fast, and the same everywhere.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed ^ 0x5EED_0F5A_5C0D_E500)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (`0` when `n == 0`).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }

    pub fn byte(&mut self) -> u8 {
        self.next_u64() as u8
    }

    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.byte()).collect()
    }
}

/// Byte sequences that make parsers work hard: CBOR heads with the largest lengths, indefinite
/// containers and breaks, tags, JSON delimiters, escapes and numbers out of range.
const TOKENS: &[&[u8]] = &[
    &[0x00],
    &[0xFF],
    &[0x7F],
    &[0x80],
    // CBOR: byte string, text, array, map of 2^64-1 elements.
    &[0x5B, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
    &[0x7B, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
    &[0x9B, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
    &[0xBB, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
    // CBOR: 4 GiB byte string, u64::MAX, negative -2^64.
    &[0x5A, 0xFF, 0xFF, 0xFF, 0xFF],
    &[0x1B, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
    &[0x3B, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
    // CBOR: indefinite byte string, array, map; break; a tag; reserved additional info.
    &[0x5F],
    &[0x9F],
    &[0xBF],
    &[0xFF],
    &[0xC6],
    &[0x1C],
    // CBOR: half float NaN, a simple value.
    &[0xF9, 0x7E, 0x00],
    &[0xF8, 0x20],
    // JSON.
    b"\"",
    b"\\",
    b"{",
    b"}",
    b"[",
    b"]",
    b",",
    b":",
    b"null",
    b"\\u0000",
    b"\\ud800",
    b"-",
    b"1e999999",
    b"-9223372036854775809",
    b"18446744073709551616",
    b"0.0000000000000000000000000000001",
    // UTF-8: a lone continuation byte, an overlong form, a 4-byte character.
    &[0x80, 0x80],
    &[0xC0, 0xAF],
    "\u{1F600}".as_bytes(),
];

/// A mutation of `valid` chosen by `seed`: one to four edits.
pub fn mutate(valid: &[u8], seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let mut v = valid.to_vec();
    let edits = 1 + rng.below(4);
    for _ in 0..edits {
        let at = rng.below(v.len() + 1);
        match rng.below(10) {
            // Flip one bit.
            0 if !v.is_empty() => {
                let i = rng.below(v.len());
                v[i] ^= 1 << rng.below(8);
            }
            // Set one byte.
            1 if !v.is_empty() => {
                let i = rng.below(v.len());
                v[i] = rng.byte();
            }
            // Insert random bytes.
            2 => {
                let n = 1 + rng.below(8);
                let ins = rng.bytes(n);
                v.splice(at..at, ins);
            }
            // Delete a range.
            3 if !v.is_empty() => {
                let n = 1 + rng.below(16.min(v.len()));
                let end = (at + n).min(v.len());
                v.drain(at.min(end)..end);
            }
            // Truncate.
            4 => v.truncate(at),
            // Insert a hard token.
            5 | 6 => {
                let t = TOKENS[rng.below(TOKENS.len())];
                v.splice(at..at, t.iter().copied());
            }
            // Overwrite with a hard token.
            7 => {
                let t = TOKENS[rng.below(TOKENS.len())];
                let end = (at + t.len()).min(v.len());
                v.splice(at..end, t.iter().copied());
            }
            // Duplicate a slice somewhere else (repeated fields, longer lists).
            8 if !v.is_empty() => {
                let from = rng.below(v.len());
                let n = 1 + rng.below((v.len() - from).min(64));
                let piece = v[from..from + n].to_vec();
                v.splice(at..at, piece);
            }
            // Change a small integer by one (lengths, counts, versions).
            _ if !v.is_empty() => {
                let i = rng.below(v.len());
                v[i] = if rng.below(2) == 0 {
                    v[i].wrapping_add(1)
                } else {
                    v[i].wrapping_sub(1)
                };
            }
            _ => {}
        }
    }
    v
}

/// [`mutate`] for text formats: the mutated bytes as (lossy) UTF-8, the way a reader that takes
/// `&str` would see them.
pub fn mutate_text(valid: &str, seed: u64) -> String {
    String::from_utf8_lossy(&mutate(valid.as_bytes(), seed)).into_owned()
}

/// `depth` nested containers around a scalar: CBOR arrays (`0x81`) or JSON arrays.
pub fn deep_cbor(depth: usize) -> Vec<u8> {
    let mut v = vec![0x81; depth];
    v.push(0x00);
    v
}

pub fn deep_json(depth: usize) -> String {
    format!("{}0{}", "[".repeat(depth), "]".repeat(depth))
}

/// Runs `f` on its own thread; fails (naming `what`) if it panics or runs longer than `limit`.
pub fn bounded<T: Send + 'static>(limit: Duration, what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    let started = Instant::now();
    let handle = std::thread::Builder::new()
        .name(format!("robustness: {what}"))
        .spawn(move || {
            let _ = tx.send(f());
        })
        .expect("a thread for the case");
    match rx.recv_timeout(limit) {
        Ok(value) => {
            let _ = handle.join();
            value
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!(
                "{what}: still running after {:?} (hang or super-linear time)",
                started.elapsed()
            )
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            let message = match handle.join() {
                Err(payload) => payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "non-string panic".into()),
                Ok(()) => "thread ended without a result".into(),
            };
            panic!("{what}: panicked: {message}")
        }
    }
}

/// [`bounded`] with [`PER_CASE`].
pub fn guard<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    bounded(PER_CASE, what, f)
}
