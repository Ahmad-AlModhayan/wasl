package com.almodhayan.drivesync.scheduler

import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.db.RunStatus

/** When a scheduled check runs a pair again, and when a sync worker asks WorkManager to retry. */
object RunPolicy {
    /**
     * A scheduled check skips a pair that synced within its interval. A run that did not finish
     * (stopped by Android, failed, some files failed) is picked up by the next check instead of
     * waiting a whole interval, and a retry of a failed worker really runs it again.
     */
    fun isDue(pair: FolderPairEntity, trigger: String, manual: Boolean, now: Long, intervalMs: Long): Boolean {
        if (manual || trigger != "periodic") return true
        val last = pair.lastRunAt ?: return true
        if (pair.lastStatus == RunStatus.CANCELLED || pair.lastStatus == RunStatus.FAILED || pair.lastStatus == RunStatus.PARTIAL) return true
        return now - last >= intervalMs - 60_000L
    }

    /** Scheduled syncs retry failures; a stalled run is retried even when it was started by hand. */
    fun shouldRetry(anyFailure: Boolean, anyStall: Boolean, manual: Boolean, attempt: Int): Boolean =
        attempt < 3 && (anyStall || (anyFailure && !manual))
}
