//! Google sign-in for the desktop (SRS §2.5): the installed-app flow with a loopback redirect and
//! PKCE. The browser opens Google's consent page; Google redirects to `http://127.0.0.1:<port>/`
//! where this module waits for the code, then exchanges it for tokens. The refresh token goes to
//! the OS keychain (see `tokens`); nothing is written to disk in the clear.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// The OAuth client and Google's endpoints. Tests point the endpoints at a mock.
#[derive(Debug, Clone)]
pub struct OAuthConfig {
    pub client_id: String,
    /// Google issues a "secret" to installed apps; it is not confidential (RFC 8252 §8.5).
    pub client_secret: String,
    pub auth_url: String,
    pub token_url: String,
    pub userinfo_url: String,
    pub scopes: Vec<String>,
}

impl OAuthConfig {
    /// Google's endpoints with the client baked in at build time (`WASL_GOOGLE_CLIENT_ID`,
    /// `WASL_GOOGLE_CLIENT_SECRET`), overridable at run time by the same environment variables in
    /// debug builds only.
    pub fn google() -> OAuthConfig {
        let id = run_time_override("WASL_GOOGLE_CLIENT_ID")
            .or(option_env!("WASL_GOOGLE_CLIENT_ID").map(str::to_owned))
            .unwrap_or_default();
        let secret = run_time_override("WASL_GOOGLE_CLIENT_SECRET")
            .or(option_env!("WASL_GOOGLE_CLIENT_SECRET").map(str::to_owned))
            .unwrap_or_default();
        OAuthConfig {
            client_id: id,
            client_secret: secret,
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token_url: "https://oauth2.googleapis.com/token".into(),
            userinfo_url: "https://openidconnect.googleapis.com/v1/userinfo".into(),
            scopes: vec![
                "openid".into(),
                "email".into(),
                "https://www.googleapis.com/auth/drive".into(),
            ],
        }
    }

    pub fn is_configured(&self) -> bool {
        !self.client_id.is_empty()
    }

    /// Google's revocation endpoint. It sits beside the token endpoint, so a test that mocks one
    /// mocks both.
    pub fn revoke_url(&self) -> String {
        match self.token_url.strip_suffix("/token") {
            Some(base) => format!("{base}/revoke"),
            None => GOOGLE_REVOKE_URL.into(),
        }
    }
}

const GOOGLE_REVOKE_URL: &str = "https://oauth2.googleapis.com/revoke";

/// An environment variable that replaces a built-in OAuth client setting, in debug builds only:
/// in a release build, anything that can set the daemon's environment could otherwise send the
/// sign-in (and the refresh token) to a client of its choosing (security review 2026-10).
fn run_time_override(name: &str) -> Option<String> {
    if cfg!(debug_assertions) {
        std::env::var(name).ok()
    } else {
        None
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    #[error("This build of Wasl has no Google sign-in client. Set WASL_GOOGLE_CLIENT_ID.")]
    NotConfigured,
    #[error("Sign-in was cancelled or refused: {0}")]
    Refused(String),
    #[error("Sign-in took too long. Try again.")]
    TimedOut,
    #[error("Google's answer could not be used: {0}")]
    Protocol(String),
    #[error("Couldn't reach Google: {0}")]
    Network(String),
    #[error("Google no longer accepts this account's sign-in. Sign in again.")]
    Revoked,
    /// A failure on Google's side (a 5xx, or an error code OAuth does not define, such as
    /// `internal_failure`). It passes: a refresh is retried and the account stays signed in. The
    /// raw code goes to the log only (docs/ERRORS.md story 5).
    #[error("Google's sign-in service had a problem on its side. Try again in a few minutes.")]
    Unavailable,
}

/// What a finished sign-in yields. Its `Debug` leaves the tokens out.
#[derive(Clone)]
pub struct SignedIn {
    pub email: String,
    pub refresh_token: String,
    pub access_token: String,
    pub expires_in: u64,
}

impl std::fmt::Debug for SignedIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignedIn")
            .field("email", &self.email)
            .field("refresh_token", &"…")
            .field("access_token", &"…")
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Deserialize)]
struct TokenError {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct UserInfo {
    email: String,
}

/// A sign-in waiting for the browser: open [`Pending::url`], then await [`Pending::finish`].
pub struct Pending {
    pub url: String,
    listener: TcpListener,
    redirect: String,
    verifier: String,
    state: String,
    config: OAuthConfig,
}

fn random(len: usize) -> Result<String, OAuthError> {
    let mut b = vec![0u8; len];
    getrandom::fill(&mut b).map_err(|e| OAuthError::Protocol(e.to_string()))?;
    Ok(B64URL.encode(b))
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn dec(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let h = std::str::from_utf8(&bytes[i + 1..i + 3])
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok());
                match h {
                    Some(v) => {
                        out.push(v);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Starts a sign-in: binds the loopback port and builds the consent URL.
pub async fn begin(config: &OAuthConfig) -> Result<Pending, OAuthError> {
    if !config.is_configured() {
        return Err(OAuthError::NotConfigured);
    }
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| OAuthError::Network(e.to_string()))?;
    let port = listener
        .local_addr()
        .map_err(|e| OAuthError::Network(e.to_string()))?
        .port();
    let redirect = format!("http://127.0.0.1:{port}/");
    let verifier = random(48)?;
    let challenge = B64URL.encode(Sha256::digest(verifier.as_bytes()));
    let state = random(24)?;
    let url = format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={}&code_challenge_method=S256&state={}&access_type=offline&prompt=consent",
        config.auth_url,
        enc(&config.client_id),
        enc(&redirect),
        enc(&config.scopes.join(" ")),
        challenge,
        enc(&state),
    );
    Ok(Pending {
        url,
        listener,
        redirect,
        verifier,
        state,
        config: config.clone(),
    })
}

const PAGE_OK: &str = "<!doctype html><meta charset=utf-8><title>Wasl</title><body style=\"font-family:system-ui;margin:3em\"><h1>Signed in</h1><p>You can close this tab and go back to Wasl.</p><p dir=rtl lang=ar>تم تسجيل الدخول. يمكنك إغلاق هذه الصفحة والعودة إلى وصل.</p>";
const PAGE_FAIL: &str = "<!doctype html><meta charset=utf-8><title>Wasl</title><body style=\"font-family:system-ui;margin:3em\"><h1>Sign-in didn't finish</h1><p>Go back to Wasl and try again.</p><p dir=rtl lang=ar>لم يكتمل تسجيل الدخول. عُد إلى وصل وحاول مرة أخرى.</p>";

impl Pending {
    /// Waits (up to `timeout`) for Google's redirect, then exchanges the code.
    pub async fn finish(self, http: &reqwest::Client, timeout: Duration) -> Result<SignedIn, OAuthError> {
        let code = tokio::time::timeout(timeout, self.wait_for_code())
            .await
            .map_err(|_| OAuthError::TimedOut)??;
        let resp = http
            .post(&self.config.token_url)
            .form(&[
                ("code", code.as_str()),
                ("client_id", self.config.client_id.as_str()),
                ("client_secret", self.config.client_secret.as_str()),
                ("redirect_uri", self.redirect.as_str()),
                ("grant_type", "authorization_code"),
                ("code_verifier", self.verifier.as_str()),
            ])
            .send()
            .await
            .map_err(|e| OAuthError::Network(e.to_string()))?;
        let tokens: TokenResponse = token_body(resp).await?;
        let refresh_token = tokens
            .refresh_token
            .ok_or_else(|| OAuthError::Protocol("Google sent no refresh token".into()))?;
        let email = userinfo(http, &self.config, &tokens.access_token).await?;
        Ok(SignedIn {
            email,
            refresh_token,
            access_token: tokens.access_token,
            expires_in: tokens.expires_in.unwrap_or(3600),
        })
    }

    /// Serves the loopback port until a request carries `code` (or `error`) with our `state`.
    /// Anything else (a favicon request, a stray visitor) gets a 404 and is ignored.
    async fn wait_for_code(&self) -> Result<String, OAuthError> {
        loop {
            let (mut sock, _) = self
                .listener
                .accept()
                .await
                .map_err(|e| OAuthError::Network(e.to_string()))?;
            let mut buf = vec![0u8; 8192];
            let n = tokio::time::timeout(Duration::from_secs(5), sock.read(&mut buf))
                .await
                .unwrap_or(Ok(0))
                .unwrap_or(0);
            let head = String::from_utf8_lossy(&buf[..n]);
            let target = head
                .lines()
                .next()
                .and_then(|l| l.strip_prefix("GET "))
                .and_then(|l| l.split(' ').next())
                .unwrap_or("");
            let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
            let params: Vec<(String, String)> = query
                .split('&')
                .filter_map(|kv| kv.split_once('='))
                .map(|(k, v)| (dec(k), dec(v)))
                .collect();
            let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
            if get("state").as_deref() != Some(self.state.as_str()) {
                let _ = sock
                    .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await;
                continue;
            }
            let (page, result) = match (get("code"), get("error")) {
                (Some(code), _) => (PAGE_OK, Ok(code)),
                (None, Some(err)) => (PAGE_FAIL, Err(OAuthError::Refused(err))),
                (None, None) => (
                    PAGE_FAIL,
                    Err(OAuthError::Protocol("no code in the redirect".into())),
                ),
            };
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
                page.len()
            );
            let _ = sock.write_all(reply.as_bytes()).await;
            let _ = sock.shutdown().await;
            return result;
        }
    }
}

async fn token_body(resp: reqwest::Response) -> Result<TokenResponse, OAuthError> {
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| OAuthError::Network(e.to_string()))?;
    if status.is_success() {
        return serde_json::from_str(&text).map_err(|e| OAuthError::Protocol(e.to_string()));
    }
    let err: Option<TokenError> = serde_json::from_str(&text).ok();
    match err {
        _ if status.is_server_error() => Err(unavailable(
            &err.map_or_else(|| format!("HTTP {status}"), |e| e.error),
        )),
        Some(e) if e.error == "invalid_grant" => Err(OAuthError::Revoked),
        Some(e) if REQUEST_ERRORS.contains(&e.error.as_str()) => {
            Err(OAuthError::Refused(e.error_description.unwrap_or(e.error)))
        }
        // `internal_failure` and the like: Google's own failure, not this request's (#52).
        Some(e) => Err(unavailable(&e.error)),
        None => Err(OAuthError::Protocol(format!("HTTP {status}"))),
    }
}

/// [`OAuthError::Unavailable`], with Google's raw answer in the log.
fn unavailable(raw: &str) -> OAuthError {
    eprintln!("wasld: Google's token endpoint failed on its side: {raw}");
    OAuthError::Unavailable
}

/// The token endpoint's error codes for a request Google refuses (RFC 6749 §5.2): only these, and
/// `invalid_grant`, end a sign-in.
const REQUEST_ERRORS: [&str; 5] = [
    "invalid_request",
    "invalid_client",
    "unauthorized_client",
    "unsupported_grant_type",
    "invalid_scope",
];

async fn userinfo(http: &reqwest::Client, config: &OAuthConfig, access: &str) -> Result<String, OAuthError> {
    let resp = http
        .get(&config.userinfo_url)
        .bearer_auth(access)
        .send()
        .await
        .map_err(|e| OAuthError::Network(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(OAuthError::Protocol(format!("userinfo HTTP {}", resp.status())));
    }
    let info: UserInfo = resp
        .json()
        .await
        .map_err(|e| OAuthError::Protocol(e.to_string()))?;
    Ok(info.email)
}

/// A fresh access token from a refresh token.
pub async fn refresh(
    http: &reqwest::Client,
    config: &OAuthConfig,
    refresh_token: &str,
) -> Result<(String, u64), OAuthError> {
    let resp = http
        .post(&config.token_url)
        .form(&[
            ("client_id", config.client_id.as_str()),
            ("client_secret", config.client_secret.as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|e| OAuthError::Network(e.to_string()))?;
    let t = token_body(resp).await?;
    Ok((t.access_token, t.expires_in.unwrap_or(3600)))
}

/// Asks Google to revoke `token` (a refresh token revokes the whole grant). Any HTTP answer
/// counts; only a request that got none is an error.
pub async fn revoke(http: &reqwest::Client, config: &OAuthConfig, token: &str) -> Result<(), OAuthError> {
    http.post(config.revoke_url())
        .form(&[("token", token)])
        .send()
        .await
        .map(|_| ())
        .map_err(|e| OAuthError::Network(e.without_url().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Security review L2 (#136): a sign-in result printed with `{:?}` (a log line, a panic)
    /// carries neither token.
    #[test]
    fn a_sign_in_result_never_prints_its_tokens() {
        let s = SignedIn {
            email: "me@example.com".into(),
            refresh_token: "fake-refresh-1234".into(),
            access_token: "fake-access-5678".into(),
            expires_in: 3600,
        };
        let shown = format!("{s:?}");
        assert!(!shown.contains("fake-refresh-1234"), "{shown}");
        assert!(!shown.contains("fake-access-5678"), "{shown}");
        assert!(shown.contains("me@example.com"), "{shown}");
    }

    #[test]
    fn query_values_round_trip() {
        assert_eq!("a b/c+d", dec(&enc("a b/c+d")));
        assert_eq!("4/0AY0e", dec("4%2F0AY0e"));
        assert_eq!("x y", dec("x+y"));
        assert_eq!("50%", dec("50%"));
    }

    /// An escape at the very end of the value decodes too, and a cut-off one stays as it is.
    #[test]
    fn a_trailing_escape_decodes() {
        assert_eq!("A", dec("%41"));
        assert_eq!("aA", dec("a%41"));
        assert_eq!("code/", dec("code%2F"));
        assert_eq!("%4", dec("%4"));
        assert_eq!("a%", dec("a%"));
    }

    #[test]
    fn revocation_sits_beside_the_token_endpoint() {
        let google = OAuthConfig::google();
        assert_eq!("https://oauth2.googleapis.com/revoke", google.revoke_url());
        let mock = OAuthConfig {
            token_url: "http://127.0.0.1:9/token".into(),
            ..google
        };
        assert_eq!("http://127.0.0.1:9/revoke", mock.revoke_url());
    }
}
