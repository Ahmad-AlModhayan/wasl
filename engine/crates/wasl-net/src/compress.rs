//! Chunk compression on the wire (docs/COMPRESSION.md §2): zstd per chunk, only when it helps.
//!
//! The asking device says how hard the sender may work (a zstd level; 0 asks for raw bytes); the
//! sending device caps that by its own budget (a phone on battery) and sends a chunk compressed only
//! when it pays: a level-1 trial on its first 64 KiB must save a tenth, and the result must save a
//! twentieth of the whole. Photos, video and archives (already compressed) therefore cost one quick
//! trial and go raw. A chunk's identity is the BLAKE3 of its raw bytes either way, so compression
//! never changes what a device has or asks for.

use crate::peer::MAX_CHUNK_BYTES;

/// Levels by path (docs/COMPRESSION.md §2.3): fast on a local network, where the link is rarely
/// the bottleneck; harder over the internet, where bytes are slower and may cost money.
pub const LEVEL_LAN: u8 = 1;
pub const LEVEL_INTERNET: u8 = 3;
/// Over a metered link (mobile data), bytes cost more than CPU time.
pub const LEVEL_METERED: u8 = 9;
/// Highest level a device asks for or uses: the metered level. Higher zstd levels cost many times
/// the CPU for a few percent more, and a peer must not be able to make this device spend that.
pub const MAX_LEVEL: u8 = LEVEL_METERED;
/// Chunks a device compresses at once for its peers; a request beyond that is answered raw, so
/// peers asking many chunks at once cannot hold this device's CPU.
pub const MAX_PARALLEL: usize = 2;

/// Chunks smaller than this go raw: the frame overhead is a fair share of any saving.
const MIN_BYTES: usize = 512;
/// The trial compresses this much of the chunk at level 1.
const SAMPLE: usize = 64 << 10;

/// A chunk as it travels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Packed {
    Raw(Vec<u8>),
    /// zstd frame of `len` raw bytes.
    Zstd {
        len: u32,
        data: Vec<u8>,
    },
}

impl Packed {
    /// Bytes on the wire (payload only).
    pub fn wire_len(&self) -> usize {
        match self {
            Packed::Raw(b) => b.len(),
            Packed::Zstd { data, .. } => data.len(),
        }
    }
}

/// Compresses `raw` at `level` when that pays (see the module docs); otherwise returns it as is.
pub fn pack(raw: Vec<u8>, level: u8) -> Packed {
    let level = level.min(MAX_LEVEL);
    if level == 0 || raw.len() < MIN_BYTES {
        return Packed::Raw(raw);
    }
    let sample = &raw[..raw.len().min(SAMPLE)];
    match zstd::bulk::compress(sample, 1) {
        Ok(c) if c.len() * 10 <= sample.len() * 9 => {}
        _ => return Packed::Raw(raw),
    }
    match zstd::bulk::compress(&raw, i32::from(level)) {
        Ok(data) if data.len() * 20 <= raw.len() * 19 => Packed::Zstd {
            len: raw.len() as u32,
            data,
        },
        _ => Packed::Raw(raw),
    }
}

/// The raw bytes of a compressed chunk that says it holds `len` bytes. Refuses a `len` above the
/// largest chunk and output of any other length; decompression never allocates past `len`.
pub fn unpack(len: u32, data: &[u8]) -> Result<Vec<u8>, String> {
    let len = len as usize;
    if len > MAX_CHUNK_BYTES {
        return Err(format!("compressed chunk claims {len} bytes"));
    }
    let raw = zstd::bulk::decompress(data, len).map_err(|e| format!("bad compressed chunk: {e}"))?;
    if raw.len() != len {
        return Err(format!("compressed chunk holds {} bytes, says {len}", raw.len()));
    }
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(len: usize) -> Vec<u8> {
        let line = b"fn main() { println!(\"hello from wasl\"); } // a line of source code\n";
        line.iter().copied().cycle().take(len).collect()
    }

    fn noise(len: usize) -> Vec<u8> {
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect()
    }

    #[test]
    fn text_is_compressed_and_comes_back() {
        let raw = text(256 << 10);
        let packed = pack(raw.clone(), LEVEL_INTERNET);
        let Packed::Zstd { len, data } = &packed else {
            panic!("text should compress");
        };
        assert!(data.len() * 5 < raw.len(), "{} of {}", data.len(), raw.len());
        assert_eq!(raw, unpack(*len, data).unwrap());
    }

    #[test]
    fn incompressible_tiny_or_unwanted_goes_raw() {
        let n = noise(256 << 10);
        assert_eq!(Packed::Raw(n.clone()), pack(n, MAX_LEVEL));
        let small = text(MIN_BYTES - 1);
        assert_eq!(Packed::Raw(small.clone()), pack(small, LEVEL_LAN));
        let t = text(4096);
        assert_eq!(Packed::Raw(t.clone()), pack(t, 0));
    }

    #[test]
    fn a_peer_cannot_ask_for_more_than_the_metered_level() {
        let raw = text(256 << 10);
        // zstd is deterministic: level 19 asked is level 9 done.
        assert_eq!(pack(raw.clone(), LEVEL_METERED), pack(raw, 19));
    }

    #[test]
    fn a_lying_or_hostile_frame_is_refused() {
        let raw = text(64 << 10);
        let Packed::Zstd { len, data } = pack(raw, LEVEL_LAN) else {
            panic!("text should compress");
        };
        // Says fewer bytes than it holds: refused without growing past the claim.
        assert!(unpack(len - 1, &data).is_err());
        // Says more: the length check catches it.
        assert!(unpack(len + 1, &data).is_err());
        // Claims more than any chunk may hold.
        assert!(unpack(MAX_CHUNK_BYTES as u32 + 1, &data).is_err());
        assert!(unpack(16, b"not zstd at all").is_err());
        // A bomb: a few bytes that expand to 16 MiB of zeros, claimed as 1 KiB.
        let bomb = zstd::bulk::compress(&vec![0u8; 16 << 20], 19).unwrap();
        assert!(unpack(1024, &bomb).is_err());
    }
}
