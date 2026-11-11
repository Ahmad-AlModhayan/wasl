package com.almodhayan.drivesync.scheduler

import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.db.RunStatus
import com.almodhayan.drivesync.sync.PairProgress
import com.almodhayan.drivesync.sync.Phase
import com.almodhayan.drivesync.ui.components.Fmt
import org.junit.Assert.assertEquals
import org.junit.Test

/** The always-on status notification: one short title and one line about what Drive Sync is doing. */
class StatusLineTest {
    private val now = 1_800_000_000_000L
    private val min = 60_000L

    private fun pair(id: String, name: String = id, paused: Boolean = false, lastRunAt: Long? = now - 10 * min,
                     lastStatus: RunStatus? = RunStatus.SUCCESS, lastError: String? = null, pendingApproval: Boolean = false,
                     intervalMinutes: Int = 0) =
        FolderPairEntity(id = id, accountId = "a@example.com", name = name, localPath = "/storage/emulated/0/$id",
            remoteFolderId = "r-$id", remotePath = "/$id", paused = paused, lastRunAt = lastRunAt, lastStatus = lastStatus,
            lastError = lastError, pendingApproval = pendingApproval, intervalMinutes = intervalMinutes)

    private fun status(
        pairs: List<FolderPairEntity>,
        running: Map<String, PairProgress> = emptyMap(),
        lastProgressAt: Map<String, Long> = emptyMap(),
        instant: Boolean = true,
        interval: Int = 60,
        live: Map<String, String> = emptyMap(),
    ) = StatusLine.of(StatusInput(now, pairs, running, lastProgressAt, instant, interval, live))

    private fun transferring(id: String, done: Long, total: Long, startedAgoMin: Long = 2) =
        PairProgress(id, "run-$id", phase = Phase.TRANSFERRING, totalActions = 100, doneActions = 42,
            bytesTotal = total, bytesDone = done, startedAt = now - startedAgoMin * min)

    @Test
    fun syncingOnePair_namesItWithItsPercentage() {
        val s = status(listOf(pair("p1", "LifeOS")), running = mapOf("p1" to transferring("p1", 42, 100)))
        assertEquals("Syncing LifeOS · 42%", s.title)
        assertEquals("About 3 min left", s.text)
    }

    @Test
    fun syncingBeforeTransfers_saysItIsComparing() {
        val p = PairProgress("p1", "r", phase = Phase.SCANNING_LOCAL, startedAt = now - min)
        val s = status(listOf(pair("p1", "LifeOS")), running = mapOf("p1" to p))
        assertEquals("Syncing LifeOS", s.title)
        assertEquals("Comparing folders…", s.text)
    }

    @Test
    fun syncingTwoPairs_namesTheFirstAndCountsTheRest() {
        val s = status(listOf(pair("p1", "LifeOS"), pair("p2", "Camera")),
            running = mapOf("p1" to transferring("p1", 10, 100), "p2" to transferring("p2", 30, 100)))
        assertEquals("Syncing LifeOS and 1 more", s.title)
    }

    @Test
    fun noProgressForTenMinutes_isStalled() {
        val s = status(listOf(pair("p1", "LifeOS")), running = mapOf("p1" to transferring("p1", 42, 100, startedAgoMin = 30)),
            lastProgressAt = mapOf("p1" to now - 14 * min))
        assertEquals("Stalled · LifeOS", s.title)
        assertEquals("No progress for 14 min. It retries on its own.", s.text)
    }

    @Test
    fun idleWithInstantSync_waitsForChangesAndGivesTheNextCheck() {
        val s = status(listOf(pair("p1", lastRunAt = now - 10 * min)), instant = true, interval = 60)
        assertEquals("Waiting for changes", s.title)
        assertEquals("Next check ${Fmt.clock(now + 50 * min)}", s.text)
    }

    @Test
    fun idleWithoutInstantSync_isUpToDateAndUsesTheEarliestPairCheck() {
        val s = status(listOf(pair("p1", lastRunAt = now - 10 * min), pair("p2", lastRunAt = now - 50 * min, intervalMinutes = 15)),
            instant = false, interval = 60)
        assertEquals("Up to date", s.title)
        assertEquals("Next check soon", s.text)
    }

    @Test
    fun allPairsPaused_saysSo() {
        val s = status(listOf(pair("p1", paused = true), pair("p2", paused = true)))
        assertEquals("Paused", s.title)
        assertEquals("All pairs are paused", s.text)
    }

    @Test
    fun aPairWaitingForApproval_needsAttention() {
        val s = status(listOf(pair("p1", "LifeOS", pendingApproval = true, lastStatus = RunStatus.BLOCKED, lastError = "21 files would be deleted")))
        assertEquals("Needs attention", s.title)
        assertEquals("LifeOS: 21 files would be deleted", s.text)
    }

    @Test
    fun aFailedPair_needsAttention() {
        val s = status(listOf(pair("p1", "LifeOS", lastStatus = RunStatus.FAILED, lastError = "No internet connection")))
        assertEquals("Needs attention", s.title)
        assertEquals("LifeOS: No internet connection", s.text)
    }

    /** E2E 2026-10: offline nothing runs, so no run says why; the notification read "Waiting for changes". */
    @Test
    fun aPairHeldRightNow_saysWhyEvenWithoutARun() {
        val s = status(listOf(pair("p1", "LifeOS")), live = mapOf("p1" to "No internet connection"))
        assertEquals("Waiting", s.title)
        assertEquals("LifeOS: No internet connection", s.text)
    }

    @Test
    fun aPairHeldByItsConditions_isWaiting() {
        val s = status(listOf(pair("p1", "LifeOS", lastStatus = RunStatus.SKIPPED, lastError = "Waiting for Wi-Fi")))
        assertEquals("Waiting", s.title)
        assertEquals("LifeOS: Waiting for Wi-Fi", s.text)
    }

    @Test
    fun aRunAndroidStopped_isWaitingAndSaysWhy() {
        // Files are still left to sync, so "Waiting for changes" would be wrong.
        val s = status(listOf(pair("p1", "LifeOS", lastStatus = RunStatus.CANCELLED,
            lastError = "Stopped by Android before it finished. The next sync carries on.")))
        assertEquals("Waiting", s.title)
        assertEquals("LifeOS: Stopped by Android before it finished. The next sync carries on.", s.text)
    }
}
