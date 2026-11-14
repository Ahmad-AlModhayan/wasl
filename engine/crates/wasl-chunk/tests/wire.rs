//! The wire and JSON forms of a chunk list.

mod common;

use common::Synth;
use proptest::prelude::*;
use wasl_chunk::{ChunkList, ChunkParams, WIRE_VERSION, WireError, chunk_reader};

fn sample() -> ChunkList {
    let p = ChunkParams::new(1024, 4096, 16384).unwrap();
    chunk_reader(Synth::plain(40, 100_000), &p).unwrap()
}

#[test]
fn wire_round_trip() {
    let list = sample();
    assert!(list.chunks.len() > 5);
    let bytes = list.to_bytes();
    assert_eq!(bytes[0], WIRE_VERSION);
    assert_eq!(bytes.len(), 45 + 36 * list.chunks.len());
    assert_eq!(ChunkList::from_bytes(&bytes).unwrap(), list);

    let empty = chunk_reader(&[][..], &ChunkParams::DEFAULT).unwrap();
    assert_eq!(ChunkList::from_bytes(&empty.to_bytes()).unwrap(), empty);
}

#[test]
fn wire_rejects_bad_input() {
    let bytes = sample().to_bytes();
    for cut in 0..bytes.len() {
        assert!(ChunkList::from_bytes(&bytes[..cut]).is_err(), "prefix {cut}");
    }
    let mut longer = bytes.clone();
    longer.push(0);
    assert!(matches!(
        ChunkList::from_bytes(&longer),
        Err(WireError::Length { .. })
    ));
    let mut version = bytes.clone();
    version[0] = 2;
    assert_eq!(ChunkList::from_bytes(&version), Err(WireError::Version(2)));
    // A size that the chunks do not add up to.
    let mut size = bytes.clone();
    size[1] ^= 1;
    assert!(matches!(ChunkList::from_bytes(&size), Err(WireError::List(_))));
    // A huge count with no entries behind it is refused before allocating.
    let mut count = bytes[..45].to_vec();
    count[41..45].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        ChunkList::from_bytes(&count),
        Err(WireError::Length { .. })
    ));
}

#[test]
fn json_uses_hex() {
    let list = sample();
    let json = serde_json::to_value(&list).unwrap();
    let hex = json["blake3"].as_str().unwrap();
    assert_eq!(hex, wasl_chunk::to_hex(&list.blake3));
    assert_eq!(hex.len(), 64);
    assert_eq!(json["chunks"][0]["offset"], 0);
    let back: ChunkList = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(back, list);

    let mut bad = json;
    bad["blake3"] = "abc".into();
    assert!(serde_json::from_value::<ChunkList>(bad).is_err());
}

proptest! {
    #[test]
    fn garbage_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
        if let Ok(list) = ChunkList::from_bytes(&bytes) {
            prop_assert_eq!(list.to_bytes(), bytes);
        }
    }

    #[test]
    fn mutated_lists_decode_consistently(at in 0usize..1000, bit in 0u8..8, cut in 0usize..1000) {
        let mut bytes = sample().to_bytes();
        let at = at % bytes.len();
        bytes[at] ^= 1 << bit;
        let cut = bytes.len() - cut % 40;
        bytes.truncate(cut);
        if let Ok(list) = ChunkList::from_bytes(&bytes) {
            list.check().unwrap();
            prop_assert_eq!(list.to_bytes(), bytes);
        }
    }

    #[test]
    fn valid_headers_with_random_entries(
        entries in proptest::collection::vec((1u32..5000, any::<[u8; 32]>()), 0..20),
        fudge in 0u64..3,
    ) {
        let size: u64 = entries.iter().map(|(l, _)| u64::from(*l)).sum::<u64>() + fudge;
        let mut bytes = vec![WIRE_VERSION];
        bytes.extend_from_slice(&size.to_le_bytes());
        bytes.extend_from_slice(&[7; 32]);
        bytes.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for (len, hash) in &entries {
            bytes.extend_from_slice(&len.to_le_bytes());
            bytes.extend_from_slice(hash);
        }
        let decoded = ChunkList::from_bytes(&bytes);
        prop_assert_eq!(decoded.is_ok(), fudge == 0);
    }
}
