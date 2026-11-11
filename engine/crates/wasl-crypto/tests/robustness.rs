//! Robustness of everything `wasl-crypto` reads from Drive, where anyone with access to the folder
//! can write (docs/SECURITY.md S5-2): `vault.json` v1 and v2, v2 file manifests (sealed and their
//! CBOR plaintext), sealed chunks, the `wasl.m` property, `.wasl-dir` markers and `wasl.dir`
//! properties, encrypted names, v1 content streams and chunks, and a whole downloaded vault folder.
//! Arbitrary bytes and mutations of real encodings must give a value or an error: no panic, in
//! bounded time, and nothing tampered may authenticate.
//!
//! Coverage-guided fuzzing of the same parsers: `engine/fuzz` (cargo-fuzz, nightly, Linux/macOS).

#[path = "../../../robustness/mutate.rs"]
mod mutate;

use mutate::{deep_json, guard, mutate, mutate_text};
use proptest::prelude::*;
use wasl_crypto::content::{self, VaultCipher};
use wasl_crypto::kdf::VaultKeys;
use wasl_crypto::recover::{self, Unlocked, Vault};
use wasl_crypto::v2::chunks::{ChunkId, open_chunk, seal_chunk_with_nonce, seal_padded_chunk_with_nonce};
use wasl_crypto::v2::format::Format;
use wasl_crypto::v2::keys::{KdfParams, VaultKeysV2};
use wasl_crypto::v2::manifest::{ChunkRef, FileId, FileManifest, MetaBlob};
use wasl_crypto::v2::names::{self, DirId, Placement};
use wasl_crypto::v2::state::{StateRecord, WriterId};
use wasl_crypto::v2::{RangeReader, VaultManifestV2};
use wasl_crypto::{VaultManifest, names as names_v1};

const CASES: u32 = 512;

fn keys() -> VaultKeysV2 {
    VaultKeysV2::from_ikm(&[0x55; 32])
}

fn keys_v1() -> VaultKeys {
    VaultKeys::new([0x11; 32], [0x22; 32])
}

fn at() -> Placement {
    Placement::for_name(&keys(), &DirId([3; 16]), "report.pdf").unwrap()
}

fn vault_json_v1() -> String {
    VaultManifest::create("Private", &[7; 16], 600_000, &[9; 32], 1_700_000_000_123).to_json()
}

fn vault_json_v2() -> String {
    VaultManifestV2::create(
        "Private",
        &[7; 16],
        KdfParams::DEFAULT,
        &[9; 32],
        1_700_000_000_123,
    )
    .to_json()
}

fn manifest() -> FileManifest {
    let lens = [300_000u32, 1 << 20, 4 << 20, 17];
    let mut size = 0u64;
    let chunks = lens
        .iter()
        .enumerate()
        .map(|(i, &len)| {
            size += u64::from(len);
            ChunkRef {
                id: ChunkId([i as u8 + 1; 32]),
                len,
            }
        })
        .collect();
    FileManifest {
        file_id: FileId([0x42; 16]),
        size,
        mtime_ms: -1_234,
        blake3: [0xB3; 32],
        chunks,
        version: 7,
    }
}

fn check_vault_json(text: String) {
    let r = guard("vault.json", move || {
        let v1 = VaultManifest::parse(&text).map(|m| (m.created_ms(), m.salt_bytes().is_ok()));
        let v2 = VaultManifestV2::parse(&text).map(|m| (m.created_ms(), m.params().is_ok()));
        let any = Vault::parse(&text).map(|v| (v.version(), v.name().to_owned(), v.created().to_owned()));
        (v1.ok(), v2.ok(), any.ok())
    });
    // Whatever parses keeps the reader bounds: v2 costs within limits, v1 iterations within limits.
    if let (_, Some((_, params_ok)), _) = &r {
        assert!(params_ok, "a parsed v2 vault.json has usable KDF parameters");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(CASES))]

    // ---- vault.json ----

    #[test]
    fn mutated_vault_json_parses_or_fails(v2 in any::<bool>(), seed in any::<u64>()) {
        let valid = if v2 { vault_json_v2() } else { vault_json_v1() };
        check_vault_json(mutate_text(&valid, seed));
    }

    #[test]
    fn mutated_v2_vault_json_never_raises_the_kdf_cost(seed in any::<u64>()) {
        let text = mutate_text(&vault_json_v2(), seed);
        if let Ok(m) = guard("VaultManifestV2::parse", move || VaultManifestV2::parse(&text)) {
            let p = m.params().unwrap();
            prop_assert!(p.check().is_ok());
            prop_assert!(p.m_kib <= wasl_crypto::v2::keys::MAX_M_KIB && p.t <= wasl_crypto::v2::keys::MAX_T);
        }
    }

    #[test]
    fn arbitrary_text_is_not_a_vault(text in "\\PC{0,300}") {
        check_vault_json(text);
    }

    // ---- v2 file manifests ----

    #[test]
    fn mutated_manifest_cbor_decodes_or_fails(seed in any::<u64>()) {
        let cbor = mutate(&manifest().to_cbor(), seed);
        let len = cbor.len();
        if let Ok(m) = guard("FileManifest::from_cbor", move || FileManifest::from_cbor(&cbor)) {
            m.validate().unwrap();
            // Each chunk entry takes at least 37 input bytes: no amplification.
            prop_assert!(m.chunks.len() * 37 <= len);
            prop_assert_eq!(FileManifest::from_cbor(&m.to_cbor()).unwrap(), m.clone());
            // Range queries on anything that decodes stay in bounds.
            let k = keys();
            let r = RangeReader::new(&k, &m);
            for (off, n) in [(0, 1), (m.size / 2, 4096), (m.size.saturating_sub(1), u64::MAX), (u64::MAX, 1)] {
                let range = r.covering(off, n);
                prop_assert!(range.end <= m.chunks.len());
            }
        }
    }

    #[test]
    fn random_bytes_are_not_manifests(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
        let b2 = bytes.clone();
        let _ = guard("FileManifest::from_cbor", move || FileManifest::from_cbor(&b2).is_ok());
        let refused = guard("FileManifest::open", move || FileManifest::open(&keys(), &at(), &bytes).is_err());
        prop_assert!(refused);
    }

    #[test]
    fn a_mutated_sealed_manifest_never_opens(seed in any::<u64>()) {
        let sealed = manifest().seal_with_nonce(&keys(), &at(), &[1; 12]);
        let bad = mutate(&sealed, seed);
        prop_assume!(bad != sealed);
        let r = guard("FileManifest::open", move || FileManifest::open(&keys(), &at(), &bad));
        prop_assert!(r.is_err());
    }

    // ---- chunks ----

    #[test]
    fn a_mutated_sealed_chunk_never_opens(seed in any::<u64>(), plain in proptest::collection::vec(any::<u8>(), 1..2048)) {
        let k = keys();
        let file = FileId([5; 16]);
        let id = ChunkId::compute(&k, &file, &plain);
        let sealed = if seed % 2 == 0 {
            seal_chunk_with_nonce(&k, &file, &id, &plain, &[2; 12])
        } else {
            seal_padded_chunk_with_nonce(&k, &file, &id, &plain, &[2; 12])
        };
        let bad = mutate(&sealed, seed);
        prop_assume!(bad != sealed);
        let len = plain.len() as u32;
        let r = guard("open_chunk", move || open_chunk(&keys(), &file, &id, Some(len), &bad));
        prop_assert!(r.is_err());
    }

    // ---- vault state records (rollback detection) ----

    #[test]
    fn mutated_state_records_never_verify(seed in any::<u64>(), name_seed in any::<u64>(), raw in proptest::collection::vec(any::<u8>(), 0..120)) {
        let writer = WriterId([0x5A; 8]);
        let record = StateRecord { writer, counter: 41, time_ms: 1_700_000_000_000, digest: [7; 32] };
        let k_state = *keys().k_state();
        let sealed = record.seal(&k_state);
        let bad = mutate(&sealed, seed);
        let changed = bad != sealed;
        let name = writer.object_name();
        let n2 = name.clone();
        let got = guard("StateRecord::open", move || StateRecord::open(&k_state, &n2, &bad).ok());
        let expected = if changed { None } else { Some(record) };
        prop_assert_eq!(got, expected);
        let bad_name = mutate_text(&name, name_seed);
        let renamed = bad_name != name;
        let got = guard("StateRecord::open name", move || StateRecord::open(&k_state, &bad_name, &sealed).ok());
        prop_assert!(!renamed || got.is_none());
        let n3 = name.clone();
        let _ = guard("StateRecord::open raw", move || StateRecord::open(&k_state, &n3, &raw).is_ok());
    }

    #[test]
    fn chunk_object_names_parse_or_fail(text in "\\PC{0,80}", seed in any::<u64>()) {
        let valid = ChunkId([0xC4; 32]).object_name();
        for t in [text, mutate_text(&valid, seed)] {
            if let Some(id) = guard("ChunkId::from_object_name", move || ChunkId::from_object_name(&t)) {
                prop_assert_eq!(ChunkId::from_object_name(&id.object_name()), Some(id));
            }
        }
    }

    // ---- wasl.m, .wasl-dir, wasl.dir, names ----

    #[test]
    fn mutated_meta_properties_never_verify(seed in any::<u64>(), text in "\\PC{0,130}") {
        let blob = manifest().meta();
        let valid = blob.encode_with_nonce(&keys(), &at(), &[4; 12]);
        let bad = mutate_text(&valid, seed);
        let changed = bad != valid;
        let r = guard("MetaBlob::decode", move || MetaBlob::decode(&keys(), &at(), &bad));
        if changed {
            prop_assert!(r.as_ref().map_or(true, |b| *b == blob));
        }
        let _ = guard("MetaBlob::decode text", move || MetaBlob::decode(&keys(), &at(), &text).is_ok());
    }

    #[test]
    fn mutated_dir_markers_and_properties_never_verify(seed in any::<u64>()) {
        let id = DirId([0xD1; 16]);
        let marker = names::dir_marker(&keys(), &at(), &id);
        let bad = mutate(&marker, seed);
        let changed = bad != marker;
        let got = guard("parse_dir_marker", move || names::parse_dir_marker(&keys(), &at(), &bad));
        prop_assert!(!changed || got.is_none());

        let prop = id.to_property(&keys(), &at());
        let bad = mutate_text(&prop, seed);
        let changed = bad != prop;
        let got = guard("DirId::from_property", move || DirId::from_property(&keys(), &at(), &bad));
        // base64url without padding may have several spellings of the last symbol; any accepted
        // value must still be this folder's id.
        prop_assert!(!changed || got.is_none() || got == Some(id));
    }

    #[test]
    fn mutated_names_decrypt_to_nothing_or_the_name(seed in any::<u64>(), text in "\\PC{0,200}") {
        let parent = DirId([8; 16]);
        let valid = names::encrypt_name(&keys(), &parent, "Taxes 2026.xlsx").unwrap();
        let bad = mutate_text(&valid, seed);
        let t2 = text.clone();
        let got = guard("v2 decrypt_name", move || names::decrypt_name(&keys(), &parent, &bad));
        prop_assert!(got.is_none() || got.as_deref() == Some("Taxes 2026.xlsx"));
        let _ = guard("v2 decrypt_name text", move || names::decrypt_name(&keys(), &parent, &t2));
        let _ = guard("Placement::from_encrypted", move || Placement::from_encrypted(&parent, &text));

        let valid_v1 = names_v1::encrypt_name(&keys_v1(), "parent-token", "Taxes 2026.xlsx").unwrap();
        let bad = mutate_text(&valid_v1, seed);
        let got = guard("v1 decrypt_name", move || names_v1::decrypt_name(&keys_v1(), &bad));
        prop_assert!(got.is_none() || got.as_deref() == Some("Taxes 2026.xlsx"));
    }

    // ---- v1 content ----

    #[test]
    fn a_mutated_v1_stream_never_decrypts_to_other_content(
        plain in proptest::collection::vec(any::<u8>(), 0..3000),
        seed in any::<u64>(),
    ) {
        let cipher = VaultCipher::new(&keys_v1());
        let mut sealed = Vec::new();
        cipher.encrypt(&plain[..], &mut sealed).unwrap();
        let bad = mutate(&sealed, seed);
        let changed = bad != sealed;
        let r = guard("VaultCipher::decrypt", move || {
            let mut out = Vec::new();
            VaultCipher::new(&keys_v1()).decrypt(&bad[..], &mut out).map(|_| out)
        });
        if changed {
            prop_assert!(r.is_err(), "tampered stream decrypted");
        }
    }

    #[test]
    fn v1_chunks_and_headers_parse_or_fail(bytes in proptest::collection::vec(any::<u8>(), 0..200), index in any::<u32>(), last in any::<bool>()) {
        let b2 = bytes.clone();
        let _ = guard("read_header", move || content::read_header(&b2).is_ok());
        let r = guard("decrypt_chunk", move || {
            VaultCipher::new(&keys_v1()).decrypt_chunk(&[0; 8], index, last, &bytes).is_ok()
        });
        prop_assert!(!r);
    }

    /// Sizes come from Drive listings: the layout arithmetic never panics for any of them.
    #[test]
    fn v1_layout_arithmetic_is_total(cipher_size in any::<u64>(), index in any::<u32>(), offset in any::<u64>()) {
        if let Some(plain) = content::plaintext_size(cipher_size) {
            prop_assert_eq!(content::encrypted_size(plain), cipher_size);
            let _ = content::chunk_cipher_range(plain, index);
            let _ = content::chunk_plain_range(plain, index);
            let _ = content::chunk_index_of(plain, offset);
            let _ = content::is_last_chunk(plain, index);
        }
    }
}

proptest! {
    // Each case writes and decrypts a small vault on disk.
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// A downloaded vault folder with one file damaged: the walk reports it and never writes
    /// content that did not verify.
    #[test]
    fn a_damaged_vault_folder_is_reported_not_trusted(pick in any::<prop::sample::Index>(), seed in any::<u64>()) {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("plain");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), b"alpha".repeat(100)).unwrap();
        std::fs::write(src.join("sub").join("b.bin"), (0..70_000u32).map(|i| i as u8).collect::<Vec<_>>()).unwrap();
        let vault = dir.path().join("vault");
        let k = keys();
        let format = if seed % 2 == 0 { Format::V2_0 } else { Format::V2_1 };
        wasl_crypto::v2::layout::write_tree_as(&k, format, &src, &vault).unwrap();

        let mut files = Vec::new();
        let mut stack = vec![vault.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() { stack.push(p) } else { files.push(p) }
            }
        }
        files.sort();
        let victim = pick.get(&files).clone();
        let bytes = std::fs::read(&victim).unwrap();
        let bad = mutate(&bytes, seed);
        prop_assume!(bad != bytes);
        std::fs::write(&victim, bad).unwrap();

        let out = dir.path().join("out");
        let (vault2, out2) = (vault.clone(), out.clone());
        let summary = guard("decrypt_tree", move || {
            recover::decrypt_tree(&vault2, &out2, &Unlocked::V2(keys()), false, |_| {}).unwrap()
        });
        // Every file written is byte-identical to its original.
        for (rel, original) in [("a.txt", src.join("a.txt")), ("sub/b.bin", src.join("sub").join("b.bin"))] {
            let p = out.join(rel);
            if p.exists() {
                prop_assert_eq!(std::fs::read(&p).unwrap(), std::fs::read(&original).unwrap(), "{} decrypted wrong", rel);
            }
        }
        prop_assert!(summary.files <= 2);
    }
}

#[test]
fn deep_and_huge_vault_json_fail_fast() {
    for text in [
        deep_json(100_000),
        format!("{{\"format\":\"wasl-vault\",\"name\":\"{}\"}}", "x".repeat(1 << 20)),
        r#"{"format":"wasl-vault","version":2,"kdf":"argon2id","m":4294967296,"t":1,"p":1,"salt":"","keyCheck":"","created":"","name":""}"#.into(),
        r#"{"format":"wasl-vault","version":2,"kdf":"argon2id","m":262144,"t":4,"p":4,"salt":"AAAAAAAAAAAAAAAAAAAAAA==","keyCheck":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=","created":"9999-99-99T99:99:99Z","name":""}"#.into(),
        r#"{"format":"drivesync-vault","version":1,"kdf":"PBKDF2-HMAC-SHA256","iterations":2147483647,"salt":"AAAAAAAAAAA=","keyCheck":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=","created":"+99999-01-01T00:00:00Z","name":""}"#.into(),
    ] {
        let t = text.clone();
        assert!(guard("Vault::parse", move || Vault::parse(&t).is_err()), "{}", &text[..text.len().min(80)]);
    }
}

#[test]
fn manifest_counts_beyond_the_input_are_refused_before_allocating() {
    // A chunk array announcing 2^20 entries (the maximum) with none behind it.
    let mut cbor = manifest().to_cbor();
    let at = cbor.windows(7).position(|w| w == b"\x66chunks").unwrap() + 7;
    cbor[at] = 0x9A; // array, 4-byte length
    cbor.splice(at + 1..at + 1, [0x00, 0x10, 0x00, 0x00]);
    assert!(guard("from_cbor", move || FileManifest::from_cbor(&cbor).is_err()));
}
