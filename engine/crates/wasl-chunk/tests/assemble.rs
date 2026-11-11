//! Building a file from local and fetched chunks.

mod common;

use common::Synth;
use std::collections::HashMap;
use std::io;
use wasl_chunk::{
    AssembleError, Assembly, ChunkIndex, ChunkList, ChunkLocation, ChunkParams, FetchPlan, Hash, assemble,
    chunk_reader, plan_fetch,
};

const KIB: u64 = 1 << 10;

/// An old version on this side, a new one (edited twice) on the peer's.
struct Case {
    old: Vec<u8>,
    new: Vec<u8>,
    new_list: ChunkList,
    plan: FetchPlan,
}

fn case() -> Case {
    let p = ChunkParams::new(1024, 4096, 16384).unwrap();
    let len = 512 * KIB;
    let old = Synth::plain(30, len).bytes();
    let mut new = Synth::insert(30, len, 100 * KIB, 100, 31).bytes();
    new[300 * KIB as usize..302 * KIB as usize].fill(0xAB);
    let old_list = chunk_reader(&old[..], &p).unwrap();
    let new_list = chunk_reader(&new[..], &p).unwrap();
    let plan = plan_fetch(&new_list, &ChunkIndex::from_lists([("old", &old_list)]));
    assert!(plan.bytes_local() > 0 && plan.bytes_to_fetch() > 0);
    Case {
        old,
        new,
        new_list,
        plan,
    }
}

impl Case {
    fn local(&self) -> impl FnMut(&ChunkLocation) -> io::Result<Vec<u8>> + '_ {
        |at| {
            assert_eq!(at.file, "old");
            let start = at.offset as usize;
            Ok(self.old[start..start + at.len as usize].to_vec())
        }
    }

    /// The peer: serves any chunk of the new version.
    fn peer(&self) -> HashMap<Hash, Vec<u8>> {
        self.new_list
            .chunks
            .iter()
            .map(|c| (c.hash, self.new[c.offset as usize..c.end() as usize].to_vec()))
            .collect()
    }
}

fn serve(peer: &HashMap<Hash, Vec<u8>>) -> impl FnMut(&Hash) -> io::Result<Vec<u8>> + '_ {
    |h| {
        peer.get(h)
            .cloned()
            .ok_or_else(|| io::Error::other("no such chunk"))
    }
}

#[test]
fn round_trip_from_local_and_fetched() {
    let c = case();
    let peer = c.peer();
    let mut out = Vec::new();
    let stats = assemble(&c.plan, c.local(), serve(&peer), &mut out).unwrap();
    assert_eq!(out, c.new);
    assert_eq!(stats.stale, 0);
    assert_eq!(stats.bytes_fetched, c.plan.bytes_to_fetch());
    assert_eq!(stats.local + stats.fetched, c.new_list.chunks.len());
    assert!(stats.local > 0 && stats.fetched > 0);
}

#[test]
fn corrupt_fetched_chunk_is_rejected_by_name() {
    let c = case();
    let mut peer = c.peer();
    let victim = c.plan.to_fetch()[0];
    peer.get_mut(&victim.hash).unwrap()[17] ^= 1;
    let err = assemble(&c.plan, c.local(), serve(&peer), &mut Vec::new()).unwrap_err();
    let index = c
        .new_list
        .chunks
        .iter()
        .position(|x| x.offset == victim.offset)
        .unwrap();
    match &err {
        AssembleError::Chunk {
            index: i,
            offset,
            expected,
        } => {
            assert_eq!((*i, *offset), (index, victim.offset));
            assert_eq!(expected, &wasl_chunk::to_hex(&victim.hash));
        }
        other => panic!("{other}"),
    }
    assert!(err.to_string().contains(&format!("chunk {index}")));
}

#[test]
fn short_fetched_chunk_is_rejected() {
    let c = case();
    let mut peer = c.peer();
    let victim = c.plan.to_fetch()[0];
    peer.get_mut(&victim.hash).unwrap().pop();
    let err = assemble(&c.plan, c.local(), serve(&peer), &mut Vec::new()).unwrap_err();
    assert!(matches!(err, AssembleError::Chunk { .. }), "{err}");
}

#[test]
fn wrong_file_hash_is_rejected() {
    let c = case();
    let mut target = c.new_list.clone();
    target.blake3[0] ^= 1;
    let plan = plan_fetch(&target, &ChunkIndex::new());
    let peer = c.peer();
    let err = assemble(&plan, |_| unreachable!(), serve(&peer), &mut Vec::new()).unwrap_err();
    assert!(matches!(err, AssembleError::File { .. }), "{err}");
}

#[test]
fn stale_local_chunk_is_fetched_instead() {
    let c = case();
    let peer = c.peer();
    let mut out = Vec::new();
    let mut calls = 0;
    let stats = assemble(
        &c.plan,
        |at| {
            calls += 1;
            if calls % 2 == 0 {
                Err(io::Error::from(io::ErrorKind::NotFound))
            } else {
                Ok(vec![0; at.len as usize])
            }
        },
        serve(&peer),
        &mut out,
    )
    .unwrap();
    assert_eq!(out, c.new);
    assert_eq!(stats.local, 0);
    assert_eq!(stats.stale, calls);
}

#[test]
fn fetch_failure_names_the_chunk() {
    let c = case();
    let err = assemble(
        &c.plan,
        c.local(),
        |_| Err(io::Error::other("peer gone")),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert!(matches!(err, AssembleError::Fetch { .. }), "{err}");
}

#[test]
fn resume_keeps_the_verified_head() {
    let c = case();
    let peer = c.peer();
    let chunks = &c.new_list.chunks;
    assert!(chunks.len() > 6);

    // An earlier attempt got through chunk 4 and part of 5, with a bad byte in chunk 4.
    let mut partial = c.new[..chunks[5].offset as usize + 100].to_vec();
    partial[chunks[4].offset as usize + 3] ^= 0xFF;
    let resumed = Assembly::resume(&c.new_list, &partial[..]).unwrap();
    assert_eq!(resumed.verified_chunks(), 4);
    assert_eq!(resumed.verified_bytes(), chunks[4].offset);

    partial.truncate(resumed.verified_bytes() as usize);
    let stats = resumed
        .run(&c.plan, c.local(), serve(&peer), &mut partial)
        .unwrap();
    assert_eq!(partial, c.new);
    assert_eq!(stats.local + stats.fetched, chunks.len() - 4);

    // A complete file resumes to the end and only the file hash is left to check.
    let done = Assembly::resume(&c.new_list, &c.new[..]).unwrap();
    assert_eq!(done.verified_chunks(), chunks.len());
    let mut nothing = Vec::new();
    done.run(&c.plan, |_| unreachable!(), |_| unreachable!(), &mut nothing)
        .unwrap();
    assert!(nothing.is_empty());
}

#[test]
fn resume_against_another_plan_is_refused() {
    let c = case();
    let other = chunk_reader(&c.old[..], &ChunkParams::new(1024, 4096, 16384).unwrap()).unwrap();
    let resumed = Assembly::resume(&other, &[][..]).unwrap();
    let err = resumed
        .run(&c.plan, c.local(), |_| unreachable!(), &mut Vec::new())
        .unwrap_err();
    assert!(matches!(err, AssembleError::WrongPlan));
}

#[test]
fn empty_file_assembles() {
    let list = chunk_reader(&[][..], &ChunkParams::DEFAULT).unwrap();
    let plan = plan_fetch(&list, &ChunkIndex::new());
    assert_eq!(plan.bytes_to_fetch(), 0);
    let mut out = Vec::new();
    assemble(&plan, |_| unreachable!(), |_| unreachable!(), &mut out).unwrap();
    assert!(out.is_empty());
}
