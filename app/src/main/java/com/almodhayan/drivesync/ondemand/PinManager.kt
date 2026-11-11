package com.almodhayan.drivesync.ondemand

import android.content.Context
import android.util.Log
import androidx.work.Constraints
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Hashing
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.DriveNodeEntity
import com.almodhayan.drivesync.data.db.PinEntity
import com.almodhayan.drivesync.drive.DriveApi
import com.almodhayan.drivesync.drive.DriveNodeCache
import com.almodhayan.drivesync.drive.DriveNodeCache.Companion.toNode
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import javax.inject.Inject
import javax.inject.Singleton

private const val TAG = "OnDemand"

/**
 * Keeps pinned Drive items (PinEntity rows written by the UI) available offline and pushes local
 * edits made through the provider back to Drive.
 *
 * - [start] reacts to [PinRequests.changes] by scheduling a [PinWorker] run.
 * - [syncPins] (run by the worker) downloads every pinned file, and every file under a pinned
 *   folder, into the full-file cache when missing or when Drive's md5/version changed; copies of
 *   items no longer pinned become ordinary LRU entries that go when the cache needs space.
 * - [uploadPending] / [flushPendingUploads] finish write sessions whose upload failed (offline).
 */
@Singleton
class PinManager @Inject constructor(
    @ApplicationContext private val appContext: Context,
    private val db: Db,
    private val api: DriveApi,
    private val nodeCache: DriveNodeCache,
    private val cache: OnDemandCache,
    private val fetcher: ContentFetcher,
    private val pinRequests: PinRequests,
) {
    data class Result(
        val checked: Int,
        val downloaded: Int,
        val failed: Int,
        val uploaded: Int,
        /** True when at least one failure was transient (network, rate limit): the worker retries. */
        val transient: Boolean,
    )

    /** Bumped by every [schedule]; a running sync loops until it has seen the latest value, so a pin added mid-run is not missed under ExistingWorkPolicy.KEEP. */
    private val generation = AtomicInteger()
    private val uploadLocks = ConcurrentHashMap<String, Mutex>()
    private val _syncing = MutableStateFlow(false)
    /** For the UI: true while a pin sync runs. */
    val syncing: StateFlow<Boolean> = _syncing

    /** Call once from Application.onCreate: every pin change made by the UI schedules a sync. */
    fun start(scope: CoroutineScope) {
        scope.launch {
            pinRequests.changes.collect {
                try { schedule(appContext) } catch (t: Throwable) { Log.w(TAG, "pin sync not scheduled: ${t.message}") }
            }
        }
    }

    /** One-off sync as soon as the device is connected. */
    fun schedule(context: Context = appContext) {
        generation.incrementAndGet()
        val req = OneTimeWorkRequestBuilder<PinWorker>()
            .setConstraints(connected())
            .addTag(TAG_PINS)
            .build()
        WorkManager.getInstance(context.applicationContext).enqueueUniqueWork(WORK_NOW, ExistingWorkPolicy.KEEP, req)
    }

    /** Refreshes pinned content every 6 hours (new Drive versions, evicted copies). */
    fun ensurePeriodic(context: Context = appContext) {
        val req = PeriodicWorkRequestBuilder<PinWorker>(6, TimeUnit.HOURS)
            .setConstraints(connected())
            .addTag(TAG_PINS)
            .build()
        WorkManager.getInstance(context.applicationContext).enqueueUniquePeriodicWork(WORK_PERIODIC, ExistingPeriodicWorkPolicy.KEEP, req)
    }

    fun cancelPeriodic(context: Context = appContext) {
        WorkManager.getInstance(context.applicationContext).cancelUniqueWork(WORK_PERIODIC)
    }

    /** True when the file can be opened without a network connection (cached copy or unsent edit). */
    fun isAvailableOffline(accountId: String, fileId: String): Boolean = cache.hasAnyFull(accountId, fileId)

    suspend fun syncPins(): Result {
        _syncing.value = true
        try {
            val c = Counters()
            do {
                val seen = generation.get()
                pass(c)
            } while (seen != generation.get())
            cache.flush()
            return Result(c.checked, c.downloaded, c.failed, c.uploaded, c.transient)
        } finally {
            _syncing.value = false
        }
    }

    private class Counters { var checked = 0; var downloaded = 0; var failed = 0; var uploaded = 0; var transient = false }

    private suspend fun pass(c: Counters) {
        c.uploaded += flushPendingUploads()
        for (acct in db.accounts().getAll()) {
            currentCoroutineContext().ensureActive()
            val pins = db.pins().forAccount(acct.id)
            if (pins.isEmpty()) { cache.setPinned(acct.id, emptySet()); continue }
            try {
                nodeCache.refresh(acct.id)   // pinned folders must reflect Drive's current contents
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                val err = AppError.wrap(t)
                Log.w(TAG, "Drive index refresh failed for ${acct.id}, pinning from the cached index: ${err.message}")
                if (err.isTransient) c.transient = true
            }
            val wanted = LinkedHashMap<String, MappedDoc>()
            for (pin in pins) collect(acct.id, pin, wanted)
            cache.setPinned(acct.id, wanted.values.map { CacheEntry.fileKey(it.fileId, it.cacheTag) }.toSet())
            for (doc in wanted.values) {
                currentCoroutineContext().ensureActive()
                c.checked++
                if (cache.hasFull(acct.id, doc.fileId, doc.cacheTag)) continue
                try {
                    fetcher.ensureFull(acct.id, doc, pinned = true)
                    c.downloaded++
                } catch (e: CancellationException) {
                    throw e
                } catch (t: Throwable) {
                    val err = AppError.wrap(t)
                    c.failed++
                    if (err.isTransient) c.transient = true
                    // The kind only: names and messages can carry file names (security review 2026-10).
                    Log.w(TAG, "pin download failed: ${err.javaClass.simpleName}")
                }
            }
            val now = System.currentTimeMillis()
            for (pin in pins) db.pins().touch(acct.id, pin.fileId, now)
        }
        Log.i(TAG, "pin sync: ${c.checked} checked, ${c.downloaded} downloaded, ${c.failed} failed, ${c.uploaded} edits uploaded")
    }

    private suspend fun collect(accountId: String, pin: PinEntity, out: MutableMap<String, MappedDoc>) {
        val node = db.nodes().get(accountId, pin.fileId) ?: try {
            api.getFile(accountId, pin.fileId).also { nodeCache.remember(accountId, it) }.toNode(accountId)
        } catch (e: CancellationException) {
            throw e
        } catch (t: Throwable) {
            Log.w(TAG, "pinned item ${pin.fileId} not found: ${AppError.wrap(t).message}")
            return
        }
        walk(accountId, node, out, depth = 0)
    }

    private suspend fun walk(accountId: String, node: DriveNodeEntity, out: MutableMap<String, MappedDoc>, depth: Int) {
        if (depth > 64 || node.trashed) return
        val nodes = db.nodes()
        val doc = DocMapper.map(node, nodes.shortcutResolver(accountId, listOf(node))) ?: return
        if (doc.isFolder) {
            for (child in nodes.children(accountId, doc.fileId)) walk(accountId, child, out, depth + 1)
        } else {
            out.putIfAbsent(doc.fileId, doc)
        }
    }

    // ---------------------------------------------------------------- write-back of local edits

    /** Uploads every edit whose upload failed earlier (device was offline when the app closed the file). */
    suspend fun flushPendingUploads(): Int {
        var n = 0
        for ((account, fileId) in cache.pendingUploads()) {
            currentCoroutineContext().ensureActive()
            if (uploadPending(account, fileId)) n++
        }
        return n
    }

    /**
     * Serialises work on one file's scratch copy: the provider prepares a write session under this
     * lock so it never starts while [uploadPending] is moving that copy into the cache. Not reentrant.
     */
    suspend fun <T> withFileLock(accountId: String, fileId: String, block: suspend () -> T): T =
        uploadLocks.getOrPut("$accountId|$fileId") { Mutex() }.withLock { block() }

    /**
     * Uploads the pending work copy of [fileId] as the new content of the existing Drive file and,
     * when Drive's md5 matches, adopts that copy as the cached version so the next open is local.
     * Returns true when nothing is pending anymore (uploaded, or nothing to do).
     */
    suspend fun uploadPending(accountId: String, fileId: String): Boolean {
        return withFileLock(accountId, fileId) {
            val work = cache.pendingWork(accountId, fileId) ?: return@withFileLock true
            try {
                val node = db.nodes().get(accountId, fileId)
                val name = node?.name ?: fileId
                val mime = node?.mimeType?.takeIf { it.isNotBlank() && !DocMapper.isGoogleDoc(it) } ?: DocMapper.OCTET_STREAM
                val result = api.upload(
                    account = accountId, src = work, name = name, parentId = null, existingFileId = fileId,
                    modifiedTime = work.lastModified(), mimeType = mime,
                )
                nodeCache.remember(accountId, result.file)
                val uploaded = result.file
                val localMd5 = withContext(Dispatchers.IO) { Hashing.md5(work) }
                val remoteMd5 = uploaded.md5Checksum
                if (remoteMd5 == null || remoteMd5.equals(localMd5, ignoreCase = true)) {
                    val tag = DocMapper.fileCacheTag(remoteMd5 ?: localMd5, uploaded.versionLong, work.lastModified())
                    val pinned = db.pins().get(accountId, fileId) != null || cache.isPinned(accountId, fileId)
                    cache.adoptWorkAsFull(accountId, fileId, tag, pinned)
                } else {
                    Log.w(TAG, "Drive reports a different md5 after upload than the local copy has; dropping the local copy")
                    cache.dropFile(accountId, fileId)
                    cache.discardWork(accountId, fileId)
                }
                Log.i(TAG, "uploaded an edit (${work.length()} bytes)")   // no file name (security review 2026-10)
                true
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                val err = AppError.wrap(t)
                if (err is AppError.NotFound) {
                    Log.w(TAG, "edit of $fileId dropped: the file no longer exists on Drive")
                    cache.discardWork(accountId, fileId)
                    return@withFileLock true
                }
                Log.w(TAG, "upload of edited $fileId failed, will retry: ${err.message}")
                false
            }
        }
    }

    private fun connected() = Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build()

    companion object {
        const val WORK_NOW = "ondemand-pins-now"
        const val WORK_PERIODIC = "ondemand-pins-periodic"
        const val TAG_PINS = "ondemand-pins"
    }
}
