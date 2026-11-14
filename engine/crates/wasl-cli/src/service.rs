//! Starting `wasld` at login (SRS FR-B2), per user and without admin rights:
//! - Windows: a value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, written by `reg.exe`;
//! - macOS: a LaunchAgent in `~/Library/LaunchAgents`, loaded with `launchctl bootstrap`;
//! - Linux: a systemd user unit in `~/.config/systemd/user`, enabled with `systemctl --user`.
//!
//! The text of each (registry arguments, plist, unit) is built by pure functions so it can be
//! tested without installing anything. Shared by the CLI and the tray app.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The registry key holding per-user login items on Windows.
pub const WINDOWS_RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
/// The value name under [`WINDOWS_RUN_KEY`].
pub const WINDOWS_VALUE: &str = "Wasl";
/// The launchd label on macOS.
pub const LAUNCHD_LABEL: &str = "com.kahflabs.wasld";
/// The systemd user unit on Linux.
pub const SYSTEMD_UNIT: &str = "wasld.service";

/// The daemon's file name on this OS.
pub fn daemon_file_name() -> &'static str {
    if cfg!(windows) { "wasld.exe" } else { "wasld" }
}

/// Finds `wasld`: next to the running executable first (installed together), then on `PATH`.
pub fn find_daemon() -> Option<PathBuf> {
    let name = daemon_file_name();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// Whether login start is set up, as far as this OS tells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceState {
    Installed {
        /// The registry value, plist or unit file.
        location: String,
    },
    NotInstalled,
}

// ------------------------------------------------------------------- the generated text

/// `reg.exe` arguments that add the Run value. The data is the quoted path, so a path with spaces
/// still starts the right program.
pub fn windows_add_args(daemon: &Path) -> Vec<String> {
    vec![
        "add".into(),
        WINDOWS_RUN_KEY.into(),
        "/v".into(),
        WINDOWS_VALUE.into(),
        "/t".into(),
        "REG_SZ".into(),
        "/d".into(),
        format!("\"{}\"", daemon.display()),
        "/f".into(),
    ]
}

/// `reg.exe` arguments that remove the Run value.
pub fn windows_delete_args() -> Vec<String> {
    vec![
        "delete".into(),
        WINDOWS_RUN_KEY.into(),
        "/v".into(),
        WINDOWS_VALUE.into(),
        "/f".into(),
    ]
}

/// `reg.exe` arguments that read the Run value.
pub fn windows_query_args() -> Vec<String> {
    vec![
        "query".into(),
        WINDOWS_RUN_KEY.into(),
        "/v".into(),
        WINDOWS_VALUE.into(),
    ]
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The LaunchAgent: started at login, not restarted when it quits on purpose.
pub fn launchd_plist(daemon: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LAUNCHD_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
    <key>ProcessType</key>
    <string>Background</string>
</dict>
</plist>
"#,
        xml_escape(&daemon.display().to_string())
    )
}

/// The systemd user unit: started with the session, restarted after a crash only.
pub fn systemd_unit(daemon: &Path) -> String {
    // systemd expands `%` specifiers and splits on spaces: escape the one, quote the other.
    let exec = daemon
        .display()
        .to_string()
        .replace('%', "%%")
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!(
        "[Unit]
Description=Wasl sync daemon
After=network-online.target

[Service]
Type=simple
ExecStart=\"{exec}\"
Restart=on-failure
RestartSec=10

[Install]
WantedBy=default.target
"
    )
}

/// Where the plist goes on macOS.
pub fn launchd_plist_path() -> io::Result<PathBuf> {
    Ok(home()?
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist")))
}

/// Where the unit goes on Linux.
pub fn systemd_unit_path() -> io::Result<PathBuf> {
    let config = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(c) if !c.is_empty() => PathBuf::from(c),
        _ => home()?.join(".config"),
    };
    Ok(config.join("systemd").join("user").join(SYSTEMD_UNIT))
}

fn home() -> io::Result<PathBuf> {
    dirs::home_dir().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no home folder for this user"))
}

// ------------------------------------------------------------------- doing it

fn run(program: &str, args: &[String]) -> io::Result<()> {
    let out = Command::new(program).args(args).output()?;
    if out.status.success() {
        Ok(())
    } else {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        Err(io::Error::other(if msg.is_empty() {
            format!("{program} failed ({})", out.status)
        } else {
            msg
        }))
    }
}

#[cfg(target_os = "macos")]
fn gui_domain() -> io::Result<String> {
    let out = Command::new("id").arg("-u").output()?;
    let uid = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if uid.is_empty() {
        return Err(io::Error::other("could not read this user's id"));
    }
    Ok(format!("gui/{uid}"))
}

/// Starts `daemon` at every login of this user from now on.
pub fn install(daemon: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        run("reg", &windows_add_args(daemon))
    }
    #[cfg(target_os = "macos")]
    {
        let path = launchd_plist_path()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, launchd_plist(daemon))?;
        let domain = gui_domain()?;
        // Replacing an older registration: unload it first (fails harmlessly when absent).
        let _ = run(
            "launchctl",
            &["bootout".into(), format!("{domain}/{LAUNCHD_LABEL}")],
        );
        run(
            "launchctl",
            &["bootstrap".into(), domain, path.display().to_string()],
        )
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let path = systemd_unit_path()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, systemd_unit(daemon))?;
        run("systemctl", &["--user".into(), "daemon-reload".into()])?;
        run(
            "systemctl",
            &[
                "--user".into(),
                "enable".into(),
                "--now".into(),
                SYSTEMD_UNIT.into(),
            ],
        )
    }
}

/// Stops starting `wasld` at login. Not an error when it was not set up.
pub fn uninstall() -> io::Result<()> {
    if status()? == ServiceState::NotInstalled {
        return Ok(());
    }
    #[cfg(windows)]
    {
        run("reg", &windows_delete_args())
    }
    #[cfg(target_os = "macos")]
    {
        let path = launchd_plist_path()?;
        let _ = run(
            "launchctl",
            &["bootout".into(), format!("{}/{LAUNCHD_LABEL}", gui_domain()?)],
        );
        std::fs::remove_file(path)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let path = systemd_unit_path()?;
        let _ = run(
            "systemctl",
            &[
                "--user".into(),
                "disable".into(),
                "--now".into(),
                SYSTEMD_UNIT.into(),
            ],
        );
        std::fs::remove_file(path)?;
        let _ = run("systemctl", &["--user".into(), "daemon-reload".into()]);
        Ok(())
    }
}

/// Whether login start is set up.
pub fn status() -> io::Result<ServiceState> {
    #[cfg(windows)]
    {
        let out = Command::new("reg").args(windows_query_args()).output()?;
        Ok(if out.status.success() {
            ServiceState::Installed {
                location: format!(r"{WINDOWS_RUN_KEY}\{WINDOWS_VALUE}"),
            }
        } else {
            ServiceState::NotInstalled
        })
    }
    #[cfg(target_os = "macos")]
    {
        let path = launchd_plist_path()?;
        Ok(if path.is_file() {
            ServiceState::Installed {
                location: path.display().to_string(),
            }
        } else {
            ServiceState::NotInstalled
        })
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let path = systemd_unit_path()?;
        Ok(if path.is_file() {
            ServiceState::Installed {
                location: path.display().to_string(),
            }
        } else {
            ServiceState::NotInstalled
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_value_quotes_the_path() {
        let args = windows_add_args(Path::new(r"C:\Program Files\Wasl\wasld.exe"));
        assert_eq!(
            vec![
                "add",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "Wasl",
                "/t",
                "REG_SZ",
                "/d",
                r#""C:\Program Files\Wasl\wasld.exe""#,
                "/f"
            ],
            args
        );
        assert_eq!(
            vec![
                "delete",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "Wasl",
                "/f"
            ],
            windows_delete_args()
        );
    }

    #[test]
    fn the_launch_agent_runs_at_load_and_is_not_kept_alive() {
        let plist = launchd_plist(Path::new("/Applications/Wasl & Co.app/Contents/MacOS/wasld"));
        assert!(plist.contains("<string>com.kahflabs.wasld</string>"));
        assert!(plist.contains("<key>RunAtLoad</key>\n    <true/>"));
        assert!(plist.contains("<key>KeepAlive</key>\n    <false/>"));
        assert!(plist.contains("<string>/Applications/Wasl &amp; Co.app/Contents/MacOS/wasld</string>"));
    }

    #[test]
    fn the_systemd_unit_restarts_on_failure_and_quotes_the_path() {
        let unit = systemd_unit(Path::new("/home/a b/.local/bin/wasld%1"));
        assert!(unit.contains("ExecStart=\"/home/a b/.local/bin/wasld%%1\"\n"));
        assert!(unit.contains("Restart=on-failure\n"));
        assert!(unit.contains("WantedBy=default.target\n"));
        assert!(unit.starts_with("[Unit]\n"));
    }

    #[test]
    fn the_unit_and_plist_live_in_the_users_profile() {
        if let (Ok(p), Some(home)) = (launchd_plist_path(), dirs::home_dir()) {
            assert!(p.starts_with(&home));
            assert!(p.ends_with("Library/LaunchAgents/com.kahflabs.wasld.plist"));
        }
        let unit = systemd_unit_path().unwrap();
        assert!(unit.ends_with("systemd/user/wasld.service"));
    }
}
