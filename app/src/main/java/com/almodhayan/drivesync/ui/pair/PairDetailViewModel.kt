package com.almodhayan.drivesync.ui.pair

import com.almodhayan.drivesync.sync.Conditions
import android.content.Context
import android.content.Intent
import android.os.Environment
import android.provider.DocumentsContract
import androidx.compose.runtime.Immutable
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.actions.PairActions
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.ConflictEntity
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.db.PairTotals
import com.almodhayan.drivesync.data.db.SyncEventEntity
import com.almodhayan.drivesync.data.db.SyncRunEntity
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import com.almodhayan.drivesync.fs.LocalFs
import com.almodhayan.drivesync.sync.PairProgress
import com.almodhayan.drivesync.sync.PlanSummary
import com.almodhayan.drivesync.sync.SyncStateHolder
import com.almodhayan.drivesync.ui.components.ChangeAction
import com.almodhayan.drivesync.ui.home.LoudSpec
import com.almodhayan.drivesync.ui.home.LoudStates
import com.almodhayan.drivesync.ui.home.PairRemoval
import com.almodhayan.drivesync.ui.status.PairStatusResolver
import com.almodhayan.drivesync.ui.status.PairStatusUi
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import kotlinx.serialization.json.Json
import java.io.File
import javax.inject.Inject

/** One finished action of the run in progress, as a ChangeRow shows it. */
@Immutable
data class LiveChange(val id: Long, val action: ChangeAction, val key: String, val name: String, val folder: String, val reason: String)

@Immutable
data class PairDetailUi(
    val loading: Boolean = true,
    val now: Long = 0L,
    /** Null once loaded = the pair was removed. */
    val pair: FolderPairEntity? = null,
    val status: PairStatusUi? = null,
    val loud: LoudSpec? = null,
    val progress: PairProgress? = null,
    val totals: PairTotals? = null,
    val conditions: String = "",
    val conflicts: List<ConflictEntity> = emptyList(),
    val runs: List<SyncRunEntity> = emptyList(),
    /** Live log of the current run, newest first, capped at [PairDetailViewModel.LIVE_LIMIT]. */
    val done: List<LiveChange> = emptyList(),
    val doneTotal: Int = 0,
    /** Conflicts met by the current run: both copies kept, waiting for a choice. */
    val waiting: List<LiveChange> = emptyList(),
)

@HiltViewModel
class PairDetailViewModel @Inject constructor(
    handle: SavedStateHandle,
    private val db: Db,
    stateHolder: SyncStateHolder,
    settings: SettingsRepository,
    private val json: Json,
    private val localFs: LocalFs,
    private val actions: PairActions,
    private val removal: PairRemoval,
    private val conditions: Conditions,
) : ViewModel() {

    val pairId: String = handle.get<String>("pairId") ?: ""

    private val progress: Flow<PairProgress?> = stateHolder.running.map { it[pairId] }.distinctUntilChanged()

    private val blocked: Flow<PlanSummary?> = db.blockedPlans().observe(pairId).map { b ->
        b?.let { runCatching { json.decodeFromString(PlanSummary.serializer(), it.summaryJson) }.getOrNull() }
    }

    private val totals: Flow<PairTotals?> = db.fileStates().observePairTotals()
        .map { list -> list.firstOrNull { it.pairId == pairId } }
        .distinctUntilChanged()

    /** Events of the run in progress (none when idle). */
    private val events: Flow<List<SyncEventEntity>> = progress.map { it?.runId }.distinctUntilChanged()
        .flatMapLatest { runId -> if (runId == null) flowOf(emptyList<SyncEventEntity>()) else db.events().observeForRun(runId) }

    private val clock: Flow<Long> = flow {
        while (true) {
            emit(System.currentTimeMillis())
            delay(30_000)
        }
    }

    private class Core(val pair: FolderPairEntity?, val progress: PairProgress?, val blocked: PlanSummary?, val totals: PairTotals?, val interval: Int)

    private val core: Flow<Core> = combine(db.pairs().observe(pairId), progress, blocked, totals, settings.settings) { pair, p, b, t, s ->
        Core(pair, p, b, t, s.globalIntervalMinutes)
    }

    val ui: StateFlow<PairDetailUi> = combine(
        core,
        db.conflicts().observeOpenForPair(pairId),
        db.runs().observeForPair(pairId, 20),
        events,
        clock,
    ) { c, conflicts, runs, ev, now -> build(c, conflicts, runs, ev, now) }
        .flowOn(Dispatchers.Default)
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), PairDetailUi())

    private val _messages = MutableSharedFlow<String>(extraBufferCapacity = 8)
    val messages: SharedFlow<String> = _messages

    fun syncNow() = io { actions.syncNow(pairId) }
    fun pause() = io { actions.pause(pairId) }
    fun resume() = io { actions.resume(pairId) }
    fun turnOn() = io { actions.setEnabled(pairId, true) }

    /** Forgets the sync baseline; the next sync compares every file again. No file is deleted. */
    fun resetSyncState() = io {
        db.fileStates().clearPair(pairId)
        _messages.tryEmit(Strings.get(R.string.pair_detail_state_reset))
    }

    /** Removes the pair (Home offers Undo). Finishes even if the screen goes away. True when removed. */
    suspend fun remove(): Boolean = viewModelScope.async(Dispatchers.IO) {
        try {
            removal.remove(pairId) != null
        } catch (e: CancellationException) {
            throw e
        } catch (t: Throwable) {
            _messages.tryEmit(AppError.wrap(t).userHint)
            false
        }
    }.await()

    /** Opens the phone folder in the system Files app. False when no app could open it. */
    fun openPhoneFolder(context: Context, localPath: String): Boolean = try {
        val primary = Environment.getExternalStorageDirectory()
        val volume: File? = (listOf(primary) + localFs.volumes()).firstOrNull { localPath == it.path || localPath.startsWith(it.path + "/") }
        val volumeId = if (volume == null || volume.path == primary.path) "primary" else volume.name
        val rel = if (volume == null) localPath.trimStart('/') else localPath.removePrefix(volume.path).trimStart('/')
        val uri = DocumentsContract.buildDocumentUri("com.android.externalstorage.documents", "$volumeId:$rel")
        context.startActivity(
            Intent(Intent.ACTION_VIEW).setDataAndType(uri, DocumentsContract.Document.MIME_TYPE_DIR).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        )
        true
    } catch (t: Throwable) {
        false
    }

    private fun io(block: suspend () -> Unit) {
        viewModelScope.launch(Dispatchers.IO) {
            try {
                block()
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _messages.tryEmit(AppError.wrap(t).userHint)
            }
        }
    }

    // ------------------------------------------------------------------ state

    private fun build(c: Core, conflicts: List<ConflictEntity>, runs: List<SyncRunEntity>, events: List<SyncEventEntity>, now: Long): PairDetailUi {
        val pair = c.pair ?: return PairDetailUi(loading = false, now = now)
        val heldBy = if (pair.enabled && !pair.paused) conditions.blockReason(pair) else null
        val st = PairStatusResolver.resolve(pair, c.progress, conflicts.size, c.blocked, now, heldBy)
        val live = if (c.progress == null) emptyList() else events.mapNotNull(::liveChange)
        val done = live.filter { it.action != ChangeAction.Conflict }
        return PairDetailUi(
            loading = false,
            now = now,
            pair = pair,
            status = st,
            loud = LoudStates.forPair(pair, st, c.blocked, conflicts.size, conflicts.firstOrNull()?.id),
            progress = c.progress,
            totals = c.totals,
            conditions = PairStatusResolver.conditions(pair, c.interval),
            conflicts = conflicts,
            runs = runs,
            done = done.asReversed().take(LIVE_LIMIT),
            doneTotal = done.size,
            waiting = live.filter { it.action == ChangeAction.Conflict },
        )
    }

    /** Engine event → ChangeRow. Folders made, info lines and errors are not file changes. */
    private fun liveChange(e: SyncEventEntity): LiveChange? {
        val key = e.path ?: return null
        val (action, reason) = when (e.action) {
            "UPLOAD" -> ChangeAction.Upload to Strings.get(R.string.pair_live_uploaded)
            "DOWNLOAD" -> ChangeAction.Download to Strings.get(R.string.pair_live_downloaded)
            "EXPORT" -> ChangeAction.Download to Strings.get(R.string.pair_live_exported)
            "DELETE_LOCAL" -> ChangeAction.Bin to when {
                e.message.startsWith("Removed from device after upload") -> Strings.get(R.string.pair_live_removed_after_upload)
                e.message.startsWith("Removed folder") -> Strings.get(R.string.pair_live_folder_removed_on_phone)
                else -> Strings.get(R.string.pair_live_binned_on_phone)
            }
            "DELETE_REMOTE" -> ChangeAction.Bin to Strings.get(if (e.message.contains("after download")) R.string.pair_live_binned_on_drive_after_download else R.string.pair_live_binned_on_drive)
            "CONFLICT" -> ChangeAction.Conflict to Strings.get(R.string.pair_live_both_kept)
            "SKIP" -> ChangeAction.Skip to Strings.get(R.string.pair_live_left_in_place)
            else -> return null
        }
        return LiveChange(e.id, action, key, key.substringAfterLast('/'), key.substringBeforeLast('/', ""), reason)
    }

    companion object {
        const val LIVE_LIMIT = 50
    }
}
