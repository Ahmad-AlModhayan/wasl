package com.almodhayan.drivesync.ondemand

/** Arithmetic for the 1 MiB streaming blocks. Pure so it is unit-tested. */
object BlockMath {
    const val BLOCK_SIZE: Int = 1 shl 20

    fun blockIndex(offset: Long): Int = (offset / BLOCK_SIZE).toInt()

    fun blockOffset(index: Int): Long = index.toLong() * BLOCK_SIZE

    fun blockCount(fileSize: Long): Int = if (fileSize <= 0) 0 else ((fileSize - 1) / BLOCK_SIZE + 1).toInt()

    /** Bytes in block [index] of a file of [fileSize] bytes; 0 when the index is past the end. */
    fun blockLength(index: Int, fileSize: Long): Int {
        val start = blockOffset(index)
        if (start >= fileSize) return 0
        return minOf(BLOCK_SIZE.toLong(), fileSize - start).toInt()
    }

    /** Inclusive range of block indexes that cover [offset, offset + length), clipped to the file. */
    fun blocksFor(offset: Long, length: Int, fileSize: Long): IntRange {
        val end = minOf(offset + length, fileSize)
        if (length <= 0 || offset < 0 || offset >= end) return IntRange.EMPTY
        return blockIndex(offset)..blockIndex(end - 1)
    }
}
