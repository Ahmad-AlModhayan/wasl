//! Vault v2 interoperability with the independent Python implementation, `tools/decrypt_vault.py`.
//!
//! * Python checks the shared known-answer vectors (`tests/vectors/v2.json`).
//! * Rust → Python: a v2 vault folder written by Rust (nested folders, Arabic/CJK/emoji names, an
//!   empty file, an empty folder, multi-chunk and repeated-chunk files) is decrypted by the tool
//!   byte for byte.
//! * Python → Rust: the tool derives keys and encrypts names, a folder marker, a manifest, a
//!   `wasl.m` blob and a chunk; Rust reads all of them.
//!
//! Every test passes with a printed note when `python` with `cryptography` (AES-SIV) is missing.

mod common;

use base64::Engine as _;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use wasl_crypto::v2::chunks::{ChunkId, open_chunk};
use wasl_crypto::v2::keys::{self, KdfParams};
use wasl_crypto::v2::manifest::{FileId, FileManifest, MetaBlob};
use wasl_crypto::v2::names::{self, DirId, Placement};
use wasl_crypto::v2::{VaultManifestV2, layout};

const PASSPHRASE: &str = "correct horse battery staple ✓";

fn tools_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tools")
}

fn python_ready() -> bool {
    let ok = Command::new("python")
        .args([
            "-I",
            "-c",
            "from cryptography.hazmat.primitives.ciphers.aead import AESSIV",
        ])
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        println!("note: skipped, `python` with a `cryptography` that has AES-SIV is not available");
    }
    ok
}

fn python(script: &str, args: &[&str]) -> String {
    let run = Command::new("python")
        .arg("-I")
        .arg("-c")
        .arg(script)
        .arg(tools_dir())
        .args(args)
        .env("PYTHONIOENCODING", "utf-8")
        .output()
        .unwrap();
    let stdout = String::from_utf8(run.stdout).unwrap();
    assert!(
        run.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    stdout
}

fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if e.file_type().unwrap().is_dir() {
                out.insert(format!("{rel}/"), Vec::new());
                walk(&e.path(), &rel, out);
            } else {
                out.insert(rel, std::fs::read(e.path()).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, "", &mut out);
    out
}

#[test]
fn python_checks_the_shared_vectors() {
    if !python_ready() {
        return;
    }
    let vectors = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/v2.json");
    let out = python(
        "import sys\nsys.path.insert(0, sys.argv[1])\nimport decrypt_vault as d\nprint('checks', d.check_vectors(sys.argv[2]))",
        &[vectors.to_str().unwrap()],
    );
    println!("{out}");
    assert!(out.contains("checks "), "{out}");
}

#[test]
fn python_tool_decrypts_a_rust_v2_vault() {
    if !python_ready() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("plain");
    common::v2::sample_tree(&src);
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    let salt = keys::new_salt().unwrap();
    let k = keys::derive(PASSPHRASE, &salt, KdfParams::MIN).unwrap();
    let vj = VaultManifestV2::create_as(
        wasl_crypto::v2::Format::V2_0, // what tools/decrypt_vault.py reads today
        "Cross-check v2",
        &salt,
        KdfParams::MIN,
        &k.key_check(),
        1_760_000_000_000,
    );
    std::fs::write(vault.join("vault.json"), vj.to_json()).unwrap();
    layout::write_tree(&k, &src, &vault).unwrap();
    std::fs::write(vault.join("dropped-by-someone.txt"), b"plain").unwrap();

    let out = tmp.path().join("out");
    let run = Command::new("python")
        .arg("-I")
        .arg(tools_dir().join("decrypt_vault.py"))
        .arg(&vault)
        .arg(&out)
        .args(["--passphrase", PASSPHRASE])
        .env("PYTHONIOENCODING", "utf-8")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "tool failed\n{stdout}\n{stderr}");
    assert!(stdout.contains("Format:     v2"), "{stdout}");
    assert!(stdout.contains("Passphrase: ok"), "{stdout}");
    assert!(stdout.contains("Skipped 1 item(s)"), "{stdout}");
    assert_eq!(tree(&src), tree(&out), "{stdout}");
    let t = |p: &Path| layout::system_time_ms(std::fs::metadata(p).unwrap().modified().unwrap());
    assert_eq!(t(&src.join("hello.txt")), t(&out.join("hello.txt")));

    let wrong = Command::new("python")
        .arg("-I")
        .arg(tools_dir().join("decrypt_vault.py"))
        .arg(&vault)
        .arg(tmp.path().join("out2"))
        .args(["--passphrase", "not it"])
        .output()
        .unwrap();
    assert_eq!(Some(2), wrong.status.code());
}

#[test]
fn rust_reads_what_the_python_tool_encrypts() {
    if !python_ready() {
        return;
    }
    let script = r#"
import base64, json, os, sys
sys.path.insert(0, sys.argv[1])
import decrypt_vault as d
salt = bytes(range(16))
keys = d.KeysV2(d.argon2id(sys.argv[2], salt, 19 * 1024, 1, 1))
out = {"key_check": base64.b64encode(keys.key_check).decode(), "names": []}
sub = bytes([0x11]) * 16
for dir_id, name in [(bytes(16), "hello.txt"), (sub, "ملف مهم.txt"), (sub, "写真 📷.jpg")]:
    out["names"].append([dir_id.hex(), name, d.encrypt_name_v2(keys, dir_id, name)])
docs_enc = d.encrypt_name_v2(keys, bytes(16), "Docs")
docs_at = d.placement(bytes(16), docs_enc)
docs_id = bytes([0x22]) * 16
out["dir"] = {"name": "Docs", "dir_id": docs_id.hex(), "marker": d.dir_marker(keys, docs_at, docs_id).hex(),
              "property": d.dir_property(keys, docs_at, docs_id)}
file_enc = d.encrypt_name_v2(keys, docs_id, "report.pdf")
at = d.placement(docs_id, file_enc)
file_id = os.urandom(16)
plain = "سلام, world".encode("utf-8") * 50
out["file"] = {"file_id": file_id.hex(), "plain": plain.hex()}
if d._blake3 is not None:
    digest = d._blake3.blake3(plain).digest()
    cid = d.chunk_id(keys, file_id, digest)
    chunks = [(cid, len(plain))]
    out["file"]["chunk_id"] = cid.hex()
    out["file"]["chunk"] = d.seal_chunk(keys, file_id, cid, plain, os.urandom(12)).hex()
else:
    digest = bytes(32)
    chunks = []
m = {"file_id": file_id, "size": sum(c[1] for c in chunks), "mtime": -1234, "blake3": digest,
     "chunks": chunks, "version": 5}
out["file"]["manifest"] = d.seal_manifest(keys, at, m, os.urandom(12)).hex()
out["file"]["meta"] = d.encode_meta(keys, at, 42, -1234, digest, os.urandom(12))
print(json.dumps(out))
"#;
    let stdout = python(script, &[PASSPHRASE]);
    let v: serde_json::Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    let unhex = |s: &serde_json::Value| common::unhex(s.as_str().unwrap());

    let salt: [u8; 16] = core::array::from_fn(|i| i as u8);
    let check: [u8; 32] = base64::engine::general_purpose::STANDARD
        .decode(v["key_check"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let vj = VaultManifestV2::create_as(
        wasl_crypto::v2::Format::V2_0, // what tools/decrypt_vault.py reads today
        "py",
        &salt,
        KdfParams::MIN,
        &check,
        0,
    );
    let k = vj
        .verify_passphrase(PASSPHRASE)
        .unwrap()
        .expect("the Python key check verifies in Rust");
    for n in v["names"].as_array().unwrap() {
        let dir = DirId(unhex(&n[0]).try_into().unwrap());
        let enc = n[2].as_str().unwrap();
        assert_eq!(n[1].as_str(), names::decrypt_name(&k, &dir, enc).as_deref());
        assert_eq!(
            enc,
            names::encrypt_name(&k, &dir, n[1].as_str().unwrap()).unwrap()
        );
    }
    let docs = Placement::for_name(&k, &DirId::ROOT, "Docs").unwrap();
    let docs_id = DirId(unhex(&v["dir"]["dir_id"]).try_into().unwrap());
    assert_eq!(
        Some(docs_id),
        names::parse_dir_marker(&k, &docs, &unhex(&v["dir"]["marker"]))
    );
    assert_eq!(
        Some(docs_id),
        DirId::from_property(&k, &docs, v["dir"]["property"].as_str().unwrap())
    );

    let f = &v["file"];
    let at = Placement::for_name(&k, &docs_id, "report.pdf").unwrap();
    let m = FileManifest::open(&k, &at, &unhex(&f["manifest"])).unwrap();
    assert_eq!((5, -1234), (m.version, m.mtime_ms));
    assert_eq!(FileId(unhex(&f["file_id"]).try_into().unwrap()), m.file_id);
    let meta = MetaBlob::decode(&k, &at, f["meta"].as_str().unwrap()).unwrap();
    assert_eq!((42, -1234), (meta.size, meta.mtime_ms));
    if let Some(cid) = f.get("chunk_id") {
        let id = ChunkId(unhex(cid).try_into().unwrap());
        assert_eq!(vec![id], m.chunks.iter().map(|c| c.id).collect::<Vec<_>>());
        let plain = open_chunk(&k, &m.file_id, &id, Some(m.chunks[0].len), &unhex(&f["chunk"])).unwrap();
        assert_eq!(unhex(&f["plain"]), plain);
        assert_eq!(*blake3::hash(&plain).as_bytes(), m.blake3);
    } else {
        println!("note: chunk Python -> Rust not checked: the `blake3` package is not installed");
    }
}
