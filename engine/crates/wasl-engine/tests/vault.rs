//! Encrypted pairs: [`VaultRemote`] over the in-memory Drive, `ensure_manifest`, and whole runs
//! through the engine. The first three tests port the Android `EncryptedRemoteFsTest`.

mod common;

use common::World;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use wasl_core::{RemoteEntry, RunStatus};
use wasl_crypto::content::{CHUNK_BYTES, read_file_nonce};
use wasl_crypto::{VaultCipher, VaultKeys, VaultManifest, kdf, names};
use wasl_drive::DriveError;
use wasl_engine::{
    DIFFERENT_VAULT, DOCS_UNSUPPORTED, ManifestStatus, MemoryRemote, NEEDS_EMPTY_FOLDER, Op, ROOT_ID, Remote,
    UploadSpec, V1_NO_NEW_FOLDERS, VaultRemote, ensure_manifest,
};

fn keys() -> Arc<VaultKeys> {
    Arc::new(VaultKeys::new([0x11; 32], [0x22; 32]))
}

fn other_keys() -> Arc<VaultKeys> {
    Arc::new(VaultKeys::new([0x33; 32], [0x44; 32]))
}

struct Fixture {
    dir: tempfile::TempDir,
    drive: Arc<MemoryRemote>,
    vault: VaultRemote,
    folder: String,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let drive = Arc::new(MemoryRemote::new());
        let folder = drive.mkdirs("Vault");
        let vault = VaultRemote::new(drive.clone(), keys(), dir.path().join("vault-tmp"));
        Fixture {
            dir,
            drive,
            vault,
            folder,
        }
    }

    fn tmp(&self) -> PathBuf {
        self.dir.path().join("vault-tmp")
    }

    fn src(&self, name: &str, content: &[u8]) -> PathBuf {
        let p = self.dir.path().join("src").join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, content).unwrap();
        p
    }

    async fn upload(&self, parent: &str, name: &str, content: &[u8]) -> wasl_engine::RemoteInfo {
        let src = self.src(name, content);
        self.vault
            .upload(
                UploadSpec {
                    src: &src,
                    name,
                    parent_id: parent,
                    existing_id: None,
                    mtime_ms: 1_000,
                    session_key: None,
                },
                &|_| {},
            )
            .await
            .unwrap()
    }

    /// Every file on the inner Drive under the vault folder: encrypted path → raw bytes.
    fn raw_files(&self) -> Vec<(String, Vec<u8>)> {
        self.drive
            .files()
            .into_iter()
            .filter_map(|(k, v)| k.strip_prefix("Vault/").map(|k| (k.to_owned(), v)))
            .collect()
    }

    async fn entry(&self, key: &str) -> RemoteEntry {
        let listing = self.vault.list_subtree(&self.folder, None).await.unwrap();
        listing
            .entries
            .into_iter()
            .find(|e| e.key == key)
            .unwrap_or_else(|| panic!("{key} not listed"))
    }
}

fn decrypt_bytes(ct: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    VaultCipher::new(&keys()).decrypt(ct, &mut out).unwrap();
    out
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn no_temps(dir: &Path) -> bool {
    std::fs::read_dir(dir).map_or(true, |mut rd| rd.next().is_none())
}

// ---------------------------------------------------------------- port of EncryptedRemoteFsTest

#[tokio::test]
async fn upload_sends_an_encrypted_name() {
    let f = Fixture::new();
    f.upload(&f.folder, "salary-review-2026.txt", b"anything").await;

    let files = f.raw_files();
    assert_eq!(1, files.len());
    let sent = &files[0].0;
    assert_ne!("salary-review-2026.txt", sent);
    assert!(
        !sent.to_lowercase().contains("salary"),
        "name sent to Drive leaks the plaintext: {sent}"
    );
    assert_eq!(
        Some("salary-review-2026.txt".to_owned()),
        names::decrypt_name(&keys(), sent)
    );
}

#[tokio::test]
async fn upload_sends_ciphertext_only() {
    let f = Fixture::new();
    let plaintext = "TOP-SECRET-MARKER ".repeat(4096).into_bytes(); // spans several cipher chunks
    f.upload(&f.folder, "notes.txt", &plaintext).await;

    let sent = &f.raw_files()[0].1;
    assert!(
        !contains(sent, b"TOP-SECRET-MARKER"),
        "bytes sent to Drive contain the plaintext"
    );
    assert_eq!(plaintext, decrypt_bytes(sent));
}

#[tokio::test]
async fn download_returns_the_plaintext() {
    let f = Fixture::new();
    let plaintext = "round trip through Drive ".repeat(3000).into_bytes();
    f.upload(&f.folder, "notes.txt", &plaintext).await;
    let dest = f.dir.path().join("out").join("restored.txt");
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();

    f.vault
        .download(&f.entry("notes.txt").await, &dest, &|_| {})
        .await
        .unwrap();

    assert_eq!(plaintext, std::fs::read(&dest).unwrap());
}

// ---------------------------------------------------------------- names, listing, sizes

#[tokio::test]
async fn names_on_drive_are_ciphertext_and_list_as_plaintext() {
    let f = Fixture::new();
    let docs = f.vault.create_folder(&f.folder, "Docs").await.unwrap();
    assert_eq!("Docs", docs.name);
    let sub = f.vault.create_folder(&docs.id, "ملف").await.unwrap();
    let info = f.upload(&sub.id, "report — résumé.pdf", &[7u8; 5000]).await;
    assert_eq!(Some(5000), info.size, "plaintext size");
    assert!(
        info.md5.is_some(),
        "Drive's MD5 of the ciphertext, the file's identity"
    );
    f.upload(&f.folder, "empty.txt", b"").await;
    f.upload(&f.folder, "big.bin", &vec![1u8; CHUNK_BYTES + 1]).await;

    // On Drive: only ciphertext names, each encrypted with its parent's Drive id.
    let raw = f.drive.list_subtree(&f.folder, None).await.unwrap();
    for e in &raw.entries {
        assert!(names::is_vault_name(&e.name), "{}", e.key);
        let parent = e.parent_id.as_deref().unwrap();
        let plain = names::decrypt_name(&keys(), &e.name).unwrap();
        assert_eq!(e.name, names::encrypt_name(&keys(), parent, &plain).unwrap());
    }
    let cipher_sizes: Vec<u64> = raw.entries.iter().filter_map(|e| e.size).collect();
    assert!(cipher_sizes.contains(&(12 + 5000 + 16)), "{cipher_sizes:?}");
    let raw_md5 = |id: &str| {
        raw.entries
            .iter()
            .find(|e| e.id == id)
            .and_then(|e| e.md5.clone())
    };
    assert_eq!(raw_md5(&info.id), info.md5);

    // Through the vault: plaintext keys and sizes; a file's MD5 is Drive's, of the ciphertext.
    let listing = f.vault.list_subtree(&f.folder, None).await.unwrap();
    let mut got: Vec<(String, bool, Option<u64>)> = listing
        .entries
        .iter()
        .map(|e| (e.key.clone(), e.is_dir, e.size))
        .collect();
    got.sort();
    let expected = vec![
        ("Docs".to_owned(), true, None),
        ("Docs/ملف".to_owned(), true, None),
        ("Docs/ملف/report — résumé.pdf".to_owned(), false, Some(5000)),
        ("big.bin".to_owned(), false, Some(CHUNK_BYTES as u64 + 1)),
        ("empty.txt".to_owned(), false, Some(0)),
    ];
    assert_eq!(expected, got);
    for e in &listing.entries {
        assert_eq!(e.is_dir, e.md5.is_none(), "{}", e.key);
        assert_eq!(raw_md5(&e.id), e.md5, "{}", e.key);
    }
    assert!(!f.vault.plaintext_hashes());
    assert_eq!(0, f.vault.last_skipped());
}

#[tokio::test]
async fn same_name_in_same_folder_keeps_its_ciphertext() {
    let f = Fixture::new();
    let a = f.vault.create_folder(&f.folder, "Camera Roll").await.unwrap();
    f.drive.trash(&a.id).await.unwrap();
    let b = f.vault.create_folder(&f.folder, "Camera Roll").await.unwrap();
    let names_on_drive: Vec<String> = f
        .drive
        .list_subtree(ROOT_ID, None)
        .await
        .unwrap()
        .entries
        .into_iter()
        .filter(|e| e.parent_id.as_deref() == Some(f.folder.as_str()))
        .map(|e| e.name)
        .collect();
    assert_eq!(1, names_on_drive.len());
    assert_eq!(
        names_on_drive[0],
        names::encrypt_name(&keys(), &f.folder, "Camera Roll").unwrap()
    );
    assert_ne!(a.id, b.id);
}

// ---------------------------------------------------------------- content

#[tokio::test]
async fn content_round_trips_byte_identical() {
    let f = Fixture::new();
    let out = f.dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    for (i, n) in [0, 1, 4096, CHUNK_BYTES, CHUNK_BYTES + 1, 2 * CHUNK_BYTES + 17]
        .into_iter()
        .enumerate()
    {
        let content: Vec<u8> = (0..n).map(|j| (j * 31 + i) as u8).collect();
        let name = format!("file-{i}.bin");
        f.upload(&f.folder, &name, &content).await;
        let entry = f.entry(&name).await;
        assert_eq!(Some(n as u64), entry.size);
        let dest = out.join(format!("{name}.wasl-part"));
        f.vault.download(&entry, &dest, &|_| {}).await.unwrap();
        assert_eq!(content, std::fs::read(&dest).unwrap(), "{n} bytes");
    }
    // Upload temps are gone, and so are the downloaded ciphertexts next to the destinations.
    assert!(no_temps(&f.tmp()));
    let left: Vec<_> = std::fs::read_dir(&out)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".dsv"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[tokio::test]
async fn tampered_ciphertext_fails_with_integrity_and_leaves_no_file() {
    let f = Fixture::new();
    f.upload(&f.folder, "a.txt", &vec![5u8; CHUNK_BYTES + 10]).await;
    let enc = names::encrypt_name(&keys(), &f.folder, "a.txt").unwrap();
    let path = format!("Vault/{enc}");
    let good = f.drive.content(&path).unwrap();
    let out = f.dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();

    let variants: Vec<(&str, Vec<u8>)> = vec![
        ("flipped byte", {
            let mut b = good.clone();
            b[100] ^= 1;
            b
        }),
        ("truncated mid-chunk", good[..good.len() - 3].to_vec()),
        ("last chunk dropped", good[..12 + CHUNK_BYTES + 16].to_vec()),
        ("extended", [good.as_slice(), b"xyz"].concat()),
        ("bad magic", [b"DSV2".as_slice(), &good[4..]].concat()),
    ];
    for (what, bytes) in variants {
        f.drive.put_file(&path, &bytes);
        let mut entry = f.entry("a.txt").await;
        // The listing's size follows the tampered ciphertext; keep the real one so the inner
        // download is asked for exactly what Drive has.
        entry.size = wasl_crypto::content::plaintext_size(bytes.len() as u64);
        let dest = out.join("a.txt.wasl-part");
        let err = f.vault.download(&entry, &dest, &|_| {}).await.unwrap_err();
        assert!(matches!(err, DriveError::Integrity(_)), "{what}: {err:?}");
        assert!(!dest.exists(), "{what}: no unverified plaintext may be left");
        assert!(
            !out.join("a.txt.dsv.wasl-part").exists(),
            "{what}: a ciphertext that failed must not be resumed"
        );
    }
}

#[tokio::test]
async fn a_file_from_another_vault_fails_with_integrity() {
    let f = Fixture::new();
    let foreign = VaultRemote::new(f.drive.clone(), other_keys(), f.tmp());
    let src = f.src("x.txt", b"from elsewhere");
    let info = foreign
        .upload(
            UploadSpec {
                src: &src,
                name: "x.txt",
                parent_id: &f.folder,
                existing_id: None,
                mtime_ms: 1,
                session_key: None,
            },
            &|_| {},
        )
        .await
        .unwrap();
    // Its name does not decrypt with our keys, so the listing hides it...
    let listing = f.vault.list_subtree(&f.folder, None).await.unwrap();
    assert!(listing.entries.is_empty());
    assert_eq!(1, f.vault.last_skipped());
    // ...and its content does not verify either.
    let entry = RemoteEntry {
        key: "x.txt".into(),
        id: info.id,
        name: "x.txt".into(),
        is_dir: false,
        size: Some(14),
        md5: None,
        mtime: 1,
        version: info.version,
        google_mime: None,
        parent_id: Some(f.folder.clone()),
    };
    let dest = f.dir.path().join("x.txt");
    let err = f.vault.download(&entry, &dest, &|_| {}).await.unwrap_err();
    assert!(matches!(err, DriveError::Integrity(_)), "{err:?}");
    assert!(!dest.exists());
}

// ---------------------------------------------------------------- foreign items, docs, export

#[tokio::test]
async fn foreign_items_and_google_docs_are_skipped() {
    let f = Fixture::new();
    f.upload(&f.folder, "mine.txt", b"mine").await;
    f.drive.put_file("Vault/vault.json", b"{}");
    f.drive.put_file("Vault/readme.txt", b"plain");
    f.drive.put_file("Vault/not-base64!.dsv", b"x");
    // A plain folder: neither it nor anything inside it is a vault item, even a valid-looking name.
    let plain_dir = f.drive.mkdirs("Vault/Plain folder");
    let inner_name = names::encrypt_name(&keys(), &plain_dir, "hidden.txt").unwrap();
    f.drive
        .put_file(&format!("Vault/Plain folder/{inner_name}"), b"x");
    f.drive
        .put_google_doc("Vault/Notes", "application/vnd.google-apps.document");

    let listing = f.vault.list_subtree(&f.folder, None).await.unwrap();
    let keys_listed: Vec<&str> = listing.entries.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(vec!["mine.txt"], keys_listed);
    assert_eq!(4, f.vault.last_skipped(), "readme, bad name, plain folder, doc");
    assert_eq!(1, f.vault.last_skips().google_docs);
}

#[tokio::test]
async fn plaintext_duplicates_are_reported_not_listed() {
    let f = Fixture::new();
    f.upload(&f.folder, "Report.txt", b"one").await;
    f.upload(&f.folder, "report.txt", b"two").await; // a different ciphertext name on Drive
    let listing = f.vault.list_subtree(&f.folder, None).await.unwrap();
    assert_eq!(1, listing.entries.len());
    assert_eq!(1, listing.duplicates.len());
}

/// ADR-0004: names that differ only in Unicode normalisation are the same name in a vault too.
#[tokio::test]
async fn nfc_and_nfd_plaintext_names_are_duplicates() {
    let f = Fixture::new();
    f.upload(&f.folder, "caf\u{e9}.txt", b"one").await;
    f.upload(&f.folder, "cafe\u{301}.txt", b"two").await;
    let listing = f.vault.list_subtree(&f.folder, None).await.unwrap();
    assert_eq!(1, listing.entries.len());
    assert_eq!(1, listing.duplicates.len());
}

#[tokio::test]
async fn export_is_refused() {
    let f = Fixture::new();
    let entry = f.entry_for_doc().await;
    let err = f
        .vault
        .export(&entry, "application/pdf", &f.dir.path().join("d.pdf"), &|_| {})
        .await
        .unwrap_err();
    assert_eq!(DOCS_UNSUPPORTED, err.to_string());
}

impl Fixture {
    async fn entry_for_doc(&self) -> RemoteEntry {
        self.drive
            .put_google_doc("Vault/Doc", "application/vnd.google-apps.document");
        self.drive
            .list_subtree(&self.folder, None)
            .await
            .unwrap()
            .entries
            .remove(0)
    }
}

#[tokio::test]
async fn trash_and_untrash_pass_through() {
    let f = Fixture::new();
    let info = f.upload(&f.folder, "a.txt", b"a").await;
    f.vault.trash(&info.id).await.unwrap();
    assert!(f.drive.is_trashed(&info.id));
    f.vault.untrash(&info.id).await.unwrap();
    assert!(!f.drive.is_trashed(&info.id));
}

// ---------------------------------------------------------------- resumable uploads

#[tokio::test]
async fn an_interrupted_resumable_upload_reuses_its_ciphertext() {
    let f = Fixture::new();
    let src = f.src("big.bin", &vec![3u8; 3000]);
    let spec = || UploadSpec {
        src: &src,
        name: "big.bin",
        parent_id: &f.folder,
        existing_id: None,
        mtime_ms: 1,
        session_key: Some("p1|big.bin|md5"),
    };
    f.drive
        .fail_next(Op::Upload, DriveError::Network("connection reset".into()));
    let err = f.vault.upload(spec(), &|_| {}).await.unwrap_err();
    assert!(err.is_transient());
    let kept: Vec<PathBuf> = std::fs::read_dir(f.tmp())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    assert_eq!(1, kept.len(), "the ciphertext is kept for the next attempt");
    let nonce = read_file_nonce(&kept[0]).unwrap();

    f.vault.upload(spec(), &|_| {}).await.unwrap();
    let sent = &f.raw_files()[0].1;
    assert_eq!(&nonce[..], &sent[4..12], "the same ciphertext was uploaded");
    assert!(no_temps(&f.tmp()));

    // A permanent failure drops the temp: nothing to resume.
    f.drive
        .poison(&names::encrypt_name(&keys(), &f.folder, "other.bin").unwrap());
    let other = f.src("other.bin", b"o");
    let err = f
        .vault
        .upload(
            UploadSpec {
                src: &other,
                name: "other.bin",
                parent_id: &f.folder,
                existing_id: None,
                mtime_ms: 1,
                session_key: Some("p1|other.bin|md5"),
            },
            &|_| {},
        )
        .await
        .unwrap_err();
    assert!(!err.is_transient());
    assert!(no_temps(&f.tmp()));
}

// ---------------------------------------------------------------- vault.json

fn manifest_for(keys_passphrase: &str, salt: &[u8]) -> VaultManifest {
    let keys = kdf::derive(keys_passphrase, salt, 100_000).unwrap();
    VaultManifest::create(
        "Photos vault",
        salt,
        100_000,
        &kdf::key_check(&keys),
        1_760_000_000_000,
    )
}

#[tokio::test]
async fn ensure_manifest_never_starts_a_v1_folder_but_verifies_existing_ones() {
    let f = Fixture::new();
    let m = manifest_for("pass", &[1; 16]);
    // Security review S4-2: v1 keeps its folders, but an empty folder is not made a v1 vault.
    let err = ensure_manifest(f.drive.as_ref(), &f.folder, None, &m, &f.tmp())
        .await
        .unwrap_err();
    assert!(matches!(err, DriveError::Integrity(_)), "{err:?}");
    assert_eq!(V1_NO_NEW_FOLDERS, err.to_string());
    assert_eq!(None, f.drive.content("Vault/vault.json"), "nothing uploaded");
    // An existing v1 vault folder (made before) verifies.
    f.drive.put_file("Vault/vault.json", m.to_json().as_bytes());
    let status = ensure_manifest(f.drive.as_ref(), &f.folder, None, &m, &f.tmp())
        .await
        .unwrap();
    assert_eq!(ManifestStatus::Verified, status);

    // Same vault (another device: different name and creation time) verifies.
    let same = VaultManifest {
        name: "renamed".into(),
        created: "2000-01-01T00:00:00Z".into(),
        ..m.clone()
    };
    let status = ensure_manifest(f.drive.as_ref(), &f.folder, None, &same, &f.tmp())
        .await
        .unwrap();
    assert_eq!(ManifestStatus::Verified, status);
    // Once the folder has vault items, it still verifies.
    f.upload(&f.folder, "a.txt", b"a").await;
    assert_eq!(
        ManifestStatus::Verified,
        ensure_manifest(f.drive.as_ref(), &f.folder, None, &m, &f.tmp())
            .await
            .unwrap()
    );
    assert!(no_temps(&f.tmp()));
}

#[tokio::test]
async fn ensure_manifest_refuses_a_different_vault() {
    let f = Fixture::new();
    f.drive.put_file(
        "Vault/vault.json",
        manifest_for("pass", &[1; 16]).to_json().as_bytes(),
    );
    for other in [
        manifest_for("other pass", &[1; 16]),
        manifest_for("pass", &[2; 16]),
    ] {
        let err = ensure_manifest(f.drive.as_ref(), &f.folder, None, &other, &f.tmp())
            .await
            .unwrap_err();
        assert!(matches!(err, DriveError::Integrity(_)), "{err:?}");
        assert_eq!(DIFFERENT_VAULT, err.to_string());
    }
}

#[tokio::test]
async fn ensure_manifest_refuses_a_non_empty_folder_without_one() {
    let f = Fixture::new();
    f.drive.put_file("Vault/holiday.jpg", b"jpg");
    let err = ensure_manifest(
        f.drive.as_ref(),
        &f.folder,
        None,
        &manifest_for("pass", &[1; 16]),
        &f.tmp(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, DriveError::Integrity(_)), "{err:?}");
    assert_eq!(NEEDS_EMPTY_FOLDER, err.to_string());
    assert_eq!(None, f.drive.content("Vault/vault.json"), "nothing uploaded");
    // A folder with only a sub-folder or a shortcut is not empty either.
    let g = Fixture::new();
    g.drive.put_shortcut("Vault/link");
    let err = ensure_manifest(
        g.drive.as_ref(),
        &g.folder,
        None,
        &manifest_for("pass", &[1; 16]),
        &g.tmp(),
    )
    .await
    .unwrap_err();
    assert_eq!(NEEDS_EMPTY_FOLDER, err.to_string());
}

#[tokio::test]
async fn ensure_manifest_refuses_an_unusable_manifest() {
    let f = Fixture::new();
    let m = manifest_for("pass", &[1; 16]);
    for text in [
        "not json".to_owned(),
        m.to_json().replace("\"version\": 1", "\"version\": 2"),
    ] {
        f.drive.put_file("Vault/vault.json", text.as_bytes());
        let err = ensure_manifest(f.drive.as_ref(), &f.folder, None, &m, &f.tmp())
            .await
            .unwrap_err();
        assert!(matches!(err, DriveError::Integrity(_)), "{err:?}");
    }
}

// ---------------------------------------------------------------- whole runs through the engine

/// A world whose remote is the in-memory Drive behind a [`VaultRemote`]. The returned temp dir
/// holds the vault's upload temps and must outlive the world.
fn vault_world() -> (World, Arc<VaultKeys>, tempfile::TempDir) {
    let k = keys();
    let tmp_dir = tempfile::tempdir().unwrap();
    let tmp = tmp_dir.path().to_path_buf();
    let wrap_keys = k.clone();
    let w = World::with(
        |_| {},
        Some(Box::new(move |drive| {
            Arc::new(VaultRemote::new(drive, wrap_keys, tmp)) as Arc<dyn Remote>
        })),
    );
    (w, k, tmp_dir)
}

/// Drive files under the pair's folder decrypted by hand: plaintext path → content.
fn decrypt_drive(w: &World, k: &VaultKeys) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    for (path, bytes) in w.drive.files() {
        let Some(rel) = path.strip_prefix("Sync/") else {
            continue;
        };
        if rel == VaultManifest::FILE_NAME {
            continue;
        }
        let plain_path: Vec<String> = rel
            .split('/')
            .map(|seg| names::decrypt_name(k, seg).unwrap_or_else(|| panic!("{seg} is not a vault name")))
            .collect();
        let mut plain = Vec::new();
        VaultCipher::new(k).decrypt(bytes.as_slice(), &mut plain).unwrap();
        out.insert(plain_path.join("/"), String::from_utf8(plain).unwrap());
    }
    out
}

#[tokio::test]
async fn engine_syncs_through_the_vault_drive_holds_only_ciphertext() {
    let (w, k, _tmp) = vault_world();
    let folder = w.pair().drive_folder_id.unwrap();
    let manifest = manifest_for("pass", &[9; 16]);
    // An existing v1 vault folder (new ones are not started any more).
    w.drive.put_file("Sync/vault.json", manifest.to_json().as_bytes());
    ensure_manifest(
        w.drive.as_ref(),
        &folder,
        None,
        &manifest,
        &w.dir.path().join("t"),
    )
    .await
    .unwrap();
    w.write("a.txt", "local a secret");
    w.write("docs/b.txt", "local b secret");

    let run = w.sync().await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    assert_eq!(2, run.summary.unwrap().uploads);

    // Drive: vault.json plus ciphertext names and bytes only.
    for (path, bytes) in w.drive.files() {
        let rel = path.strip_prefix("Sync/").unwrap();
        if rel == VaultManifest::FILE_NAME {
            continue;
        }
        assert!(rel.split('/').all(names::is_vault_name), "{rel}");
        assert!(!contains(&bytes, b"secret"), "{rel} holds plaintext");
    }
    let expected = common::map(&[("a.txt", "local a secret"), ("docs/b.txt", "local b secret")]);
    assert_eq!(expected, decrypt_drive(&w, &k));
    assert_eq!(expected, w.local_files(), "the device keeps plaintext");

    // Nothing changed: the next run is quiet (identity by Drive id and version, no MD5).
    let again = w.sync().await;
    assert_eq!(RunStatus::Success, again.status);
    let s = again.summary.unwrap();
    assert!(s.is_empty(), "{s:?}");
    assert_eq!(2, s.unchanged);

    // Another device adds and edits files through the vault: they arrive as plaintext.
    let other_device = VaultRemote::new(w.drive.clone(), k.clone(), w.dir.path().join("t2"));
    let src = w.dir.path().join("from-elsewhere.txt");
    std::fs::write(&src, "from the other phone").unwrap();
    other_device
        .upload(
            UploadSpec {
                src: &src,
                name: "c.txt",
                parent_id: &folder,
                existing_id: None,
                mtime_ms: 2_000_000_000_000,
                session_key: None,
            },
            &|_| {},
        )
        .await
        .unwrap();
    let a_id = w
        .drive
        .id_of(&format!(
            "Sync/{}",
            names::encrypt_name(&k, &folder, "a.txt").unwrap()
        ))
        .unwrap();
    std::fs::write(&src, "edited on the other phone").unwrap();
    other_device
        .upload(
            UploadSpec {
                src: &src,
                name: "a.txt",
                parent_id: &folder,
                existing_id: Some(&a_id),
                mtime_ms: 2_000_000_000_000,
                session_key: None,
            },
            &|_| {},
        )
        .await
        .unwrap();
    // Someone drops a plain file into the encrypted folder: ignored, never deleted.
    w.drive.put_file("Sync/dropped.txt", b"plain");

    let run = w.sync().await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    assert_eq!(2, run.summary.unwrap().downloads);
    assert_eq!(Some("from the other phone".into()), w.read("c.txt"));
    assert_eq!(Some("edited on the other phone".into()), w.read("a.txt"));
    assert_eq!(None, w.read("dropped.txt"));
    assert_eq!(Some(b"plain".to_vec()), w.drive.content("Sync/dropped.txt"));

    // A local delete trashes the ciphertext on Drive; the foreign file stays.
    w.remove("docs/b.txt");
    let run = w.sync().await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    let left = decrypt_drive_without(&w, &k, "dropped.txt");
    assert_eq!(
        common::map(&[
            ("a.txt", "edited on the other phone"),
            ("c.txt", "from the other phone")
        ]),
        left
    );
    assert!(w.drive.content("Sync/dropped.txt").is_some());
    let quiet = w.sync().await;
    assert!(quiet.summary.unwrap().is_empty());
}

fn decrypt_drive_without(
    w: &World,
    k: &VaultKeys,
    foreign: &str,
) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    for (path, bytes) in w.drive.files() {
        let Some(rel) = path.strip_prefix("Sync/") else {
            continue;
        };
        if rel == VaultManifest::FILE_NAME || rel == foreign {
            continue;
        }
        let plain_path: Vec<String> = rel
            .split('/')
            .map(|seg| names::decrypt_name(k, seg).unwrap())
            .collect();
        let mut plain = Vec::new();
        VaultCipher::new(k).decrypt(bytes.as_slice(), &mut plain).unwrap();
        out.insert(plain_path.join("/"), String::from_utf8(plain).unwrap());
    }
    out
}

#[tokio::test]
async fn tampered_drive_file_fails_that_file_only() {
    let (w, k, _tmp) = vault_world();
    let folder = w.pair().drive_folder_id.unwrap();
    let other_device = VaultRemote::new(w.drive.clone(), k.clone(), w.dir.path().join("t2"));
    for (name, body) in [("good.txt", "fine"), ("bad.txt", "will be tampered")] {
        let src = w.dir.path().join(name);
        std::fs::write(&src, body).unwrap();
        other_device
            .upload(
                UploadSpec {
                    src: &src,
                    name,
                    parent_id: &folder,
                    existing_id: None,
                    mtime_ms: 1,
                    session_key: None,
                },
                &|_| {},
            )
            .await
            .unwrap();
    }
    let bad = format!("Sync/{}", names::encrypt_name(&k, &folder, "bad.txt").unwrap());
    let mut bytes = w.drive.content(&bad).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    w.drive.put_file(&bad, &bytes);

    let run = w.sync().await;
    assert_ne!(RunStatus::Success, run.status);
    assert_eq!(Some("fine".into()), w.read("good.txt"));
    assert_eq!(None, w.read("bad.txt"), "no unverified plaintext");
    assert!(
        w.local_files().keys().all(|k| !k.contains(".dsv")),
        "{:?}",
        w.local_files()
    );
    assert_eq!(
        Some("bad.txt: Couldn't download. Transferred data did not verify: authentication failed on the final chunk (0): the file was modified or truncated".into()),
        run.message
    );
}

#[tokio::test]
async fn the_engine_checks_the_folders_manifest_before_every_run() {
    let tmp = tempfile::tempdir().unwrap();
    let mine = manifest_for("correct horse", &[7; 16]);
    let w = World::with(
        |_| {},
        Some(Box::new({
            let mine = mine.clone();
            let tmp = tmp.path().to_path_buf();
            move |m| Arc::new(VaultRemote::new(m, keys(), tmp).with_manifest(mine))
        })),
    );
    w.write("secret.txt", "hush");
    // An empty folder does not become a v1 vault (S4-2); nothing is uploaded.
    let run = w.sync().await;
    assert_eq!(RunStatus::Failed, run.status);
    assert!(run.message.unwrap().contains(V1_NO_NEW_FOLDERS));
    assert!(w.drive.files().is_empty());
    // The vault's existing folder syncs.
    w.drive.put_file("Sync/vault.json", mine.to_json().as_bytes());
    let run = w.sync().await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    let on_drive = w.drive.files();
    assert_eq!(
        2,
        on_drive.len(),
        "the manifest and one ciphertext file: {:?}",
        on_drive.keys()
    );

    // A folder holding another vault's manifest is refused before anything moves.
    let tmp2 = tempfile::tempdir().unwrap();
    let theirs = manifest_for("other words", &[9; 16]);
    let w2 = World::with(
        |_| {},
        Some(Box::new({
            let tmp = tmp2.path().to_path_buf();
            move |m| Arc::new(VaultRemote::new(m, keys(), tmp).with_manifest(theirs))
        })),
    );
    w2.drive.put_file("Sync/vault.json", mine.to_json().as_bytes());
    w2.write("a.txt", "a");
    let r = w2.sync().await;
    assert_eq!(RunStatus::Failed, r.status);
    assert!(r.message.unwrap().contains(DIFFERENT_VAULT));
    assert_eq!(1, w2.drive.files().len(), "nothing was uploaded");
}
