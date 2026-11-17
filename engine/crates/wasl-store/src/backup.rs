//! The copy of the database taken before a migration (wasl#22).
//!
//! The copy sits beside the database and is named by the schema version it was taken at
//! (`wasl.db.pre-v7.bak`), so a copy left by an interrupted run can never be taken for a copy of a
//! later schema: only the copy this run made is ever restored, and every start that gets as far as
//! opening the database deletes the rest.

use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

const MARK: &str = ".pre-v";
const EXT: &str = ".bak";

fn sibling(db: &Path, suffix: &str) -> PathBuf {
    let mut name = db.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Where the copy of `db` taken at schema `version` lives.
pub(crate) fn path_for(db: &Path, version: i64) -> PathBuf {
    sibling(db, &format!("{MARK}{version}{EXT}"))
}

/// Flushes a file to disk. Opened for writing without changing it: Windows refuses to flush a
/// handle opened read-only.
fn flush(path: &Path) -> io::Result<()> {
    OpenOptions::new().write(true).open(path)?.sync_all()
}

/// Copies the database file (same volume, same permissions) and flushes the copy to disk. The
/// caller has already folded the write-ahead log into the file, so the copy opens on its own.
pub(crate) fn take(db: &Path, version: i64) -> io::Result<PathBuf> {
    let to = path_for(db, version);
    fs::copy(db, &to)?;
    flush(&to)?;
    Ok(to)
}

/// Puts the copy back over the database and drops the write-ahead log and shared-memory files of
/// the failed attempt, so SQLite can't replay them onto the restored file. The connection that ran
/// the migration must be closed first.
pub(crate) fn restore(db: &Path, copy: &Path) -> io::Result<()> {
    // Through a file of its own and a rename, so an interrupted restore never leaves a database
    // that is half the copy.
    let part = sibling(db, ".restoring");
    fs::copy(copy, &part)?;
    flush(&part)?;
    for ext in ["-wal", "-shm"] {
        match fs::remove_file(sibling(db, ext)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    fs::rename(&part, db)?;
    fs::remove_file(copy)
}

/// Sets aside a copy that could not be put back, under a name no later start deletes or
/// overwrites. Returns where it is.
pub(crate) fn keep(db: &Path, copy: &Path, version: i64) -> PathBuf {
    let kept = sibling(db, &format!(".unrestored-v{version}.bak"));
    if fs::rename(copy, &kept).is_ok() {
        kept
    } else {
        copy.to_path_buf()
    }
}

/// Deletes every copy of `db`, whatever schema it was taken at. Best effort: a copy that can't be
/// deleted now is tried again by the next start.
pub(crate) fn prune(db: &Path) {
    let (Some(dir), Some(name)) = (db.parent(), db.file_name().and_then(|n| n.to_str())) else {
        return;
    };
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    let prefix = format!("{name}{MARK}");
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let file = entry.file_name();
        let Some(file) = file.to_str() else { continue };
        let Some(rest) = file.strip_prefix(&prefix).and_then(|r| r.strip_suffix(EXT)) else {
            continue;
        };
        if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) {
            let _ = fs::remove_file(entry.path());
        }
    }
}
