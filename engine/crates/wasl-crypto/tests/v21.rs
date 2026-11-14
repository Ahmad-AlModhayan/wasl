//! Vault format 2.1 (ENCRYPTION.md §11.10–11.11): keyed chunking, padded chunks, state records.
//!
//! Known-answer vectors live in `tests/vectors/v21.json` (committed): this test recomputes every
//! value from fixed inputs and compares; set `WASL_WRITE_VECTORS=1` to rewrite the file after a
//! deliberate format change. Values that can be worked out by hand (padmé, the gear table as a
//! plain HKDF output) are also checked against independent computations here.

mod common;

use common::{hex, random_bytes};
use serde_json::{Value, json};
use std::path::PathBuf;
use wasl_crypto::CryptoError;
use wasl_crypto::v2::chunks::{
    self, ChunkId, GEAR_INFO, Gear, MAGIC_PADDED, MAX_CHUNK_BYTES, MAX_SEALED_BYTES, OVERHEAD, open_chunk,
    padded_len, padme, seal_chunk, seal_chunk_with_nonce, seal_padded_chunk_with_nonce,
};
use wasl_crypto::v2::keys::{KdfParams, VaultKeysV2};
use wasl_crypto::v2::manifest::FileId;
use wasl_crypto::v2::state::{RECORD_BYTES, StateRecord, WriterId, set_digest};
use wasl_crypto::v2::{Format, VaultManifestV2, decrypt_file, encrypt_stream, encrypt_update, plan_update};

const IKM: [u8; 32] = [0x42; 32];

fn keys() -> VaultKeysV2 {
    VaultKeysV2::from_ikm(&IKM)
}

fn path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/v21.json")
}

fn cuts(format: Format, data: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    chunks::cut_as(&keys(), format, data, |c| {
        out.push(c.len());
        Ok(())
    })
    .unwrap();
    out
}

fn compute() -> Value {
    let k = keys();
    let data = random_bytes(12 << 20, 2026);
    let fid = FileId([0x07; 16]);
    let plain = b"wasl 2.1".to_vec();
    let id = ChunkId::compute(&k, &fid, &plain);
    let sealed = seal_padded_chunk_with_nonce(&k, &fid, &id, &plain, &[0x24; 12]);
    let record = StateRecord {
        writer: WriterId([1, 2, 3, 4, 5, 6, 7, 8]),
        counter: 42,
        time_ms: 1_760_000_000_000,
        digest: set_digest([(&b"placement-a"[..], "blob-a"), (&b"placement-b"[..], "")]),
    };
    let padded: Vec<Value> = [0usize, 1, 4092, 4093, 100_000, 262_144, 1 << 20, MAX_CHUNK_BYTES]
        .iter()
        .map(|&n| json!([n, padded_len(n)]))
        .collect();
    let gear: Vec<String> = Gear::keyed(&k)
        .head(4)
        .iter()
        .map(|v| format!("{v:016x}"))
        .collect();
    json!({
        "comment": "Vault format 2.1 known answers. Regenerate only for a deliberate format change: \
                    WASL_WRITE_VECTORS=1 cargo test -p wasl-crypto --test v21.",
        "ikm": hex(&IKM),
        "kCdc": hex(k.k_cdc()),
        "kState": hex(k.k_state()),
        "gear_head": gear,
        "padded_len": padded,
        "cdc": {"generator": "splitmix64", "seed": 2026, "len": data.len(), "cuts": cuts(Format::V2_1, &data)},
        "chunk": {"file_id": hex(&fid.0), "plain": hex(&plain), "chunk_id": hex(id.as_bytes()),
                  "nonce": "242424242424242424242424", "sealed_len": sealed.len(),
                  "sealed_sha256": wasl_crypto::sha256_hex(&sealed)},
        "state": {"record": hex(&record.seal(k.k_state())), "digest": hex(&record.digest)},
    })
}

#[test]
fn vectors_match_the_committed_file() {
    let fresh = compute();
    let p = path();
    if std::env::var_os("WASL_WRITE_VECTORS").is_some() || !p.exists() {
        std::fs::write(&p, serde_json::to_string_pretty(&fresh).unwrap() + "\n").unwrap();
    }
    let committed: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    assert_eq!(committed, fresh, "format 2.1 changed: {}", p.display());
}

#[test]
fn padme_matches_hand_computed_values() {
    // padmé(L): E = floor(log2 L), S = floor(log2 E) + 1, keep all but the low E - S bits.
    assert_eq!(4096, padme(4096)); // E 12, S 4: 256-byte steps; already a multiple
    assert_eq!(4352, padme(4100));
    assert_eq!(1_081_344, padme(1_048_580)); // E 20, S 5: 32 KiB steps
    assert_eq!(4_325_376, padme(4_194_308)); // E 22, S 5: 128 KiB steps
    assert_eq!(10, padme(9)); // E 3, S 2: 2-byte steps
    // A chunk body is the 4-byte length, the plaintext and zeros, at least 4 KiB.
    assert_eq!(4096, padded_len(0));
    assert_eq!(4096, padded_len(4092));
    assert_eq!(4352, padded_len(4093)); // 4097 rounds up to the next 256
    assert_eq!(4_325_376, padded_len(MAX_CHUNK_BYTES));
    assert_eq!(OVERHEAD + 4_325_376, MAX_SEALED_BYTES);
    // Never more than 1/8 extra for chunk-sized inputs (the bound is 11.1 % in general, under
    // 3.2 % from 64 KiB up).
    for n in [65_536usize, 262_144, 300_001, 1 << 20, 3_000_000, MAX_CHUNK_BYTES] {
        let extra = padded_len(n) - n;
        assert!(extra * 1000 <= n * 32, "{n}: {extra}");
    }
}

#[test]
fn the_gear_table_is_a_plain_hkdf_output_of_kcdc() {
    let k = keys();
    // The sub-keys are plain HKDF outputs of the Argon2id result, as every other v2 key.
    for (info, key) in [("wasl-v2-cdc", k.k_cdc()), ("wasl-v2-state", k.k_state())] {
        let mut expect = [0u8; 32];
        hkdf::Hkdf::<sha2::Sha256>::new(None, &IKM)
            .expand(info.as_bytes(), &mut expect)
            .unwrap();
        assert_eq!(&expect, key, "{info}");
    }
    let mut bytes = [0u8; 2048];
    hkdf::Hkdf::<sha2::Sha256>::new(None, k.k_cdc())
        .expand(GEAR_INFO.as_bytes(), &mut bytes)
        .unwrap();
    let expect: Vec<u64> = bytes[..32]
        .chunks(8)
        .map(|c| u64::from_be_bytes(c.try_into().unwrap()))
        .collect();
    assert_eq!(expect, Gear::keyed(&k).head(4));
    // Another vault, another table.
    assert_ne!(
        Gear::keyed(&k).head(4),
        Gear::keyed(&VaultKeysV2::from_ikm(&[0x43; 32])).head(4)
    );
    // The sub-keys are independent of each other.
    assert_ne!(k.k_cdc(), k.k_state());
    assert_ne!(k.k_cdc(), k.k_chunk_id());
}

#[test]
fn keyed_cuts_differ_from_public_ones_and_between_vaults_but_stay_in_bounds() {
    let data = random_bytes(16 << 20, 7);
    let public = cuts(Format::V2_0, &data);
    let keyed = cuts(Format::V2_1, &data);
    let mut other = Vec::new();
    chunks::cut_as(
        &VaultKeysV2::from_ikm(&[0x43; 32]),
        Format::V2_1,
        &data[..],
        |c| {
            other.push(c.len());
            Ok(())
        },
    )
    .unwrap();
    assert_ne!(public, keyed);
    assert_ne!(keyed, other);
    for list in [&keyed, &other] {
        assert_eq!(data.len(), list.iter().sum::<usize>());
        let (last, body) = list.split_last().unwrap();
        assert!(*last <= MAX_CHUNK_BYTES);
        for &l in body {
            assert!((256 << 10..=MAX_CHUNK_BYTES).contains(&l), "{l}");
        }
    }
    // 2.0 is unchanged: the public table.
    let mut plain_cut = Vec::new();
    chunks::cut(&data[..], |c| {
        plain_cut.push(c.len());
        Ok(())
    })
    .unwrap();
    assert_eq!(public, plain_cut);
}

#[test]
fn a_2_1_file_round_trips_with_padded_chunks_and_edits_still_reuse_chunks() {
    let k = keys();
    let data = random_bytes(9 << 20, 11);
    let mut objects = std::collections::HashMap::new();
    let m = encrypt_stream(&k, Format::V2_1, FileId([5; 16]), 123, &data[..], |c| {
        let sealed = c.seal()?;
        assert_eq!(MAGIC_PADDED, sealed[..4]);
        assert_eq!(OVERHEAD + padded_len(c.len() as usize), sealed.len());
        objects.insert(*c.id(), sealed);
        Ok(())
    })
    .unwrap();
    assert!(m.chunks.len() >= 3);
    let mut out = Vec::new();
    decrypt_file(&k, &m, |id| Ok(objects[id].clone()), &mut out).unwrap();
    assert_eq!(data, out);

    // A small insertion early on changes one or two chunks, not the rest: keyed cut points are
    // still content-defined.
    let mut edited = data.clone();
    edited.splice(300_000..300_000, b"inserted".iter().copied());
    let m2 = encrypt_update(&k, Format::V2_1, &m, 124, &edited[..], |_| Ok(())).unwrap();
    let plan = plan_update(&m, &m2);
    assert!(plan.upload.len() <= 2, "{} chunks changed", plan.upload.len());
}

#[test]
fn padded_chunks_refuse_any_other_padding_and_framing_confusion() {
    use aes_gcm::aead::{AeadInPlace, KeyInit};
    let k = keys();
    let fid = FileId([9; 16]);
    let plain = b"abc".to_vec();
    let id = ChunkId::compute(&k, &fid, &plain);
    let good = seal_chunk(&k, Format::V2_1, &fid, &id, &plain).unwrap();
    assert_eq!(plain, open_chunk(&k, &fid, &id, Some(3), &good).unwrap());

    // A body sealed with the right key but non-zero padding, or the wrong length, is refused.
    let forge = |body: &[u8]| {
        let nonce = [1u8; 12];
        let mut buf = body.to_vec();
        let mut aad = b"wasl-v2.1-chunk".to_vec();
        aad.extend_from_slice(&fid.0);
        aad.extend_from_slice(id.as_bytes());
        let tag = aes_gcm::Aes256Gcm::new(k.k_chunk().into())
            .encrypt_in_place_detached(aes_gcm::Nonce::from_slice(&nonce), &aad, &mut buf)
            .unwrap();
        let mut out = MAGIC_PADDED.to_vec();
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&buf);
        out.extend_from_slice(&tag);
        out
    };
    let mut body = vec![0u8; 4096];
    body[..4].copy_from_slice(&3u32.to_be_bytes());
    body[4..7].copy_from_slice(&plain);
    assert_eq!(plain, open_chunk(&k, &fid, &id, Some(3), &forge(&body)).unwrap());
    let mut dirty = body.clone();
    dirty[4095] = 1;
    assert!(matches!(
        open_chunk(&k, &fid, &id, Some(3), &forge(&dirty)),
        Err(CryptoError::Integrity(_))
    ));
    let mut long = body.clone();
    long.extend_from_slice(&[0u8; 256]);
    assert!(matches!(
        open_chunk(&k, &fid, &id, Some(3), &forge(&long)),
        Err(CryptoError::Integrity(_))
    ));

    // A 2.0 chunk relabelled as 2.1 (or the reverse) does not verify: the AAD labels differ.
    let v20 = seal_chunk_with_nonce(&k, &fid, &id, &plain, &[2; 12]);
    let mut relabelled = v20.clone();
    relabelled[..4].copy_from_slice(&MAGIC_PADDED);
    assert!(open_chunk(&k, &fid, &id, Some(3), &relabelled).is_err());
    let mut back = good.clone();
    back[..4].copy_from_slice(b"W2C1");
    assert!(open_chunk(&k, &fid, &id, Some(3), &back).is_err());
    // And 2.0 chunks still open (a 2.1 reader reads 2.0 vaults).
    assert_eq!(plain, open_chunk(&k, &fid, &id, Some(3), &v20).unwrap());
}

#[test]
fn vault_json_records_the_revision_and_old_readers_refuse_2_1() {
    let k = keys();
    let new = VaultManifestV2::create("V", &[1; 16], KdfParams::MIN, &k.key_check(), 0);
    assert_eq!(Format::V2_1, new.format());
    assert_eq!(3, new.version);
    let old = VaultManifestV2::create_as(Format::V2_0, "V", &[1; 16], KdfParams::MIN, &k.key_check(), 0);
    assert_eq!(
        Format::V2_0,
        VaultManifestV2::parse(&old.to_json()).unwrap().format()
    );
    assert_eq!(
        Format::V2_1,
        VaultManifestV2::parse(&new.to_json()).unwrap().format()
    );
    // A 2.1 vault.json edited back to 2.0 is not the same vault.
    assert!(!old.same_vault_as(&new));
    // What every reader before 2.1 does (it accepted exactly version 2): it refuses version 3
    // with "vault format version 3 is not supported here", instead of writing into it.
    let v: Value = serde_json::from_str(&new.to_json()).unwrap();
    assert_ne!(Some(2), v["version"].as_i64());
}

#[test]
fn state_records_verify_only_under_their_own_name_and_key() {
    let k = keys();
    let r = StateRecord {
        writer: WriterId::from_u64(0x0102_0304_0506_0708),
        counter: 7,
        time_ms: -5,
        digest: [3; 32],
    };
    let bytes = r.seal(k.k_state());
    assert_eq!(RECORD_BYTES, bytes.len());
    let name = r.writer.object_name();
    assert_eq!(".wasl-state.0102030405060708", name);
    assert_eq!(r, StateRecord::open(k.k_state(), &name, &bytes).unwrap());
    // Another writer's name, another vault's key, a flipped bit, a short object: refused.
    let other = WriterId::from_u64(9).object_name();
    assert!(StateRecord::open(k.k_state(), &other, &bytes).is_err());
    let k2 = VaultKeysV2::from_ikm(&[0x43; 32]);
    assert!(StateRecord::open(k2.k_state(), &name, &bytes).is_err());
    for i in 0..bytes.len() {
        let mut b = bytes.clone();
        b[i] ^= 1;
        assert!(StateRecord::open(k.k_state(), &name, &b).is_err(), "byte {i}");
    }
    assert!(StateRecord::open(k.k_state(), &name, &bytes[..bytes.len() - 1]).is_err());
    assert_eq!(None, WriterId::from_object_name(".wasl-state.0102030405060708x"));
    assert_eq!(None, WriterId::from_object_name(".wasl-state.010203040506070G"));

    // The set digest ignores order and tells sets apart, also by a blob or a split.
    let a = set_digest([(&b"p1"[..], "x"), (&b"p2"[..], "y")]);
    assert_eq!(a, set_digest([(&b"p2"[..], "y"), (&b"p1"[..], "x")]));
    assert_ne!(a, set_digest([(&b"p1"[..], "x")]));
    assert_ne!(a, set_digest([(&b"p1"[..], "x"), (&b"p2"[..], "z")]));
    assert_ne!(set_digest([(&b"p"[..], "1x")]), set_digest([(&b"p1"[..], "x")]));
}
