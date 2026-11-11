//! Vault v2 pairs: [`VaultV2Remote`] over the in-memory Drive, whole engine runs, chunked edits,
//! the sweep, integrity failures, rollback, rename/move, and on-demand range reads.

mod common;

use async_trait::async_trait;
use common::World;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wasl_core::{RemoteEntry, RunStatus};
use wasl_crypto::recover::{Unlocked, decrypt_tree};
use wasl_crypto::v2::chunks::{CHUNKS_DIR, MAX_CHUNK_BYTES, MAX_SEALED_BYTES};
use wasl_crypto::v2::manifest::{MAX_MANIFEST_BYTES, META_PROPERTY};
use wasl_crypto::v2::names::{DIR_MARKER_BYTES, DIR_MARKER_NAME, DIR_PROPERTY, is_vault_name};
use wasl_crypto::v2::state::RECORD_BYTES;
use wasl_crypto::v2::{ChunkId, KdfParams, VaultKeysV2, VaultManifestV2};
use wasl_crypto::{VaultManifest, kdf};
use wasl_drive::DriveError;
use wasl_engine::{
    DIFFERENT_VAULT, Listing, ManifestStatus, MemoryRemote, NEEDS_EMPTY_FOLDER, ProgressFn, Remote,
    RemoteInfo, SWEEP_GRACE, UploadSpec, VAULT_IS_V1, VAULT_IS_V2, VaultV2Remote, ensure_manifest,
    ensure_manifest_v2,
};
use wasl_store::{Pair, SharedStore, Store};
use wasl_vfs::{FileRef, Source};

fn keys() -> Arc<VaultKeysV2> {
    Arc::new(VaultKeysV2::from_ikm(&[0x42; 32]))
}

fn other_keys() -> Arc<VaultKeysV2> {
    Arc::new(VaultKeysV2::from_ikm(&[0x43; 32]))
}

fn vault_json(k: &VaultKeysV2, salt: u8) -> VaultManifestV2 {
    VaultManifestV2::create(
        "Test",
        &[salt; 16],
        KdfParams::MIN,
        &k.key_check(),
        1_760_000_000_000,
    )
}

/// Deterministic bytes that do not compress or repeat (so chunk cuts are realistic).
fn pseudo(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut out = Vec::with_capacity(n + 8);
    while out.len() < n {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.truncate(n);
    out
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

struct Fixture {
    dir: tempfile::TempDir,
    drive: Arc<MemoryRemote>,
    vault: VaultV2Remote,
    folder: String,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let drive = Arc::new(MemoryRemote::new());
        let folder = drive.mkdirs("Vault");
        let vault = VaultV2Remote::new(drive.clone(), keys(), dir.path().join("tmp"));
        Fixture {
            dir,
            drive,
            vault,
            folder,
        }
    }

    /// Another device on the same Drive folder.
    fn device(&self, name: &str) -> VaultV2Remote {
        VaultV2Remote::new(self.drive.clone(), keys(), self.dir.path().join(name))
    }

    fn src(&self, content: &[u8]) -> PathBuf {
        let p = self.dir.path().join("src").join(format!("{}", content.len()));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, content).unwrap();
        p
    }

    async fn list(&self) -> BTreeMap<String, RemoteEntry> {
        list(&self.vault, &self.folder).await
    }

    async fn put(
        &self,
        parent: &str,
        name: &str,
        content: &[u8],
        existing: Option<&str>,
    ) -> wasl_engine::RemoteInfo {
        put(&self.vault, &self.src(content), parent, name, existing).await
    }

    async fn get(&self, v: &VaultV2Remote, entry: &RemoteEntry) -> Result<Vec<u8>, DriveError> {
        let dest = self.dir.path().join("out").join(&entry.id);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        v.download(entry, &dest, &|_| {}).await?;
        let bytes = std::fs::read(&dest).unwrap();
        Ok(bytes)
    }

    /// Drive paths (relative to the vault folder) → content.
    fn raw(&self) -> BTreeMap<String, Vec<u8>> {
        self.drive
            .files()
            .into_iter()
            .filter_map(|(k, v)| k.strip_prefix("Vault/").map(|k| (k.to_owned(), v)))
            .collect()
    }

    fn chunk_paths(&self) -> BTreeSet<String> {
        self.raw()
            .into_keys()
            .filter(|k| k.starts_with(CHUNKS_DIR))
            .collect()
    }
}

async fn list(v: &VaultV2Remote, folder: &str) -> BTreeMap<String, RemoteEntry> {
    v.list_subtree(folder, None)
        .await
        .unwrap()
        .entries
        .into_iter()
        .map(|e| (e.key.clone(), e))
        .collect()
}

async fn put(
    v: &VaultV2Remote,
    src: &Path,
    parent: &str,
    name: &str,
    existing: Option<&str>,
) -> wasl_engine::RemoteInfo {
    v.upload(
        UploadSpec {
            src,
            name,
            parent_id: parent,
            existing_id: existing,
            mtime_ms: 1_760_000_123_000,
            session_key: None,
        },
        &|_| {},
    )
    .await
    .unwrap()
}

/// Writes the Drive folder to disk as a Drive download would and decrypts it with the recovery
/// code (an independent reader of the layout): plaintext path → content.
fn recover(drive: &MemoryRemote, prefix: &str, k: &VaultKeysV2, dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let dump = dir.join("dump");
    let out = dir.join("recovered");
    let _ = std::fs::remove_dir_all(&dump);
    let _ = std::fs::remove_dir_all(&out);
    for (path, bytes) in drive.files() {
        if let Some(rel) = path.strip_prefix(prefix) {
            let p = dump.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        }
    }
    let summary = decrypt_tree(&dump, &out, &Unlocked::V2(k.clone()), false, |_| {}).unwrap();
    assert!(summary.failed.is_empty(), "{:?}", summary.failed);
    let mut files = BTreeMap::new();
    collect(&out, "", &mut files);
    files
}

fn collect(dir: &Path, prefix: &str, out: &mut BTreeMap<String, Vec<u8>>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if e.file_type().unwrap().is_dir() {
            collect(&e.path(), &rel, out);
        } else {
            out.insert(rel, std::fs::read(e.path()).unwrap());
        }
    }
}

// ---------------------------------------------------------------- whole runs through the engine

/// A world whose remote is the in-memory Drive behind a [`VaultV2Remote`] that checks `vault.json`
/// and keeps manifest versions in the engine's store.
fn v2_world(grace: Duration) -> (World, Arc<VaultV2Remote>, tempfile::TempDir) {
    let tmp_dir = tempfile::tempdir().unwrap();
    let tmp = tmp_dir.path().to_path_buf();
    let slot: Arc<Mutex<Option<Arc<VaultV2Remote>>>> = Arc::default();
    let w = World::with(
        |p| p.vault_id = Some("v2".into()),
        Some(Box::new({
            let slot = slot.clone();
            move |drive| {
                let k = keys();
                let v = Arc::new(
                    VaultV2Remote::new(drive, k.clone(), tmp)
                        .with_manifest(vault_json(&k, 7))
                        .with_sweep_grace(grace),
                );
                *slot.lock().unwrap() = Some(v.clone());
                v as Arc<dyn Remote>
            }
        })),
    );
    let v = slot.lock().unwrap().clone().unwrap();
    v.use_store(w.engine.store().clone(), w.pair_id.clone());
    (w, v, tmp_dir)
}

#[tokio::test]
async fn engine_round_trip_drive_holds_only_ciphertext() {
    let (w, v, _tmp) = v2_world(Duration::ZERO);
    let big = pseudo(9 * 1024 * 1024 + 123, 1);
    w.write("a.txt", "secret alpha");
    w.write("docs/deep/ملاحظات سرية.txt", "secret arabic محتوى");
    w.write("وثائق/فارغ.txt", "");
    std::fs::create_dir_all(w.path("media")).unwrap();
    std::fs::write(w.path("media/big.bin"), &big).unwrap();

    let run = w.sync().await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    assert_eq!(4, run.summary.unwrap().uploads);
    // E2E 2026-10: progress counted the padded, encrypted bytes and showed 102%.
    let (done, total) = *w.most_done.lock().unwrap();
    assert!(done > 0 && done <= total, "{done} of {total}");

    // Drive: vault.json, encrypted names, `.wasl-dir` markers and chunk objects only.
    let mut chunk_count = 0;
    let mut state_records = 0;
    for (path, bytes) in w.drive.files() {
        let rel = path.strip_prefix("Sync/").unwrap();
        if rel == VaultManifestV2::FILE_NAME {
            continue;
        }
        if let Some(writer) = rel.strip_prefix(".wasl-state.") {
            // This device's state record (ENCRYPTION.md §11.11): a writer id, nothing else.
            assert_eq!(16, writer.len(), "{rel}");
            state_records += 1;
            continue;
        }
        if let Some(chunk) = rel.strip_prefix(&format!("{CHUNKS_DIR}/")) {
            chunk_count += 1;
            assert_eq!(2, chunk.split('/').count(), "{rel}");
        } else {
            let segs: Vec<&str> = rel.split('/').collect();
            let (last, dirs) = segs.split_last().unwrap();
            assert!(dirs.iter().all(|s| is_vault_name(s)), "{rel}");
            assert!(is_vault_name(last) || *last == DIR_MARKER_NAME, "{rel}");
        }
        for leak in ["secret alpha", "secret arabic", "ملاحظات"] {
            assert!(!contains(&bytes, leak.as_bytes()), "{rel} holds plaintext");
        }
        for leak in ["secret", "ملاحظات", "وثائق", "big.bin", "docs", "media"] {
            assert!(!rel.contains(leak), "{rel} leaks a name");
        }
    }
    assert!(
        chunk_count >= 4,
        "the 9 MiB file is several chunks: {chunk_count}"
    );
    assert_eq!(1, state_records);

    // The recovery reader (independent of the remote) decrypts what is on Drive.
    let recovered = recover(&w.drive, "Sync/", &keys(), w.dir.path());
    let expected: BTreeMap<String, Vec<u8>> = [
        ("a.txt", b"secret alpha".to_vec()),
        (
            "docs/deep/ملاحظات سرية.txt",
            "secret arabic محتوى".as_bytes().to_vec(),
        ),
        ("media/big.bin", big.clone()),
        ("وثائق/فارغ.txt", Vec::new()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    assert_eq!(expected, recovered);
    assert_eq!(0, v.last_skips().foreign);

    // Nothing changed: the next run is quiet.
    let again = w.sync().await;
    assert_eq!(RunStatus::Success, again.status, "{:?}", again.message);
    let s = again.summary.unwrap();
    assert!(s.is_empty(), "{s:?}");

    // Another device edits and adds through the vault; this one downloads plaintext.
    let other = VaultV2Remote::new(w.drive.clone(), keys(), w.dir.path().join("t2"));
    let folder = w.pair().drive_folder_id.unwrap();
    let listed = list(&other, &folder).await;
    let docs_deep = listed["docs/deep"].id.clone();
    let src = w.dir.path().join("elsewhere");
    std::fs::write(&src, "edited elsewhere").unwrap();
    put(&other, &src, &folder, "a.txt", Some(&listed["a.txt"].id)).await;
    std::fs::write(&src, "new elsewhere").unwrap();
    put(&other, &src, &docs_deep, "c.txt", None).await;
    let mut big2 = big.clone();
    big2[5_000_000..5_000_100].copy_from_slice(&[0xAB; 100]);
    std::fs::write(&src, &big2).unwrap();
    put(
        &other,
        &src,
        &listed["media"].id,
        "big.bin",
        Some(&listed["media/big.bin"].id),
    )
    .await;
    w.drive.put_file("Sync/dropped.txt", b"plain"); // foreign: ignored, never deleted

    let run = w.sync().await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    assert_eq!(3, run.summary.unwrap().downloads);
    assert_eq!(Some("edited elsewhere".into()), w.read("a.txt"));
    assert_eq!(Some("new elsewhere".into()), w.read("docs/deep/c.txt"));
    assert_eq!(big2, std::fs::read(w.path("media/big.bin")).unwrap());
    assert_eq!(None, w.read("dropped.txt"));

    // A local delete trashes the manifest; the foreign file stays.
    std::fs::remove_file(w.path("docs/deep/c.txt")).unwrap();
    let run = w.sync().await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    assert!(w.drive.content("Sync/dropped.txt").is_some());
    let quiet = w.sync().await;
    assert!(quiet.summary.unwrap().is_empty());

    // The bin purge runs the sweep (here with no grace): c.txt's chunk goes, nothing else.
    let chunks_before = w.drive.files().keys().filter(|k| k.contains(CHUNKS_DIR)).count();
    w.engine.purge_expired_bin().await.unwrap();
    let chunks_after = w.drive.files().keys().filter(|k| k.contains(CHUNKS_DIR)).count();
    assert_eq!(chunks_before - 1, chunks_after);
    let recovered = recover(&w.drive, "Sync/", &keys(), w.dir.path());
    assert_eq!(big2, recovered["media/big.bin"]);
}

/// The Drive id of the only file manifest in `w`'s vault.
fn only_manifest(w: &World) -> String {
    let manifests: Vec<String> = w
        .drive
        .files()
        .into_keys()
        .filter_map(|path| {
            let rel = path.strip_prefix("Sync/")?;
            let other = rel == VaultManifestV2::FILE_NAME
                || rel.starts_with(".wasl-state.")
                || rel.starts_with(&format!("{CHUNKS_DIR}/"))
                || rel.ends_with(DIR_MARKER_NAME);
            (!other).then(|| path.clone())
        })
        .collect();
    assert_eq!(1, manifests.len(), "{manifests:?}");
    w.drive.id_of(&manifests[0]).unwrap()
}

// E2E 2026-11-02: a vault pair uploaded 4 files, and its next run downloaded all 4 back ("changed
// on Drive") with nothing changed. Drive had raised the manifests' versions after the upload, and a
// vault file was listed without an md5, so the version alone decided. Now the manifest's md5 does.
#[tokio::test]
async fn a_vault_pair_does_not_download_what_it_just_uploaded_when_drive_raises_the_version() {
    let (w, _v, _tmp) = v2_world(Duration::ZERO);
    w.write("a.txt", "secret alpha");
    let run = w.sync().await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    assert_eq!(1, run.summary.unwrap().uploads);

    w.drive.bump_version(&only_manifest(&w));

    let run = w.sync().await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    let s = run.summary.unwrap();
    assert_eq!((0, 0, 0), (s.uploads, s.downloads, s.conflicts));
}

#[tokio::test]
async fn a_vault_file_written_by_another_device_is_still_downloaded() {
    let (w, _v, _tmp) = v2_world(Duration::ZERO);
    w.write("a.txt", "secret alpha");
    let run = w.sync().await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);

    // The same size; the manifest's md5 changes with the write (a fresh nonce).
    let other = VaultV2Remote::new(w.drive.clone(), keys(), w.dir.path().join("t2"));
    let folder = w.pair().drive_folder_id.unwrap();
    let before = list(&other, &folder).await.remove("a.txt").unwrap();
    assert!(
        before.md5.is_some(),
        "a vault file is listed with its manifest's md5"
    );
    let src = w.dir.path().join("elsewhere");
    std::fs::write(&src, "secret ALPHA").unwrap();
    put(&other, &src, &folder, "a.txt", Some(&before.id)).await;
    let after = list(&other, &folder).await.remove("a.txt").unwrap();
    assert_eq!((&before.id, before.size), (&after.id, after.size));
    assert_ne!(before.md5, after.md5);

    let run = w.sync().await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    let s = run.summary.unwrap();
    assert_eq!((0, 1, 0), (s.uploads, s.downloads, s.conflicts));
    assert_eq!(Some("secret ALPHA".into()), w.read("a.txt"));
}

// Baseline rows written before vault listings carried an md5 have the Drive version only. The first
// run after the upgrade adopts the md5 without a transfer, and from then on the md5 decides.
#[tokio::test]
async fn a_vault_baseline_row_without_an_md5_is_adopted_without_a_transfer() {
    let (w, _v, _tmp) = v2_world(Duration::ZERO);
    w.write("a.txt", "secret alpha");
    let run = w.sync().await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    let store = w.engine.store().clone();
    let mut row = store.lock().baseline_entry(&w.pair_id, "a.txt").unwrap().unwrap();
    let md5 = row.remote_md5.take();
    assert!(md5.is_some(), "an upload records the manifest's md5");
    store.lock().put_baseline(&w.pair_id, &row).unwrap();

    let run = w.sync().await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    let s = run.summary.unwrap();
    assert_eq!((0, 0, 0), (s.uploads, s.downloads, s.conflicts));
    let row = store.lock().baseline_entry(&w.pair_id, "a.txt").unwrap().unwrap();
    assert_eq!(md5, row.remote_md5, "the md5 is recorded");

    w.drive.bump_version(&only_manifest(&w));
    let run = w.sync().await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    let s = run.summary.unwrap();
    assert_eq!((0, 0, 0), (s.uploads, s.downloads, s.conflicts));
}

#[tokio::test]
async fn the_engine_checks_vault_json_before_every_run() {
    // An empty folder gets vault.json.
    let (w, _v, _tmp) = v2_world(SWEEP_GRACE);
    w.write("x.txt", "x");
    let run = w.sync().await;
    assert_eq!(RunStatus::Success, run.status, "{:?}", run.message);
    let json = w.drive.content("Sync/vault.json").unwrap();
    assert!(VaultManifestV2::parse(std::str::from_utf8(&json).unwrap()).is_ok());

    // A v1 vault folder is refused before anything moves.
    let (w2, _v2, _tmp2) = v2_world(SWEEP_GRACE);
    let v1 = VaultManifest::create(
        "Old",
        &[1; 16],
        1000,
        &kdf::key_check(&wasl_crypto::VaultKeys::new([2; 32], [3; 32])),
        0,
    );
    w2.drive.put_file("Sync/vault.json", v1.to_json().as_bytes());
    w2.write("a.txt", "a");
    let r = w2.sync().await;
    assert_eq!(RunStatus::Failed, r.status);
    assert!(r.message.unwrap().contains(VAULT_IS_V1));
    assert_eq!(1, w2.drive.files().len(), "nothing was uploaded");
}

#[tokio::test]
async fn ensure_manifest_v2_creates_verifies_and_refuses() {
    let f = Fixture::new();
    let tmp = f.dir.path().join("t");
    let mine = vault_json(&keys(), 7);
    assert_eq!(
        ManifestStatus::Created,
        ensure_manifest_v2(f.drive.as_ref(), &f.folder, None, &mine, &tmp)
            .await
            .unwrap()
    );
    assert_eq!(
        ManifestStatus::Verified,
        ensure_manifest_v2(f.drive.as_ref(), &f.folder, None, &mine, &tmp)
            .await
            .unwrap()
    );
    // Another v2 vault.
    let theirs = vault_json(&other_keys(), 8);
    let e = ensure_manifest_v2(f.drive.as_ref(), &f.folder, None, &theirs, &tmp)
        .await
        .unwrap_err();
    assert!(e.to_string().contains(DIFFERENT_VAULT), "{e}");
    // A v1 pair on a v2 folder.
    let v1 = VaultManifest::create(
        "Old",
        &[1; 16],
        1000,
        &kdf::key_check(&wasl_crypto::VaultKeys::new([2; 32], [3; 32])),
        0,
    );
    let e = ensure_manifest(f.drive.as_ref(), &f.folder, None, &v1, &tmp)
        .await
        .unwrap_err();
    assert!(e.to_string().contains(VAULT_IS_V2), "{e}");
    // A non-empty folder without a manifest.
    let other = f.drive.mkdirs("Other");
    f.drive.put_file("Other/x.txt", b"x");
    let e = ensure_manifest_v2(f.drive.as_ref(), &other, None, &mine, &tmp)
        .await
        .unwrap_err();
    assert!(e.to_string().contains(NEEDS_EMPTY_FOLDER), "{e}");
}

// ---------------------------------------------------------------- chunked edits

#[tokio::test]
async fn a_1mb_edit_of_a_64mib_file_uploads_little_and_leaves_no_orphans() {
    let f = Fixture::new();
    f.list().await;
    let size = 64 * 1024 * 1024;
    let mut content = pseudo(size, 64);
    let first = f.put(&f.folder, "video.bin", &content, None).await;
    let full_upload = f.drive.bytes_uploaded();
    assert!(full_upload >= size as u64);

    // Overwrite 1 MB (10^6 bytes) in the middle.
    let at = 30 * 1024 * 1024 + 12_345;
    content[at..at + 1_000_000].copy_from_slice(&pseudo(1_000_000, 99));
    let before = f.drive.bytes_uploaded();
    let calls_before = f.drive.calls("Upload");
    let second = f.put(&f.folder, "video.bin", &content, Some(&first.id)).await;
    let edit_bytes = f.drive.bytes_uploaded() - before;
    let edit_objects = f.drive.calls("Upload") - calls_before;
    println!(
        "vault v2 edit cost: 1,000,000 bytes changed in a 64 MiB file -> {edit_bytes} bytes uploaded \
         in {edit_objects} objects (chunks + manifest); the full upload was {full_upload} bytes"
    );
    assert!(edit_bytes < 32_000_000, "{edit_bytes} bytes for a 1 MB edit");
    assert!(edit_bytes < 3 * MAX_CHUNK_BYTES as u64, "{edit_bytes}");
    assert_eq!(first.id, second.id, "the manifest keeps its Drive file");
    assert_eq!(Some(size as u64), second.size);

    // No orphan chunks: what is on Drive is exactly what the manifest lists.
    let fresh = f.device("fresh");
    let report = fresh.sweep(&f.folder, None, Duration::ZERO).await.unwrap();
    assert_eq!(0, report.deleted, "{report:?}");
    assert_eq!(f.chunk_paths().len(), report.referenced);

    let listed = list(&fresh, &f.folder).await;
    assert_eq!(content, f.get(&fresh, &listed["video.bin"]).await.unwrap());
}

// ---------------------------------------------------------------- delete and sweep

#[tokio::test]
async fn delete_leaves_chunks_for_restore_until_the_sweep() {
    let f = Fixture::new();
    f.list().await;
    let keep = pseudo(5 * 1024 * 1024, 5);
    let gone = pseudo(6 * 1024 * 1024, 6);
    f.put(&f.folder, "keep.bin", &keep, None).await;
    let keep_chunks = f.chunk_paths();
    let g = f.put(&f.folder, "gone.bin", &gone, None).await;
    let all_chunks = f.chunk_paths();
    let gone_chunks = all_chunks.len() - keep_chunks.len();
    assert!(gone_chunks >= 2);

    f.vault.trash(&g.id).await.unwrap();
    assert!(f.drive.is_trashed(&g.id), "the manifest is in Drive's bin");
    assert_eq!(all_chunks, f.chunk_paths(), "chunks stay");

    // Young chunks survive a sweep: the file can come back from Drive's bin.
    let r = f
        .vault
        .sweep(&f.folder, None, Duration::from_secs(3600))
        .await
        .unwrap();
    assert_eq!((0, gone_chunks), (r.deleted, r.kept_recent), "{r:?}");
    f.vault.untrash(&g.id).await.unwrap();
    let listed = f.list().await;
    assert_eq!(gone, f.get(&f.vault, &listed["gone.bin"]).await.unwrap());

    // Binned again and past the grace: its chunks go, the other file's stay.
    f.vault.trash(&g.id).await.unwrap();
    let r = f.vault.sweep(&f.folder, None, Duration::ZERO).await.unwrap();
    assert_eq!(gone_chunks, r.deleted, "{r:?}");
    assert_eq!(keep_chunks, f.chunk_paths());
    let listed = f.list().await;
    assert_eq!(keep, f.get(&f.vault, &listed["keep.bin"]).await.unwrap());
}

#[tokio::test]
async fn an_old_file_deleted_now_keeps_its_chunks_for_the_whole_grace() {
    let f = Fixture::new();
    f.list().await;
    let grace = Duration::from_millis(1500);
    let before = f.chunk_paths();
    let g = f
        .put(&f.folder, "old.bin", &pseudo(3 * 1024 * 1024, 9), None)
        .await;
    let its_chunks = f.chunk_paths().len() - before.len();
    assert!(its_chunks >= 1);
    // The file is older than the grace by the time it is deleted.
    tokio::time::sleep(grace + Duration::from_millis(200)).await;
    f.vault.trash(&g.id).await.unwrap();

    // Unreferenced only now: kept, though its chunks were uploaded longer ago than the grace.
    let r = f.vault.sweep(&f.folder, None, grace).await.unwrap();
    assert_eq!((0, its_chunks), (r.deleted, r.kept_recent), "{r:?}");
    f.vault.untrash(&g.id).await.unwrap();
    let listed = f.list().await;
    assert!(listed.contains_key("old.bin"), "restorable from the bin");

    // Referenced again: forgotten as an orphan. Binned again, the grace starts over.
    let r = f.vault.sweep(&f.folder, None, grace).await.unwrap();
    assert_eq!(0, r.deleted, "{r:?}");
    f.vault.trash(&g.id).await.unwrap();
    let r = f.vault.sweep(&f.folder, None, grace).await.unwrap();
    assert_eq!((0, its_chunks), (r.deleted, r.kept_recent), "{r:?}");

    // A whole grace after the sweep that first saw them unreferenced, they go.
    tokio::time::sleep(grace + Duration::from_millis(200)).await;
    let r = f.vault.sweep(&f.folder, None, grace).await.unwrap();
    assert_eq!(its_chunks, r.deleted, "{r:?}");
}

#[tokio::test]
async fn the_sweep_refuses_to_run_when_a_manifest_cannot_be_read() {
    let f = Fixture::new();
    f.list().await;
    f.put(&f.folder, "a.bin", &pseudo(100_000, 1), None).await;
    f.put(&f.folder, "b.bin", &pseudo(100_000, 2), None).await;
    let listed = f.list().await;
    let path = f
        .drive
        .files()
        .into_keys()
        .find(|k| f.drive.id_of(k).as_deref() == Some(listed["a.bin"].id.as_str()));
    let path = path.unwrap();
    let mut bytes = f.drive.content(&path).unwrap();
    bytes[20] ^= 1;
    f.drive.put_file(&path, &bytes);
    let chunks = f.chunk_paths();
    let e = f.vault.sweep(&f.folder, None, Duration::ZERO).await.unwrap_err();
    assert!(matches!(e, DriveError::Integrity(_)), "{e}");
    assert_eq!(chunks, f.chunk_paths(), "nothing was deleted");
}

// ---------------------------------------------------------------- integrity

#[tokio::test]
async fn a_tampered_chunk_fails_that_file_only() {
    let (w, _v, _tmp) = v2_world(SWEEP_GRACE);
    let folder = w.pair().drive_folder_id.unwrap();
    let other = VaultV2Remote::new(w.drive.clone(), keys(), w.dir.path().join("t2"));
    ensure_manifest_v2(
        w.drive.as_ref(),
        &folder,
        None,
        &vault_json(&keys(), 7),
        &w.dir.path().join("t3"),
    )
    .await
    .unwrap();
    list(&other, &folder).await;
    let src = w.dir.path().join("src");
    std::fs::write(&src, pseudo(2_000_000, 3)).unwrap();
    put(&other, &src, &folder, "bad.bin", None).await;
    let bad_chunks: BTreeSet<String> = w
        .drive
        .files()
        .into_keys()
        .filter(|k| k.contains(CHUNKS_DIR))
        .collect();
    std::fs::write(&src, "fine").unwrap();
    put(&other, &src, &folder, "good.txt", None).await;

    let chunk = bad_chunks.iter().next().unwrap();
    let mut bytes = w.drive.content(chunk).unwrap();
    bytes[100] ^= 0xff;
    w.drive.put_file(chunk, &bytes);

    let run = w.sync().await;
    assert_ne!(RunStatus::Success, run.status);
    assert!(
        run.message
            .as_deref()
            .is_some_and(|m| m.starts_with("bad.bin: Couldn't download. Transferred data did not verify")),
        "{:?}",
        run.message
    );
    assert_eq!(Some("fine".into()), w.read("good.txt"));
    assert!(!w.path("bad.bin").exists(), "no unverified plaintext");
    assert!(
        w.local_files().keys().all(|k| k == "good.txt"),
        "{:?}",
        w.local_files().keys()
    );
}

#[tokio::test]
async fn a_swapped_manifest_is_refused() {
    let f = Fixture::new();
    f.list().await;
    f.put(&f.folder, "a.txt", b"content of a", None).await;
    f.put(&f.folder, "b.txt", b"content of b", None).await;
    let listed = f.list().await;
    let path_of = |id: &str| {
        f.drive
            .files()
            .into_keys()
            .find(|k| f.drive.id_of(k).as_deref() == Some(id))
            .unwrap()
    };
    let (pa, pb) = (path_of(&listed["a.txt"].id), path_of(&listed["b.txt"].id));
    let b_bytes = f.drive.content(&pb).unwrap();
    f.drive.put_file(&pa, &b_bytes);

    let listed = f.list().await;
    let e = f.get(&f.vault, &listed["a.txt"]).await.unwrap_err();
    assert!(matches!(e, DriveError::Integrity(_)), "{e}");
    assert_eq!(
        b"content of b".to_vec(),
        f.get(&f.vault, &listed["b.txt"]).await.unwrap()
    );

    // The same for a listing blob moved to another file: it does not verify, so the manifest
    // is read instead (and refused for a.txt).
    let blob = f.drive.properties(&listed["b.txt"].id)[META_PROPERTY].clone();
    f.drive
        .set_property(&listed["a.txt"].id, META_PROPERTY, Some(&blob));
    let listed = f.list().await;
    assert_eq!(None, listed["a.txt"].size, "no size from a foreign blob");
}

#[tokio::test]
async fn a_rolled_back_manifest_is_refused_also_after_a_restart() {
    let f = Fixture::new();
    let store = SharedStore::new(Store::open_in_memory().unwrap());
    store
        .lock()
        .create_pair(&Pair::new("p1", "Vault", "/tmp/vault"))
        .unwrap();
    let vault = f.device("a").with_store(store.clone(), "p1");
    list(&vault, &f.folder).await;
    let v1 = put(&vault, &f.src(b"version one"), &f.folder, "doc.txt", None).await;
    let path = f
        .drive
        .files()
        .into_keys()
        .find(|k| f.drive.id_of(k).as_deref() == Some(v1.id.as_str()))
        .unwrap();
    let old = f.drive.content(&path).unwrap();
    put(
        &vault,
        &f.src(b"version two!"),
        &f.folder,
        "doc.txt",
        Some(&v1.id),
    )
    .await;

    // Someone with write access puts the first manifest back.
    f.drive.put_file(&path, &old);
    let listed = list(&vault, &f.folder).await;
    let e = f.get(&vault, &listed["doc.txt"]).await.unwrap_err();
    assert!(e.to_string().contains("rolled back"), "{e}");

    // A restarted device with the same store still refuses it. One without history cannot tell a
    // rollback, but the old version's chunks were deleted with the update, so it cannot read it
    // either (and says so instead of producing old content).
    let restarted = f.device("b").with_store(store, "p1");
    let listed = list(&restarted, &f.folder).await;
    let e = f.get(&restarted, &listed["doc.txt"]).await.unwrap_err();
    assert!(matches!(e, DriveError::Integrity(_)), "{e}");
    let naive = f.device("c");
    let listed = list(&naive, &f.folder).await;
    let e = f.get(&naive, &listed["doc.txt"]).await.unwrap_err();
    assert!(e.to_string().contains("missing"), "{e}");
}

// ---------------------------------------------------------------- folders, rename, move

#[tokio::test]
async fn rename_and_move_reseal_files_and_folders() {
    let f = Fixture::new();
    f.list().await;
    let a = f.vault.create_folder(&f.folder, "أ").await.unwrap();
    let b = f.vault.create_folder(&a.id, "B").await.unwrap();
    f.put(&b.id, "inner.txt", b"inner", None).await;
    let file = f.put(&b.id, "f.txt", &pseudo(300_000, 7), None).await;
    let chunks = f.chunk_paths();

    // A file moves to the root under a new name: same chunks, new manifest.
    let moved = f.vault.move_item(&file.id, &f.folder, "g.txt").await.unwrap();
    assert_eq!("g.txt", moved.name);
    assert!(f.drive.is_trashed(&file.id), "the old manifest is in the bin");
    assert_eq!(chunks, f.chunk_paths(), "no chunk moves or uploads");

    // A folder moves to the root and is renamed; its children keep their names.
    let c = f.vault.move_item(&b.id, &f.folder, "C").await.unwrap();
    assert_eq!(b.id, c.id, "folders keep their Drive id");

    let fresh = f.device("fresh");
    let listed = list(&fresh, &f.folder).await;
    let keys_now: Vec<&str> = listed.keys().map(String::as_str).collect();
    assert_eq!(vec!["C", "C/inner.txt", "g.txt", "أ"], keys_now);
    assert_eq!(pseudo(300_000, 7), f.get(&fresh, &listed["g.txt"]).await.unwrap());
    assert_eq!(
        b"inner".to_vec(),
        f.get(&fresh, &listed["C/inner.txt"]).await.unwrap()
    );

    // Without properties (a plain Drive download loses them) the re-sealed `.wasl-dir` names the
    // moved folder's children, and manifests give the sizes.
    f.drive.set_property(&c.id, DIR_PROPERTY, None);
    f.drive.set_property(&listed["g.txt"].id, META_PROPERTY, None);
    let again = list(&f.device("props-gone"), &f.folder).await;
    assert!(again.contains_key("C/inner.txt"), "{:?}", again.keys());
    assert_eq!(Some(300_000), again["g.txt"].size);
    assert_eq!(listed["g.txt"].mtime, again["g.txt"].mtime);

    // The recovery reader agrees.
    let recovered = recover(&f.drive, "Vault/", &keys(), f.dir.path());
    let names: Vec<&str> = recovered.keys().map(String::as_str).collect();
    assert_eq!(vec!["C/inner.txt", "g.txt"], names);
}

#[tokio::test]
async fn foreign_items_and_unverified_folders_are_skipped_never_deleted() {
    let f = Fixture::new();
    f.list().await;
    let d = f.vault.create_folder(&f.folder, "Docs").await.unwrap();
    f.put(&d.id, "x.txt", b"x", None).await;
    f.drive.put_file("Vault/dropped.txt", b"plain");
    f.drive
        .put_google_doc("Vault/sheet", "application/vnd.google-apps.spreadsheet");
    let other = VaultV2Remote::new(f.drive.clone(), other_keys(), f.dir.path().join("o"));
    list(&other, &f.folder).await;
    put(&other, &f.src(b"theirs"), &f.folder, "theirs.txt", None).await;

    let listed = f.list().await;
    assert_eq!(
        vec!["Docs", "Docs/x.txt"],
        listed.keys().map(String::as_str).collect::<Vec<_>>()
    );
    let skips = f.vault.last_skips();
    // Foreign: dropped.txt, theirs.txt and the other vault's state record.
    assert_eq!(
        (3, 1, 0),
        (skips.foreign, skips.google_docs, skips.unverified_folders),
        "{skips:?}"
    );

    // A folder whose id was tampered with (property and marker) is skipped with its contents.
    f.drive.set_property(&d.id, DIR_PROPERTY, Some(&"A".repeat(64)));
    let marker = f
        .drive
        .files()
        .into_keys()
        .find(|k| k.ends_with(DIR_MARKER_NAME))
        .unwrap();
    f.drive.put_file(&marker, b"W2D1 not a marker");
    let listed = f.list().await;
    assert!(listed.is_empty(), "{:?}", listed.keys());
    assert_eq!(1, f.vault.last_skips().unverified_folders);
    assert!(f.drive.content("Vault/dropped.txt").is_some());
}

// ---------------------------------------------------------------- files on demand

#[tokio::test]
async fn the_on_demand_source_reads_a_deep_range_fetching_only_its_chunks() {
    let f = Fixture::new();
    f.list().await;
    let size = 40 * 1024 * 1024;
    let content = pseudo(size, 40);
    f.put(&f.folder, "movie.bin", &content, None).await;
    let reader = f.device("reader");
    let entry = list(&reader, &f.folder).await["movie.bin"].clone();
    let file = FileRef {
        id: entry.id.clone(),
        version: entry.version.unwrap().to_string(),
        size: entry.size.unwrap(),
    };
    assert_eq!(size as u64, file.size);

    let (offset, len) = (35 * 1024 * 1024 + 777, 4096u64);
    let down_before = f.drive.bytes_downloaded();
    let calls_before = f.drive.calls("Download");
    let got = reader.read(&file, offset, len).await.unwrap();
    assert_eq!(
        &content[offset as usize..offset as usize + len as usize],
        &got[..]
    );
    let fetched = f.drive.bytes_downloaded() - down_before;
    let objects = f.drive.calls("Download") - calls_before;
    // The manifest plus the chunk(s) covering 4 KiB: one, or two at a boundary.
    assert!((2..=3).contains(&objects), "{objects} objects");
    assert!(
        fetched < 2 * MAX_CHUNK_BYTES as u64 + 64 * 1024,
        "{fetched} bytes"
    );

    // Once the manifest is known, the natural range is the covering chunk(s), and a second read
    // fetches only chunks.
    let (start, n) = reader.natural_range(&file, offset, len);
    assert!(start <= offset && start + n >= offset + len, "{start} {n}");
    assert!(n <= 2 * MAX_CHUNK_BYTES as u64);
    let calls_before = f.drive.calls("Download");
    let whole = reader.read(&file, start, n).await.unwrap();
    assert_eq!(&content[start as usize..(start + n) as usize], &whole[..]);
    assert_eq!(objects - 1, f.drive.calls("Download") - calls_before);

    // The end of the file and past it.
    let tail = reader.read(&file, size as u64 - 10, 100).await.unwrap();
    assert_eq!(&content[size - 10..], &tail[..]);
    assert!(reader.read(&file, size as u64 + 5, 10).await.unwrap().is_empty());
}

// ---------------------------------------------------------------- object size bounds

/// Security review 2026-10: anyone with write access to the folder can replace a vault object
/// with something huge. An object listed bigger than its format allows is never downloaded.
#[tokio::test]
async fn objects_listed_bigger_than_their_format_allows_are_not_downloaded() {
    let f = Fixture::new();
    f.list().await;
    let d = f.vault.create_folder(&f.folder, "Docs").await.unwrap();
    f.put(&f.folder, "chunked.bin", &pseudo(300_000, 9), None).await;
    let chunk = f.chunk_paths().into_iter().next().unwrap();
    f.put(&f.folder, "small.txt", b"small", None).await;

    // A chunk one byte over a sealed full chunk.
    f.drive
        .put_file(&format!("Vault/{chunk}"), &vec![0u8; MAX_SEALED_BYTES + 1]);
    let reader = f.device("r1");
    let listed = list(&reader, &f.folder).await;
    let before = f.drive.bytes_downloaded();
    let err = f.get(&reader, &listed["chunked.bin"]).await.unwrap_err();
    assert!(matches!(err, DriveError::Integrity(_)), "{err:?}");
    let fetched = f.drive.bytes_downloaded() - before;
    assert!(fetched < MAX_SEALED_BYTES as u64, "{fetched} bytes downloaded");

    // A manifest one byte over the manifest maximum.
    let small_id = listed["small.txt"].id.clone();
    let manifest = f
        .raw()
        .into_keys()
        .find(|k| f.drive.id_of(&format!("Vault/{k}")).as_deref() == Some(small_id.as_str()))
        .unwrap();
    f.drive
        .put_file(&format!("Vault/{manifest}"), &vec![0u8; MAX_MANIFEST_BYTES + 1]);
    let reader = f.device("r2");
    let listed = list(&reader, &f.folder).await;
    let before = f.drive.bytes_downloaded();
    let err = f.get(&reader, &listed["small.txt"]).await.unwrap_err();
    assert!(matches!(err, DriveError::Integrity(_)), "{err:?}");
    assert_eq!(before, f.drive.bytes_downloaded());

    // A `.wasl-dir` marker of the wrong size: the folder stays unverified, nothing is read.
    f.drive.set_property(&d.id, DIR_PROPERTY, None);
    let marker = f
        .drive
        .files()
        .into_keys()
        .find(|k| k.ends_with(DIR_MARKER_NAME))
        .unwrap();
    f.drive.put_file(&marker, &[b'W'; DIR_MARKER_BYTES + 1]);
    let reader = f.device("r3");
    let before = f.drive.bytes_downloaded();
    let listed = list(&reader, &f.folder).await;
    assert!(!listed.contains_key("Docs"), "{:?}", listed.keys());
    assert_eq!(1, reader.last_skips().unverified_folders);
    // The listing read the writer's state record and nothing else.
    assert_eq!(before + RECORD_BYTES as u64, f.drive.bytes_downloaded());
}

/// A remote whose chunk downloads never end, whatever size it listed.
struct Endless {
    inner: Arc<MemoryRemote>,
    written: AtomicU64,
}

/// Where [`Endless`] gives up when nothing stops it.
const ENDLESS_LIMIT: u64 = 32 << 20;

#[async_trait]
impl Remote for Endless {
    async fn list_subtree(&self, root_id: &str, drive_id: Option<&str>) -> wasl_drive::Result<Listing> {
        self.inner.list_subtree(root_id, drive_id).await
    }
    async fn download(
        &self,
        entry: &RemoteEntry,
        dest: &Path,
        progress: ProgressFn<'_>,
    ) -> wasl_drive::Result<()> {
        if ChunkId::from_object_name(&entry.name).is_none() {
            return self.inner.download(entry, dest, progress).await;
        }
        let mut file = std::fs::File::create(dest).unwrap();
        let piece = vec![0u8; 1 << 20];
        let mut done = 0u64;
        while done < ENDLESS_LIMIT {
            std::io::Write::write_all(&mut file, &piece).unwrap();
            done += piece.len() as u64;
            self.written.fetch_max(done, Ordering::SeqCst);
            progress(done);
            tokio::task::yield_now().await;
        }
        Ok(())
    }
    async fn export(
        &self,
        entry: &RemoteEntry,
        mime: &str,
        dest: &Path,
        progress: ProgressFn<'_>,
    ) -> wasl_drive::Result<()> {
        self.inner.export(entry, mime, dest, progress).await
    }
    async fn upload(&self, spec: UploadSpec<'_>, progress: ProgressFn<'_>) -> wasl_drive::Result<RemoteInfo> {
        self.inner.upload(spec, progress).await
    }
    async fn create_folder(&self, parent_id: &str, name: &str) -> wasl_drive::Result<RemoteInfo> {
        self.inner.create_folder(parent_id, name).await
    }
    async fn trash(&self, id: &str) -> wasl_drive::Result<()> {
        self.inner.trash(id).await
    }
    async fn untrash(&self, id: &str) -> wasl_drive::Result<()> {
        self.inner.untrash(id).await
    }
}

/// Security review 2026-10: a listed size can lie; the bytes actually written are capped too.
#[tokio::test]
async fn a_chunk_download_that_does_not_stop_is_cut_off() {
    let f = Fixture::new();
    f.list().await;
    f.put(&f.folder, "a.bin", &pseudo(300_000, 11), None).await;
    let endless = Arc::new(Endless {
        inner: f.drive.clone(),
        written: AtomicU64::new(0),
    });
    let reader = VaultV2Remote::new(endless.clone(), keys(), f.dir.path().join("endless"));
    let listed = list(&reader, &f.folder).await;
    let err = f.get(&reader, &listed["a.bin"]).await.unwrap_err();
    assert!(matches!(err, DriveError::Integrity(_)), "{err:?}");
    let written = endless.written.load(Ordering::SeqCst);
    assert!(
        written <= MAX_SEALED_BYTES as u64 + (2 << 20),
        "{written} bytes written"
    );
}
