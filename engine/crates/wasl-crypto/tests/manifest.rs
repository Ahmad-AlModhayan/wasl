//! Port of `VaultManifestTest.kt`, same order.

mod common;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use common::{assert_integrity, assert_message_contains};
use wasl_crypto::VaultManifest;
use wasl_crypto::kdf;

/// Kotlin's tests use 1000 iterations; the Rust manifest refuses fewer than 100,000 (a manifest on
/// Drive must not be able to weaken or stall the KDF), so the parsed manifests here use the minimum.
const ITER: u32 = 100_000;

fn salt() -> Vec<u8> {
    (0u8..16).collect()
}

fn check() -> [u8; 32] {
    let keys = kdf::derive("correct horse battery staple", &salt(), ITER).unwrap();
    kdf::key_check(&keys)
}

#[test]
fn serialize_and_parse_round_trip() {
    let m = VaultManifest::create("Photos vault", &salt(), ITER, &check(), 1_700_000_000_000);
    let json = m.to_json();
    assert!(json.contains("\"format\": \"drivesync-vault\""), "{json}");
    assert!(json.contains("\"version\": 1"), "{json}");
    assert!(json.contains("\"kdf\": \"PBKDF2-HMAC-SHA256\""), "{json}");
    assert!(json.contains("\"iterations\": 100000"), "{json}");
    assert!(json.contains("\"created\": \"2023-11-14T22:13:20Z\""), "{json}");
    let parsed = VaultManifest::parse(&json).unwrap();
    assert_eq!(m, parsed);
    assert_eq!(salt(), parsed.salt_bytes().unwrap());
    assert_eq!(check().to_vec(), parsed.key_check_bytes().unwrap());
    assert!(parsed.same_vault_as(&m));
}

#[test]
fn parse_accepts_compact_json_with_unknown_keys() {
    let text = r#"{"format":"drivesync-vault","version":1,"kdf":"PBKDF2-HMAC-SHA256","iterations":600000,"salt":"AAECAwQFBgcICQoLDA0ODw==","keyCheck":"F4rq5G5A9kz3h90RlZsFN5YBkI/8NO9O5L7TqXA7ioU=","created":"2026-09-30T12:00:00Z","name":"x","extra":true}"#;
    let m = VaultManifest::parse(text).unwrap();
    assert_eq!(600000, m.iterations);
    assert_eq!("x", m.name);
    assert_eq!(salt(), m.salt_bytes().unwrap());
}

/// Kotlin maps the manifest to its Room `VaultEntity` and back. The engine has no such row yet,
/// so this checks the same round trip through the fields a store would keep (name, salt,
/// iterations, keyCheck, created as epoch milliseconds).
#[test]
fn entity_round_trip() {
    let m = VaultManifest::create("Docs", &salt(), ITER, &check(), 1_700_000_000_000);
    assert_eq!(Some(1_700_000_000_000), m.created_ms());
    let again = VaultManifest::create(
        &m.name,
        &m.salt_bytes().unwrap(),
        u32::try_from(m.iterations).unwrap(),
        &m.key_check_bytes().unwrap(),
        m.created_ms().unwrap(),
    );
    assert_eq!(m, again);
    let with_ms = VaultManifest::create("Docs", &salt(), ITER, &check(), 1_759_235_696_789);
    assert_eq!("2025-09-30T12:34:56.789Z", with_ms.created);
    assert_eq!(Some(1_759_235_696_789), with_ms.created_ms());
}

#[test]
fn keys_derived_from_manifest_match_key_check() {
    let keys = kdf::derive("correct horse battery staple", &salt(), ITER).unwrap();
    let m =
        VaultManifest::parse(&VaultManifest::create("v", &salt(), ITER, &kdf::key_check(&keys), 0).to_json())
            .unwrap();
    let again = m.verify_passphrase("correct horse battery staple").unwrap();
    let again = again.expect("the right passphrase verifies");
    assert_eq!(keys.key_enc(), again.key_enc());
    assert!(
        m.verify_passphrase("correct horse battery stapler")
            .unwrap()
            .is_none()
    );
}

#[test]
fn same_vault_as_compares_salt_iterations_and_check() {
    let a = VaultManifest::create("a", &salt(), ITER, &check(), 0);
    let renamed = VaultManifest {
        name: "renamed".into(),
        created: "2000-01-01T00:00:00Z".into(),
        ..a.clone()
    };
    assert!(a.same_vault_as(&renamed));
    assert!(!a.same_vault_as(&VaultManifest {
        iterations: i64::from(ITER) + 1,
        ..a.clone()
    }));
    assert!(!a.same_vault_as(&VaultManifest {
        salt: STANDARD.encode([9u8; 16]),
        ..a.clone()
    }));
    assert!(!a.same_vault_as(&VaultManifest {
        key_check: STANDARD.encode([0u8; 32]),
        ..a.clone()
    }));
}

#[test]
fn parse_rejects_invalid_manifests() {
    let good = VaultManifest::create("v", &salt(), ITER, &check(), 0);
    let bad = |edit: &dyn Fn(&mut VaultManifest)| {
        let mut m = good.clone();
        edit(&mut m);
        assert_integrity(VaultManifest::parse(&m.to_json()))
    };
    assert_message_contains(
        &assert_integrity(VaultManifest::parse("not json at all")),
        "not a valid",
    );
    assert_message_contains(&assert_integrity(VaultManifest::parse("{}")), "not a valid");
    assert_message_contains(&bad(&|m| m.format = "other".into()), "format");
    assert_message_contains(&bad(&|m| m.version = 2), "version 2");
    assert_message_contains(&bad(&|m| m.kdf = "scrypt".into()), "KDF");
    assert_message_contains(&bad(&|m| m.iterations = 0), "iteration");
    assert_message_contains(&bad(&|m| m.salt = "***".into()), "salt");
    assert_message_contains(&bad(&|m| m.salt = "AAAA".into()), "salt is too short");
    assert_message_contains(&bad(&|m| m.key_check = "AAAA".into()), "keyCheck");
}

// ---------------------------------------------------------------- Rust-only: hostile input

#[test]
fn parse_never_panics_on_hostile_text() {
    for text in [
        "",
        "null",
        "[]",
        "\"vault\"",
        "{\"format\":1}",
        "{\"format\":\"drivesync-vault\",\"version\":1e999}",
        "{\"iterations\":-1}",
        "\u{feff}{}",
    ] {
        assert_integrity(VaultManifest::parse(text));
    }
    let mut huge = VaultManifest::create("v", &salt(), ITER, &check(), 0);
    huge.iterations = i64::MAX;
    assert_message_contains(
        &assert_integrity(VaultManifest::parse(&huge.to_json())),
        "iteration",
    );
}

/// vault.json sits on Drive: anyone who can write there must not be able to make every device
/// spend hours in PBKDF2 (or accept a uselessly weak count).
#[test]
fn iteration_count_is_bounded() {
    use wasl_crypto::manifest::{MAX_ITERATIONS, MIN_ITERATIONS};
    assert_eq!((100_000, 10_000_000), (MIN_ITERATIONS, MAX_ITERATIONS));
    let base = VaultManifest::create("v", &salt(), ITER, &check(), 0);
    let with = |n: i64| VaultManifest {
        iterations: n,
        ..base.clone()
    };
    for refused in [10_000_001, 99_999, 0, -1, i64::from(i32::MAX)] {
        let e = assert_integrity(VaultManifest::parse(&with(refused).to_json()));
        assert_message_contains(&e, "iteration");
        // Also refused when a manifest is built by hand, before any PBKDF2 work.
        assert!(with(refused).verify_passphrase("x").is_err(), "{refused}");
    }
    for accepted in [100_000, 600_000, 10_000_000] {
        let m = VaultManifest::parse(&with(accepted).to_json()).unwrap();
        assert_eq!(accepted, m.iterations);
    }
}
