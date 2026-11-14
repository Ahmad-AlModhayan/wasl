//! Vaults on a computer: keys in a keychain (an in-memory one here, never the real one), vaults
//! made and unlocked against the in-memory Drive, a vault pair syncing through its vault, and
//! vault views (v1 and v2) listing and reading through the view manager.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use wasl_core::RunStatus;
use wasl_core::conditions::{Conditions, DeviceState};
use wasl_crypto::v2::KdfParams;
use wasl_crypto::v2::vault_json::VaultManifestV2;
use wasl_crypto::{VaultManifest, kdf};
use wasl_drive::DriveError;
use wasl_engine::{
    Engine, EngineConfig, Environment, MemoryRemote, Op, Remote, SystemClock, Trigger, UploadSpec,
    VaultRemote, VaultV2Remote,
};
use wasl_fs::{LocalFs, StdFs};
use wasl_ipc::{ErrorKind, NewView, Passphrase, VaultFolder, VaultInfo};
use wasl_store::{Pair, SharedStore, Store};
use wasld::tokens::{MemorySecrets, Secrets};
use wasld::vaults::{Keys, NoKeys, VaultViews, Vaults, WRONG_PASSPHRASE};
use wasld::views::{MemoryBackend, VAULT_LOCKED, Views};

const PASS: &str = "correct horse battery";
const ME: &str = "me@example.com";

struct Setup {
    dir: tempfile::TempDir,
    store: SharedStore,
    drive: Arc<MemoryRemote>,
    keychain: Arc<MemorySecrets>,
    vaults: Arc<Vaults>,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let store = SharedStore::new(Store::open(dir.path().join("wasl.db")).unwrap());
    computer(dir, store, Arc::new(MemoryRemote::new()))
}

/// A computer with its own store and keychain, on `drive`.
fn computer(dir: tempfile::TempDir, store: SharedStore, drive: Arc<MemoryRemote>) -> Setup {
    let keychain = Arc::new(MemorySecrets::default());
    let vaults = Arc::new(
        Vaults::new(store.clone(), keychain.clone(), dir.path().join("tmp")).with_kdf(KdfParams::MIN),
    );
    Setup {
        dir,
        store,
        drive,
        keychain,
        vaults,
    }
}

impl Setup {
    fn folder(&self, path: &str) -> VaultFolder {
        VaultFolder {
            account_id: ME.into(),
            folder_id: self.drive.mkdirs(path),
            drive_id: None,
            remote_path: format!("My Drive/{path}"),
        }
    }

    async fn create(&self, path: &str) -> (VaultFolder, VaultInfo) {
        let folder = self.folder(path);
        let info = self
            .vaults
            .create(self.drive.as_ref(), folder.clone(), path, Passphrase::new(PASS))
            .await
            .unwrap();
        (folder, info)
    }

    fn v2_keys(&self, id: &str) -> Arc<wasl_crypto::v2::VaultKeysV2> {
        match self.vaults.keys(id) {
            Ok(Keys::V2 { keys, .. }) => keys,
            _ => panic!("not an unlocked v2 vault"),
        }
    }

    fn file(&self, name: &str, content: &[u8]) -> PathBuf {
        let p = self.dir.path().join("src").join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, content).unwrap();
        p
    }
}

async fn upload(remote: &dyn Remote, src: &Path, name: &str, parent: &str) {
    remote
        .upload(
            UploadSpec {
                src,
                name,
                parent_id: parent,
                existing_id: None,
                mtime_ms: 1_760_000_000_000,
                session_key: None,
            },
            &|_| {},
        )
        .await
        .unwrap();
}

/// Bytes that do not repeat, so chunks are realistic.
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

// ---------------------------------------------------------------- keys

#[tokio::test]
async fn keys_live_in_the_keychain_and_locking_forgets_them() {
    let s = setup();
    let (_, info) = s.create("Private").await;
    assert!(info.unlocked);
    assert_eq!(("Private", 2), (info.name.as_str(), info.version));

    // The Argon2id output (32 bytes), never the passphrase; the store has no secret at all.
    let kept = s.keychain.get(&info.id).unwrap();
    assert!(kept.starts_with("2:") && kept.len() == 2 + 64, "{}", kept.len());
    assert!(!kept.contains(PASS));
    let record = s.store.lock().vault(&info.id).unwrap().unwrap();
    assert!(!record.manifest_json.contains(PASS) && !record.manifest_json.contains(&kept[2..]));
    assert!(matches!(s.vaults.keys(&info.id), Ok(Keys::V2 { .. })));

    s.vaults.lock(&info.id).unwrap();
    assert_eq!(None, s.keychain.get(&info.id));
    assert!(matches!(s.vaults.keys(&info.id), Err(NoKeys::Locked)));
    assert!(!s.vaults.info(&info.id).unwrap().unlocked);
    assert!(!s.vaults.list().unwrap()[0].unlocked);

    // Whatever else sits in the keychain entry is refused, never used as keys.
    s.keychain
        .set(&info.id, &format!("2:{}", "00".repeat(32)))
        .unwrap();
    assert!(matches!(s.vaults.keys(&info.id), Err(NoKeys::Damaged(_))));
    s.keychain
        .set(&info.id, &format!("1:{}", "00".repeat(64)))
        .unwrap();
    assert!(matches!(s.vaults.keys(&info.id), Err(NoKeys::Damaged(_))));
    s.keychain.set(&info.id, "not hex").unwrap();
    assert!(matches!(s.vaults.keys(&info.id), Err(NoKeys::Damaged(_))));
    assert!(matches!(s.vaults.keys("nope"), Err(NoKeys::Unknown)));
    assert_eq!(ErrorKind::NotFound, s.vaults.lock("nope").unwrap_err().kind);
}

#[tokio::test]
async fn a_vault_is_made_unlocked_again_and_refuses_a_wrong_passphrase() {
    let s = setup();
    let (folder, info) = s.create("Private").await;
    // Only its vault.json is on Drive, at the cost asked for.
    assert_eq!(
        vec!["Private/vault.json"],
        s.drive.files().into_keys().collect::<Vec<_>>()
    );
    let json = String::from_utf8(s.drive.content("Private/vault.json").unwrap()).unwrap();
    let manifest = VaultManifestV2::parse(&json).unwrap();
    assert_eq!(
        ("Private", 19456, 1, 1),
        (manifest.name.as_str(), manifest.m, manifest.t, manifest.p)
    );
    assert!(!json.contains(PASS));

    s.vaults.lock(&info.id).unwrap();
    let e = s
        .vaults
        .unlock(&info.id, Passphrase::new("not the passphrase"))
        .await
        .unwrap_err();
    assert_eq!(
        (ErrorKind::Invalid, WRONG_PASSPHRASE),
        (e.kind, e.message.as_str())
    );
    assert!(!s.vaults.info(&info.id).unwrap().unlocked);
    let e = s.vaults.unlock(&info.id, Passphrase::new("")).await.unwrap_err();
    assert_eq!(ErrorKind::Invalid, e.kind);
    assert!(
        s.vaults
            .unlock(&info.id, Passphrase::new(PASS))
            .await
            .unwrap()
            .unlocked
    );

    // Another computer unlocks the folder: the same vault, found by its vault.json. A wrong
    // passphrase there records nothing.
    let other = computer(
        tempfile::tempdir().unwrap(),
        SharedStore::new(Store::open_in_memory().unwrap()),
        s.drive.clone(),
    );
    let e = other
        .vaults
        .unlock_folder(
            other.drive.as_ref(),
            folder.clone(),
            Passphrase::new("nope nope nope"),
        )
        .await
        .unwrap_err();
    assert_eq!(
        (ErrorKind::Invalid, WRONG_PASSPHRASE),
        (e.kind, e.message.as_str())
    );
    assert!(other.vaults.list().unwrap().is_empty());
    let found = other
        .vaults
        .unlock_folder(other.drive.as_ref(), folder.clone(), Passphrase::new(PASS))
        .await
        .unwrap();
    assert_eq!((info.id.as_str(), true), (found.id.as_str(), found.unlocked));
    assert_eq!(Some(folder.folder_id.as_str()), found.drive_folder_id.as_deref());
    assert_eq!(
        s.keychain.get(&info.id),
        other.keychain.get(&info.id),
        "the same keys"
    );
}

#[tokio::test]
async fn a_new_vault_needs_an_empty_folder_a_name_and_a_long_passphrase() {
    let s = setup();
    let (vault, _) = s.create("Private").await;
    let busy = s.folder("Busy");
    s.drive.put_file("Busy/notes.txt", b"plain");
    let make = |f: VaultFolder, name: &str, pass: &str| {
        let (vaults, drive) = (s.vaults.clone(), s.drive.clone());
        let (name, pass) = (name.to_owned(), pass.to_owned());
        async move {
            vaults
                .create(drive.as_ref(), f, &name, Passphrase::new(pass))
                .await
        }
    };
    let e = make(busy, "Busy", PASS).await.unwrap_err();
    assert_eq!(ErrorKind::Invalid, e.kind);
    assert!(e.message.contains("isn't empty"), "{e:?}");
    let e = make(vault.clone(), "Private", PASS).await.unwrap_err();
    assert!(e.message.contains("already holds a vault"), "{e:?}");
    let e = make(s.folder("Short"), "Short", "seven77").await.unwrap_err();
    assert!(e.message.contains("at least 8"), "{e:?}");
    let e = make(s.folder("Nameless"), "  ", PASS).await.unwrap_err();
    assert_eq!(ErrorKind::Invalid, e.kind);
    assert_eq!(1, s.vaults.list().unwrap().len());

    let plain = s.folder("Plain");
    let e = s
        .vaults
        .unlock_folder(s.drive.as_ref(), plain, Passphrase::new(PASS))
        .await
        .unwrap_err();
    assert!(e.message.contains("isn't a vault"), "{e:?}");
}

// ---------------------------------------------------------------- pairs

struct PairEnv {
    root: PathBuf,
    bin: PathBuf,
    drive: Arc<MemoryRemote>,
    vaults: Arc<Vaults>,
}

impl Environment for PairEnv {
    fn local_fs(&self, _pair: &Pair) -> wasl_engine::Result<Arc<dyn LocalFs>> {
        Ok(Arc::new(StdFs::new(&self.root, &self.bin)))
    }
    fn remote(&self, pair: &Pair) -> wasl_engine::Result<Arc<dyn Remote>> {
        // As the daemon builds it (DesktopEnv::remote), over the in-memory Drive.
        self.vaults.pair_remote(self.drive.clone(), pair)
    }
    fn device_state(&self) -> DeviceState {
        DeviceState::unrestricted()
    }
}

#[tokio::test]
async fn a_vault_pair_syncs_only_ciphertext_and_waits_while_locked() {
    let s = setup();
    let (folder, info) = s.create("Private").await;
    let root = s.dir.path().join("Private here");
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(root.join("a.txt"), "secret plans").unwrap();
    std::fs::write(root.join("docs/b.txt"), "more secret plans").unwrap();
    let mut pair = Pair::new("p1", "Private", root.to_str().unwrap());
    pair.account_id = Some(ME.into());
    pair.drive_folder_id = Some(folder.folder_id.clone());
    pair.remote_path = folder.remote_path.clone();
    pair.vault_id = Some(info.id.clone());
    pair.conditions = Conditions::none();
    s.store.lock().create_pair(&pair).unwrap();
    let env = Arc::new(PairEnv {
        root: root.clone(),
        bin: s.dir.path().join("bin"),
        drive: s.drive.clone(),
        vaults: s.vaults.clone(),
    });
    let engine = Engine::new(
        s.store.clone(),
        env,
        Arc::new(SystemClock),
        EngineConfig::default(),
        None,
    );
    let out = engine.sync("p1", Trigger::Manual).await;
    assert_eq!(RunStatus::Success, out.status, "{:?}", out.message);

    // Drive holds ciphertext only: no plaintext name, no plaintext content.
    let files = s.drive.files();
    assert!(files.keys().any(|k| k.ends_with(".w2")), "{files:?}");
    for (path, content) in &files {
        assert!(!path.contains("a.txt") && !path.contains("docs"), "{path}");
        assert!(!contains(content, b"secret plans"), "{path}");
    }
    // Someone else with the vault unlocked reads the plaintext names.
    let reader = VaultV2Remote::new(s.drive.clone(), s.v2_keys(&info.id), s.dir.path().join("t2"));
    let mut names: Vec<String> = reader
        .list_subtree(&folder.folder_id, None)
        .await
        .unwrap()
        .entries
        .into_iter()
        .map(|e| e.key)
        .collect();
    names.sort();
    assert_eq!(vec!["a.txt", "docs", "docs/b.txt"], names);

    // Locked: the pair stops and says to unlock the vault; nothing moves.
    s.vaults.lock(&info.id).unwrap();
    std::fs::write(root.join("c.txt"), "later").unwrap();
    let before = s.drive.bytes_uploaded();
    let out = engine.sync("p1", Trigger::Manual).await;
    assert_ne!(RunStatus::Success, out.status);
    assert_eq!(Some(wasl_core::ErrorKind::Locked), out.error_kind);
    assert!(
        out.message.as_deref().unwrap_or("").contains("Unlock it"),
        "{:?}",
        out.message
    );
    assert_eq!(before, s.drive.bytes_uploaded());

    // Unlocked again: it syncs.
    s.vaults.unlock(&info.id, Passphrase::new(PASS)).await.unwrap();
    let out = engine.sync("p1", Trigger::Manual).await;
    assert_eq!(RunStatus::Success, out.status, "{:?}", out.message);
    assert!(s.drive.bytes_uploaded() > before);
}

// ---------------------------------------------------------------- views

struct ViewSetup {
    s: Setup,
    views: Views,
    backend: Arc<MemoryBackend>,
}

fn views(s: Setup) -> ViewSetup {
    let backend = Arc::new(MemoryBackend::default());
    let drive = s.drive.clone();
    let remotes = Arc::new(VaultViews {
        vaults: s.vaults.clone(),
        drive: Arc::new(move |_account: &str| -> Arc<dyn Remote> { drive.clone() }),
    });
    let views = Views::new(
        s.store.clone(),
        &s.dir.path().join("data"),
        remotes,
        backend.clone(),
        Duration::from_secs(300),
        Arc::new(|| {}),
    );
    ViewSetup { s, views, backend }
}

impl ViewSetup {
    fn new_view(&self, folder: &VaultFolder, name: &str) -> NewView {
        NewView {
            name: name.into(),
            account_id: ME.into(),
            drive_folder_id: folder.folder_id.clone(),
            drive_id: None,
            remote_path: folder.remote_path.clone(),
            local_path: self
                .s
                .dir
                .path()
                .join(format!("{name} view"))
                .display()
                .to_string(),
            cache_limit: Some(64 << 20),
        }
    }

    fn state(&self, id: &str) -> wasl_ipc::ViewState {
        self.views
            .info()
            .unwrap()
            .views
            .into_iter()
            .find(|v| v.view.id == id)
            .unwrap()
    }

    fn shown(&self, id: &str) -> Vec<String> {
        let mut v = self
            .backend
            .shown
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .unwrap_or_default();
        v.sort();
        v
    }

    /// Reads a file of a mounted view as an app would.
    async fn read(&self, id: &str, path: &str, offset: u64, len: u64) -> Vec<u8> {
        let (tree, hydrator) = self.backend.reads.lock().unwrap().get(id).cloned().unwrap();
        let file = tree.get(path).unwrap().file.clone().unwrap();
        hydrator.read(&file, offset, len).await.unwrap()
    }
}

#[tokio::test]
async fn a_vault_v2_view_shows_names_and_reads_plaintext() {
    let v = views(setup());
    let s = &v.s;
    let (folder, info) = s.create("Private").await;
    let big = pseudo(3 << 20, 7);
    let writer = VaultV2Remote::new(s.drive.clone(), s.v2_keys(&info.id), s.dir.path().join("w"));
    writer.list_subtree(&folder.folder_id, None).await.unwrap();
    upload(&writer, &s.file("a.bin", &big), "a.bin", &folder.folder_id).await;
    let trip = writer.create_folder(&folder.folder_id, "trip").await.unwrap();
    upload(&writer, &s.file("b.txt", b"postcard"), "b.txt", &trip.id).await;
    upload(
        &writer,
        &s.file("c.txt", b"never opened"),
        "c.txt",
        &folder.folder_id,
    )
    .await;
    let uploaded = s.drive.bytes_uploaded();

    let view = v.views.add(v.new_view(&folder, "Private"), &[]).await.unwrap();
    // Linked to its vault when it mounted, unlocked here: it shows the files.
    let record = s.store.lock().view(&view.id).unwrap().unwrap();
    assert_eq!(Some(info.id.clone()), record.vault_id);
    let st = v.state(&view.id);
    assert!(st.mounted, "{:?}", st.view.last_error);
    assert_eq!(
        (3, big.len() as u64 + 8 + 12),
        (st.files, st.bytes),
        "plaintext sizes"
    );
    assert_eq!(vec!["a.bin", "c.txt", "trip", "trip/b.txt"], v.shown(&view.id));

    assert_eq!(b"postcard".to_vec(), v.read(&view.id, "trip/b.txt", 0, 100).await);
    let at = 2_500_000u64;
    assert_eq!(
        big[at as usize..at as usize + 4096].to_vec(),
        v.read(&view.id, "a.bin", at, 4096).await
    );
    assert_eq!(uploaded, s.drive.bytes_uploaded(), "a view never uploads");

    // Keep one file on this computer; the start of a.bin is only in the cache.
    let local = Path::new(&view.local_path);
    v.views
        .set_pinned(local.join("trip").join("b.txt").to_str().unwrap(), true)
        .await
        .unwrap();
    let (tree, held) = v.backend.reads.lock().unwrap().get(&view.id).cloned().unwrap();
    let a_bin = tree.get("a.bin").unwrap().file.clone().unwrap();
    let c_txt = tree.get("c.txt").unwrap().file.clone().unwrap();
    let (used, pinned) = held.cache().usage();
    assert!(used > pinned && pinned == 8, "{used} {pinned}");

    // Locked: at once, nothing reads through the vault's keys any more, even a hydrator someone
    // still holds (a fetch Windows asked for just before); the view stops showing files, says
    // why, and its cache lets go of everything but the file kept here.
    s.vaults.lock(&info.id).unwrap();
    let Err(cut) = held.read(&c_txt, 0, 12).await else {
        panic!("a read went through the vault's keys after the lock");
    };
    assert!(cut.to_string().contains("locked on this computer"), "{cut}");
    v.views.vault_locked(&info.id).await;
    // Security review 2026-10: the folder lets go of what was read too, not only the cache.
    assert_eq!(
        Some((view.id.clone(), wasld::views::Close::Lock)),
        v.backend.closes.lock().unwrap().last().cloned()
    );
    assert_eq!((8, 8), held.cache().usage(), "only the pinned file stays");
    assert!(
        held.read(&a_bin, 0, 10).await.is_err(),
        "what was cached went too"
    );
    let st = v.state(&view.id);
    assert!(!st.mounted);
    assert!(!v.backend.reads.lock().unwrap().contains_key(&view.id));
    assert_eq!(Some(VAULT_LOCKED), st.view.last_error.as_deref());
    assert_eq!(Some(ErrorKind::Locked), st.last_error_kind);
    drop(held);

    // Unlocked: it shows again and reads.
    s.vaults.unlock(&info.id, Passphrase::new(PASS)).await.unwrap();
    v.views.vault_unlocked(&info.id).await;
    let st = v.state(&view.id);
    assert!(st.mounted);
    assert_eq!(None, st.view.last_error);
    assert_eq!(big[..64].to_vec(), v.read(&view.id, "a.bin", 0, 64).await);
}

#[tokio::test]
async fn a_vault_v1_view_waits_for_its_vault_then_reads_a_chunk_at_a_time() {
    let v = views(setup());
    let s = &v.s;
    // A vault made on the phone: v1, PBKDF2 at the lowest cost readers accept.
    let salt = [5u8; 16];
    let iterations = wasl_crypto::manifest::MIN_ITERATIONS as u32;
    let keys = kdf::derive(PASS, &salt, iterations).unwrap();
    let manifest = VaultManifest::create(
        "Phone vault",
        &salt,
        iterations,
        &kdf::key_check(&keys),
        1_760_000_000_000,
    );
    let folder = s.folder("Phone");
    s.drive
        .put_file("Phone/vault.json", manifest.to_json().as_bytes());
    let writer = VaultRemote::new(s.drive.clone(), Arc::new(keys), s.dir.path().join("w"));
    let big = pseudo((2 << 20) + 12345, 11);
    upload(
        &writer,
        &s.file("video.bin", &big),
        "video.bin",
        &folder.folder_id,
    )
    .await;
    upload(
        &writer,
        &s.file("note.txt", b"hello"),
        "note.txt",
        &folder.folder_id,
    )
    .await;

    // Never unlocked here: the view is linked to the vault (now recorded, locked) and waits.
    let view = v.views.add(v.new_view(&folder, "Phone"), &[]).await.unwrap();
    let st = v.state(&view.id);
    assert!(!st.mounted);
    assert_eq!(Some(ErrorKind::Locked), st.last_error_kind);
    assert_eq!(Some(VAULT_LOCKED), st.view.last_error.as_deref());
    let vault_id = st.view.vault_id.clone().unwrap();
    let known = s.vaults.info(&vault_id).unwrap();
    assert_eq!(
        ("Phone vault", 1, false),
        (known.name.as_str(), known.version, known.unlocked)
    );
    assert!(
        v.backend.mounts.lock().unwrap().is_empty(),
        "never shown as ciphertext"
    );

    s.vaults.unlock(&vault_id, Passphrase::new(PASS)).await.unwrap();
    v.views.vault_unlocked(&vault_id).await;
    let st = v.state(&view.id);
    assert!(st.mounted, "{:?}", st.view.last_error);
    assert_eq!(vec!["note.txt", "video.bin"], v.shown(&view.id));
    assert_eq!(big.len() as u64 + 5, st.bytes, "plaintext sizes");
    assert_eq!(b"hello".to_vec(), v.read(&view.id, "note.txt", 0, 5).await);
    // Across the boundary of the first and second 1 MiB chunk.
    let at = (1 << 20) - 100;
    assert_eq!(
        big[at..at + 300].to_vec(),
        v.read(&view.id, "video.bin", at as u64, 300).await
    );
    let tail = big.len() - 10;
    assert_eq!(
        big[tail..].to_vec(),
        v.read(&view.id, "video.bin", tail as u64, 100).await
    );
}

// ---------------------------------------------------------------- plain pairs never touch a vault

/// `vault.json` files that are not usable manifests and not clearly someone else's file either.
fn damaged_manifests() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("Malformed", b"not json {".to_vec()),
        ("Oversized", vec![b' '; 70 << 10]),
        ("Formatless", br#"{"name":"x"}"#.to_vec()),
        ("Truncated", br#"{"format":"wasl-vault","version":2}"#.to_vec()),
        ("NotText", vec![0xff, 0xfe, 0x00]),
    ]
}

fn plain_pair(s: &Setup, folder: &VaultFolder, root: &Path) -> Pair {
    let mut pair = Pair::new("plain", "Plain", root.to_str().unwrap());
    pair.account_id = Some(ME.into());
    pair.drive_folder_id = Some(folder.folder_id.clone());
    pair.remote_path = folder.remote_path.clone();
    pair.conditions = Conditions::none();
    let _ = s;
    pair
}

#[tokio::test]
async fn a_new_pair_on_a_vault_folder_is_linked_and_one_that_cannot_be_checked_is_refused() {
    let s = setup();
    let (vault_folder, info) = s.create("Private").await;
    s.vaults.lock(&info.id).unwrap();
    let root = s.dir.path().join("here");

    let mut pair = plain_pair(&s, &vault_folder, &root);
    s.vaults.link_pair(s.drive.as_ref(), &mut pair).await.unwrap();
    assert_eq!(
        Some(info.id.clone()),
        pair.vault_id,
        "linked, even while the vault is locked"
    );

    let plain = s.folder("Plain");
    s.drive
        .put_file("Plain/vault.json", br#"{"format":"another app"}"#);
    let mut pair = plain_pair(&s, &plain, &root);
    s.vaults.link_pair(s.drive.as_ref(), &mut pair).await.unwrap();
    assert_eq!(
        None, pair.vault_id,
        "someone else's vault.json is an ordinary file"
    );

    // A vault.json that can't be read as a manifest may be a damaged vault's: refused, never
    // taken for a plain folder.
    for (name, content) in damaged_manifests() {
        let f = s.folder(name);
        s.drive.put_file(&format!("{name}/vault.json"), &content);
        let mut pair = plain_pair(&s, &f, &root);
        let e = s.vaults.link_pair(s.drive.as_ref(), &mut pair).await.unwrap_err();
        assert_eq!(ErrorKind::Invalid, e.kind, "{name}: {e:?}");
        assert!(e.message.contains("damaged or too large"), "{name}: {e:?}");
        assert_eq!(None, pair.vault_id);
    }

    // Drive can't be read: whether the folder is a vault isn't known, so the pair is refused.
    let mut pair = plain_pair(&s, &vault_folder, &root);
    s.drive
        .fail_next(Op::ListTopLevel, DriveError::Network("offline".into()));
    let e = s.vaults.link_pair(s.drive.as_ref(), &mut pair).await.unwrap_err();
    assert_eq!(ErrorKind::Network, e.kind);
    assert!(e.message.contains("couldn't check whether"), "{e:?}");
    assert_eq!(None, pair.vault_id);

    // A pair can only name a vault this computer knows.
    pair.vault_id = Some("nope".into());
    let e = s.vaults.link_pair(s.drive.as_ref(), &mut pair).await.unwrap_err();
    assert_eq!(ErrorKind::NotFound, e.kind);
}

#[tokio::test]
async fn checking_a_new_pairs_folder_reads_only_its_top_level() {
    // Before a pair is saved, its Drive folder is checked for a vault. That used to list the whole
    // folder; on a big one, before the account's first full read, the live walk outlasted the
    // check's 20 s and a new pair failed as "Can't reach Google Drive" (2026-10). Only the
    // folder's own items say whether it is a vault, or empty.
    let s = setup();
    let (vault_folder, info) = s.create("Private").await;
    let big = s.folder("Big");
    for i in 0..30 {
        s.drive
            .put_file(&format!("Big/year {i}/month/day/photo.jpg"), b"x");
    }
    let root = s.dir.path().join("here");
    // Any whole-subtree listing would fail from here on.
    for _ in 0..10 {
        s.drive
            .fail_next(Op::List, DriveError::Network("a whole-subtree listing".into()));
    }
    let (subtree, top) = (s.drive.calls("List"), s.drive.calls("ListTopLevel"));

    let mut pair = plain_pair(&s, &vault_folder, &root);
    s.vaults.link_pair(s.drive.as_ref(), &mut pair).await.unwrap();
    assert_eq!(Some(info.id.clone()), pair.vault_id);
    let mut pair = plain_pair(&s, &big, &root);
    s.vaults.link_pair(s.drive.as_ref(), &mut pair).await.unwrap();
    assert_eq!(None, pair.vault_id);
    // A new vault still needs an empty folder: one level shows that too.
    let e = s
        .vaults
        .create(s.drive.as_ref(), big.clone(), "Big", Passphrase::new(PASS))
        .await
        .unwrap_err();
    assert_eq!(ErrorKind::Invalid, e.kind);
    assert!(e.message.contains("isn't empty"), "{e:?}");

    assert_eq!(subtree, s.drive.calls("List"), "no whole-subtree listing");
    assert_eq!(top + 3, s.drive.calls("ListTopLevel"), "one level per check");
}

#[tokio::test]
async fn a_plain_pair_whose_folder_becomes_a_vault_stops_before_uploading_anything() {
    let s = setup();
    let folder = s.folder("Shared");
    let root = s.dir.path().join("Shared here");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), "not for a vault in plaintext").unwrap();
    let pair = plain_pair(&s, &folder, &root);
    s.store.lock().create_pair(&pair).unwrap();
    let engine = Engine::new(
        s.store.clone(),
        Arc::new(PairEnv {
            root: root.clone(),
            bin: s.dir.path().join("bin"),
            drive: s.drive.clone(),
            vaults: s.vaults.clone(),
        }),
        Arc::new(SystemClock),
        EngineConfig::default(),
        None,
    );

    // Someone made the folder a vault (another device, another tool) before the first run.
    let (_, other) = s.create("Elsewhere").await;
    let json = s.drive.content("Elsewhere/vault.json").unwrap();
    s.drive.put_file("Shared/vault.json", &json);
    let _ = other;
    let before = s.drive.bytes_uploaded();
    let out = engine.sync("plain", Trigger::Manual).await;
    assert_ne!(RunStatus::Success, out.status);
    assert!(
        out.message
            .as_deref()
            .unwrap_or("")
            .contains("isn't linked to it"),
        "{:?}",
        out.message
    );
    assert_eq!(before, s.drive.bytes_uploaded(), "nothing went up unencrypted");
    assert!(
        !std::fs::read_dir(&root)
            .unwrap()
            .any(|e| e.unwrap().file_name() == "vault.json")
    );

    // A vault.json that can't be read (damaged, oversized) may be a vault's: refused too.
    for (name, content) in damaged_manifests() {
        s.drive.put_file("Shared/vault.json", &content);
        let out = engine.sync("plain", Trigger::Manual).await;
        assert_ne!(RunStatus::Success, out.status, "{name}");
        assert!(
            out.message.as_deref().unwrap_or("").contains("damaged or too"),
            "{name}: {:?}",
            out.message
        );
        assert_eq!(before, s.drive.bytes_uploaded(), "{name}: nothing went up");
    }

    // A plain folder with someone else's vault.json syncs as any other.
    s.drive
        .put_file("Shared/vault.json", br#"{"format":"another app"}"#);
    // (Started from its preview: a first sync merging two non-empty folders asks first.)
    let out = engine.sync("plain", Trigger::Preview).await;
    assert_eq!(RunStatus::Success, out.status, "{:?}", out.message);
    assert!(s.drive.bytes_uploaded() > before);
}

#[test]
fn only_a_final_folder_exclusion_lets_a_plain_pair_skip_a_nested_vault() {
    let mut pair = Pair::new("p", "P", "/tmp/p");
    assert!(!wasl_engine::excluded_by_pair_settings(&pair, "Work/Secret"));

    pair.excluded_subfolders = vec!["Work/Secret".into()];
    assert!(wasl_engine::excluded_by_pair_settings(&pair, "Work/Secret"));
    pair.excluded_subfolders = vec!["Work".into()];
    assert!(
        wasl_engine::excluded_by_pair_settings(&pair, "Work/Secret"),
        "through a folder above"
    );

    // A file-only include list rejects the name as a file, not the folder: the planner still
    // walks into it, so the guard must not skip it.
    pair.excluded_subfolders.clear();
    pair.include_patterns = vec!["*.txt".into()];
    assert!(!wasl_engine::excluded_by_pair_settings(&pair, "Work/Secret"));
    // Excluding vault.json alone leaves the vault's other files in: not a skip either.
    pair.include_patterns.clear();
    pair.exclude_patterns = vec!["vault.json".into()];
    assert!(!wasl_engine::excluded_by_pair_settings(&pair, "Work/Secret"));
}

#[tokio::test]
async fn a_vault_in_a_subfolder_of_a_plain_pair_stops_it_until_the_subfolder_is_excluded() {
    let s = setup();
    let folder = s.folder("Work");
    let root = s.dir.path().join("Work here");
    std::fs::create_dir_all(root.join("Secret")).unwrap();
    std::fs::write(root.join("a.txt"), "plain").unwrap();
    std::fs::write(root.join("Secret").join("b.txt"), "must never go up in plaintext").unwrap();
    let mut pair = plain_pair(&s, &folder, &root);
    s.store.lock().create_pair(&pair).unwrap();
    let engine = Engine::new(
        s.store.clone(),
        Arc::new(PairEnv {
            root: root.clone(),
            bin: s.dir.path().join("bin"),
            drive: s.drive.clone(),
            vaults: s.vaults.clone(),
        }),
        Arc::new(SystemClock),
        EngineConfig::default(),
        None,
    );

    // A vault sits in Work/Secret on Drive.
    let (_, _) = s.create("Elsewhere").await;
    let json = s.drive.content("Elsewhere/vault.json").unwrap();
    s.drive.put_file("Work/Secret/vault.json", &json);
    let before = s.drive.bytes_uploaded();
    let out = engine.sync("plain", Trigger::Preview).await;
    assert_ne!(RunStatus::Success, out.status);
    let msg = out.message.unwrap_or_default();
    assert!(msg.contains("Secret") && msg.contains("Exclude"), "{msg}");
    assert_eq!(before, s.drive.bytes_uploaded(), "nothing went up unencrypted");

    // Excluded from the pair, the vault folder is left alone and the rest syncs.
    pair.excluded_subfolders = vec!["Secret".into()];
    s.store.lock().update_pair(&pair).unwrap();
    let out = engine.sync("plain", Trigger::Preview).await;
    assert_eq!(RunStatus::Success, out.status, "{:?}", out.message);
    assert!(s.drive.content("Work/a.txt").is_some());
    assert!(
        s.drive.content("Work/Secret/b.txt").is_none(),
        "the vault got no plaintext"
    );
}

#[tokio::test]
async fn a_plain_view_whose_folder_becomes_a_vault_is_shown_through_the_vault() {
    let v = views(setup());
    let s = &v.s;
    let folder = s.folder("Shared");
    s.drive.put_file("Shared/a.txt", b"plain");
    let view = v.views.add(v.new_view(&folder, "Shared"), &[]).await.unwrap();
    assert!(v.state(&view.id).mounted);
    assert_eq!(None, view.vault_id);

    let (_, info) = s.create("Elsewhere").await;
    s.drive.put_file(
        "Shared/vault.json",
        &s.drive.content("Elsewhere/vault.json").unwrap(),
    );
    s.vaults.lock(&info.id).unwrap();
    let e = v.views.refresh_now(&view.id).await.unwrap_err();
    assert_eq!(ErrorKind::Locked, e.kind);
    assert_eq!(
        Some(info.id.clone()),
        s.store.lock().view(&view.id).unwrap().unwrap().vault_id
    );
    assert_eq!(vec!["a.txt"], v.shown(&view.id), "its ciphertext was never shown");
    // Mounted again through the vault, which is locked here: nothing shows.
    v.views.tick().await;
    let st = v.state(&view.id);
    assert!(!st.mounted);
    assert_eq!(Some(VAULT_LOCKED), st.view.last_error.as_deref());
}

#[tokio::test]
async fn a_view_of_a_folder_with_a_damaged_vault_json_shows_nothing() {
    let v = views(setup());
    let s = &v.s;
    for (name, content) in damaged_manifests() {
        let folder = s.folder(name);
        s.drive.put_file(&format!("{name}/a.dsv"), b"ciphertext, maybe");
        s.drive.put_file(&format!("{name}/vault.json"), &content);
        let view = v.views.add(v.new_view(&folder, name), &[]).await.unwrap();
        let st = v.state(&view.id);
        assert!(!st.mounted, "{name}");
        assert!(
            st.view
                .last_error
                .as_deref()
                .unwrap_or("")
                .contains("damaged or too large"),
            "{name}: {:?}",
            st.view.last_error
        );
        assert!(v.shown(&view.id).is_empty(), "{name}");
    }
}

// ---------------------------------------------------------------- S4: upgrade, unlocked state

/// Counts how often a vault's secret is read.
#[derive(Default)]
struct CountingSecrets {
    inner: MemorySecrets,
    gets: std::sync::atomic::AtomicUsize,
}

impl Secrets for CountingSecrets {
    fn get(&self, account: &str) -> Option<String> {
        self.gets.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.get(account)
    }
    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        self.inner.set(account, secret)
    }
    fn remove(&self, account: &str) {
        self.inner.remove(account)
    }
}

#[tokio::test]
async fn showing_whether_a_vault_is_unlocked_does_not_read_its_keys_each_time() {
    let s = setup();
    let keychain = Arc::new(CountingSecrets::default());
    let vaults =
        Vaults::new(s.store.clone(), keychain.clone(), s.dir.path().join("t2")).with_kdf(KdfParams::MIN);
    let info = vaults
        .create(
            s.drive.as_ref(),
            s.folder("Counted"),
            "Counted",
            Passphrase::new(PASS),
        )
        .await
        .unwrap();
    let gets = || keychain.gets.load(std::sync::atomic::Ordering::SeqCst);
    let before = gets();
    for _ in 0..5 {
        assert!(vaults.list().unwrap()[0].unlocked);
        assert!(vaults.info(&info.id).unwrap().unlocked);
    }
    assert_eq!(before, gets(), "known since it was made here");
    vaults.lock(&info.id).unwrap();
    assert!(!vaults.info(&info.id).unwrap().unlocked);
    assert_eq!(before, gets());

    // After a restart each vault's entry is read once, then remembered.
    vaults.unlock(&info.id, Passphrase::new(PASS)).await.unwrap();
    let restarted = Vaults::new(s.store.clone(), keychain.clone(), s.dir.path().join("t3"));
    let before = gets();
    for _ in 0..5 {
        assert!(restarted.list().unwrap()[0].unlocked);
    }
    assert_eq!(before + 1, gets());
}

#[tokio::test]
async fn a_v1_vault_is_upgraded_into_a_new_folder_and_the_old_one_is_left_alone() {
    let s = setup();
    // A v1 vault made on the phone, with a folder and two files.
    let salt = [6u8; 16];
    let iterations = wasl_crypto::manifest::MIN_ITERATIONS as u32;
    let v1_keys = kdf::derive(PASS, &salt, iterations).unwrap();
    let manifest = VaultManifest::create("Old", &salt, iterations, &kdf::key_check(&v1_keys), 0);
    let old_folder = s.folder("Old");
    s.drive.put_file("Old/vault.json", manifest.to_json().as_bytes());
    let writer = VaultRemote::new(s.drive.clone(), Arc::new(v1_keys), s.dir.path().join("w"));
    let big = pseudo((3 << 20) + 7, 4);
    upload(
        &writer,
        &s.file("big.bin", &big),
        "big.bin",
        &old_folder.folder_id,
    )
    .await;
    upload(&writer, &s.file("n.txt", b"note"), "n.txt", &old_folder.folder_id).await;
    let old = s
        .vaults
        .unlock_folder(s.drive.as_ref(), old_folder.clone(), Passphrase::new(PASS))
        .await
        .unwrap();
    assert_eq!((1, "1"), (old.version, old.format.as_str()));
    let old_bytes: Vec<_> = s
        .drive
        .files()
        .into_iter()
        .filter(|(k, _)| k.starts_with("Old/"))
        .collect();

    let drive: Arc<dyn Remote> = s.drive.clone();
    let target = s.folder("New");
    // Refused: a wrong passphrase, the vault's own folder.
    let e = s
        .vaults
        .upgrade(
            drive.clone(),
            &old.id,
            target.clone(),
            Passphrase::new("not it at all"),
        )
        .await
        .unwrap_err();
    assert_eq!(WRONG_PASSPHRASE, e.message);
    let e = s
        .vaults
        .upgrade(drive.clone(), &old.id, old_folder.clone(), Passphrase::new(PASS))
        .await
        .unwrap_err();
    assert_eq!(ErrorKind::Invalid, e.kind);
    assert!(
        s.drive.files().keys().all(|k| !k.starts_with("New/")),
        "nothing made yet"
    );
    // A folder holding another 2.1 vault the same passphrase opens is not "an interrupted
    // upgrade": nothing is copied into it.
    let taken = s.folder("Taken");
    let other = s
        .vaults
        .create(s.drive.as_ref(), taken.clone(), "Other", Passphrase::new(PASS))
        .await
        .unwrap();
    let before: Vec<_> = s
        .drive
        .files()
        .into_iter()
        .filter(|(k, _)| k.starts_with("Taken/"))
        .collect();
    let e = s
        .vaults
        .upgrade(drive.clone(), &old.id, taken.clone(), Passphrase::new(PASS))
        .await
        .unwrap_err();
    assert_eq!(ErrorKind::Invalid, e.kind, "{e:?}");
    assert!(e.message.contains("Taken"), "{}", e.message);
    let after: Vec<_> = s
        .drive
        .files()
        .into_iter()
        .filter(|(k, _)| k.starts_with("Taken/"))
        .collect();
    assert_eq!(before, after, "nothing written into the other vault");
    assert!(s.vaults.info(&other.id).is_ok());

    let (new, report) = s
        .vaults
        .upgrade(drive.clone(), &old.id, target.clone(), Passphrase::new(PASS))
        .await
        .unwrap();
    assert_eq!((2, true, "Old"), (new.version, new.unlocked, new.name.as_str()));
    // E2E 2026-10: `wasl vault list` said "v2" for 2.0 and 2.1 alike.
    assert_eq!("2.1", new.format);
    assert_ne!(old.id, new.id);
    assert_eq!(2, report.files, "{report:?}");
    let json = s.drive.content("New/vault.json").unwrap();
    let json = VaultManifestV2::parse(std::str::from_utf8(&json).unwrap()).unwrap();
    assert_eq!(wasl_crypto::v2::Format::V2_1, json.format());

    // The new vault reads back; the old folder is untouched and its vault still unlocked.
    let reader = VaultV2Remote::new(s.drive.clone(), s.v2_keys(&new.id), s.dir.path().join("r"));
    let listing = reader.list_subtree(&target.folder_id, None).await.unwrap();
    let big_entry = listing.entries.iter().find(|e| e.key == "big.bin").unwrap();
    let dest = s.dir.path().join("big.out");
    reader.download(big_entry, &dest, &|_| {}).await.unwrap();
    assert_eq!(big, std::fs::read(&dest).unwrap());
    let after: Vec<_> = s
        .drive
        .files()
        .into_iter()
        .filter(|(k, _)| k.starts_with("Old/"))
        .collect();
    assert_eq!(old_bytes, after);
    assert!(s.vaults.info(&old.id).unwrap().unlocked);

    // Running it again continues the same new vault and copies nothing twice.
    let (again, report) = s
        .vaults
        .upgrade(drive.clone(), &old.id, target.clone(), Passphrase::new(PASS))
        .await
        .unwrap();
    assert_eq!(new.id, again.id);
    assert_eq!((0, 2), (report.files, report.already_there), "{report:?}");
    // The new vault is already the newest format.
    let e = s
        .vaults
        .upgrade(drive, &new.id, s.folder("Newer"), Passphrase::new(PASS))
        .await
        .unwrap_err();
    assert_eq!(ErrorKind::Invalid, e.kind);
}
