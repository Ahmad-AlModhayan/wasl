//! Files on demand through the phone's binding (docs/ON_DEMAND.md §2.2): the reader asks Drive for
//! the one block a seek needs, decrypts only the vault v1 chunk or the vault v2 chunks a range
//! covers, pins keep files whole, and a vault file never reads blocks cached for its ciphertext.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::Notify;
use wasl_core::RemoteEntry;
use wasl_core::conditions::DeviceState;
use wasl_crypto::VaultKeys;
use wasl_crypto::content::{CHUNK_BYTES, HEADER_BYTES};
use wasl_crypto::v2::VaultKeysV2;
use wasl_crypto::v2::chunks::MAX_CHUNK_BYTES;
use wasl_engine::{
    Listing, MemoryRemote, ProgressFn, Remote, RemoteInfo, RemoteSource, UploadSpec, VaultRemote,
    VaultV2Remote,
};
use wasl_mobile::{
    EngineInit, OnDemandFile, OnDemandOrigin, OnDemandReader, OnDemandRemotes, Platform, SafBridge,
    TokenSource, VaultAccess, VaultItem, WaslEngine, WaslError,
};
use wasl_vfs::{BLOCK, Source};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const ACCOUNT: &str = "me@example.com";
/// What the reader reads from a v2 vault.json: its format (the keys come as material).
const V2_JSON: &str = r#"{"format":"wasl-vault","version":2}"#;

/// Deterministic bytes that do not repeat (so vault v2 chunk cuts are realistic).
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

// ---------------------------------------------------------------- plain Drive files

struct Tokens;

impl TokenSource for Tokens {
    fn access_token(&self, _account: String) -> Result<String, WaslError> {
        Ok("tok".into())
    }
    fn invalidate(&self, _account: String) {}
}

struct Device;

impl Platform for Device {
    fn device_state(&self) -> DeviceState {
        DeviceState::unrestricted()
    }
    fn saf(&self) -> Option<Arc<dyn SafBridge>> {
        None
    }
    fn vault_access(&self, _vault_id: String) -> Option<VaultAccess> {
        None
    }
}

/// Serves `alt=media` with Range support and records the ranges asked for.
struct Ranged {
    body: Vec<u8>,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Respond for Ranged {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let range = req
            .headers
            .get("range")
            .map(|v| v.to_str().unwrap().to_owned())
            .unwrap_or_default();
        self.seen.lock().unwrap().push(range.clone());
        let (a, b) = range.trim_start_matches("bytes=").split_once('-').unwrap();
        let a: usize = a.parse().unwrap();
        if a >= self.body.len() {
            return ResponseTemplate::new(416);
        }
        let b: usize = b.parse::<usize>().unwrap().min(self.body.len() - 1);
        ResponseTemplate::new(206).set_body_bytes(self.body[a..=b].to_vec())
    }
}

fn init(dir: &Path, server: &MockServer) -> EngineInit {
    EngineInit {
        db_path: dir.join("wasl.db").to_string_lossy().into_owned(),
        bin_dir: dir.join("bin").to_string_lossy().into_owned(),
        staging_dir: dir.join("staging").to_string_lossy().into_owned(),
        device_name: "Pixel".into(),
        drive_base_url: Some(format!("{}/drive/v3", server.uri())),
        drive_upload_url: Some(format!("{}/upload/drive/v3", server.uri())),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_seek_into_a_drive_file_asks_drive_for_one_block_and_a_pin_keeps_it_whole() {
    let server = MockServer::start().await;
    let size = 12 * BLOCK + 345;
    let body = pseudo(size as usize, 1);
    let seen = Arc::new(Mutex::new(Vec::new()));
    Mock::given(method("GET"))
        .and(path("/drive/v3/files/F1"))
        .and(query_param("alt", "media"))
        .respond_with(Ranged {
            body: body.clone(),
            seen: seen.clone(),
        })
        .mount(&server)
        .await;
    // After every range read the source checks the file is still the listed version.
    Mock::given(method("GET"))
        .and(path("/drive/v3/files/F1"))
        .and(query_param("fields", "id,version,md5Checksum"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "F1", "version": "7"})),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let engine = WaslEngine::new(
        init(dir.path(), &server),
        Arc::new(Tokens),
        Arc::new(Device),
        None,
    )
    .unwrap();
    let cache_dir = dir.path().join("ondemand").to_string_lossy().into_owned();
    let reader = engine.on_demand(cache_dir.clone(), 64 << 20, None).unwrap();
    let origin = OnDemandOrigin::Drive {
        account: ACCOUNT.into(),
    };
    let file = OnDemandFile {
        id: "F1".into(),
        version: "7".into(),
        size,
    };

    // A seek deep into the file: one block, nothing else.
    let at = 9 * BLOCK + 10;
    let got = reader.read(origin.clone(), file.clone(), at, 1000).await.unwrap();
    assert_eq!(&body[at as usize..at as usize + 1000], &got[..]);
    assert_eq!(
        vec![format!("bytes={}-{}", 9 * BLOCK, 10 * BLOCK - 1)],
        *seen.lock().unwrap()
    );
    // Elsewhere in the same block: the cache answers.
    let near = reader
        .read(origin.clone(), file.clone(), at + 5000, 100)
        .await
        .unwrap();
    assert_eq!(&body[(at + 5000) as usize..(at + 5100) as usize], &near[..]);
    assert_eq!(1, seen.lock().unwrap().len());
    // The end of the file and past it.
    let tail = reader
        .read(origin.clone(), file.clone(), size - 10, 100)
        .await
        .unwrap();
    assert_eq!(&body[size as usize - 10..], &tail[..]);
    assert!(
        reader
            .read(origin.clone(), file.clone(), size, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        reader
            .read(origin.clone(), file.clone(), size + 1, 10)
            .await
            .is_err()
    );

    // A pin fetches the rest and keeps it.
    assert!(!reader.is_complete(origin.clone(), file.clone()));
    reader.pin(origin.clone(), file.clone()).await.unwrap();
    assert!(reader.is_complete(origin.clone(), file.clone()));
    assert!(reader.is_pinned(origin.clone(), file.clone()));
    assert_eq!(size, reader.usage().pinned);
    let asked = seen.lock().unwrap().len();
    let all = reader.read(origin.clone(), file.clone(), 0, size).await.unwrap();
    assert_eq!(body, all);
    assert_eq!(
        asked,
        seen.lock().unwrap().len(),
        "a pinned file reads from the phone"
    );
    // Over the limit, a pinned file stays; unpinned it can go.
    reader.set_limit(BLOCK);
    assert!(reader.is_complete(origin.clone(), file.clone()));
    reader.unpin(origin.clone(), file.clone()).unwrap();
    assert!(!reader.is_pinned(origin.clone(), file.clone()));
    let u = reader.usage();
    assert_eq!(0, u.pinned);
    assert!(u.used <= BLOCK, "{u:?}");

    // The engine keeps one reader per cache folder.
    let again = engine.on_demand(cache_dir, 32 << 20, None).unwrap();
    assert!(Arc::ptr_eq(&reader, &again));
    assert_eq!(32 << 20, again.usage().limit);
}

// ---------------------------------------------------------------- vaults on an in-memory Drive

struct Memory {
    drive: Arc<MemoryRemote>,
    access: Mutex<Option<VaultAccess>>,
}

impl OnDemandRemotes for Memory {
    fn remote(&self, _account: &str) -> Result<Arc<dyn Remote>, WaslError> {
        Ok(self.drive.clone())
    }
    fn source(&self, _account: &str) -> Result<Arc<dyn Source>, WaslError> {
        Ok(Arc::new(RemoteSource(self.drive.clone())))
    }
    fn vault_access(&self, _vault_id: &str) -> Option<VaultAccess> {
        self.access.lock().unwrap().clone()
    }
}

async fn upload(vault: &dyn Remote, dir: &Path, parent: &str, name: &str, content: &[u8]) -> String {
    let src = dir.join(format!("src-{name}"));
    std::fs::write(&src, content).unwrap();
    vault
        .upload(
            UploadSpec {
                src: &src,
                name,
                parent_id: parent,
                existing_id: None,
                mtime_ms: 1_760_000_123_000,
                session_key: None,
            },
            &|_| {},
        )
        .await
        .unwrap()
        .id
}

fn vault_origin(folder: &str) -> OnDemandOrigin {
    OnDemandOrigin::Vault {
        account: ACCOUNT.into(),
        vault_id: "vault-1".into(),
        folder_id: folder.into(),
        drive_id: None,
    }
}

fn file_of(item: &VaultItem) -> OnDemandFile {
    OnDemandFile {
        id: item.id.clone(),
        version: item.version.clone(),
        size: item.size,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_vault_v1_seek_decrypts_one_chunk_and_never_reads_the_ciphertext_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let drive = Arc::new(MemoryRemote::new());
    let folder = drive.mkdirs("Vault");
    let keys = VaultKeys::new([0x11; 32], [0x22; 32]);
    let writer = VaultRemote::new(
        drive.clone(),
        Arc::new(VaultKeys::new([0x11; 32], [0x22; 32])),
        dir.path().join("w"),
    );
    let docs = writer.create_folder(&folder, "Docs").await.unwrap().id;
    let size = 6 * CHUNK_BYTES + 777;
    let plain = pseudo(size, 2);
    let enc_id = upload(&writer, dir.path(), &docs, "movie.bin", &plain).await;
    let remotes = Arc::new(Memory {
        drive: drive.clone(),
        access: Mutex::new(None),
    });
    let reader = OnDemandReader::open(dir.path().join("cache"), 64 << 20, remotes.clone()).unwrap();
    let origin = vault_origin(&folder);

    // Locked: no listing, no reads.
    let err = reader.vault_items(origin.clone()).await.unwrap_err();
    assert!(matches!(err, WaslError::Locked { .. }), "{err:?}");
    let mut material = keys.key_enc().to_vec();
    material.extend_from_slice(keys.key_name());
    *remotes.access.lock().unwrap() = Some(VaultAccess {
        manifest_json: r#"{"format":"drivesync-vault","version":1}"#.into(),
        keys: material,
    });

    // The listing shows plaintext names, sizes and paths.
    let items = reader.vault_items(origin.clone()).await.unwrap();
    let paths: Vec<&str> = items.iter().map(|i| i.path.as_str()).collect();
    assert_eq!(vec!["Docs", "Docs/movie.bin"], paths);
    let movie = items.iter().find(|i| i.name == "movie.bin").unwrap();
    assert_eq!(
        (enc_id.as_str(), size as u64, false),
        (movie.id.as_str(), movie.size, movie.is_dir)
    );
    assert!(items[0].is_dir);
    let file = file_of(movie);

    // Read the encrypted file as a plain Drive file first: its blocks are ciphertext.
    let drive_origin = OnDemandOrigin::Drive {
        account: ACCOUNT.into(),
    };
    let cipher_file = OnDemandFile {
        id: enc_id.clone(),
        version: file.version.clone(),
        size: 2 * BLOCK,
    };
    let raw = reader
        .read(drive_origin, cipher_file, 0, 2 * BLOCK)
        .await
        .unwrap();
    assert_ne!(&plain[..1000], &raw[..1000]);

    // A seek into chunk 4 fetches its header and that one chunk, and the plaintext comes back.
    let before = drive.bytes_downloaded();
    let at = 4 * CHUNK_BYTES as u64 + 123;
    let got = reader.read(origin.clone(), file.clone(), at, 4096).await.unwrap();
    assert_eq!(&plain[at as usize..at as usize + 4096], &got[..]);
    let fetched = drive.bytes_downloaded() - before;
    assert!(
        fetched <= (HEADER_BYTES + CHUNK_BYTES + 16) as u64,
        "{fetched} bytes for one chunk"
    );

    // The start of the file, through the same cache as the ciphertext: plaintext.
    let head = reader.read(origin.clone(), file.clone(), 0, 1000).await.unwrap();
    assert_eq!(&plain[..1000], &head[..]);

    // Pinned, the whole file is on the phone and reads back without Drive.
    reader.pin(origin.clone(), file.clone()).await.unwrap();
    assert!(reader.is_complete(origin.clone(), file.clone()));
    let before = drive.bytes_downloaded();
    let all = reader
        .read(origin.clone(), file.clone(), 0, size as u64)
        .await
        .unwrap();
    assert_eq!(plain, all);
    assert_eq!(before, drive.bytes_downloaded());

    // Locking the vault: the listing is refused again.
    *remotes.access.lock().unwrap() = None;
    let err = reader.vault_items(origin.clone()).await.unwrap_err();
    assert!(matches!(err, WaslError::Locked { .. }), "{err:?}");
    // A plain Drive folder has no vault listing.
    assert!(
        reader
            .vault_items(OnDemandOrigin::Drive {
                account: ACCOUNT.into()
            })
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_vault_v2_seek_fetches_only_the_chunks_it_needs() {
    let dir = tempfile::tempdir().unwrap();
    let drive = Arc::new(MemoryRemote::new());
    let folder = drive.mkdirs("Vault");
    let ikm = [0x42u8; 32];
    let writer = VaultV2Remote::new(
        drive.clone(),
        Arc::new(VaultKeysV2::from_ikm(&ikm)),
        dir.path().join("w"),
    );
    writer.prepare(&folder, None).await.unwrap();
    writer.list_subtree(&folder, None).await.unwrap();
    let size = 24 << 20;
    let plain = pseudo(size, 3);
    upload(&writer, dir.path(), &folder, "movie.bin", &plain).await;
    upload(&writer, dir.path(), &folder, "note.txt", b"a short note").await;

    let access = VaultAccess {
        manifest_json: V2_JSON.into(),
        keys: ikm.to_vec(),
    };
    let remotes = Arc::new(Memory {
        drive: drive.clone(),
        access: Mutex::new(Some(access.clone())),
    });
    let reader = OnDemandReader::open(dir.path().join("cache"), 256 << 20, remotes).unwrap();
    let origin = vault_origin(&folder);

    // Reads before any listing list the vault themselves.
    let items = reader.vault_items(origin.clone()).await.unwrap();
    let mut names: Vec<&str> = items.iter().map(|i| i.path.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        vec!["movie.bin", "note.txt"],
        names,
        "no vault.json, chunks or markers"
    );
    let movie = file_of(items.iter().find(|i| i.name == "movie.bin").unwrap());
    let note = file_of(items.iter().find(|i| i.name == "note.txt").unwrap());
    assert_eq!(size as u64, movie.size);

    let fresh = OnDemandReader::open(
        dir.path().join("cache2"),
        256 << 20,
        Arc::new(Memory {
            drive: drive.clone(),
            access: Mutex::new(Some(access)),
        }),
    )
    .unwrap();
    assert_eq!(
        b"a short note".to_vec(),
        fresh.read(origin.clone(), note.clone(), 0, 100).await.unwrap()
    );

    // A seek deep into the file: the manifest and the chunks covering one block.
    let calls = drive.calls("Download");
    let bytes = drive.bytes_downloaded();
    let at = 19 * BLOCK + 777;
    let got = reader
        .read(origin.clone(), movie.clone(), at, 4096)
        .await
        .unwrap();
    assert_eq!(&plain[at as usize..at as usize + 4096], &got[..]);
    let objects = drive.calls("Download") - calls;
    let fetched = drive.bytes_downloaded() - bytes;
    assert!((2..=5).contains(&objects), "{objects} objects for one block");
    assert!(
        fetched < 3 * MAX_CHUNK_BYTES as u64,
        "{fetched} bytes for one block"
    );

    // Another seek, the manifest known: chunks only, a few at most.
    let calls = drive.calls("Download");
    let at = 7 * BLOCK + 5;
    let got = reader.read(origin.clone(), movie.clone(), at, 100).await.unwrap();
    assert_eq!(&plain[at as usize..at as usize + 100], &got[..]);
    let objects = drive.calls("Download") - calls;
    // Chunks average 1 MiB but can be as small as 256 KiB, and where they fall depends on the
    // vault's keys (format 2.1), so a 1 MiB block with read-ahead can need a few more.
    assert!((1..=6).contains(&objects), "{objects} chunk objects");

    // Pinned, all of it reads back from the phone.
    reader.pin(origin.clone(), movie.clone()).await.unwrap();
    assert!(reader.is_pinned(origin.clone(), movie.clone()));
    let calls = drive.calls("Download");
    let all = reader
        .read(origin.clone(), movie.clone(), 0, size as u64)
        .await
        .unwrap();
    assert_eq!(plain, all);
    assert_eq!(calls, drive.calls("Download"));
    assert_eq!(size as u64, reader.usage().pinned);

    // A wrong key reads nothing: the names do not decrypt, so nothing is listed.
    let wrong = OnDemandReader::open(
        dir.path().join("cache3"),
        1 << 20,
        Arc::new(Memory {
            drive,
            access: Mutex::new(Some(VaultAccess {
                manifest_json: V2_JSON.into(),
                keys: vec![0x43; 32],
            })),
        }),
    )
    .unwrap();
    assert!(wrong.vault_items(origin.clone()).await.unwrap().is_empty());
    assert!(wrong.read(origin, movie, 0, 10).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_locked_vault_reads_nothing_and_forgets_its_unpinned_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let drive = Arc::new(MemoryRemote::new());
    let folder = drive.mkdirs("Vault");
    let keys = VaultKeys::new([0x11; 32], [0x22; 32]);
    let writer = VaultRemote::new(drive.clone(), Arc::new(keys.clone()), dir.path().join("w"));
    let a_plain = pseudo(3 * CHUNK_BYTES + 5, 4);
    let b_plain = pseudo(2 * CHUNK_BYTES, 5);
    let a_id = upload(&writer, dir.path(), &folder, "a.bin", &a_plain).await;
    upload(&writer, dir.path(), &folder, "b.bin", &b_plain).await;
    let mut material = keys.key_enc().to_vec();
    material.extend_from_slice(keys.key_name());
    let access = VaultAccess {
        manifest_json: r#"{"format":"drivesync-vault","version":1}"#.into(),
        keys: material,
    };
    let remotes = Arc::new(Memory {
        drive: drive.clone(),
        access: Mutex::new(Some(access.clone())),
    });
    let cache = dir.path().join("cache");
    let reader = OnDemandReader::open(&cache, 64 << 20, remotes.clone()).unwrap();
    let origin = vault_origin(&folder);
    let items = reader.vault_items(origin.clone()).await.unwrap();
    let a = file_of(items.iter().find(|i| i.name == "a.bin").unwrap());
    let b = file_of(items.iter().find(|i| i.name == "b.bin").unwrap());
    let size = a.size;
    assert_eq!(
        a_plain,
        reader.read(origin.clone(), a.clone(), 0, size).await.unwrap()
    );
    reader.pin(origin.clone(), b.clone()).await.unwrap();
    // The same Drive file read as plain Drive bytes (its ciphertext) is not the vault's to forget.
    let drive_origin = OnDemandOrigin::Drive {
        account: ACCOUNT.into(),
    };
    let raw = OnDemandFile {
        id: a_id.clone(),
        version: a.version.clone(),
        size: BLOCK,
    };
    reader
        .read(drive_origin.clone(), raw.clone(), 0, BLOCK)
        .await
        .unwrap();
    assert!(reader.is_complete(origin.clone(), a.clone()));

    // Locked behind the reader's back: the next read checks, fails, and closes the vault.
    *remotes.access.lock().unwrap() = None;
    let err = reader.read(origin.clone(), a.clone(), 0, 10).await.unwrap_err();
    assert!(matches!(err, WaslError::Locked { .. }), "{err:?}");
    let err = reader.pin(origin.clone(), a.clone()).await.unwrap_err();
    assert!(matches!(err, WaslError::Locked { .. }), "{err:?}");
    // Its decrypted blocks are gone; the pinned file and the plain Drive blocks stay.
    assert!(!reader.is_complete(origin.clone(), a.clone()));
    assert!(reader.is_complete(origin.clone(), b.clone()));
    assert!(reader.is_complete(drive_origin.clone(), raw.clone()));

    // Unlocked again: reads work.
    *remotes.access.lock().unwrap() = Some(access);
    let got = reader.read(origin.clone(), a.clone(), 100, 50).await.unwrap();
    assert_eq!(&a_plain[100..150], &got[..]);
    assert_eq!(
        a_plain,
        reader.read(origin.clone(), a.clone(), 0, size).await.unwrap()
    );

    // A later run (a new reader on the same cache) still knows what this one read through the
    // vault: closing it there forgets those blocks too.
    drop(reader);
    let later = OnDemandReader::open(&cache, 64 << 20, remotes).unwrap();
    assert!(later.is_complete(origin.clone(), a.clone()));
    later.close_vault("vault-1".into());
    assert!(!later.is_complete(origin.clone(), a.clone()));
    assert!(
        later.is_complete(origin.clone(), b.clone()),
        "a pinned file stays on the phone"
    );
    assert!(later.is_complete(drive_origin, raw));
    // Unpinned and closed again, nothing of the vault is left.
    later.unpin(origin.clone(), b.clone()).unwrap();
    later.close_vault("vault-1".into());
    assert!(!later.is_complete(origin, b));
    assert!(!cache.join("vaults").join("vault-1.json").exists());
}

/// An in-memory Drive whose next range read waits until the test lets it go.
struct Gated {
    inner: Arc<MemoryRemote>,
    armed: AtomicBool,
    entered: Notify,
    release: Notify,
}

#[async_trait]
impl Remote for Gated {
    async fn list_subtree(&self, root_id: &str, drive_id: Option<&str>) -> wasl_drive::Result<Listing> {
        self.inner.list_subtree(root_id, drive_id).await
    }
    async fn download(
        &self,
        entry: &RemoteEntry,
        dest: &Path,
        progress: ProgressFn<'_>,
    ) -> wasl_drive::Result<()> {
        self.inner.download(entry, dest, progress).await
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
    async fn read_range(&self, id: &str, expect: &str, offset: u64, len: u64) -> wasl_drive::Result<Vec<u8>> {
        if self.armed.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        self.inner.read_range(id, expect, offset, len).await
    }
}

struct GatedRemotes {
    drive: Arc<Gated>,
    access: Mutex<Option<VaultAccess>>,
}

impl OnDemandRemotes for GatedRemotes {
    fn remote(&self, _account: &str) -> Result<Arc<dyn Remote>, WaslError> {
        Ok(self.drive.clone())
    }
    fn source(&self, _account: &str) -> Result<Arc<dyn Source>, WaslError> {
        Ok(Arc::new(RemoteSource(self.drive.clone())))
    }
    fn vault_access(&self, _vault_id: &str) -> Option<VaultAccess> {
        self.access.lock().unwrap().clone()
    }
}

/// A v1 vault in folder `Vault` with `a.bin` and `b.bin`, and its key material.
async fn v1_vault(dir: &Path, drive: &Arc<MemoryRemote>) -> (String, VaultAccess) {
    let folder = drive.mkdirs("Vault");
    let keys = VaultKeys::new([0x11; 32], [0x22; 32]);
    let writer = VaultRemote::new(drive.clone(), Arc::new(keys.clone()), dir.join("w"));
    upload(&writer, dir, &folder, "a.bin", &pseudo(2 * CHUNK_BYTES + 9, 6)).await;
    upload(&writer, dir, &folder, "b.bin", &pseudo(CHUNK_BYTES + 3, 7)).await;
    let mut material = keys.key_enc().to_vec();
    material.extend_from_slice(keys.key_name());
    let access = VaultAccess {
        manifest_json: r#"{"format":"drivesync-vault","version":1}"#.into(),
        keys: material,
    };
    (folder, access)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_vault_closed_during_a_read_returns_nothing_and_keeps_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let drive = Arc::new(MemoryRemote::new());
    let (folder, access) = v1_vault(dir.path(), &drive).await;
    let gated = Arc::new(Gated {
        inner: drive,
        armed: AtomicBool::new(false),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let remotes = Arc::new(GatedRemotes {
        drive: gated.clone(),
        access: Mutex::new(Some(access)),
    });
    let reader = OnDemandReader::open(dir.path().join("cache"), 64 << 20, remotes.clone()).unwrap();
    let origin = vault_origin(&folder);
    let items = reader.vault_items(origin.clone()).await.unwrap();
    let a = file_of(items.iter().find(|i| i.name == "a.bin").unwrap());

    // The read gets past its lock check and waits inside Drive...
    gated.armed.store(true, Ordering::SeqCst);
    let (r, o, f) = (reader.clone(), origin.clone(), a.clone());
    let task = tokio::spawn(async move { r.read(o, f, 0, 1000).await });
    gated.entered.notified().await;
    // ...while the vault is locked and closed.
    *remotes.access.lock().unwrap() = None;
    reader.close_vault("vault-1".into());
    gated.release.notify_one();

    let err = task.await.unwrap().unwrap_err();
    assert!(matches!(err, WaslError::Locked { .. }), "{err:?}");
    assert!(!reader.is_complete(origin.clone(), a.clone()));
    assert_eq!(0, reader.usage().used, "nothing decrypted stays on the phone");
    let err = reader.read(origin, a, 0, 10).await.unwrap_err();
    assert!(matches!(err, WaslError::Locked { .. }), "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_damaged_record_makes_a_close_evict_every_unpinned_block() {
    let dir = tempfile::tempdir().unwrap();
    let drive = Arc::new(MemoryRemote::new());
    let (folder, access) = v1_vault(dir.path(), &drive).await;
    let plain_bytes = pseudo(BLOCK as usize + 10, 8);
    let plain = OnDemandFile {
        id: drive.put_file("plain.bin", &plain_bytes),
        version: "1".into(),
        size: plain_bytes.len() as u64,
    };
    let cache = dir.path().join("cache");
    let record = cache.join("vaults").join("vault-1.json");
    std::fs::create_dir_all(record.parent().unwrap()).unwrap();
    std::fs::write(&record, b"{ not a record").unwrap();
    let remotes = Arc::new(Memory {
        drive: drive.clone(),
        access: Mutex::new(Some(access)),
    });
    let reader = OnDemandReader::open(&cache, 64 << 20, remotes).unwrap();
    let origin = vault_origin(&folder);
    let drive_origin = OnDemandOrigin::Drive {
        account: ACCOUNT.into(),
    };
    let items = reader.vault_items(origin.clone()).await.unwrap();
    let a = file_of(items.iter().find(|i| i.name == "a.bin").unwrap());
    let b = file_of(items.iter().find(|i| i.name == "b.bin").unwrap());
    reader.read(origin.clone(), a.clone(), 0, a.size).await.unwrap();
    reader.pin(origin.clone(), b.clone()).await.unwrap();
    reader
        .read(drive_origin.clone(), plain.clone(), 0, plain.size)
        .await
        .unwrap();
    // What earlier runs read is unknown: the damaged record is not papered over.
    assert_eq!(b"{ not a record".to_vec(), std::fs::read(&record).unwrap());

    reader.close_vault("vault-1".into());
    assert!(!reader.is_complete(origin.clone(), a.clone()));
    assert!(reader.is_complete(origin.clone(), b.clone()), "pinned files stay");
    assert!(
        !reader.is_complete(drive_origin.clone(), plain.clone()),
        "every unpinned block goes"
    );
    let fresh: Vec<(String, String, u64)> = serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
    assert_eq!(1, fresh.len(), "the record starts again with the pinned file");

    // From then on a close forgets the vault's blocks only.
    reader.read(origin.clone(), a.clone(), 0, a.size).await.unwrap();
    reader
        .read(drive_origin.clone(), plain.clone(), 0, plain.size)
        .await
        .unwrap();
    reader.close_vault("vault-1".into());
    assert!(!reader.is_complete(origin, a));
    assert!(reader.is_complete(drive_origin, plain));
}
