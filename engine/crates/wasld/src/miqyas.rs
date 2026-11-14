//! The Miqyas test build (feature `miqyas`, off in every release build; docs/testing/MIQYAS.md).
//! A `wasld` built with it serves only a test folder: it refuses to start without `--home` or
//! `WASL_HOME`, refuses this user's real Wasl folder, keeps its secrets in a file in that folder
//! instead of the OS keychain, listens on loopback with LAN discovery off, mounts no views, and
//! sends nothing to Google (Drive and sign-in point at a dead loopback port unless
//! `WASL_MIQYAS_DRIVE_URL` names a fake Drive).

use crate::daemon::DaemonConfig;
use crate::oauth::OAuthConfig;
use crate::tokens::Secrets;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wasl_ipc::Paths;

/// Shown by `wasld --version`, so a test build is never taken for a release.
pub const BUILD: &str = "miqyas test build";

/// The device's listen address (default `127.0.0.1:0`, any free port).
pub const LISTEN_VAR: &str = "WASL_MIQYAS_LISTEN";
/// The device's name in its group (default "Miqyas").
pub const NAME_VAR: &str = "WASL_MIQYAS_NAME";
/// A fake Drive's base URL (default: nothing listens there).
pub const DRIVE_VAR: &str = "WASL_MIQYAS_DRIVE_URL";

/// The test folder, or why this build won't serve it.
pub fn home(flag: Option<String>) -> Result<Paths, String> {
    let home = flag
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("WASL_HOME").map(PathBuf::from))
        .ok_or("this is a Miqyas test build: start it with --home DIR or WASL_HOME set to a test folder")?;
    let real = Paths::user_default().map_err(|e| e.to_string())?;
    for own in [&real.data, &real.runtime] {
        if near(&home, own) {
            return Err(format!(
                "this is a Miqyas test build: it never opens Wasl's own folder ({})",
                own.display()
            ));
        }
    }
    Ok(Paths::at(home))
}

/// One folder is the other or holds it, compared as the system resolves them.
fn near(a: &Path, b: &Path) -> bool {
    let resolve = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let (a, b) = (resolve(a), resolve(b));
    a.starts_with(&b) || b.starts_with(&a)
}

/// The daemon's settings for the test folder at `paths`.
pub fn config(paths: &Paths) -> Result<DaemonConfig, String> {
    let listen = match std::env::var(LISTEN_VAR) {
        Ok(v) => v
            .parse()
            .map_err(|_| format!("{LISTEN_VAR}: not an address: {v}"))?,
        Err(_) => std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
    };
    // Port 9 (discard): nothing answers, so no request leaves the machine.
    let drive = std::env::var(DRIVE_VAR).unwrap_or_else(|_| "http://127.0.0.1:9".into());
    let drive = drive.trim_end_matches('/');
    Ok(DaemonConfig {
        data_dir: paths.data.clone(),
        oauth: OAuthConfig {
            client_id: "miqyas".into(),
            client_secret: "miqyas".into(),
            auth_url: format!("{drive}/auth"),
            token_url: format!("{drive}/token"),
            userinfo_url: format!("{drive}/userinfo"),
            scopes: vec![],
        },
        secrets: Arc::new(FileSecrets::open(paths.data.join("miqyas-secrets.json"))),
        vault_keys: Arc::new(FileSecrets::open(paths.data.join("miqyas-vault-keys.json"))),
        drive_endpoints: Some(wasl_drive::DriveEndpoints {
            base: format!("{drive}/drive/v3"),
            upload: format!("{drive}/upload/drive/v3"),
        }),
        open_browser: false,
        device_name: std::env::var(NAME_VAR).unwrap_or_else(|_| "Miqyas".into()),
        sign_in_timeout: Duration::from_secs(5),
        devices: Some(wasl_devices::DeviceConfig {
            listen,
            discover: false,
        }),
        // macOS: the File Provider socket sits in Wasl.app's group container, shared with the
        // real install; a test daemon must not take it.
        view_backend: Some(Arc::new(crate::views::Unsupported)),
        view_refresh: crate::views::REFRESH_EVERY,
    })
}

/// Secrets in a JSON file readable by this user only: the test daemon's device key and tokens,
/// apart from the real install's keychain entries.
pub struct FileSecrets {
    path: PathBuf,
    map: Mutex<BTreeMap<String, String>>,
}

impl FileSecrets {
    pub fn open(path: PathBuf) -> FileSecrets {
        let map = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        FileSecrets {
            path,
            map: Mutex::new(map),
        }
    }

    fn save(&self, map: &BTreeMap<String, String>) -> Result<(), String> {
        let tmp = self.path.with_extension("tmp");
        let bytes = serde_json::to_vec(map).map_err(|e| e.to_string())?;
        write_private(&tmp, &bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

impl Secrets for FileSecrets {
    fn get(&self, account: &str) -> Option<String> {
        self.map
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(account)
            .cloned()
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        let mut map = self.map.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(account.to_owned(), secret.to_owned());
        self.save(&map)
    }

    fn remove(&self, account: &str) {
        let mut map = self.map.lock().unwrap_or_else(|p| p.into_inner());
        if map.remove(account).is_some() {
            let _ = self.save(&map);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_real_wasl_folder_and_no_folder_are_refused() {
        assert!(home(None).is_err() || std::env::var_os("WASL_HOME").is_some());
        let real = Paths::user_default().unwrap();
        assert!(home(Some(real.data.display().to_string())).is_err());
        assert!(home(Some(real.data.join("x").display().to_string())).is_err());
        let dir = tempfile::tempdir().unwrap();
        assert!(home(Some(dir.path().display().to_string())).is_ok());
    }

    #[test]
    fn secrets_survive_a_restart_in_their_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.json");
        let s = FileSecrets::open(path.clone());
        s.set("device-key", "abc").unwrap();
        s.set("other", "x").unwrap();
        s.remove("other");
        let again = FileSecrets::open(path);
        assert_eq!(Some("abc".into()), again.get("device-key"));
        assert_eq!(None, again.get("other"));
    }
}
