//! Run outcomes and error categories shared by the engine, the store and the apps.

use serde::{Deserialize, Serialize};

/// How a run ended. Stored as 1.1.4's names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum RunStatus {
    Running,
    Success,
    /// Finished with some items failed.
    Partial,
    Failed,
    /// A safety guard stopped it to ask.
    Blocked,
    /// Did not start (conditions, already running, waiting for the preview).
    Skipped,
    Cancelled,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Running => "RUNNING",
            RunStatus::Success => "SUCCESS",
            RunStatus::Partial => "PARTIAL",
            RunStatus::Failed => "FAILED",
            RunStatus::Blocked => "BLOCKED",
            RunStatus::Skipped => "SKIPPED",
            RunStatus::Cancelled => "CANCELLED",
        }
    }

    pub fn parse(s: &str) -> Option<RunStatus> {
        Some(match s {
            "RUNNING" => RunStatus::Running,
            "SUCCESS" => RunStatus::Success,
            "PARTIAL" => RunStatus::Partial,
            "FAILED" => RunStatus::Failed,
            "BLOCKED" => RunStatus::Blocked,
            "SKIPPED" => RunStatus::Skipped,
            "CANCELLED" => RunStatus::Cancelled,
            _ => return None,
        })
    }
}

/// Coarse error categories persisted on a pair, so the apps can name the one fix (1.1.4 `ErrorKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum ErrorKind {
    Auth,
    Permission,
    Quota,
    NotFound,
    Locked,
    Network,
    RateLimit,
    Server,
    Io,
    Integrity,
    Other,
}

impl ErrorKind {
    /// Retrying on its own: never badged until it keeps failing.
    pub fn transient(self) -> bool {
        matches!(
            self,
            ErrorKind::Network | ErrorKind::RateLimit | ErrorKind::Server
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Auth => "AUTH",
            ErrorKind::Permission => "PERMISSION",
            ErrorKind::Quota => "QUOTA",
            ErrorKind::NotFound => "NOT_FOUND",
            ErrorKind::Locked => "LOCKED",
            ErrorKind::Network => "NETWORK",
            ErrorKind::RateLimit => "RATE_LIMIT",
            ErrorKind::Server => "SERVER",
            ErrorKind::Io => "IO",
            ErrorKind::Integrity => "INTEGRITY",
            ErrorKind::Other => "OTHER",
        }
    }

    pub fn parse(s: &str) -> Option<ErrorKind> {
        [
            ErrorKind::Auth,
            ErrorKind::Permission,
            ErrorKind::Quota,
            ErrorKind::NotFound,
            ErrorKind::Locked,
            ErrorKind::Network,
            ErrorKind::RateLimit,
            ErrorKind::Server,
            ErrorKind::Io,
            ErrorKind::Integrity,
            ErrorKind::Other,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
    }
}
