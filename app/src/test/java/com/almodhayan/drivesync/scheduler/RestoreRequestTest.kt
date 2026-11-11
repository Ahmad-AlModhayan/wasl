package com.almodhayan.drivesync.scheduler

import androidx.work.NetworkType
import org.junit.Assert.assertEquals
import org.junit.Test

/** #150 M3: "Restore on phone" waits for a connection instead of failing its retries offline. */
class RestoreRequestTest {
    @Test fun theRestoreWaitsForAConnection() {
        assertEquals(NetworkType.CONNECTED, RestoreDeletionsWorker.request("p1").workSpec.constraints.requiredNetworkType)
    }
}
