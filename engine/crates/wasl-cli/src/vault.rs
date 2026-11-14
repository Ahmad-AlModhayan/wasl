//! Vault passphrase prompts and `wasl vault decrypt` delegation to offline `wasld`.
//!
//! Cryptographic recovery lives in AGPL-licensed `wasld`/`wasl-crypto` across this process
//! boundary so `wasl-cli` stays strictly Apache-2.0.

use crate::cmd::{Fail, GENERAL, NOT_RUNNING, USAGE};
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use wasl_cli::{service, t};
use zeroize::Zeroizing;

fn resolve_daemon() -> Result<PathBuf, Fail> {
    if let Some(override_bin) = std::env::var_os("WASLD_BIN") {
        if override_bin.is_empty() {
            return Err(Fail::new(NOT_RUNNING, t!(DaemonNotFound)));
        }
        return Ok(PathBuf::from(override_bin));
    }
    service::find_daemon().ok_or_else(|| Fail::new(NOT_RUNNING, t!(DaemonNotFound)))
}

pub fn decrypt(folder: &Path, output: &Path, passphrase_stdin: bool, overwrite: bool) -> Result<u8, Fail> {
    let daemon = resolve_daemon()?;
    let mut cmd = Command::new(&daemon);
    cmd.arg("vault").arg("decrypt").arg(folder).arg(output);
    if passphrase_stdin {
        cmd.arg("--passphrase-stdin");
    }
    if overwrite {
        cmd.arg("--overwrite");
    }
    let status = cmd
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| Fail::new(GENERAL, t!(DaemonSpawnFailed, daemon.display(), e)))?;
    Ok(status
        .code()
        .and_then(|c| u8::try_from(c).ok())
        .unwrap_or(GENERAL))
}

pub fn read_passphrase(from_stdin: bool) -> Result<Zeroizing<String>, Fail> {
    read_passphrase_as(from_stdin, &t!(VaultPassPrompt))
}

pub fn read_new_passphrase(from_stdin: bool) -> Result<Zeroizing<String>, Fail> {
    let first = read_passphrase_as(from_stdin, &t!(VaultPassNew))?;
    if !from_stdin {
        let again = read_passphrase_as(false, &t!(VaultPassRepeat))?;
        if *first != *again {
            return Err(Fail::new(USAGE, t!(VaultPassMismatch)));
        }
    }
    Ok(first)
}

fn read_passphrase_as(from_stdin: bool, prompt: &str) -> Result<Zeroizing<String>, Fail> {
    if from_stdin {
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|e| Fail::new(GENERAL, e.to_string()))?;
        while line.ends_with(['\n', '\r']) {
            line.pop();
        }
        return Ok(Zeroizing::new(line));
    }
    if !std::io::stdin().is_terminal() {
        return Err(Fail::new(USAGE, t!(VaultNoTerminal)));
    }
    let _ = std::io::stderr().flush();
    rpassword::prompt_password(prompt)
        .map(Zeroizing::new)
        .map_err(|e| Fail::new(GENERAL, e.to_string()))
}
