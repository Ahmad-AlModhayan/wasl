//! Drive v3 wire types (port of `DriveModels.kt`). Unknown JSON fields are ignored.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
pub const SHORTCUT_MIME: &str = "application/vnd.google-apps.shortcut";
pub const GOOGLE_APPS_PREFIX: &str = "application/vnd.google-apps.";

/// Fields requested for every file resource. Quota cost is per request, not per field, but payload
/// size matters, so keep it lean.
pub const FILE_FIELDS: &str = "id,name,mimeType,parents,size,md5Checksum,modifiedTime,version,trashed,driveId,shortcutDetails(targetId,targetMimeType),shared,permissionIds,appProperties";

/// A file resource. Drive sends int64 values (`size`, `version`) as strings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveFile {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mime_type: String,
    #[serde(default)]
    pub parents: Option<Vec<String>>,
    #[serde(default)]
    pub size: Option<String>,
    #[serde(default)]
    pub md5_checksum: Option<String>,
    #[serde(default)]
    pub modified_time: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub trashed: bool,
    #[serde(default)]
    pub drive_id: Option<String>,
    #[serde(default)]
    pub shortcut_details: Option<ShortcutDetails>,
    #[serde(default)]
    pub shared: bool,
    #[serde(default)]
    pub permission_ids: Option<Vec<String>>,
    /// Key/value pairs only this app sees (Drive `appProperties`).
    #[serde(default)]
    pub app_properties: Option<BTreeMap<String, String>>,
}

impl DriveFile {
    pub fn is_folder(&self) -> bool {
        self.mime_type == FOLDER_MIME
    }
    pub fn is_shortcut(&self) -> bool {
        self.mime_type == SHORTCUT_MIME
    }
    /// A Google-native document (Docs, Sheets, ...) that has no binary content and must be exported.
    pub fn is_google_doc(&self) -> bool {
        self.mime_type.starts_with(GOOGLE_APPS_PREFIX) && !self.is_folder() && !self.is_shortcut()
    }
    pub fn size_u64(&self) -> Option<u64> {
        self.size.as_deref().and_then(|s| s.parse().ok())
    }
    pub fn version_u64(&self) -> Option<u64> {
        self.version.as_deref().and_then(|s| s.parse().ok())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ShortcutDetails {
    pub target_id: Option<String>,
    pub target_mime_type: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FileList {
    pub next_page_token: Option<String>,
    pub files: Vec<DriveFile>,
    pub incomplete_search: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartPageToken {
    pub start_page_token: String,
}

/// One entry of the Changes feed. `change_type` is `"file"` or `"drive"`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Change {
    pub change_type: Option<String>,
    pub file_id: Option<String>,
    pub removed: bool,
    pub file: Option<DriveFile>,
    pub drive_id: Option<String>,
    pub time: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChangeList {
    pub next_page_token: Option<String>,
    pub new_start_page_token: Option<String>,
    pub changes: Vec<Change>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AboutUser {
    pub email_address: Option<String>,
    pub display_name: Option<String>,
    pub photo_link: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StorageQuota {
    pub limit: Option<String>,
    pub usage: Option<String>,
    pub usage_in_drive: Option<String>,
    pub usage_in_drive_trash: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct About {
    pub user: Option<AboutUser>,
    pub storage_quota: Option<StorageQuota>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct SharedDrive {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct DriveList {
    pub next_page_token: Option<String>,
    pub drives: Vec<SharedDrive>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RevisionUser {
    pub display_name: Option<String>,
    pub email_address: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Revision {
    pub id: String,
    #[serde(default)]
    pub modified_time: Option<String>,
    #[serde(default)]
    pub size: Option<String>,
    #[serde(default)]
    pub md5_checksum: Option<String>,
    #[serde(default)]
    pub keep_forever: bool,
    #[serde(default)]
    pub original_filename: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub last_modifying_user: Option<RevisionUser>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct RevisionList {
    pub next_page_token: Option<String>,
    pub revisions: Vec<Revision>,
}

/// Metadata of a file being created. Absent fields are omitted from the JSON.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewFileMetadata {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parents: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_properties: Option<BTreeMap<String, String>>,
}

/// A metadata patch. Only the fields that are set are sent.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchFileMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trashed: Option<bool>,
    /// RFC 3339, see [`crate::to_rfc3339`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_time: Option<String>,
    /// Keys set here replace those keys only; others stay.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_properties: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PatchRevision {
    pub keep_forever: bool,
}

/// Export formats for Google-native documents.
pub mod docs_export {
    /// The MIME type a Google document is exported to and the local file extension.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Target {
        pub mime: &'static str,
        pub extension: &'static str,
    }

    const fn t(mime: &'static str, extension: &'static str) -> Target {
        Target { mime, extension }
    }

    /// The export target of `google_mime`, office formats unless `prefer_pdf`.
    pub fn target(google_mime: &str, prefer_pdf: bool) -> Option<Target> {
        let pdf = t("application/pdf", "pdf");
        let script = t("application/vnd.google-apps.script+json", "json");
        Some(match (google_mime, prefer_pdf) {
            ("application/vnd.google-apps.script", _) => script,
            ("application/vnd.google-apps.jam", _) => pdf,
            (
                "application/vnd.google-apps.document"
                | "application/vnd.google-apps.spreadsheet"
                | "application/vnd.google-apps.presentation"
                | "application/vnd.google-apps.drawing",
                true,
            ) => pdf,
            ("application/vnd.google-apps.document", false) => t(
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                "docx",
            ),
            ("application/vnd.google-apps.spreadsheet", false) => t(
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                "xlsx",
            ),
            ("application/vnd.google-apps.presentation", false) => t(
                "application/vnd.openxmlformats-officedocument.presentationml.presentation",
                "pptx",
            ),
            ("application/vnd.google-apps.drawing", false) => t("image/png", "png"),
            _ => return None,
        })
    }

    /// Local file name of an exported doc: "Budget 2026.xlsx". An existing extension is kept.
    pub fn local_name(name: &str, target: Target) -> String {
        if name.to_lowercase().ends_with(&format!(".{}", target.extension)) {
            name.to_string()
        } else {
            format!("{name}.{}", target.extension)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn targets_and_local_names() {
            let sheet = target("application/vnd.google-apps.spreadsheet", false).unwrap();
            assert_eq!("xlsx", sheet.extension);
            assert_eq!("Budget 2026.xlsx", local_name("Budget 2026", sheet));
            assert_eq!("Budget.XLSX", local_name("Budget.XLSX", sheet));
            let pdf = target("application/vnd.google-apps.document", true).unwrap();
            assert_eq!("application/pdf", pdf.mime);
            assert_eq!(
                "json",
                target("application/vnd.google-apps.script", true)
                    .unwrap()
                    .extension
            );
            assert!(target("application/vnd.google-apps.form", false).is_none());
        }
    }
}
