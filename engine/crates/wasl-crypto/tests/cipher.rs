//! Port of `VaultCipherTest.kt`, same order.

mod common;

use common::{assert_integrity, assert_message_contains, fixed_keys, hex, other_keys, random_bytes};
use sha2::{Digest, Sha256};
use wasl_crypto::VaultCipher;
use wasl_crypto::content::{
    self, CHUNK_BYTES, FILE_NONCE_BYTES, MAGIC, chunk_aad, chunk_count, chunk_iv, encrypted_size,
    read_file_nonce,
};

const MIB: usize = CHUNK_BYTES;

fn cipher() -> VaultCipher {
    VaultCipher::new(&fixed_keys())
}

fn encrypt_with(plain: &[u8], c: &VaultCipher) -> Vec<u8> {
    let mut out = Vec::new();
    c.encrypt(plain, &mut out).unwrap();
    out
}

fn encrypt(plain: &[u8]) -> Vec<u8> {
    encrypt_with(plain, &cipher())
}

fn decrypt_with(ct: &[u8], c: &VaultCipher) -> wasl_crypto::Result<Vec<u8>> {
    let mut out = Vec::new();
    c.decrypt(ct, &mut out).map(|_| out)
}

fn decrypt(ct: &[u8]) -> wasl_crypto::Result<Vec<u8>> {
    decrypt_with(ct, &cipher())
}

fn round_trip(n: usize) {
    let plain = random_bytes(n, n as u64);
    let ct = encrypt(&plain);
    assert_eq!(
        encrypted_size(n as u64),
        ct.len() as u64,
        "ciphertext size for {n} bytes"
    );
    assert_eq!(plain, decrypt(&ct).unwrap());
}

#[test]
fn round_trip_empty() {
    round_trip(0);
}

#[test]
fn round_trip_one_byte() {
    round_trip(1);
}

#[test]
fn round_trip_exactly_one_mib() {
    round_trip(MIB);
}

#[test]
fn round_trip_one_mib_plus_one() {
    round_trip(MIB + 1);
}

#[test]
fn round_trip_three_and_a_half_mib() {
    round_trip(3 * MIB + MIB / 2);
}

#[test]
fn round_trip_exactly_two_mib() {
    round_trip(2 * MIB);
}

#[test]
fn chunk_count_and_size() {
    let mib = MIB as u64;
    assert_eq!(1, chunk_count(0));
    assert_eq!(1, chunk_count(1));
    assert_eq!(1, chunk_count(mib));
    assert_eq!(2, chunk_count(mib + 1));
    assert_eq!(4, chunk_count(3 * mib + mib / 2));
    assert_eq!(12 + 16, encrypted_size(0));
    assert_eq!(12 + mib + 16, encrypted_size(mib));
    assert_eq!(12 + mib + 1 + 32, encrypted_size(mib + 1));
}

#[test]
fn file_round_trip_and_nonce() {
    let plain = random_bytes(2 * MIB + 12345, 7);
    let dir = tempfile::tempdir().unwrap();
    let (src, enc, out) = (
        dir.path().join("vault-src.bin"),
        dir.path().join("vault-enc.dsv"),
        dir.path().join("vault-out.bin"),
    );
    std::fs::write(&src, &plain).unwrap();
    let c = cipher();
    let nonce = c.encrypt_file(&src, &enc).unwrap();
    assert_eq!(FILE_NONCE_BYTES, nonce.len());
    assert_eq!(Some(nonce), read_file_nonce(&enc));
    assert_eq!(
        encrypted_size(plain.len() as u64),
        std::fs::metadata(&enc).unwrap().len()
    );
    c.decrypt_file(&enc, &out).unwrap();
    assert_eq!(plain, std::fs::read(&out).unwrap());
    assert_eq!(None, read_file_nonce(&src));
}

// Known answers generated with Python `cryptography` (AESGCM) for fileNonce 01..08.
fn fixed_nonce_cipher() -> VaultCipher {
    VaultCipher::with_nonce_source(&fixed_keys(), || Ok([1, 2, 3, 4, 5, 6, 7, 8]))
}

#[test]
fn known_answer_hello_world() {
    let ct = encrypt_with(b"hello world", &fixed_nonce_cipher());
    assert_eq!(
        "4453563101020304050607089a8aaa4e57b08e47d70191df91e92ea93ac8b2541b937ff1e1988d",
        hex(&ct)
    );
    assert_eq!(b"hello world".to_vec(), decrypt(&ct).unwrap());
}

#[test]
fn known_answer_empty() {
    let ct = encrypt_with(&[], &fixed_nonce_cipher());
    assert_eq!(
        "445356310102030405060708c30f0aec6d312875fabd66c8a89a7797",
        hex(&ct)
    );
    assert_eq!(0, decrypt(&ct).unwrap().len());
}

#[test]
fn known_answer_one_mib_plus_one_of_zeros() {
    let ct = encrypt_with(&vec![0u8; MIB + 1], &fixed_nonce_cipher());
    assert_eq!(1048621, ct.len());
    assert_eq!(
        "7d793d13c57d20f67559b0b98a5dd0e70ca2d49d94d023c18aa4e8621682693a",
        hex(&Sha256::digest(&ct))
    );
    assert_eq!("3b46260f6698a0366d97aeea64f486ebda", hex(&ct[ct.len() - 17..]));
}

#[test]
fn same_content_encrypts_differently_each_time() {
    let plain = b"same bytes";
    assert_ne!(encrypt(plain), encrypt(plain));
}

#[test]
fn flipped_byte_in_every_region_is_detected() {
    let plain = random_bytes(MIB + 100, 3);
    let ct = encrypt(&plain);
    let positions = [
        5,
        11,
        12,
        12 + 1000,
        12 + MIB,
        12 + MIB + 15,
        12 + MIB + 16,
        ct.len() - 1,
    ];
    for pos in positions {
        let mut bad = ct.clone();
        bad[pos] ^= 0x01;
        let e = assert_integrity(decrypt(&bad));
        assert_message_contains(&e, "authentication failed");
    }
}

#[test]
fn bad_magic_is_rejected() {
    let mut bad = encrypt(b"x");
    bad[3] = b'2'; // "DSV2"
    let e = assert_integrity(decrypt(&bad));
    assert_message_contains(&e, "unknown format");
}

#[test]
fn dropping_the_last_chunk_is_detected() {
    let plain = random_bytes(2 * MIB + MIB / 2, 5); // 3 chunks
    let ct = encrypt(&plain);
    let cut = &ct[..12 + 2 * (MIB + 16)]; // header + two full chunks
    let e = assert_integrity(decrypt(cut));
    assert_message_contains(&e, "truncated");
}

#[test]
fn dropping_the_last_chunk_of_aligned_file_is_detected() {
    let plain = random_bytes(2 * MIB, 6); // 2 full chunks, last has lastFlag = 1
    let ct = encrypt(&plain);
    assert_integrity(decrypt(&ct[..12 + (MIB + 16)]));
}

#[test]
fn truncation_inside_a_chunk_is_detected() {
    let ct = encrypt(&random_bytes(MIB + 5000, 8));
    assert_integrity(decrypt(&ct[..ct.len() - 100]));
    assert_integrity(decrypt(&ct[..ct.len() - 1]));
}

#[test]
fn header_only_and_too_short_are_rejected() {
    let ct = encrypt(b"abc");
    assert_message_contains(&assert_integrity(decrypt(&ct[..12])), "no data chunk");
    assert_message_contains(&assert_integrity(decrypt(&ct[..20])), "no data chunk");
    assert_message_contains(&assert_integrity(decrypt(&ct[..5])), "shorter than the header");
    assert_integrity(decrypt(&[]));
}

#[test]
fn appended_bytes_are_rejected() {
    let ct = encrypt(&random_bytes(1000, 9));
    assert_integrity(decrypt(&[ct.as_slice(), &[1, 2, 3]].concat()));
    assert_integrity(decrypt(&[ct.as_slice(), &[0; 40]].concat()));
    let full = encrypt(&random_bytes(MIB, 10));
    assert_integrity(decrypt(&[full.as_slice(), &[0]].concat()));
}

#[test]
fn wrong_key_is_rejected() {
    let ct = encrypt(b"secret");
    assert_integrity(decrypt_with(&ct, &VaultCipher::new(&other_keys())));
}

#[test]
fn chunk_reordering_is_rejected() {
    let ct = encrypt(&random_bytes(3 * MIB, 11)); // 3 full chunks
    let c = MIB + 16;
    let mut swapped = ct.clone();
    swapped[12 + c..12 + 2 * c].copy_from_slice(&ct[12..12 + c]);
    swapped[12..12 + c].copy_from_slice(&ct[12 + c..12 + 2 * c]);
    assert_integrity(decrypt(&swapped));
}

#[test]
fn decrypt_file_deletes_partial_output_on_failure() {
    let dir = tempfile::tempdir().unwrap();
    let (enc, out) = (dir.path().join("vault-bad.dsv"), dir.path().join("vault-out.bin"));
    std::fs::write(&out, b"stale").unwrap();
    let ct = encrypt(&random_bytes(MIB + 10, 12));
    std::fs::write(&enc, &ct[..ct.len() - 3]).unwrap();
    assert_integrity(cipher().decrypt_file(&enc, &out));
    assert!(!out.exists(), "partial plaintext must not be left behind");
}

#[test]
fn iv_and_aad_layout() {
    let nonce = [1, 2, 3, 4, 5, 6, 7, 8];
    assert_eq!("01020304050607080000002a", hex(&chunk_iv(&nonce, 42)));
    assert_eq!("010203040506070800000000", hex(&chunk_iv(&nonce, 0)));
    assert_eq!(
        "647269766573796e632d6368756e6b2d7631".to_owned() + "00000001" + "01",
        hex(&chunk_aad(1, true))
    );
    assert_eq!(
        "647269766573796e632d6368756e6b2d7631".to_owned() + "00000000" + "00",
        hex(&chunk_aad(0, false))
    );
    assert_eq!("44535631", hex(&MAGIC));
    assert_eq!(8, content::random_file_nonce().unwrap().len());
}

// ---------------------------------------------------------------- Rust-only

/// Drive reports ciphertext sizes; the vault remote shows plaintext sizes (spec section 5).
#[test]
fn plaintext_size_inverts_encrypted_size() {
    let mib = MIB as u64;
    for n in [
        0,
        1,
        15,
        16,
        17,
        mib - 1,
        mib,
        mib + 1,
        2 * mib,
        3 * mib + 7,
        5_000_000_000,
    ] {
        assert_eq!(Some(n), content::plaintext_size(encrypted_size(n)), "{n}");
    }
    // Sizes no encryption can produce: header only, a final chunk shorter than its tag.
    for bad in [0, 11, 12, 13, 27, 12 + mib + 16 + 15] {
        assert_eq!(None, content::plaintext_size(bad), "{bad}");
    }
}

/// A reader that hands out one byte at a time and is interrupted now and then still decrypts.
#[test]
fn short_reads_and_interrupts_are_handled() {
    struct Dribble<'a> {
        data: &'a [u8],
        calls: usize,
    }
    impl std::io::Read for Dribble<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.calls += 1;
            if self.calls.is_multiple_of(7) {
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            let n = self.data.len().min(buf.len()).min(3);
            buf[..n].copy_from_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Ok(n)
        }
    }
    let plain = random_bytes(3000, 13);
    let mut ct = Vec::new();
    cipher()
        .encrypt(
            Dribble {
                data: &plain,
                calls: 0,
            },
            &mut ct,
        )
        .unwrap();
    let mut out = Vec::new();
    cipher()
        .decrypt(Dribble { data: &ct, calls: 0 }, &mut out)
        .unwrap();
    assert_eq!(plain, out);
}
