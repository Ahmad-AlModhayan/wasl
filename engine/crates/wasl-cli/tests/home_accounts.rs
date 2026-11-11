//! #111: the CLI against the real daemon, both started with the same fresh `WASL_HOME`.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use wasl_ipc::{Client, Paths};
use wasld::daemon::DaemonConfig;
use wasld::oauth::OAuthConfig;
use wasld::schedule::Timing;
use wasld::tokens::{MemorySecrets, Secrets};

/// Runs `wasl` with `WASL_HOME` set to `home` and returns its JSON output.
async fn wasl_json(home: &Path, args: &[&str]) -> serde_json::Value {
    let home = home.to_owned();
    let args: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
    tokio::task::spawn_blocking(move || {
        let out = Command::new(env!("CARGO_BIN_EXE_wasl"))
            .args(&args)
            .env("WASL_HOME", &home)
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{args:?}: {e}: {stdout}"))
    })
    .await
    .unwrap()
}

/// The smoke run of #111: the user's real token sits in the shared secret store, the daemon has a
/// fresh home. `status --json` and `account list --json` must both report no account.
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
    for _ in 0..100 {
        if Client::connect(&paths).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let status = wasl_json(&paths.data, &["status", "--json"]).await;
    let listed = wasl_json(&paths.data, &["account", "list", "--json"]).await;
    assert_eq!(serde_json::json!([]), status["accounts"], "{status}");
    assert_eq!(status["accounts"], listed);
}
