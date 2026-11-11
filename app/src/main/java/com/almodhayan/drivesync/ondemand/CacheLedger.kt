package com.almodhayan.drivesync.ondemand

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

/**
 * One cached item: either a complete file ([block] == [FULL]) or one streaming block of a file.
 * The pair (fileId, tag) identifies the CONTENT (see [DocMapper.fileCacheTag]); a new Drive
 * version gets a new tag, so stale copies never masquerade as current ones.
 */
@Serializable
data class CacheEntry(
    val account: String,
    val fileId: String,
    val tag: String,
    val block: Int = FULL,
    val size: Long,
    val lastUsed: Long,
    val pinned: Boolean = false,
) {
    val isFull: Boolean get() = block == FULL
    val fileKey: String get() = fileKey(fileId, tag)
    val id: String get() = entryId(account, fileId, tag, block)

    companion object {
        const val FULL = -1
        fun fileKey(fileId: String, tag: String): String = "$fileId-$tag"
        fun entryId(account: String, fileId: String, tag: String, block: Int): String = "$account|${fileKey(fileId, tag)}|$block"
    }
}

@Serializable
data class LedgerSnapshot(val version: Int = 1, val entries: List<CacheEntry> = emptyList())

/** Relative layout of the cache directory; pure so the parsing side is testable. */
object CachePaths {
    const val FULL_DIR = "full"
    const val BLOCKS_DIR = "blocks"
    const val WORK_DIR = "work"
    const val TMP_DIR = "tmp"
    const val PENDING_SUFFIX = ".pending"

    fun fullPath(e: CacheEntry): String = "${e.account}/$FULL_DIR/${e.fileKey}"
    fun blockPath(e: CacheEntry): String = "${e.account}/$BLOCKS_DIR/${e.fileKey}/${e.block}"
    fun relativePath(e: CacheEntry): String = if (e.isFull) fullPath(e) else blockPath(e)

    /**
     * "<fileId>-<tag>" -> (fileId, tag). Drive ids may contain '-', tags never do (md5 hex,
     * "v123" or "v123.docx"), so the LAST '-' is the separator. Used to re-adopt files whose index
     * entry was lost (crash between the file rename and the index save).
     */
    fun parseFileKey(name: String): Pair<String, String>? {
        val i = name.lastIndexOf('-')
        if (i <= 0 || i == name.length - 1) return null
        val tag = name.substring(i + 1)
        if (tag.contains('-') || tag.contains('/')) return null
        return name.substring(0, i) to tag
    }
}

/**
 * In-memory bookkeeping of the on-demand cache: entries, total size, LRU order and pin flags.
 * NOT thread-safe; [OnDemandCache] serialises every access under one lock. Pure Kotlin so the
 * eviction rules are covered by JVM unit tests.
 */
class CacheLedger(initial: Collection<CacheEntry> = emptyList()) {
    private val entries = HashMap<String, CacheEntry>()

    var totalBytes: Long = 0
        private set

    init { initial.forEach { put(it) } }

    val size: Int get() = entries.size

    fun all(): List<CacheEntry> = entries.values.toList()

    fun get(account: String, fileId: String, tag: String, block: Int = CacheEntry.FULL): CacheEntry? =
        entries[CacheEntry.entryId(account, fileId, tag, block)]

    /** Inserts or replaces; returns the previous entry with the same identity, if any. */
    fun put(e: CacheEntry): CacheEntry? {
        val prev = entries.put(e.id, e)
        if (prev != null) totalBytes -= prev.size
        totalBytes += e.size
        return prev
    }

    fun touch(e: CacheEntry, now: Long): CacheEntry {
        val cur = entries[e.id] ?: return e
        if (cur.lastUsed >= now) return cur
        val n = cur.copy(lastUsed = now)
        entries[n.id] = n
        return n
    }

    fun remove(e: CacheEntry): Boolean {
        val r = entries.remove(e.id) ?: return false
        totalBytes -= r.size
        return true
    }

    fun forAccount(account: String): List<CacheEntry> = entries.values.filter { it.account == account }

    fun forFile(account: String, fileId: String): List<CacheEntry> =
        entries.values.filter { it.account == account && it.fileId == fileId }

    fun fullFiles(account: String, fileId: String): List<CacheEntry> = forFile(account, fileId).filter { it.isFull }

    fun pinnedBytes(): Long = entries.values.sumOf { if (it.pinned) it.size else 0L }

    /**
     * Marks the complete files whose key is in [pinnedKeys] as pinned and every other complete file
     * of [account] as unpinned. Blocks are never pinned: pinned content always has a full copy.
     * Returns how many entries changed.
     */
    fun setPinned(account: String, pinnedKeys: Set<String>): Int {
        var changed = 0
        for (e in entries.values.toList()) {
            if (e.account != account || !e.isFull) continue
            val want = e.fileKey in pinnedKeys
            if (want != e.pinned) { entries[e.id] = e.copy(pinned = want); changed++ }
        }
        return changed
    }

    /**
     * Least-recently-used unpinned entries whose removal brings the total to or under
     * [limitBytes]; empty when already within the limit. [exclude] protects an entry that was just
     * added so a caller never gets back a file that was evicted in the same breath.
     */
    fun victims(limitBytes: Long, exclude: String? = null): List<CacheEntry> {
        if (totalBytes <= limitBytes) return emptyList()
        var excess = totalBytes - limitBytes
        val out = ArrayList<CacheEntry>()
        for (e in entries.values.filter { !it.pinned && it.id != exclude }.sortedWith(compareBy({ it.lastUsed }, { it.id }))) {
            if (excess <= 0) break
            out += e
            excess -= e.size
        }
        return out
    }

    fun snapshot(): LedgerSnapshot = LedgerSnapshot(entries = all().sortedBy { it.id })

    companion object {
        private val json = Json { ignoreUnknownKeys = true; encodeDefaults = true }

        fun encode(ledger: CacheLedger): String = json.encodeToString(LedgerSnapshot.serializer(), ledger.snapshot())

        fun decode(text: String): CacheLedger = CacheLedger(json.decodeFromString(LedgerSnapshot.serializer(), text).entries)
    }
}
