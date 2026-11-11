package com.almodhayan.drivesync.drive

import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.core.TimeFmt
import com.almodhayan.drivesync.data.db.AccountDao
import com.almodhayan.drivesync.data.db.DriveNodeDao
import com.almodhayan.drivesync.data.db.DriveNodeEntity
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.util.concurrent.ConcurrentHashMap
import javax.inject.Inject
import javax.inject.Singleton

/**
 * Account-wide metadata cache. One full crawl per account, then cheap deltas through the Changes
 * API. Every pair resolves its subtree from this cache instead of crawling Drive folder by folder.
 */
@Singleton
class DriveNodeCache @Inject constructor(
    private val api: DriveApi,
    private val nodes: DriveNodeDao,
    private val accounts: AccountDao,
) {
    private val locks = ConcurrentHashMap<String, Mutex>()
    private val crawledDrives = ConcurrentHashMap<String, MutableSet<String>>()

    class Snapshot(val accountId: String, val rootId: String, private val byId: Map<String, DriveNodeEntity>, private val children: Map<String?, List<DriveNodeEntity>>) {
        fun node(id: String): DriveNodeEntity? = byId[id]
        fun childrenOf(id: String): List<DriveNodeEntity> = children[id] ?: emptyList()

        /**
         * Display path like "My Drive/Photos/2026". The Drive API never lists the roots themselves
         * (My Drive, shared drives), so the walk usually stops at a node whose parent is not cached;
         * that parent id is what identifies the root.
         */
        fun pathOf(id: String, driveNames: Map<String, String> = emptyMap()): String {
            val parts = ArrayList<String>()
            var cur = byId[id] ?: return ""
            var guard = 0
            while (true) {
                parts += cur.name
                val p = cur.parentId ?: break
                cur = byId[p] ?: break
                if (++guard >= 256) break
            }
            // cur is the top-most cached ancestor; its parent (if any) is not in the cache.
            val top = cur
            val missingParent = top.parentId
            val prefix: String? = when {
                top.id == rootId -> { parts.removeAt(parts.size - 1); Strings.get(R.string.drive_my_drive) }
                missingParent == rootId -> Strings.get(R.string.drive_my_drive)
                top.driveId != null -> { if (top.id == top.driveId) parts.removeAt(parts.size - 1); driveNames[top.driveId] ?: Strings.get(R.string.common_shared_drive) }
                missingParent != null && missingParent in driveNames -> driveNames.getValue(missingParent)
                else -> null   // shared-with-me item or orphan: its own name is the first segment
            }
            parts.reverse()
            return (listOfNotNull(prefix) + parts).filter { it.isNotEmpty() }.joinToString("/")
        }
    }

    /** Ensure the cache is initialised and apply pending changes; returns true when anything changed. */
    suspend fun refresh(accountId: String, driveId: String? = null, force: Boolean = false): Boolean {
        val lock = locks.getOrPut(accountId) { Mutex() }
        return lock.withLock {
            val acct = accounts.get(accountId) ?: return@withLock false
            var changed = false
            if (acct.rootFolderId == null || acct.changesToken == null || force || nodes.count(accountId) == 0) {
                fullCrawl(accountId)
                changed = true
            }
            if (ensureSharedDrive(accountId, driveId)) changed = true
            val delta = try {
                applyDelta(accountId)
            } catch (e: AppError) {
                if (!rejectedToken(e)) throw e
                // Drive no longer accepts the saved changes token. Read everything again instead of
                // failing every run of every pair of this account until the account is re-added.
                fullCrawl(accountId)
                ensureSharedDrive(accountId, driveId)
                applyDelta(accountId)
                true
            }
            changed || delta
        }
    }

    private fun rejectedToken(e: AppError) = e is AppError.NotFound || (e is AppError.Http && (e.code == 400 || e.code == 410))

    /** Crawls a shared drive once per process when the cache has nothing for it; marked done only after it completed. */
    private suspend fun ensureSharedDrive(accountId: String, driveId: String?): Boolean {
        if (driveId == null) return false
        val done = crawledDrives.getOrPut(accountId) { ConcurrentHashMap.newKeySet() }
        if (driveId in done) return false
        var crawled = false
        if (nodesForDrive(accountId, driveId) == 0) { crawlSharedDrive(accountId, driveId); crawled = true }
        done += driveId
        return crawled
    }

    private suspend fun nodesForDrive(accountId: String, driveId: String): Int =
        nodes.allForAccount(accountId).count { it.driveId == driveId }

    private suspend fun fullCrawl(accountId: String) {
        val rootId = api.rootId(accountId)
        val token = api.startPageToken(accountId)      // taken before the crawl so nothing is lost
        // Until the crawl completes there is no token, so an interrupted crawl starts over next time
        // instead of leaving a half-read Drive that makes files look deleted.
        accounts.setChangesToken(accountId, null, System.currentTimeMillis())
        crawledDrives.remove(accountId)               // clearAccount drops shared-drive nodes too
        nodes.clearAccount(accountId)
        var pageToken: String? = null
        val batch = ArrayList<DriveNodeEntity>(1000)
        do {
            val page = api.listPage(accountId, "trashed = false", pageToken)
            pagesRead.incrementAndGet()
            page.files.forEach { batch += it.toNode(accountId) }
            if (batch.size >= 500) { nodes.upsertAll(batch); batch.clear() }
            pageToken = page.nextPageToken
        } while (pageToken != null)
        if (batch.isNotEmpty()) nodes.upsertAll(batch)
        val acct = accounts.get(accountId)!!
        accounts.upsert(acct.copy(rootFolderId = rootId, changesToken = token, lastCrawlAt = System.currentTimeMillis()))
    }

    /**
     * Reads a whole shared drive, then stores it in one go. Nothing is stored if the read stops half
     * way: "no nodes for this drive" is what tells the next refresh to crawl it again.
     */
    private suspend fun crawlSharedDrive(accountId: String, driveId: String) {
        nodes.clearDrive(accountId, driveId)
        val all = ArrayList<DriveNodeEntity>()
        try {
            var pageToken: String? = null
            do {
                val page = api.listPage(accountId, "trashed = false", pageToken, driveId = driveId)
                pagesRead.incrementAndGet()
                page.files.forEach { all += it.toNode(accountId) }
                pageToken = page.nextPageToken
            } while (pageToken != null)
            nodes.upsertAll(all)
        } catch (t: Throwable) {
            withContext(NonCancellable) { nodes.clearDrive(accountId, driveId) }
            throw t
        }
    }

    private suspend fun applyDelta(accountId: String): Boolean {
        val acct = accounts.get(accountId) ?: return false
        var token = acct.changesToken ?: return false
        var changed = false
        val upserts = ArrayList<DriveNodeEntity>()
        val removals = ArrayList<String>()
        while (true) {
            val page = api.changesPage(accountId, token)
            pagesRead.incrementAndGet()
            for (c in page.changes) {
                if (c.changeType == "drive") continue
                val id = c.fileId ?: continue
                val f = c.file
                if (c.removed || f == null || f.trashed) removals += id else upserts += f.toNode(accountId)
            }
            if (page.nextPageToken != null) { token = page.nextPageToken; continue }
            if (upserts.isNotEmpty()) { nodes.upsertAll(upserts); changed = true }
            if (removals.isNotEmpty()) { removals.chunked(500).forEach { nodes.deleteIds(accountId, it) }; changed = true }
            accounts.setChangesToken(accountId, page.newStartPageToken ?: token, System.currentTimeMillis())
            break
        }
        return changed
    }

    suspend fun snapshot(accountId: String): Snapshot {
        val acct = accounts.get(accountId) ?: error("account missing")
        val all = nodes.allForAccount(accountId)
        return Snapshot(accountId, acct.rootFolderId ?: "root", all.associateBy { it.id }, all.groupBy { it.parentId })
    }

    suspend fun invalidate(accountId: String) {
        val acct = accounts.get(accountId) ?: return
        accounts.upsert(acct.copy(changesToken = null))
        crawledDrives.remove(accountId)
    }

    /** Upsert a node we just created/updated ourselves so the next planning pass sees it without waiting for a delta. */
    suspend fun remember(accountId: String, f: DriveFile) { nodes.upsert(f.toNode(accountId)) }
    suspend fun forget(accountId: String, id: String) { nodes.deleteIds(accountId, listOf(id)) }

    companion object {
        /**
         * Pages read from Drive so far, by any crawl. The first read of a big Drive takes minutes
         * without other visible progress; this tells the sync's stall watchdog it is still moving.
         */
        val pagesRead = java.util.concurrent.atomic.AtomicLong()

        fun DriveFile.toNode(accountId: String) = DriveNodeEntity(
            accountId = accountId,
            id = id,
            name = name,
            mimeType = mimeType,
            parentId = parents?.firstOrNull(),
            driveId = driveId,
            size = sizeLong,
            md5 = md5Checksum,
            modifiedTime = TimeFmt.parseRfc3339(modifiedTime) ?: 0L,
            version = versionLong,
            trashed = trashed,
            isFolder = isFolder,
            isShortcut = isShortcut,
            shortcutTargetId = shortcutDetails?.targetId,
            shortcutTargetMime = shortcutDetails?.targetMimeType,
            shared = shared,
            permissionCount = permissionIds?.size,
        )
    }
}
