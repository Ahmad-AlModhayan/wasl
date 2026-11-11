package com.almodhayan.drivesync.ondemand

import android.content.Context
import android.util.Log
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import java.io.File
import java.io.IOException
import java.util.UUID
import java.util.concurrent.locks.ReentrantLock
import javax.inject.Inject
import javax.inject.Singleton
import kotlin.concurrent.withLock

private const val TAG = "OnDemand"
private const val MB = 1024L * 1024L
private const val MIN_LIMIT_MB = 16
private const val PERSIST_INTERVAL_MS = 5_000L

/**
 * Disk cache for the files-on-demand provider, under `cacheDir/ondemand/<accountEmail>/`:
 *
 *  - `full/<fileId>-<tag>`         complete copies (downloads, exports, pinned files)
 *  - `blocks/<fileId>-<tag>/<n>`   1 MiB streaming blocks
 *  - `work/<fileId>[.pending]`     scratch copies an app is editing / edits waiting for upload
 *  - `tmp/`                        in-flight transfers, always written here and renamed into place
 *  - `index.json`                  the [CacheLedger] snapshot, written to a temp file then renamed
 *
 * Total size is bounded by its share of `AppSettings.onDemandCacheMb` ([CacheBudget]: a quarter;
 * the engine's block cache has the rest); when over the limit the least recently
 * used UNPINNED entries go first. Pinned copies are exempt, so pinning more than the limit simply
 * makes the cache larger than the limit. Everything is guarded by one lock; slow disk reads and
 * writes happen outside it (see [readBlock]/[writeBlock]).
 *
 * Crash safety: files are complete before they get an index entry (temp + rename), and on load any
 * file without an entry is re-adopted while any entry without a file is dropped, so a crash between
 * the rename and the index save loses nothing.
 */
@Singleton
class OnDemandCache @Inject constructor(
    @ApplicationContext private val context: Context,
    private val settings: SettingsRepository,
) {
    data class Stats(val totalBytes: Long, val limitBytes: Long, val pinnedBytes: Long, val fullFiles: Int, val blocks: Int)

    private val root: File get() = File(context.cacheDir, "ondemand")
    private val indexFile: File get() = File(root, "index.json")
    private val lock = ReentrantLock()
    private var ledger: CacheLedger? = null
    private var dirty = false
    private var lastPersist = 0L
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    @Volatile
    var limitBytes: Long = 512L * MB
        private set

    init {
        scope.launch {
            try {
                settings.settings.map { it.onDemandCacheMb }.distinctUntilChanged().collect { mb ->
                    limitBytes = CacheBudget.filesBytes(mb.coerceAtLeast(MIN_LIMIT_MB))
                    lock.withLock { enforceLimitLocked(ledgerLocked()); persistLocked(force = true) }
                }
            } catch (t: Throwable) {
                Log.w(TAG, "cache limit setting not observed, keeping ${limitBytes / MB} MB: ${t.message}")
            }
        }
    }

    // ---------------------------------------------------------------- complete files

    /** Complete cached copy of (fileId, tag) or null. Counts as a use for LRU purposes. */
    fun fullFile(account: String, fileId: String, tag: String): File? = lock.withLock {
        val l = ledgerLocked()
        val e = l.get(account, fileId, tag) ?: return null
        val f = fileFor(e)
        if (!f.isFile) { l.remove(e); dirty = true; return null }
        l.touch(e, now()); dirty = true
        persistLocked(force = false)
        f
    }

    /** Size of the complete cached copy without touching LRU (for listings); null when absent. */
    fun fullFileSize(account: String, fileId: String, tag: String): Long? = lock.withLock {
        ledgerLocked().get(account, fileId, tag)?.size
    }

    fun hasFull(account: String, fileId: String, tag: String): Boolean = fullFileSize(account, fileId, tag) != null

    /** True when ANY version of the file is cached completely (or an unsent local edit exists). */
    fun hasAnyFull(account: String, fileId: String): Boolean = lock.withLock {
        ledgerLocked().fullFiles(account, fileId).isNotEmpty()
    } || pendingWork(account, fileId) != null

    fun isPinned(account: String, fileId: String): Boolean = lock.withLock {
        ledgerLocked().fullFiles(account, fileId).any { it.pinned }
    }

    /** Fresh path in the account's tmp dir; the caller fills it and hands it to [commitFull]. */
    fun newTempFile(account: String): File {
        val dir = File(accountDir(account), CachePaths.TMP_DIR)
        dir.mkdirs()
        return File(dir, UUID.randomUUID().toString() + ".part")
    }

    /**
     * Moves the fully written [temp] into place as the complete copy of (fileId, tag). Every other
     * cached item of the same file (older versions, streaming blocks) is dropped: stale or redundant.
     */
    fun commitFull(account: String, fileId: String, tag: String, temp: File, pinned: Boolean): File = lock.withLock {
        val l = ledgerLocked()
        val e = CacheEntry(account, fileId, tag, CacheEntry.FULL, temp.length(), now(), pinned)
        val target = fileFor(e)
        moveInto(temp, target)
        dropVersionsLocked(l, account, fileId, keepTag = tag)
        l.put(e)
        enforceLimitLocked(l, protect = e.id)
        dirty = true
        persistLocked(force = true)
        target
    }

    /** Pins exactly the complete files whose "<fileId>-<tag>" key is in [pinnedKeys]; everything else of the account becomes evictable. */
    fun setPinned(account: String, pinnedKeys: Set<String>) = lock.withLock {
        val l = ledgerLocked()
        if (l.setPinned(account, pinnedKeys) > 0) {
            dirty = true
            enforceLimitLocked(l)
            persistLocked(force = true)
        }
    }

    /** Removes every cached copy and block of a file (deleted on Drive, or superseded by a local edit). */
    fun dropFile(account: String, fileId: String) = lock.withLock {
        val l = ledgerLocked()
        dropVersionsLocked(l, account, fileId, keepTag = null)
        dirty = true
        persistLocked(force = true)
    }

    // ---------------------------------------------------------------- streaming blocks

    fun hasBlock(account: String, fileId: String, tag: String, index: Int): Boolean = lock.withLock {
        ledgerLocked().get(account, fileId, tag, index) != null
    }

    /** Cached block bytes or null. The disk read happens outside the lock; a concurrent eviction just yields null. */
    fun readBlock(account: String, fileId: String, tag: String, index: Int): ByteArray? {
        val f = lock.withLock {
            val l = ledgerLocked()
            val e = l.get(account, fileId, tag, index) ?: return null
            l.touch(e, now()); dirty = true
            fileFor(e)
        }
        return try { if (f.isFile) f.readBytes() else null } catch (e: IOException) { null }
    }

    /** Stores one complete block. Written to tmp outside the lock, renamed into place under it. */
    fun writeBlock(account: String, fileId: String, tag: String, index: Int, data: ByteArray) {
        if (data.isEmpty()) return
        val tmp = newTempFile(account)
        try {
            tmp.writeBytes(data)
        } catch (e: IOException) {
            Log.w(TAG, "block $index of $fileId not cached: ${e.message}")
            tmp.delete()
            return
        }
        lock.withLock {
            val l = ledgerLocked()
            val e = CacheEntry(account, fileId, tag, index, data.size.toLong(), now(), pinned = false)
            val target = fileFor(e)
            if (l.get(account, fileId, tag, index) != null && target.isFile) { tmp.delete(); return }
            try {
                moveInto(tmp, target)
            } catch (ex: IOException) {
                Log.w(TAG, "block $index of $fileId not cached: ${ex.message}")
                tmp.delete()
                return
            }
            l.put(e)
            enforceLimitLocked(l, protect = e.id)
            dirty = true
            persistLocked(force = false)
        }
    }

    // ---------------------------------------------------------------- local edits (write sessions)

    private fun workPath(account: String, fileId: String): File = File(File(accountDir(account), CachePaths.WORK_DIR), fileId)

    private fun pendingMarker(account: String, fileId: String): File =
        File(File(accountDir(account), CachePaths.WORK_DIR), fileId + CachePaths.PENDING_SUFFIX)

    /** Scratch copy an app writes to through a plain file descriptor; uploaded when the app closes it. Creates the directory. */
    fun workFile(account: String, fileId: String): File {
        val f = workPath(account, fileId)
        f.parentFile?.mkdirs()
        return f
    }

    /** Work file whose upload has not completed yet. It holds the NEWEST content of the file, so reads must prefer it. */
    fun pendingWork(account: String, fileId: String): File? {
        val f = workPath(account, fileId)
        return if (f.isFile && pendingMarker(account, fileId).exists()) f else null
    }

    fun markPending(account: String, fileId: String) {
        runCatching { pendingMarker(account, fileId).createNewFile() }
            .onFailure { Log.w(TAG, "pending marker for $fileId not written: ${it.message}") }
    }

    fun discardWork(account: String, fileId: String) {
        pendingMarker(account, fileId).delete()
        workPath(account, fileId).delete()
    }

    /** (account, fileId) of every local edit still waiting for its upload. */
    fun pendingUploads(): List<Pair<String, String>> {
        val out = ArrayList<Pair<String, String>>()
        root.listFiles()?.filter { it.isDirectory }?.forEach { acctDir ->
            File(acctDir, CachePaths.WORK_DIR).listFiles()?.forEach { f ->
                if (f.name.endsWith(CachePaths.PENDING_SUFFIX)) {
                    val fileId = f.name.removeSuffix(CachePaths.PENDING_SUFFIX)
                    if (File(f.parentFile, fileId).isFile) out += acctDir.name to fileId else f.delete()
                }
            }
        }
        return out
    }

    /** After a successful upload the work file IS the current content: adopt it as the complete copy under the new tag. */
    fun adoptWorkAsFull(account: String, fileId: String, tag: String, pinned: Boolean): File? {
        val work = workPath(account, fileId)
        if (!work.isFile) return null
        pendingMarker(account, fileId).delete()
        return commitFull(account, fileId, tag, work, pinned)
    }

    // ---------------------------------------------------------------- housekeeping

    fun stats(): Stats = lock.withLock {
        val l = ledgerLocked()
        val all = l.all()
        Stats(l.totalBytes, limitBytes, l.pinnedBytes(), all.count { it.isFull }, all.count { !it.isFull })
    }

    fun totalBytes(): Long = lock.withLock { ledgerLocked().totalBytes }

    /** Deletes everything including pinned copies and unsent edits (Settings "Clear cache"); pins re-download on the next PinWorker run. */
    fun clear() {
        lock.withLock {
            ledgerLocked()
            root.deleteRecursively()
            ledger = CacheLedger()
            dirty = true
            persistLocked(force = true)
        }
        Log.i(TAG, "cache cleared")
    }

    /** Drops everything cached for one account (call when the account is removed from the app). */
    fun clearAccount(account: String) {
        lock.withLock {
            val l = ledgerLocked()
            l.forAccount(account).forEach { l.remove(it) }
            accountDir(account).deleteRecursively()
            dirty = true
            persistLocked(force = true)
        }
        Log.i(TAG, "cache cleared for $account")
    }

    /** Writes the index now (called when a streaming session ends and after pin syncs). */
    fun flush() = lock.withLock { persistLocked(force = true) }

    // ---------------------------------------------------------------- internals (all under lock)

    private fun now() = System.currentTimeMillis()

    private fun accountDir(account: String): File = File(root, account)

    private fun fileFor(e: CacheEntry): File = File(root, CachePaths.relativePath(e))

    private fun moveInto(src: File, target: File) {
        target.parentFile?.mkdirs()
        if (target.exists()) target.delete()
        if (src.renameTo(target)) return
        // Same filesystem in practice (everything lives in cacheDir); copy is only a fallback.
        src.copyTo(target, overwrite = true)
        if (!src.delete()) Log.w(TAG, "temp file not deleted: ${src.path}")
    }

    private fun dropVersionsLocked(l: CacheLedger, account: String, fileId: String, keepTag: String?) {
        for (o in l.forFile(account, fileId)) {
            if (keepTag != null && o.isFull && o.tag == keepTag) continue
            val f = fileFor(o)
            if (f.exists() && !f.delete()) Log.w(TAG, "could not delete ${f.path}")
            l.remove(o)
            if (!o.isFull) f.parentFile?.delete()   // removes the block dir once empty; fails harmlessly otherwise
        }
    }

    private fun enforceLimitLocked(l: CacheLedger, protect: String? = null) {
        val victims = l.victims(limitBytes, exclude = protect)
        if (victims.isEmpty()) return
        var freed = 0L
        for (v in victims) {
            val f = fileFor(v)
            if (f.exists() && !f.delete()) { Log.w(TAG, "could not evict ${f.path}"); continue }
            l.remove(v)
            freed += v.size
            if (!v.isFull) f.parentFile?.delete()
        }
        dirty = true
        Log.i(TAG, "evicted ${victims.size} entries (${freed / MB} MB); cache is ${l.totalBytes / MB} MB of ${limitBytes / MB} MB")
    }

    private fun ledgerLocked(): CacheLedger {
        ledger?.let { return it }
        val l = loadLocked()
        ledger = l
        return l
    }

    /** Reads the index, verifies every entry against the disk, adopts orphans, clears tmp and abandoned work files. */
    private fun loadLocked(): CacheLedger {
        root.mkdirs()
        val saved: CacheLedger? = try {
            if (indexFile.isFile) CacheLedger.decode(indexFile.readText()) else null
        } catch (t: Throwable) {
            Log.w(TAG, "cache index unreadable, rebuilding from disk: ${t.message}")
            null
        }
        val l = CacheLedger()
        var dropped = 0
        saved?.all()?.forEach { e ->
            val f = fileFor(e)
            if (f.isFile && f.length() == e.size) l.put(e) else dropped++
        }
        var adopted = 0
        root.listFiles()?.filter { it.isDirectory }?.forEach { acctDir ->
            val account = acctDir.name
            File(acctDir, CachePaths.TMP_DIR).listFiles()?.forEach { it.delete() }
            File(acctDir, CachePaths.FULL_DIR).listFiles()?.forEach { f ->
                if (!f.isFile) return@forEach
                val (id, tag) = CachePaths.parseFileKey(f.name) ?: return@forEach
                if (l.get(account, id, tag) == null) { l.put(CacheEntry(account, id, tag, CacheEntry.FULL, f.length(), f.lastModified())); adopted++ }
            }
            File(acctDir, CachePaths.BLOCKS_DIR).listFiles()?.forEach { dir ->
                if (!dir.isDirectory) return@forEach
                val (id, tag) = CachePaths.parseFileKey(dir.name) ?: return@forEach
                dir.listFiles()?.forEach { b ->
                    val idx = b.name.toIntOrNull() ?: return@forEach
                    if (b.isFile && l.get(account, id, tag, idx) == null) { l.put(CacheEntry(account, id, tag, idx, b.length(), b.lastModified())); adopted++ }
                }
            }
            // A work file without a pending marker belonged to a write session that died with the
            // process; its content is of unknown completeness and is never uploaded.
            val workDir = File(acctDir, CachePaths.WORK_DIR)
            workDir.listFiles()?.forEach { w ->
                if (w.name.endsWith(CachePaths.PENDING_SUFFIX)) return@forEach
                if (!File(workDir, w.name + CachePaths.PENDING_SUFFIX).exists()) w.delete()
            }
        }
        Log.i(TAG, "cache index loaded: ${l.size} entries, ${l.totalBytes / MB} MB (dropped $dropped, adopted $adopted)")
        if (dropped > 0 || adopted > 0) dirty = true
        return l
    }

    private fun persistLocked(force: Boolean) {
        if (!dirty) return
        val t = now()
        if (!force && t - lastPersist < PERSIST_INTERVAL_MS) return
        val l = ledger ?: return
        try {
            root.mkdirs()
            val tmp = File(root, "index.json.tmp")
            tmp.writeText(CacheLedger.encode(l))
            if (!tmp.renameTo(indexFile)) {
                indexFile.delete()
                if (!tmp.renameTo(indexFile)) throw IOException("rename to ${indexFile.name} failed")
            }
            dirty = false
            lastPersist = t
        } catch (e: IOException) {
            Log.w(TAG, "cache index not saved: ${e.message}")
        }
    }
}
