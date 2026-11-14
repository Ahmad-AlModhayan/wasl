//! What can go wrong in a run, and how each failure is classified (fatal or not, which fix).

use wasl_core::ErrorKind;
use wasl_drive::DriveError;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Drive(#[from] DriveError),
    #[error("local database: {0}")]
    Store(#[from] wasl_store::StoreError),
    #[error("{0}")]
    Io(String),
    /// The local folder cannot be read or written at all (permission withdrawn, card removed).
    #[error("{0}")]
    Permission(String),
    #[error("{0}")]
    NotFound(String),
    /// A device file moved or deleted between the scan and the read: skipped, never a failure;
    /// the next run sees what is there (#154).
    #[error("{0}")]
    Vanished(String),
    #[error("{0}")]
    Integrity(String),
    /// A vault pair whose vault is locked on this device.
    #[error("{0}")]
    Locked(String),
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Other(String),
}

pub type Result<T, E = EngineError> = std::result::Result<T, E>;

impl From<std::io::Error> for EngineError {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::PermissionDenied => EngineError::Permission(e.to_string()),
            std::io::ErrorKind::Interrupted => EngineError::Cancelled,
            _ => EngineError::Io(e.to_string()),
        }
    }
}

impl EngineError {
    pub fn kind(&self) -> ErrorKind {
        match self {
            EngineError::Drive(d) => match d {
                DriveError::Auth { .. } => ErrorKind::Auth,
                DriveError::Network(_) => ErrorKind::Network,
                DriveError::RateLimited { .. } => ErrorKind::RateLimit,
                DriveError::StorageQuota(_) => ErrorKind::Quota,
                DriveError::NotFound(_) => ErrorKind::NotFound,
                DriveError::Http { code, .. } if *code >= 500 => ErrorKind::Server,
                DriveError::SignInUnavailable => ErrorKind::Server,
                DriveError::Http { .. } => ErrorKind::Other,
                DriveError::Io(_) => ErrorKind::Io,
                DriveError::Integrity(_) => ErrorKind::Integrity,
                DriveError::Unknown(_) => ErrorKind::Other,
            },
            EngineError::Store(_) | EngineError::Io(_) | EngineError::Vanished(_) => ErrorKind::Io,
            EngineError::Permission(_) => ErrorKind::Permission,
            EngineError::NotFound(_) => ErrorKind::NotFound,
            EngineError::Integrity(_) => ErrorKind::Integrity,
            EngineError::Locked(_) => ErrorKind::Locked,
            EngineError::Cancelled | EngineError::Other(_) => ErrorKind::Other,
        }
    }

    /// What to show the user for one file: what happened and what to do (or that Wasl retries),
    /// without raw API text. The raw error goes to the event's detail.
    pub fn user_message(&self) -> String {
        match self {
            EngineError::Drive(d) => d.user_hint(),
            EngineError::Store(e) => format!(
                "Wasl couldn't write its records on this device ({e}). Free some space; the next sync tries again."
            ),
            EngineError::Io(m) => format!("{m}. The next sync tries again."),
            other => other.to_string(),
        }
    }

    /// Drive's malware scan refuses this file to everyone but its owner: retrying cannot help.
    pub fn is_flagged_by_drive(&self) -> bool {
        matches!(self, EngineError::Drive(d) if d.is_flagged_abusive())
    }

    /// One file on the device couldn't be read: that file fails and the run goes on. Android can
    /// refuse one file while the folder is fine (#127); a folder Wasl really lost access to fails
    /// its listing first, which stays fatal.
    pub fn file_read(e: std::io::Error) -> EngineError {
        match e.kind() {
            std::io::ErrorKind::Interrupted => EngineError::Cancelled,
            std::io::ErrorKind::NotFound => EngineError::Vanished(e.to_string()),
            _ => EngineError::Io(e.to_string()),
        }
    }

    /// Failures no other file in the run can get past: stop the run (1.1.4: Auth, Permission, Quota).
    pub fn is_fatal(&self) -> bool {
        matches!(
            self.kind(),
            ErrorKind::Auth | ErrorKind::Permission | ErrorKind::Quota
        )
    }
}
