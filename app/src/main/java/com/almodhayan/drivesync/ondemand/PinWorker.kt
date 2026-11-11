package com.almodhayan.drivesync.ondemand

import android.content.Context
import android.util.Log
import androidx.hilt.work.HiltWorker
import androidx.work.CoroutineWorker
import androidx.work.WorkerParameters
import dagger.assisted.Assisted
import dagger.assisted.AssistedInject
import kotlinx.coroutines.CancellationException

private const val TAG = "OnDemand"
private const val MAX_RETRIES = 5

/**
 * Runs [PinManager.syncPins]: downloads pinned content that is missing or outdated and uploads
 * edits whose write-back failed. Scheduled on every pin change ([PinManager.schedule]) and every
 * six hours ([PinManager.ensurePeriodic]); both require a network connection.
 */
@HiltWorker
class PinWorker @AssistedInject constructor(
    @Assisted appContext: Context,
    @Assisted params: WorkerParameters,
    private val pins: PinManager,
) : CoroutineWorker(appContext, params) {

    override suspend fun doWork(): Result {
        val r = try {
            pins.syncPins()
        } catch (e: CancellationException) {
            throw e
        } catch (t: Throwable) {
            Log.e(TAG, "pin sync failed", t)
            return if (runAttemptCount < MAX_RETRIES) Result.retry() else Result.failure()
        }
        return if (r.transient && runAttemptCount < MAX_RETRIES) Result.retry() else Result.success()
    }
}
