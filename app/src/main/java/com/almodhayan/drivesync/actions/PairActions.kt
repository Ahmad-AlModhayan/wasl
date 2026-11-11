package com.almodhayan.drivesync.actions

import android.content.Context
import androidx.work.WorkManager
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.scheduler.SyncScheduler
import com.almodhayan.drivesync.engine.WaslRuntime
import com.almodhayan.drivesync.sync.SyncStateHolder
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map
import javax.inject.Inject
import javax.inject.Singleton

/** Everyday commands on pairs, in one place for screens, notifications, the tile and automation. */
@Singleton
class PairActions @Inject constructor(
    @ApplicationContext private val context: Context,
    private val db: Db,
    private val scheduler: SyncScheduler,
    private val runtime: WaslRuntime,
    private val state: SyncStateHolder,
) {
    fun syncNow(pairId: String) = scheduler.syncNow(pairId, "manual", manual = true)
    fun syncAll() = scheduler.syncNow(null, "manual", manual = true)

    /** Stops the pair now (files synced so far stay synced) and holds it until resumed. */
    suspend fun pause(pairId: String, reason: String = PAUSED_BY_YOU) {
        db.pairs().setPaused(pairId, true, reason)
        runtime.engine.cancel(pairId)
        WorkManager.getInstance(context).cancelUniqueWork("sync-$pairId")
    }

    suspend fun resume(pairId: String) {
        db.pairs().setPaused(pairId, false, null)
        scheduler.syncNow(pairId, "resume", manual = false)
    }

    suspend fun pauseAll() { db.pairs().getAll().forEach { pause(it.id) } }
    suspend fun resumeAll() { db.pairs().getAll().filter { it.paused }.forEach { resume(it.id) } }
    fun observeAllPaused(): Flow<Boolean> = db.pairs().observeAll().map { list -> list.isNotEmpty() && list.all { it.paused } }

    /** Pause every pair that is syncing right now (notification "Pause"). */
    suspend fun pauseRunning() { state.running.value.keys.toList().forEach { pause(it) } }

    /** Stops the current run without pausing the pair. */
    fun cancel(pairId: String) {
        runtime.engine.cancel(pairId)
        WorkManager.getInstance(context).cancelUniqueWork("sync-$pairId")
    }

    suspend fun setEnabled(pairId: String, enabled: Boolean) {
        db.pairs().setEnabled(pairId, enabled)
        if (!enabled) cancel(pairId)
    }

    /** Approve a stopped plan (large delete or first merge) once, and run it now. */
    suspend fun approveAndRun(pairId: String) {
        // The engine uses the approval once and clears the stop when the run goes ahead.
        db.pairs().setApproveOnce(pairId, true)
        scheduler.syncNow(pairId, "approved", manual = true)
    }

    /** Removes the pair and its sync records. Files on the phone and on Drive are not touched. */
    suspend fun remove(pairId: String) {
        cancel(pairId)
        // Removing the pair removes its baseline, history, conflicts and stopped plan with it, and
        // lets go of its folder grant when no other pair uses it (EngineData).
        db.pairs().delete(pairId)
    }

    companion object {
        /** The pause reason shown on the pair, in the app's language at the time of the pause. */
        val PAUSED_BY_YOU: String get() = Strings.get(R.string.actions_paused_by_you)
    }
}
