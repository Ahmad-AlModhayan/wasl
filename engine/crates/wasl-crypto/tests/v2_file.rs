//! Vault v2 files: streaming encryption, range reads (FR-V2 acceptance a), edit cost (FR-V2
//! acceptance b), update plans, and recovery of whole folders.

mod common;

use common::{assert_integrity, random_bytes};
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::io;
use wasl_crypto::v2::chunks::{ChunkId, open_chunk, seal_chunk};
use wasl_crypto::v2::manifest::{ChunkRef, FileId, FileManifest};
use wasl_crypto::v2::{RangeReader, VaultKeysV2, decrypt_file, encrypt_stream, plan_update, read_range};

const MIB: usize = 1 << 20;

fn keys() -> VaultKeysV2 {
    VaultKeysV2::from_ikm(&[0x55; 32])
}

type Store = HashMap<ChunkId, Vec<u8>>;

fn encrypt(k: &VaultKeysV2, fid: FileId, data: &[u8], store: &mut Store) -> FileManifest {
    encrypt_stream(
        k,
        wasl_crypto::v2::Format::V2_0,
        fid,
        1_760_000_000_000,
        data,
        |c| {
            store.entry(*c.id()).or_insert(c.seal()?);
            Ok(())
        },
    )
    .unwrap()
}

fn fetcher(store: &Store) -> impl FnMut(&ChunkId) -> io::Result<Vec<u8>> + '_ {
    |id| {
        store
            .get(id)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "missing"))
    }
}

#[test]
fn ranges_read_exactly_the_covering_chunks() {
    let k = keys();
    let data = random_bytes(23 * MIB + 777, 11);
    let mut store = Store::new();
    let m = encrypt(&k, FileId::random().unwrap(), &data, &mut store);
    assert!(m.chunks.len() > 5, "{}", m.chunks.len());
    let reader = RangeReader::new(&k, &m);
    let offsets = m.offsets();
    let size = data.len() as u64;
    let cases = [
        (0, 0),
        (0, 1),
        (0, 100),
        (offsets[1] - 1, 2),
        (offsets[2], 1),
        (offsets[3] - 10, (offsets[5] - offsets[3]) + 20),
        (size - 5, 5),
        (size - 5, 1000),
        (size, 10),
        (size + 10, 10),
        (12345, u64::MAX),
    ];
    for (off, len) in cases {
        let fetched = Cell::new(0usize);
        let got = reader
            .read(off, len, |id| {
                fetched.set(fetched.get() + 1);
                fetcher(&store)(id)
            })
            .unwrap();
        let end = off.saturating_add(len).min(size);
        let want = if off >= size {
            &[][..]
        } else {
            &data[off as usize..end as usize]
        };
        assert_eq!(want, &got[..], "{off}+{len}");
        assert_eq!(reader.covering(off, len).len(), fetched.get());
    }
    assert_eq!(1, reader.covering(offsets[2], 1).len());
    assert_eq!(2, reader.covering(offsets[1] - 1, 2).len());
    assert_eq!(0..0, reader.covering(size, 1));
    // The free function agrees.
    assert_eq!(
        &data[100..200],
        &read_range(&k, &m, 100, 100, fetcher(&store)).unwrap()[..]
    );
    // A missing chunk is an I/O error, a damaged one an integrity error.
    let mut broken = store.clone();
    broken.remove(&m.chunks[0].id);
    assert!(matches!(
        read_range(&k, &m, 0, 10, fetcher(&broken)),
        Err(wasl_crypto::CryptoError::Io(_))
    ));
    let mut damaged = store.clone();
    damaged.get_mut(&m.chunks[1].id).unwrap()[40] ^= 1;
    assert_integrity(read_range(&k, &m, offsets[1], 10, fetcher(&damaged)));
}

/// FR-V2 acceptance (a): seeking to 4 GiB in a large file downloads and decrypts one chunk. The
/// file (5 GiB + a tail) is synthetic: chunk `i` is 4 MiB of the byte `i % 7` (the tail: 1000
/// bytes of 0xEE), generated only when fetched.
#[test]
fn a_read_at_4_gib_fetches_exactly_one_chunk() {
    let k = keys();
    let fid = FileId([0x42; 16]);
    let full = 4 * MIB;
    let n_full = 1280; // 5 GiB
    let content = |i: usize| -> Vec<u8> {
        if i < n_full {
            vec![(i % 7) as u8; full]
        } else {
            vec![0xEE; 1000]
        }
    };
    let ids: Vec<ChunkId> = (0..7).map(|b| ChunkId::compute(&k, &fid, &content(b))).collect();
    let tail_id = ChunkId::compute(&k, &fid, &content(n_full));
    let mut chunks: Vec<ChunkRef> = (0..n_full)
        .map(|i| ChunkRef {
            id: ids[i % 7],
            len: full as u32,
        })
        .collect();
    chunks.push(ChunkRef {
        id: tail_id,
        len: 1000,
    });
    let m = FileManifest {
        file_id: fid,
        size: (n_full * full + 1000) as u64,
        mtime_ms: 0,
        blake3: [0; 32], // not checked by range reads
        chunks,
        version: 1,
    };
    m.validate().unwrap();
    // Round trip through the sealed manifest, as a reader would get it.
    let here = wasl_crypto::v2::Placement::for_name(&k, &wasl_crypto::v2::DirId::ROOT, "big.iso").unwrap();
    let m = FileManifest::open(&k, &here, &m.seal(&k, &here).unwrap()).unwrap();

    let fetches = Cell::new(0usize);
    let fetch = |id: &ChunkId| -> io::Result<Vec<u8>> {
        fetches.set(fetches.get() + 1);
        let i = if *id == tail_id {
            n_full
        } else {
            ids.iter().position(|x| x == id).unwrap()
        };
        Ok(seal_chunk(&k, wasl_crypto::v2::Format::V2_0, &fid, id, &content(i)).unwrap())
    };
    let reader = RangeReader::new(&k, &m);
    let four_gib: u64 = 4 << 30;
    let got = reader.read(four_gib + 100, 64 * 1024, fetch).unwrap();
    assert_eq!(1, fetches.get(), "a read at 4 GiB touches exactly one chunk");
    assert_eq!(vec![(1024 % 7) as u8; 64 * 1024], got);
    assert_eq!(1024..1025, reader.covering(four_gib + 100, 64 * 1024));

    // Across a boundary: two chunks; at the very end: the tail only.
    fetches.set(0);
    let fetch = |id: &ChunkId| -> io::Result<Vec<u8>> {
        fetches.set(fetches.get() + 1);
        let i = if *id == tail_id {
            n_full
        } else {
            ids.iter().position(|x| x == id).unwrap()
        };
        Ok(seal_chunk(&k, wasl_crypto::v2::Format::V2_0, &fid, id, &content(i)).unwrap())
    };
    let mut fetch = fetch;
    let got = reader.read(four_gib - 2, 4, &mut fetch).unwrap();
    let (a, b) = ((1023 % 7) as u8, (1024 % 7) as u8);
    assert_eq!(vec![a, a, b, b], got);
    assert_eq!(2, fetches.get());
    fetches.set(0);
    let got = reader.read(m.size - 10, 100, &mut fetch).unwrap();
    assert_eq!(vec![0xEE; 10], got);
    assert_eq!(1, fetches.get());
}

fn upload_cost(k: &VaultKeysV2, fid: FileId, old: &[u8], new: &[u8]) -> (u64, usize, usize) {
    let mut store = Store::new();
    let m_old = encrypt(k, fid, old, &mut store);
    let m_new = wasl_crypto::v2::encrypt_update(k, wasl_crypto::v2::Format::V2_0, &m_old, 0, new, |_| Ok(()))
        .unwrap();
    assert_eq!(m_old.version + 1, m_new.version);
    let plan = plan_update(&m_old, &m_new);
    // Upload what the plan says, then the new version decrypts.
    let mut sealed = 0u64;
    let wanted: std::collections::HashSet<ChunkId> = plan.upload.iter().map(|c| c.id).collect();
    encrypt_stream(k, wasl_crypto::v2::Format::V2_0, fid, 0, new, |c| {
        if wanted.contains(c.id()) {
            let s = c.seal()?;
            sealed += s.len() as u64;
            store.insert(*c.id(), s);
        }
        Ok(())
    })
    .unwrap();
    let mut out = Vec::new();
    decrypt_file(k, &m_new, fetcher(&store), &mut out).unwrap();
    assert_eq!(new, &out[..]);
    for id in &plan.delete {
        store.remove(id);
    }
    let mut out = Vec::new();
    decrypt_file(k, &m_new, fetcher(&store), &mut out).unwrap();
    assert_eq!(sealed, plan.upload_bytes() + 32 * plan.upload.len() as u64);
    (plan.upload_bytes(), plan.upload.len(), plan.delete.len())
}

/// FR-V2 acceptance (b): a 1 MB change in a 64 MiB file re-uploads far less than 32 MB.
#[test]
fn a_1_mb_edit_of_a_64_mib_file_uploads_a_few_mib() {
    let k = keys();
    let fid = FileId::random().unwrap();
    let base = random_bytes(64 * MIB, 64);
    let patch = random_bytes(1_000_000, 65);

    let mut overwritten = base.clone();
    overwritten[30 * MIB..30 * MIB + patch.len()].copy_from_slice(&patch);
    let (bytes, up, del) = upload_cost(&k, fid, &base, &overwritten);
    println!("overwrite 1 MB at 30 MiB: upload {bytes} bytes in {up} chunks, delete {del}");
    assert!(bytes < 32_000_000, "{bytes}");
    assert!(bytes <= 12 * MIB as u64, "in practice a few MiB: {bytes}");

    let mut inserted = base[..10 * MIB].to_vec();
    inserted.extend_from_slice(&patch);
    inserted.extend_from_slice(&base[10 * MIB..]);
    let (bytes, up, del) = upload_cost(&k, fid, &base, &inserted);
    println!("insert 1 MB at 10 MiB: upload {bytes} bytes in {up} chunks, delete {del}");
    assert!(bytes < 32_000_000, "{bytes}");
    assert!(bytes <= 12 * MIB as u64, "in practice a few MiB: {bytes}");

    let mut appended = base.clone();
    appended.extend_from_slice(&patch);
    let (bytes, _, _) = upload_cost(&k, fid, &base, &appended);
    assert!(bytes <= 12 * MIB as u64, "{bytes}");

    // Unchanged: nothing to do.
    let (bytes, up, del) = upload_cost(&k, fid, &base, &base);
    assert_eq!((0, 0, 0), (bytes, up, del));
}

#[test]
fn plan_update_dedupes_and_handles_new_file_ids() {
    let k = keys();
    let data = random_bytes(9 * MIB, 3);
    let a = encrypt_stream(
        &k,
        wasl_crypto::v2::Format::V2_0,
        FileId([1; 16]),
        0,
        &data[..],
        |_| Ok(()),
    )
    .unwrap();
    let b = encrypt_stream(
        &k,
        wasl_crypto::v2::Format::V2_0,
        FileId([2; 16]),
        0,
        &data[..],
        |_| Ok(()),
    )
    .unwrap();
    let plan = plan_update(&a, &b);
    assert_eq!(
        b.chunks.len(),
        plan.upload.len(),
        "another file id shares nothing"
    );
    assert_eq!(a.chunks.len(), plan.delete.len());
    // Repeated content inside one file is one object, uploaded once.
    let zeros = vec![0u8; 20 * MIB];
    let mut handed = 0;
    let z = encrypt_stream(
        &k,
        wasl_crypto::v2::Format::V2_0,
        FileId([3; 16]),
        0,
        &zeros[..],
        |_| {
            handed += 1;
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(5, z.chunks.len());
    assert_eq!(1, handed);
    let empty = FileManifest {
        size: 0,
        chunks: vec![],
        blake3: *blake3::hash(b"").as_bytes(),
        ..z.clone()
    };
    let plan = plan_update(&empty, &z);
    assert_eq!(1, plan.upload.len());
    assert_eq!(4 * MIB as u64, plan.upload_bytes());
    let plan = plan_update(&z, &empty);
    assert_eq!((0, 1), (plan.upload.len(), plan.delete.len()));
}

#[test]
fn empty_and_small_files() {
    let k = keys();
    let mut store = Store::new();
    let m = encrypt(&k, FileId([5; 16]), b"", &mut store);
    assert_eq!((0, 0), (m.size, m.chunks.len()));
    assert_eq!(*blake3::hash(b"").as_bytes(), m.blake3);
    let mut out = Vec::new();
    assert_eq!(0, decrypt_file(&k, &m, fetcher(&store), &mut out).unwrap());
    assert!(read_range(&k, &m, 0, 10, fetcher(&store)).unwrap().is_empty());
    let m = encrypt(&k, FileId([5; 16]), b"hello", &mut store);
    assert_eq!(1, m.chunks.len());
    assert_eq!(
        b"ell".to_vec(),
        read_range(&k, &m, 1, 3, fetcher(&store)).unwrap()
    );
}

/// Chunks swapped between files, reordered inside a file, or a file cut short never decrypt to
/// something else.
#[test]
fn swaps_reorders_and_truncations_are_refused() {
    let k = keys();
    let mut store = Store::new();
    let a = encrypt(&k, FileId([1; 16]), &random_bytes(12 * MIB, 1), &mut store);
    let b = encrypt(&k, FileId([2; 16]), &random_bytes(12 * MIB, 2), &mut store);
    assert!(a.chunks.len() >= 3 && b.chunks.len() >= 3);

    // Swap: file b's chunk object stored under file a's chunk name.
    let mut swapped = store.clone();
    let theirs = store[&b.chunks[0].id].clone();
    swapped.insert(a.chunks[0].id, theirs);
    assert_integrity(decrypt_file(&k, &a, fetcher(&swapped), io::sink()));

    // Reorder objects: chunk 1's object under chunk 0's name.
    let mut reordered = store.clone();
    let c1 = store[&a.chunks[1].id].clone();
    reordered.insert(a.chunks[0].id, c1);
    assert_integrity(decrypt_file(&k, &a, fetcher(&reordered), io::sink()));

    // Reorder the list (only a key holder can re-seal a manifest): the whole-file hash catches it.
    let mut list_reordered = a.clone();
    list_reordered.chunks.swap(0, 1);
    assert_integrity(decrypt_file(&k, &list_reordered, fetcher(&store), io::sink()));

    // A list cut short does not add up to the size.
    let mut short = a.clone();
    short.chunks.pop();
    assert_integrity(decrypt_file(&k, &short, fetcher(&store), io::sink()));
    assert_integrity(FileManifest::from_cbor(&short.to_cbor()));

    // A truncated chunk object.
    let mut cut = store.clone();
    let obj = cut.get_mut(&a.chunks[2].id).unwrap();
    obj.truncate(obj.len() - 1);
    assert_integrity(decrypt_file(&k, &a, fetcher(&cut), io::sink()));

    // A chunk of file a listed in file b's manifest.
    let mut mixed = b.clone();
    mixed.chunks[0] = a.chunks[0];
    assert!(open_chunk(&k, &b.file_id, &a.chunks[0].id, None, &store[&a.chunks[0].id]).is_err());
    assert!(decrypt_file(&k, &mixed, fetcher(&store), io::sink()).is_err());
}

// ---------------------------------------------------------------- recovery

fn tree(dir: &std::path::Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(dir: &std::path::Path, prefix: &str, out: &mut BTreeMap<String, Vec<u8>>) {
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
fn recover_decrypts_a_v2_folder() {
    use wasl_crypto::recover::{Reason, Unlocked, Vault, decrypt_tree};
    use wasl_crypto::v2::{KdfParams, VaultManifestV2, keys, layout};
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("plain");
    common::v2::sample_tree(&src);
    let vault = tmp.path().join("vault");
    let salt = keys::new_salt().unwrap();
    let k = keys::derive("pass", &salt, KdfParams::MIN).unwrap();
    std::fs::create_dir_all(&vault).unwrap();
    let vj = VaultManifestV2::create("Test", &salt, KdfParams::MIN, &k.key_check(), 0);
    std::fs::write(vault.join("vault.json"), vj.to_json()).unwrap();
    layout::write_tree(&k, &src, &vault).unwrap();
    std::fs::write(vault.join("dropped.txt"), b"x").unwrap();

    let v = Vault::load(&vault).unwrap();
    assert_eq!(2, v.version());
    assert!(v.unlock("nope").unwrap().is_none());
    let keys = v.unlock("pass").unwrap().unwrap();
    assert!(matches!(keys, Unlocked::V2(_)));
    let out = tmp.path().join("out");
    let mut events = 0;
    let s = decrypt_tree(&vault, &out, &keys, false, |_| events += 1).unwrap();
    assert!(s.failed.is_empty(), "{:?}", s.failed);
    assert_eq!(vec![("dropped.txt".to_owned(), Reason::NotVaultItem)], s.skipped);
    assert_eq!(tree(&src), tree(&out));
    assert!(events > 0);
    // mtimes come back.
    let t_src = std::fs::metadata(src.join("Docs/notes.txt"))
        .unwrap()
        .modified()
        .unwrap();
    let t_out = std::fs::metadata(out.join("Docs/notes.txt"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(layout::system_time_ms(t_src), layout::system_time_ms(t_out));

    // A second run skips what exists; --overwrite replaces.
    let s = decrypt_tree(&vault, &out, &keys, false, |_| {}).unwrap();
    assert!(s.skipped.iter().any(|(_, r)| *r == Reason::AlreadyExists));
    let s = decrypt_tree(&vault, &out, &keys, true, |_| {}).unwrap();
    assert_eq!(1, s.skipped.len());

    // Damage: a flipped bit in one chunk fails that file only; a folder without its .wasl-dir
    // cannot be named; a missing chunk is reported.
    let chunk_dir = vault.join(".wasl-chunks");
    let mut objects: Vec<_> = walkdir(&chunk_dir);
    objects.sort();
    let victim = objects
        .iter()
        .max_by_key(|p| std::fs::metadata(p).unwrap().len())
        .unwrap()
        .clone();
    let mut bytes = std::fs::read(&victim).unwrap();
    bytes[50] ^= 1;
    std::fs::write(&victim, &bytes).unwrap();
    let out2 = tmp.path().join("out2");
    let s = decrypt_tree(&vault, &out2, &keys, false, |_| {}).unwrap();
    assert_eq!(1, s.failed.len(), "{:?}", s.failed);
    assert!(matches!(s.failed[0].1, Reason::Damaged(_)));
    assert!(
        !out2.join(&s.failed[0].0).exists(),
        "no unverified plaintext left"
    );
    std::fs::remove_file(&victim).unwrap();
    let s = decrypt_tree(&vault, &tmp.path().join("out3"), &keys, false, |_| {}).unwrap();
    assert!(matches!(s.failed[0].1, Reason::MissingChunk(_)), "{:?}", s.failed);
    for d in std::fs::read_dir(&vault).unwrap().flatten() {
        if d.path().join(".wasl-dir").exists() {
            std::fs::remove_file(d.path().join(".wasl-dir")).unwrap();
        }
    }
    let s = decrypt_tree(&vault, &tmp.path().join("out4"), &keys, false, |_| {}).unwrap();
    assert!(
        s.failed.iter().any(|(_, r)| *r == Reason::NoDirId),
        "{:?}",
        s.failed
    );
}

fn walkdir(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        if e.file_type().unwrap().is_dir() {
            out.extend(walkdir(&e.path()));
        } else {
            out.push(e.path());
        }
    }
    out
}

#[test]
fn recover_decrypts_a_v1_folder() {
    use wasl_crypto::recover::{Unlocked, Vault, decrypt_tree};
    use wasl_crypto::{VaultCipher, VaultManifest, kdf, names};
    let tmp = tempfile::tempdir().unwrap();
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    let salt = kdf::new_salt().unwrap();
    let k = kdf::derive("pw", &salt, 100_000).unwrap();
    let vj = VaultManifest::create("v1", &salt, 100_000, &kdf::key_check(&k), 0);
    std::fs::write(vault.join("vault.json"), vj.to_json()).unwrap();
    let dir = vault.join(names::encrypt_name(&k, "root", "مجلد").unwrap());
    std::fs::create_dir_all(&dir).unwrap();
    let data = random_bytes(MIB + 5, 9);
    let c = VaultCipher::new(&k);
    c.encrypt(
        &data[..],
        std::fs::File::create(dir.join(names::encrypt_name(&k, "d", "a.bin").unwrap())).unwrap(),
    )
    .unwrap();
    let v = Vault::load(&vault).unwrap();
    assert_eq!(1, v.version());
    let keys = v.unlock("pw").unwrap().unwrap();
    assert!(matches!(keys, Unlocked::V1(_)));
    let out = tmp.path().join("out");
    let s = decrypt_tree(&vault, &out, &keys, false, |_| {}).unwrap();
    assert!(s.failed.is_empty() && s.skipped.is_empty());
    assert_eq!(data, std::fs::read(out.join("مجلد").join("a.bin")).unwrap());
}

/// A manifest moved into another folder, two manifests swapped, a folder marker altered: each is
/// refused by recovery instead of producing a wrong tree.
#[test]
fn recover_refuses_moved_manifests_and_tampered_markers() {
    use wasl_crypto::recover::{Reason, Unlocked, decrypt_tree};
    use wasl_crypto::v2::layout;
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("plain");
    std::fs::create_dir_all(src.join("A")).unwrap();
    std::fs::write(src.join("one.txt"), b"one").unwrap();
    std::fs::write(src.join("two.txt"), b"two").unwrap();
    std::fs::write(src.join("A").join("inner.txt"), b"inner").unwrap();
    let vault = tmp.path().join("vault");
    let k = keys();
    layout::write_tree(&k, &src, &vault).unwrap();
    let unlocked = Unlocked::V2(k.clone());
    let root = wasl_crypto::v2::DirId::ROOT;
    let enc = |name: &str| wasl_crypto::v2::names::encrypt_name(&k, &root, name).unwrap();
    let (one, two, a) = (
        vault.join(enc("one.txt")),
        vault.join(enc("two.txt")),
        vault.join(enc("A")),
    );

    // Swap the two manifests.
    let (b1, b2) = (std::fs::read(&one).unwrap(), std::fs::read(&two).unwrap());
    std::fs::write(&one, &b2).unwrap();
    std::fs::write(&two, &b1).unwrap();
    let s = decrypt_tree(&vault, &tmp.path().join("o1"), &unlocked, false, |_| {}).unwrap();
    let failed: Vec<&str> = s.failed.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(vec!["one.txt", "two.txt"], failed);
    assert!(s.failed.iter().all(|(_, r)| matches!(r, Reason::Damaged(_))));
    std::fs::write(&one, &b1).unwrap();
    std::fs::write(&two, &b2).unwrap();

    // Move a manifest into folder A, keeping its encrypted name: it no longer decrypts there.
    std::fs::rename(&one, a.join(enc("one.txt"))).unwrap();
    let s = decrypt_tree(&vault, &tmp.path().join("o2"), &unlocked, false, |_| {}).unwrap();
    assert!(
        s.skipped
            .iter()
            .any(|(p, r)| p.starts_with("A/") && *r == Reason::NotVaultItem),
        "{s:?}"
    );
    std::fs::rename(a.join(enc("one.txt")), &one).unwrap();

    // Tamper with folder A's marker: point it at another id, or flip a bit.
    let marker = a.join(".wasl-dir");
    let good = std::fs::read(&marker).unwrap();
    let mut forged = good.clone();
    forged[4..20].copy_from_slice(&[7; 16]);
    std::fs::write(&marker, &forged).unwrap();
    let s = decrypt_tree(&vault, &tmp.path().join("o3"), &unlocked, false, |_| {}).unwrap();
    assert_eq!(vec![("A".to_owned(), Reason::NoDirId)], s.failed);
    let mut flipped = good.clone();
    flipped[30] ^= 0x10;
    std::fs::write(&marker, &flipped).unwrap();
    let s = decrypt_tree(&vault, &tmp.path().join("o4"), &unlocked, false, |_| {}).unwrap();
    assert_eq!(vec![("A".to_owned(), Reason::NoDirId)], s.failed);
    std::fs::write(&marker, &good).unwrap();
    let s = decrypt_tree(&vault, &tmp.path().join("o5"), &unlocked, false, |_| {}).unwrap();
    assert!(s.failed.is_empty() && s.skipped.is_empty(), "{s:?}");
    assert_eq!(3, s.files);
}
