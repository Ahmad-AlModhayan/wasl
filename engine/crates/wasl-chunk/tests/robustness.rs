//! Robustness of chunk lists and assembly against a hostile peer (docs/SECURITY.md S5-2): the wire
//! and JSON forms of a chunk list, arbitrary and mutated, and assembling a file from chunks a peer
//! sends (wrong, short, swapped) or from a damaged partial file. No panic, bounded time and
//! output, and a file is only ever written as the bytes the list names.

#[path = "../../../robustness/mutate.rs"]
mod mutate;

use mutate::{guard, mutate};
use proptest::prelude::*;
use wasl_chunk::{Assembly, ChunkIndex, ChunkList, ChunkParams, assemble, chunk_reader, plan_fetch};

const CASES: u32 = 512;

fn data(seed: u64, len: usize) -> Vec<u8> {
    let mut rng = mutate::Rng::new(seed);
    rng.bytes(len)
}

fn params() -> ChunkParams {
    ChunkParams::new(1024, 4096, 16384).unwrap()
}

fn list_of(bytes: &[u8]) -> ChunkList {
    chunk_reader(bytes, &params()).unwrap()
}

/// The chunk with this hash, from the original bytes.
fn chunk_bytes<'a>(data: &'a [u8], list: &ChunkList, hash: &[u8; 32]) -> Option<&'a [u8]> {
    let c = list.chunks.iter().find(|c| &c.hash == hash)?;
    Some(&data[c.offset as usize..c.end() as usize])
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(CASES))]

    #[test]
    fn mutated_wire_lists_decode_or_fail(seed in any::<u64>()) {
        let bytes = mutate(&list_of(&data(1, 60_000)).to_bytes(), seed);
        let b2 = bytes.clone();
        if let Ok(list) = guard("ChunkList::from_bytes", move || ChunkList::from_bytes(&b2)) {
            prop_assert!(list.check().is_ok());
            // The wire form has one spelling per list.
            prop_assert_eq!(list.to_bytes(), bytes);
        }
    }

    #[test]
    fn random_bytes_are_not_lists(bytes in proptest::collection::vec(any::<u8>(), 0..600)) {
        let len = bytes.len();
        if let Ok(list) = guard("ChunkList::from_bytes", move || ChunkList::from_bytes(&bytes)) {
            prop_assert!(list.check().is_ok());
            prop_assert!(list.chunks.len() * 36 < len);
        }
    }

    #[test]
    fn mutated_json_lists_parse_or_fail_and_checked_ones_plan(seed in any::<u64>()) {
        let original = list_of(&data(2, 30_000));
        let json = mutate(&serde_json::to_vec(&original).unwrap(), seed);
        let r = guard("ChunkList JSON", move || {
            let list: ChunkList = serde_json::from_slice(&json).ok()?;
            list.check().ok()?;
            let mut index = ChunkIndex::new();
            index.add("local.bin", &original);
            let plan = plan_fetch(&list, &index);
            Some((plan.bytes_to_fetch() + plan.bytes_local(), list.size))
        });
        if let Some((planned, size)) = r {
            prop_assert_eq!(planned, size);
        }
    }

    /// A peer answering with damaged chunks: the file is refused or exactly right.
    #[test]
    fn assembly_from_a_hostile_peer_writes_only_named_bytes(
        len in 0usize..80_000,
        seed in any::<u64>(),
        victim in any::<prop::sample::Index>(),
    ) {
        let original = data(seed, len);
        let list = list_of(&original);
        let r = guard("assemble", move || {
            let plan = plan_fetch(&list, &ChunkIndex::new());
            let bad = if list.chunks.is_empty() { None } else { Some(victim.index(list.chunks.len())) };
            let mut out = Vec::new();
            let result = assemble(
                &plan,
                |_| Err(std::io::Error::other("no local copies")),
                |hash| {
                    let good = chunk_bytes(&original, &list, hash).unwrap().to_vec();
                    match bad {
                        Some(i) if list.chunks[i].hash == *hash => Ok(mutate(&good, seed)),
                        _ => Ok(good),
                    }
                },
                &mut out,
            );
            (result.is_ok(), out, original)
        });
        let (ok, out, original) = r;
        if ok {
            prop_assert_eq!(out, original);
        } else {
            // Whatever was written before the refusal is a verified prefix.
            prop_assert!(original.starts_with(&out));
        }
    }

    /// Resuming from a damaged temp file keeps only the verified head.
    #[test]
    fn resume_keeps_only_verified_chunks(len in 1usize..60_000, seed in any::<u64>()) {
        let original = data(seed ^ 7, len);
        let list = list_of(&original);
        let partial = mutate(&original, seed);
        let (at, original, list) = guard("Assembly::resume", move || {
            (Assembly::resume(&list, &partial[..]).unwrap(), original, list)
        });
        let kept = at.verified_bytes() as usize;
        prop_assert!(kept <= original.len());
        prop_assert_eq!(list.chunks[..at.verified_chunks()].iter().map(|c| c.len as usize).sum::<usize>(), kept);
        let plan = plan_fetch(&list, &ChunkIndex::new());
        let mut out = original[..kept].to_vec();
        at.run(
            &plan,
            |_| Err(std::io::Error::other("none")),
            |hash| Ok(chunk_bytes(&original, &list, hash).unwrap().to_vec()),
            &mut out,
        ).unwrap();
        prop_assert_eq!(out, original);
    }
}

#[test]
fn a_list_naming_offsets_near_the_end_of_u64_is_refused() {
    let json = format!(
        r#"{{"size":0,"blake3":"{h}","chunks":[{{"offset":18446744073709551615,"len":1,"hash":"{h}"}}]}}"#,
        h = "00".repeat(32)
    );
    let list: ChunkList = serde_json::from_str(&json).unwrap();
    assert!(list.check().is_err());
}
