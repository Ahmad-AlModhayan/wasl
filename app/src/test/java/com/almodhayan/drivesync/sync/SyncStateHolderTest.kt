package com.almodhayan.drivesync.sync

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** A change that arrives while its pair is syncing is remembered, so the pair runs once more after. */
class SyncStateHolderTest {
    @Test
    fun aFollowUpRequestedDuringARun_isTakenOnce() {
        val s = SyncStateHolder()
        s.start(PairProgress("p1", "r1"))

        s.requestFollowUp("p1")

        assertTrue(s.takeFollowUp("p1"))
        assertFalse(s.takeFollowUp("p1"))
    }

    @Test
    fun followUpsArePerPair() {
        val s = SyncStateHolder()
        s.requestFollowUp("p1")

        assertFalse(s.takeFollowUp("p2"))
        assertTrue(s.takeFollowUp("p1"))
    }
}
