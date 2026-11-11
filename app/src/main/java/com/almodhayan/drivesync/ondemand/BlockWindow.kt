package com.almodhayan.drivesync.ondemand

import java.io.IOException

/**
 * Serves reads at any offset from whole 1 MiB blocks ([BlockMath]) that [fetch] returns, keeping
 * the last [keep] in memory. A proxy file descriptor is asked for ~128 KiB at a time, and every
 * fetch is a call into the engine's reader (which keeps the blocks on disk and fetches only those
 * it lacks), so a reader going through a block asks the engine once, and a seek costs one block.
 * Pure so it is unit-tested; not thread-safe (one per open file, called on its own thread).
 */
class BlockWindow(
    private val size: Long,
    private val keep: Int = 3,
    private val fetch: (index: Int) -> ByteArray,
) {
    private val memory = object : LinkedHashMap<Int, ByteArray>(8, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<Int, ByteArray>): Boolean = size > keep
    }

    /** Fetches so far (for tests and logs). */
    var fetches: Int = 0
        private set

    /**
     * Copies up to [length] bytes at [offset] into [data] and returns how many: fewer only at the
     * end of the file, 0 at or past it. A block shorter than the file says is an [IOException].
     */
    fun read(offset: Long, data: ByteArray, length: Int): Int {
        require(offset >= 0) { "negative offset" }
        if (length <= 0 || offset >= size) return 0
        val want = minOf(length.toLong(), size - offset, data.size.toLong()).toInt()
        var done = 0
        while (done < want) {
            val pos = offset + done
            val idx = BlockMath.blockIndex(pos)
            val block = block(idx)
            val within = (pos - BlockMath.blockOffset(idx)).toInt()
            val n = minOf(want - done, block.size - within)
            if (n <= 0) throw IOException("Block $idx has ${block.size} bytes; the file has more")
            System.arraycopy(block, within, data, done, n)
            done += n
        }
        return done
    }

    fun clear() = memory.clear()

    private fun block(idx: Int): ByteArray {
        memory[idx]?.let { return it }
        val expected = BlockMath.blockLength(idx, size)
        val bytes = fetch(idx)
        fetches++
        if (bytes.size != expected) throw IOException("Block $idx came back with ${bytes.size} of $expected bytes")
        memory[idx] = bytes
        return bytes
    }
}
