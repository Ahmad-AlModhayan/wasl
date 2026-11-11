//! `wasl vault decrypt` (SRS FR-V3): recover a downloaded vault folder with only the passphrase.
//! The work is `wasl_crypto::recover`; this is prompts and messages. `wasl vault create`,
//! `unlock` and `lock` ask wasld (see `cmd.rs`) and read the passphrase here.

use crate::cmd::{AUTH, Fail, GENERAL, NOT_FOUND, OK, USAGE};
use crate::out;
use std::io::{BufRead, IsTerminal};
use std::path::Path;
use wasl_cli::t;
use wasl_crypto::recover::{Event, Reason, Vault, decrypt_tree};
use zeroize::Zeroizing;

pub fn decrypt(folder: &Path, output: &Path, passphrase_stdin: bool, overwrite: bool) -> Result<u8, Fail> {
    if !folder.is_dir() {
        return Err(Fail::new(NOT_FOUND, t!(NoSuchFolder, folder.display())));
    }
    let vault = Vault::load(folder).map_err(|e| match e {
        wasl_crypto::CryptoError::Io(io) if io.kind() == std::io::ErrorKind::NotFound => {
            Fail::new(NOT_FOUND, t!(VaultNoManifest, folder.display()))
        }
        e => Fail::new(GENERAL, t!(VaultBadManifest, e)),
    })?;
    println!(
        "{}",
        t!(VaultInfo, vault.name(), vault.format_label(), vault.created())
    );

    let passphrase = read_passphrase(passphrase_stdin)?;
    if passphrase.is_empty() {
        return Err(Fail::new(USAGE, t!(VaultPassEmpty)));
    }
    let keys = vault
        .unlock(&passphrase)
        .map_err(|e| Fail::new(GENERAL, t!(VaultBadManifest, e)))?
        .ok_or_else(|| Fail::new(AUTH, t!(VaultWrongPass)))?;
    drop(passphrase);
    println!("{}", t!(VaultPassOk));
    println!("{}", t!(VaultDecrypting, output.display()));

    let summary = decrypt_tree(folder, output, &keys, overwrite, |ev| match ev {
        Event::File(path, _) => println!("{}", t!(VaultFileOk, path)),
        Event::Failed(path, why) => eprintln!("{}", t!(VaultFileFailed, path, reason(why))),
        Event::Folder(_) | Event::Skipped(..) => {}
    })
    .map_err(|e| Fail::new(GENERAL, t!(ErrorLine, e)))?;

    println!();
    println!(
        "{}",
        t!(
            VaultSummary,
            summary.folders,
            summary.files,
            out::size(summary.bytes)
        )
    );
    if !summary.skipped.is_empty() {
        println!("{}", t!(VaultSkippedHeader, summary.skipped.len()));
        for (path, why) in &summary.skipped {
            println!("{}", t!(VaultItem, path, reason(why)));
        }
    }
    if summary.failed.is_empty() {
        return Ok(OK);
    }
    println!("{}", t!(VaultFailedHeader, summary.failed.len()));
    for (path, why) in &summary.failed {
        println!("{}", t!(VaultItem, path, reason(why)));
    }
    Ok(GENERAL)
}

/// The passphrase, from standard input's first line or typed without echo. Never an argument.
pub fn read_passphrase(from_stdin: bool) -> Result<Zeroizing<String>, Fail> {
    read_passphrase_as(from_stdin, &t!(VaultPassPrompt))
}

/// A new vault's passphrase: typed twice without echo (or standard input's first line).
pub fn read_new_passphrase(from_stdin: bool) -> Result<Zeroizing<String>, Fail> {
    let first = read_passphrase_as(from_stdin, &t!(VaultPassNew))?;
    if !from_stdin {
        let again = read_passphrase_as(false, &t!(VaultPassRepeat))?;
        if *again != *first {
            return Err(Fail::new(USAGE, t!(VaultPassMismatch)));
        }
    }
    Ok(first)
}

fn read_passphrase_as(from_stdin: bool, prompt: &str) -> Result<Zeroizing<String>, Fail> {
    if from_stdin {
        let mut line = Zeroizing::new(String::new());
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|e| Fail::new(GENERAL, e.to_string()))?;
        let trimmed = line.trim_end_matches(['\r', '\n']).len();
        line.truncate(trimmed);
        return Ok(line);
    }
    if !std::io::stdin().is_terminal() {
        return Err(Fail::new(USAGE, t!(VaultNoTerminal)));
    }
    rpassword::prompt_password(prompt)
        .map(Zeroizing::new)
        .map_err(|e| Fail::new(GENERAL, e.to_string()))
}

fn reason(r: &Reason) -> String {
    match r {
        Reason::NotVaultItem => t!(ReasonNotVaultItem),
        Reason::AlreadyExists => t!(ReasonExists),
        Reason::SameName => t!(ReasonSameName),
        Reason::UnsafeName => t!(ReasonUnsafeName),
        Reason::NoDirId => t!(ReasonNoDirId),
        Reason::MissingChunk(what) => t!(ReasonMissingChunk, what),
        Reason::Damaged(what) => t!(ReasonDamaged, what),
        Reason::Io(what) => t!(ReasonIo, what),
    }
}
