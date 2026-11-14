//! Interoperability with the independent Python implementation, `tools/decrypt_vault.py`.
//!
//! Rust → Python: a small vault laid out like the Drive folder (plaintext `vault.json`, one
//! encrypted name per item, names encrypted with the parent folder's Drive id) is decrypted by the
//! tool, and the result must be byte-identical.
//!
//! Python → Rust: the tool can only derive keys and encrypt names (it has no content encryptor),
//! so that direction checks key derivation, the key check and names.
//!
//! Both tests pass with a printed note when `python` or its `cryptography` package is missing.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use wasl_crypto::{VaultCipher, VaultManifest, kdf, names};

const PASSPHRASE: &str = "correct horse battery staple ✓";
// The manifest refuses fewer than 100,000 PBKDF2 iterations.
const ITERATIONS: u32 = 100_000;

fn tool() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tools/decrypt_vault.py")
}

fn python_ready() -> bool {
    let ok = Command::new("python")
        .args(["-I", "-c", "import cryptography"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        println!("note: skipped, `python` with the `cryptography` package is not available");
    }
    ok
}

/// Every file under `dir` as relative path → content.
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
fn python_tool_decrypts_a_rust_vault() {
    if !python_ready() {
        return;
    }
    let salt = kdf::new_salt().unwrap();
    let keys = kdf::derive(PASSPHRASE, &salt, ITERATIONS).unwrap();
    let cipher = VaultCipher::new(&keys);
    let tmp = tempfile::tempdir().unwrap();
    let vault = tmp.path().join("vault");
    let out = tmp.path().join("out");
    std::fs::create_dir_all(&vault).unwrap();
    let manifest = VaultManifest::create(
        "Cross-check vault",
        &salt,
        ITERATIONS,
        &kdf::key_check(&keys),
        1_760_000_000_000,
    );
    std::fs::write(vault.join(VaultManifest::FILE_NAME), manifest.to_json()).unwrap();

    // (Drive id of the parent, relative plaintext path, content or None for a folder). Folder ids
    // are made up the way Drive would hand them out; the name nonce depends on them.
    let mib = wasl_crypto::content::CHUNK_BYTES;
    let items: Vec<(&str, &str, Option<Vec<u8>>)> = vec![
        ("root-id", "hello.txt", Some(b"hello world".to_vec())),
        ("root-id", "Docs", None),
        (
            "docs-id",
            "Docs/report — résumé.pdf",
            Some(common::random_bytes(2 * mib + mib / 2, 1)),
        ),
        ("docs-id", "Docs/empty.txt", Some(Vec::new())),
        ("docs-id", "Docs/ملف", None),
        (
            "arabic-id",
            "Docs/ملف/exact.bin",
            Some(common::random_bytes(mib, 2)),
        ),
        ("root-id", "写真 📷.jpg", Some(common::random_bytes(4097, 3))),
    ];
    let mut enc_path: BTreeMap<&str, PathBuf> = BTreeMap::new();
    let mut expected = BTreeMap::new();
    for (parent_id, rel, content) in &items {
        let (parent_rel, name) = rel.rsplit_once('/').unwrap_or(("", rel));
        let parent_dir = if parent_rel.is_empty() {
            vault.clone()
        } else {
            enc_path[parent_rel].clone()
        };
        let path = parent_dir.join(names::encrypt_name(&keys, parent_id, name).unwrap());
        match content {
            None => std::fs::create_dir_all(&path).unwrap(),
            Some(bytes) => {
                cipher
                    .encrypt(bytes.as_slice(), &mut std::fs::File::create(&path).unwrap())
                    .unwrap();
                expected.insert((*rel).to_owned(), bytes.clone());
            }
        }
        enc_path.insert(rel, path);
    }
    // Someone else's file in the folder: the tool skips it, as the sync does.
    std::fs::write(vault.join("dropped-by-someone.txt"), b"plain").unwrap();

    let run = Command::new("python")
        .arg("-I")
        .arg(tool())
        .arg(&vault)
        .arg(&out)
        .args(["--passphrase", PASSPHRASE])
        .env("PYTHONIOENCODING", "utf-8")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "tool failed\n{stdout}\n{stderr}");
    assert!(stdout.contains("Passphrase: ok"), "{stdout}");
    assert!(stdout.contains("Skipped 1 item(s)"), "{stdout}");
    assert_eq!(expected, tree(&out), "{stdout}");
    assert!(out.join("Docs").join("ملف").is_dir());

    // A wrong passphrase is refused by the tool's key check (exit code 2).
    let wrong = Command::new("python")
        .arg("-I")
        .arg(tool())
        .arg(&vault)
        .arg(tmp.path().join("out2"))
        .args(["--passphrase", "not it"])
        .output()
        .unwrap();
    assert_eq!(Some(2), wrong.status.code());
}

#[test]
fn rust_reads_what_the_python_tool_derives_and_encrypts() {
    if !python_ready() {
        return;
    }
    let tools_dir = tool().parent().unwrap().to_path_buf();
    let script = r#"
import base64, sys
sys.path.insert(0, sys.argv[1])
import decrypt_vault as d
salt = bytes(range(16))
key_enc, key_name = d.derive_keys(sys.argv[2], salt, int(sys.argv[3]))
print(base64.b64encode(d.key_check(key_name)).decode())
for parent, name in [("root-id", "hello.txt"), ("", "Report — résumé.pdf"), ("p", "写真 📷.jpg")]:
    print(d.encrypt_name(key_enc, key_name, parent, name))
"#;
    let run = Command::new("python")
        .arg("-I")
        .arg("-c")
        .arg(script)
        .arg(&tools_dir)
        .arg(PASSPHRASE)
        .arg(ITERATIONS.to_string())
        .env("PYTHONIOENCODING", "utf-8")
        .output()
        .unwrap();
    let stdout = String::from_utf8(run.stdout).unwrap();
    assert!(
        run.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let lines: Vec<&str> = stdout.lines().map(str::trim).collect();
    assert_eq!(4, lines.len(), "{stdout}");

    let salt: Vec<u8> = (0u8..16).collect();
    let manifest = VaultManifest::create("py", &salt, ITERATIONS, &[0; 32], 0);
    let manifest = VaultManifest {
        key_check: lines[0].to_owned(),
        ..manifest
    };
    let keys = manifest
        .verify_passphrase(PASSPHRASE)
        .unwrap()
        .expect("the Python key check verifies in Rust");
    let expected = [
        ("root-id", "hello.txt"),
        ("", "Report — résumé.pdf"),
        ("p", "写真 📷.jpg"),
    ];
    for ((parent, name), enc) in expected.iter().zip(&lines[1..]) {
        assert_eq!(Some((*name).to_owned()), names::decrypt_name(&keys, enc));
        assert_eq!(*enc, names::encrypt_name(&keys, parent, name).unwrap());
    }
    println!("note: content Python → Rust is not checked: tools/decrypt_vault.py has no content encryptor");
}
