package com.almodhayan.drivesync.drive

import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeoutOrNull
import org.junit.After
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import kotlin.system.measureTimeMillis

/**
 * Transfers ask the limiter for up to 256 KB at a time. A limit below that (a restored backup can
 * carry any value) must slow transfers down, never stop them for good.
 */
class BandwidthLimiterTest {
    private val chunk = 256 * 1024

    @After
    fun reset() { BandwidthLimiter.bytesPerSecond = 0 }

    @Test
    fun aChunkLargerThanOneSecondOfBandwidth_stillGoesThrough() = runBlocking {
        BandwidthLimiter.bytesPerSecond = 100L * 1024

        val done = withTimeoutOrNull(3_000) { BandwidthLimiter.acquire(chunk) }

        assertNotNull("a 256 KB chunk waited forever under a 100 KB/s limit", done)
    }

    @Test
    fun theBlockingVariant_alsoLetsALargeChunkThrough() {
        BandwidthLimiter.bytesPerSecond = 100L * 1024
        val pool = Executors.newSingleThreadExecutor()
        try {
            val f = pool.submit { BandwidthLimiter.acquireBlocking(chunk) }
            f.get(3, TimeUnit.SECONDS)
        } finally {
            pool.shutdownNow()
        }
    }

    @Test
    fun largeChunks_stillKeepToTheLimitOnAverage() = runBlocking {
        BandwidthLimiter.bytesPerSecond = 100L * 1024
        assertNotNull("never hang the suite on a regression", withTimeoutOrNull(3_000) { BandwidthLimiter.acquire(chunk) })

        // The first chunk borrowed about 156 KB; the next one waits for that to be paid back.
        val second = measureTimeMillis { withTimeoutOrNull(5_000) { BandwidthLimiter.acquire(chunk) } }

        assertTrue("the second chunk went through in $second ms, faster than the limit allows", second >= 1_500)
    }
}
