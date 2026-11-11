package com.almodhayan.drivesync.scheduler

import android.content.Context
import android.content.Intent
import androidx.work.Constraints
import androidx.work.Data
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.OutOfQuotaPolicy
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkInfo
import androidx.work.WorkManager
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import com.almodhayan.drivesync.sync.SyncStateHolder
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map
import java.util.concurrent.TimeUnit
import javax.inject.Inject
import javax.inject.Singleton

@Singleton
class SyncScheduler @Inject constructor(
    @ApplicationContext private val context: Context,
    private val settings: SettingsRepository,
    private val state: SyncStateHolder,
) {
    private val wm get() = WorkManager.getInstance(context)

    suspend fun ensurePeriodic(policy: ExistingPeriodicWorkPolicy = ExistingPeriodicWorkPolicy.KEEP) {
        val cfg = settings.current()
        val minutes = cfg.globalIntervalMinutes.coerceAtLeast(15).toLong()
        val req = PeriodicWorkRequestBuilder<SyncWorker>(minutes, TimeUnit.MINUTES, (minutes / 3).coerceAtLeast(5), TimeUnit.MINUTES)
            .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
            .setInputData(Data.Builder().putString(SyncWorker.KEY_TRIGGER, "periodic").build())
            .addTag(TAG_SYNC)
            .build()
        wm.enqueueUniquePeriodicWork(PERIODIC, policy, req)
        val maint = PeriodicWorkRequestBuilder<MaintenanceWorker>(1, TimeUnit.DAYS).addTag(TAG_MAINT).build()
        wm.enqueueUniquePeriodicWork(MAINTENANCE, ExistingPeriodicWorkPolicy.KEEP, maint)
    }

    suspend fun reschedulePeriodic() = ensurePeriodic(ExistingPeriodicWorkPolicy.UPDATE)

    /**
     * Manual or instant trigger. One unique job per pair keeps bursts from piling up. Only a sync
     * the person started is expedited: automatic triggers spending the expedited quota would leave
     * the next "Sync now" waiting behind the system (#150 M2).
     */
    fun syncNow(pairId: String? = null, trigger: String = "manual", manual: Boolean = true, expedited: Boolean = manual) {
        // The pair's job is busy syncing, so this request would be dropped and the change it brings
        // would wait for the next scheduled check. Remember it: the pair runs once more when it finishes.
        if (pairId != null) { if (state.isRunning(pairId)) state.requestFollowUp(pairId) }
        else state.running.value.keys.forEach { state.requestFollowUp(it) }
        val data = Data.Builder().putString(SyncWorker.KEY_TRIGGER, trigger).putBoolean(SyncWorker.KEY_MANUAL, manual)
            .apply { if (pairId != null) putString(SyncWorker.KEY_PAIR_ID, pairId) }.build()
        val b = OneTimeWorkRequestBuilder<SyncWorker>()
            .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
            .setInputData(data).addTag(TAG_SYNC)
        if (expedited) b.setExpedited(OutOfQuotaPolicy.RUN_AS_NON_EXPEDITED_WORK_REQUEST)
        // "Sync now" must not wait behind an automatic request that is only waiting (a retry's back-off,
        // the network) and would then apply the pair's Wi-Fi or charging rules. Nothing is syncing the
        // pair, so replacing that request loses nothing.
        val policy = if (manual && pairId != null && !state.isRunning(pairId)) ExistingWorkPolicy.REPLACE else ExistingWorkPolicy.KEEP
        wm.enqueueUniqueWork(if (pairId != null) "sync-$pairId" else ALL_NOW, policy, b.build())
    }

    fun cancelAll() { wm.cancelAllWorkByTag(TAG_SYNC); ensurePeriodicLater() }

    /** Reads (or re-reads) an account's Drive folder list in the background; survives leaving the screen. */
    fun indexAccount(accountId: String) {
        val req = OneTimeWorkRequestBuilder<IndexWorker>()
            .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
            .setInputData(Data.Builder().putString(IndexWorker.KEY_ACCOUNT, accountId).build())
            .build()
        wm.enqueueUniqueWork("index-$accountId", ExistingWorkPolicy.KEEP, req)
    }

    private fun ensurePeriodicLater() {
        // cancelAllWorkByTag also removes the periodic job; re-create it on next app start or boot.
        context.sendBroadcast(Intent(context, InternalActionReceiver::class.java).setAction(InternalActionReceiver.ACTION_RESCHEDULE))
    }

    fun observeRunning(): Flow<Boolean> = wm.getWorkInfosByTagFlow(TAG_SYNC).map { list -> list.any { it.state == WorkInfo.State.RUNNING } }

    fun observeWorkInfos(): Flow<List<WorkInfo>> = wm.getWorkInfosByTagFlow(TAG_SYNC)

    companion object {
        const val PERIODIC = "periodic-sync"
        const val MAINTENANCE = "maintenance"
        const val ALL_NOW = "sync-all-now"
        const val TAG_SYNC = "sync"
        const val TAG_MAINT = "maintenance"
    }
}
