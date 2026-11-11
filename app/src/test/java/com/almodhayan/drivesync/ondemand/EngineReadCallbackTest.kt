package com.almodhayan.drivesync.ondemand

import com.kahflabs.wasl.ffi.CacheUsage
import com.kahflabs.wasl.ffi.OnDemandFile
import com.kahflabs.wasl.ffi.OnDemandOrigin
import com.kahflabs.wasl.ffi.OnDemandReaderInterface
import com.kahflabs.wasl.ffi.VaultItem
import com.kahflabs.wasl.ffi.WaslException
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.assertThrows
import org.junit.Test

/**
 * A vault file's descriptor holds no decrypted bytes of its own: after the vault is locked, even a
 * read inside a block it already read fails, because every read asks the engine (and its lock
 * check) again. Plain Drive files keep their block window.
 */
class EngineReadCallbackTest {
    private val size = 3L * BlockMath.BLOCK_SIZE
    private val content = ByteArray(size.toInt()) { (it * 7 % 251).toByte() }

    private class FakeReader(private val content: ByteArray) : OnDemandReaderInterface {
        var locked = false
        var reads = 0

        override suspend fun read(origin: OnDemandOrigin, file: OnDemandFile, offset: ULong, len: ULong): ByteArray {
            reads++
            if (locked) throw WaslException.Locked("locked")
            val from = offset.toInt()
            return content.copyOfRange(from, minOf(content.size, from + len.toInt()))
        }

        override fun closeVault(vaultId: String) = Unit
        override fun forget(origin: OnDemandOrigin, file: OnDemandFile) = Unit
        override fun isComplete(origin: OnDemandOrigin, file: OnDemandFile) = false
        override fun isPinned(origin: OnDemandOrigin, file: OnDemandFile) = false
        override suspend fun pin(origin: OnDemandOrigin, file: OnDemandFile) = Unit
        override fun setLimit(limit: ULong) = Unit
        override fun unpin(origin: OnDemandOrigin, file: OnDemandFile) = Unit
        override fun usage(): CacheUsage = throw UnsupportedOperationException()
        override suspend fun vaultItems(origin: OnDemandOrigin): List<VaultItem> = emptyList()
    }

    private fun file() = OnDemandFile(id = "f1", version = "1", size = size.toULong())

    @Test
    fun a_locked_vault_file_reads_nothing_even_from_a_block_it_read_before() {
        val reader = FakeReader(content)
        val cb = EngineReadCallback(reader, OnDemandOrigin.Vault("me", "v1", "folder", null), file()) {}
        val buf = ByteArray(4096)
        assertEquals(4096, cb.onRead(0, 4096, buf))
        assertArrayEquals(content.copyOfRange(0, 4096), buf)

        reader.locked = true
        // Same block, a little further on: it must go to the engine and fail, not come from memory.
        assertThrows(Throwable::class.java) { cb.onRead(4096, 4096, buf) }
        assertEquals(2, reader.reads)
    }

    @Test
    fun a_vault_read_returns_exactly_the_bytes_asked_for_and_stops_at_the_end() {
        val reader = FakeReader(content)
        val cb = EngineReadCallback(reader, OnDemandOrigin.Vault("me", "v1", "folder", null), file()) {}
        val buf = ByteArray(100)
        assertEquals(10, cb.onRead(size - 10, 100, buf))
        assertArrayEquals(content.copyOfRange(content.size - 10, content.size), buf.copyOfRange(0, 10))
        assertEquals(0, cb.onRead(size, 100, buf))
    }

    /** Security review L4 (#136): a failed read logs neither the file's name nor the engine's detail text. */
    @Test
    fun a_failed_read_logs_no_file_name_for_plain_or_vault_files() {
        val f = OnDemandFile(id = "1AbCdEfGhIjK", version = "1", size = size.toULong())
        val error = WaslException.NotFound("Quarterly plan.pdf was not found")
        val line = EngineReadCallback.failureLine(f, error)
        assertFalse(line, line.contains("Quarterly"))
        assertTrue(line, line.contains("…fGhIjK") && line.contains("NotFound"))
    }
}
