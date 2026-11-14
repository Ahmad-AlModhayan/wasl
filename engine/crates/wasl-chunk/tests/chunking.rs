//! Chunk boundaries and what an edit costs (FR-N9).

mod common;

use common::{Synth, Trickle};
use wasl_chunk::{ChunkIndex, ChunkList, ChunkParams, FetchPlan, chunk_reader, plan_fetch};

const MIB: u64 = 1 << 20;
const KIB: u64 = 1 << 10;

fn small() -> ChunkParams {
    ChunkParams::new(4096, 16384, 65536).unwrap()
}

fn check_shape(list: &ChunkList, p: &ChunkParams) {
    list.check().unwrap();
    let (last, rest) = list.chunks.split_last().unwrap();
    for c in rest {
        assert!(
            (p.min()..=p.max()).contains(&c.len),
            "chunk at {} is {} bytes",
            c.offset,
            c.len
        );
    }
    assert!(last.len > 0 && last.len <= p.max());
    let sum: u64 = list.chunks.iter().map(|c| u64::from(c.len)).sum();
    assert_eq!(sum, list.size);
}

#[test]
fn same_input_same_chunks_within_bounds() {
    let p = small();
    let data = Synth::plain(1, 8 * MIB + 333).bytes();
    let a = chunk_reader(&data[..], &p).unwrap();
    let b = chunk_reader(Synth::plain(1, 8 * MIB + 333), &p).unwrap();
    assert_eq!(a, b);
    check_shape(&a, &p);
    assert_eq!(a.size, data.len() as u64);
    assert_eq!(&a.blake3, blake3::hash(&data).as_bytes());
    for c in &a.chunks {
        let bytes = &data[c.offset as usize..c.end() as usize];
        assert_eq!(&c.hash, blake3::hash(bytes).as_bytes());
    }
    // Plenty of chunks, averaging near the target.
    let avg = a.size / a.chunks.len() as u64;
    assert!(a.chunks.len() > 100, "{} chunks", a.chunks.len());
    assert!((8 * KIB..=40 * KIB).contains(&avg), "average {avg}");
}

#[test]
fn tiny_reads_do_not_move_boundaries() {
    let p = small();
    let data = Synth::plain(2, MIB + 77).bytes();
    let whole = chunk_reader(&data[..], &p).unwrap();
    let trickled = chunk_reader(
        Trickle {
            inner: &data[..],
            step: 0,
        },
        &p,
    )
    .unwrap();
    assert_eq!(whole, trickled);
}

#[test]
fn default_bounds_hold_on_a_large_file() {
    let p = ChunkParams::DEFAULT;
    let list = chunk_reader(Synth::plain(3, 80 * MIB + 5), &p).unwrap();
    check_shape(&list, &p);
}

#[test]
fn files_up_to_max_are_one_chunk() {
    let p = small();
    let empty = chunk_reader(&[][..], &p).unwrap();
    assert_eq!(empty.size, 0);
    assert!(empty.chunks.is_empty());
    assert_eq!(&empty.blake3, blake3::hash(b"").as_bytes());

    let max = u64::from(p.max());
    let at_max = chunk_reader(Synth::plain(4, max), &p).unwrap();
    assert_eq!(at_max.chunks.len(), 1);
    assert_eq!(at_max.chunks[0].hash, at_max.blake3);
    assert_eq!(at_max.chunks[0].len, p.max());

    let over = chunk_reader(Synth::plain(4, max + 1), &p).unwrap();
    assert!(over.chunks.len() >= 2);
    check_shape(&over, &p);

    // The protocol's 16 MiB threshold.
    let d = ChunkParams::DEFAULT;
    let sixteen = chunk_reader(Synth::plain(5, 16 * MIB), &d).unwrap();
    assert_eq!(sixteen.chunks.len(), 1);
    let more = chunk_reader(Synth::plain(5, 16 * MIB + 1), &d).unwrap();
    assert!(more.chunks.len() >= 2);
}

/// The plan for `new` when the receiver holds `old`.
fn plan(old: &ChunkList, new: &ChunkList) -> FetchPlan {
    plan_fetch(new, &ChunkIndex::from_lists([("old", old)]))
}

/// FR-N9, scaled to 64 MiB: overwriting 256 KiB in the middle costs one chunk here; the test
/// allows two maximum-size chunks, the envelope measured in `edit_cost_distribution`.
#[test]
fn overwrite_in_the_middle_fetches_one_chunk() {
    let p = ChunkParams::DEFAULT;
    let len = 64 * MIB;
    let old = chunk_reader(Synth::plain(10, len), &p).unwrap();
    let new = chunk_reader(Synth::overwrite(10, len, len / 2, 256 * KIB, 11), &p).unwrap();
    check_shape(&old, &p);
    check_shape(&new, &p);
    let plan = plan(&old, &new);
    let fetch = plan.to_fetch();
    eprintln!(
        "64 MiB overwrite: {} -> {} chunks, fetch {} chunk(s), {} bytes",
        old.chunks.len(),
        new.chunks.len(),
        fetch.len(),
        plan.bytes_to_fetch()
    );
    assert!(fetch.len() <= 2);
    assert!(plan.bytes_to_fetch() <= 2 * u64::from(p.max()));
    // The changed chunk is the one holding the edit.
    assert!(
        fetch
            .iter()
            .all(|c| c.offset < len / 2 + 256 * KIB + 64 && c.end() > len / 2 - 64)
    );
    assert_eq!(plan.bytes_local() + plan.bytes_to_fetch(), len);
}

#[test]
fn insertion_near_the_start_fetches_little() {
    let p = ChunkParams::DEFAULT;
    let len = 64 * MIB;
    let old = chunk_reader(Synth::plain(20, len), &p).unwrap();
    let new = chunk_reader(Synth::insert(20, len, 100 * KIB, KIB, 21), &p).unwrap();
    assert_eq!(new.size, len + KIB);
    let plan = plan(&old, &new);
    eprintln!(
        "64 MiB insert: {} -> {} chunks, fetch {} chunk(s), {} bytes",
        old.chunks.len(),
        new.chunks.len(),
        plan.to_fetch().len(),
        plan.bytes_to_fetch()
    );
    assert!(plan.to_fetch().len() <= 2);
    assert!(plan.bytes_to_fetch() <= 2 * u64::from(p.max()));
}

/// The same edits over many seeds at a smaller scale (sizes / 256, the edit in proportion), to
/// see the distribution the crate docs describe rather than one sample. Scaling keeps the odds
/// of a stray cut in the edited window about the same (some 13 % here, 12.5 % at full size).
/// `WASL_CHUNK_TRIALS` raises the number of seeds.
#[test]
fn edit_cost_distribution() {
    let p = ChunkParams::new(4096, 16384, 65536).unwrap();
    let len = 256 * KIB;
    let trials: u64 = std::env::var("WASL_CHUNK_TRIALS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);
    let mut overwrite = [0u64; 8];
    let mut insert = [0u64; 8];
    let mut worst_bytes = 0u64;
    for seed in 0..trials {
        let old = chunk_reader(Synth::plain(1000 + seed, len), &p).unwrap();
        let index = ChunkIndex::from_lists([("old", &old)]);

        let edited = Synth::overwrite(1000 + seed, len, len / 2, KIB, 7 + seed);
        let plan = plan_fetch(&chunk_reader(edited, &p).unwrap(), &index);
        overwrite[plan.to_fetch().len().min(7)] += 1;
        worst_bytes = worst_bytes.max(plan.bytes_to_fetch());

        let edited = Synth::insert(1000 + seed, len, 400 + seed * 13 % 8000, 4, 7 + seed);
        let plan = plan_fetch(&chunk_reader(edited, &p).unwrap(), &index);
        insert[plan.to_fetch().len().min(7)] += 1;
        worst_bytes = worst_bytes.max(plan.bytes_to_fetch());
    }
    eprintln!(
        "{trials} trials, chunks fetched 0..7+: overwrite {overwrite:?}, insert {insert:?}; worst {:.2} x max",
        worst_bytes as f64 / f64::from(p.max())
    );
    for hist in [overwrite, insert] {
        assert_eq!(hist[0], 0);
        // One chunk in at least 85 % of edits, more than three in at most 2 %.
        assert!(hist[1] * 100 >= trials * 85, "{hist:?}");
        assert!(hist[4..].iter().sum::<u64>() * 100 <= trials * 2, "{hist:?}");
    }
    // In bytes: never more than two maximum-size chunks (worst seen in 10,000 edits: 1.88).
    assert!(worst_bytes <= 2 * u64::from(p.max()));
}
