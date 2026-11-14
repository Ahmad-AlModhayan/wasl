//! Error taxonomy of the Drive transport (port of `core/AppError.kt` + `drive/DriveErrors.kt`).
//!
//! Only the variants the Drive client produces are ported. `Permission` (Android storage
//! permission), `Locked` (vault) and `Cancelled` (dropping a future is cancellation in Rust) live
//! elsewhere or have no equivalent here.

use serde::Deserialize;

/// Everything the Drive client can fail with.
#[derive(Debug, thiserror::Error)]
pub enum DriveError {
    /// Sign-in or consent is required. When `user_action_required` is false the client invalidates
    /// the token once and retries; when true (Kotlin: a `resolution` intent was attached) only the
    /// user can fix it.
    #[error("{message}")]
    Auth {
        message: String,
        user_action_required: bool,
    },
    /// The connection failed, dropped or timed out.
    #[error("{0}")]
    Network(String),
    /// Google asked us to slow down. `retry_after_ms` is 0 when no usable `Retry-After` was sent.
    #[error("{message}")]
    RateLimited { retry_after_ms: u64, message: String },
    /// The Drive is full.
    #[error("{0}")]
    StorageQuota(String),
    /// The file, folder or upload session does not exist (any more).
    #[error("{0}")]
    NotFound(String),
    /// Any other HTTP failure. `code` 0 means "no HTTP status" (a protocol-level failure).
    #[error("{message}")]
    Http {
        code: u16,
        reason: Option<String>,
        message: String,
    },
    /// Local storage failed (reading the source or writing the destination).
    #[error("{0}")]
    Io(String),
    /// Transferred data did not verify (size or checksum).
    #[error("{0}")]
    Integrity(String),
    /// Google's sign-in service failed on its side while renewing the access token. Retried like a
    /// server error; the account stays signed in.
    #[error("Google's sign-in service had a problem on its side")]
    SignInUnavailable,
    /// Anything else, e.g. a response body that is not the JSON Drive documents.
    #[error("{0}")]
    Unknown(String),
}

pub type Result<T, E = DriveError> = std::result::Result<T, E>;

/// The longest `Retry-After` wait honoured (2 minutes).
pub const MAX_RETRY_AFTER_MS: u64 = 120_000;

const RATE_LIMIT_REASONS: [&str; 4] = [
    "userRateLimitExceeded",
    "rateLimitExceeded",
    "dailyLimitExceeded",
    "sharingRateLimitExceeded",
];

#[derive(Deserialize, Default)]
struct ApiErrorItem {
    reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct ApiErrorBody {
    message: Option<String>,
    errors: Option<Vec<ApiErrorItem>>,
    status: Option<String>,
}

#[derive(Deserialize, Default)]
struct ApiErrorEnvelope {
    error: Option<ApiErrorBody>,
}

impl DriveError {
    /// An `Auth` error the client may try to fix by refreshing the token.
    pub fn auth(message: impl Into<String>) -> Self {
        DriveError::Auth {
            message: message.into(),
            user_action_required: false,
        }
    }

    /// Maps a non-successful Drive response to an error.
    ///
    /// `status_text` is the HTTP reason phrase (may be blank), `retry_after` the raw `Retry-After`
    /// header and `body` the response body, if any could be read.
    pub fn from_response(
        status: u16,
        status_text: &str,
        retry_after: Option<&str>,
        body: Option<&str>,
    ) -> Self {
        let parsed = body
            .and_then(|b| serde_json::from_str::<ApiErrorEnvelope>(b).ok())
            .and_then(|env| env.error);
        let reason = parsed.as_ref().and_then(|p| {
            p.errors
                .as_ref()
                .and_then(|e| e.first())
                .and_then(|e| e.reason.clone())
                .or_else(|| p.status.clone())
        });
        let message = match parsed.as_ref().and_then(|p| p.message.clone()) {
            Some(m) => m,
            None if status_text.trim().is_empty() => format!("HTTP {status}"),
            None => status_text.to_string(),
        };
        // Only delta-seconds are honoured; an HTTP-date is ignored like in the Kotlin client. Capped,
        // so six waits stay under the run watchdog's 15 minutes.
        let retry_after_ms = retry_after
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(|s| s.saturating_mul(1000).min(MAX_RETRY_AFTER_MS))
            .unwrap_or(0);
        let reason_is = |r: &str| reason.as_deref() == Some(r);

        match status {
            401 => DriveError::auth(format!("Access token rejected ({message})")),
            403 if RATE_LIMIT_REASONS.iter().any(|r| reason_is(r)) => DriveError::RateLimited {
                retry_after_ms,
                message,
            },
            429 => DriveError::RateLimited {
                retry_after_ms,
                message,
            },
            403 if reason_is("storageQuotaExceeded")
                || reason_is("quotaExceeded")
                || message.to_lowercase().contains("storage quota") =>
            {
                DriveError::StorageQuota(message)
            }
            403 if reason_is("insufficientPermissions") => {
                DriveError::auth(format!("Missing Drive permission: {message}"))
            }
            404 => DriveError::NotFound(message),
            500.. => DriveError::Http {
                code: status,
                reason,
                message: format!("Google Drive server error: {message}"),
            },
            _ => DriveError::Http {
                code: status,
                reason,
                message,
            },
        }
    }

    /// Whether a retry later, without user action, could succeed.
    pub fn is_transient(&self) -> bool {
        match self {
            DriveError::Network(_) | DriveError::RateLimited { .. } | DriveError::SignInUnavailable => true,
            // 408: the server gave up waiting for the request; sending it again is the fix.
            DriveError::Http { code, .. } => *code >= 500 || *code == 408,
            _ => false,
        }
    }

    /// Drive's malware/spam scan flagged the file and refuses to serve it unless the request
    /// acknowledges that (`acknowledgeAbuse=true`, honoured only for the file's owner).
    pub fn is_flagged_abusive(&self) -> bool {
        matches!(self, DriveError::Http { code: 403, reason: Some(r), .. } if r == "cannotDownloadAbusiveFile")
    }

    /// Short, user-facing explanation with the fix. Never blank.
    pub fn user_hint(&self) -> String {
        match self {
            DriveError::Auth { .. } => {
                "Google sign-in needs your attention. Open Accounts and re-authorize.".into()
            }
            DriveError::Network(_) => {
                "No network or the connection dropped. Sync will retry automatically.".into()
            }
            DriveError::RateLimited { retry_after_ms, .. } => format!(
                "Google is rate limiting requests. Waiting {}s before retrying.",
                retry_after_ms / 1000
            ),
            DriveError::StorageQuota(_) => {
                "Google Drive storage is full. Free up space or the uploads stay paused.".into()
            }
            DriveError::NotFound(_) => {
                "The remote folder or file no longer exists. Check the pair's Drive folder.".into()
            }
            e if e.is_flagged_abusive() => "Google Drive flagged this file as malware or spam and \
                 only serves it to its owner. Download it from Drive's website if you trust it."
                .into(),
            DriveError::Http {
                code: 403,
                reason: Some(r),
                ..
            } if matches!(
                r.as_str(),
                "insufficientFilePermissions"
                    | "cannotModifyViewersCanCopyContent"
                    | "forbidden"
                    | "appNotAuthorizedToFile"
            ) =>
            {
                "You can only view this item on Drive, so Wasl can't change it. Ask its owner for \
                 edit access, or leave it out of the pair (Edit pair, Excluded)."
                    .into()
            }
            DriveError::Http { code, reason, .. } if *code >= 500 => match reason {
                Some(r) => {
                    format!("Google Drive had a server problem ({code}, {r}). The next sync tries again.")
                }
                None => format!("Google Drive had a server problem ({code}). The next sync tries again."),
            },
            DriveError::Http { code, reason, .. } => match reason {
                Some(r) => format!("Google Drive refused it ({code}, {r}). The next sync tries again."),
                None => format!("Google Drive refused it ({code}). The next sync tries again."),
            },
            DriveError::SignInUnavailable => {
                "Google's sign-in service had a problem on its side. The account stays signed in;                  the next sync tries again."
                    .into()
            }
            DriveError::Io(m) => format!("Local storage error: {m}"),
            DriveError::Integrity(m) => format!("Transferred data did not verify: {m}"),
            DriveError::Unknown(m) if !m.trim().is_empty() => m.clone(),
            DriveError::Unknown(_) => "Unknown error".into(),
        }
    }

    /// Classifies a transport error. Everything reqwest reports while talking to the server is a
    /// network failure (the Kotlin `net {}` wrapper); only a request we could not even build is not.
    /// The text keeps the URL's path but never its query: that holds file names (`q`), page tokens
    /// and upload session ids, and these messages are stored and shown (security review 2026-10).
    pub(crate) fn from_reqwest(e: reqwest::Error) -> Self {
        let at = e.url().map(|u| format!(" ({})", u.path())).unwrap_or_default();
        let e = e.without_url();
        if e.is_builder() {
            DriveError::Unknown(format!("invalid request{at}: {e}"))
        } else {
            DriveError::Network(format!("network error{at}: {e}"))
        }
    }

    pub(crate) fn decode(e: serde_json::Error) -> Self {
        DriveError::Unknown(format!("unexpected response from Google Drive: {e}"))
    }
}
