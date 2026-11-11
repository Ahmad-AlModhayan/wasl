package com.almodhayan.drivesync.ondemand

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** The two on-demand caches share one setting: together they never exceed it (above the floors). */
class CacheBudgetTest {
    private val mb = 1024L * 1024L

    @Test
    fun the_shares_add_up_to_the_setting() {
        for (setting in listOf(16, 64, 512, 2048, 10_000, 2049, 33)) {
            val total = CacheBudget.engineBytes(setting) + CacheBudget.filesBytes(setting)
            assertEquals("setting $setting MB", setting * mb, total)
        }
        assertEquals(1536 * mb, CacheBudget.engineBytes(2048))
        assertEquals(512 * mb, CacheBudget.filesBytes(2048))
    }

    @Test
    fun a_tiny_setting_still_leaves_each_cache_room() {
        assertTrue(CacheBudget.engineBytes(1) >= CacheBudget.MIN_ENGINE_MB * mb)
        assertTrue(CacheBudget.filesBytes(1) >= CacheBudget.MIN_FILES_MB * mb)
    }
}
