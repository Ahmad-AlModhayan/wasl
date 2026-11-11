//! The remote side of a pair, as the executor sees it. [`crate::DriveRemote`] implements it over
//! Google Drive; [`crate::MemoryRemote`] is an in-memory Drive for tests and demos. Later routes
//! (devices, vaults) implement the same trait.

use async_trait::async_trait;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use wasl_core::RemoteEntry;
use wasl_drive::Result;

/// Everything under a pair's remote folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    pub entries: Vec<RemoteEntry>,
    /// Keys of same-named siblings that were left out (FR-C8).
    pub duplicates: Vec<String>,
    /// Keys of shortcuts, which are not followed (FR-C8).
    pub shortcuts: Vec<String>,
    /// Item properties (Drive `appProperties`) by item id, for the listed items that have any.
    /// Vault v2 keeps its folder ids and listing blobs there; remotes that cannot report them
    /// leave this empty and vault v2 falls back to reading its objects.
    pub properties: HashMap<String, BTreeMap<String, String>>,
}

/// A metadata change of one item, applied at once ([`Remote::update_item`]).
#[derive(Debug, Clone, Copy, Default)]
pub struct ItemUpdate<'a> {
    pub name: Option<&'a str>,
    /// Move from the first parent to the second.
    pub parents: Option<(&'a str, &'a str)>,
    /// Properties to set (others are kept).
    pub properties: &'a [(&'a str, &'a str)],
}

/// A file or folder the remote just created or updated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteInfo {
    pub id: String,
    pub name: String,
    pub size: Option<u64>,
    pub md5: Option<String>,
    pub mtime: i64,
    pub version: Option<i64>,
}

/// One upload.
pub struct UploadSpec<'a> {
    pub src: &'a Path,
    pub name: &'a str,
    pub parent_id: &'a str,
    /// Replace this file's content instead of creating a new file.
    pub existing_id: Option<&'a str>,
    pub mtime_ms: i64,
    /// Key for a resumable session; a changed file must get a different key.
    pub session_key: Option<&'a str>,
}

/// A stored version of a remote file (Drive keeps 30 days or 100 versions; FR-F6).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct Revision {
    pub id: String,
    pub modified_ms: i64,
    pub size: Option<u64>,
    pub md5: Option<String>,
    pub keep_forever: bool,
    /// Who made it, when the remote knows (another person on a shared folder).
    pub modified_by: Option<String>,
}

/// Bytes moved so far in one transfer.
pub type ProgressFn<'a> = &'a (dyn Fn(u64) + Send + Sync);

#[async_trait]
pub trait Remote: Send + Sync {
    /// False when content is transformed on the way (vaults): the remote's MD5 cannot be compared
    /// with local hashes.
    fn plaintext_hashes(&self) -> bool {
        true
    }

    /// Runs before every listing of a pair's folder. Wrappers use it to check the folder is theirs
    /// (a vault verifies or creates its `vault.json`). The default does nothing.
    async fn prepare(&self, _root_id: &str, _drive_id: Option<&str>) -> Result<()> {
        Ok(())
    }

    /// Everything under `root_id`. A root that no longer exists fails with `NotFound`, never lists
    /// as empty (which would read as "everything deleted").
    async fn list_subtree(&self, root_id: &str, drive_id: Option<&str>) -> Result<Listing>;

    /// The items directly in `folder_id`, keyed by name, for checks that look at one level only
    /// (whether a folder holds a vault, whether it is empty). The default lists the subtree and
    /// keeps its top level; [`crate::DriveRemote`] reads just the one folder, which stays quick
    /// however big the folder is.
    async fn list_top_level(&self, folder_id: &str, drive_id: Option<&str>) -> Result<Listing> {
        let mut listing = self.list_subtree(folder_id, drive_id).await?;
        // A top-level item's key is its own name; anything deeper has a parent's in front.
        listing.entries.retain(|e| e.key == e.name);
        listing.duplicates.retain(|k| !k.contains('/'));
        listing.shortcuts.retain(|k| !k.contains('/'));
        let kept: HashSet<&str> = listing.entries.iter().map(|e| e.id.as_str()).collect();
        listing.properties.retain(|id, _| kept.contains(id.as_str()));
        Ok(listing)
    }

    async fn download(&self, entry: &RemoteEntry, dest: &Path, progress: ProgressFn<'_>) -> Result<()>;

    /// True when the last download of `id` went through only because the remote's malware/spam
    /// flag was acknowledged (the user's own file), so the run log can say so.
    fn flagged_by_remote(&self, _id: &str) -> bool {
        false
    }

    /// Exports a Google document to `dest` as `mime`.
    async fn export(
        &self,
        entry: &RemoteEntry,
        mime: &str,
        dest: &Path,
        progress: ProgressFn<'_>,
    ) -> Result<()>;

    async fn upload(&self, spec: UploadSpec<'_>, progress: ProgressFn<'_>) -> Result<RemoteInfo>;

    /// The item as Drive has it now, read past any listing cache, so an upload can check nothing
    /// replaced it since the listing. `None` when this remote cannot tell; a gone item fails with
    /// `NotFound`.
    async fn current(&self, _id: &str) -> Result<Option<RemoteInfo>> {
        Ok(None)
    }

    async fn create_folder(&self, parent_id: &str, name: &str) -> Result<RemoteInfo>;

    /// [`Self::upload`] that also sets item properties in the same write. The default drops the
    /// properties: they are a listing cache whose readers fall back to the content.
    async fn upload_with_properties(
        &self,
        spec: UploadSpec<'_>,
        _properties: &[(&str, &str)],
        progress: ProgressFn<'_>,
    ) -> Result<RemoteInfo> {
        self.upload(spec, progress).await
    }

    /// [`Self::create_folder`] that also sets item properties. The default drops them, like
    /// [`Self::upload_with_properties`].
    async fn create_folder_with_properties(
        &self,
        parent_id: &str,
        name: &str,
        _properties: &[(&str, &str)],
    ) -> Result<RemoteInfo> {
        self.create_folder(parent_id, name).await
    }

    /// Renames, moves and/or sets properties of an item in one write, keeping its id.
    async fn update_item(&self, _id: &str, _update: ItemUpdate<'_>) -> Result<RemoteInfo> {
        Err(wasl_drive::DriveError::Unknown(
            "this remote cannot rename or move items".into(),
        ))
    }

    /// Periodic upkeep, run with the recycle-bin purge (vault v2 sweeps unreferenced chunks).
    /// The default does nothing.
    async fn housekeeping(&self, _root_id: &str, _drive_id: Option<&str>) -> Result<()> {
        Ok(())
    }

    /// Moves to the remote's bin (recoverable).
    async fn trash(&self, id: &str) -> Result<()>;

    /// Bytes `offset..offset+len` of a file's stored content (fewer at its end), for views (files
    /// on demand). `expect` is the content the caller listed ([`wasl_vfs::FileRef::version`]: the
    /// MD5 or the Drive version): bytes are returned only if they belong to it, so a file that
    /// changes between two reads fails with [`DriveError::Integrity`] instead of mixing versions.
    /// The default refuses.
    async fn read_range(&self, _id: &str, _expect: &str, _offset: u64, _len: u64) -> Result<Vec<u8>> {
        Err(wasl_drive::DriveError::Unknown(
            "this remote cannot read part of a file".into(),
        ))
    }

    /// Takes a file back out of the remote's bin.
    async fn untrash(&self, id: &str) -> Result<()>;

    /// Deletes for good, bypassing the bin ("Delete forever" in the recycle bin only).
    async fn delete_permanently(&self, _id: &str) -> Result<()> {
        Err(wasl_drive::DriveError::Unknown(
            "this remote cannot delete permanently".into(),
        ))
    }

    /// Stored versions of a file, oldest first.
    async fn list_revisions(&self, _id: &str) -> Result<Vec<Revision>> {
        Ok(Vec::new())
    }

    async fn download_revision(&self, _id: &str, _revision_id: &str, _dest: &Path) -> Result<()> {
        Err(wasl_drive::DriveError::Unknown(
            "this remote has no revisions".into(),
        ))
    }

    /// Keeps a revision past the remote's retention, or lets it expire again.
    async fn keep_revision(&self, _id: &str, _revision_id: &str, _keep: bool) -> Result<()> {
        Err(wasl_drive::DriveError::Unknown(
            "this remote has no revisions".into(),
        ))
    }
}
