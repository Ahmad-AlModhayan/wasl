//! #111: the daemon on a fresh `WASL_HOME` when the shared secret store already holds a token.

use std::sync::Arc;
use std::time::Duration;
use wasl_ipc::{Client, Paths};
use wasld::daemon::DaemonConfig;
use wasld::oauth::OAuthConfig;
use wasld::schedule::Timing;
use wasld::tokens::{MemorySecrets, Secrets};

/// The smoke run of #111: the user's real token sits in the shared secret store, the daemon has a
/// fresh home. `Status` and `Accounts` over IPC must both report no accounts.
#[tokio::test(flavor = "multi_thread")]
async fn a_fresh_home_lists_the_same_no_accounts_in_status_and_the_account_list() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::at(dir.path().join("home"));
    let secrets = Arc::new(MemorySecrets::default());
    secrets.set("me@example.com", "refresh-real").unwrap();
    // Nothing here is reached: a home with no account never signs in or calls Drive.
    let unused = "http://127.0.0.1:9";
    let cfg = DaemonConfig {
        data_dir: paths.data.clone(),
        oauth: OAuthConfig {
            client_id: "test-client".into(),
            client_secret: "test-secret".into(),
            auth_url: format!("{unused}/auth"),
            token_url: format!("{unused}/token"),
            userinfo_url: format!("{unused}/userinfo"),
            scopes: vec!["openid".into(), "email".into()],
        },
        secrets,
        vault_keys: Arc::new(MemorySecrets::default()),
        drive_endpoints: None,
        open_browser: false,
        device_name: "Test PC".into(),
        sign_in_timeout: Duration::from_secs(10),
        devices: None,
        view_backend: None,
        view_refresh: Duration::from_secs(300),
    };
    let p = paths.clone();
    tokio::spawn(async move { wasld::serve(&p, cfg, Timing::default()).await });
    let client = loop {
        if let Ok(c) = Client::connect(&paths).await {
            break c;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    let status = client.status().await.unwrap();
    let listed = client.accounts().await.unwrap();
    assert!(status.accounts.is_empty(), "{status:?}");
    assert_eq!(status.accounts, listed);
}
