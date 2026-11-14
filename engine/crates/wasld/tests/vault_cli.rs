//! `wasl vault decrypt` (FR-V3): the real binary recovers v1 and v2 vault folders.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use wasl_crypto::v2::{KdfParams, VaultManifestV2, keys, layout};
use wasl_crypto::{VaultCipher, VaultManifest, kdf, names};

fn wasl(args: &[&Path], stdin: &str, lang: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_wasld"))
        .args(["vault", "decrypt"])
        .args(args)
        .arg("--passphrase-stdin")
        .env("WASL_LANG", lang)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

fn text(o: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn decrypts_a_v2_vault() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("plain");
    std::fs::create_dir_all(src.join("مجلد").join("deeper")).unwrap();
    std::fs::write(src.join("hello.txt"), b"hello").unwrap();
    std::fs::write(src.join("empty"), b"").unwrap();
    let big: Vec<u8> = (0..9_000_000u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    std::fs::write(src.join("مجلد").join("deeper").join("big.bin"), &big).unwrap();
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    let salt = keys::new_salt().unwrap();
    let k = keys::derive("pass words", &salt, KdfParams::MIN).unwrap();
    let vj = VaultManifestV2::create("Docs", &salt, KdfParams::MIN, &k.key_check(), 0);
    std::fs::write(vault.join("vault.json"), vj.to_json()).unwrap();
    // A 2.1 vault, as new vaults are: keyed cuts, padded chunks.
    layout::write_tree_as(&k, wasl_crypto::v2::Format::V2_1, &src, &vault).unwrap();

    let out = tmp.path().join("out");
    let o = wasl(&[&vault, &out], "pass words\n", "en");
    assert_eq!(Some(0), o.status.code(), "{}", text(&o));
    assert!(text(&o).contains("format v2.1"), "{}", text(&o));
    assert!(text(&o).contains("Passphrase: ok"), "{}", text(&o));
    assert_eq!(b"hello".to_vec(), std::fs::read(out.join("hello.txt")).unwrap());
    assert_eq!(0, std::fs::metadata(out.join("empty")).unwrap().len());
    assert_eq!(
        big,
        std::fs::read(out.join("مجلد").join("deeper").join("big.bin")).unwrap()
    );

    // Again without --overwrite: everything is skipped, exit 0; Arabic messages.
    let o = wasl(&[&vault, &out], "pass words\r\n", "ar");
    assert_eq!(Some(0), o.status.code(), "{}", text(&o));
    assert!(text(&o).contains("عبارة المرور: صحيحة"), "{}", text(&o));
    assert!(text(&o).contains("--overwrite"), "{}", text(&o));

    // Wrong passphrase: exit 5, nothing written.
    let o = wasl(&[&vault, &tmp.path().join("out2")], "wrong\n", "en");
    assert_eq!(Some(5), o.status.code(), "{}", text(&o));
    assert!(!tmp.path().join("out2").exists());

    // A damaged chunk: that file fails (exit 1), the others are written.
    let mut objects = Vec::new();
    for shard in std::fs::read_dir(vault.join(".wasl-chunks")).unwrap().flatten() {
        for obj in std::fs::read_dir(shard.path()).unwrap().flatten() {
            objects.push(obj.path());
        }
    }
    let victim = objects
        .iter()
        .max_by_key(|p| std::fs::metadata(p).unwrap().len())
        .unwrap();
    let mut bytes = std::fs::read(victim).unwrap();
    bytes[100] ^= 1;
    std::fs::write(victim, bytes).unwrap();
    let out3 = tmp.path().join("out3");
    let o = wasl(&[&vault, &out3], "pass words\n", "en");
    assert_eq!(Some(1), o.status.code(), "{}", text(&o));
    assert!(text(&o).contains("big.bin"), "{}", text(&o));
    assert!(out3.join("hello.txt").exists());
    assert!(!out3.join("مجلد").join("deeper").join("big.bin").exists());
}

#[test]
fn decrypts_a_v1_vault() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    let salt = kdf::new_salt().unwrap();
    let k = kdf::derive("old words", &salt, 100_000).unwrap();
    let vj = VaultManifest::create("Old", &salt, 100_000, &kdf::key_check(&k), 0);
    std::fs::write(vault.join("vault.json"), vj.to_json()).unwrap();
    let file = vault.join(names::encrypt_name(&k, "root", "note.txt").unwrap());
    VaultCipher::new(&k)
        .encrypt(&b"v1 content"[..], std::fs::File::create(&file).unwrap())
        .unwrap();
    let out = tmp.path().join("out");
    let o = wasl(&[&vault, &out], "old words\n", "en");
    assert_eq!(Some(0), o.status.code(), "{}", text(&o));
    assert!(text(&o).contains("format v1"), "{}", text(&o));
    assert_eq!(
        b"v1 content".to_vec(),
        std::fs::read(out.join("note.txt")).unwrap()
    );
}

#[test]
fn missing_folder_or_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let o = wasl(&[&tmp.path().join("nope"), &tmp.path().join("out")], "x\n", "en");
    assert_eq!(Some(4), o.status.code(), "{}", text(&o));
    let o = wasl(&[tmp.path(), &tmp.path().join("out")], "x\n", "en");
    assert_eq!(Some(4), o.status.code(), "{}", text(&o));
    assert!(text(&o).contains("vault.json"), "{}", text(&o));
}
