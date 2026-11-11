package com.almodhayan.drivesync.ondemand

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test
import java.io.IOException

/**
 * How the provider's proxy descriptor reads through the engine: whole blocks asked for once, a
 * seek costs one block, FUSE-sized reads are served from memory, and the end of the file is exact.
 */
class BlockWindowTest {
    private val block = BlockMath.BLOCK_SIZE
    private val size = 5L * block + 1234
    private val content = ByteArray(size.toInt()) { (it * 31 % 251).toByte() }
    private val asked = mutableListOf<Int>()

    private fun window(keep: Int = 3) = BlockWindow(size, keep) { idx ->
        asked += idx
        val from = BlockMath.blockOffset(idx).toInt()
        content.copyOfRange(from, from + BlockMath.blockLength(idx, size))
    }

    private fun read(w: BlockWindow, offset: Long, length: Int): ByteArray {
        val buf = ByteArray(length)
        val n = w.read(offset, buf, length)
        return buf.copyOf(n)
    }

    @Test fun `a seek asks for the one block it falls in`() {
        val w = window()
        val at = 3L * block + 77
        assertArrayEquals(content.copyOfRange(at.toInt(), at.toInt() + 4096), read(w, at, 4096))
        assertEquals(listOf(3), asked)
    }

    @Test fun `small reads through a block ask for it once`() {
        val w = window()
        var off = 0L
        while (off < 2L * block) {
            assertArrayEquals(content.copyOfRange(off.toInt(), off.toInt() + 128 * 1024), read(w, off, 128 * 1024))
            off += 128 * 1024
        }
        assertEquals(listOf(0, 1), asked)
        assertEquals(2, w.fetches)
    }

    @Test fun `a read across blocks joins them`() {
        val w = window()
        val at = 2L * block - 10
        assertArrayEquals(content.copyOfRange(at.toInt(), at.toInt() + 20), read(w, at, 20))
        assertEquals(listOf(1, 2), asked)
    }

    @Test fun `the end of the file is exact and past it is empty`() {
        val w = window()
        assertArrayEquals(content.copyOfRange(size.toInt() - 10, size.toInt()), read(w, size - 10, 4096))
        assertEquals(0, w.read(size, ByteArray(10), 10))
        assertEquals(0, w.read(size + 5, ByteArray(10), 10))
        assertEquals(listOf(5), asked)
    }

    @Test fun `only the last few blocks stay in memory`() {
        val w = window(keep = 2)
        read(w, 0, 10); read(w, 1L * block, 10); read(w, 2L * block, 10)
        read(w, 0, 10)   // evicted: asked again
        assertEquals(listOf(0, 1, 2, 0), asked)
        read(w, 2L * block + 5, 10)   // still there
        assertEquals(4, w.fetches)
    }

    @Test fun `a short block is an error, not short data`() {
        val w = BlockWindow(size) { ByteArray(10) }
        assertThrows(IOException::class.java) { w.read(0, ByteArray(100), 100) }
    }

    @Test fun `an empty file reads nothing`() {
        val w = BlockWindow(0) { error("never asked") }
        assertEquals(0, w.read(0, ByteArray(10), 10))
    }
}
