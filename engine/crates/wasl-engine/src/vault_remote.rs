//! An encrypted pair's remote: [`VaultRemote`] wraps any [`Remote`] so that everything stored
//! under the pair's Drive folder is vault v1 ciphertext (`docs/ENCRYPTION.md`). Names go through
//! [`wasl_crypto::names`], contents through [`VaultCipher`]; the engine above sees plaintext keys,
//! names and sizes and never knows the difference.
//!
//! Port of the Android app's `EncryptedRemoteFs` with one deliberate difference: listings report
//! plaintext sizes (ciphertext − 12 − 16 per chunk) instead of ciphertext sizes. The MD5 a listing
//! or an upload reports is Drive's, of the ciphertext: it never matches a local hash
//! ([`Remote::plaintext_hashes`] is false), but every write seals with a fresh file nonce, so it
//! changes exactly when the content is written. The planner's id + MD5 identity then sees every
//! write and nothing else (Drive raises a file's version by itself shortly after many uploads).
//!
//! [`ensure_manifest`] checks or creates `vault.json` before an encrypted pair's first sync.

use crate::remote::{Listing, ProgressFn, Remote, RemoteInfo, Revision, UploadSpec};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};
use wasl_core::RemoteEntry;
use wasl_crypto::content::{encrypted_size, plaintext_size, random_file_nonce, read_file_nonce};
use wasl_crypto::{CryptoError, VaultCipher, VaultKeys, VaultManifest, names};
use wasl_drive::{DriveError, Result};

/// Interrupted resumable uploads keep their ciphertext this long, so a later run can resume.
const TEMP_MAX_AGE: Duration = Duration::from_secs(3 * 24 * 3600);
/// A half-written ciphertext is useless after a crash; it goes sooner.
const ENCRYPTING_MAX_AGE: Duration = Duration::from_secs(6 * 3600);
const ENCRYPTING_SUFFIX: &str = ".enc";
const UPLOAD_PREFIX: &str = "ul-";

/// User-facing sentences, identical to the Android app's.
pub const DIFFERENT_VAULT: &str = "The Drive folder belongs to a different vault/passphrase";
pub const NEEDS_EMPTY_FOLDER: &str = "Encrypted pairs need an empty Drive folder or an existing vault";
pub const DOCS_UNSUPPORTED: &str = "Google Docs are not supported inside encrypted vaults";
/// A v1 vault's pair on an empty Drive folder: v1 is no longer started anywhere (security review
/// S4-2: 64-bit nonces, PBKDF2).
pub const V1_NO_NEW_FOLDERS: &str = "A vault of format v1 can't start a new Drive folder. Upgrade the vault (wasl vault upgrade) and pair the folder with the upgraded vault";

/// What [`VaultRemote::list_subtree`] left out of its last listing, for logging.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VaultSkips {
    /// Items whose names are not vault names for these keys (dropped in by someone else, another
    /// vault, tampered). Never deleted: the engine does not see them at all.
    pub foreign: usize,
    /// Google-native documents, which vaults ignore.
    pub google_docs: usize,
}

impl VaultSkips {
    pub fn total(&self) -> usize {
        self.foreign + self.google_docs
    }
}

pub struct VaultRemote {
    inner: Arc<dyn Remote>,
    keys: Arc<VaultKeys>,
    cipher: Arc<VaultCipher>,
    tmp_dir: PathBuf,
    skipped_foreign: AtomicUsize,
    skipped_docs: AtomicUsize,
    manifest: Option<VaultManifest>,
}

impl VaultRemote {
    /// Checks the pair's folder against `manifest` before every listing ([`ensure_manifest`]).
    pub fn with_manifest(mut self, manifest: VaultManifest) -> VaultRemote {
        self.manifest = Some(manifest);
        self
    }

    /// Wraps `inner` with the vault's `keys`. `tmp_dir` holds upload ciphertext (as large as the
    /// file being uploaded); it is created when needed, and stale temps in it are removed here.
    pub fn new(inner: Arc<dyn Remote>, keys: Arc<VaultKeys>, tmp_dir: impl Into<PathBuf>) -> VaultRemote {
        let tmp_dir = tmp_dir.into();
        purge_stale_temps(&tmp_dir);
        VaultRemote {
            cipher: Arc::new(VaultCipher::new(&keys)),
            inner,
            keys,
            tmp_dir,
            skipped_foreign: AtomicUsize::new(0),
            skipped_docs: AtomicUsize::new(0),
            manifest: None,
        }
    }

    /// Total number of items the last listing skipped (see [`Self::last_skips`]).
    pub fn last_skipped(&self) -> usize {
        self.last_skips().total()
    }

    /// What the last listing skipped, by reason. Items inside a skipped folder are not counted.
    pub fn last_skips(&self) -> VaultSkips {
        VaultSkips {
            foreign: self.skipped_foreign.load(Ordering::Relaxed),
            google_docs: self.skipped_docs.load(Ordering::Relaxed),
        }
    }

    fn encrypt_name(&self, parent_id: &str, name: &str) -> Result<String> {
        names::encrypt_name(&self.keys, parent_id, name).map_err(crypto_error)
    }

    /// The plaintext key for an encrypted key whose parent is already known.
    fn decrypt_key(&self, enc_key: &str, plain_by_enc: &HashMap<String, String>) -> Option<String> {
        let (enc_parent, enc_name) = split_key(enc_key);
        let plain_parent = if enc_parent.is_empty() {
            ""
        } else {
            plain_by_enc.get(enc_parent)?.as_str()
        };
        let name = names::decrypt_name(&self.keys, enc_name)?;
        Some(join_key(plain_parent, &name))
    }
}

#[async_trait]
impl Remote for VaultRemote {
    /// Drive's MD5 is of the ciphertext.
    fn plaintext_hashes(&self) -> bool {
        false
    }

    async fn list_subtree(&self, root_id: &str, drive_id: Option<&str>) -> Result<Listing> {
        let raw = self.inner.list_subtree(root_id, drive_id).await?;
        let mut plain_by_enc: HashMap<String, String> = HashMap::new();
        // Encrypted keys of folders whose whole subtree is skipped.
        let mut dropped: HashSet<String> = HashSet::new();
        // Parent plaintext key + NUL + lowercased plaintext name: Drive's duplicate rule, applied to
        // plaintext (two ciphertexts can decrypt to names that differ only in case).
        let mut seen: HashSet<String> = HashSet::new();
        let mut out = Listing::default();
        let (mut foreign, mut docs) = (0usize, 0usize);

        // Parents before children; within a folder the inner order (newest first) is kept, so the
        // newest of two same-named items wins, as in the plain listing.
        let mut entries = raw.entries;
        entries.sort_by_key(|e| e.key.matches('/').count());
        for e in entries {
            let (enc_parent, _) = split_key(&e.key);
            if !enc_parent.is_empty() && dropped.contains(enc_parent) {
                if e.is_dir {
                    dropped.insert(e.key);
                }
                continue;
            }
            if enc_parent.is_empty() && e.name == VaultManifest::FILE_NAME {
                continue;
            }
            if e.google_mime.is_some() {
                docs += 1;
                continue;
            }
            let plain_parent = if enc_parent.is_empty() {
                String::new()
            } else {
                match plain_by_enc.get(enc_parent) {
                    Some(p) => p.clone(),
                    None => {
                        if e.is_dir {
                            dropped.insert(e.key);
                        }
                        continue;
                    }
                }
            };
            let Some(plain_name) = names::decrypt_name(&self.keys, &e.name) else {
                foreign += 1;
                if e.is_dir {
                    dropped.insert(e.key);
                }
                continue;
            };
            let plain_key = join_key(&plain_parent, &plain_name);
            if !seen.insert(format!(
                "{plain_parent}\u{0}{}",
                wasl_core::identity::name_key(&plain_name)
            )) {
                out.duplicates.push(plain_key);
                if e.is_dir {
                    dropped.insert(e.key);
                }
                continue;
            }
            plain_by_enc.insert(e.key.clone(), plain_key.clone());
            out.entries.push(RemoteEntry {
                key: plain_key,
                name: plain_name,
                size: if e.is_dir {
                    e.size
                } else {
                    e.size.and_then(plaintext_size)
                },
                ..e
            });
        }
        out.duplicates.extend(
            raw.duplicates
                .iter()
                .filter_map(|d| self.decrypt_key(d, &plain_by_enc)),
        );
        out.shortcuts = raw
            .shortcuts
            .iter()
            .filter_map(|s| self.decrypt_key(s, &plain_by_enc))
            .collect();
        self.skipped_foreign.store(foreign, Ordering::Relaxed);
        self.skipped_docs.store(docs, Ordering::Relaxed);
        Ok(out)
    }

    /// Downloads the ciphertext next to `dest` and decrypts it into `dest`. Anything that does not
    /// verify fails with `DriveError::Integrity` and leaves no `dest`.
    async fn download(&self, entry: &RemoteEntry, dest: &Path, progress: ProgressFn<'_>) -> Result<()> {
        let temp = cipher_temp(dest);
        let cipher_entry = RemoteEntry {
            size: if entry.is_dir {
                entry.size
            } else {
                entry.size.map(encrypted_size)
            },
            md5: None,
            ..entry.clone()
        };
        if let Err(e) = self.inner.download(&cipher_entry, &temp, progress).await {
            // A transient failure keeps the partial ciphertext so the next attempt resumes it.
            if !e.is_transient() {
                let _ = std::fs::remove_file(&temp);
            }
            return Err(e);
        }
        let cipher = self.cipher.clone();
        let (src, dst) = (temp.clone(), dest.to_path_buf());
        let decrypted = tokio::task::spawn_blocking(move || cipher.decrypt_file(&src, &dst))
            .await
            .map_err(|e| DriveError::Unknown(format!("decryption task failed: {e}")));
        // A ciphertext that did not verify must not be resumed; a verified one is done.
        let _ = std::fs::remove_file(&temp);
        match decrypted {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => {
                let _ = std::fs::remove_file(dest);
                Err(crypto_error(e))
            }
            Err(e) => {
                let _ = std::fs::remove_file(dest);
                Err(e)
            }
        }
    }

    async fn export(
        &self,
        _entry: &RemoteEntry,
        _mime: &str,
        _dest: &Path,
        _progress: ProgressFn<'_>,
    ) -> Result<()> {
        Err(DriveError::Unknown(DOCS_UNSUPPORTED.into()))
    }

    /// Encrypts the source to a temp file and uploads it under its encrypted name. A resumable
    /// upload reuses the same ciphertext (and so the same inner session) until it completes; a
    /// fresh encryption always gets a fresh session key.
    async fn upload(&self, spec: UploadSpec<'_>, progress: ProgressFn<'_>) -> Result<RemoteInfo> {
        let enc_name = self.encrypt_name(spec.parent_id, spec.name)?;
        std::fs::create_dir_all(&self.tmp_dir).map_err(io_error)?;
        let resumable = spec.session_key.is_some();
        let temp = self.tmp_dir.join(format!(
            "{UPLOAD_PREFIX}{}.dsv",
            match spec.session_key {
                Some(k) => wasl_crypto::sha256_hex(k.as_bytes()),
                None => unique_suffix()?,
            }
        ));
        let plain_size = std::fs::metadata(spec.src).map_err(io_error)?.len();
        let reusable = if resumable
            && std::fs::metadata(&temp).is_ok_and(|m| m.is_file() && m.len() == encrypted_size(plain_size))
        {
            read_file_nonce(&temp)
        } else {
            None
        };
        let file_nonce = match reusable {
            Some(n) => n,
            None => {
                let cipher = self.cipher.clone();
                let (src, dst) = (spec.src.to_path_buf(), temp.clone());
                tokio::task::spawn_blocking(move || encrypt_into_place(&cipher, &src, &dst))
                    .await
                    .map_err(|e| DriveError::Unknown(format!("encryption task failed: {e}")))??
            }
        };
        let inner_session = spec.session_key.map(|k| format!("{k}|dsv:{}", hex(&file_nonce)));
        let result = self
            .inner
            .upload(
                UploadSpec {
                    src: &temp,
                    name: &enc_name,
                    parent_id: spec.parent_id,
                    existing_id: spec.existing_id,
                    mtime_ms: spec.mtime_ms,
                    session_key: inner_session.as_deref(),
                },
                progress,
            )
            .await;
        match result {
            Ok(info) => {
                let _ = std::fs::remove_file(&temp);
                // Drive's MD5 of the ciphertext: the file's identity (module docs).
                Ok(RemoteInfo {
                    name: spec.name.to_owned(),
                    size: Some(info.size.and_then(plaintext_size).unwrap_or(plain_size)),
                    ..info
                })
            }
            Err(e) => {
                if !resumable || !e.is_transient() {
                    let _ = std::fs::remove_file(&temp);
                }
                Err(e)
            }
        }
    }

    async fn create_folder(&self, parent_id: &str, name: &str) -> Result<RemoteInfo> {
        let enc_name = self.encrypt_name(parent_id, name)?;
        let info = self.inner.create_folder(parent_id, &enc_name).await?;
        Ok(RemoteInfo {
            name: name.to_owned(),
            md5: None,
            ..info
        })
    }

    async fn trash(&self, id: &str) -> Result<()> {
        self.inner.trash(id).await
    }

    async fn untrash(&self, id: &str) -> Result<()> {
        self.inner.untrash(id).await
    }

    async fn delete_permanently(&self, id: &str) -> Result<()> {
        self.inner.delete_permanently(id).await
    }

    async fn list_revisions(&self, id: &str) -> Result<Vec<Revision>> {
        // Sizes and checksums of ciphertext mean nothing to the user: report plaintext sizes.
        Ok(self
            .inner
            .list_revisions(id)
            .await?
            .into_iter()
            .map(|r| Revision {
                size: r.size.and_then(plaintext_size),
                md5: None,
                ..r
            })
            .collect())
    }

    async fn download_revision(&self, id: &str, revision_id: &str, dest: &Path) -> Result<()> {
        let temp = cipher_temp(dest);
        let fetched = self.inner.download_revision(id, revision_id, &temp).await;
        if let Err(e) = fetched {
            let _ = std::fs::remove_file(&temp);
            return Err(e);
        }
        let cipher = self.cipher.clone();
        let (src, dst) = (temp.clone(), dest.to_path_buf());
        let decrypted = tokio::task::spawn_blocking(move || cipher.decrypt_file(&src, &dst))
            .await
            .map_err(|e| DriveError::Unknown(format!("decryption task failed: {e}")));
        let _ = std::fs::remove_file(&temp);
        match decrypted {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => {
                let _ = std::fs::remove_file(dest);
                Err(crypto_error(e))
            }
            Err(e) => {
                let _ = std::fs::remove_file(dest);
                Err(e)
            }
        }
    }

    async fn keep_revision(&self, id: &str, revision_id: &str, keep: bool) -> Result<()> {
        self.inner.keep_revision(id, revision_id, keep).await
    }

    async fn prepare(&self, root_id: &str, drive_id: Option<&str>) -> Result<()> {
        if let Some(m) = &self.manifest {
            ensure_manifest(self.inner.as_ref(), root_id, drive_id, m, &self.tmp_dir).await?;
        }
        Ok(())
    }
}

/// What [`ensure_manifest`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestStatus {
    /// The folder already holds this vault's `vault.json`.
    Verified,
    /// The folder was empty; `vault.json` was uploaded.
    Created,
}

/// Makes sure the Drive folder `folder_id` belongs to the vault described by `manifest` before an
/// encrypted pair syncs: an existing `vault.json` must have the same salt, iterations and key check.
/// A folder without one is refused: a non-empty one with [`NEEDS_EMPTY_FOLDER`], an empty one with
/// [`V1_NO_NEW_FOLDERS`] (v1 keeps the folders it has, readable and writable, but never starts
/// another; new vaults and new folders are v2). `inner` is the plain (unencrypted) remote;
/// `tmp_dir` holds the manifest briefly.
pub async fn ensure_manifest(
    inner: &dyn Remote,
    folder_id: &str,
    drive_id: Option<&str>,
    manifest: &VaultManifest,
    tmp_dir: &Path,
) -> Result<ManifestStatus> {
    let listing = inner.list_subtree(folder_id, drive_id).await?;
    let existing = listing
        .entries
        .iter()
        .find(|e| e.key == VaultManifest::FILE_NAME && !e.is_dir && e.google_mime.is_none());
    std::fs::create_dir_all(tmp_dir).map_err(io_error)?;
    let temp = tmp_dir.join(format!("manifest-{}.json", unique_suffix()?));
    let outcome = async {
        match existing {
            Some(entry) => {
                inner.download(entry, &temp, &|_| {}).await?;
                let text = std::fs::read_to_string(&temp)
                    .map_err(|e| DriveError::Integrity(format!("vault.json cannot be read as text ({e})")))?;
                if crate::vault_v2_remote::vault_json_format(&text).as_deref()
                    == Some(wasl_crypto::v2::VaultManifestV2::FORMAT)
                {
                    return Err(DriveError::Integrity(crate::vault_v2_remote::VAULT_IS_V2.into()));
                }
                let remote = VaultManifest::parse(&text).map_err(crypto_error)?;
                if !remote.same_vault_as(manifest) {
                    return Err(DriveError::Integrity(DIFFERENT_VAULT.into()));
                }
                Ok(ManifestStatus::Verified)
            }
            None => {
                let has_other_content = !listing.entries.is_empty()
                    || !listing.duplicates.is_empty()
                    || !listing.shortcuts.is_empty();
                if has_other_content {
                    return Err(DriveError::Integrity(NEEDS_EMPTY_FOLDER.into()));
                }
                Err(DriveError::Integrity(V1_NO_NEW_FOLDERS.into()))
            }
        }
    }
    .await;
    let _ = std::fs::remove_file(&temp);
    outcome
}

pub(crate) fn crypto_error(e: CryptoError) -> DriveError {
    match e {
        CryptoError::Integrity(m) => DriveError::Integrity(m),
        CryptoError::Io(e) => DriveError::Io(e.to_string()),
        CryptoError::InvalidInput(m) => DriveError::Unknown(m),
    }
}

pub(crate) fn io_error(e: std::io::Error) -> DriveError {
    DriveError::Io(e.to_string())
}

/// Encrypts `src` to `<dst>.enc`, then renames it to `dst`, so `dst` is always complete.
fn encrypt_into_place(cipher: &VaultCipher, src: &Path, dst: &Path) -> Result<[u8; 8]> {
    let mut partial = dst.as_os_str().to_owned();
    partial.push(ENCRYPTING_SUFFIX);
    let partial = PathBuf::from(partial);
    let nonce = cipher.encrypt_file(src, &partial).map_err(crypto_error)?;
    if let Err(e) = std::fs::rename(&partial, dst) {
        let _ = std::fs::remove_file(&partial);
        return Err(io_error(e));
    }
    Ok(nonce)
}

/// Where the ciphertext of a download into `dest` is kept: next to it, with the engine's part
/// suffix so the local scan never picks it up (`a.txt.wasl-part` → `a.txt.dsv.wasl-part`).
fn cipher_temp(dest: &Path) -> PathBuf {
    let name = dest.file_name().unwrap_or_default().to_string_lossy();
    let stem = name.strip_suffix(wasl_fs::PART_SUFFIX).unwrap_or(&name);
    dest.with_file_name(format!("{stem}.dsv{}", wasl_fs::PART_SUFFIX))
}

fn split_key(key: &str) -> (&str, &str) {
    key.rsplit_once('/').unwrap_or(("", key))
}

fn join_key(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unique_suffix() -> Result<String> {
    random_file_nonce().map(|n| hex(&n)).map_err(io_error)
}

/// Removes upload temps left by crashed or abandoned runs.
fn purge_stale_temps(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let now = SystemTime::now();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with(UPLOAD_PREFIX) && !name.starts_with("manifest-") {
            continue;
        }
        let limit = if name.ends_with(ENCRYPTING_SUFFIX) {
            ENCRYPTING_MAX_AGE
        } else {
            TEMP_MAX_AGE
        };
        let age = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok());
        if age.is_some_and(|a| a > limit) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}
