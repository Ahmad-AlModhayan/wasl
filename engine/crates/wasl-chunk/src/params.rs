//! Chunk size limits.

/// The sizes FastCDC cuts to. Every chunk but a file's last is at least `min` and at most `max`
/// bytes; sizes cluster around `avg`. A file of at most `max` bytes is one chunk.
///
/// Two tiers are used ([`ChunkParams::for_size`]): [`ChunkParams::MEDIUM`] (16 KiB, 64 KiB, 256 KiB)
/// for files up to [`TIER_LIMIT`] and [`ChunkParams::DEFAULT`] (1 MiB, 4 MiB, 16 MiB; FR-N9) above
/// it. Devices that cut alike find the most chunks in common; a device that cuts differently (an
/// older version) still syncs correctly, since a receiver fetches whatever chunks a list names,
/// only with less reuse. Other values are for tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkParams {
    min: u32,
    avg: u32,
    max: u32,
}

/// Limits [`ChunkParams::new`] enforces. They are FastCDC's own (the 2020 variant): its masks
/// cover average sizes from 256 B to 4 MiB.
const MIN_RANGE: (u32, u32) = (64, 1 << 20);
const AVG_RANGE: (u32, u32) = (256, 4 << 20);
const MAX_RANGE: (u32, u32) = (1024, 16 << 20);

/// Why [`ChunkParams::new`] refused a set of sizes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("chunk sizes {min}/{avg}/{max} are not usable: {reason}")]
pub struct ParamsError {
    pub min: u32,
    pub avg: u32,
    pub max: u32,
    pub reason: &'static str,
}

/// Files up to this size are cut with [`ChunkParams::MEDIUM`], larger ones with
/// [`ChunkParams::DEFAULT`].
pub const TIER_LIMIT: u64 = 16 << 20;

impl ChunkParams {
    /// Sizes for files up to [`TIER_LIMIT`]: 16 KiB, 64 KiB, 256 KiB. An edit to a document of a few
    /// megabytes then costs about one 64 KiB chunk instead of the whole file; a file of at most
    /// 256 KiB stays one chunk.
    pub const MEDIUM: ChunkParams = ChunkParams {
        min: 16 << 10,
        avg: 64 << 10,
        max: 256 << 10,
    };

    /// The sizes for a file of `size` bytes.
    pub fn for_size(size: u64) -> ChunkParams {
        if size <= TIER_LIMIT {
            ChunkParams::MEDIUM
        } else {
            ChunkParams::DEFAULT
        }
    }

    /// Sizes for files above [`TIER_LIMIT`]: 1 MiB, 4 MiB, 16 MiB.
    pub const DEFAULT: ChunkParams = ChunkParams {
        min: 1 << 20,
        avg: 4 << 20,
        max: 16 << 20,
    };

    /// Checks and builds a set of sizes. All three must be even (FastCDC scans two bytes at a
    /// time), in order, and inside FastCDC's ranges: `min` 64 B–1 MiB, `avg` 256 B–4 MiB, `max`
    /// 1 KiB–16 MiB.
    pub fn new(min: u32, avg: u32, max: u32) -> Result<ChunkParams, ParamsError> {
        let fail = |reason| {
            Err(ParamsError {
                min,
                avg,
                max,
                reason,
            })
        };
        let within = |v: u32, (lo, hi): (u32, u32)| (lo..=hi).contains(&v);
        if !within(min, MIN_RANGE) || !within(avg, AVG_RANGE) || !within(max, MAX_RANGE) {
            return fail("out of range");
        }
        if !min.is_multiple_of(2) || !avg.is_multiple_of(2) || !max.is_multiple_of(2) {
            return fail("sizes must be even");
        }
        if !(min <= avg && avg <= max) {
            return fail("need min <= avg <= max");
        }
        Ok(ChunkParams { min, avg, max })
    }

    pub fn min(&self) -> u32 {
        self.min
    }

    pub fn avg(&self) -> u32 {
        self.avg
    }

    /// Also the largest file that is kept as a single chunk.
    pub fn max(&self) -> u32 {
        self.max
    }
}

impl Default for ChunkParams {
    fn default() -> Self {
        ChunkParams::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_valid() {
        let d = ChunkParams::DEFAULT;
        assert_eq!(ChunkParams::new(d.min, d.avg, d.max), Ok(d));
        let m = ChunkParams::MEDIUM;
        assert_eq!(ChunkParams::new(m.min, m.avg, m.max), Ok(m));
    }

    #[test]
    fn tiers_split_at_the_limit() {
        assert_eq!(ChunkParams::MEDIUM, ChunkParams::for_size(0));
        assert_eq!(ChunkParams::MEDIUM, ChunkParams::for_size(TIER_LIMIT));
        assert_eq!(ChunkParams::DEFAULT, ChunkParams::for_size(TIER_LIMIT + 1));
    }

    #[test]
    fn rejects_bad_sizes() {
        assert!(ChunkParams::new(32, 4096, 16384).is_err());
        assert!(ChunkParams::new(1024, 8 << 20, 16 << 20).is_err());
        assert!(ChunkParams::new(1024, 4096, 32 << 20).is_err());
        assert!(ChunkParams::new(1025, 4096, 16384).is_err());
        assert!(ChunkParams::new(8192, 4096, 16384).is_err());
        assert!(ChunkParams::new(1024, 4096, 2048).is_err());
    }
}
