//! `wasl vault decrypt` subprocess delegation to `wasld vault decrypt`.
//! End-to-end cryptographic recovery tests run in `wasld/tests/vault_cli.rs`.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn run_cli(args: &[&str], stdin: &str, wasld_bin: Option<&Path>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wasl"));
    cmd.args(["vault", "decrypt"])
        .args(args)
        .env("WASL_LANG", "en")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match wasld_bin {
        Some(p) => {
            cmd.env("WASLD_BIN", p);
        }
        None => {
            cmd.env("WASLD_BIN", "");
        }
    }
    let mut child = cmd.spawn().unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn missing_wasld_reports_not_running() {
    let o = run_cli(&["vault-dir", "out-dir", "--passphrase-stdin"], "pw\n", None);
    assert_eq!(Some(3), o.status.code());
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(stderr.contains("wasld"), "{stderr}");
}

#[test]
fn delegates_arguments_stdin_and_exit_code_to_wasld() {
    let tmp = tempfile::tempdir().unwrap();
    let capture = tmp.path().join("captured.txt");

    #[cfg(windows)]
    let shim = {
        let p = tmp.path().join("fake-wasld.cmd");
        let body = format!(
            "@echo off\r\n(\r\n  echo ARGS=%*\r\n  set /p PASS=\r\n  call echo STDIN=%%PASS%%\r\n) > \"{}\"\r\necho from-fake-wasld\r\nexit /b 5\r\n",
            capture.display()
        );
        std::fs::write(&p, body).unwrap();
        p
    };

    #[cfg(unix)]
    let shim = {
        use std::os::unix::fs::PermissionsExt;
        let p = tmp.path().join("fake-wasld.sh");
        let body = format!(
            "#!/bin/sh\nPASS=$(head -n 1)\nprintf 'ARGS=%s\\nSTDIN=%s\\n' \"$*\" \"$PASS\" > \"{}\"\necho from-fake-wasld\nexit 5\n",
            capture.display()
        );
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    };

    let o = run_cli(
        &["my-vault", "my-out", "--passphrase-stdin", "--overwrite"],
        "correct-horse\n",
        Some(&shim),
    );
    assert_eq!(Some(5), o.status.code(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(
        String::from_utf8_lossy(&o.stdout).contains("from-fake-wasld"),
        "{}",
        String::from_utf8_lossy(&o.stdout)
    );
    let recorded = std::fs::read_to_string(&capture).unwrap();
    assert!(
        recorded.contains("vault decrypt my-vault my-out --passphrase-stdin --overwrite"),
        "{recorded}"
    );
    assert!(recorded.contains("STDIN=correct-horse"), "{recorded}");
}
