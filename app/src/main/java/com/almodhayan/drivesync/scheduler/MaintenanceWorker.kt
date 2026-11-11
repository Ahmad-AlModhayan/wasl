package com.almodhayan.drivesync.scheduler

import android.content.Context
import androidx.hilt.work.HiltWorker
import androidx.work.CoroutineWorker
import androidx.work.WorkerParameters
import com.almodhayan.drivesync.actions.RecycleBin
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.TrashEntity
import com.almodhayan.drivesync.data.db.TrashReason
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import dagger.assisted.Assisted
import dagger.assisted.AssistedInject
import java.io.File

/**
 * Daily housekeeping: the engine empties bin entries past their 30 days (Drive empties its own bin)
 * and old history is trimmed.
 */
@HiltWorker
class MaintenanceWorker @AssistedInject constructor(
    @Assisted appContext: Context,
    @Assisted params: WorkerParameters,
    private val db: Db,
    private val runtime: com.almodhayan.drivesync.engine.WaslRuntime,
    private val settings: SettingsRepository,
) : CoroutineWorker(appContext, params) {
    override suspend fun doWork(): Result {
        val cfg = settings.current()
        val now = System.currentTimeMillis()
        val day = 86_400_000L
        // The engine empties expired bin entries (local files deleted, Drive entries forgotten) and
        // already keeps only the newest replaced copies of each file.
        runtime.engine.purgeExpiredBin()
        runtime.engine.pruneRuns(now - cfg.historyRetentionDays * day)
        db.uploadSessions().deleteOlderThan(now - 6 * day)
        runtime.changed()
        return Result.success()
    }
}