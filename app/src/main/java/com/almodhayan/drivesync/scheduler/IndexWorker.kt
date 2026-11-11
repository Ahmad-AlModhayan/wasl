package com.almodhayan.drivesync.scheduler

import android.content.Context
import androidx.hilt.work.HiltWorker
import androidx.work.CoroutineWorker
import androidx.work.WorkerParameters
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.drive.DriveNodeCache
import dagger.assisted.Assisted
import dagger.assisted.AssistedInject

/** Reads an account's Drive folder list (the first crawl can take a minute) without depending on a screen staying open. */
@HiltWorker
class IndexWorker @AssistedInject constructor(
    @Assisted appContext: Context,
    @Assisted params: WorkerParameters,
    private val cache: DriveNodeCache,
    private val db: Db,
) : CoroutineWorker(appContext, params) {
    override suspend fun doWork(): Result {
        val account = inputData.getString(KEY_ACCOUNT) ?: return Result.failure()
        if (db.accounts().get(account) == null) return Result.success()
        return try {
            cache.refresh(account)
            Result.success()
        } catch (t: Throwable) {
            val err = AppError.wrap(t)
            if (err is AppError.Auth) { db.accounts().setAuthError(account, err.message ?: Strings.get(R.string.drive_auth_reauthorize)); Result.failure() }
            else if (err.isTransient && runAttemptCount < 5) Result.retry() else Result.failure()
        }
    }

    companion object { const val KEY_ACCOUNT = "account" }
}
