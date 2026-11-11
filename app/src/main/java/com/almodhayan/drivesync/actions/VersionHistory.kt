package com.almodhayan.drivesync.actions

import android.os.Environment
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.core.TimeFmt
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.drive.DriveApi
import com.almodhayan.drivesync.engine.WaslRuntime
import com.kahflabs.wasl.ffi.engine.VersionRef as WVersionRef
import com.kahflabs.wasl.ffi.engine.VersionSource as WVersionSource
import com.almodhayan.drivesync.fs.LocalFs
import com.almodhayan.drivesync.scheduler.SyncScheduler
import com.almodhayan.drivesync.ui.components.VersionSource
import java.io.File
import javax.inject.Inject
import javax.inject.Singleton

sealed interface VersionRef {
    data class Drive(val revisionId: String) : VersionRef
    data class Local(val trashId: String) : VersionRef
}

data class VersionEntry(val ref: VersionRef, val at: Long, val source: VersionSource, val size: Long, val note: String?, val current: Boolean, val keepForever: Boolean = false)

data class FileVersions(
    val name: String,
    val path: String,              // human path for the header card
    val pairName: String?,
    val size: Long?,
    val entries: List<VersionEntry>,
    val canRestore: Boolean,       // the file belongs to a pair, so a version can be made current on both sides
)

/**
 * A file's timeline, newest first: Drive revisions (30 days or 100 versions) merged with the last
 * versions this phone overwrote. Restore makes a version current on the phone now and on Drive at the
 * next sync, and keeps the replaced one. Save as copy adds it beside the file.
 */
@Singleton
class VersionHistory @Inject constructor(
    private val db: Db,
    private val api: DriveApi,
    private val local: LocalFs,
    private val runtime: WaslRuntime,
    private val scheduler: SyncScheduler,
) {
    suspend fun load(accountId: String, fileId: String, name: String, pairId: String?, key: String?): FileVersions {
        val pair = pairId?.let { db.pairs().get(it) }
        if (pair != null && key != null) {
            // A synced file: the engine merges Drive revisions with the copies this phone replaced.
            val v = runtime.engine.fileVersions(pair.id, key)
            return FileVersions(
                name = v.name,
                path = v.path.replace("/", " / "),
                pairName = v.pairName,
                size = v.size?.toLong(),
                entries = v.entries.map { e ->
                    VersionEntry(
                        ref = when (val r = e.version) {
                            is WVersionRef.Drive -> VersionRef.Drive(r.revisionId)
                            is WVersionRef.Local -> VersionRef.Local(r.binEntryId)
                        },
                        at = e.at,
                        source = when (e.source) {
                            WVersionSource.DEVICE -> VersionSource.Phone
                            WVersionSource.DRIVE -> VersionSource.Drive
                            WVersionSource.CONFLICT -> VersionSource.Conflict
                        },
                        size = e.size.toLong(),
                        note = e.note,
                        current = e.current,
                        keepForever = e.keepForever,
                    )
                },
                canRestore = true,
            )
        }
        // A file browsed on Drive outside any pair: its Drive revisions only.
        val revisions = runCatching { api.listRevisions(accountId, fileId) }.getOrElse { if (it is AppError.NotFound) emptyList() else throw AppError.wrap(it) }
        val entries = revisions.map { r ->
            VersionEntry(VersionRef.Drive(r.id), TimeFmt.parseRfc3339(r.modifiedTime) ?: 0L, VersionSource.Drive, r.size?.toLongOrNull() ?: 0L,
                note = r.lastModifyingUser?.displayName, current = false, keepForever = r.keepForever)
        }.sortedByDescending { it.at }.toMutableList()
        if (entries.isNotEmpty()) entries[0] = entries[0].copy(current = true)
        return FileVersions(name, name, null, entries.firstOrNull()?.size, entries, canRestore = false)
    }

    /** Makes [ref] current: on the phone now, on Drive at the next sync. The replaced version stays in the bin. */
    suspend fun restore(accountId: String, fileId: String, pairId: String, key: String, ref: VersionRef) {
        runtime.engine.restoreVersion(pairId, key, ref.toEngine())
        runtime.changed()
        scheduler.syncNow(pairId, "restore-version", manual = false)
    }

    /** Saves [ref] as "name (version …).ext" beside the file, or in Downloads for files outside a pair. */
    suspend fun saveCopy(accountId: String, fileId: String, name: String, pairId: String?, key: String?, ref: VersionRef, at: Long): File {
        val pair = pairId?.let { db.pairs().get(it) }
        if (pair != null && key != null) {
            val rel = runtime.engine.saveVersionCopy(pair.id, key, ref.toEngine(), at)
            runtime.changed()
            scheduler.syncNow(pair.id, "version-copy", manual = false)
            return File(pair.localPath, rel)
        }
        val dir = Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS)
        dir.mkdirs()
        val dot = name.lastIndexOf('.')
        val base = if (dot > 0) name.substring(0, dot) else name
        val ext = if (dot > 0) name.substring(dot) else ""
        var out = File(dir, "$base (version ${TimeFmt.stamp(at)})$ext")
        var i = 2
        while (out.exists()) out = File(dir, "$base (version ${TimeFmt.stamp(at)}) ($i)$ext").also { i++ }
        val revision = (ref as? VersionRef.Drive)?.revisionId ?: throw AppError.NotFound(Strings.get(R.string.actions_version_not_on_drive))
        val temp = local.tempFor(out)
        api.downloadRevision(accountId, fileId, revision, temp)
        local.commit(temp, out, at)
        return out
    }

    private fun VersionRef.toEngine(): WVersionRef = when (this) {
        is VersionRef.Drive -> WVersionRef.Drive(revisionId)
        is VersionRef.Local -> WVersionRef.Local(trashId)
    }
}
