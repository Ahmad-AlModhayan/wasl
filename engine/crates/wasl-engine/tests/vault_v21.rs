//! Vault format 2.1 in the engine (security review S4): the revision a vault is written in, Drive
//! times that say nothing, state records that catch a rolled-back folder, and copying a vault into
//! a new 2.1 one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use wasl_core::RemoteEntry;
use wasl_crypto::v2::chunks::{CHUNKS_DIR, MAGIC, MAGIC_PADDED};
use wasl_crypto::v2::{Format, KdfParams, VaultKeysV2, VaultManifestV2};
use wasl_crypto::{VaultKeys, VaultManifest, kdf};
use wasl_drive::DriveError;
use wasl_engine::{
    MemoryRemote, Op, Remote, StateCheck, UploadSpec, VAULT_ROLLED_BACK, VaultRemote, VaultV2Remote,
    copy_vault,
};
use wasl_store::{Pair, SharedStore, Store};

const MTIME: i64 = 1_600_000_000_000;

fn keys() -> Arc<VaultKeysV2> {
    Arc::new(VaultKeysV2::from_ikm(&[0x42; 32]))
}

fn manifest(format: Format, k: &VaultKeysV2) -> VaultManifestV2 {
    VaultManifestV2::create_as(format, "T", &[5; 16], KdfParams::MIN, &k.key_check(), 0)
}

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

struct Fixture {
    dir: tempfile::TempDir,
    drive: Arc<MemoryRemote>,
    folder: String,
}

impl Fixture {
    fn new() -> Fixture {
        let drive = Arc::new(MemoryRemote::new());
        let folder = drive.mkdirs("Vault");
        Fixture {
            dir: tempfile::tempdir().unwrap(),
            drive,
            folder,
        }
    }

    fn device(&self, name: &str, format: Format) -> VaultV2Remote {
        let k = keys();
        let m = manifest(format, &k);
        VaultV2Remote::new(self.drive.clone(), k, self.dir.path().join(name)).with_manifest(m)
    }

    fn src(&self, name: &str, content: &[u8]) -> PathBuf {
        let p = self.dir.path().join("src").join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, content).unwrap();
        p
    }

    /// Drive paths under the vault folder → content.
    fn raw(&self, prefix: &str) -> BTreeMap<String, Vec<u8>> {
        self.drive
            .files()
            .into_iter()
            .filter_map(|(k, v)| k.strip_prefix(prefix).map(|k| (k.to_owned(), v)))
            .collect()
    }
}

async fn list(v: &dyn Remote, folder: &str) -> Result<BTreeMap<String, RemoteEntry>, DriveError> {
    v.prepare(folder, None).await?;
    Ok(v.list_subtree(folder, None)
        .await?
        .entries
        .into_iter()
        .map(|e| (e.key.clone(), e))
        .collect())
}

async fn put(v: &dyn Remote, src: &Path, parent: &str, name: &str, existing: Option<&str>) -> String {
    v.upload(
        UploadSpec {
            src,
            name,
            parent_id: parent,
            existing_id: existing,
            mtime_ms: MTIME,
            session_key: None,
        },
        &|_| {},
    )
    .await
    .unwrap()
    .id
}

async fn get(v: &dyn Remote, entry: &RemoteEntry, dir: &Path) -> Vec<u8> {
    let dest = dir.join("out").join(&entry.id);
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    v.download(entry, &dest, &|_| {}).await.unwrap();
    std::fs::read(dest).unwrap()
}

fn store() -> SharedStore {
    let store = SharedStore::new(Store::open_in_memory().unwrap());
    store
        .lock()
        .create_pair(&Pair::new("p1", "Vault", "/tmp/vault"))
        .unwrap();
    store
}

// ---------------------------------------------------------------- revisions

#[tokio::test]
async fn a_2_0_vault_keeps_writing_2_0_and_a_2_1_vault_pads_and_records_its_state() {
    for (format, magic, records) in [(Format::V2_0, MAGIC, 0), (Format::V2_1, MAGIC_PADDED, 1)] {
        let f = Fixture::new();
        let v = f.device("a", format);
        list(&v, &f.folder).await.unwrap();
        put(&v, &f.src("big", &pseudo(5 << 20, 1)), &f.folder, "big.bin", None).await;
        put(&v, &f.src("small", b"tiny"), &f.folder, "small.txt", None).await;
        let raw = f.raw("Vault/");
        let chunks: Vec<&Vec<u8>> = raw
            .iter()
            .filter(|(k, _)| k.starts_with(CHUNKS_DIR))
            .map(|(_, v)| v)
            .collect();
        assert!(chunks.len() >= 3, "{format:?}");
        assert!(chunks.iter().all(|c| c[..4] == magic), "{format:?}");
        if format == Format::V2_1 {
            // Padded: a 4-byte file is stored as a 4 KiB body.
            assert!(chunks.iter().all(|c| c.len() >= 4096 + 32), "{format:?}");
        }
        let state = raw.keys().filter(|k| k.starts_with(".wasl-state.")).count();
        assert_eq!(records, state, "{format:?}");

        // Both read back, through a reader that has no vault.json at all.
        let reader = VaultV2Remote::new(f.drive.clone(), keys(), f.dir.path().join("r"));
        let listed = list(&reader, &f.folder).await.unwrap();
        assert_eq!(
            b"tiny".to_vec(),
            get(&reader, &listed["small.txt"], f.dir.path()).await
        );
        assert_eq!(
            5 << 20,
            get(&reader, &listed["big.bin"], f.dir.path()).await.len()
        );
    }
}

#[tokio::test]
async fn drive_times_are_upload_times_and_listings_keep_the_files_own() {
    let f = Fixture::new();
    let v = f.device("a", Format::V2_1);
    list(&v, &f.folder).await.unwrap();
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let id = put(&v, &f.src("a", b"alpha"), &f.folder, "a.txt", None).await;

    // What Drive (the plain listing) shows: the upload's time, not 2020.
    let raw = f.drive.list_subtree(&f.folder, None).await.unwrap();
    let on_drive = raw.entries.iter().find(|e| e.id == id).unwrap();
    assert!(on_drive.mtime >= before, "{} < {before}", on_drive.mtime);

    // What every device lists: the file's own time, from the encrypted listing blob.
    let other = f.device("b", Format::V2_1);
    let listed = list(&other, &f.folder).await.unwrap();
    assert_eq!(MTIME, listed["a.txt"].mtime);
    // A rename keeps it, too.
    other
        .move_item(&listed["a.txt"].id, &f.folder, "b.txt")
        .await
        .unwrap();
    let listed = list(&v, &f.folder).await.unwrap();
    assert_eq!(MTIME, listed["b.txt"].mtime);
    let raw = f.drive.list_subtree(&f.folder, None).await.unwrap();
    assert!(raw.entries.iter().all(|e| e.mtime != MTIME));
}

// ---------------------------------------------------------------- rollback (S4-4)

#[tokio::test]
async fn a_folder_put_back_to_an_earlier_state_is_reported_and_not_applied() {
    let f = Fixture::new();
    let writer = f.device("w", Format::V2_1);
    list(&writer, &f.folder).await.unwrap();
    put(&writer, &f.src("1", b"one"), &f.folder, "one.txt", None).await;
    let earlier = f.raw("");

    let store = store();
    let reader = f.device("r", Format::V2_1).with_store(store.clone(), "p1");
    list(&reader, &f.folder).await.unwrap();
    assert_eq!(StateCheck::Consistent, reader.last_state_check());

    // The writer goes on: a file deleted, another added.
    let listed = list(&writer, &f.folder).await.unwrap();
    writer.trash(&listed["one.txt"].id).await.unwrap();
    put(&writer, &f.src("2", b"two"), &f.folder, "two.txt", None).await;
    let now = list(&reader, &f.folder).await.unwrap();
    assert_eq!(vec!["two.txt"], now.keys().collect::<Vec<_>>());
    assert_eq!(StateCheck::Consistent, reader.last_state_check());

    // Someone with access to Drive puts every object back as it was, records included: the
    // deleted file reappears and the new one is gone, all genuine.
    for (path, bytes) in &earlier {
        f.drive.put_file(path, bytes);
    }
    for (path, _) in f.raw("") {
        if !earlier.contains_key(&path) {
            f.drive.trash_path(&path);
        }
    }
    let err = list(&reader, &f.folder).await.unwrap_err();
    assert!(err.to_string().contains(VAULT_ROLLED_BACK), "{err}");
    // Also after a restart (the store remembers), and nothing is listed, so nothing is applied.
    let restarted = f.device("r2", Format::V2_1).with_store(store, "p1");
    let err = list(&restarted, &f.folder).await.unwrap_err();
    assert!(matches!(err, DriveError::Integrity(_)), "{err}");
}

#[tokio::test]
async fn a_removed_state_record_is_reported() {
    let f = Fixture::new();
    let writer = f.device("w", Format::V2_1);
    list(&writer, &f.folder).await.unwrap();
    put(&writer, &f.src("1", b"one"), &f.folder, "one.txt", None).await;
    let reader = f.device("r", Format::V2_1);
    list(&reader, &f.folder).await.unwrap();

    let record = f
        .raw("")
        .into_keys()
        .find(|k| k.contains(".wasl-state."))
        .unwrap();
    // Replaced by junk: skipped as foreign, so the record is gone, which is reported.
    f.drive.put_file(&record, b"junk");
    let err = list(&reader, &f.folder).await.unwrap_err();
    assert!(err.to_string().contains(VAULT_ROLLED_BACK), "{err}");
    // Removed outright: the same.
    f.drive.trash_path(&record);
    let err = list(&reader, &f.folder).await.unwrap_err();
    assert!(err.to_string().contains(VAULT_ROLLED_BACK), "{err}");
    // A device that never saw it cannot tell (the documented limit).
    let fresh = f.device("fresh", Format::V2_1);
    assert!(list(&fresh, &f.folder).await.is_ok());
}

#[tokio::test]
async fn a_deleted_file_brought_back_with_the_records_kept_is_flagged() {
    let f = Fixture::new();
    let writer = f.device("w", Format::V2_1);
    list(&writer, &f.folder).await.unwrap();
    put(&writer, &f.src("1", b"one"), &f.folder, "one.txt", None).await;
    let listed = list(&writer, &f.folder).await.unwrap();
    writer.trash(&listed["one.txt"].id).await.unwrap();

    let reader = f.device("r", Format::V2_1);
    assert!(list(&reader, &f.folder).await.unwrap().is_empty());
    assert_eq!(StateCheck::Consistent, reader.last_state_check());

    // Brought back from Drive's bin by someone else: listed (it might be the person restoring it
    // on purpose), but the listing no longer matches what any writer published.
    f.drive.untrash(&listed["one.txt"].id).await.unwrap();
    let again = list(&reader, &f.folder).await.unwrap();
    assert!(again.contains_key("one.txt"));
    assert_eq!(StateCheck::Unpublished, reader.last_state_check());

    // The next write by a device that sees it publishes the state again.
    put(&reader, &f.src("2", b"two"), &f.folder, "two.txt", None).await;
    list(&writer, &f.folder).await.unwrap();
    assert_eq!(StateCheck::Consistent, writer.last_state_check());
}

#[tokio::test]
async fn concurrent_writers_never_trip_the_rollback_check() {
    let f = Fixture::new();
    let a = f.device("a", Format::V2_1);
    let b = f.device("b", Format::V2_1);
    let reader = f.device("r", Format::V2_1);
    list(&a, &f.folder).await.unwrap();
    list(&b, &f.folder).await.unwrap();
    for i in 0..4u8 {
        // Each writes from a stale listing of the other's work.
        put(
            &a,
            &f.src(&format!("a{i}"), &[i]),
            &f.folder,
            &format!("a{i}.txt"),
            None,
        )
        .await;
        put(
            &b,
            &f.src(&format!("b{i}"), &[i, i]),
            &f.folder,
            &format!("b{i}.txt"),
            None,
        )
        .await;
        assert!(list(&reader, &f.folder).await.is_ok());
    }
    assert_eq!(8, list(&reader, &f.folder).await.unwrap().len());
    let records = f
        .raw("Vault/")
        .keys()
        .filter(|k| k.starts_with(".wasl-state."))
        .count();
    assert_eq!(2, records);
}

// ---------------------------------------------------------------- upgrade (S4-2)

#[tokio::test]
async fn a_v1_vault_is_copied_into_a_new_2_1_vault_and_a_rerun_continues() {
    let f = Fixture::new();
    // The old v1 vault, as an earlier version made it.
    let v1_keys = Arc::new(VaultKeys::new([0x11; 32], [0x22; 32]));
    let v1_json = VaultManifest::create("Old", &[3; 16], 100_000, &kdf::key_check(&v1_keys), 0);
    f.drive.put_file("Vault/vault.json", v1_json.to_json().as_bytes());
    let old = VaultRemote::new(f.drive.clone(), v1_keys, f.dir.path().join("old")).with_manifest(v1_json);
    list(&old, &f.folder).await.unwrap();
    let docs = old.create_folder(&f.folder, "Docs").await.unwrap().id;
    let deep = old.create_folder(&docs, "ملفات").await.unwrap().id;
    let big = pseudo(6 << 20, 3);
    put(&old, &f.src("a", b"alpha"), &f.folder, "a.txt", None).await;
    put(&old, &f.src("b", &big), &deep, "big.bin", None).await;
    put(&old, &f.src("c", b""), &docs, "empty.txt", None).await;
    let old_before = f.raw("Vault/");

    // The new vault, in a new folder; the first attempt stops on a network error.
    let new_folder = f.drive.mkdirs("Vault 2.1");
    let new = f.device("new", Format::V2_1);
    let tmp = f.dir.path().join("copy");
    f.drive
        .fail_next(Op::Upload, DriveError::Network("offline".into()));
    let first = copy_vault(&old, &f.folder, None, &new, &new_folder, None, &tmp).await;
    assert!(matches!(first, Err(DriveError::Network(_))), "{first:?}");

    let report = copy_vault(&old, &f.folder, None, &new, &new_folder, None, &tmp)
        .await
        .unwrap();
    assert!(
        report.failed.is_empty() && report.left_out.is_empty(),
        "{report:?}"
    );
    assert_eq!((2, 3), (report.folders, report.files), "{report:?}");

    // Interrupted after some files (here: one is missing again): a rerun copies only that one.
    let listed = list(&new, &new_folder).await.unwrap();
    new.trash(&listed["a.txt"].id).await.unwrap();
    let resumed = copy_vault(&old, &f.folder, None, &new, &new_folder, None, &tmp)
        .await
        .unwrap();
    assert_eq!(
        (0, 1, 2),
        (resumed.folders, resumed.files, resumed.already_there),
        "{resumed:?}"
    );

    // Everything reads back from the new vault, with its time, through a fresh device.
    let reader = VaultV2Remote::new(f.drive.clone(), keys(), f.dir.path().join("r"));
    let listed = list(&reader, &new_folder).await.unwrap();
    assert_eq!(
        vec![
            "Docs",
            "Docs/empty.txt",
            "Docs/ملفات",
            "Docs/ملفات/big.bin",
            "a.txt"
        ],
        listed.keys().map(String::as_str).collect::<Vec<_>>()
    );
    assert_eq!(
        b"alpha".to_vec(),
        get(&reader, &listed["a.txt"], f.dir.path()).await
    );
    assert_eq!(
        big,
        get(&reader, &listed["Docs/ملفات/big.bin"], f.dir.path()).await
    );
    assert_eq!(MTIME, listed["a.txt"].mtime);
    let new_json = f.drive.content("Vault 2.1/vault.json").unwrap();
    let new_json = VaultManifestV2::parse(std::str::from_utf8(&new_json).unwrap()).unwrap();
    assert_eq!(Format::V2_1, new_json.format());

    // Run again: nothing to copy. The old folder is exactly as it was; no plaintext left behind.
    let again = copy_vault(&old, &f.folder, None, &new, &new_folder, None, &tmp)
        .await
        .unwrap();
    assert_eq!((0, 3), (again.files, again.already_there), "{again:?}");
    assert_eq!(old_before, f.raw("Vault/"));
    assert_eq!(0, std::fs::read_dir(&tmp).unwrap().count());
}

#[tokio::test]
async fn a_2_0_vault_is_copied_into_2_1_and_a_damaged_file_is_reported_not_fatal() {
    let f = Fixture::new();
    let old = f.device("old", Format::V2_0);
    list(&old, &f.folder).await.unwrap();
    let chunks = |f: &Fixture| -> Vec<String> {
        f.raw("")
            .into_keys()
            .filter(|k| k.starts_with(&format!("Vault/{CHUNKS_DIR}/")))
            .collect()
    };
    put(&old, &f.src("a", b"alpha"), &f.folder, "a.txt", None).await;
    let before = chunks(&f);
    put(&old, &f.src("b", &pseudo(300_000, 1)), &f.folder, "b.bin", None).await;
    // b.bin's chunk is damaged on Drive.
    let chunk = chunks(&f).into_iter().find(|c| !before.contains(c)).unwrap();
    f.drive.put_file(&chunk, b"W2C1 damaged");

    let new_folder = f.drive.mkdirs("New");
    let other_keys = Arc::new(VaultKeysV2::from_ikm(&[0x44; 32]));
    let new = VaultV2Remote::new(f.drive.clone(), other_keys.clone(), f.dir.path().join("n"))
        .with_manifest(manifest(Format::V2_1, &other_keys));
    let report = copy_vault(
        &old,
        &f.folder,
        None,
        &new,
        &new_folder,
        None,
        &f.dir.path().join("t"),
    )
    .await
    .unwrap();
    assert_eq!(1, report.files);
    assert_eq!(vec!["b.bin".to_owned()], report.failed);
}

#[tokio::test]
async fn a_restarted_writer_goes_on_with_its_own_record() {
    let f = Fixture::new();
    let store = store();
    let first = f.device("a", Format::V2_1).with_store(store.clone(), "p1");
    list(&first, &f.folder).await.unwrap();
    put(&first, &f.src("1", b"one"), &f.folder, "one.txt", None).await;
    let records = |f: &Fixture| -> Vec<String> {
        f.raw("Vault/")
            .into_keys()
            .filter(|k| k.starts_with(".wasl-state."))
            .collect()
    };
    let before = records(&f);
    assert_eq!(1, before.len());
    drop(first);
    let again = f.device("b", Format::V2_1).with_store(store, "p1");
    list(&again, &f.folder).await.unwrap();
    put(&again, &f.src("2", b"two"), &f.folder, "two.txt", None).await;
    assert_eq!(before, records(&f), "the same writer, the same record");
    let reader = f.device("r", Format::V2_1);
    list(&reader, &f.folder).await.unwrap();
    assert_eq!(StateCheck::Consistent, reader.last_state_check());
}
