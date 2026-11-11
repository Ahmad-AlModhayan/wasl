//! Port of `VaultNamesTest.kt`, same order.

mod common;

use common::{fixed_keys, other_keys};
use wasl_crypto::CryptoError;
use wasl_crypto::names::{self, SUFFIX};

const PARENT: &str = "1AbCdEf-parent";

fn round_trip(name: &str) {
    let keys = fixed_keys();
    let enc = names::encrypt_name(&keys, PARENT, name).unwrap();
    assert!(enc.ends_with(SUFFIX), "encrypted name must end with .dsv: {enc}");
    assert!(
        enc.strip_suffix(SUFFIX)
            .unwrap()
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "encrypted name must be base64url: {enc}"
    );
    assert_eq!(Some(name.to_owned()), names::decrypt_name(&keys, &enc));
}

#[test]
fn round_trip_ascii() {
    round_trip("hello.txt");
}

#[test]
fn round_trip_spaces_and_dots() {
    round_trip("My Report (final).v2.docx");
}

#[test]
fn round_trip_unicode() {
    round_trip("Report (final) — résumé.pdf");
}

#[test]
fn round_trip_arabic() {
    round_trip("ملف مهم.txt");
}

#[test]
fn round_trip_cjk_and_emoji() {
    round_trip("写真 📷 2026.jpg");
}

#[test]
fn round_trip_single_char() {
    round_trip("a");
}

#[test]
fn round_trip_long_name() {
    round_trip(&format!("{}.bin", "x".repeat(200)));
}

#[test]
fn round_trip_folder_like_name() {
    round_trip("Camera Roll");
}

#[test]
fn deterministic_for_same_parent_and_name() {
    let keys = fixed_keys();
    let a = names::encrypt_name(&keys, PARENT, "hello.txt").unwrap();
    let b = names::encrypt_name(&keys, PARENT, "hello.txt").unwrap();
    assert_eq!(a, b);
}

#[test]
fn different_parent_gives_different_ciphertext() {
    let keys = fixed_keys();
    let a = names::encrypt_name(&keys, PARENT, "hello.txt").unwrap();
    let b = names::encrypt_name(&keys, "otherParent", "hello.txt").unwrap();
    let c = names::encrypt_name(&keys, "", "hello.txt").unwrap();
    assert_ne!(a, b);
    assert_ne!(a, c);
    assert_ne!(b, c);
    assert_eq!(Some("hello.txt".to_owned()), names::decrypt_name(&keys, &b));
    assert_eq!(Some("hello.txt".to_owned()), names::decrypt_name(&keys, &c));
}

#[test]
fn different_names_give_different_ciphertext() {
    let keys = fixed_keys();
    let e = |n: &str| names::encrypt_name(&keys, PARENT, n).unwrap();
    assert_ne!(e("hello.txt"), e("Hello.txt"));
    assert_ne!(e("hello.txt"), e("hello.tx"));
}

// Known answers generated with the Python reference implementation (tools/decrypt_vault.py primitives).
#[test]
fn known_answers() {
    let keys = fixed_keys();
    assert_eq!(
        "NTuQV7P2RFdk1_IqEnfNkO_CbgbqOzMJV2F5P0hg0NzwmWYJSw.dsv",
        names::encrypt_name(&keys, PARENT, "hello.txt").unwrap()
    );
    assert_eq!(
        "BjB1rRvvamd0oJytoL1H5otLMB4RRWX0qQmXhfXcVlvWmQTr8Vg9hz7Y00H8UgJL0GfGvQKdl3M6fro.dsv",
        names::encrypt_name(&keys, PARENT, "Report (final) — résumé.pdf").unwrap()
    );
    assert_eq!(
        "Omq2FLBxK0GM-h4b-agoelYTaqsWccb36bGO7CT7gcVcaKrJ8Q.dsv",
        names::encrypt_name(&keys, "", "hello.txt").unwrap()
    );
    assert_eq!(
        Some("hello.txt".to_owned()),
        names::decrypt_name(&keys, "NTuQV7P2RFdk1_IqEnfNkO_CbgbqOzMJV2F5P0hg0NzwmWYJSw.dsv")
    );
}

#[test]
fn encrypted_length_is_predictable() {
    // base64url of (12 nonce + 9 plaintext + 16 tag = 37 bytes) = 50 chars, plus ".dsv".
    let enc = names::encrypt_name(&fixed_keys(), PARENT, "hello.txt").unwrap();
    assert_eq!(54, enc.len());
}

#[test]
fn decrypt_returns_null_for_non_vault_names() {
    let keys = fixed_keys();
    let d = |n: &str| names::decrypt_name(&keys, n);
    assert_eq!(None, d("vault.json"));
    assert_eq!(None, d("hello.txt"));
    assert_eq!(None, d(".dsv"));
    assert_eq!(None, d("notbase64!!.dsv"));
    assert_eq!(None, d("AAAA.dsv")); // too short
    assert_eq!(
        None,
        d("NTuQV7P2RFdk1_IqEnfNkO_CbgbqOzMJV2F5P0hg0NzwmWYJSw==.dsv")
    ); // padding not allowed
    assert_eq!(None, d("NTuQV7P2RFdk1_IqEnfNkO_CbgbqOzMJV2F5P0hg0NzwmWYJSw.DSV")); // suffix is case sensitive
}

#[test]
fn decrypt_returns_null_for_tampered_name() {
    let keys = fixed_keys();
    let enc = names::encrypt_name(&keys, PARENT, "hello.txt").unwrap();
    let body = enc.strip_suffix(SUFFIX).unwrap();
    let flip = if body.as_bytes()[20] == b'A' { "B" } else { "A" };
    let flipped = format!("{}{}{}{}", &body[..20], flip, &body[21..], SUFFIX);
    assert_eq!(None, names::decrypt_name(&keys, &flipped));
}

#[test]
fn decrypt_returns_null_with_wrong_keys() {
    let enc = names::encrypt_name(&fixed_keys(), PARENT, "hello.txt").unwrap();
    assert_eq!(None, names::decrypt_name(&other_keys(), &enc));
}

#[test]
fn encrypt_rejects_slash() {
    let r = names::encrypt_name(&fixed_keys(), PARENT, "a/b");
    assert!(matches!(r, Err(CryptoError::InvalidInput(_))), "{r:?}");
}

#[test]
fn encrypt_rejects_empty() {
    let r = names::encrypt_name(&fixed_keys(), PARENT, "");
    assert!(matches!(r, Err(CryptoError::InvalidInput(_))), "{r:?}");
}

#[test]
fn is_vault_name() {
    assert!(names::is_vault_name("abc.dsv"));
    assert!(!names::is_vault_name(".dsv"));
    assert!(!names::is_vault_name("abc.txt"));
}

// ---------------------------------------------------------------- Rust-only: hostile input

/// Names arrive from Drive and anyone with access can write them: nothing may panic.
#[test]
fn decrypt_never_panics_on_hostile_names() {
    let keys = fixed_keys();
    let mut inputs: Vec<String> = vec![
        String::new(),
        "x.dsv".into(),
        "A.dsv".into(),
        "AA.dsv".into(),
        "AAAAA.dsv".into(),
        "ééé.dsv".into(),
        "📷📷📷📷.dsv".into(),
        "\u{0}\u{0}.dsv".into(),
        format!("{}.dsv", "A".repeat(10_000)),
        format!("{}.dsv", "_".repeat(37)),
        "====.dsv".into(),
        "+/+/.dsv".into(),
    ];
    for len in 0..64 {
        inputs.push(format!("{}.dsv", "-".repeat(len)));
    }
    for name in inputs {
        assert_eq!(None, names::decrypt_name(&keys, &name), "{name}");
    }
}
