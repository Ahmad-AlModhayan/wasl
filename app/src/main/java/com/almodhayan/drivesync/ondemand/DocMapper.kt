package com.almodhayan.drivesync.ondemand

import com.almodhayan.drivesync.core.NameMapper
import com.almodhayan.drivesync.data.db.DriveNodeDao
import com.almodhayan.drivesync.data.db.DriveNodeEntity
import com.almodhayan.drivesync.drive.DocsExport
import com.almodhayan.drivesync.drive.FOLDER_MIME
import com.almodhayan.drivesync.drive.GOOGLE_APPS_PREFIX
import com.almodhayan.drivesync.drive.SHORTCUT_MIME

enum class DocKind { FOLDER, FILE, GOOGLE_DOC }

/**
 * Everything the provider needs to describe one Drive node as a SAF document. Produced by
 * [DocMapper] from a [DriveNodeEntity]; pure data so the mapping is unit-tested without Android.
 */
data class MappedDoc(
    /** Id used in the document id. For shortcuts this is the TARGET id, so opening resolves the real file. */
    val fileId: String,
    /** What the Files app shows; exported Google Docs carry the export extension ("Budget.xlsx"). */
    val displayName: String,
    val mimeType: String,
    val kind: DocKind,
    val size: Long?,
    val lastModified: Long?,
    val parentId: String?,
    /** Drive's own name (no export extension); what a rename must be based on. */
    val driveName: String,
    /** Export format for Google-native docs, null otherwise. */
    val exportTarget: DocsExport.Target?,
    /** Content identity used in cache keys: md5 for binaries, "v<version>.<ext>" for exports, "" for folders. */
    val cacheTag: String,
    val md5: String?,
) {
    val isFolder: Boolean get() = kind == DocKind.FOLDER
    /** Google-native docs are exported snapshots; edits cannot be written back. */
    val readOnly: Boolean get() = kind == DocKind.GOOGLE_DOC
}

object DocMapper {
    /** Same value as DocumentsContract.Document.MIME_TYPE_DIR; kept literal so this file stays JVM-only. */
    const val DIR_MIME = "vnd.android.document/directory"
    const val OCTET_STREAM = "application/octet-stream"

    fun isGoogleDoc(mime: String): Boolean =
        mime.startsWith(GOOGLE_APPS_PREFIX) && mime != FOLDER_MIME && mime != SHORTCUT_MIME

    /** Tag for a regular file: Drive's md5 when it has one, else the version counter. */
    fun fileCacheTag(node: DriveNodeEntity): String = fileCacheTag(node.md5, node.version, node.modifiedTime)

    fun fileCacheTag(md5: String?, version: Long?, modifiedTime: Long): String =
        md5?.lowercase() ?: "v${version ?: modifiedTime}"

    /**
     * Tag for an export. Exports have no md5 and change whenever the doc changes (version bumps), and
     * a different export format must not hit the same cached bytes, hence the extension suffix.
     * Uses '.' rather than '-' so [CachePaths.parseFileKey] can split "<fileId>-<tag>" at the last '-'.
     */
    fun exportCacheTag(version: Long?, modifiedTime: Long, target: DocsExport.Target): String =
        "v${version ?: modifiedTime}.${target.extension}"

    /**
     * Maps one node. Returns null for nodes that must not be listed: trashed nodes, Google-native
     * types that cannot be exported (Forms, Sites, Maps, ...) and dangling shortcuts. [resolveShortcut]
     * looks a shortcut's target up in the local index; it is never given network access here so
     * listings stay fast and work offline.
     */
    fun map(node: DriveNodeEntity, resolveShortcut: (String) -> DriveNodeEntity? = { null }): MappedDoc? {
        if (node.trashed) return null
        val mtime = node.modifiedTime.takeIf { it > 0 }
        if (node.isShortcut || node.mimeType == SHORTCUT_MIME) {
            val targetId = node.shortcutTargetId ?: return null
            val target = resolveShortcut(targetId)
            if (target != null) {
                // Nested shortcuts are not followed (a shortcut to a shortcut is not something Drive creates).
                val m = map(target) { null } ?: return null
                val name = m.exportTarget?.let { DocsExport.localName(node.name, it) } ?: node.name
                return m.copy(displayName = display(name), parentId = node.parentId)
            }
            val targetMime = node.shortcutTargetMime ?: OCTET_STREAM
            return when {
                targetMime == FOLDER_MIME ->
                    MappedDoc(targetId, display(node.name), DIR_MIME, DocKind.FOLDER, null, mtime, node.parentId, node.name, null, "", null)
                isGoogleDoc(targetMime) -> {
                    val t = DocsExport.target(targetMime, preferPdf = false) ?: return null
                    MappedDoc(targetId, display(DocsExport.localName(node.name, t)), t.mime, DocKind.GOOGLE_DOC, null, mtime, node.parentId, node.name, t, "v0.${t.extension}", null)
                }
                else ->
                    MappedDoc(targetId, display(node.name), targetMime, DocKind.FILE, null, mtime, node.parentId, node.name, null, "v0", null)
            }
        }
        if (node.isFolder || node.mimeType == FOLDER_MIME) {
            return MappedDoc(node.id, display(node.name), DIR_MIME, DocKind.FOLDER, null, mtime, node.parentId, node.name, null, "", null)
        }
        if (isGoogleDoc(node.mimeType)) {
            val t = DocsExport.target(node.mimeType, preferPdf = false) ?: return null
            return MappedDoc(
                node.id, display(DocsExport.localName(node.name, t)), t.mime, DocKind.GOOGLE_DOC, null, mtime, node.parentId,
                node.name, t, exportCacheTag(node.version, node.modifiedTime, t), null,
            )
        }
        return MappedDoc(
            node.id, display(node.name), node.mimeType.ifBlank { OCTET_STREAM }, DocKind.FILE, node.size, mtime, node.parentId,
            node.name, null, fileCacheTag(node), node.md5,
        )
    }

    /**
     * Name to send to Drive when the user renames a document. For an exported Google Doc the
     * displayed name carries the export extension; typing "Budget 2027.xlsx" must rename the sheet
     * to "Budget 2027", otherwise the next listing would show "Budget 2027.xlsx" for a doc actually
     * called "Budget 2027.xlsx" and a later export would not add a second extension anyway.
     */
    fun driveNameForRename(doc: MappedDoc, newDisplayName: String): String {
        val t = doc.exportTarget ?: return newDisplayName
        val suffix = "." + t.extension
        return if (newDisplayName.length > suffix.length && newDisplayName.lowercase().endsWith(suffix)) {
            newDisplayName.dropLast(suffix.length)
        } else newDisplayName
    }

    /** SAF clients build paths from display names, so keep them FAT/FUSE-safe like the sync engine does. */
    private fun display(name: String): String = NameMapper.toLocalName(name)
}

/**
 * Pre-resolves the shortcut targets among [nodes] from the local index (suspend DAO calls) and
 * returns the plain lookup [DocMapper.map] expects. Non-shortcut nodes cost nothing.
 */
suspend fun DriveNodeDao.shortcutResolver(accountId: String, nodes: Iterable<DriveNodeEntity>): (String) -> DriveNodeEntity? {
    var targets: HashMap<String, DriveNodeEntity>? = null
    for (n in nodes) {
        if (!n.isShortcut && n.mimeType != SHORTCUT_MIME) continue
        val id = n.shortcutTargetId ?: continue
        val map = targets ?: HashMap<String, DriveNodeEntity>().also { targets = it }
        if (map.containsKey(id)) continue
        get(accountId, id)?.let { map[id] = it }
    }
    val resolved: Map<String, DriveNodeEntity> = targets ?: emptyMap()
    return { id -> resolved[id] }
}
