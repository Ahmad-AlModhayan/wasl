package com.almodhayan.drivesync.scheduler

import android.content.Context
import androidx.hilt.work.HiltWorker
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.NetworkType
import androidx.work.Data
import androidx.work.ExistingWorkPolicy
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import com.almodhayan.drivesync.actions.DeletionReview
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Strings
import kotlinx.coroutines.CancellationException
import dagger.assisted.Assisted
import dagger.assisted.AssistedInject

/** "Restore on phone" from the approval notification: copies the files back without opening the app. */
@HiltWorker
class RestoreDeletionsWorker @AssistedInject constructor(
    @Assisted appContext: Context,
    @Assisted params: WorkerParameters,
    private val review: DeletionReview,
    private val notifications: SyncNotifications,
) : CoroutineWorker(appContext, params) {
    override suspend fun doWork(): Result {
        val pairId = inputData.getString(KEY_PAIR) ?: return Result.failure()
        return try {
            review.restore(pairId)
            notifications.clearFor(pairId)
            Result.success()
        } catch (e: CancellationException) {
            throw e
        } catch (e: AppError.NotFound) {
            // The pair was removed: nothing left to restore, and a retry can't change that.
            notifications.clearFor(pairId)
            Result.failure()
        } catch (e: Exception) {
            if (runAttemptCount < MAX_RETRIES) return Result.retry()
            // Out of retries: say so, with the way to try again (#150 M3 review).
            notifications.failed(
                pairId,
                Strings.get(R.string.notif_restore_failed_title),
                Strings.get(R.string.notif_restore_failed_body),
                Strings.get(R.string.notif_restore_failed_action),
            )
            Result.failure()
        }
    }

    companion object {
        const val KEY_PAIR = "pairId"
        private const val MAX_RETRIES = 5

        /** Waits for a connection: offline, the restore used to fail its retries within minutes (#150 M3). */
        fun request(pairId: String) = OneTimeWorkRequestBuilder<RestoreDeletionsWorker>()
            .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
            .setInputData(Data.Builder().putString(KEY_PAIR, pairId).build())
            .build()

        fun enqueue(context: Context, pairId: String) {
            val req = request(pairId)
            WorkManager.getInstance(context).enqueueUniqueWork("restore-$pairId", ExistingWorkPolicy.KEEP, req)
        }
    }
}
