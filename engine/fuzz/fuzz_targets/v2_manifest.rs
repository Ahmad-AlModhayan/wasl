//! Vault v2 file manifests: the CBOR plaintext decoder (reached after decryption, so a fuzzer
//! goes straight to it), and the sealed object as listed on Drive.
//!
//! Invariants: no panic; a decoded manifest validates, re-encodes to the same value, and answers
//! range queries within its chunk list.

#![no_main]

use libfuzzer_sys::fuzz_target;
use wasl_crypto::v2::RangeReader;
use wasl_crypto::v2::keys::VaultKeysV2;
use wasl_crypto::v2::manifest::FileManifest;
use wasl_crypto::v2::names::{DirId, Placement};

fuzz_target!(|data: &[u8]| {
    let keys = VaultKeysV2::from_ikm(&[0x55; 32]);
    if let Ok(m) = FileManifest::from_cbor(data) {
        m.validate().expect("decoded manifests are valid");
        assert_eq!(FileManifest::from_cbor(&m.to_cbor()).ok().as_ref(), Some(&m));
        let reader = RangeReader::new(&keys, &m);
        for (offset, len) in [(0, 1), (m.size / 2, 4096), (m.size.saturating_sub(1), u64::MAX)] {
            assert!(reader.covering(offset, len).end <= m.chunks.len());
        }
    }
    let at = Placement {
        parent: DirId::ROOT,
        name_siv: vec![7; 24],
    };
    // The framing checks in front of AES-GCM (size cap, magic, nonce and tag split).
    let _ = FileManifest::open(&keys, &at, data);
});
