//! v1 random access (files on demand): each chunk decrypts on its own, at its own position only.

mod common;

use common::{assert_integrity, fixed_keys, other_keys, random_bytes};
use wasl_crypto::VaultCipher;
use wasl_crypto::content::{
    CHUNK_BYTES, HEADER_BYTES, TAG_BYTES, chunk_cipher_range, chunk_count, chunk_index_of, chunk_plain_range,
    encrypted_size, is_last_chunk, read_header,
};

const MIB: usize = CHUNK_BYTES;

fn encrypt(plain: &[u8], c: &VaultCipher) -> Vec<u8> {
    let mut out = Vec::new();
    c.encrypt(plain, &mut out).unwrap();
    out
}

/// Decrypts every chunk separately and concatenates the plaintexts.
fn by_chunks(ct: &[u8], plain_size: u64, c: &VaultCipher) -> Vec<u8> {
    let nonce = read_header(ct).unwrap();
    let mut out = Vec::new();
    for i in 0..chunk_count(plain_size) as u32 {
        let (off, len) = chunk_cipher_range(plain_size, i).unwrap();
        let bytes = &ct[off as usize..(off + len) as usize];
        let plain = c
            .decrypt_chunk(&nonce, i, is_last_chunk(plain_size, i), bytes)
            .unwrap();
        let (poff, plen) = chunk_plain_range(plain_size, i).unwrap();
        assert_eq!(poff, out.len() as u64);
        assert_eq!(plen, plain.len() as u64);
        out.extend_from_slice(&plain);
    }
    out
}

#[test]
fn every_chunk_alone_equals_the_stream() {
    let c = VaultCipher::new(&fixed_keys());
    for size in [0, 1, 100, MIB - 1, MIB, MIB + 1, 2 * MIB, 3 * MIB + 12345] {
        let plain = random_bytes(size, size as u64 + 1);
        let ct = encrypt(&plain, &c);
        assert_eq!(encrypted_size(size as u64), ct.len() as u64);
        let mut streamed = Vec::new();
        c.decrypt(ct.as_slice(), &mut streamed).unwrap();
        assert_eq!(plain, streamed);
        assert_eq!(plain, by_chunks(&ct, size as u64, &c), "size {size}");
        // The last chunk's range ends exactly at the end of the ciphertext.
        let last = chunk_count(size as u64) as u32 - 1;
        let (off, len) = chunk_cipher_range(size as u64, last).unwrap();
        assert_eq!(ct.len() as u64, off + len);
        assert_eq!(None, chunk_cipher_range(size as u64, last + 1));
    }
}

#[test]
fn ranges_and_indexes() {
    assert_eq!(
        Some((HEADER_BYTES as u64, TAG_BYTES as u64)),
        chunk_cipher_range(0, 0)
    );
    assert_eq!(
        Some(((HEADER_BYTES + MIB + TAG_BYTES) as u64, 5 + TAG_BYTES as u64)),
        chunk_cipher_range(MIB as u64 + 5, 1)
    );
    assert!(is_last_chunk(0, 0));
    assert!(!is_last_chunk(MIB as u64 + 1, 0));
    assert!(is_last_chunk(MIB as u64, 0));
    assert_eq!(0, chunk_index_of(10, 5));
    assert_eq!(2, chunk_index_of(5 * MIB as u64, 2 * MIB as u64));
    assert_eq!(1, chunk_index_of(2 * MIB as u64, 99 * MIB as u64));
    // 4 GiB: chunk 4096.
    assert_eq!(4096, chunk_index_of(5 << 30, 4 << 30));
}

#[test]
fn wrong_index_last_flag_or_file_fails() {
    let c = VaultCipher::new(&fixed_keys());
    let size = 3 * MIB as u64 + 7;
    let a = encrypt(&random_bytes(size as usize, 1), &c);
    let b = encrypt(&random_bytes(size as usize, 2), &c);
    let na = read_header(&a).unwrap();
    let nb = read_header(&b).unwrap();
    let chunk = |ct: &[u8], i: u32| {
        let (off, len) = chunk_cipher_range(size, i).unwrap();
        ct[off as usize..(off + len) as usize].to_vec()
    };
    let c1 = chunk(&a, 1);
    assert!(c.decrypt_chunk(&na, 1, false, &c1).is_ok());
    assert_integrity(c.decrypt_chunk(&na, 2, false, &c1));
    assert_integrity(c.decrypt_chunk(&na, 0, false, &c1));
    // A full-size chunk claimed to be the last one: wrong flag.
    assert_integrity(c.decrypt_chunk(&na, 1, true, &c1));
    // The same position in another file.
    assert_integrity(c.decrypt_chunk(&nb, 1, false, &c1));
    assert_integrity(c.decrypt_chunk(&na, 1, false, &chunk(&b, 1)));
    // The last chunk needs the last flag.
    let c3 = chunk(&a, 3);
    assert!(c.decrypt_chunk(&na, 3, true, &c3).is_ok());
    assert_integrity(c.decrypt_chunk(&na, 3, false, &c3));
    // Wrong key, flipped bit, truncation.
    assert_integrity(VaultCipher::new(&other_keys()).decrypt_chunk(&na, 1, false, &c1));
    let mut flipped = c1.clone();
    flipped[100] ^= 1;
    assert_integrity(c.decrypt_chunk(&na, 1, false, &flipped));
    assert_integrity(c.decrypt_chunk(&na, 1, false, &c1[..c1.len() - 1]));
    assert_integrity(c.decrypt_chunk(&na, 3, true, &c3[..10]));
}

#[test]
fn header_parse() {
    let c = VaultCipher::new(&fixed_keys());
    let ct = encrypt(b"hi", &c);
    assert_eq!(&ct[4..12], &read_header(&ct).unwrap()[..]);
    assert_eq!(&ct[4..12], &read_header(&ct[..12]).unwrap()[..]);
    assert_integrity(read_header(&ct[..11]));
    let mut bad = ct.clone();
    bad[0] = b'X';
    assert_integrity(read_header(&bad));
}
