//! Recovery (SRS FR-V3): decrypt a whole vault folder downloaded from Drive, v1 or v2, with
//! nothing but the passphrase. `wasl vault decrypt` is a thin shell around this module;
//! `tools/decrypt_vault.py` is the independent reference doing the same.
//!
//! Items that are not part of the vault (files someone else dropped into the folder, names that do
//! not decrypt) are skipped, never treated as errors; items that fail to verify are reported and
//! no unverified plaintext is left in the output.

use crate::error::{CryptoError, Result};
use crate::v2::chunks::{CHUNKS_DIR, ChunkId};
use crate::v2::layout::ms_system_time;
use crate::v2::manifest::MAX_MANIFEST_BYTES;
use crate::v2::names::{DIR_MARKER_NAME, DirId, Placement, parse_dir_marker};
use crate::v2::{self, FileManifest, VaultKeysV2, VaultManifestV2};
use crate::{VaultCipher, VaultKeys, VaultManifest, names};
use std::collections::HashSet;
use std::io::{self, BufWriter, Read};
use std::path::{Component, Path};

/// The vault manifest of a downloaded folder, either version.
#[derive(Debug, Clone)]
pub enum Vault {
    V1(VaultManifest),
    V2(VaultManifestV2),
}

/// The keys of an unlocked vault, either version.
#[derive(Debug, Clone)]
// Two more sub-keys (format 2.1) made the v2 variant bigger; one value per recovery run, and
// boxing it would change how every caller builds one.
#[allow(clippy::large_enum_variant)]
pub enum Unlocked {
    V1(VaultKeys),
    V2(VaultKeysV2),
}

impl Vault {
    /// Reads `<folder>/vault.json`. A missing manifest is an [`io::ErrorKind::NotFound`] error.
    pub fn load(folder: &Path) -> Result<Vault> {
        let path = folder.join(VaultManifest::FILE_NAME);
        let meta = std::fs::metadata(&path)?;
        if meta.len() > 64 * 1024 {
            return Err(CryptoError::Integrity(
                "vault.json is too large to be a vault manifest".into(),
            ));
        }
        Vault::parse(&std::fs::read_to_string(path)?)
    }

    /// Detects the version from `format` and `version` and parses strictly.
    pub fn parse(text: &str) -> Result<Vault> {
        let v: serde_json::Value = serde_json::from_str(text.trim())
            .map_err(|e| CryptoError::Integrity(format!("vault.json is not valid JSON ({e})")))?;
        match v.get("format").and_then(|f| f.as_str()) {
            Some(VaultManifestV2::FORMAT) => VaultManifestV2::parse(text).map(Vault::V2),
            Some(VaultManifest::FORMAT) => VaultManifest::parse(text).map(Vault::V1),
            other => Err(CryptoError::Integrity(format!(
                "vault.json has an unknown format ({})",
                other.unwrap_or("none")
            ))),
        }
    }

    /// 1 or 2.
    pub fn version(&self) -> u8 {
        match self {
            Vault::V1(_) => 1,
            Vault::V2(_) => 2,
        }
    }

    /// The format for people: "1", "2.0" or "2.1".
    pub fn format_label(&self) -> &'static str {
        match self {
            Vault::V1(_) => "1",
            Vault::V2(m) => m.format().label(),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Vault::V1(m) => &m.name,
            Vault::V2(m) => &m.name,
        }
    }

    pub fn created(&self) -> &str {
        match self {
            Vault::V1(m) => &m.created,
            Vault::V2(m) => &m.created,
        }
    }

    /// Derives the keys (expensive). `Ok(None)` for a wrong passphrase.
    pub fn unlock(&self, passphrase: &str) -> Result<Option<Unlocked>> {
        Ok(match self {
            Vault::V1(m) => m.verify_passphrase(passphrase)?.map(Unlocked::V1),
            Vault::V2(m) => m.verify_passphrase(passphrase)?.map(Unlocked::V2),
        })
    }
}

/// Why an item was skipped or failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// Its name is not a name of this vault (someone else's file, or moved from another folder).
    NotVaultItem,
    /// The output already has it (pass `overwrite`).
    AlreadyExists,
    /// Another item of the same folder decrypts to the same name (ignoring case).
    SameName,
    /// The name cannot be written safely on this system (path separators, drive prefixes).
    UnsafeName,
    /// A v2 folder whose `.wasl-dir` is missing or does not verify (tampered, or copied from another
    /// folder): its contents cannot be named.
    NoDirId,
    /// A chunk object the file needs is not in `.wasl-chunks`.
    MissingChunk(String),
    /// Does not verify: modified, truncated or damaged.
    Damaged(String),
    /// Reading or writing failed.
    Io(String),
}

/// Progress, reported as the walk goes. Paths are `/`-separated and relative to the vault root.
#[derive(Debug)]
pub enum Event<'a> {
    Folder(&'a str),
    File(&'a str, u64),
    Skipped(&'a str, &'a Reason),
    Failed(&'a str, &'a Reason),
}

#[derive(Debug, Default, Clone)]
pub struct Summary {
    pub folders: u64,
    pub files: u64,
    pub bytes: u64,
    pub skipped: Vec<(String, Reason)>,
    pub failed: Vec<(String, Reason)>,
}

const PARTIAL: &str = ".wasl-partial";

/// Decrypts the vault folder `src` into `out` (created when missing).
pub fn decrypt_tree(
    src: &Path,
    out: &Path,
    keys: &Unlocked,
    overwrite: bool,
    mut on_event: impl FnMut(Event<'_>),
) -> Result<Summary> {
    std::fs::create_dir_all(out)?;
    let mut w = Walk {
        root: src,
        keys,
        overwrite,
        summary: Summary::default(),
        on_event: &mut on_event,
    };
    w.dir(src, out, "", &DirId::ROOT);
    Ok(w.summary)
}

struct Walk<'a> {
    root: &'a Path,
    keys: &'a Unlocked,
    overwrite: bool,
    summary: Summary,
    on_event: &'a mut dyn FnMut(Event<'_>),
}

impl Walk<'_> {
    fn skip(&mut self, rel: String, r: Reason) {
        (self.on_event)(Event::Skipped(&rel, &r));
        self.summary.skipped.push((rel, r));
    }

    fn fail(&mut self, rel: String, r: Reason) {
        (self.on_event)(Event::Failed(&rel, &r));
        self.summary.failed.push((rel, r));
    }

    fn dir(&mut self, src: &Path, dst: &Path, rel: &str, dir_id: &DirId) {
        let mut entries: Vec<_> = match std::fs::read_dir(src) {
            Ok(it) => it.filter_map(|e| e.ok()).collect(),
            Err(e) => return self.fail(display(rel), Reason::Io(e.to_string())),
        };
        entries.sort_by_key(|e| e.file_name());
        let mut written = HashSet::new();
        for e in entries {
            let enc = e.file_name().to_string_lossy().into_owned();
            let rel_enc = join(rel, &enc);
            // Vault state records (ENCRYPTION.md §11.11) say nothing about the files.
            if rel.is_empty()
                && (enc == VaultManifest::FILE_NAME
                    || enc == CHUNKS_DIR
                    || crate::v2::state::WriterId::from_object_name(&enc).is_some())
            {
                continue;
            }
            if enc == DIR_MARKER_NAME || enc.ends_with(PARTIAL) {
                continue;
            }
            let plain = match self.keys {
                Unlocked::V1(k) => names::decrypt_name(k, &enc),
                Unlocked::V2(k) => v2::names::decrypt_name(k, dir_id, &enc),
            };
            let Some(plain) = plain else {
                self.skip(rel_enc, Reason::NotVaultItem);
                continue;
            };
            let rel_plain = join(rel, &plain);
            if !safe_component(&plain) {
                self.fail(rel_plain, Reason::UnsafeName);
                continue;
            }
            if !written.insert(plain.to_lowercase()) {
                self.fail(rel_plain, Reason::SameName);
                continue;
            }
            let target = dst.join(&plain);
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                let child_id = match self.keys {
                    Unlocked::V1(_) => DirId::ROOT,
                    Unlocked::V2(k) => match Placement::from_encrypted(dir_id, &enc)
                        .and_then(|at| read_marker(k, &at, &e.path()))
                    {
                        Some(id) => id,
                        None => {
                            self.fail(rel_plain, Reason::NoDirId);
                            continue;
                        }
                    },
                };
                if let Err(err) = std::fs::create_dir_all(&target) {
                    self.fail(rel_plain, Reason::Io(err.to_string()));
                    continue;
                }
                self.summary.folders += 1;
                (self.on_event)(Event::Folder(&rel_plain));
                self.dir(&e.path(), &target, &rel_plain, &child_id);
            } else if ft.is_file() {
                if target.exists() && !self.overwrite {
                    self.skip(rel_plain, Reason::AlreadyExists);
                    continue;
                }
                match self.file(&e.path(), &target, dir_id, &enc) {
                    Ok(n) => {
                        self.summary.files += 1;
                        self.summary.bytes += n;
                        (self.on_event)(Event::File(&rel_plain, n));
                    }
                    Err(r) => self.fail(rel_plain, r),
                }
            }
        }
    }

    fn file(&self, src: &Path, target: &Path, dir_id: &DirId, enc: &str) -> Result<u64, Reason> {
        let mut tmp = target.as_os_str().to_owned();
        tmp.push(PARTIAL);
        let tmp = std::path::PathBuf::from(tmp);
        let r = match self.keys {
            Unlocked::V1(k) => VaultCipher::new(k).decrypt_file(src, &tmp).map_err(reason),
            Unlocked::V2(k) => match Placement::from_encrypted(dir_id, enc) {
                Some(at) => self.file_v2(k, &at, src, &tmp),
                None => Err(Reason::NotVaultItem),
            },
        };
        let r = r.and_then(|n| {
            std::fs::rename(&tmp, target)
                .map(|()| n)
                .map_err(|e| Reason::Io(e.to_string()))
        });
        if r.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        r
    }

    fn file_v2(&self, keys: &VaultKeysV2, at: &Placement, src: &Path, tmp: &Path) -> Result<u64, Reason> {
        let manifest = read_bounded(src, MAX_MANIFEST_BYTES as u64)
            .map_err(|e| Reason::Io(e.to_string()))
            .and_then(|b| FileManifest::open(keys, at, &b).map_err(reason))?;
        let fetch = |id: &ChunkId| -> io::Result<Vec<u8>> {
            let path = self.root.join(CHUNKS_DIR).join(id.shard()).join(id.object_name());
            read_bounded(&path, v2::chunks::MAX_SEALED_BYTES as u64).map_err(|e| {
                if e.kind() == io::ErrorKind::NotFound {
                    io::Error::new(io::ErrorKind::NotFound, id.object_path())
                } else {
                    e
                }
            })
        };
        let f = std::fs::File::create(tmp).map_err(|e| Reason::Io(e.to_string()))?;
        let mut w = BufWriter::new(f);
        let n = v2::decrypt_file(keys, &manifest, fetch, &mut w).map_err(reason)?;
        let f = w.into_inner().map_err(|e| Reason::Io(e.to_string()))?;
        let _ = f.set_modified(ms_system_time(manifest.mtime_ms));
        f.sync_all().map_err(|e| Reason::Io(e.to_string()))?;
        Ok(n)
    }
}

fn reason(e: CryptoError) -> Reason {
    match e {
        CryptoError::Integrity(m) | CryptoError::InvalidInput(m) => Reason::Damaged(m),
        CryptoError::Io(e) if e.kind() == io::ErrorKind::NotFound => Reason::MissingChunk(e.to_string()),
        CryptoError::Io(e) => Reason::Io(e.to_string()),
    }
}

fn read_marker(keys: &VaultKeysV2, at: &Placement, dir: &Path) -> Option<DirId> {
    parse_dir_marker(keys, at, &read_bounded(&dir.join(DIR_MARKER_NAME), 128).ok()?)
}

/// Reads a file, refusing one larger than `max` bytes (it cannot be what the format wrote).
fn read_bounded(path: &Path, max: u64) -> io::Result<Vec<u8>> {
    let f = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    f.take(max + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file is larger than the format allows",
        ));
    }
    Ok(buf)
}

/// True when `name` is exactly one ordinary path component on this system.
fn safe_component(name: &str) -> bool {
    let mut c = Path::new(name).components();
    matches!((c.next(), c.next()), (Some(Component::Normal(n)), None) if n == name)
}

fn join(rel: &str, name: &str) -> String {
    if rel.is_empty() {
        name.to_owned()
    } else {
        format!("{rel}/{name}")
    }
}

fn display(rel: &str) -> String {
    if rel.is_empty() { ".".into() } else { rel.into() }
}
