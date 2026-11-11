package com.almodhayan.drivesync.actions

import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.engine.WaslRuntime
import com.almodhayan.drivesync.engine.toKotlin
import com.almodhayan.drivesync.scheduler.SyncScheduler
import com.almodhayan.drivesync.sync.PlanSummary
import com.almodhayan.drivesync.ui.components.Side
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import javax.inject.Inject
import javax.inject.Singleton

/** One file or folder a stopped plan would delete. Folders carry the count and size of what's inside. */
data class PendingDeletion(
    val key: String,
    val name: String,
    val folder: String,
    val isDir: Boolean,
    val files: Int,
    val bytes: Long,
    val deleteOn: Side,
)

data class DeletionReviewModel(
    val pair: FolderPairEntity,
    val summary: PlanSummary,
    val items: List<PendingDeletion>,
    val onDrive: Int,        // files that would be deleted on Drive (they're gone from the phone)
    val onPhone: Int,        // files that would be deleted on the phone (they're gone from Drive)
    val bytes: Long,
    val limitPercent: Int,
    val limitFiles: Int,
)

/**
 * "Review deletions": shows every file a stopped plan would delete, with two recoverable answers.
 * Restore copies the files back to the side that lost them; approve lets the deletions through once
 * (they go to the recycle bin / Drive's bin for 30 days). Both are done by the engine.
 */
@Singleton
class DeletionReview @Inject constructor(
    private val db: Db,
    private val runtime: WaslRuntime,
    private val scheduler: SyncScheduler,
    private val pairs: PairActions,
    private val settings: com.almodhayan.drivesync.data.prefs.SettingsRepository,
) {
    suspend fun load(pairId: String): DeletionReviewModel {
        val pair = db.pairs().get(pairId) ?: throw AppError.NotFound(Strings.get(R.string.actions_pair_removed))
        val r = runtime.engine.deletionReview(pairId)
        val cfg = settings.current()
        return DeletionReviewModel(
            pair = pair,
            summary = r.summary.toKotlin(),
            items = r.items.map {
                PendingDeletion(it.key, it.name, it.folder, it.isDir, it.files.toInt(), it.bytes.toLong(), if (it.onDrive) Side.Drive else Side.Phone)
            },
            onDrive = r.filesOnDrive.toInt(),
            onPhone = r.filesOnDevice.toInt(),
            bytes = r.bytes.toLong(),
            limitPercent = cfg.massDeleteThresholdPercent,
            limitFiles = cfg.massDeleteMinFiles,
        )
    }

    /** Copies every file the plan would delete back to the side that lost it. Returns how many were restored. */
    suspend fun restore(pairId: String): Int {
        db.pairs().get(pairId) ?: throw AppError.NotFound(Strings.get(R.string.actions_pair_removed))
        val n = runtime.engine.restoreDeletions(pairId).toInt()
        runtime.changed()
        withContext(Dispatchers.IO) { scheduler.syncNow(pairId, "restored", manual = false) }
        return n
    }

    suspend fun approve(pairId: String) = pairs.approveAndRun(pairId)
}
