//! `vault.json`, the one plaintext file in a vault folder on Drive (anyone with access can write
//! it): version detection, the v1 and v2 readers, and the fields read from a parsed manifest.
//!
//! Invariants: no panic; a parsed v2 manifest never asks for a KDF cost beyond the reader bounds.

#![no_main]

use libfuzzer_sys::fuzz_target;
use wasl_crypto::VaultManifest;
use wasl_crypto::recover::Vault;
use wasl_crypto::v2::VaultManifestV2;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(v) = Vault::parse(text) {
        let _ = (v.version(), v.name(), v.created());
    }
    if let Ok(m) = VaultManifest::parse(text) {
        let _ = (m.created_ms(), m.salt_bytes(), m.key_check_bytes());
    }
    if let Ok(m) = VaultManifestV2::parse(text) {
        let params = m
            .params()
            .expect("a parsed v2 manifest has usable KDF parameters");
        params.check().expect("within the reader bounds");
        let _ = (m.created_ms(), m.salt_bytes(), m.key_check_bytes());
    }
});
