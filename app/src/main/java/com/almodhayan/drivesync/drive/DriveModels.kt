package com.almodhayan.drivesync.drive

import kotlinx.serialization.Serializable

const val FOLDER_MIME = "application/vnd.google-apps.folder"
const val SHORTCUT_MIME = "application/vnd.google-apps.shortcut"
const val GOOGLE_APPS_PREFIX = "application/vnd.google-apps."

/** Fields requested for every file resource. Keep lean: quota cost is per request, not per field, but payload size matters. */
const val FILE_FIELDS = "id,name,mimeType,parents,size,md5Checksum,modifiedTime,version,trashed,driveId,shortcutDetails(targetId,targetMimeType),shared,permissionIds"

@Serializable
data class DriveFile(
    val id: String,
    val name: String = "",
    val mimeType: String = "",
    val parents: List<String>? = null,
    val size: String? = null,                // Drive returns int64 as string
    val md5Checksum: String? = null,
    val modifiedTime: String? = null,
    val version: String? = null,
    val trashed: Boolean = false,
    val driveId: String? = null,
    val shortcutDetails: ShortcutDetails? = null,
    val shared: Boolean = false,
    val permissionIds: List<String>? = null,
) {
    val isFolder: Boolean get() = mimeType == FOLDER_MIME
    val isShortcut: Boolean get() = mimeType == SHORTCUT_MIME
    val isGoogleDoc: Boolean get() = mimeType.startsWith(GOOGLE_APPS_PREFIX) && !isFolder && !isShortcut
    val sizeLong: Long? get() = size?.toLongOrNull()
    val versionLong: Long? get() = version?.toLongOrNull()
}

@Serializable data class ShortcutDetails(val targetId: String? = null, val targetMimeType: String? = null)

@Serializable data class FileList(val nextPageToken: String? = null, val files: List<DriveFile> = emptyList(), val incompleteSearch: Boolean = false)

@Serializable data class StartPageToken(val startPageToken: String)

@Serializable
data class Change(
    val changeType: String? = null,          // "file" | "drive"
    val fileId: String? = null,
    val removed: Boolean = false,
    val file: DriveFile? = null,
    val driveId: String? = null,
    val time: String? = null,
)

@Serializable data class ChangeList(val nextPageToken: String? = null, val newStartPageToken: String? = null, val changes: List<Change> = emptyList())

@Serializable data class AboutUser(val emailAddress: String? = null, val displayName: String? = null, val photoLink: String? = null)
@Serializable data class StorageQuota(val limit: String? = null, val usage: String? = null, val usageInDrive: String? = null, val usageInDriveTrash: String? = null)
@Serializable data class About(val user: AboutUser? = null, val storageQuota: StorageQuota? = null)

@Serializable data class SharedDrive(val id: String, val name: String = "")
@Serializable data class DriveList(val nextPageToken: String? = null, val drives: List<SharedDrive> = emptyList())

@Serializable data class RevisionUser(val displayName: String? = null, val emailAddress: String? = null)
@Serializable
data class Revision(
    val id: String,
    val modifiedTime: String? = null,
    val size: String? = null,
    val md5Checksum: String? = null,
    val keepForever: Boolean = false,
    val originalFilename: String? = null,
    val mimeType: String? = null,
    val lastModifyingUser: RevisionUser? = null,
)
@Serializable data class RevisionList(val nextPageToken: String? = null, val revisions: List<Revision> = emptyList())

@Serializable data class ApiErrorItem(val domain: String? = null, val reason: String? = null, val message: String? = null)
@Serializable data class ApiErrorBody(val code: Int? = null, val message: String? = null, val errors: List<ApiErrorItem>? = null, val status: String? = null)
@Serializable data class ApiErrorEnvelope(val error: ApiErrorBody? = null)

@Serializable data class NewFileMetadata(val name: String, val mimeType: String? = null, val parents: List<String>? = null, val modifiedTime: String? = null)
@Serializable data class PatchFileMetadata(val name: String? = null, val trashed: Boolean? = null, val modifiedTime: String? = null)
@Serializable data class PatchRevision(val keepForever: Boolean)

/** Export formats for Google-native documents. */
object DocsExport {
    data class Target(val mime: String, val extension: String)

    private val office = mapOf(
        "application/vnd.google-apps.document" to Target("application/vnd.openxmlformats-officedocument.wordprocessingml.document", "docx"),
        "application/vnd.google-apps.spreadsheet" to Target("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", "xlsx"),
        "application/vnd.google-apps.presentation" to Target("application/vnd.openxmlformats-officedocument.presentationml.presentation", "pptx"),
        "application/vnd.google-apps.drawing" to Target("image/png", "png"),
        "application/vnd.google-apps.script" to Target("application/vnd.google-apps.script+json", "json"),
        "application/vnd.google-apps.jam" to Target("application/pdf", "pdf"),
    )
    private val pdf = mapOf(
        "application/vnd.google-apps.document" to Target("application/pdf", "pdf"),
        "application/vnd.google-apps.spreadsheet" to Target("application/pdf", "pdf"),
        "application/vnd.google-apps.presentation" to Target("application/pdf", "pdf"),
        "application/vnd.google-apps.drawing" to Target("application/pdf", "pdf"),
        "application/vnd.google-apps.script" to Target("application/vnd.google-apps.script+json", "json"),
        "application/vnd.google-apps.jam" to Target("application/pdf", "pdf"),
    )

    fun target(googleMime: String, preferPdf: Boolean): Target? = (if (preferPdf) pdf else office)[googleMime]

    /** Local file name for an exported doc: "Budget 2026.xlsx". Existing extension is not duplicated. */
    fun localName(name: String, target: Target): String =
        if (name.lowercase().endsWith("." + target.extension)) name else "$name.${target.extension}"
}
