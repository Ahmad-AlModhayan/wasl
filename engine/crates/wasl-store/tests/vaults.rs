//! Vaults known on a device: no secret in the store, only the vault's public description.

use wasl_store::{Store, VaultRecord};

fn vault(id: &str, name: &str) -> VaultRecord {
    VaultRecord {
        id: id.into(),
        name: name.into(),
        format_version: 2,
        kdf_params_json: r#"{"kdf":"argon2id","m":19456,"t":1,"p":1}"#.into(),
        key_check: vec![7; 32],
        manifest_json: r#"{"format":"wasl-vault"}"#.into(),
        account_id: Some("me@example.com".into()),
        drive_folder_id: Some("F".into()),
        drive_id: None,
        remote_path: "My Drive/Private".into(),
        created_at: 1_760_000_000_000,
    }
}

#[test]
fn vaults_round_trip_and_replace_by_id() {
    let s = Store::open_in_memory().unwrap();
    s.put_vault(&vault("b", "Work")).unwrap();
    s.put_vault(&vault("a", "Private")).unwrap();
    assert_eq!(
        vec!["Private", "Work"],
        s.vaults()
            .unwrap()
            .iter()
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(vault("a", "Private"), s.vault("a").unwrap().unwrap());

    let mut renamed = vault("a", "Private 2");
    renamed.format_version = 1;
    s.put_vault(&renamed).unwrap();
    assert_eq!(2, s.vaults().unwrap().len());
    assert_eq!(renamed, s.vault("a").unwrap().unwrap());

    s.delete_vault("a").unwrap();
    assert_eq!(None, s.vault("a").unwrap());
}

#[test]
fn a_row_from_before_desktop_vaults_reads_with_defaults() {
    // Rows written by schema 1 (the Android import) have only the first five columns.
    let s = Store::open_in_memory().unwrap();
    s.connection()
        .execute_batch(
            "INSERT INTO vaults (id, name, format_version, kdf_params_json, key_check)
             VALUES ('old', 'Old', 1, '{}', x'0102')",
        )
        .unwrap();
    let old = s.vault("old").unwrap().unwrap();
    assert_eq!(
        ("Old", 1, vec![1u8, 2]),
        (old.name.as_str(), old.format_version, old.key_check)
    );
    assert_eq!(
        ("", None, 0),
        (old.manifest_json.as_str(), old.account_id, old.created_at)
    );
}
