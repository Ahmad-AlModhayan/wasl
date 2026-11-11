//! Vault v2 for the phone (docs/ENCRYPTION.md §11): creating one and unlocking it run Argon2id
//! here, in Rust, so the app needs no implementation of its own. The app keeps the returned key
//! material wrapped by the Android Keystore, as it does v1's, and hands it back in
//! [`crate::VaultAccess`]: 64 bytes for v1 (keyEnc ‖ keyName), 32 for v2 (the Argon2id output).
//!
//! New vaults are format 2.1 (`vault.json` `"version": 3`: keyed chunking, padded chunks, state
//! records); 2.0 vaults stay as they are. Both keep the same 32 bytes and go through the same
//! remote, so for the app both are "v2" ([`vault_version`]); [`vault_format`] tells them apart for
//! people.
//!
//! Key material (security review S4-5): the passphrase and the Argon2id output are wiped here when
//! dropped. The one copy that leaves (the returned bytes) has to: UniFFI copies it into a buffer
//! the app owns, which the app should wrap with the Keystore at once and then overwrite.

use wasl_crypto::v2::{KdfParams, VaultKeysV2, keys, vault_json::VaultManifestV2};
use zeroize::Zeroizing;

use crate::WaslError;

/// A new vault: its `vault.json` and the key material to keep.
#[derive(uniffi::Record)]
pub struct NewVault {
    pub manifest_json: String,
    pub keys: Vec<u8>,
}

impl std::fmt::Debug for NewVault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NewVault { .. }")
    }
}

fn crypto(e: wasl_crypto::CryptoError) -> WaslError {
    WaslError::Other {
        detail: e.to_string(),
    }
}

/// Creates a vault v2 (Argon2id with the default cost). Slow on purpose: call it off the main
/// thread.
#[uniffi::export]
pub fn vault_v2_create(name: String, passphrase: String, created_ms: i64) -> Result<NewVault, WaslError> {
    let passphrase = Zeroizing::new(passphrase);
    let salt = keys::new_salt().map_err(crypto)?;
    let params = KdfParams::DEFAULT;
    let ikm = keys::argon2id(&passphrase, &salt, params).map_err(crypto)?;
    let k = VaultKeysV2::from_ikm(&ikm);
    let manifest = VaultManifestV2::create(&name, &salt, params, &k.key_check(), created_ms);
    drop(k);
    Ok(NewVault {
        manifest_json: manifest.to_json(),
        keys: ikm.to_vec(),
    })
}

/// Unlocks a vault v2: the key material to keep, or `None` for a wrong passphrase.
#[uniffi::export]
pub fn vault_v2_unlock(manifest_json: String, passphrase: String) -> Result<Option<Vec<u8>>, WaslError> {
    let passphrase = Zeroizing::new(passphrase);
    let manifest = VaultManifestV2::parse(&manifest_json).map_err(crypto)?;
    let ikm = keys::argon2id(
        &passphrase,
        &manifest.salt_bytes().map_err(crypto)?,
        manifest.params().map_err(crypto)?,
    )
    .map_err(crypto)?;
    let k = VaultKeysV2::from_ikm(&ikm);
    Ok(k.key_check_matches(&manifest.key_check_bytes().map_err(crypto)?)
        .then(|| ikm.to_vec()))
}

/// The format family of a `vault.json`: 1, or 2 for formats 2.0 and 2.1 (`"version"` 2 or 3,
/// the same key material and remote); `None` if it is not a Wasl vault this build reads.
#[uniffi::export]
pub fn vault_version(manifest_json: String) -> Option<u32> {
    let v: serde_json::Value = serde_json::from_str(&manifest_json).ok()?;
    match (v.get("format")?.as_str()?, v.get("version")?.as_u64()?) {
        ("drivesync-vault", 1) => Some(1),
        ("wasl-vault", 2 | 3) => Some(2),
        _ => None,
    }
}

/// The format for people: "1", "2.0" or "2.1"; `None` if it is not a Wasl vault this build reads.
#[uniffi::export]
pub fn vault_format(manifest_json: String) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(&manifest_json).ok()?;
    match (v.get("format")?.as_str()?, v.get("version")?.as_i64()?) {
        ("drivesync-vault", 1) => Some("1".into()),
        ("wasl-vault", n) => wasl_crypto::v2::Format::from_json_version(n).map(|f| f.label().into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_vaults_are_2_1_and_both_revisions_are_v2_for_the_app() {
        let v = vault_v2_create("Photos".into(), "correct horse".into(), 1).unwrap();
        assert_eq!(Some(2), vault_version(v.manifest_json.clone()));
        assert_eq!(Some("2.1".to_owned()), vault_format(v.manifest_json.clone()));
        let v20 = v.manifest_json.replace("\"version\": 3", "\"version\": 2");
        assert_ne!(v20, v.manifest_json);
        assert_eq!(Some(2), vault_version(v20.clone()));
        assert_eq!(Some("2.0".to_owned()), vault_format(v20));
        let v1 = r#"{"format":"drivesync-vault","version":1}"#;
        assert_eq!(Some("1".to_owned()), vault_format(v1.into()));
        let future = r#"{"format":"wasl-vault","version":4}"#;
        assert_eq!(
            (None, None),
            (vault_version(future.into()), vault_format(future.into()))
        );
    }
}
