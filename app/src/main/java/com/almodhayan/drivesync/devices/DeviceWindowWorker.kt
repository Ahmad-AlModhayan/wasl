package com.almodhayan.drivesync.devices

import android.content.Context
import androidx.hilt.work.HiltWorker
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import com.almodhayan.drivesync.sync.Conditions
import dagger.assisted.Assisted
import dagger.assisted.AssistedInject
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.withTimeoutOrNull
import java.util.concurrent.TimeUnit

/**
 * The charging-on-Wi-Fi window (SRS FR-N12): when the person allowed it, WorkManager wakes this
 * while the phone charges on an unmetered network; the phone joins its devices for a few minutes,
 * syncs the pairs shared with them, and leaves again when the window ends or the phone is unplugged.
 */
@HiltWorker
class DeviceWindowWorker @AssistedInject constructor(
    @Assisted appContext: Context,
    @Assisted params: WorkerParameters,
    private val manager: DeviceSyncManager,
    private val settings: SettingsRepository,
    private val conditions: Conditions,
) : CoroutineWorker(appContext, params) {

    private fun windowHolds(enabled: Boolean): Boolean {
        val d = conditions.device()
        return Participation.chargingWindow(enabled, d.charging, d.wifi, d.metered)
    }

    override suspend fun doWork(): Result {
        if (!windowHolds(settings.current().devicesWhileCharging)) return Result.success()
        manager.enter(Window.CHARGING_WIFI)
        try {
            withTimeoutOrNull(30_000) { manager.open.first { it } }
            if (manager.open.value) manager.syncDevicePairs()
            // Stay reachable for a while, within WorkManager's 10-minute limit, while the window holds.
            val until = System.currentTimeMillis() + STAY_MS
            while (!isStopped && System.currentTimeMillis() < until && windowHolds(settings.current().devicesWhileCharging)) {
                delay(30_000)
            }
        } finally {
            manager.leave(Window.CHARGING_WIFI)
        }
        return Result.success()
    }

    companion object {
        private const val NAME = "devices-charging-wifi"
        private const val STAY_MS = 8 * 60_000L

        /** Arms or disarms the window to follow Settings. */
        fun schedule(context: Context, enabled: Boolean) {
            val wm = WorkManager.getInstance(context)
            if (!enabled) {
                wm.cancelUniqueWork(NAME)
                return
            }
            val req = PeriodicWorkRequestBuilder<DeviceWindowWorker>(30, TimeUnit.MINUTES)
                .setConstraints(Constraints.Builder().setRequiresCharging(true).setRequiredNetworkType(NetworkType.UNMETERED).build())
                .build()
            wm.enqueueUniquePeriodicWork(NAME, ExistingPeriodicWorkPolicy.KEEP, req)
        }
    }
}
