//! A plain pair's Drive, guarded: a plain (unencrypted) pair never syncs a folder that holds a
//! Wasl vault. Shared by the computer's daemon and the phone (E2E 2026-10: the phone had no guard,
//! and a phone pair on a vault made on the computer would have uploaded plain files into it).

use crate::remote::Remote;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use wasl_crypto::v2::vault_json::VaultManifestV2;
use wasl_crypto::{CryptoError, VaultManifest};
use wasl_store::Pair;

/// A `vault.json` is a few hundred bytes; anything much larger is not one.
pub const MAX_MANIFEST_BYTES: u64 = 64 << 10;
/// What a `vault.json` that can't be read as a manifest says (it may be a damaged vault's).
pub const DAMAGED_MANIFEST: &str =
    "vault.json is damaged or too large to read, so this folder may be an encrypted vault";
/// What a plain pair says when its Drive folder holds a `vault.json` that can't be read.
pub const PLAIN_ON_DAMAGED_VAULT: &str = "This pair's Drive folder has a vault.json that is damaged or too \
     large to read, so it may be an encrypted vault: Wasl synced nothing, so no file went up \
     unencrypted. Check the folder on Drive.";
/// What a plain pair says when its Drive folder holds a vault.
pub const PLAIN_ON_VAULT: &str = "This pair's Drive folder is an encrypted vault, but the pair isn't \
     linked to it, so Wasl synced nothing: no file went up unencrypted. Remove the pair and add it \
     again to sync through the vault.";
/// What a plain pair says when a folder inside it holds a vault.
pub fn plain_on_nested_vault(folder: &str) -> String {
    format!(
        "The folder {folder} inside this pair's Drive folder is an encrypted vault, so Wasl synced \
         nothing: no file went up unencrypted. Exclude that folder from this pair (Excluded \
         subfolders) and sync it as its own vault pair."
    )
}

/// The format version of a Wasl `vault.json` (1 or 2); `None` only when the text is definitely not
/// a Wasl vault's: a JSON object naming another `format`. Anything else that is not a usable Wasl
/// manifest is an error: it may be a damaged vault, and is never taken for a plain file.
pub fn vault_format(text: &str) -> Result<Option<u32>, CryptoError> {
    let damaged = || CryptoError::Integrity(DAMAGED_MANIFEST.into());
    let value: serde_json::Value = serde_json::from_str(text.trim()).map_err(|_| damaged())?;
    match value.get("format").and_then(|f| f.as_str()) {
        Some(VaultManifest::FORMAT) => VaultManifest::parse(text).map(|_| Some(1)),
        Some(VaultManifestV2::FORMAT) => VaultManifestV2::parse(text).map(|_| Some(2)),
        Some(_) => Ok(None),
        None => Err(damaged()),
    }
}

/// The text of a `vault.json` found in a listing; an error when it is too large to be a vault's.
pub async fn manifest_text(
    drive: &dyn Remote,
    entry: &wasl_core::RemoteEntry,
    tmp: &Path,
) -> Result<String, wasl_drive::DriveError> {
    use wasl_drive::DriveError;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let damaged = || DriveError::Integrity(DAMAGED_MANIFEST.into());
    if entry.size.is_some_and(|n| n > MAX_MANIFEST_BYTES) {
        return Err(damaged());
    }
    std::fs::create_dir_all(tmp).map_err(|e| DriveError::Io(e.to_string()))?;
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let temp = tmp.join(format!("vault-{}-{n}.json", std::process::id()));
    let got = drive.download(entry, &temp, &|_| {}).await;
    let bytes = got.and_then(|()| std::fs::read(&temp).map_err(|e| DriveError::Io(e.to_string())));
    let _ = std::fs::remove_file(&temp);
    let bytes = bytes?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(damaged());
    }
    String::from_utf8(bytes).map_err(|_| damaged())
}

/// Whether a `vault.json` in a listing is a Wasl vault's (fails closed, see [`vault_format`]).
async fn is_wasl_vault(
    drive: &dyn Remote,
    entry: &wasl_core::RemoteEntry,
    tmp: &Path,
) -> Result<bool, wasl_drive::DriveError> {
    let text = manifest_text(drive, entry, tmp).await?;
    vault_format(&text).map(|v| v.is_some()).map_err(|e| match e {
        CryptoError::Integrity(m) if m == DAMAGED_MANIFEST => wasl_drive::DriveError::Integrity(m),
        e => wasl_drive::DriveError::Integrity(format!("{DAMAGED_MANIFEST} ({e})")),
    })
}

/// Every `vault.json` in a listing, at any depth (a vault can sit in a subfolder of a plain pair).
pub fn manifests(listing: &crate::Listing) -> impl Iterator<Item = &wasl_core::RemoteEntry> {
    listing.entries.iter().filter(|e| {
        (e.key == "vault.json" || e.key.ends_with("/vault.json")) && !e.is_dir && e.google_mime.is_none()
    })
}

/// A plain pair's Drive. It refuses a folder that holds a Wasl vault's `vault.json` (v1 or v2) at
/// its root or in a subfolder the pair's settings don't exclude, before anything is planned: plain
/// files would go up unencrypted into the vault and its ciphertext would come down as files.
/// Someone else's `vault.json` is an ordinary file.
pub struct PlainGuard {
    inner: Arc<dyn Remote>,
    tmp: PathBuf,
    /// The pair, for its excluded subfolders.
    pair: Pair,
}

impl PlainGuard {
    /// `tmp` holds a `vault.json` while it is read.
    pub fn new(inner: Arc<dyn Remote>, tmp: PathBuf, pair: Pair) -> PlainGuard {
        PlainGuard { inner, tmp, pair }
    }
}

type DriveResult<T> = wasl_drive::Result<T>;

#[async_trait]
impl Remote for PlainGuard {
    fn plaintext_hashes(&self) -> bool {
        self.inner.plaintext_hashes()
    }

    async fn prepare(&self, root_id: &str, drive_id: Option<&str>) -> DriveResult<()> {
        self.inner.prepare(root_id, drive_id).await
    }

    async fn list_subtree(&self, root_id: &str, drive_id: Option<&str>) -> DriveResult<crate::Listing> {
        let listing = self.inner.list_subtree(root_id, drive_id).await?;
        for entry in manifests(&listing) {
            let folder = entry
                .key
                .strip_suffix("vault.json")
                .unwrap_or("")
                .trim_end_matches('/');
            if !folder.is_empty() && crate::excluded_by_pair_settings(&self.pair, folder) {
                continue; // left out of this pair entirely: never listed, uploaded or downloaded
            }
            // Only a vault.json that is definitely someone else's lets a plain pair run here.
            let refused = |damaged: bool| {
                wasl_drive::DriveError::Integrity(if !folder.is_empty() {
                    plain_on_nested_vault(folder)
                } else if damaged {
                    PLAIN_ON_DAMAGED_VAULT.into()
                } else {
                    PLAIN_ON_VAULT.into()
                })
            };
            match is_wasl_vault(self.inner.as_ref(), entry, &self.tmp).await {
                Ok(false) => {}
                Ok(true) => return Err(refused(false)),
                Err(wasl_drive::DriveError::Integrity(_)) => return Err(refused(true)),
                Err(e) => return Err(e),
            }
        }
        Ok(listing)
    }

    async fn download(
        &self,
        entry: &wasl_core::RemoteEntry,
        dest: &Path,
        progress: crate::ProgressFn<'_>,
    ) -> DriveResult<()> {
        self.inner.download(entry, dest, progress).await
    }

    fn flagged_by_remote(&self, id: &str) -> bool {
        self.inner.flagged_by_remote(id)
    }

    async fn export(
        &self,
        entry: &wasl_core::RemoteEntry,
        mime: &str,
        dest: &Path,
        progress: crate::ProgressFn<'_>,
    ) -> DriveResult<()> {
        self.inner.export(entry, mime, dest, progress).await
    }

    async fn upload(
        &self,
        spec: crate::UploadSpec<'_>,
        progress: crate::ProgressFn<'_>,
    ) -> DriveResult<crate::RemoteInfo> {
        self.inner.upload(spec, progress).await
    }

    async fn current(&self, id: &str) -> DriveResult<Option<crate::RemoteInfo>> {
        self.inner.current(id).await
    }

    async fn create_folder(&self, parent_id: &str, name: &str) -> DriveResult<crate::RemoteInfo> {
        self.inner.create_folder(parent_id, name).await
    }

    async fn upload_with_properties(
        &self,
        spec: crate::UploadSpec<'_>,
        properties: &[(&str, &str)],
        progress: crate::ProgressFn<'_>,
    ) -> DriveResult<crate::RemoteInfo> {
        self.inner
            .upload_with_properties(spec, properties, progress)
            .await
    }

    async fn create_folder_with_properties(
        &self,
        parent_id: &str,
        name: &str,
        properties: &[(&str, &str)],
    ) -> DriveResult<crate::RemoteInfo> {
        self.inner
            .create_folder_with_properties(parent_id, name, properties)
            .await
    }

    async fn update_item(&self, id: &str, update: crate::ItemUpdate<'_>) -> DriveResult<crate::RemoteInfo> {
        self.inner.update_item(id, update).await
    }

    async fn housekeeping(&self, root_id: &str, drive_id: Option<&str>) -> DriveResult<()> {
        self.inner.housekeeping(root_id, drive_id).await
    }

    async fn trash(&self, id: &str) -> DriveResult<()> {
        self.inner.trash(id).await
    }

    async fn read_range(&self, id: &str, expect: &str, offset: u64, len: u64) -> DriveResult<Vec<u8>> {
        self.inner.read_range(id, expect, offset, len).await
    }

    async fn untrash(&self, id: &str) -> DriveResult<()> {
        self.inner.untrash(id).await
    }

    async fn delete_permanently(&self, id: &str) -> DriveResult<()> {
        self.inner.delete_permanently(id).await
    }

    async fn list_revisions(&self, id: &str) -> DriveResult<Vec<crate::Revision>> {
        self.inner.list_revisions(id).await
    }

    async fn download_revision(&self, id: &str, revision_id: &str, dest: &Path) -> DriveResult<()> {
        self.inner.download_revision(id, revision_id, dest).await
    }

    async fn keep_revision(&self, id: &str, revision_id: &str, keep: bool) -> DriveResult<()> {
        self.inner.keep_revision(id, revision_id, keep).await
    }
}
