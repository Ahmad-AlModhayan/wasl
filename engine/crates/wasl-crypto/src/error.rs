//! Vault failures. `Integrity` is what the app shows as `AppError.Integrity` (and the Python tool
//! as `VaultError`): data that does not verify or a manifest this version cannot use.

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    /// Ciphertext, a name or a manifest did not verify (modified, truncated, wrong key, unknown
    /// format). Never retried: the same bytes fail the same way.
    #[error("{0}")]
    Integrity(String),
    /// The caller passed something the format cannot represent (Kotlin: `require` failures).
    #[error("{0}")]
    InvalidInput(String),
    /// Reading, writing or the OS random number generator failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = CryptoError> = std::result::Result<T, E>;
