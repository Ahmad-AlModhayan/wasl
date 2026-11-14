//! The v2 Drive folder layout on a local disk (ON_DEMAND.md §1.2): what a Drive download of a v2
//! vault looks like, and how [`crate::recover`] reads it.
//!
//! ```text
//! vault.json
//! <name>.w2/                      a folder; holds .wasl-dir = "W2D1" || its dir id || tag
//! <name>.w2/<name>.w2             a file manifest (W2M1)
//! .wasl-chunks/<00..ff>/<id>      every chunk object (W2C1 in 2.0, W2C2 in 2.1)
//! ```
//!
//! [`write_tree`] encrypts a local folder into that layout. The engine writes the same objects to
//! Drive; this writer exists for tests, the cross-check with `tools/decrypt_vault.py`, and exports.

use super::chunks::CHUNKS_DIR;
use super::file::encrypt_stream;
use super::format::Format;
use super::keys::VaultKeysV2;
use super::manifest::FileId;
use super::names::{self, DIR_MARKER_NAME, DirId, Placement};
use crate::error::{CryptoError, Result};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Encrypts every folder and file under `src` into the v2 layout under `vault_root` (which must
/// already hold, or be given later, its `vault.json`). Folders get random dir ids, files random
/// file ids; symlinks and other special files are skipped. Writes format 2.0, the revision
/// `tools/decrypt_vault.py` reads; [`write_tree_as`] writes another.
pub fn write_tree(keys: &VaultKeysV2, src: &Path, vault_root: &Path) -> Result<()> {
    write_tree_as(keys, Format::V2_0, src, vault_root)
}

/// [`write_tree`] in the format revision `format` (the vault's).
pub fn write_tree_as(keys: &VaultKeysV2, format: Format, src: &Path, vault_root: &Path) -> Result<()> {
    std::fs::create_dir_all(vault_root.join(CHUNKS_DIR))?;
    write_dir(keys, format, src, vault_root, vault_root, &DirId::ROOT)
}

fn write_dir(
    keys: &VaultKeysV2,
    format: Format,
    src: &Path,
    root: &Path,
    dst: &Path,
    dir_id: &DirId,
) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(src)?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e
            .file_name()
            .into_string()
            .map_err(|_| CryptoError::InvalidInput("file name is not UTF-8".into()))?;
        let enc = names::encrypt_name(keys, dir_id, &name)?;
        let at = Placement::from_encrypted(dir_id, &enc)
            .ok_or_else(|| CryptoError::InvalidInput("name did not encrypt".into()))?;
        let ft = e.file_type()?;
        if ft.is_dir() {
            let child = dst.join(&enc);
            let id = DirId::random()?;
            std::fs::create_dir_all(&child)?;
            std::fs::write(child.join(DIR_MARKER_NAME), names::dir_marker(keys, &at, &id))?;
            write_dir(keys, format, &e.path(), root, &child, &id)?;
        } else if ft.is_file() {
            let mtime_ms = e.metadata()?.modified().map(system_time_ms).unwrap_or(0);
            let input = std::io::BufReader::new(std::fs::File::open(e.path())?);
            let manifest = encrypt_stream(keys, format, FileId::random()?, mtime_ms, input, |chunk| {
                let path = root.join(chunk.object_path());
                if !path.exists() {
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&path, chunk.seal()?)?;
                }
                Ok(())
            })?;
            std::fs::write(dst.join(&enc), manifest.seal(keys, &at)?)?;
        }
    }
    Ok(())
}

/// Milliseconds since the Unix epoch (negative before it).
pub fn system_time_ms(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_millis()).unwrap_or(i64::MAX),
    }
}

/// The inverse of [`system_time_ms`].
pub fn ms_system_time(ms: i64) -> SystemTime {
    let d = std::time::Duration::from_millis(ms.unsigned_abs());
    if ms >= 0 { UNIX_EPOCH + d } else { UNIX_EPOCH - d }
}
