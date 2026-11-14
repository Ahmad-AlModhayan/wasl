//! Access tokens for the Drive client. Refresh tokens live in the OS keychain (Windows Credential
//! Manager, macOS Keychain, Secret Service); access tokens only in memory, refreshed shortly
//! before they expire.

use crate::oauth::{self, OAuthConfig, OAuthError};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wasl_drive::{DriveError, TokenProvider};
use wasl_store::SharedStore;

/// Where refresh tokens are kept. The keychain in the app, a map in tests.
pub trait Secrets: Send + Sync {
    fn get(&self, account: &str) -> Option<String>;
    fn set(&self, account: &str, secret: &str) -> Result<(), String>;
    fn remove(&self, account: &str);
}

/// The OS keychain, one entry per account under the service "Wasl".
pub struct Keychain {
    service: String,
}

impl Keychain {
    pub fn new(service: impl Into<String>) -> Keychain {
        Keychain {
            service: service.into(),
        }
    }
}

impl Secrets for Keychain {
    fn get(&self, account: &str) -> Option<String> {
        keyring::Entry::new(&self.service, account)
            .ok()?
            .get_password()
            .ok()
    }
    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        keyring::Entry::new(&self.service, account)
            .and_then(|e| e.set_password(secret))
            .map_err(|e| format!("The system keychain refused the sign-in: {e}"))
    }
    fn remove(&self, account: &str) {
        if let Ok(e) = keyring::Entry::new(&self.service, account) {
            let _ = e.delete_credential();
        }
    }
}

/// One data folder's share of a secret store (#111). The OS keychain is one per user, shared by
/// every daemon: the default data folder keeps the bare entry names the installed app has always
/// used, and any other folder (`WASL_HOME`, `--home`) files each entry under its folder's id, so a
/// test or portable daemon neither reads nor overwrites the real install's tokens and device key.
pub struct HomeSecrets {
    inner: Arc<dyn Secrets>,
    scope: Option<String>,
}

impl HomeSecrets {
    /// `scope` from [`home_scope`]: `None` for the default data folder.
    pub fn new(inner: Arc<dyn Secrets>, scope: Option<String>) -> HomeSecrets {
        HomeSecrets { inner, scope }
    }

    fn key(&self, account: &str) -> String {
        match &self.scope {
            Some(scope) => format!("{scope}/{account}"),
            None => account.to_string(),
        }
    }
}

impl Secrets for HomeSecrets {
    fn get(&self, account: &str) -> Option<String> {
        self.inner.get(&self.key(account))
    }
    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        self.inner.set(&self.key(account), secret)
    }
    fn remove(&self, account: &str) {
        self.inner.remove(&self.key(account))
    }
}

/// The id a data folder's secrets are filed under: `None` for this user's default folder
/// (`default`), else `home-` and 16 hex digits of the folder's resolved path. With no default
/// folder known, every folder is scoped.
///
/// On Unix the default folder is recognised by file identity (device and inode), so another
/// spelling of it still counts: a symlink, `..`, or a different case on macOS's case-insensitive
/// volumes, where `canonicalize` keeps the case it was given. On Windows `canonicalize` returns
/// the case on disk. When either folder can't be read, the paths are compared as resolved; a
/// mismatch only ever scopes the folder, never shares the installed app's entries.
pub fn home_scope(data: &std::path::Path, default: Option<&std::path::Path>) -> Option<String> {
    use sha2::{Digest, Sha256};
    let resolve = |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let same_folder = |a: &std::path::Path, b: &std::path::Path| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let (Ok(x), Ok(y)) = (std::fs::metadata(a), std::fs::metadata(b)) {
                return x.dev() == y.dev() && x.ino() == y.ino();
            }
        }
        resolve(a) == resolve(b)
    };
    if default.is_some_and(|d| same_folder(d, data)) {
        return None;
    }
    let data = resolve(data);
    let h = Sha256::digest(data.to_string_lossy().as_bytes());
    let hex: String = h[..8].iter().map(|b| format!("{b:02x}")).collect();
    Some(format!("home-{hex}"))
}

/// The keychain service a daemon's tokens and device key live under.
pub const KEYCHAIN_SERVICE: &str = "Wasl";

/// A daemon's two secret stores: tokens and the device key, and vault keys.
pub struct HomeStores {
    pub secrets: Arc<dyn Secrets>,
    pub vault_keys: Arc<dyn Secrets>,
}

/// The secret stores for the data folder `data` (#111): `open` gives the shared store of a
/// keychain service, and both are scoped to the folder unless it is the default one.
pub fn secrets_for(
    data: &std::path::Path,
    default: Option<&std::path::Path>,
    open: impl Fn(&str) -> Arc<dyn Secrets>,
) -> HomeStores {
    let scope = home_scope(data, default);
    HomeStores {
        secrets: Arc::new(HomeSecrets::new(open(KEYCHAIN_SERVICE), scope.clone())),
        vault_keys: Arc::new(HomeSecrets::new(open(crate::vaults::KEYCHAIN_SERVICE), scope)),
    }
}

/// In memory, for tests.
#[derive(Default)]
pub struct MemorySecrets(Mutex<HashMap<String, String>>);

impl Secrets for MemorySecrets {
    fn get(&self, account: &str) -> Option<String> {
        self.0.lock().ok()?.get(account).cloned()
    }
    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        self.0
            .lock()
            .map_err(|e| e.to_string())?
            .insert(account.into(), secret.into());
        Ok(())
    }
    fn remove(&self, account: &str) {
        if let Ok(mut m) = self.0.lock() {
            m.remove(account);
        }
    }
}

struct Cached {
    token: String,
    until: Instant,
}

/// The Drive client's token source for every account of this user.
pub struct DesktopTokens {
    pub config: OAuthConfig,
    pub secrets: Arc<dyn Secrets>,
    pub store: SharedStore,
    http: reqwest::Client,
    cache: tokio::sync::Mutex<HashMap<String, Cached>>,
    /// Per account: refreshes Google failed on its side in a row, and when the first of them was.
    unavailable: Mutex<HashMap<String, (u32, Instant)>>,
}

/// Google-side refresh failures in a row before the account asks to be signed in again: about
/// three syncs, as each tries six times. Signing in again is what cleared #52.
const UNAVAILABLE_TRIES: u32 = 18;
/// How long Google-side refresh failures may last before the account asks to be signed in again.
const UNAVAILABLE_FOR: Duration = Duration::from_secs(3600);
/// The account's problem once Google's failures last (#52).
const KEEPS_FAILING: &str =
    "Google's sign-in service keeps failing for this account. Sign in to this Google account again.";

/// How long account removal waits for Google to revoke the refresh token.
const REVOKE_TIMEOUT: Duration = Duration::from_secs(10);

impl DesktopTokens {
    pub fn new(config: OAuthConfig, secrets: Arc<dyn Secrets>, store: SharedStore) -> DesktopTokens {
        DesktopTokens {
            config,
            secrets,
            store,
            http: token_client(),
            cache: tokio::sync::Mutex::new(HashMap::new()),
            unavailable: Mutex::new(HashMap::new()),
        }
    }

    /// Keeps a finished sign-in: refresh token to the keychain, access token in memory.
    pub async fn remember(&self, signed: &oauth::SignedIn) -> Result<(), String> {
        self.secrets.set(&signed.email, &signed.refresh_token)?;
        self.cache.lock().await.insert(
            signed.email.clone(),
            Cached {
                token: signed.access_token.clone(),
                until: Instant::now() + Duration::from_secs(signed.expires_in.saturating_sub(120)),
            },
        );
        let s = self.store.lock();
        let _ = s.set_account_email(&signed.email, &signed.email);
        let _ = s.set_account_auth_error(&signed.email, None);
        Ok(())
    }

    /// Signs `account` out of this computer: Google revokes its refresh token, then the keychain
    /// entry goes. Revocation is best effort (security review 2026-10): without a network the
    /// account is still removed here, and the grant stays in the Google account's third-party
    /// access until the person removes it there.
    pub async fn forget(&self, account: &str) {
        self.cache.lock().await.remove(account);
        if let Some(refresh) = self.secrets.get(account) {
            let revoke = oauth::revoke(&self.http, &self.config, &refresh);
            let _ = tokio::time::timeout(REVOKE_TIMEOUT, revoke).await;
        }
        self.secrets.remove(account);
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }
}

impl DesktopTokens {
    fn unavailable_lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, (u32, Instant)>> {
        self.unavailable.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[async_trait]
impl TokenProvider for DesktopTokens {
    async fn access_token(&self, account: &str) -> Result<String, DriveError> {
        if let Some(c) = self.cache.lock().await.get(account)
            && c.until > Instant::now()
        {
            return Ok(c.token.clone());
        }
        let Some(refresh) = self.secrets.get(account) else {
            let msg = "Sign in to this Google account again.".to_owned();
            let _ = self.store.lock().set_account_auth_error(account, Some(&msg));
            return Err(DriveError::Auth {
                message: msg,
                user_action_required: true,
            });
        };
        match oauth::refresh(&self.http, &self.config, &refresh).await {
            Ok((token, expires_in)) => {
                self.cache.lock().await.insert(
                    account.into(),
                    Cached {
                        token: token.clone(),
                        until: Instant::now() + Duration::from_secs(expires_in.saturating_sub(120)),
                    },
                );
                let _ = self.store.lock().set_account_auth_error(account, None);
                self.unavailable_lock().remove(account);
                Ok(token)
            }
            Err(OAuthError::Network(e)) => Err(DriveError::Network(e)),
            // Retried with backoff like any server error; the account stays signed in until the
            // failures last too long.
            Err(OAuthError::Unavailable) => {
                let lasting = {
                    let mut seen = self.unavailable_lock();
                    let (tries, since) = seen.entry(account.into()).or_insert((0, Instant::now()));
                    *tries += 1;
                    *tries >= UNAVAILABLE_TRIES || since.elapsed() >= UNAVAILABLE_FOR
                };
                if !lasting {
                    return Err(DriveError::SignInUnavailable);
                }
                let _ = self
                    .store
                    .lock()
                    .set_account_auth_error(account, Some(KEEPS_FAILING));
                Err(DriveError::Auth {
                    message: KEEPS_FAILING.into(),
                    user_action_required: true,
                })
            }
            Err(e) => {
                let msg = e.to_string();
                let _ = self.store.lock().set_account_auth_error(account, Some(&msg));
                Err(DriveError::Auth {
                    message: msg,
                    user_action_required: true,
                })
            }
        }
    }

    async fn invalidate(&self, account: &str) {
        self.cache.lock().await.remove(account);
    }
}

/// The client for Google's OAuth endpoints. It carries authorization codes and refresh tokens, so
/// it follows no redirect, and it never waits forever: the Drive client's retries wait on it
/// (security review 2026-10).
fn token_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(60))
        .build()
        // Building fails only when the TLS backend cannot start, and then `Client::new` panics
        // the same way; there is nothing better to fall back to.
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Keychain entry holding the device key's seed (hex).
const KEY_ACCOUNT: &str = "device-key";

/// The device key's seed in the OS keychain.
pub struct KeychainKeyStore(pub Arc<dyn Secrets>);

impl wasl_net::KeyStore for KeychainKeyStore {
    fn load(&self) -> Result<Option<zeroize::Zeroizing<[u8; 32]>>, wasl_net::IdentityError> {
        let Some(hex) = self.0.get(KEY_ACCOUNT).map(zeroize::Zeroizing::new) else {
            return Ok(None);
        };
        let mut seed = zeroize::Zeroizing::new([0u8; 32]);
        if hex.len() != 64 {
            return Err(wasl_net::IdentityError::KeyStore(
                "the stored device key is damaged".into(),
            ));
        }
        for (i, b) in seed.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16)
                .map_err(|_| wasl_net::IdentityError::KeyStore("the stored device key is damaged".into()))?;
        }
        Ok(Some(seed))
    }

    fn save(&self, seed: &[u8; 32]) -> Result<(), wasl_net::IdentityError> {
        let hex = zeroize::Zeroizing::new(seed.iter().map(|b| format!("{b:02x}")).collect::<String>());
        self.0
            .set(KEY_ACCOUNT, &hex)
            .map_err(wasl_net::IdentityError::KeyStore)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_string, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ME: &str = "me@example.com";

    fn tokens(token_url: String) -> (DesktopTokens, Arc<MemorySecrets>) {
        let secrets = Arc::new(MemorySecrets::default());
        let config = OAuthConfig {
            client_id: "test-client".into(),
            client_secret: "test-secret".into(),
            auth_url: "http://127.0.0.1:9/auth".into(),
            token_url,
            userinfo_url: "http://127.0.0.1:9/userinfo".into(),
            scopes: vec![],
        };
        let store = SharedStore::new(wasl_store::Store::open_in_memory().unwrap());
        (DesktopTokens::new(config, secrets.clone(), store), secrets)
    }

    /// #52: Google's token endpoint answering `internal_failure` (its own failure) is a passing
    /// problem Wasl retries, not a sign-in the user must redo; `invalid_grant` still is one.
    #[tokio::test]
    async fn a_failure_on_google_s_side_is_retried_not_a_sign_in_to_redo() {
        for (status, body) in [
            (400, r#"{"error":"internal_failure"}"#),
            (
                500,
                r#"{"error":"server_error","error_description":"Backend Error"}"#,
            ),
            (503, "Service Unavailable"),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/token"))
                .respond_with(ResponseTemplate::new(status).set_body_raw(body, "application/json"))
                .mount(&server)
                .await;
            let (t, secrets) = tokens(format!("{}/token", server.uri()));
            t.store
                .lock()
                .upsert_drive_account(&wasl_drive::AccountState::new(ME))
                .unwrap();
            secrets.set(ME, "refresh-1").unwrap();
            let e = t.access_token(ME).await.unwrap_err();
            assert!(e.is_transient(), "{status} {body}: {e:?}");
            assert_eq!(
                None,
                t.store.lock().account_auth_error(ME).unwrap(),
                "{status} {body}"
            );
            assert_eq!(Some("refresh-1".to_owned()), secrets.get(ME));
        }

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(
                ResponseTemplate::new(400).set_body_raw(r#"{"error":"invalid_grant"}"#, "application/json"),
            )
            .mount(&server)
            .await;
        let (t, secrets) = tokens(format!("{}/token", server.uri()));
        t.store
            .lock()
            .upsert_drive_account(&wasl_drive::AccountState::new(ME))
            .unwrap();
        secrets.set(ME, "refresh-1").unwrap();
        let e = t.access_token(ME).await.unwrap_err();
        assert!(!e.is_transient(), "{e:?}");
        assert!(t.store.lock().account_auth_error(ME).unwrap().is_some());
    }

    /// #52: when Google keeps failing (about three syncs in a row), the account asks to be signed in
    /// again, which is what cleared it; a success in between starts the count over.
    #[tokio::test]
    async fn google_failing_again_and_again_asks_to_sign_in_again() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_raw(r#"{"error":"internal_failure"}"#, "application/json"),
            )
            .mount(&server)
            .await;
        let (t, secrets) = tokens(format!("{}/token", server.uri()));
        t.store
            .lock()
            .upsert_drive_account(&wasl_drive::AccountState::new(ME))
            .unwrap();
        secrets.set(ME, "refresh-1").unwrap();
        for _ in 1..UNAVAILABLE_TRIES {
            let e = t.access_token(ME).await.unwrap_err();
            assert!(matches!(e, DriveError::SignInUnavailable), "{e:?}");
        }
        assert_eq!(None, t.store.lock().account_auth_error(ME).unwrap());
        let e = t.access_token(ME).await.unwrap_err();
        assert!(
            matches!(&e, DriveError::Auth { user_action_required: true, message } if message == KEEPS_FAILING),
            "{e:?}"
        );
        assert_eq!(
            Some(KEEPS_FAILING.to_owned()),
            t.store.lock().account_auth_error(ME).unwrap()
        );
        // The user reads plain words, never Google's raw code.
        assert!(
            !DriveError::SignInUnavailable
                .user_hint()
                .contains("internal_failure")
        );
    }

    #[tokio::test]
    async fn removing_an_account_revokes_its_refresh_token_first() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/revoke"))
            .and(body_string("token=refresh-1"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let (t, secrets) = tokens(format!("{}/token", server.uri()));
        secrets.set(ME, "refresh-1").unwrap();
        t.forget(ME).await;
        assert_eq!(None, secrets.get(ME));
        server.verify().await;
    }

    #[tokio::test]
    async fn removal_goes_on_when_google_refuses_or_cannot_be_reached() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/revoke"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let (t, secrets) = tokens(format!("{}/token", server.uri()));
        secrets.set(ME, "refresh-1").unwrap();
        t.forget(ME).await;
        assert_eq!(None, secrets.get(ME));

        // Nothing listens there.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = closed.local_addr().unwrap();
        drop(closed);
        let (t, secrets) = tokens(format!("http://{addr}/token"));
        secrets.set(ME, "refresh-1").unwrap();
        t.forget(ME).await;
        assert_eq!(None, secrets.get(ME));
    }

    /// A redirect from the token endpoint is an answer to refuse, not one to follow with the
    /// refresh token.
    #[tokio::test]
    async fn the_token_client_follows_no_redirect() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(
                ResponseTemplate::new(307).insert_header("Location", format!("{}/elsewhere", server.uri())),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/elsewhere"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"access_token":"stolen","expires_in":3600}"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        let (t, secrets) = tokens(format!("{}/token", server.uri()));
        secrets.set(ME, "refresh-1").unwrap();
        assert!(t.access_token(ME).await.is_err());
        let followed = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.url.path() == "/elsewhere");
        assert!(!followed);
    }

    /// The installed app's entries for an account and the device key, as the default home files them.
    fn default_home_entries() -> Arc<MemorySecrets> {
        let shared = Arc::new(MemorySecrets::default());
        shared.set(ME, "refresh-real").unwrap();
        shared.set(KEY_ACCOUNT, "real-device-seed").unwrap();
        shared
    }

    #[test]
    fn a_separate_home_neither_sees_nor_overwrites_the_default_homes_secrets() {
        let shared = default_home_entries();
        let dir = tempfile::tempdir().unwrap();
        let scope = home_scope(&dir.path().join("test-home"), Some(&dir.path().join("Wasl")));
        assert!(scope.is_some());
        let home = HomeSecrets::new(shared.clone(), scope);
        assert_eq!(None, home.get(ME));
        assert_eq!(None, home.get(KEY_ACCOUNT));

        home.set(ME, "refresh-test").unwrap();
        home.remove(KEY_ACCOUNT);
        assert_eq!(Some("refresh-test".to_string()), home.get(ME));
        assert_eq!(Some("refresh-real".to_string()), shared.get(ME));
        assert_eq!(Some("real-device-seed".to_string()), shared.get(KEY_ACCOUNT));
        home.remove(ME);
        assert_eq!(None, home.get(ME));
        assert_eq!(Some("refresh-real".to_string()), shared.get(ME));
    }

    #[test]
    fn the_default_home_keeps_the_names_the_installed_app_already_uses() {
        let shared = default_home_entries();
        let dir = tempfile::tempdir().unwrap();
        let default = dir.path().join("Wasl");
        std::fs::create_dir_all(&default).unwrap();
        // The same folder by another spelling is still the default home.
        let scope = home_scope(&default.join("..").join("Wasl"), Some(&default));
        assert_eq!(None, scope);
        let home = HomeSecrets::new(shared, scope);
        assert_eq!(Some("refresh-real".to_string()), home.get(ME));
        assert_eq!(Some("real-device-seed".to_string()), home.get(KEY_ACCOUNT));
    }

    #[test]
    fn the_default_home_spelled_in_another_case_is_still_the_default_home() {
        let dir = tempfile::tempdir().unwrap();
        let default = dir.path().join("Wasl");
        std::fs::create_dir_all(&default).unwrap();
        let other_case = dir.path().join("WASL");
        // Case-insensitive volumes (macOS's default) find the folder by either name; on a
        // case-sensitive one it is another folder, which is scoped.
        let same_folder = other_case.exists();
        assert_eq!(same_folder, home_scope(&other_case, Some(&default)).is_none());
        #[cfg(target_os = "macos")]
        if same_folder {
            assert_eq!(None, home_scope(&other_case, Some(&default)));
        }
    }

    /// The raw stores of both keychain services, holding the installed app's entries.
    fn shared_services() -> (Arc<MemorySecrets>, Arc<MemorySecrets>) {
        let vaults = Arc::new(MemorySecrets::default());
        vaults.set("vault-1", "real-vault-key").unwrap();
        (default_home_entries(), vaults)
    }

    fn open_from(
        tokens: &Arc<MemorySecrets>,
        vaults: &Arc<MemorySecrets>,
    ) -> impl Fn(&str) -> Arc<dyn Secrets> {
        let (tokens, vaults) = (tokens.clone(), vaults.clone());
        move |service: &str| -> Arc<dyn Secrets> {
            match service {
                KEYCHAIN_SERVICE => tokens.clone(),
                crate::vaults::KEYCHAIN_SERVICE => vaults.clone(),
                other => panic!("unexpected keychain service {other}"),
            }
        }
    }

    #[test]
    fn a_separate_homes_tokens_device_key_and_vault_keys_are_all_its_own() {
        use wasl_net::KeyStore as _;
        let (tokens, vaults) = shared_services();
        let dir = tempfile::tempdir().unwrap();
        let stores = secrets_for(
            &dir.path().join("test-home"),
            Some(&dir.path().join("Wasl")),
            open_from(&tokens, &vaults),
        );
        assert_eq!(None, stores.secrets.get(ME));
        assert!(KeychainKeyStore(stores.secrets.clone()).load().unwrap().is_none());
        assert_eq!(None, stores.vault_keys.get("vault-1"));
        stores.secrets.set(ME, "refresh-test").unwrap();
        stores.vault_keys.set("vault-1", "test-vault-key").unwrap();
        assert_eq!(Some("refresh-real".to_string()), tokens.get(ME));
        assert_eq!(Some("real-vault-key".to_string()), vaults.get("vault-1"));
    }

    #[test]
    fn the_default_homes_stores_read_the_installed_apps_entries() {
        let (tokens, vaults) = shared_services();
        let dir = tempfile::tempdir().unwrap();
        let default = dir.path().join("Wasl");
        std::fs::create_dir_all(&default).unwrap();
        let stores = secrets_for(&default, Some(&default), open_from(&tokens, &vaults));
        assert_eq!(Some("refresh-real".to_string()), stores.secrets.get(ME));
        assert_eq!(
            Some("real-device-seed".to_string()),
            stores.secrets.get(KEY_ACCOUNT)
        );
        assert_eq!(
            Some("real-vault-key".to_string()),
            stores.vault_keys.get("vault-1")
        );
    }

    #[test]
    fn each_home_has_its_own_stable_scope() {
        let dir = tempfile::tempdir().unwrap();
        let default = dir.path().join("Wasl");
        let a = home_scope(&dir.path().join("a"), Some(&default));
        let b = home_scope(&dir.path().join("b"), Some(&default));
        assert!(a.is_some() && b.is_some());
        assert_ne!(a, b);
        assert_eq!(a, home_scope(&dir.path().join("a"), Some(&default)));
        // With no default home known, every home is scoped: never the installed app's entries.
        assert!(home_scope(&default, None).is_some());
    }
}
