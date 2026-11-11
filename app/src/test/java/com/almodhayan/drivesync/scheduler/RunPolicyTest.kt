package com.almodhayan.drivesync.scheduler

import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.db.RunStatus
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** When a scheduled check runs a pair again, and when a worker asks to be retried. */
class RunPolicyTest {
    private val now = 1_800_000_000_000L
    private val min = 60_000L
    private val hour = 60 * min

    private fun pair(lastStatus: RunStatus?, ranAgo: Long?) = FolderPairEntity(id = "p1", accountId = "a@example.com", name = "Notes",
        localPath = "/storage/emulated/0/Notes", remoteFolderId = "r", remotePath = "/Notes",
        lastRunAt = ranAgo?.let { now - it }, lastStatus = lastStatus)

    private fun due(p: FolderPairEntity, trigger: String = "periodic", manual: Boolean = false) = RunPolicy.isDue(p, trigger, manual, now, hour)

    @Test
    fun aPairSyncedWithinItsInterval_waitsForTheNextCheck() {
        assertFalse(due(pair(RunStatus.SUCCESS, ranAgo = 10 * min)))
    }

    @Test
    fun aPairSyncedLongerAgoThanItsInterval_isDue() {
        assertTrue(due(pair(RunStatus.SUCCESS, ranAgo = 2 * hour)))
        assertTrue(due(pair(null, ranAgo = null)))
    }

    @Test
    fun aRunThatAndroidStoppedOrThatFailed_isPickedUpByTheNextCheck() {
        assertTrue("stopped", due(pair(RunStatus.CANCELLED, ranAgo = 5 * min)))
        assertTrue("failed", due(pair(RunStatus.FAILED, ranAgo = 5 * min)))
        assertTrue("some files failed", due(pair(RunStatus.PARTIAL, ranAgo = 5 * min)))
    }

    @Test
    fun aPairWaitingForApproval_isCheckedOnItsNormalSchedule() {
        assertFalse(due(pair(RunStatus.BLOCKED, ranAgo = 5 * min)))
    }

    @Test
    fun manualAndInstantSyncs_areAlwaysDue() {
        val fresh = pair(RunStatus.SUCCESS, ranAgo = min)
        assertTrue(due(fresh, manual = true))
        assertTrue(due(fresh, trigger = "instant"))
    }

    @Test
    fun failures_retryScheduledSyncsOnly_stallsRetryAlways_withinThreeAttempts() {
        assertTrue(RunPolicy.shouldRetry(anyFailure = true, anyStall = false, manual = false, attempt = 0))
        assertFalse("a manual sync reports its failure instead", RunPolicy.shouldRetry(anyFailure = true, anyStall = false, manual = true, attempt = 0))
        assertTrue("a stalled run always gets another go", RunPolicy.shouldRetry(anyFailure = true, anyStall = true, manual = true, attempt = 0))
        assertFalse(RunPolicy.shouldRetry(anyFailure = true, anyStall = true, manual = false, attempt = 3))
        assertFalse(RunPolicy.shouldRetry(anyFailure = false, anyStall = false, manual = false, attempt = 0))
    }
}
