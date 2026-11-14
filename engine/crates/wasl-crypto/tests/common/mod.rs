//! Port of the Kotlin `TestSupport` object shared by the vault tests.
#![allow(dead_code)]

use wasl_crypto::{CryptoError, VaultKeys};

/// Fixed keys shared with the Python reference vectors (keyEnc = 0x11 * 32, keyName = 0x22 * 32).
pub fn fixed_keys() -> VaultKeys {
    VaultKeys::new([0x11; 32], [0x22; 32])
}

pub fn other_keys() -> VaultKeys {
    VaultKeys::new([0x33; 32], [0x44; 32])
}

/// Deterministic pseudo-random bytes (SplitMix64). The Kotlin tests use `java.util.Random`; the
/// exact bytes never matter, only that they are reproducible and not all alike.
pub fn random_bytes(n: usize, seed: u64) -> Vec<u8> {
    let mut state = seed;
    let mut out = Vec::with_capacity(n + 8);
    while out.len() < n {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        out.extend_from_slice(&z.to_le_bytes());
    }
    out.truncate(n);
    out
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
        .collect()
}

/// The error of an operation that must fail with `CryptoError::Integrity`; returns its message.
#[track_caller]
pub fn assert_integrity<T: std::fmt::Debug>(r: Result<T, CryptoError>) -> String {
    match r {
        Err(CryptoError::Integrity(m)) => m,
        Err(e) => panic!("expected CryptoError::Integrity but got {e:?}"),
        Ok(v) => panic!("expected CryptoError::Integrity but got Ok({v:?})"),
    }
}

#[track_caller]
pub fn assert_message_contains(message: &str, fragment: &str) {
    assert!(
        message.contains(fragment),
        "message \"{message}\" should contain \"{fragment}\""
    );
}

pub mod v2 {
    use super::random_bytes;
    use std::path::Path;

    /// A plaintext folder exercising the v2 layout: nested folders, Arabic, CJK and emoji names,
    /// an empty file, an empty folder, a multi-chunk file and a file of repeated chunks.
    pub fn sample_tree(root: &Path) {
        let mib = 1 << 20;
        let files: Vec<(&str, Vec<u8>)> = vec![
            ("hello.txt", b"hello world".to_vec()),
            ("empty.txt", Vec::new()),
            ("Docs/notes.txt", b"notes\n".repeat(1000)),
            ("Docs/ملف مهم/big.bin", random_bytes(10 * mib + 3, 77)),
            ("Docs/ملف مهم/deeper/写真 📷.jpg", random_bytes(4097, 78)),
            ("zeros.bin", vec![0u8; 9 * mib]),
        ];
        for (rel, content) in files {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, content).unwrap();
        }
        std::fs::create_dir_all(root.join("Empty folder")).unwrap();
    }
}
