package com.almodhayan.drivesync.ondemand

import android.util.Log
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Hashing
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.drive.DriveApi
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.io.File
import java.util.concurrent.ConcurrentHashMap
import javax.inject.Inject
import javax.inject.Singleton

private const val TAG = "OnDemand"

/**
 * Brings a complete copy of a document into [OnDemandCache]: download for regular files (md5
 * verified when Drive reports one), export for Google-native docs. Shared by the provider (opening
 * a file) and [PinManager] (keeping pins offline) so two callers wanting the same content share one
 * transfer instead of racing.
 */
@Singleton
class ContentFetcher @Inject constructor(
    private val api: DriveApi,
    private val cache: OnDemandCache,
) {
    private val locks = ConcurrentHashMap<String, Mutex>()

    suspend fun ensureFull(account: String, doc: MappedDoc, pinned: Boolean, onProgress: (Long, Long?) -> Unit = { _, _ -> }): File {
        require(!doc.isFolder) { "${doc.displayName} is a folder" }
        val key = "$account|${doc.fileId}|${doc.cacheTag}"
        val mutex = locks.getOrPut(key) { Mutex() }
        try {
            return mutex.withLock {
                cache.fullFile(account, doc.fileId, doc.cacheTag) ?: fetch(account, doc, pinned, onProgress)
            }
        } finally {
            locks.remove(key, mutex)
        }
    }

    private suspend fun fetch(account: String, doc: MappedDoc, pinned: Boolean, onProgress: (Long, Long?) -> Unit): File {
        val temp = cache.newTempFile(account)
        try {
            if (doc.kind == DocKind.GOOGLE_DOC) {
                val target = doc.exportTarget ?: throw AppError.Unknown(Strings.get(R.string.ondemand_fetch_no_export_format, doc.displayName))
                api.export(account, doc.fileId, target.mime, temp, onProgress)
            } else {
                api.download(account, doc.fileId, temp, doc.size, onProgress)
                val expected = doc.md5
                if (expected != null) {
                    val actual = withContext(Dispatchers.IO) { Hashing.md5(temp) }
                    if (!actual.equals(expected, ignoreCase = true)) {
                        throw AppError.Integrity("${doc.displayName}: downloaded content hashes to $actual, Drive says $expected")
                    }
                }
            }
            // No file names in logcat, release builds included (security review 2026-10).
            Log.i(TAG, "cached a file (${temp.length()} bytes${if (pinned) ", pinned" else ""})")
            return cache.commitFull(account, doc.fileId, doc.cacheTag, temp, pinned)
        } finally {
            if (temp.exists()) temp.delete()
        }
    }
}
