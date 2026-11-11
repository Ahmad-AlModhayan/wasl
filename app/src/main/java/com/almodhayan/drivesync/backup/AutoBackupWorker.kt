package com.almodhayan.drivesync.backup

import android.content.Context
import androidx.hilt.work.HiltWorker
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import dagger.assisted.Assisted
import dagger.assisted.AssistedInject
import java.util.concurrent.TimeUnit

/**
 * Writes the configuration to every account's Drive app data when "Back up automatically" is on.
 * Scheduled daily by [schedulePeriodic] (Application.onCreate) and shortly after configuration
 * changes by [scheduleSoon].
 */
@HiltWorker
class AutoBackupWorker @AssistedInject constructor(
    @Assisted appContext: Context,
    @Assisted params: WorkerParameters,
    private val backup: ConfigBackup,
    private val settings: SettingsRepository,
) : CoroutineWorker(appContext, params) {

    override suspend fun doWork(): Result {
        if (!settings.current().autoBackupConfig) return Result.success()
        val result = backup.backupAll()
        // Only retry when nothing got through and the cause looks temporary (network, rate limit, 5xx).
        val retry = result.allFailed && result.failed.values.any { it.isTransient } && runAttemptCount < 3
        return if (retry) Result.retry() else Result.success()
    }

    companion object {
        const val PERIODIC = "config-backup-periodic"
        const val SOON = "config-backup-soon"
    }
}

/** Daily backup while online. KEEP: an existing schedule is left untouched. Call from Application.onCreate. */
fun ConfigBackup.schedulePeriodic(context: Context) {
    val request = PeriodicWorkRequestBuilder<AutoBackupWorker>(1, TimeUnit.DAYS, 4, TimeUnit.HOURS)
        .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
        .setInitialDelay(1, TimeUnit.HOURS)
        .build()
    WorkManager.getInstance(context.applicationContext).enqueueUniquePeriodicWork(AutoBackupWorker.PERIODIC, ExistingPeriodicWorkPolicy.KEEP, request)
}

/** One backup about 10 minutes from now. REPLACE: repeated edits collapse into a single upload. Call after pair/vault/settings changes. */
fun ConfigBackup.scheduleSoon(context: Context) {
    val request = OneTimeWorkRequestBuilder<AutoBackupWorker>()
        .setInitialDelay(10, TimeUnit.MINUTES)
        .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
        .build()
    WorkManager.getInstance(context.applicationContext).enqueueUniqueWork(AutoBackupWorker.SOON, ExistingWorkPolicy.REPLACE, request)
}
