//! Where the daemon keeps its data and where clients find it. Everything lives in the user's own
//! profile, so other OS users cannot read it.

use std::io;
use std::path::PathBuf;

/// The daemon's folders. Tests and portable installs point them elsewhere with `WASL_HOME`.
#[derive(Debug, Clone)]
pub struct Paths {
    /// Database, bin, staging and the IPC token.
    pub data: PathBuf,
    /// Where the Unix socket lives (unused on Windows).
    pub runtime: PathBuf,
}

impl Paths {
    pub fn for_user() -> io::Result<Paths> {
        if let Some(home) = std::env::var_os("WASL_HOME") {
            let home = PathBuf::from(home);
            return Ok(Paths {
                data: home.clone(),
                runtime: home,
            });
        }
        Paths::user_default()
    }

    /// This user's own folders, whatever `WASL_HOME` says: the real install's data, which a test
    /// build must never open.
    pub fn user_default() -> io::Result<Paths> {
        let data = dirs::data_local_dir()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no local data folder for this user"))?
            .join(if cfg!(target_os = "linux") { "wasl" } else { "Wasl" });
        let runtime = dirs::runtime_dir()
            .map(|d| d.join("wasl"))
            .unwrap_or_else(|| data.clone());
        Ok(Paths { data, runtime })
    }

    pub fn at(dir: impl Into<PathBuf>) -> Paths {
        let d = dir.into();
        Paths {
            data: d.clone(),
            runtime: d,
        }
    }

    pub fn db(&self) -> PathBuf {
        self.data.join("wasl.db")
    }

    pub fn bin(&self) -> PathBuf {
        self.data.join("bin")
    }

    pub fn staging(&self) -> PathBuf {
        self.data.join("staging")
    }

    pub fn log_dir(&self) -> PathBuf {
        self.data.join("logs")
    }

    /// The secret both sides prove they know (see `auth`). Readable by this user only.
    pub fn token(&self) -> PathBuf {
        self.data.join("ipc.token")
    }

    #[cfg(unix)]
    pub fn socket(&self) -> PathBuf {
        self.runtime.join("wasld.sock")
    }

    /// One pipe per user and data folder, so two portable installs never meet.
    #[cfg(windows)]
    pub fn pipe_name(&self) -> String {
        use sha2::{Digest, Sha256};
        let h = Sha256::digest(self.data.to_string_lossy().as_bytes());
        let user = std::env::var("USERNAME").unwrap_or_default();
        let user: String = user
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(32)
            .collect();
        format!(
            r"\\.\pipe\wasl-{user}-{:02x}{:02x}{:02x}{:02x}",
            h[0], h[1], h[2], h[3]
        )
    }

    /// Creates the folders, readable by this user only. On Windows the profile folder already is,
    /// but a portable install (`WASL_HOME`, `--home`) may sit anywhere, such as a shared folder
    /// whose access list lets every user read the token (security review 2026-10).
    pub fn ensure(&self) -> io::Result<()> {
        self.ensure_checked(|_| Err(io::Error::other("ACL volume capability check not configured")))
    }

    pub fn ensure_checked<F>(&self, persists_acls: F) -> io::Result<()>
    where
        F: Fn(&std::path::Path) -> io::Result<bool>,
    {
        #[cfg(not(windows))]
        let _ = &persists_acls;
        for d in [&self.data, &self.runtime] {
            std::fs::create_dir_all(d)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        #[cfg(windows)]
        {
            user_only_checked(&self.data, true, &persists_acls)?;
            if self.runtime != self.data {
                user_only_checked(&self.runtime, true, &persists_acls)?;
            }
        }
        Ok(())
    }
}

/// Gives `path` a protected access list (nothing inherited from its parent) that grants full
/// control to the current user and SYSTEM only; for a folder, inherited by what it holds.
///
/// Done with `icacls` (by full path, not through `PATH`) because the Win32 security calls are
/// unsafe and this crate forbids unsafe code; the daemon does this a few times per start only.
///
/// If `icacls` fails, it fails closed (security review S3-3): on a volume that keeps access lists
/// (NTFS, ReFS, a share) the folder may be readable by others, so the daemon must not put its
/// token there. Only a drive without access lists (FAT, exFAT), which cannot be protected at all,
/// gets a warning instead: anyone holding such a drive reads all of it anyway.
#[cfg(windows)]
pub(crate) fn user_only(path: &std::path::Path, folder: bool) -> io::Result<()> {
    user_only_checked(path, folder, &|_| {
        Err(io::Error::other("ACL volume capability check not configured"))
    })
}

#[cfg(windows)]
fn user_only_checked(
    path: &std::path::Path,
    folder: bool,
    persists_acls: &dyn Fn(&std::path::Path) -> io::Result<bool>,
) -> io::Result<()> {
    use std::process::{Command, Stdio};
    let system = system32();
    let sid = current_user_sid(&system)?;
    let inherit = if folder { "(OI)(CI)" } else { "" };
    let out = Command::new(system.join("icacls.exe"))
        .arg(path)
        .args(["/inheritance:r", "/grant:r"])
        .arg(format!("*{sid}:{inherit}F"))
        .arg(format!("*S-1-5-18:{inherit}F"))
        // Explicit entries a copied or hand-made folder may carry: Everyone, Users,
        // Authenticated Users.
        .args(["/remove:g", "*S-1-1-0", "*S-1-5-32-545", "*S-1-5-11", "/Q"])
        .stdin(Stdio::null())
        .output()?;
    if out.status.success() {
        return Ok(());
    }
    let why = String::from_utf8_lossy(if out.stderr.is_empty() {
        &out.stdout
    } else {
        &out.stderr
    })
    .trim()
    .to_owned();
    after_icacls_failed(path, &why, persists_acls(path))
}

/// What an `icacls` failure means, given whether the volume keeps access lists (or why that
/// could not be told).
#[cfg(windows)]
fn after_icacls_failed(path: &std::path::Path, why: &str, acls: io::Result<bool>) -> io::Result<()> {
    match acls {
        Ok(false) => {
            eprintln!(
                "wasl: {} is on a drive without access lists (FAT or exFAT), so it cannot be limited \
                 to this user; anyone with the drive can read it",
                path.display()
            );
            Ok(())
        }
        // Access lists exist (or it cannot be told): an unprotected folder is not safe to use.
        Ok(true) => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "could not limit {} to this user ({why}); Wasl will not keep its data there",
                path.display()
            ),
        )),
        Err(e) => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "could not limit {} to this user ({why}), nor tell whether its drive keeps access \
                 lists ({e}); Wasl will not keep its data there",
                path.display()
            ),
        )),
    }
}

#[cfg(windows)]
fn system32() -> PathBuf {
    std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("System32")
}

/// The current user's SID, from `whoami /user /fo csv /nh`: `"machine\user","S-1-5-21-…"`.
#[cfg(windows)]
fn current_user_sid(system: &std::path::Path) -> io::Result<String> {
    let out = std::process::Command::new(system.join("whoami.exe"))
        .args(["/user", "/fo", "csv", "/nh"])
        .stdin(std::process::Stdio::null())
        .output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.trim()
        .rsplit(',')
        .next()
        .map(|s| s.trim_matches('"').to_owned())
        .filter(|s| s.starts_with("S-1-") && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .ok_or_else(|| io::Error::other("cannot tell which Windows user this is"))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn a_failed_access_list_is_fatal_where_the_drive_keeps_them() {
        let p = std::path::Path::new(r"C:\data");
        let e = after_icacls_failed(p, "Access is denied.", Ok(true)).unwrap_err();
        assert_eq!(io::ErrorKind::PermissionDenied, e.kind());
        assert!(e.to_string().contains("Access is denied."), "{e}");
        let e = after_icacls_failed(p, "x", Err(io::Error::other("no volume"))).unwrap_err();
        assert_eq!(io::ErrorKind::PermissionDenied, e.kind(), "unknown: closed too");
        // FAT and exFAT have no access lists to set: a warning, not a failure.
        after_icacls_failed(p, "x", Ok(false)).unwrap();
    }

    #[test]
    fn a_folder_on_ntfs_is_limited_to_this_user() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().join("data");
        std::fs::create_dir_all(&d).unwrap();
        user_only(&d, true).unwrap();
        let out = std::process::Command::new(system32().join("icacls.exe"))
            .arg(&d)
            .output()
            .unwrap();
        let acl = String::from_utf8_lossy(&out.stdout);
        // Nothing inherited from the temp folder, only the entries for this user and SYSTEM
        // (account names are localized, so the inherited marker is what is checked).
        assert!(out.status.success() && !acl.contains("(I)"), "{acl}");
        // icacls failing on an NTFS folder (here: it does not exist) now fails closed.
        assert!(user_only(&dir.path().join("missing"), true).is_err());
    }
}
