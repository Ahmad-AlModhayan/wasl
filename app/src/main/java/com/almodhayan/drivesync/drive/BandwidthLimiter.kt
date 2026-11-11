package com.almodhayan.drivesync.drive

import kotlinx.coroutines.delay

/**
 * Process-wide token bucket used by the Drive client's streaming loops. 0 = unlimited.
 * Kept as a plain object so the API client's constructor stays test-friendly.
 */
object BandwidthLimiter {
    @Volatile var bytesPerSecond: Long = 0
    private val lock = Any()
    private var allowance = 0.0
    private var lastRefill = System.nanoTime()

    /** Suspends as long as needed so that [bytes] fit the configured rate. */
    suspend fun acquire(bytes: Int) {
        while (true) {
            val waitMs = take(bytes)
            if (waitMs == 0L) return
            delay(waitMs)
        }
    }

    /** Blocking variant for OkHttp request bodies (writeTo runs on an OkHttp thread). */
    fun acquireBlocking(bytes: Int) {
        while (true) {
            val waitMs = take(bytes)
            if (waitMs == 0L) return
            Thread.sleep(waitMs)
        }
    }

    /** Takes [bytes] from the bucket and returns 0, or returns how long to wait before asking again. */
    private fun take(bytes: Int): Long {
        val rate = bytesPerSecond
        if (rate <= 0 || bytes <= 0) return 0L
        return synchronized(lock) {
            val now = System.nanoTime()
            allowance = minOf(rate.toDouble(), allowance + (now - lastRefill) / 1e9 * rate)
            lastRefill = now
            // The bucket never holds more than one second of bandwidth, so a bigger request would wait
            // forever. Let it through once the bucket is full and carry the rest as debt: the average
            // rate still holds.
            val need = minOf(bytes.toDouble(), rate.toDouble())
            if (allowance >= need) { allowance -= bytes; 0L } else (((need - allowance) / rate) * 1000).toLong().coerceAtLeast(5L)
        }
    }
}
