package com.almodhayan.drivesync.scheduler

import com.almodhayan.drivesync.data.db.FolderPairEntity
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** When the status service runs (REQ-0009 plus the always-on status line) and which pairs it watches. */
class MonitorPolicyTest {
    private fun pair(id: String = "p1", enabled: Boolean = true, paused: Boolean = false, instant: Boolean = true) =
        FolderPairEntity(id = id, accountId = "a@example.com", name = id, localPath = "/storage/emulated/0/$id",
            remoteFolderId = "r-$id", remotePath = "/$id", enabled = enabled, paused = paused, instantSync = instant)

    @Test
    fun runsWhenAnyPairIsOn_evenWithInstantSyncOff() {
        assertTrue(MonitorPolicy.shouldRun(listOf(pair(instant = false))))
    }

    @Test
    fun runsWhilePairsArePaused_soTheStatusCanSaySo() {
        assertTrue(MonitorPolicy.shouldRun(listOf(pair(paused = true))))
    }

    @Test
    fun staysOffWithoutPairs() {
        assertFalse(MonitorPolicy.shouldRun(emptyList()))
    }

    @Test
    fun staysOffWhenEveryPairIsTurnedOff() {
        assertFalse(MonitorPolicy.shouldRun(listOf(pair("a", enabled = false), pair("b", enabled = false))))
    }

    @Test
    fun watchesAnActiveInstantPairWhileInstantSyncIsOn() {
        assertTrue(MonitorPolicy.watches(instantMonitoring = true, pair = pair()))
    }

    @Test
    fun watchesNothingWhileInstantSyncIsOff() {
        assertFalse(MonitorPolicy.watches(instantMonitoring = false, pair = pair()))
    }

    @Test
    fun doesNotWatchPausedDisabledOrScheduleOnlyPairs() {
        assertFalse(MonitorPolicy.watches(true, pair(paused = true)))
        assertFalse(MonitorPolicy.watches(true, pair(enabled = false)))
        assertFalse(MonitorPolicy.watches(true, pair(instant = false)))
    }

    // #133: folder-picker pairs set to sync "when files change" got no change trigger at all.

    @Test
    fun aFolderPickerPairOnSharedStorageIsWatchedByItsPath() {
        val saf = pair().copy(localPath = "content://com.android.externalstorage.documents/tree/primary%3AWaslTest%2FNotes")
        assertEquals("/storage/emulated/0/WaslTest/Notes", MonitorPolicy.watchPath(saf))
        assertEquals("/storage/emulated/0/p1", MonitorPolicy.watchPath(pair()))
    }

    @Test
    fun aFolderPickerPairFromAnotherAppHasNoPathButStillWakesOnMediaChanges() {
        val other = pair(id = "cloud").copy(localPath = "content://com.example.provider/tree/root")
        assertNull(MonitorPolicy.watchPath(other))
        val saf = pair(id = "notes").copy(localPath = "content://com.android.externalstorage.documents/tree/primary%3ANotes")
        val path = pair(id = "path")
        // The path pair's watcher failed to start: it has no trigger. Folder-picker pairs always wake.
        assertEquals(setOf("notes", "cloud"), MonitorPolicy.mediaTargets(listOf(other, saf, path), watched = emptySet()).toSet())
        assertEquals(setOf("notes", "cloud", "path"), MonitorPolicy.mediaTargets(listOf(other, saf, path), watched = setOf("path")).toSet())
        // A folder-picker pair that has its own watcher isn't woken twice (#133 review).
        assertEquals(setOf("cloud"), MonitorPolicy.mediaTargets(listOf(other, saf, path), watched = setOf("notes")).toSet())
        // The editor stops offering "when files change" for the folder another app provides.
        assertFalse(com.almodhayan.drivesync.core.SafPaths.watchable(other.localPath))
        assertTrue(com.almodhayan.drivesync.core.SafPaths.watchable(saf.localPath))
    }
}
