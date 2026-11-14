//! Vault v2 building blocks: keys, vault.json, names, manifests, the `wasl.m` blob, chunks.

mod common;

use common::{assert_integrity, random_bytes};
use proptest::prelude::*;
use wasl_crypto::v2::VaultManifestV2;
use wasl_crypto::v2::chunks::{self, ChunkId, MAX_CHUNK_BYTES, OVERHEAD, open_chunk, seal_chunk};
use wasl_crypto::v2::keys::{self, KdfParams, VaultKeysV2};
use wasl_crypto::v2::manifest::{ChunkRef, FileId, FileManifest, MAX_CHUNKS, META_PROPERTY, MetaBlob};
use wasl_crypto::v2::names::{self, DirId, Placement};

fn keys() -> VaultKeysV2 {
    VaultKeysV2::from_ikm(&[0x55; 32])
}

fn other_keys() -> VaultKeysV2 {
    VaultKeysV2::from_ikm(&[0x66; 32])
}

fn at(parent: u8, name: &str) -> Placement {
    Placement::for_name(&keys(), &DirId([parent; 16]), name).unwrap()
}

// ---------------------------------------------------------------- keys and vault.json

#[test]
fn kdf_bounds_are_enforced() {
    assert!(KdfParams::DEFAULT.check().is_ok());
    assert!(KdfParams::MIN.check().is_ok());
    // The edges of the reader bounds (security review 2026-10).
    for good in [
        KdfParams {
            m_kib: 256 * 1024,
            t: 2,
            p: 4,
        },
        KdfParams {
            m_kib: 128 * 1024,
            t: 4,
            p: 1,
        },
    ] {
        assert!(good.check().is_ok(), "{good:?}");
    }
    for bad in [
        KdfParams {
            m_kib: 19 * 1024 - 1,
            ..KdfParams::MIN
        },
        KdfParams {
            m_kib: 256 * 1024 + 1,
            ..KdfParams::MIN
        },
        KdfParams {
            t: 0,
            ..KdfParams::MIN
        },
        KdfParams {
            t: 5,
            ..KdfParams::MIN
        },
        KdfParams {
            p: 0,
            ..KdfParams::MIN
        },
        KdfParams {
            p: 5,
            ..KdfParams::MIN
        },
        // Each bound alone is fine, together they are over the m·t budget.
        KdfParams {
            m_kib: 256 * 1024,
            t: 4,
            p: 1,
        },
        KdfParams {
            m_kib: 192 * 1024,
            t: 3,
            p: 1,
        },
    ] {
        assert_integrity(bad.check());
        assert_integrity(keys::derive("pw", &[0; 16], bad).map(|_| ()));
    }
}

#[test]
fn derive_and_key_check() {
    let salt = [7u8; 16];
    let a = keys::derive("correct horse", &salt, KdfParams::MIN).unwrap();
    let b = keys::derive("correct horse", &salt, KdfParams::MIN).unwrap();
    let c = keys::derive("correct horsf", &salt, KdfParams::MIN).unwrap();
    assert_eq!(a.ikm(), b.ikm());
    assert_ne!(a.ikm(), c.ikm());
    assert!(a.key_check_matches(&b.key_check()));
    assert!(!a.key_check_matches(&c.key_check()));
    assert!(!a.key_check_matches(&a.key_check()[..31]));
    // Sub-keys are distinct from each other.
    assert_ne!(&a.k_meta()[..], &a.k_chunk()[..]);
    assert_ne!(&a.k_chunk()[..], &a.k_chunk_id()[..]);
    assert_eq!("VaultKeysV2(<redacted>)", format!("{a:?}"));
    assert!(keys::derive("", &salt, KdfParams::MIN).is_err());
    assert!(keys::derive("x", &salt[..8], KdfParams::MIN).is_err());
}

#[test]
fn vault_json_round_trip_and_strictness() {
    let salt = [9u8; 16];
    let k = keys::derive("pass phrase", &salt, KdfParams::MIN).unwrap();
    let m = VaultManifestV2::create("Photos", &salt, KdfParams::MIN, &k.key_check(), 1_760_000_000_123);
    let json = m.to_json();
    let parsed = VaultManifestV2::parse(&json).unwrap();
    assert_eq!(m, parsed);
    assert_eq!(Some(1_760_000_000_123), parsed.created_ms());
    assert!(parsed.same_vault_as(&m));
    assert!(parsed.verify_passphrase("pass phrase").unwrap().is_some());
    assert!(parsed.verify_passphrase("wrong").unwrap().is_none());

    // Unknown keys are ignored.
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    v["future"] = serde_json::json!({"x": 1});
    assert!(VaultManifestV2::parse(&v.to_string()).is_ok());
    // Other format, version, kdf, out-of-bounds costs, bad salt or key check are refused.
    let edits: Vec<(&str, serde_json::Value)> = vec![
        ("format", "drivesync-vault".into()),
        ("version", 4.into()),
        ("version", 1.into()),
        ("kdf", "argon2i".into()),
        ("m", (2u64 << 20).into()),
        ("m", 1024.into()),
        ("t", 100.into()),
        ("t", 0.into()),
        ("p", 64.into()),
        ("salt", "AAAA".into()),
        ("keyCheck", "AAAA".into()),
        ("salt", "not base64!".into()),
    ];
    for (key, value) in edits {
        let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
        v[key] = value;
        assert_integrity(VaultManifestV2::parse(&v.to_string()));
    }
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    v.as_object_mut().unwrap().remove("salt");
    assert_integrity(VaultManifestV2::parse(&v.to_string()));
    assert_integrity(VaultManifestV2::parse("not json"));
}

#[test]
fn recover_detects_the_version() {
    use wasl_crypto::recover::Vault;
    let salt = [1u8; 16];
    let v2 = VaultManifestV2::create("a", &salt, KdfParams::MIN, &[0; 32], 0);
    assert_eq!(2, Vault::parse(&v2.to_json()).unwrap().version());
    let v1 = wasl_crypto::VaultManifest::create("b", &salt, 600_000, &[0; 32], 0);
    assert_eq!(1, Vault::parse(&v1.to_json()).unwrap().version());
    assert_integrity(Vault::parse(r#"{"format":"other","version":2}"#));
}

// ---------------------------------------------------------------- names

#[test]
fn names_round_trip_and_bind_to_the_parent() {
    let k = keys();
    let a = DirId([1; 16]);
    let b = DirId([2; 16]);
    for name in [
        "hello.txt",
        "ملف مهم.txt",
        "写真 📷.jpg",
        "a",
        ".hidden",
        "...",
        &"x".repeat(250),
    ] {
        let enc = names::encrypt_name(&k, &a, name).unwrap();
        assert!(enc.ends_with(".w2"));
        assert_eq!(enc, names::encrypt_name(&k, &a, name).unwrap(), "deterministic");
        assert_ne!(enc, names::encrypt_name(&k, &b, name).unwrap());
        assert_eq!(Some(name.to_owned()), names::decrypt_name(&k, &a, &enc));
        assert_eq!(None, names::decrypt_name(&k, &b, &enc), "moved to another folder");
        assert_eq!(None, names::decrypt_name(&other_keys(), &a, &enc));
        assert_eq!(None, names::decrypt_name(&k, &DirId::ROOT, &enc));
    }
}

#[test]
fn names_refuse_invalid_and_malformed() {
    let k = keys();
    let p = DirId::ROOT;
    for bad in ["", ".", "..", "a/b", "nul\0byte"] {
        assert!(names::encrypt_name(&k, &p, bad).is_err(), "{bad:?}");
    }
    let enc = names::encrypt_name(&k, &p, "hello.txt").unwrap();
    let body = enc.strip_suffix(".w2").unwrap();
    for bad in [
        String::new(),
        ".w2".into(),
        body.to_owned(),
        format!("{body}.dsv"),
        format!("{body}=.w2"),
        format!("{body}A.w2"),
        format!("{}.w2", &body[..body.len() - 1]),
        format!("{}+.w2", &body[..body.len() - 1]),
        format!("{}/.w2", &body[..body.len() - 1]),
        "AAAAAAAAAAAAAAAAAAAAAA.w2".into(),
        "vault.json".into(),
    ] {
        assert_eq!(None, names::decrypt_name(&k, &p, &bad), "{bad:?}");
    }
}

#[test]
fn names_with_forbidden_plaintext_are_refused_on_decrypt() {
    // A key holder could seal "..", "a/b" or NUL directly with AES-SIV; readers refuse them.
    use aes_siv::KeyInit;
    use base64::Engine as _;
    let k = keys();
    let p = DirId([3; 16]);
    for bad in ["", ".", "..", "a/b", "x\0"] {
        let raw = aes_siv::siv::Aes256Siv::new(k.k_name().into())
            .encrypt([p.as_bytes()], bad.as_bytes())
            .unwrap();
        let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw) + ".w2";
        assert_eq!(None, names::decrypt_name(&k, &p, &enc), "{bad:?}");
    }
}

#[test]
fn every_bit_flip_in_a_name_is_refused() {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let k = keys();
    let p = DirId([4; 16]);
    let enc = names::encrypt_name(&k, &p, "report.pdf").unwrap();
    let raw = b64.decode(enc.strip_suffix(".w2").unwrap()).unwrap();
    for bit in 0..raw.len() * 8 {
        let mut r = raw.clone();
        r[bit / 8] ^= 1 << (bit % 8);
        assert_eq!(
            None,
            names::decrypt_name(&k, &p, &(b64.encode(&r) + ".w2")),
            "bit {bit}"
        );
    }
}

#[test]
fn dir_ids_and_markers() {
    let id = DirId::random().unwrap();
    assert_ne!(id, DirId::random().unwrap());
    let k = keys();
    let here = at(1, "Photos");
    let prop = id.to_property(&k, &here);
    assert_eq!(64, prop.len());
    assert!("wasl.dir".len() + prop.len() <= 124);
    assert_eq!(Some(id), DirId::from_property(&k, &here, &prop));
    let marker = names::dir_marker(&k, &here, &id);
    assert_eq!(52, marker.len());
    assert_eq!(Some(id), names::parse_dir_marker(&k, &here, &marker));
    // Moved to another folder, renamed, other keys, truncated, tampered (every bit): refused.
    assert_eq!(None, names::parse_dir_marker(&k, &at(2, "Photos"), &marker));
    assert_eq!(None, names::parse_dir_marker(&k, &at(1, "Photo"), &marker));
    assert_eq!(None, names::parse_dir_marker(&other_keys(), &here, &marker));
    assert_eq!(None, DirId::from_property(&k, &at(2, "Photos"), &prop));
    assert_eq!(None, names::parse_dir_marker(&k, &here, &marker[..51]));
    for bit in 0..marker.len() * 8 {
        let mut m = marker.clone();
        m[bit / 8] ^= 1 << (bit % 8);
        assert_eq!(None, names::parse_dir_marker(&k, &here, &m), "bit {bit}");
    }
    // A marker re-pointed at another id (the attack: re-keying the children's names).
    let other = DirId::random().unwrap();
    let mut forged = marker.clone();
    forged[4..20].copy_from_slice(other.as_bytes());
    assert_eq!(None, names::parse_dir_marker(&k, &here, &forged));
    assert_eq!(None, DirId::from_property(&k, &here, "short"));
}

// ---------------------------------------------------------------- manifests

fn sample_manifest() -> FileManifest {
    let k = keys();
    let fid = FileId([0xab; 16]);
    let chunks: Vec<ChunkRef> = (0..5u32)
        .map(|i| ChunkRef {
            id: ChunkId::compute(&k, &fid, &i.to_be_bytes()),
            len: 1000 + i,
        })
        .collect();
    FileManifest {
        file_id: fid,
        size: chunks.iter().map(|c| u64::from(c.len)).sum(),
        mtime_ms: -12345,
        blake3: [7; 32],
        chunks,
        version: 3,
    }
}

#[test]
fn manifest_cbor_is_deterministic_cbor() {
    let m = sample_manifest();
    let cbor = m.to_cbor();
    // A generic CBOR decoder reads the same structure.
    let v: ciborium::Value = ciborium::from_reader(&cbor[..]).unwrap();
    let map = v.as_map().unwrap();
    let keys: Vec<&str> = map.iter().map(|(k, _)| k.as_text().unwrap()).collect();
    assert_eq!(
        vec!["size", "mtime", "blake3", "chunks", "file_id", "version"],
        keys
    );
    assert_eq!(map[3].1.as_array().unwrap().len(), 5);
    // Re-encoding with ciborium gives the same bytes (shortest heads, definite lengths).
    let mut again = Vec::new();
    ciborium::into_writer(&v, &mut again).unwrap();
    assert_eq!(cbor, again);
    assert_eq!(m, FileManifest::from_cbor(&cbor).unwrap());
}

#[test]
fn manifest_decode_bounds() {
    let mut m = sample_manifest();
    m.size += 1;
    assert_integrity(FileManifest::from_cbor(&m.to_cbor()));
    let mut m = sample_manifest();
    m.chunks[2].len = 0;
    m.size -= 1002;
    assert_integrity(FileManifest::from_cbor(&m.to_cbor()));
    let mut m = sample_manifest();
    m.chunks[0].len = MAX_CHUNK_BYTES as u32 + 1;
    m.size = m.chunks.iter().map(|c| u64::from(c.len)).sum();
    assert_integrity(FileManifest::from_cbor(&m.to_cbor()));
    // Trailing bytes, a missing key, a non-shortest head.
    let mut cbor = sample_manifest().to_cbor();
    cbor.push(0);
    assert_integrity(FileManifest::from_cbor(&cbor));
    let cbor = sample_manifest().to_cbor();
    assert_integrity(FileManifest::from_cbor(&cbor[..cbor.len() - 1]));
    let mut long = vec![0xb8, 6];
    long.extend_from_slice(&cbor[1..]);
    assert_integrity(FileManifest::from_cbor(&long));
    // A chunk count far beyond the input is refused before allocating.
    let mut huge = Vec::new();
    huge.extend_from_slice(&[0xa6, 0x64]);
    huge.extend_from_slice(b"size");
    huge.push(0);
    huge.push(0x65);
    huge.extend_from_slice(b"mtime");
    huge.push(0);
    huge.push(0x66);
    huge.extend_from_slice(b"blake3");
    huge.extend_from_slice(&[0x58, 32]);
    huge.extend_from_slice(&[0; 32]);
    huge.push(0x66);
    huge.extend_from_slice(b"chunks");
    huge.extend_from_slice(&[0x9b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
    assert_integrity(FileManifest::from_cbor(&huge));
    let too_many = FileManifest {
        chunks: vec![
            ChunkRef {
                id: ChunkId([0; 32]),
                len: 1
            };
            MAX_CHUNKS + 1
        ],
        size: MAX_CHUNKS as u64 + 1,
        ..sample_manifest()
    };
    assert_integrity(too_many.validate());
    // An empty file has no chunks.
    let empty = FileManifest {
        size: 0,
        chunks: vec![],
        ..sample_manifest()
    };
    assert_eq!(empty, FileManifest::from_cbor(&empty.to_cbor()).unwrap());
}

#[test]
fn manifest_seal_open_and_every_bit_flip_is_refused() {
    let k = keys();
    let m = sample_manifest();
    let here = at(1, "a.txt");
    let sealed = m.seal(&k, &here).unwrap();
    assert_eq!(b"W2M1", &sealed[..4]);
    assert_eq!(m, FileManifest::open(&k, &here, &sealed).unwrap());
    assert_ne!(sealed, m.seal(&k, &here).unwrap(), "fresh nonce every time");
    assert_integrity(FileManifest::open(&other_keys(), &here, &sealed));
    for bit in 0..sealed.len() * 8 {
        let mut s = sealed.clone();
        s[bit / 8] ^= 1 << (bit % 8);
        assert_integrity(FileManifest::open(&k, &here, &s));
    }
    for cut in 0..sealed.len() {
        assert_integrity(FileManifest::open(&k, &here, &sealed[..cut]));
    }
    let mut longer = sealed.clone();
    longer.push(0);
    assert_integrity(FileManifest::open(&k, &here, &longer));
    // A chunk object is not a manifest, even with the magic changed.
    let fid = FileId([1; 16]);
    let id = ChunkId::compute(&k, &fid, b"abc");
    let mut chunk = seal_chunk(&k, wasl_crypto::v2::Format::V2_0, &fid, &id, b"abc").unwrap();
    chunk[..4].copy_from_slice(b"W2M1");
    assert_integrity(FileManifest::open(&k, &here, &chunk));
}

#[test]
fn manifests_are_bound_to_their_file() {
    let k = keys();
    let m_a = FileManifest {
        file_id: FileId([0xa; 16]),
        ..sample_manifest()
    };
    let m_b = FileManifest {
        file_id: FileId([0xb; 16]),
        ..sample_manifest()
    };
    let (a, b) = (at(1, "a.txt"), at(1, "b.txt"));
    let sealed_a = m_a.seal(&k, &a).unwrap();
    let sealed_b = m_b.seal(&k, &b).unwrap();
    // Swapped between two files of the same folder: neither opens under the other name.
    assert_integrity(FileManifest::open(&k, &b, &sealed_a));
    assert_integrity(FileManifest::open(&k, &a, &sealed_b));
    // Moved to another folder under the same name.
    assert_integrity(FileManifest::open(&k, &at(2, "a.txt"), &sealed_a));
    assert_eq!(m_a, FileManifest::open(&k, &a, &sealed_a).unwrap());
    // The placement from the listed (encrypted) name equals the one from the plaintext name.
    let enc = names::encrypt_name(&k, &DirId([1; 16]), "a.txt").unwrap();
    assert_eq!(Some(a.clone()), Placement::from_encrypted(&DirId([1; 16]), &enc));
    assert_eq!(None, Placement::from_encrypted(&DirId([1; 16]), "x.txt"));
}

#[test]
fn version_counter_and_rollback_check() {
    let k = keys();
    let v1 = wasl_crypto::v2::encrypt_stream(
        &k,
        wasl_crypto::v2::Format::V2_0,
        FileId([1; 16]),
        0,
        &b"one"[..],
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(1, v1.version);
    let v2 =
        wasl_crypto::v2::encrypt_update(&k, wasl_crypto::v2::Format::V2_0, &v1, 0, &b"two"[..], |_| Ok(()))
            .unwrap();
    assert_eq!((2, v1.file_id), (v2.version, v2.file_id));
    let v3 =
        wasl_crypto::v2::encrypt_update(&k, wasl_crypto::v2::Format::V2_0, &v2, 0, &b"three"[..], |_| {
            Ok(())
        })
        .unwrap();
    assert_eq!(3, v3.version);
    // A round trip keeps the version; an older genuine manifest put back is caught against the
    // highest version seen.
    let here = at(1, "f");
    let back = FileManifest::open(&k, &here, &v2.seal(&k, &here).unwrap()).unwrap();
    assert_eq!(2, back.version);
    assert_integrity(back.check_not_older_than(3));
    assert!(back.check_not_older_than(2).is_ok());
    assert!(v3.check_not_older_than(2).is_ok());
    // Version 0 is not valid.
    let zero = FileManifest { version: 0, ..v1 };
    assert_integrity(FileManifest::from_cbor(&zero.to_cbor()));
}

#[test]
fn meta_blob_fits_drive_and_verifies() {
    let k = keys();
    let meta = sample_manifest().meta();
    let here = at(1, "a.txt");
    let value = meta.encode(&k, &here).unwrap();
    assert!(META_PROPERTY.len() + value.len() <= 124, "{}", value.len());
    assert_eq!(104, value.len());
    assert_eq!(meta, MetaBlob::decode(&k, &here, &value).unwrap());
    assert_integrity(MetaBlob::decode(&other_keys(), &here, &value));
    use base64::Engine as _;
    let raw = base64::engine::general_purpose::STANDARD.decode(&value).unwrap();
    for bit in 0..raw.len() * 8 {
        let mut r = raw.clone();
        r[bit / 8] ^= 1 << (bit % 8);
        let v = base64::engine::general_purpose::STANDARD.encode(&r);
        assert_integrity(MetaBlob::decode(&k, &here, &v));
    }
    assert_integrity(MetaBlob::decode(&k, &here, &value[..100]));
    assert_integrity(MetaBlob::decode(&k, &here, &"A".repeat(200)));
    assert_integrity(MetaBlob::decode(&k, &here, "!!!!"));
    // Copied to another file's appProperties: refused.
    assert_integrity(MetaBlob::decode(&k, &at(1, "b.txt"), &value));
    assert_integrity(MetaBlob::decode(&k, &at(2, "a.txt"), &value));
}

// ---------------------------------------------------------------- chunks

#[test]
fn chunk_ids_are_keyed_per_file_and_named_in_shards() {
    let k = keys();
    let (f1, f2) = (FileId([1; 16]), FileId([2; 16]));
    let a = ChunkId::compute(&k, &f1, b"same content");
    assert_eq!(a, ChunkId::compute(&k, &f1, b"same content"));
    assert_ne!(
        a,
        ChunkId::compute(&k, &f2, b"same content"),
        "no sharing between files"
    );
    assert_ne!(a, ChunkId::compute(&other_keys(), &f1, b"same content"));
    assert_eq!(
        a,
        ChunkId::from_hash(&k, &f1, blake3::hash(b"same content").as_bytes())
    );
    let name = a.object_name();
    assert_eq!(43, name.len());
    assert_eq!(Some(a), ChunkId::from_object_name(&name));
    assert_eq!(format!("{:02x}", a.0[0]), a.shard());
    assert_eq!(format!(".wasl-chunks/{}/{}", a.shard(), name), a.object_path());
    assert_eq!(None, ChunkId::from_object_name("short"));
}

#[test]
fn chunk_seal_open_swap_reorder_truncate() {
    let k = keys();
    let (f1, f2) = (FileId([1; 16]), FileId([2; 16]));
    let p0 = random_bytes(300, 1);
    let p1 = random_bytes(300, 2);
    let id0 = ChunkId::compute(&k, &f1, &p0);
    let id1 = ChunkId::compute(&k, &f1, &p1);
    let s0 = seal_chunk(&k, wasl_crypto::v2::Format::V2_0, &f1, &id0, &p0).unwrap();
    let s1 = seal_chunk(&k, wasl_crypto::v2::Format::V2_0, &f1, &id1, &p1).unwrap();
    assert_eq!(OVERHEAD + 300, s0.len());
    assert_eq!(p0, open_chunk(&k, &f1, &id0, Some(300), &s0).unwrap());
    assert_eq!(p0, open_chunk(&k, &f1, &id0, None, &s0).unwrap());
    // Wrong length in the manifest.
    assert_integrity(open_chunk(&k, &f1, &id0, Some(299), &s0));
    // Reordered: chunk 1's object where chunk 0 belongs.
    assert_integrity(open_chunk(&k, &f1, &id0, Some(300), &s1));
    // Swapped between files: the same plaintext sealed for file 2 is not file 1's chunk.
    let id0_f2 = ChunkId::compute(&k, &f2, &p0);
    let s0_f2 = seal_chunk(&k, wasl_crypto::v2::Format::V2_0, &f2, &id0_f2, &p0).unwrap();
    assert_integrity(open_chunk(&k, &f1, &id0, Some(300), &s0_f2));
    assert_integrity(open_chunk(&k, &f1, &id0_f2, Some(300), &s0_f2));
    // A key holder sealing other content under this id is caught by the id check.
    let forged = seal_chunk(&k, wasl_crypto::v2::Format::V2_0, &f1, &id0, &p1).unwrap();
    assert_integrity(open_chunk(&k, &f1, &id0, Some(300), &forged));
    // Every bit flip, every truncation, an extension.
    for bit in 0..s0.len() * 8 {
        let mut s = s0.clone();
        s[bit / 8] ^= 1 << (bit % 8);
        assert_integrity(open_chunk(&k, &f1, &id0, Some(300), &s));
    }
    for cut in 0..s0.len() {
        assert_integrity(open_chunk(&k, &f1, &id0, None, &s0[..cut]));
    }
    let mut longer = s0.clone();
    longer.push(0);
    assert_integrity(open_chunk(&k, &f1, &id0, None, &longer));
    assert_integrity(open_chunk(&other_keys(), &f1, &id0, None, &s0));
}

#[test]
fn cut_points_equal_wasl_chunk_with_the_v2_sizes() {
    let params = wasl_chunk::ChunkParams::new(256 << 10, 1 << 20, 4 << 20).unwrap();
    for (len, seed) in [
        (0usize, 1u64),
        (1, 2),
        (300_000, 3),
        (4 << 20, 4),
        ((4 << 20) + 1, 5),
        (13 << 20, 6),
        (21 << 20 | 12345, 7),
    ] {
        let data = random_bytes(len, seed);
        let theirs: Vec<u32> = wasl_chunk::chunk_reader(&data[..], &params)
            .unwrap()
            .chunks
            .iter()
            .map(|c| c.len)
            .collect();
        let mut ours = Vec::new();
        let (size, hash) = chunks::cut(&data[..], |c| {
            ours.push(c.len() as u32);
            Ok(())
        })
        .unwrap();
        assert_eq!(theirs, ours, "len {len}");
        assert_eq!(len as u64, size);
        assert_eq!(*blake3::hash(&data).as_bytes(), hash);
        assert!(ours.iter().all(|&l| l as usize <= MAX_CHUNK_BYTES));
    }
    // Zeros cut at the maximum.
    let mut zeros = Vec::new();
    chunks::cut(&vec![0u8; 10 << 20][..], |c| {
        zeros.push(c.len());
        Ok(())
    })
    .unwrap();
    assert_eq!(vec![4 << 20, 4 << 20, 2 << 20], zeros);
}

// ---------------------------------------------------------------- properties

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn prop_names_round_trip(name in "[^/\\x00]{1,60}", dir in any::<[u8; 16]>()) {
        prop_assume!(name != "." && name != "..");
        let k = keys();
        let d = DirId(dir);
        let enc = names::encrypt_name(&k, &d, &name).unwrap();
        prop_assert_eq!(Some(name), names::decrypt_name(&k, &d, &enc));
    }

    #[test]
    fn prop_manifest_round_trip(
        lens in prop::collection::vec(1u32..=(4 << 20), 0..40),
        mtime in any::<i64>(),
        fid in any::<[u8; 16]>(),
        hash in any::<[u8; 32]>(),
        flip in any::<prop::sample::Index>(),
    ) {
        let k = keys();
        let file_id = FileId(fid);
        let chunks: Vec<ChunkRef> = lens.iter().enumerate()
            .map(|(i, &len)| ChunkRef { id: ChunkId::compute(&k, &file_id, &i.to_be_bytes()), len })
            .collect();
        let m = FileManifest { file_id, size: lens.iter().map(|&l| u64::from(l)).sum(), mtime_ms: mtime, blake3: hash, chunks, version: 1 };
        let here = at(fid[0], "f");
        let sealed = m.seal(&k, &here).unwrap();
        prop_assert_eq!(&m, &FileManifest::open(&k, &here, &sealed).unwrap());
        let mut bad = sealed.clone();
        let bit = flip.index(sealed.len() * 8);
        bad[bit / 8] ^= 1 << (bit % 8);
        prop_assert!(FileManifest::open(&k, &here, &bad).is_err());
        let meta = m.meta();
        prop_assert_eq!(meta, MetaBlob::decode(&k, &here, &meta.encode(&k, &here).unwrap()).unwrap());
    }

    #[test]
    fn prop_chunk_round_trip_and_flip(plain in prop::collection::vec(any::<u8>(), 1..2000), flip in any::<prop::sample::Index>()) {
        let k = keys();
        let fid = FileId([9; 16]);
        let id = ChunkId::compute(&k, &fid, &plain);
        let sealed = seal_chunk(&k, wasl_crypto::v2::Format::V2_0, &fid, &id, &plain).unwrap();
        prop_assert_eq!(&plain, &open_chunk(&k, &fid, &id, Some(plain.len() as u32), &sealed).unwrap());
        let mut bad = sealed.clone();
        let bit = flip.index(sealed.len() * 8);
        bad[bit / 8] ^= 1 << (bit % 8);
        prop_assert!(open_chunk(&k, &fid, &id, None, &bad).is_err());
    }

}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(12))]

    #[test]
    fn prop_stream_round_trip(len in 0usize..(9 << 20), seed in any::<u64>()) {
        let k = keys();
        let data = random_bytes(len, seed);
        let mut store = std::collections::HashMap::new();
        let m = wasl_crypto::v2::encrypt_stream(&k, wasl_crypto::v2::Format::V2_0, FileId([3; 16]), 0, &data[..], |c| {
            store.insert(*c.id(), c.seal()?);
            Ok(())
        }).unwrap();
        prop_assert_eq!(m.size, len as u64);
        let mut out = Vec::new();
        wasl_crypto::v2::decrypt_file(&k, &m, |id| Ok(store[id].clone()), &mut out).unwrap();
        prop_assert_eq!(&data, &out);
    }
}
