package com.almodhayan.drivesync.ondemand

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class BlockMathTest {
    private val mib = BlockMath.BLOCK_SIZE.toLong()

    @Test
    fun blockSizeIsOneMebibyte() {
        assertEquals(1L shl 20, mib)
    }

    @Test
    fun indexAndOffsetAreInverse() {
        assertEquals(0, BlockMath.blockIndex(0))
        assertEquals(0, BlockMath.blockIndex(mib - 1))
        assertEquals(1, BlockMath.blockIndex(mib))
        assertEquals(5, BlockMath.blockIndex(5 * mib + 17))
        assertEquals(5 * mib, BlockMath.blockOffset(5))
        assertEquals(3000L * mib, BlockMath.blockOffset(3000))   // no Int overflow for multi-GB files
    }

    @Test
    fun blockCountRoundsUp() {
        assertEquals(0, BlockMath.blockCount(0))
        assertEquals(1, BlockMath.blockCount(1))
        assertEquals(1, BlockMath.blockCount(mib))
        assertEquals(2, BlockMath.blockCount(mib + 1))
        assertEquals(3, BlockMath.blockCount(3 * mib))
    }

    @Test
    fun lastBlockIsShort() {
        val size = 2 * mib + 100
        assertEquals(BlockMath.BLOCK_SIZE, BlockMath.blockLength(0, size))
        assertEquals(BlockMath.BLOCK_SIZE, BlockMath.blockLength(1, size))
        assertEquals(100, BlockMath.blockLength(2, size))
        assertEquals(0, BlockMath.blockLength(3, size))
    }

    @Test
    fun blocksForCoversTheRequestedRangeAndClipsToFile() {
        val size = 2 * mib + 100
        assertEquals(0..0, BlockMath.blocksFor(0, 10, size))
        assertEquals(0..1, BlockMath.blocksFor(mib - 5, 10, size))
        assertEquals(2..2, BlockMath.blocksFor(2 * mib, 4096, size))   // clipped to the 100 remaining bytes
        assertTrue(BlockMath.blocksFor(size, 10, size).isEmpty())
        assertTrue(BlockMath.blocksFor(10, 0, size).isEmpty())
        assertTrue(BlockMath.blocksFor(-1, 10, size).isEmpty())
    }
}
