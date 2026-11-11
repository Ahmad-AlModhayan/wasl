package com.almodhayan.drivesync.actions

import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.ConflictEntity
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.engine.WaslRuntime
import com.almodhayan.drivesync.engine.toEntity
import com.almodhayan.drivesync.scheduler.SyncScheduler
import com.almodhayan.drivesync.ui.components.ConflictChoice
import com.almodhayan.drivesync.ui.components.ConflictSideInfo
import com.almodhayan.drivesync.ui.components.Fmt
import com.kahflabs.wasl.ffi.engine.ConflictChoice as WChoice
import com.kahflabs.wasl.ffi.engine.ConflictUndo as WUndo
import javax.inject.Inject
import javax.inject.Singleton

/** A conflict as the compare screen shows it. */
data class ConflictDetails(
    val conflict: ConflictEntity,
    val pair: FolderPairEntity?,
    val name: String,
    val folder: String,                 // pair-relative parent, "" for the pair root
    val phone: ConflictSideInfo?,       // the copy edited on this phone (kept as the conflict copy)
    val drive: ConflictSideInfo?,       // the copy from Drive (now at the original name)
)

/** What a choice changed, so the snackbar's Undo can put it back (the engine's record of it). */
data class ConflictUndo(val conflictId: String, val choice: ConflictChoice, internal val engineUndo: WUndo? = null)

/**
 * After a two-way conflict the file's original name holds the Drive version and the phone's version
 * sits beside it as "name (conflicted copy from …)". Keep both just closes the conflict; Keep phone /
 * Keep Drive move the other copy to the recycle bin and let the next sync carry the choice to Drive.
 * The engine does the file work.
 */
@Singleton
class ConflictResolver @Inject constructor(
    private val db: Db,
    private val runtime: WaslRuntime,
    private val scheduler: SyncScheduler,
) {
    suspend fun details(conflictId: String): ConflictDetails? {
        val d = runtime.engine.conflictDetails(conflictId) ?: return null
        val pair = db.pairs().get(d.conflict.pairId)
        val phone = d.device?.let { ConflictSideInfo(Fmt.dayAndTime(it.mtimeMs), Strings.get(R.string.actions_conflict_size_phone, Fmt.bytes(it.size.toLong()))) }
        val drive = d.drive?.let { ConflictSideInfo(Fmt.dayAndTime(it.mtimeMs), Strings.get(R.string.actions_conflict_size_drive, Fmt.bytes(it.size.toLong()))) }
        return ConflictDetails(d.conflict.toEntity(), pair, d.name, d.folder, phone, drive)
    }

    suspend fun resolve(conflictId: String, choice: ConflictChoice): ConflictUndo {
        val pairId = db.conflicts().get(conflictId)?.pairId
        val undo = runtime.engine.resolveConflict(conflictId, choice.toEngine())
        runtime.changed()
        if (pairId != null && choice != ConflictChoice.KeepBoth) scheduler.syncNow(pairId, "conflict", manual = false)
        return ConflictUndo(conflictId, choice, undo)
    }

    suspend fun undo(u: ConflictUndo) {
        val pairId = db.conflicts().get(u.conflictId)?.pairId
        u.engineUndo?.let { runtime.engine.undoConflict(it) } ?: db.conflicts().unresolve(u.conflictId)
        runtime.changed()
        if (pairId != null && u.choice != ConflictChoice.KeepBoth) scheduler.syncNow(pairId, "conflict-undo", manual = false)
    }

    private fun ConflictChoice.toEngine(): WChoice = when (this) {
        ConflictChoice.KeepBoth -> WChoice.KEEP_BOTH
        ConflictChoice.KeepPhone -> WChoice.KEEP_DEVICE
        ConflictChoice.KeepDrive -> WChoice.KEEP_DRIVE
    }
}
