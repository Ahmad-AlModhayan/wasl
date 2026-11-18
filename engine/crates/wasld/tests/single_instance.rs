//! wasl#21: a second engine on one data folder refuses to open.

use std::sync::Arc;
use std::time::Duration;
use wasl_drive::DriveEndpoints;
use wasld::daemon::{Daemon, DaemonConfig};
use wasld::oauth::OAuthConfig;
use wasld::tokens::MemorySecrets;

fn config(data: std::path::PathBuf) -> DaemonConfig {
    let uri = "http://127.0.0.1:9";
    DaemonConfig {
        data_dir: data,
        oauth: OAuthConfig {
            client_id: "test".into(),
            client_secret: "test".into(),
            auth_url: format!("{uri}/auth"),
            token_url: format!("{uri}/token"),
            userinfo_url: format!("{uri}/userinfo"),
            scopes: vec![],
        },
        secrets: Arc::new(MemorySecrets::default()),
        vault_keys: Arc::new(MemorySecrets::default()),
        drive_endpoints: Some(DriveEndpoints {
            base: format!("{uri}/drive/v3"),
            upload: format!("{uri}/upload/drive/v3"),
        }),
        open_browser: false,
        device_name: "Test".into(),
        sign_in_timeout: Duration::from_secs(1),
        devices: None,
        view_backend: None,
        view_refresh: Duration::from_secs(300),
    }
}

/// The real `wasld` binary, started on a folder another engine has open, exits with the reason
/// and the first engine keeps working.
#[test]
fn a_second_wasld_on_the_same_home_exits_and_the_first_keeps_running() {
    let dir = tempfile::tempdir().unwrap();
    let first = Daemon::open(config(dir.path().to_path_buf())).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_wasld"))
        .arg("--home")
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("another Wasl is already running"), "{err}");
    assert_eq!(13, first.store.lock().schema_version().unwrap());
}

#[test]
fn two_daemons_in_one_process_on_one_folder_the_second_fails_and_on_two_folders_both_open() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let _one = Daemon::open(config(a.path().to_path_buf())).unwrap();
    let err = Daemon::open(config(a.path().to_path_buf()))
        .err()
        .expect("refused");
    assert!(err.contains("another Wasl is already running"), "{err}");
    let _two = Daemon::open(config(b.path().to_path_buf())).unwrap();
}
