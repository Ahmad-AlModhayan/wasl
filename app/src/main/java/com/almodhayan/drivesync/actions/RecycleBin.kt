package com.almodhayan.drivesync.actions

import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.TrashEntity
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import com.almodhayan.drivesync.engine.WaslRuntime
import com.almodhayan.drivesync.scheduler.SyncScheduler
import com.almodhayan.drivesync.ui.components.Side
import com.kahflabs.wasl.ffi.store.BinEntry
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.combine
import javax.inject.Inject
import javax.inject.Singleton

data class BinItem(
    val entry: TrashEntity,
    val side: Side,                 // the side it was removed from
    val name: String,
    val pairName: String,
    val folder: String,             // human folder it came from
    val daysLeft: Int,
)

/** [restoredEntries] is the engine's record of what came back, for Undo. */
data class RestoreResult(
    val restored: List<TrashEntity>,
    val failed: List<Pair<TrashEntity, String>>,
    internal val restoredEntries: List<BinEntry> = emptyList(),
)

/**
 * Everything sync removed or replaced, from either side, for 30 days. Phone files sit in the app's
 * bin folder; Drive files sit in Drive's own bin. Restoring puts a file back where it was; if a newer
 * file is there now, that file goes into the bin first, so nothing is ever lost. The engine does
 * the file work.
 */
@Singleton
class RecycleBin @Inject constructor(
    private val db: Db,
    private val runtime: WaslRuntime,
    private val settings: SettingsRepository,
    private val scheduler: SyncScheduler,
) {
    fun observe(): Flow<List<BinItem>> = combine(db.trash().observeAll(), settings.settings, db.pairs().observeAll()) { rows, cfg, pairs ->
        val names = pairs.associate { it.id to it.name }
        val now = System.currentTimeMillis()
        rows.map { t ->
            val side = if (t.side == TrashEntity.SIDE_DRIVE) Side.Drive else Side.Phone
            val retention = if (side == Side.Drive) DRIVE_BIN_DAYS else cfg.trashRetentionDays
            val age = ((now - t.createdAt) / 86_400_000L).toInt()
            val folder = t.originalPath.substringBeforeLast('/', "").let { p ->
                if (side == Side.Phone) com.almodhayan.drivesync.ui.components.Fmt.humanLocalPath(p) else com.almodhayan.drivesync.ui.components.Fmt.humanRemotePath(p)
            }
            BinItem(t, side, t.displayName ?: t.originalPath.substringAfterLast('/'), names[t.pairId] ?: t.pairName ?: Strings.get(R.string.actions_removed_pair), folder, (retention - age).coerceAtLeast(0))
        }
    }

    suspend fun restore(ids: List<String>): RestoreResult {
        val before = db.trash().getMany(ids).associateBy { it.id }
        val r = runtime.engine.restoreBinEntries(ids)
        runtime.changed()
        val restoredIds = r.restored.map { it.entry.id }.toSet()
        r.restored.map { it.entry.pairId }.toSet().forEach { id -> if (db.pairs().get(id) != null) scheduler.syncNow(id, "restore", manual = false) }
        return RestoreResult(
            restored = before.values.filter { it.id in restoredIds },
            failed = r.failed.mapNotNull { f -> before[f.entryId]?.let { it to f.message } },
            restoredEntries = r.restored,
        )
    }

    /** Undo for the restore snackbar: moves the restored files back into the bin. */
    suspend fun undoRestore(result: RestoreResult) {
        runtime.engine.undoBinRestore(result.restoredEntries)
        runtime.changed()
    }

    /** Irreversible: removes the files for good (Drive files leave Drive's bin too). */
    suspend fun deleteForever(ids: List<String>): Int {
        val n = runtime.engine.deleteBinForever(ids).toInt()
        runtime.changed()
        return n
    }

    suspend fun emptyAll(): Int {
        val n = runtime.engine.emptyBin().toInt()
        runtime.changed()
        return n
    }

    companion object {
        /** Drive empties its own bin after 30 days. */
        const val DRIVE_BIN_DAYS = 30
    }
}
