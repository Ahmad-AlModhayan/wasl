//! The local protocol of the desktop apps (SRS FR-E4, FR-A3, NFR-6): `wasld` owns the engine; the
//! `wasl` CLI and the tray app are clients. One JSON value per line over a socket only the current
//! OS user can reach, opened by a handshake in which both sides prove they know a per-user token.

mod auth;
pub mod client;
pub mod paths;
pub mod proto;
pub mod server;
pub mod transport;

pub use auth::Token;
pub use client::{Client, ClientError};
pub use paths::Paths;
pub use proto::*;
pub use server::{Handler, Server};

/// Serialises a handler's result.
pub fn ok<T: serde::Serialize>(value: T) -> Result<serde_json::Value, RemoteError> {
    serde_json::to_value(value).map_err(|e| RemoteError {
        kind: ErrorKind::Other,
        message: e.to_string(),
    })
}

/// [`ok`], or an error with `too_big(bytes)` as its message when the answer would pass `max` bytes
/// as a line. A client refuses a line over [`transport::MAX_LINE`], so without this it would see
/// only a broken connection.
pub fn ok_within<T: serde::Serialize>(
    value: T,
    max: usize,
    too_big: impl FnOnce(usize) -> String,
) -> Result<serde_json::Value, RemoteError> {
    let v = ok(value)?;
    let bytes = serde_json::to_vec(&v).map_or(usize::MAX, |b| b.len());
    if bytes > max {
        return Err(RemoteError {
            kind: ErrorKind::Other,
            message: too_big(bytes),
        });
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::{ErrorKind, ok_within};

    #[test]
    fn an_answer_over_the_line_limit_is_an_error_that_says_so() {
        let fits = ok_within(vec![1, 2, 3], 7, |_| unreachable!()).unwrap();
        assert_eq!(serde_json::json!([1, 2, 3]), fits);
        let e = ok_within(vec![1, 2, 3], 6, |n| format!("{n} bytes")).unwrap_err();
        assert_eq!((ErrorKind::Other, "7 bytes"), (e.kind, e.message.as_str()));
    }
}
