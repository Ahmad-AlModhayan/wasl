//! Vault v2 known-answer vectors, shared with `tools/decrypt_vault.py` (`check_vectors`).
//!
//! `tests/vectors/v2.json` is committed. This test recomputes every value from the fixed inputs
//! and compares; set `WASL_WRITE_VECTORS=1` to (re)write the file after a deliberate format
//! change. Python checks the same file independently (see `v2_python_cross_check.rs`).

mod common;

use common::{hex, random_bytes};
use serde_json::{Value, json};
use std::path::PathBuf;
use wasl_crypto::v2::chunks::{self, ChunkId, seal_chunk_with_nonce};
use wasl_crypto::v2::keys::{self, KdfParams, VaultKeysV2};
use wasl_crypto::v2::manifest::{ChunkRef, FileId, FileManifest, MetaBlob};
use wasl_crypto::v2::names::{self, DirId, Placement};

pub const PASSPHRASE: &str = "correct horse battery staple ✓";

fn path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/v2.json")
}

fn compute() -> Value {
    let salt: Vec<u8> = (0u8..16).collect();
    let params = KdfParams::MIN;
    let ikm = keys::argon2id(PASSPHRASE, &salt, params).unwrap();
    let k = VaultKeysV2::from_ikm(&ikm);

    let root = DirId::ROOT;
    let sub = DirId([0x11; 16]);
    let mut name_items = Vec::new();
    for (dir, name) in [
        (root, "hello.txt"),
        (root, "Docs"),
        (sub, "hello.txt"),
        (sub, "ملف مهم.txt"),
        (sub, "写真 📷.jpg"),
        (sub, "a"),
    ] {
        name_items.push(json!({
            "dir_id": hex(dir.as_bytes()),
            "name": name,
            "encrypted": names::encrypt_name(&k, &dir, name).unwrap(),
        }));
    }
    let hello_root = names::encrypt_name(&k, &root, "hello.txt").unwrap();
    let body = hello_root.strip_suffix(".w2").unwrap();
    let mut flipped = body.as_bytes().to_vec();
    flipped[5] = if flipped[5] == b'A' { b'B' } else { b'A' };
    let bad_names = vec![
        json!({"dir_id": hex(sub.as_bytes()), "encrypted": hello_root, "why": "another folder"}),
        json!({"dir_id": hex(root.as_bytes()), "encrypted": format!("{body}=.w2"), "why": "padding"}),
        json!({"dir_id": hex(root.as_bytes()), "encrypted": format!("{}.w2", String::from_utf8(flipped).unwrap()), "why": "tampered"}),
        json!({"dir_id": hex(root.as_bytes()), "encrypted": body, "why": "no suffix"}),
        json!({"dir_id": hex(root.as_bytes()), "encrypted": "AAAAAAAAAAAAAAAAAAAAAAA.w2", "why": "garbage"}),
    ];

    // A folder "Docs" at the root with dir id 0x22…: its marker and wasl.dir property.
    let docs = Placement::for_name(&k, &root, "Docs").unwrap();
    let docs_id = DirId([0x22; 16]);
    let dir = json!({
        "parent": hex(root.as_bytes()),
        "name": "Docs",
        "dir_id": hex(docs_id.as_bytes()),
        "marker": hex(&names::dir_marker(&k, &docs, &docs_id)),
        "property": docs_id.to_property(&k, &docs),
    });

    let file_id = FileId(core::array::from_fn(|i| 0xa0 + i as u8));
    let plains = [b"hello world".to_vec(), random_bytes(300, 5)];
    let mut chunk_items = Vec::new();
    let mut refs = Vec::new();
    for (i, plain) in plains.iter().enumerate() {
        let id = ChunkId::compute(&k, &file_id, plain);
        let nonce = [0x30 + i as u8; 12];
        refs.push(ChunkRef {
            id,
            len: plain.len() as u32,
        });
        chunk_items.push(json!({
            "file_id": hex(file_id.as_bytes()),
            "plain": hex(plain),
            "blake3": hex(blake3::hash(plain).as_bytes()),
            "chunk_id": hex(id.as_bytes()),
            "object_path": id.object_path(),
            "nonce": hex(&nonce),
            "sealed": hex(&seal_chunk_with_nonce(&k, &file_id, &id, plain, &nonce)),
        }));
    }
    let whole: Vec<u8> = plains.concat();
    let m = FileManifest {
        file_id,
        size: whole.len() as u64,
        mtime_ms: 1_760_000_000_123,
        blake3: *blake3::hash(&whole).as_bytes(),
        chunks: refs.clone(),
        version: 7,
    };
    let at = Placement::for_name(&k, &sub, "hello.txt").unwrap();
    let nonce = [0x40; 12];
    let manifest = json!({
        "parent": hex(sub.as_bytes()),
        "name": "hello.txt",
        "file_id": hex(file_id.as_bytes()),
        "size": m.size,
        "mtime": m.mtime_ms,
        "blake3": hex(&m.blake3),
        "version": m.version,
        "chunks": refs.iter().map(|c| json!([hex(c.id.as_bytes()), c.len])).collect::<Vec<_>>(),
        "cbor": hex(&m.to_cbor()),
        "nonce": hex(&nonce),
        "sealed": hex(&m.seal_with_nonce(&k, &at, &nonce)),
    });
    let meta_nonce = [0x50; 12];
    let meta = json!({
        "parent": hex(sub.as_bytes()),
        "name": "hello.txt",
        "size": m.size,
        "mtime": -5i64,
        "blake3": hex(&m.blake3),
        "nonce": hex(&meta_nonce),
        "value": MetaBlob { size: m.size, mtime_ms: -5, blake3: m.blake3 }.encode_with_nonce(&k, &at, &meta_nonce),
    });

    // Rust-only: FastCDC cut points for seeded data (a chunker change must be deliberate).
    let data = random_bytes(13 << 20, 2026);
    let mut cuts = Vec::new();
    chunks::cut(&data[..], |c| {
        cuts.push(c.len());
        Ok(())
    })
    .unwrap();

    json!({
        "comment": "Wasl vault v2 known-answer vectors (docs/ENCRYPTION.md, Format v2). Hex unless noted; \
                    `encrypted`, `property`, `value` and `object_path` are text. Generated by \
                    engine/crates/wasl-crypto/tests/v2_vectors.rs with WASL_WRITE_VECTORS=1.",
        "kdf": {
            "passphrase": PASSPHRASE,
            "salt": hex(&salt),
            "m": params.m_kib,
            "t": params.t,
            "p": params.p,
            "ikm": hex(&ikm[..]),
            "k_name": hex(k.k_name()),
            "k_meta": hex(k.k_meta()),
            "k_chunk": hex(k.k_chunk()),
            "k_chunk_id": hex(k.k_chunk_id()),
            "k_dir": hex(k.k_dir()),
            "key_check": hex(&k.key_check()),
        },
        "names": name_items,
        "bad_names": bad_names,
        "dir": dir,
        "manifest": manifest,
        "meta": meta,
        "chunks": chunk_items,
        "cdc": {"generator": "splitmix64", "seed": 2026, "len": data.len(), "cuts": cuts},
    })
}

#[test]
fn vectors_match_the_committed_file() {
    let computed = compute();
    let p = path();
    if std::env::var_os("WASL_WRITE_VECTORS").is_some() || !p.exists() {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, serde_json::to_string_pretty(&computed).unwrap() + "\n").unwrap();
        println!("note: wrote {}", p.display());
    }
    let committed: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    assert_eq!(
        committed, computed,
        "vectors changed: the v2 format is not what tests/vectors/v2.json says"
    );
}

/// The committed vectors also decrypt through the reading API (not only re-encrypt the same).
#[test]
fn vectors_open() {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path()).unwrap()).unwrap();
    let unhex = |s: &Value| common::unhex(s.as_str().unwrap());
    let kdf = &v["kdf"];
    let k = VaultKeysV2::from_ikm(&unhex(&kdf["ikm"]).try_into().unwrap());
    for n in v["names"].as_array().unwrap() {
        let dir = DirId(unhex(&n["dir_id"]).try_into().unwrap());
        assert_eq!(
            n["name"].as_str(),
            names::decrypt_name(&k, &dir, n["encrypted"].as_str().unwrap()).as_deref()
        );
    }
    for n in v["bad_names"].as_array().unwrap() {
        let dir = DirId(unhex(&n["dir_id"]).try_into().unwrap());
        assert_eq!(
            None,
            names::decrypt_name(&k, &dir, n["encrypted"].as_str().unwrap()),
            "{n}"
        );
    }
    let m = &v["manifest"];
    let at = Placement::for_name(
        &k,
        &DirId(unhex(&m["parent"]).try_into().unwrap()),
        m["name"].as_str().unwrap(),
    )
    .unwrap();
    let opened = FileManifest::open(&k, &at, &unhex(&m["sealed"])).unwrap();
    assert_eq!(m["version"].as_u64(), Some(opened.version));
    let meta = MetaBlob::decode(&k, &at, v["meta"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(-5, meta.mtime_ms);
    for c in v["chunks"].as_array().unwrap() {
        let fid = FileId(unhex(&c["file_id"]).try_into().unwrap());
        let id = ChunkId(unhex(&c["chunk_id"]).try_into().unwrap());
        let plain = chunks::open_chunk(&k, &fid, &id, None, &unhex(&c["sealed"])).unwrap();
        assert_eq!(unhex(&c["plain"]), plain);
    }
}
