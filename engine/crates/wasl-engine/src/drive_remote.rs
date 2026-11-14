//! [`Remote`] over Google Drive: the Drive client for transfers, the account's node cache for
//! listings (one crawl, then Changes deltas), and the store for resumable sessions.

use crate::remote::{Listing, ProgressFn, Remote, RemoteInfo, Revision, UploadSpec};
use async_trait::async_trait;
use std::path::Path;
use std::sync::Arc;
use wasl_core::RemoteEntry;
use wasl_drive::{
    DriveClient, DriveFile, DriveNodeCache, Result, SessionRef, SessionStore, UploadRequest, parse_rfc3339,
};

pub struct DriveRemote {
    pub account: String,
    pub client: Arc<DriveClient>,
    pub cache: Arc<DriveNodeCache>,
    pub sessions: Arc<dyn SessionStore>,
    /// Files whose last download needed Drive's malware flag acknowledged.
    pub flagged: std::sync::Mutex<std::collections::HashSet<String>>,
}

fn to_i64(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn info(f: &DriveFile) -> RemoteInfo {
    RemoteInfo {
        id: f.id.clone(),
        name: f.name.clone(),
        size: f.size_u64(),
        md5: f.md5_checksum.clone(),
        mtime: parse_rfc3339(f.modified_time.as_deref()).unwrap_or(0),
        version: f.version_u64().map(to_i64),
    }
}

fn listing(l: wasl_drive::SubtreeListing) -> Listing {
    let properties = l
        .entries
        .iter()
        .filter_map(|e| e.app_properties.clone().map(|p| (e.id.clone(), p)))
        .collect();
    Listing {
        entries: l
            .entries
            .into_iter()
            .map(|e| RemoteEntry {
                key: e.key,
                id: e.id,
                name: e.name,
                is_dir: e.is_dir,
                size: e.size,
                md5: e.md5,
                mtime: e.mtime,
                version: e.version.map(to_i64),
                google_mime: e.google_mime,
                parent_id: e.parent_id,
            })
            .collect(),
        duplicates: l.duplicates,
        shortcuts: l.shortcuts,
        properties,
    }
}

#[async_trait]
impl Remote for DriveRemote {
    async fn list_subtree(&self, root_id: &str, drive_id: Option<&str>) -> Result<Listing> {
        Ok(listing(
            self.cache.list_subtree(&self.account, root_id, drive_id).await?,
        ))
    }

    async fn list_top_level(&self, folder_id: &str, drive_id: Option<&str>) -> Result<Listing> {
        Ok(listing(
            self.cache
                .list_top_level(&self.account, folder_id, drive_id)
                .await?,
        ))
    }

    async fn download(&self, entry: &RemoteEntry, dest: &Path, progress: ProgressFn<'_>) -> Result<()> {
        let flagged = self
            .client
            .download_flagged(&self.account, &entry.id, dest, entry.size, |done, _| {
                progress(done)
            })
            .await?;
        let mut set = self.flagged.lock().unwrap_or_else(|e| e.into_inner());
        if flagged {
            set.insert(entry.id.clone());
        } else {
            set.remove(&entry.id);
        }
        Ok(())
    }

    fn flagged_by_remote(&self, id: &str) -> bool {
        self.flagged
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(id)
    }

    async fn export(
        &self,
        entry: &RemoteEntry,
        mime: &str,
        dest: &Path,
        progress: ProgressFn<'_>,
    ) -> Result<()> {
        self.client
            .export(&self.account, &entry.id, mime, dest, |done, _| progress(done))
            .await
    }

    async fn upload(&self, spec: UploadSpec<'_>, progress: ProgressFn<'_>) -> Result<RemoteInfo> {
        let mut req = UploadRequest::new(spec.src, spec.name);
        req.parent_id = Some(spec.parent_id);
        req.existing_file_id = spec.existing_id;
        req.modified_time_ms = (spec.mtime_ms > 0).then_some(spec.mtime_ms);
        req.session = spec.session_key.map(|key| SessionRef {
            store: self.sessions.as_ref(),
            key,
        });
        let f = self
            .client
            .upload(&self.account, req, |done, _| progress(done))
            .await?;
        self.cache.remember(&self.account, &f).await?;
        Ok(info(&f))
    }

    async fn current(&self, id: &str) -> Result<Option<RemoteInfo>> {
        let f = self.client.get_file(&self.account, id).await?;
        if f.trashed {
            return Err(wasl_drive::DriveError::NotFound(format!(
                "{} is in Drive's bin",
                f.name
            )));
        }
        Ok(Some(info(&f)))
    }

    async fn create_folder(&self, parent_id: &str, name: &str) -> Result<RemoteInfo> {
        let f = self.client.create_folder(&self.account, parent_id, name).await?;
        self.cache.remember(&self.account, &f).await?;
        Ok(info(&f))
    }

    async fn upload_with_properties(
        &self,
        spec: UploadSpec<'_>,
        properties: &[(&str, &str)],
        progress: ProgressFn<'_>,
    ) -> Result<RemoteInfo> {
        let props = property_map(properties);
        let mut req = UploadRequest::new(spec.src, spec.name);
        req.parent_id = Some(spec.parent_id);
        req.existing_file_id = spec.existing_id;
        req.modified_time_ms = (spec.mtime_ms > 0).then_some(spec.mtime_ms);
        req.session = spec.session_key.map(|key| SessionRef {
            store: self.sessions.as_ref(),
            key,
        });
        req.app_properties = props.as_ref();
        let f = self
            .client
            .upload(&self.account, req, |done, _| progress(done))
            .await?;
        self.cache.remember(&self.account, &f).await?;
        Ok(info(&f))
    }

    async fn create_folder_with_properties(
        &self,
        parent_id: &str,
        name: &str,
        properties: &[(&str, &str)],
    ) -> Result<RemoteInfo> {
        let f = self
            .client
            .create_folder_with(&self.account, parent_id, name, property_map(properties))
            .await?;
        self.cache.remember(&self.account, &f).await?;
        Ok(info(&f))
    }

    /// Renames, moves and sets properties in one PATCH.
    async fn update_item(&self, id: &str, update: crate::remote::ItemUpdate<'_>) -> Result<RemoteInfo> {
        let meta = wasl_drive::PatchFileMetadata {
            name: update.name.map(str::to_owned),
            app_properties: property_map(update.properties),
            ..Default::default()
        };
        let f = self
            .client
            .patch(
                &self.account,
                id,
                &meta,
                update.parents.map(|p| p.1),
                update.parents.map(|p| p.0),
            )
            .await?;
        self.cache.remember(&self.account, &f).await?;
        Ok(info(&f))
    }

    async fn read_range(&self, id: &str, expect: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(len as usize);
        let mut at = offset;
        let end = offset.saturating_add(len);
        while at < end {
            let mut buf = vec![0u8; (end - at).min(16 << 20) as usize];
            let n = self.client.read_range(&self.account, id, at, &mut buf).await?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
            at += n as u64;
        }
        crate::on_demand::check_still(&self.client, &self.account, id, expect).await?;
        Ok(out)
    }

    async fn trash(&self, id: &str) -> Result<()> {
        self.client.trash(&self.account, id).await?;
        self.cache.forget(&self.account, id).await
    }

    async fn delete_permanently(&self, id: &str) -> Result<()> {
        // Only bin entries are deleted for good, and a bin entry's file must be in Drive's bin: a
        // row that names a live file (an edited import, a file restored in Drive since) never
        // deletes it.
        match self.client.get_file(&self.account, id).await {
            Err(wasl_drive::DriveError::NotFound(_)) => return Ok(()),
            Ok(f) if !f.trashed => {
                return Err(wasl_drive::DriveError::Http {
                    code: 409,
                    reason: Some("notTrashed".into()),
                    message: format!("{} is not in Drive's bin, so it was left alone", f.name),
                });
            }
            other => {
                other?;
            }
        }
        match self.client.delete_permanently(&self.account, id).await {
            // Already gone is what we wanted.
            Err(wasl_drive::DriveError::NotFound(_)) => {}
            other => other?,
        }
        self.cache.forget(&self.account, id).await
    }

    async fn list_revisions(&self, id: &str) -> Result<Vec<Revision>> {
        Ok(self
            .client
            .list_revisions(&self.account, id)
            .await?
            .into_iter()
            .map(|r| Revision {
                modified_ms: parse_rfc3339(r.modified_time.as_deref()).unwrap_or(0),
                size: r.size.as_deref().and_then(|s| s.parse().ok()),
                md5: r.md5_checksum,
                keep_forever: r.keep_forever,
                modified_by: r
                    .last_modifying_user
                    .and_then(|u| u.display_name.or(u.email_address)),
                id: r.id,
            })
            .collect())
    }

    async fn download_revision(&self, id: &str, revision_id: &str, dest: &Path) -> Result<()> {
        self.client
            .download_revision(&self.account, id, revision_id, dest, |_, _| {})
            .await
    }

    async fn keep_revision(&self, id: &str, revision_id: &str, keep: bool) -> Result<()> {
        self.client
            .keep_revision(&self.account, id, revision_id, keep)
            .await
    }

    async fn untrash(&self, id: &str) -> Result<()> {
        self.client.untrash(&self.account, id).await?;
        let f = self.client.get_file(&self.account, id).await?;
        self.cache.remember(&self.account, &f).await
    }
}

/// `appProperties` to send: none when there are none.
fn property_map(properties: &[(&str, &str)]) -> Option<std::collections::BTreeMap<String, String>> {
    (!properties.is_empty()).then(|| {
        properties
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    })
}
