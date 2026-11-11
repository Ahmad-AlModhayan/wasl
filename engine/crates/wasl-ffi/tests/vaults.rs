//! Vault v2 through the phone's binding: create, unlock, and telling v1 from v2.

use wasl_mobile::{vault_v2_create, vault_v2_unlock, vault_version};

#[test]
fn a_v2_vault_is_created_and_unlocked_only_with_its_passphrase() {
    let v = vault_v2_create("Photos".into(), "correct horse".into(), 1_800_000_000_000).unwrap();
    assert_eq!(32, v.keys.len());
    assert_eq!(Some(2), vault_version(v.manifest_json.clone()));
    let again = vault_v2_unlock(v.manifest_json.clone(), "correct horse".into()).unwrap();
    assert_eq!(Some(v.keys.clone()), again, "the same key material back");
    assert_eq!(
        None,
        vault_v2_unlock(v.manifest_json.clone(), "wrong".into()).unwrap()
    );
    assert!(vault_v2_unlock("{}".into(), "x".into()).is_err());
    // Each vault gets its own salt.
    let other = vault_v2_create("Photos".into(), "correct horse".into(), 1).unwrap();
    assert_ne!(v.keys, other.keys);
}

#[test]
fn versions_are_told_apart() {
    let v1 = r#"{"format":"drivesync-vault","version":1,"kdf":"PBKDF2-HMAC-SHA256"}"#;
    assert_eq!(Some(1), vault_version(v1.into()));
    assert_eq!(None, vault_version(r#"{"format":"other","version":2}"#.into()));
    assert_eq!(None, vault_version("not json".into()));
}
