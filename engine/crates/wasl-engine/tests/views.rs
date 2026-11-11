//! What read-only views read from: any remote by range, a vault v1 file a chunk at a time, and
//! the tree built from a listing.

use std::sync::Arc;

use wasl_crypto::content::CHUNK_BYTES;
use wasl_crypto::{VaultCipher, VaultKeys};
use wasl_engine::Remote;
use wasl_engine::{MemoryRemote, ROOT_ID};
use wasl_engine::{RemoteSource, VaultV1Source, view_tree};
use wasl_vfs::{BlockCache, FileRef, Hydrator, ReadAhead, Source, VfsError};

fn content(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i * 13 % 251) as u8).collect()
}

fn file(id: &str, size: u64) -> FileRef {
    FileRef {
        id: id.into(),
        version: "1".into(),
        size,
    }
}

#[tokio::test]
async fn a_remote_source_reads_ranges_and_notices_a_shorter_file() {
    let remote = Arc::new(MemoryRemote::new());
    let body = content(10_000);
    let id = remote.put_file("a.bin", &body);
    let src = RemoteSource(remote.clone());

    assert_eq!(
        body[100..600],
        src.read(&file(&id, 10_000), 100, 500).await.unwrap()[..]
    );
    // Past the end: clipped to the listed size.
    assert_eq!(
        body[9_900..],
        src.read(&file(&id, 10_000), 9_900, 500).await.unwrap()[..]
    );
    assert_eq!(600, remote.bytes_downloaded());

    // The listing said bigger than Drive has: the file changed, which is not silently short data.
    let err = src.read(&file(&id, 20_000), 9_000, 5_000).await.unwrap_err();
    assert!(matches!(err, VfsError::Integrity(_)), "{err:?}");
}

fn cipher() -> Arc<VaultCipher> {
    Arc::new(VaultCipher::new(&VaultKeys::new([0x11; 32], [0x22; 32])))
}

fn encrypt(c: &VaultCipher, plain: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    c.encrypt(plain, &mut out).unwrap();
    out
}

#[tokio::test]
async fn a_vault_v1_seek_fetches_and_decrypts_only_the_chunks_it_needs() {
    let c = cipher();
    let size = 5 * CHUNK_BYTES + 777;
    let plain = content(size);
    let remote = Arc::new(MemoryRemote::new());
    let id = remote.put_file("x.enc", &encrypt(&c, &plain));
    let src = VaultV1Source::new(remote.clone(), c.clone());
    let f = file(&id, size as u64);

    // A range across chunks 2 and 3.
    let at = 3 * CHUNK_BYTES - 10;
    assert_eq!(plain[at..at + 20], src.read(&f, at as u64, 20).await.unwrap()[..]);
    let header = 20; // magic + file nonce
    let fetched = remote.bytes_downloaded();
    assert!(
        fetched <= (header + 2 * (CHUNK_BYTES + 16)) as u64,
        "fetched {fetched} bytes for a 20-byte read"
    );

    // The short last chunk, and everything through the hydrator.
    assert_eq!(
        plain[size - 777..],
        src.read(&f, (size - 777) as u64, 5_000).await.unwrap()[..]
    );
    let dir = tempfile::tempdir().unwrap();
    let cache = Arc::new(BlockCache::open(dir.path(), 64 << 20).unwrap());
    let h = Hydrator::new(
        Arc::new(VaultV1Source::new(remote.clone(), c.clone())),
        cache,
        ReadAhead::default(),
    );
    assert_eq!(plain, h.read(&f, 0, size as u64).await.unwrap());
}

#[tokio::test]
async fn a_tampered_or_foreign_vault_v1_chunk_is_refused() {
    let c = cipher();
    let size = 3 * CHUNK_BYTES;
    let plain = content(size);
    let mut enc = encrypt(&c, &plain);
    enc[20 + CHUNK_BYTES + 16 + 5] ^= 1; // a byte of chunk 1
    let remote = Arc::new(MemoryRemote::new());
    let id = remote.put_file("x.enc", &enc);
    let src = VaultV1Source::new(remote.clone(), c.clone());
    let f = file(&id, size as u64);
    assert_eq!(plain[..100], src.read(&f, 0, 100).await.unwrap()[..]);
    let err = src.read(&f, CHUNK_BYTES as u64, 100).await.unwrap_err();
    assert!(matches!(err, VfsError::Integrity(_)), "{err:?}");

    // Another vault's key.
    let other = Arc::new(VaultCipher::new(&VaultKeys::new([0x33; 32], [0x44; 32])));
    let err = VaultV1Source::new(remote, other)
        .read(&f, 0, 10)
        .await
        .unwrap_err();
    assert!(matches!(err, VfsError::Integrity(_)), "{err:?}");

    // A listing that claims a different size moves the last-chunk flag: refused, not misread.
    let id2 = {
        let r = Arc::new(MemoryRemote::new());
        let id = r.put_file("y.enc", &encrypt(&c, &plain[..CHUNK_BYTES + 50]));
        let wrong = file(&id, CHUNK_BYTES as u64 + 40);
        let err = VaultV1Source::new(r, c.clone())
            .read(&wrong, CHUNK_BYTES as u64, 10)
            .await;
        assert!(err.is_err());
        id
    };
    assert!(!id2.is_empty());
}

#[tokio::test]
async fn a_view_tree_has_files_and_folders_but_not_google_docs() {
    let remote = MemoryRemote::new();
    let a = remote.put_file("Photos/a.jpg", b"jpeg");
    remote.put_google_doc("Photos/notes", "application/vnd.google-apps.document");
    let listing = remote.list_subtree(ROOT_ID, None).await.unwrap();
    let tree = view_tree(&listing.entries);

    let photos = tree.get("Photos").expect("folder");
    assert!(photos.is_dir && photos.file.is_none());
    let jpg = tree.get("Photos/a.jpg").expect("file");
    let f = jpg.file.as_ref().unwrap();
    assert_eq!((a.as_str(), 4), (f.id.as_str(), f.size));
    assert!(!f.version.is_empty());
    assert!(tree.get("Photos/notes").is_none());
    assert_eq!(1, tree.children("Photos").count());
}

#[tokio::test]
async fn a_vault_v1_file_shorter_than_listed_is_an_error_not_a_crash() {
    let c = cipher();
    let plain = content(CHUNK_BYTES + 50);
    let remote = Arc::new(MemoryRemote::new());
    let id = remote.put_file("x.enc", &encrypt(&c, &plain));
    let src = VaultV1Source::new(remote, c);
    // The listing says 50 bytes more than Drive holds: the last chunk comes back short.
    let listed = file(&id, CHUNK_BYTES as u64 + 100);
    let err = src.read(&listed, CHUNK_BYTES as u64, 100).await.unwrap_err();
    assert!(matches!(err, VfsError::Integrity(_)), "{err:?}");
    // A read near u64::MAX does not overflow.
    assert!(src.read(&listed, u64::MAX - 1, 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_file_that_changes_between_two_reads_is_an_error_never_a_mix() {
    let remote = Arc::new(MemoryRemote::new());
    let old = content(4 * 1024 * 1024);
    remote.put_file("v.bin", &old);
    let listed = |r: &Arc<MemoryRemote>| {
        let r = r.clone();
        async move {
            let listing = r.list_subtree(ROOT_ID, None).await.unwrap();
            view_tree(&listing.entries)
                .get("v.bin")
                .unwrap()
                .file
                .clone()
                .unwrap()
        }
    };
    let f = listed(&remote).await;
    let src = RemoteSource(remote.clone());
    assert_eq!(old[..100], src.read(&f, 0, 100).await.unwrap()[..]);

    // Same size, new content, between two reads of the view's listed version.
    let new: Vec<u8> = old.iter().map(|b| b.wrapping_add(1)).collect();
    remote.put_file("v.bin", &new);
    let err = src.read(&f, 2 * 1024 * 1024, 100).await.unwrap_err();
    assert!(matches!(err, VfsError::Integrity(_)), "{err:?}");

    // Through the hydrator too: nothing of the new content is cached under the old version.
    let dir = tempfile::tempdir().unwrap();
    let cache = Arc::new(BlockCache::open(dir.path(), 64 << 20).unwrap());
    let h = Hydrator::new(
        Arc::new(RemoteSource(remote.clone())),
        cache.clone(),
        ReadAhead::default(),
    );
    assert!(h.read(&f, 0, 100).await.is_err());
    assert_eq!(0, cache.usage().0);

    // The next listing names the new content, which reads.
    let f2 = listed(&remote).await;
    assert_ne!(f.version, f2.version);
    assert_eq!(new[..100], h.read(&f2, 0, 100).await.unwrap()[..]);
}
