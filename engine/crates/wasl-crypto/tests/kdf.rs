//! Port of `VaultKdfTest.kt`, same order.

mod common;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use common::hex;
use wasl_crypto::kdf::{self, KEY_CHECK_BYTES, SALT_BYTES};
use wasl_crypto::{CryptoError, VaultKeys};
use zeroize::Zeroize;

fn salt_0_to_15() -> Vec<u8> {
    (0u8..16).collect()
}

// RFC 7914 section 11: PBKDF2-HMAC-SHA-256 test vectors.
#[test]
fn pbkdf2_matches_rfc7914_vector1() {
    let dk = kdf::pbkdf2_hmac_sha256(b"passwd", b"salt", 1, 64).unwrap();
    assert_eq!(
        "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc49ca9cccf179b645991664b39d77ef317c71b845b1e30bd509112041d3a19783",
        hex(&dk)
    );
}

#[test]
fn pbkdf2_matches_rfc7914_vector2() {
    let dk = kdf::pbkdf2_hmac_sha256(b"Password", b"NaCl", 80000, 64).unwrap();
    assert_eq!(
        "4ddcd8f60b98be21830cee5ef22701f9641a4418d04c0414aeff08876b34ab56a1d425a1225833549adb841b51c9b3176a272bdebba1d078478f62b397f33c8d",
        hex(&dk)
    );
}

#[test]
fn pbkdf2_short_output_is_prefix_of_long_output() {
    let long = kdf::pbkdf2_hmac_sha256(b"passwd", b"salt", 3, 64).unwrap();
    let short = kdf::pbkdf2_hmac_sha256(b"passwd", b"salt", 3, 20).unwrap();
    assert_eq!(&long[..20], &short[..]);
}

// Known answers generated with Python: hashlib.pbkdf2_hmac + hmac (see tools/decrypt_vault.py).
#[test]
fn derive_known_answer() {
    let keys = kdf::derive("correct horse battery staple", &salt_0_to_15(), 1000).unwrap();
    assert_eq!(
        "a69b179e3add3c1e0aaf227a0eb3aa2aa8645ab86fecf6ca00c17512697c719e",
        hex(keys.key_enc())
    );
    assert_eq!(
        "d35f6dc6fba420111c1d0abb5dabbfc8539681e885523a94f3ea280704b6791b",
        hex(keys.key_name())
    );
    assert_eq!(
        "F4rq5G5A9kz3h90RlZsFN5YBkI/8NO9O5L7TqXA7ioU=",
        STANDARD.encode(kdf::key_check(&keys))
    );
}

#[test]
fn derive_uses_utf8_for_non_ascii_passphrases() {
    let keys = kdf::derive("pässwörd ✓", &salt_0_to_15(), 1000).unwrap();
    assert_eq!(
        "f82c708c2298e13beeff065189bf03ae7cf0beaa66d302e3cd6a7ff049e8bdce",
        hex(keys.key_enc())
    );
}

#[test]
fn wrong_passphrase_fails_key_check() {
    let salt = kdf::new_salt().unwrap();
    let right = kdf::derive("open sesame please", &salt, 500).unwrap();
    let check = kdf::key_check(&right);
    let d = |p: &str, s: &[u8], i: u32| kdf::derive(p, s, i).unwrap();
    assert!(kdf::key_check_matches(
        &d("open sesame please", &salt, 500),
        &check
    ));
    assert!(!kdf::key_check_matches(
        &d("open sesame pleas", &salt, 500),
        &check
    ));
    assert!(!kdf::key_check_matches(
        &d("Open sesame please", &salt, 500),
        &check
    ));
    assert!(
        !kdf::key_check_matches(&d("open sesame please", &kdf::new_salt().unwrap(), 500), &check),
        "different salt must not match"
    );
    assert!(
        !kdf::key_check_matches(&d("open sesame please", &salt, 501), &check),
        "different iterations must not match"
    );
}

#[test]
fn key_check_depends_only_on_key_name() {
    let a = VaultKeys::new([1; 32], [2; 32]);
    let b = VaultKeys::new([9; 32], [2; 32]);
    assert_eq!(kdf::key_check(&a), kdf::key_check(&b));
    assert_eq!(KEY_CHECK_BYTES, kdf::key_check(&a).len());
}

#[test]
fn new_salt_is_sixteen_random_bytes() {
    let a = kdf::new_salt().unwrap();
    let b = kdf::new_salt().unwrap();
    assert_eq!(SALT_BYTES, a.len());
    assert_ne!(a, b);
}

#[test]
fn derive_rejects_empty_passphrase() {
    let r = kdf::derive("", &kdf::new_salt().unwrap(), 10);
    assert!(matches!(r, Err(CryptoError::InvalidInput(_))), "{r:?}");
}

#[test]
fn zero_wipes_keys() {
    let mut keys = kdf::derive("wipe me", &kdf::new_salt().unwrap(), 10).unwrap();
    keys.zeroize();
    assert!(keys.key_enc().iter().all(|b| *b == 0) && keys.key_name().iter().all(|b| *b == 0));
}
