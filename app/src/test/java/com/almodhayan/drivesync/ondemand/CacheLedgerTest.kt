package com.almodhayan.drivesync.ondemand

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class CacheLedgerTest {
    private val a = "a@gmail.com"
    private val b = "b@gmail.com"

    private fun full(account: String = a, fileId: String, tag: String = "md5", size: Long, used: Long, pinned: Boolean = false) =
        CacheEntry(account, fileId, tag, CacheEntry.FULL, size, used, pinned)

    private fun block(account: String = a, fileId: String, tag: String = "md5", index: Int, size: Long, used: Long) =
        CacheEntry(account, fileId, tag, index, size, used)

    @Test
    fun putReplaceAndRemoveKeepTotalsRight() {
        val l = CacheLedger()
        assertNull(l.put(full(fileId = "f1", size = 100, used = 1)))
        l.put(block(fileId = "f2", index = 0, size = 50, used = 1))
        assertEquals(150, l.totalBytes)
        assertEquals(2, l.size)

        val prev = l.put(full(fileId = "f1", size = 120, used = 2))   // same identity, new size
        assertNotNull(prev)
        assertEquals(100, prev!!.size)
        assertEquals(170, l.totalBytes)
        assertEquals(2, l.size)

        assertTrue(l.remove(full(fileId = "f1", size = 0, used = 0)))
        assertFalse(l.remove(full(fileId = "f1", size = 0, used = 0)))
        assertEquals(50, l.totalBytes)
    }

    @Test
    fun identityDistinguishesAccountTagAndBlock() {
        val l = CacheLedger()
        l.put(full(fileId = "f1", tag = "v1", size = 1, used = 1))
        l.put(full(fileId = "f1", tag = "v2", size = 1, used = 1))
        l.put(full(account = b, fileId = "f1", tag = "v1", size = 1, used = 1))
        l.put(block(fileId = "f1", tag = "v1", index = 0, size = 1, used = 1))
        l.put(block(fileId = "f1", tag = "v1", index = 1, size = 1, used = 1))
        assertEquals(5, l.size)
        assertNotNull(l.get(a, "f1", "v1"))
        assertNotNull(l.get(a, "f1", "v1", 1))
        assertNull(l.get(a, "f1", "v3"))
        assertNull(l.get(b, "f1", "v2"))
        assertEquals(4, l.forFile(a, "f1").size)
        assertEquals(2, l.fullFiles(a, "f1").size)
        assertEquals(1, l.forAccount(b).size)
    }

    @Test
    fun victimsAreLeastRecentlyUsedFirstAndStopAtTheLimit() {
        val l = CacheLedger()
        l.put(full(fileId = "old", size = 100, used = 1))
        l.put(full(fileId = "mid", size = 100, used = 2))
        l.put(full(fileId = "new", size = 100, used = 3))
        val victims = l.victims(limitBytes = 150)
        assertEquals(listOf("old", "mid"), victims.map { it.fileId })   // 300 -> 100 <= 150 needs two evictions
        assertTrue(l.victims(limitBytes = 300).isEmpty())
        assertEquals(listOf("old"), l.victims(limitBytes = 250).map { it.fileId })
    }

    @Test
    fun touchingAnEntryProtectsItFromEviction() {
        val l = CacheLedger()
        val old = l.put(full(fileId = "old", size = 100, used = 1)).let { l.get(a, "old", "md5")!! }
        l.put(full(fileId = "mid", size = 100, used = 2))
        l.put(full(fileId = "new", size = 100, used = 3))
        l.touch(old, now = 10)
        assertEquals(listOf("mid"), l.victims(limitBytes = 200).map { it.fileId })
        assertEquals(10, l.get(a, "old", "md5")!!.lastUsed)
        l.touch(old, now = 5)   // never moves time backwards
        assertEquals(10, l.get(a, "old", "md5")!!.lastUsed)
    }

    @Test
    fun pinnedEntriesAreNeverVictims() {
        val l = CacheLedger()
        l.put(full(fileId = "pinned", size = 100, used = 1, pinned = true))
        l.put(full(fileId = "plain", size = 100, used = 2))
        assertEquals(listOf("plain"), l.victims(limitBytes = 50).map { it.fileId })
        assertEquals(100, l.pinnedBytes())
        // Pinned content larger than the limit simply cannot be evicted.
        l.remove(l.get(a, "plain", "md5")!!)
        assertTrue(l.victims(limitBytes = 50).isEmpty())
    }

    @Test
    fun protectedEntryIsSkippedEvenWhenOldest() {
        val l = CacheLedger()
        val fresh = full(fileId = "fresh", size = 100, used = 0)   // oldest timestamp but just committed
        l.put(fresh)
        l.put(full(fileId = "other", size = 100, used = 5))
        assertEquals(listOf("other"), l.victims(limitBytes = 100, exclude = fresh.id).map { it.fileId })
    }

    @Test
    fun setPinnedMarksOnlyFullFilesOfThatAccount() {
        val l = CacheLedger()
        l.put(full(fileId = "f1", tag = "t", size = 1, used = 1))
        l.put(block(fileId = "f1", tag = "t", index = 0, size = 1, used = 1))
        l.put(full(account = b, fileId = "f1", tag = "t", size = 1, used = 1))
        l.put(full(fileId = "f2", tag = "t", size = 1, used = 1, pinned = true))   // no longer wanted

        val changed = l.setPinned(a, setOf(CacheEntry.fileKey("f1", "t")))
        assertEquals(2, changed)
        assertTrue(l.get(a, "f1", "t")!!.pinned)
        assertFalse(l.get(a, "f1", "t", 0)!!.pinned)
        assertFalse(l.get(b, "f1", "t")!!.pinned)
        assertFalse(l.get(a, "f2", "t")!!.pinned)
        assertEquals(0, l.setPinned(a, setOf(CacheEntry.fileKey("f1", "t"))))   // idempotent
        assertEquals(1, l.setPinned(a, emptySet()))
        assertFalse(l.get(a, "f1", "t")!!.pinned)
    }

    @Test
    fun snapshotRoundTripsThroughJson() {
        val l = CacheLedger()
        l.put(full(fileId = "f1", tag = "d41d8cd98f00b204e9800998ecf8427e", size = 10, used = 1, pinned = true))
        l.put(block(fileId = "f2", tag = "v3.docx", index = 4, size = 20, used = 2))
        val text = CacheLedger.encode(l)
        val back = CacheLedger.decode(text)
        assertEquals(l.totalBytes, back.totalBytes)
        assertEquals(l.all().toSet(), back.all().toSet())
        assertTrue(back.get(a, "f1", "d41d8cd98f00b204e9800998ecf8427e")!!.pinned)
        assertEquals(4, back.get(a, "f2", "v3.docx", 4)!!.block)
    }

    @Test
    fun decodeToleratesUnknownFieldsAndOmittedDefaults() {
        val text = """{"version":1,"entries":[{"account":"a@gmail.com","fileId":"f1","tag":"t","size":5,"lastUsed":9,"future":"x"}]}"""
        val l = CacheLedger.decode(text)
        val e = l.get(a, "f1", "t")!!
        assertTrue(e.isFull)
        assertFalse(e.pinned)
        assertEquals(5, l.totalBytes)
    }

    @Test
    fun cachePathsFollowTheLayoutAndParseBack() {
        val f = full(fileId = "1AbC-def", tag = "d41d8cd98f00b204e9800998ecf8427e", size = 1, used = 1)
        assertEquals("a@gmail.com/full/1AbC-def-d41d8cd98f00b204e9800998ecf8427e", CachePaths.relativePath(f))
        val bl = block(fileId = "1AbC-def", tag = "v12.docx", index = 3, size = 1, used = 1)
        assertEquals("a@gmail.com/blocks/1AbC-def-v12.docx/3", CachePaths.relativePath(bl))

        assertEquals("1AbC-def" to "d41d8cd98f00b204e9800998ecf8427e", CachePaths.parseFileKey("1AbC-def-d41d8cd98f00b204e9800998ecf8427e"))
        assertEquals("1AbC-def" to "v12.docx", CachePaths.parseFileKey("1AbC-def-v12.docx"))
        assertEquals("id_x" to "v0", CachePaths.parseFileKey("id_x-v0"))
        assertNull(CachePaths.parseFileKey("noSeparator"))
        assertNull(CachePaths.parseFileKey("id-"))
        assertNull(CachePaths.parseFileKey("-tag"))
        // Every tag DocMapper produces survives the round trip.
        for (tag in listOf("abcdef0123456789abcdef0123456789", "v7", "v7.xlsx", "v1700000000000.pdf")) {
            assertEquals("1AbC-def_9" to tag, CachePaths.parseFileKey(CacheEntry.fileKey("1AbC-def_9", tag)))
        }
    }
}
