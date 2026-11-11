package com.almodhayan.drivesync.drive

import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.data.db.AccountDao
import com.almodhayan.drivesync.data.db.AccountEntity
import com.almodhayan.drivesync.data.db.DriveNodeDao
import com.almodhayan.drivesync.data.db.DriveNodeEntity
import com.almodhayan.drivesync.data.db.UploadSessionDao
import com.almodhayan.drivesync.data.db.UploadSessionEntity
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.test.runTest
import okhttp3.mockwebserver.MockWebServer
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import java.time.Instant

// ------------------------------------------------------------------ in-memory Room doubles

internal class FakeAccountDao : AccountDao {
    val map = LinkedHashMap<String, AccountEntity>()
    override fun observeAll(): Flow<List<AccountEntity>> = flowOf(map.values.toList())
    override suspend fun getAll(): List<AccountEntity> = map.values.toList()
    override suspend fun get(id: String): AccountEntity? = map[id]
    override fun observe(id: String): Flow<AccountEntity?> = flowOf(map[id])
    override suspend fun upsert(a: AccountEntity) { map[a.id] = a }
    override suspend fun delete(id: String) { map.remove(id) }
    override suspend fun setChangesToken(id: String, token: String?, at: Long) { map[id]?.let { map[id] = it.copy(changesToken = token, lastDeltaAt = at) } }
    override suspend fun setCrawled(id: String, at: Long) { map[id]?.let { map[id] = it.copy(lastCrawlAt = at) } }
    override suspend fun setQuota(id: String, used: Long?, limit: Long?) { map[id]?.let { map[id] = it.copy(storageUsed = used, storageLimit = limit) } }
    override suspend fun setAuthError(id: String, err: String?) { map[id]?.let { map[id] = it.copy(authError = err) } }
}

internal class FakeNodeDao : DriveNodeDao {
    val map = LinkedHashMap<Pair<String, String>, DriveNodeEntity>()
    fun put(n: DriveNodeEntity) { map[n.accountId to n.id] = n }
    fun ids(accountId: String): Set<String> = map.values.filter { it.accountId == accountId }.map { it.id }.toSet()

    override suspend fun allForAccount(accountId: String): List<DriveNodeEntity> = map.values.filter { it.accountId == accountId && !it.trashed }
    override suspend fun get(accountId: String, id: String): DriveNodeEntity? = map[accountId to id]
    override suspend fun children(accountId: String, parentId: String): List<DriveNodeEntity> =
        allForAccount(accountId).filter { it.parentId == parentId }.sortedWith(compareByDescending<DriveNodeEntity> { it.isFolder }.thenBy { it.name.lowercase() })
    override fun observeChildren(accountId: String, parentId: String): Flow<List<DriveNodeEntity>> =
        flowOf(map.values.filter { it.accountId == accountId && it.parentId == parentId && !it.trashed })
    override suspend fun count(accountId: String): Int = map.values.count { it.accountId == accountId }
    override suspend fun upsertAll(nodes: List<DriveNodeEntity>) { nodes.forEach { put(it) } }
    override suspend fun upsert(node: DriveNodeEntity) { put(node) }
    override suspend fun deleteIds(accountId: String, ids: List<String>) { ids.forEach { map.remove(accountId to it) } }
    override suspend fun clearAccount(accountId: String) { map.keys.filter { it.first == accountId }.forEach { map.remove(it) } }
    override suspend fun clearDrive(accountId: String, driveId: String) {
        map.entries.filter { it.key.first == accountId && it.value.driveId == driveId }.map { it.key }.forEach { map.remove(it) }
    }
    override suspend fun search(accountId: String, q: String): List<DriveNodeEntity> =
        allForAccount(accountId).filter { it.name.contains(q, ignoreCase = true) }.take(200)
    override suspend fun sharedWithMeRoots(accountId: String, rootId: String): List<DriveNodeEntity> =
        map.values.filter { it.accountId == accountId && !it.trashed && it.driveId == null && it.id != rootId && (it.parentId == null || (it.parentId != rootId && map.values.none { n -> n.accountId == accountId && n.id == it.parentId })) }
}

class DriveNodeCacheTest {

    private val server = MockWebServer()
    private val tokens = FakeTokens()
    private val accounts = FakeAccountDao()
    private val nodes = FakeNodeDao()
    private lateinit var api: DriveApi
    private lateinit var cache: DriveNodeCache
    private val account = "me@example.com"

    @Before
    fun setUp() {
        server.start()
        api = DriveApi(testClient(), tokens, appJson, server.endpoints())
        cache = DriveNodeCache(api, nodes, accounts)
    }

    @After
    fun tearDown() {
        server.shutdown()
    }

    private fun folderJson(id: String, name: String, parent: String?) = fileJson(id, name, FOLDER_MIME, parent)

    private fun node(id: String, name: String, parent: String? = "ROOT", isFolder: Boolean = false, driveId: String? = null, trashed: Boolean = false) =
        DriveNodeEntity(account, id, name, if (isFolder) FOLDER_MIME else "text/plain", parentId = parent, driveId = driveId, modifiedTime = 1000, isFolder = isFolder, trashed = trashed)

    /** Account that already went through a full crawl: root known, token "100", three cached nodes. */
    private fun seedCrawled() {
        accounts.map[account] = AccountEntity(account, rootFolderId = "ROOT", changesToken = "100")
        nodes.put(node("A", "a", isFolder = true))
        nodes.put(node("B", "b.txt", parent = "A"))
        nodes.put(node("C", "c.txt"))
    }

    private fun enqueueChanges(body: String) = server.enqueue(jsonResponse(body))

    // ================================================================ refresh: full crawl

    @Test
    fun firstRefresh_crawlsEverything_andTakesTheStartPageTokenBeforeListing() = runTest {
        accounts.map[account] = AccountEntity(account)
        server.enqueue(jsonResponse(fileJson("ROOT", "My Drive", FOLDER_MIME)))                       // files/root
        server.enqueue(jsonResponse("""{"startPageToken":"100"}"""))
        server.enqueue(jsonResponse("""{"nextPageToken":"p2","files":[${folderJson("A", "a", "ROOT")},${fileJson("B", "b.txt", parent = "A", extra = ""","size":"12","md5Checksum":"m","modifiedTime":"2026-01-02T03:04:05.000Z","version":"3"""")}]}"""))
        server.enqueue(jsonResponse("""{"files":[${fileJson("C", "c.txt", parent = "ROOT")}]}"""))
        enqueueChanges("""{"newStartPageToken":"100","changes":[]}""")

        assertTrue(cache.refresh(account))

        val urls = (1..5).map { server.take().requestUrl!! }
        assertEquals("/drive/v3/files/root", urls[0].encodedPath)
        assertEquals("the changes token is taken BEFORE the crawl so nothing is lost", "/drive/v3/changes/startPageToken", urls[1].encodedPath)
        assertEquals("/drive/v3/files", urls[2].encodedPath)
        assertEquals("trashed = false", urls[2].queryParameter("q"))
        assertNull(urls[2].queryParameter("pageToken"))
        assertEquals("user", urls[2].queryParameter("corpora"))
        assertEquals("p2", urls[3].queryParameter("pageToken"))
        assertEquals("/drive/v3/changes", urls[4].encodedPath)
        assertEquals("100", urls[4].queryParameter("pageToken"))
        assertEquals(5, server.requestCount)

        assertEquals(setOf("A", "B", "C"), nodes.ids(account))
        val b = nodes.get(account, "B")!!
        assertEquals("A", b.parentId)
        assertEquals(12L, b.size)
        assertEquals("m", b.md5)
        assertEquals(3L, b.version)
        assertFalse(b.isFolder)
        assertEquals(Instant.parse("2026-01-02T03:04:05.000Z").toEpochMilli(), b.modifiedTime)
        assertTrue(nodes.get(account, "A")!!.isFolder)
        assertEquals("ROOT", nodes.get(account, "C")!!.parentId)

        val acct = accounts.map[account]!!
        assertEquals("ROOT", acct.rootFolderId)
        assertEquals("100", acct.changesToken)
        assertNotNull(acct.lastCrawlAt)
        assertNotNull(acct.lastDeltaAt)
    }

    @Test
    fun refresh_unknownAccount_returnsFalseWithoutRequests() = runTest {
        assertFalse(cache.refresh("nobody@example.com"))
        assertEquals(0, server.requestCount)
    }

    @Test
    fun refresh_recrawlsWhenTheNodeTableIsEmpty() = runTest {
        accounts.map[account] = AccountEntity(account, rootFolderId = "ROOT", changesToken = "100")
        server.enqueue(jsonResponse(fileJson("ROOT", "My Drive", FOLDER_MIME)))
        server.enqueue(jsonResponse("""{"startPageToken":"150"}"""))
        server.enqueue(jsonResponse("""{"files":[${fileJson("X", "x.txt", parent = "ROOT")}]}"""))
        enqueueChanges("""{"newStartPageToken":"150","changes":[]}""")
        assertTrue(cache.refresh(account))
        assertEquals(setOf("X"), nodes.ids(account))
        assertEquals("150", accounts.map[account]!!.changesToken)
    }

    @Test
    fun refresh_force_recrawlsAndReplacesTheCache() = runTest {
        seedCrawled()
        server.enqueue(jsonResponse(fileJson("ROOT", "My Drive", FOLDER_MIME)))
        server.enqueue(jsonResponse("""{"startPageToken":"300"}"""))
        server.enqueue(jsonResponse("""{"files":[${fileJson("X", "x.txt", parent = "ROOT")}]}"""))
        enqueueChanges("""{"newStartPageToken":"300","changes":[]}""")
        assertTrue(cache.refresh(account, force = true))
        assertEquals("stale nodes are cleared by the crawl", setOf("X"), nodes.ids(account))
        assertEquals("300", accounts.map[account]!!.changesToken)
        assertEquals(4, server.requestCount)
    }

    @Test
    fun invalidate_dropsTheChangesToken_soTheNextRefreshRecrawls() = runTest {
        seedCrawled()
        cache.invalidate(account)
        assertNull(accounts.map[account]!!.changesToken)
        server.enqueue(jsonResponse(fileJson("ROOT", "My Drive", FOLDER_MIME)))
        server.enqueue(jsonResponse("""{"startPageToken":"400"}"""))
        server.enqueue(jsonResponse("""{"files":[]}"""))
        enqueueChanges("""{"newStartPageToken":"400","changes":[]}""")
        assertTrue(cache.refresh(account))
        assertEquals("/drive/v3/files/root", server.take().requestUrl!!.encodedPath)
        assertEquals("400", accounts.map[account]!!.changesToken)
    }

    // ================================================================ refresh: deltas

    @Test
    fun refresh_appliesDeltasAcrossPages() = runTest {
        seedCrawled()
        enqueueChanges("""{"nextPageToken":"101","changes":[
            {"changeType":"file","fileId":"B","removed":true},
            {"changeType":"file","fileId":"C","file":${fileJson("C", "c2.txt", parent = "ROOT", extra = ""","md5Checksum":"new","version":"2"""")}}
        ]}""")
        enqueueChanges("""{"newStartPageToken":"200","changes":[
            {"changeType":"drive","driveId":"D","fileId":"C","removed":true},
            {"changeType":"file","fileId":"D1","file":${fileJson("D1", "new.txt", parent = "A")}},
            {"changeType":"file","fileId":"A","file":${folderJson("A", "a", "ROOT").dropLast(1) + ""","trashed":true}"""}},
            {"changeType":"file","removed":true}
        ]}""")

        assertTrue(cache.refresh(account))

        assertEquals("only the two change pages were requested", 2, server.requestCount)
        assertEquals("100", server.take().requestUrl!!.queryParameter("pageToken"))
        assertEquals("101", server.take().requestUrl!!.queryParameter("pageToken"))

        assertEquals(setOf("C", "D1"), nodes.ids(account))
        val c = nodes.get(account, "C")!!
        assertEquals("c2.txt", c.name)
        assertEquals("new", c.md5)
        assertEquals(2L, c.version)
        assertEquals("A", nodes.get(account, "D1")!!.parentId)
        assertNull("removed", nodes.get(account, "B"))
        assertNull("trashed", nodes.get(account, "A"))

        val acct = accounts.map[account]!!
        assertEquals("200", acct.changesToken)
        assertNotNull(acct.lastDeltaAt)
    }

    @Test
    fun refresh_withoutChanges_returnsFalseAndKeepsTheToken() = runTest {
        seedCrawled()
        enqueueChanges("""{"newStartPageToken":"100","changes":[]}""")
        assertFalse(cache.refresh(account))
        assertEquals("100", accounts.map[account]!!.changesToken)
        assertEquals(setOf("A", "B", "C"), nodes.ids(account))
        assertEquals(1, server.requestCount)
    }

    @Test
    fun refresh_changeWithoutFileMetadata_isTreatedAsRemoval() = runTest {
        seedCrawled()
        enqueueChanges("""{"newStartPageToken":"101","changes":[{"changeType":"file","fileId":"C","removed":false}]}""")
        assertTrue(cache.refresh(account))
        assertNull(nodes.get(account, "C"))
    }

    // ================================================================ refresh: shared drives

    @Test
    fun refresh_sharedDrive_isCrawledOnceOnFirstUse() = runTest {
        seedCrawled()
        server.enqueue(jsonResponse("""{"files":[${fileJson("S1", "s1", FOLDER_MIME, parent = "D1", extra = ""","driveId":"D1"""")}]}"""))
        enqueueChanges("""{"newStartPageToken":"101","changes":[]}""")

        assertTrue(cache.refresh(account, driveId = "D1"))
        val list = server.take().requestUrl!!
        assertEquals("/drive/v3/files", list.encodedPath)
        assertEquals("drive", list.queryParameter("corpora"))
        assertEquals("D1", list.queryParameter("driveId"))
        assertEquals("trashed = false", list.queryParameter("q"))
        assertEquals("/drive/v3/changes", server.take().requestUrl!!.encodedPath)
        assertEquals("D1", nodes.get(account, "S1")!!.driveId)

        // second refresh for the same drive: only the delta
        enqueueChanges("""{"newStartPageToken":"101","changes":[]}""")
        assertFalse(cache.refresh(account, driveId = "D1"))
        assertEquals(3, server.requestCount)
    }

    @Test
    fun refresh_sharedDriveCrawlThatFails_leavesNothingHalfRead_andIsCrawledAgain() = runTest {
        seedCrawled()
        server.enqueue(jsonResponse("""{"nextPageToken":"p2","files":[${fileJson("S1", "s1", FOLDER_MIME, parent = "D1", extra = ""","driveId":"D1"""")}]}"""))
        server.enqueue(apiError(400, "badRequest", "Bad page"))

        assertFails<AppError> { cache.refresh(account, driveId = "D1") }
        assertNull("half a shared drive reads as files deleted on Drive", nodes.get(account, "S1"))

        server.enqueue(jsonResponse("""{"files":[${fileJson("S1", "s1", FOLDER_MIME, parent = "D1", extra = ""","driveId":"D1"""")},${fileJson("S2", "s2.txt", parent = "S1", extra = ""","driveId":"D1"""")}]}"""))
        enqueueChanges("""{"newStartPageToken":"101","changes":[]}""")
        assertTrue(cache.refresh(account, driveId = "D1"))
        assertNotNull(nodes.get(account, "S2"))
    }

    @Test
    fun refresh_fullCrawlThatFails_isStartedAgainNextTime() = runTest {
        accounts.map[account] = AccountEntity(account, rootFolderId = "ROOT", changesToken = "100")
        server.enqueue(jsonResponse(fileJson("ROOT", "My Drive", FOLDER_MIME)))
        server.enqueue(jsonResponse("""{"startPageToken":"150"}"""))
        // A first page big enough that part of it is already stored when the second page fails.
        val firstPage = (0 until 600).joinToString(",") { fileJson("F$it", "f$it.txt", parent = "ROOT") }
        server.enqueue(jsonResponse("""{"nextPageToken":"p2","files":[$firstPage]}"""))
        server.enqueue(apiError(400, "badRequest", "Bad page"))
        assertFails<AppError> { cache.refresh(account) }

        server.enqueue(jsonResponse(fileJson("ROOT", "My Drive", FOLDER_MIME)))
        server.enqueue(jsonResponse("""{"startPageToken":"160"}"""))
        server.enqueue(jsonResponse("""{"files":[${fileJson("X", "x.txt", parent = "ROOT")},${fileJson("Y", "y.txt", parent = "ROOT")}]}"""))
        enqueueChanges("""{"newStartPageToken":"160","changes":[]}""")
        assertTrue(cache.refresh(account))

        assertEquals("a half-read cache was trusted", setOf("X", "Y"), nodes.ids(account))
        assertEquals("160", accounts.map[account]!!.changesToken)
    }

    @Test
    fun refresh_whenDriveRejectsTheChangesToken_recrawlsInsteadOfFailingForever() = runTest {
        seedCrawled()
        server.enqueue(apiError(400, "invalid", "Invalid Value"))
        server.enqueue(jsonResponse(fileJson("ROOT", "My Drive", FOLDER_MIME)))
        server.enqueue(jsonResponse("""{"startPageToken":"500"}"""))
        server.enqueue(jsonResponse("""{"files":[${fileJson("X", "x.txt", parent = "ROOT")}]}"""))
        enqueueChanges("""{"newStartPageToken":"500","changes":[]}""")

        assertTrue(cache.refresh(account))

        assertEquals(setOf("X"), nodes.ids(account))
        assertEquals("500", accounts.map[account]!!.changesToken)
    }

    // The two DriveRemoteFs.listSubtree cases moved with that logic into the Rust engine
    // (engine/crates/wasl-drive/tests/drive_node_cache.rs).

    // ================================================================ snapshot

    @Test
    fun snapshot_exposesNodesAndChildren() = runTest {
        seedCrawled()
        val snap = cache.snapshot(account)
        assertEquals(account, snap.accountId)
        assertEquals("ROOT", snap.rootId)
        assertEquals(setOf("A", "C"), snap.childrenOf("ROOT").map { it.id }.toSet())
        assertEquals(listOf("B"), snap.childrenOf("A").map { it.id })
        assertTrue(snap.childrenOf("nope").isEmpty())
        assertEquals("b.txt", snap.node("B")!!.name)
        assertNull(snap.node("nope"))
    }

    @Test
    fun snapshot_ignoresTrashedNodes() = runTest {
        seedCrawled()
        nodes.put(node("T", "t.txt", trashed = true))
        val snap = cache.snapshot(account)
        assertNull(snap.node("T"))
        assertEquals(setOf("A", "C"), snap.childrenOf("ROOT").map { it.id }.toSet())
    }

    @Test
    fun snapshot_rootIdFallsBackToAliasBeforeTheFirstCrawl() = runTest {
        accounts.map[account] = AccountEntity(account)
        assertEquals("root", cache.snapshot(account).rootId)
    }

    @Test
    fun pathOf_prefixesMyDrive_whenTheRootItselfIsNotCached() = runTest {
        // files.list never returns the root folder, so this is the production shape of the cache.
        seedCrawled()
        val snap = cache.snapshot(account)
        assertEquals("My Drive/a/b.txt", snap.pathOf("B"))
        assertEquals("My Drive/a", snap.pathOf("A"))
        assertEquals("My Drive/c.txt", snap.pathOf("C"))
        assertEquals("", snap.pathOf("unknown"))
    }

    @Test
    fun pathOf_doesNotDuplicateTheRootName_whenTheRootIsCached() = runTest {
        seedCrawled()
        nodes.put(node("ROOT", "My Drive", parent = null, isFolder = true))
        val snap = cache.snapshot(account)
        assertEquals("My Drive/a/b.txt", snap.pathOf("B"))
        assertEquals("My Drive", snap.pathOf("ROOT"))
    }

    @Test
    fun pathOf_sharedDrive_usesTheDriveNameOrAGenericLabel() = runTest {
        seedCrawled()
        nodes.put(node("S1", "s1", parent = "D1", isFolder = true, driveId = "D1"))
        nodes.put(node("S2", "s2.txt", parent = "S1", driveId = "D1"))
        val snap = cache.snapshot(account)
        assertEquals("Team/s1/s2.txt", snap.pathOf("S2", mapOf("D1" to "Team")))
        assertEquals("Shared drive/s1/s2.txt", snap.pathOf("S2"))
    }

    @Test
    fun pathOf_sharedWithMeItem_hasNoRootPrefixAndNoDuplicatedName() = runTest {
        seedCrawled()
        nodes.put(node("W", "shared folder", parent = null, isFolder = true))
        nodes.put(node("W1", "inner.txt", parent = "W"))
        val snap = cache.snapshot(account)
        assertEquals("shared folder", snap.pathOf("W"))
        assertEquals("shared folder/inner.txt", snap.pathOf("W1"))
    }

    // ================================================================ remember / forget

    @Test
    fun remember_upsertsTheFile_andForgetRemovesIt() = runTest {
        seedCrawled()
        cache.remember(account, DriveFile(id = "N", name = "n.txt", mimeType = "text/plain", parents = listOf("ROOT"), size = "5", md5Checksum = "q", modifiedTime = "2026-01-02T03:04:05Z", version = "1"))
        val n = nodes.get(account, "N")!!
        assertEquals("n.txt", n.name)
        assertEquals("ROOT", n.parentId)
        assertEquals(5L, n.size)
        assertEquals("q", n.md5)
        assertEquals(1L, n.version)
        assertEquals(Instant.parse("2026-01-02T03:04:05Z").toEpochMilli(), n.modifiedTime)
        assertFalse(n.isFolder)
        assertEquals(setOf("N"), cache.snapshot(account).childrenOf("ROOT").map { it.id }.toSet() - setOf("A", "C"))

        cache.remember(account, DriveFile(id = "N", name = "renamed.txt", mimeType = "text/plain", parents = listOf("A")))
        assertEquals("renamed.txt", nodes.get(account, "N")!!.name)
        assertEquals("A", nodes.get(account, "N")!!.parentId)

        cache.forget(account, "N")
        assertNull(nodes.get(account, "N"))
        assertEquals(0, server.requestCount)
    }

    @Test
    fun toNode_mapsShortcutsAndMissingFields() {
        val shortcut = DriveFile(id = "S", name = "s", mimeType = SHORTCUT_MIME, shortcutDetails = ShortcutDetails(targetId = "T", targetMimeType = FOLDER_MIME))
        with(DriveNodeCache.Companion) {
            val n = shortcut.toNode(account)
            assertTrue(n.isShortcut)
            assertFalse(n.isFolder)
            assertEquals("T", n.shortcutTargetId)
            assertEquals(FOLDER_MIME, n.shortcutTargetMime)
            assertNull(n.parentId)
            assertNull(n.size)
            assertEquals(0L, n.modifiedTime)
            assertNull(n.version)
        }
    }
}
