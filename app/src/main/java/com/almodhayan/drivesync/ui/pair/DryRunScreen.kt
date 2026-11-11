package com.almodhayan.drivesync.ui.pair

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import android.util.Log
import androidx.annotation.StringRes
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import com.almodhayan.drivesync.actions.PairActions
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.ErrorKind
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.ConflictEntity
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.db.RunStatus
import com.almodhayan.drivesync.data.db.SyncEventEntity
import com.almodhayan.drivesync.data.db.SyncRunEntity
import com.kahflabs.wasl.ffi.core.Action
import com.almodhayan.drivesync.engine.WaslRuntime
import com.almodhayan.drivesync.engine.toKotlin
import com.almodhayan.drivesync.sync.Conditions
import com.almodhayan.drivesync.sync.PairProgress
import com.almodhayan.drivesync.sync.Phase
import com.almodhayan.drivesync.sync.PlanSummary
import com.almodhayan.drivesync.sync.StatusText
import com.almodhayan.drivesync.sync.SyncStateHolder
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.activity.ButtonLabel
import com.almodhayan.drivesync.ui.activity.SearchField
import com.almodhayan.drivesync.ui.activity.atTime
import com.almodhayan.drivesync.ui.activity.changeOf
import com.almodhayan.drivesync.ui.activity.durationText
import com.almodhayan.drivesync.ui.activity.runBadge
import com.almodhayan.drivesync.ui.activity.runCounts
import com.almodhayan.drivesync.ui.activity.skipReason
import com.almodhayan.drivesync.ui.common.AppScreen
import com.almodhayan.drivesync.ui.common.EmptyState
import com.almodhayan.drivesync.ui.common.EmptyStateAction
import com.almodhayan.drivesync.ui.components.BottomActionBar
import com.almodhayan.drivesync.ui.components.Callout
import com.almodhayan.drivesync.ui.components.CalloutTone
import com.almodhayan.drivesync.ui.components.ChangeAction
import com.almodhayan.drivesync.ui.components.ChangeRow
import com.almodhayan.drivesync.ui.components.ChangeSummary
import com.almodhayan.drivesync.ui.components.ChipModel
import com.almodhayan.drivesync.ui.components.DsLinearProgress
import com.almodhayan.drivesync.ui.components.FileKind
import com.almodhayan.drivesync.ui.components.FilterChipRow
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.MenuAction
import com.almodhayan.drivesync.ui.components.OverflowMenu
import com.almodhayan.drivesync.ui.components.PairDirection
import com.almodhayan.drivesync.ui.components.SectionHeader
import com.almodhayan.drivesync.ui.components.StatusBanner
import com.almodhayan.drivesync.ui.components.StatusChip
import com.almodhayan.drivesync.ui.components.SummaryItem
import com.almodhayan.drivesync.ui.components.SyncProgress
import com.almodhayan.drivesync.ui.components.SyncProgressModel
import com.almodhayan.drivesync.ui.components.UiAction
import com.almodhayan.drivesync.ui.components.showMessage
import com.almodhayan.drivesync.ui.status.PairStatusResolver
import com.almodhayan.drivesync.ui.theme.Ds
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import com.almodhayan.drivesync.ui.theme.SyncStatus
import com.almodhayan.drivesync.ui.theme.tabular
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import javax.inject.Inject

private const val TAG = "Wasl"

/**
 * Dry run (docs/design/project/components/Screen05DryRun): the plan before anything moves, then the same
 * screen as the live log once Start sync is tapped.
 *
 * The plan comes from one inspection with the guard on (`approve = false`): its actions are exactly what
 * an approved run does (the guard only adds the stop), and its summary says whether a stop is pending.
 */
@HiltViewModel
class DryRunViewModel @Inject constructor(
    handle: SavedStateHandle,
    private val db: Db,
    private val runtime: WaslRuntime,
    private val pairActions: PairActions,
    syncState: SyncStateHolder,
    private val conditions: Conditions,
    private val json: Json,
) : ViewModel() {

    val pairId: String = handle.get<String>("pairId").orEmpty()

    enum class Filter(@StringRes val label: Int) {
        All(R.string.pair_dryrun_filter_all), ToDrive(R.string.common_to_drive), ToPhone(R.string.common_to_phone),
        ToBin(R.string.pair_dryrun_filter_to_bin), Conflicts(R.string.pair_dryrun_filter_conflicts), Skipped(R.string.common_skipped),
    }

    @Immutable
    data class PlanRow(
        val id: String,
        val key: String,
        val name: String,
        val path: String,
        val action: ChangeAction,
        val filter: Filter,
        val kind: FileKind,
        val size: Long?,
        val reason: String,
    )

    /** [notes]: what needs the user's attention, the plan's warnings among them. */
    @Immutable
    data class Plan(val summary: PlanSummary, val rows: List<PlanRow>, val counts: Map<Filter, Int>, val firstSync: Boolean, val notes: List<String> = summary.warnings) {
        /** A normal run would stop for these deletions, so they go through Review deletions first. */
        val stopsForDeletions: Boolean get() = summary.blockedKind == "deletions" || summary.blockedKind == "wipe"
        /** Rows that change something (everything but skipped). */
        val changes: Int get() = counts[Filter.All] ?: 0
    }

    sealed interface Preview {
        /** [phase] and [checked] come from the engine while it compares (null before it reports). */
        data class Loading(val startedAt: Long, val phase: Phase? = null, val checked: Int = 0) : Preview
        data class Failed(val hint: String) : Preview
        data object Missing : Preview
        data class Ready(val plan: Plan) : Preview
    }

    @Immutable
    data class DoneRow(
        val id: Long,
        val action: ChangeAction,
        val name: String,
        val path: String,
        val kind: FileKind,
        val size: String?,
        val reason: String,
        val conflictId: String? = null,
    )

    /** The live log after Start. */
    data class Live(
        val startedAt: Long,
        val runId: String? = null,
        val progress: SyncProgressModel? = null,   // while the run is going
        val run: SyncRunEntity? = null,            // its record; final once finishedAt is set
        val done: List<DoneRow> = emptyList(),     // newest first, capped
        val doneTotal: Int = 0,
        val waiting: List<DoneRow> = emptyList(),  // conflicts: both copies kept until you choose
        val failed: List<DoneRow> = emptyList(),
        val blocked: PlanSummary? = null,          // when the run stopped to ask
    ) {
        val finished: Boolean get() = progress == null && run?.finishedAt != null
    }

    private class LiveInputs(val start: Long?, val progress: PairProgress?, val last: SyncRunEntity?, val pair: FolderPairEntity?)

    val pair: StateFlow<FolderPairEntity?> = db.pairs().observe(pairId).stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    private val _preview = MutableStateFlow<Preview>(Preview.Loading(System.currentTimeMillis()))
    val preview: StateFlow<Preview> = _preview

    private val liveStart = MutableStateFlow<Long?>(null)

    /** Set once Start sync is tapped (or when the pair was already syncing): the screen is then the live log. */
    val liveSince: StateFlow<Long?> = liveStart

    private val _messages = Channel<String>(Channel.BUFFERED)
    val messages: Flow<String> = _messages.receiveAsFlow()

    /** Sizes from the plan, so finished rows can still show them. */
    @Volatile private var sizes: Map<String, Long> = emptyMap()
    private var loadJob: Job? = null

    private val progressFlow: Flow<PairProgress?> = syncState.running.map { it[pairId] }.distinctUntilChanged()
    private val latestRun: Flow<SyncRunEntity?> = db.runs().observeForPair(pairId, 1).map { it.firstOrNull() }

    private val inputs: Flow<LiveInputs> = combine(liveStart, progressFlow, latestRun, db.pairs().observe(pairId)) { start, p, last, pr ->
        LiveInputs(start, p, last, pr)
    }

    private val runIdFlow: Flow<String?> = inputs.map { runIdOf(it) }.distinctUntilChanged()

    private val runEvents: Flow<List<SyncEventEntity>> = runIdFlow.flatMapLatest { id ->
        if (id == null) flowOf(emptyList()) else db.events().observeForRun(id)
    }

    private val blockedSummary: Flow<PlanSummary?> = db.blockedPlans().observe(pairId).map { b -> b?.let { decode(it.summaryJson) } }

    val live: StateFlow<Live?> = combine(inputs, runEvents, db.conflicts().observeOpenForPair(pairId), blockedSummary) { i, events, open, blocked ->
        val start = i.start
        if (start == null) null else buildLive(i, start, events, open, blocked)
    }.flowOn(Dispatchers.Default).stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    init {
        val running = syncState.running.value[pairId]
        if (running != null) liveStart.value = running.startedAt else loadPreview()
    }

    fun loadPreview() {
        loadJob?.cancel()
        loadJob = viewModelScope.launch {
            _preview.value = Preview.Loading(System.currentTimeMillis())
            try {
                val p = withContext(Dispatchers.IO) { db.pairs().get(pairId) }
                if (p == null) {
                    _preview.value = Preview.Missing
                    return@launch
                }
                // Which step the engine is on, and how many files it checked, while it compares.
                val progress = launch {
                    while (true) {
                        delay(500)
                        val now = withContext(Dispatchers.IO) { runtime.engine.previewProgress(p.id) } ?: continue
                        _preview.update { cur -> if (cur is Preview.Loading) cur.copy(phase = now.phase.toKotlin(), checked = now.checked.toInt()) else cur }
                    }
                }
                val result = try {
                    runtime.engine.preview(p.id)
                } finally {
                    progress.cancel()
                }
                // The run log has these for a sync; a preview has no run, so they go to logcat.
                (result.notes + result.infos).forEach { Log.i(TAG, "preview ${p.id}: $it") }
                val plan = buildPlan(p, result)
                sizes = plan.rows.mapNotNull { r -> r.size?.let { r.key to it } }.toMap()
                _preview.value = Preview.Ready(plan)
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _preview.value = Preview.Failed(hintOf(t))
            }
        }
    }

    /** Leaving the screen stops the comparison (dropping the call stops it too; this is the explicit stop). */
    override fun onCleared() {
        runCatching { runtime.engine.cancelPreview(pairId) }
    }

    /** Start sync: approve this plan once and run it now. The screen turns into the live log. */
    fun startSync() {
        if (liveStart.value != null) return
        liveStart.value = System.currentTimeMillis()
        viewModelScope.launch {
            try {
                withContext(Dispatchers.IO) { pairActions.approveAndRun(pairId) }
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _messages.send(hintOf(t))
            }
        }
    }

    fun pause() {
        viewModelScope.launch {
            try {
                withContext(Dispatchers.IO) { pairActions.pause(pairId) }
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _messages.send(hintOf(t))
            }
        }
    }

    /** Carries on after Pause: the same approved sync, then the pair is no longer paused. */
    fun resume() {
        liveStart.value = System.currentTimeMillis()
        viewModelScope.launch {
            try {
                withContext(Dispatchers.IO) {
                    pairActions.approveAndRun(pairId)
                    pairActions.resume(pairId)
                }
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _messages.send(hintOf(t))
            }
        }
    }

    /** Back to a fresh plan (after a run stopped to ask about a first sync). */
    fun previewAgain() {
        liveStart.value = null
        loadPreview()
    }

    /** Whether the phone has a connection (a queued sync waits for one). */
    suspend fun online(): Boolean = withContext(Dispatchers.IO) { runCatching { conditions.device().online }.getOrDefault(true) }

    // ------------------------------------------------------------------ plan

    private fun buildPlan(pair: FolderPairEntity, preview: com.kahflabs.wasl.ffi.engine.Preview): Plan {
        val root = rootName(pair)
        val rows = ArrayList<PlanRow>()
        val sized = preview.sizes.mapValues { it.value.toLong() }
        for (a in preview.plan.actions) {
            val row = when (a) {
                is Action.Upload -> planRow(a.key, a.key, root, ChangeAction.Upload, Filter.ToDrive, a.local.size.toLong(), uploadReason(a))
                is Action.Download -> planRow(a.key, a.key, root, ChangeAction.Download, Filter.ToPhone, a.remote.size?.toLong(), downloadReason(a))
                is Action.Export -> planRow(a.key, a.localRel, root, ChangeAction.Download, Filter.ToPhone, a.remote.size?.toLong(), Strings.get(R.string.pair_reason_google_doc))
                is Action.DeleteLocal -> planRow(
                    a.key, a.key, root, ChangeAction.Bin, Filter.ToBin,
                    if (a.isDir) null else sized[a.key], deleteReason(a.reason, a.isDir, onDrive = true), a.isDir,
                )
                is Action.DeleteRemote -> planRow(
                    a.key, a.key, root, ChangeAction.Bin, Filter.ToBin,
                    if (a.isDir) null else sized[a.key], deleteReason(a.reason, a.isDir, onDrive = false), a.isDir,
                )
                is Action.Conflict -> planRow(a.key, a.key, root, ChangeAction.Conflict, Filter.Conflicts, a.local.size.toLong(), Strings.get(R.string.pair_reason_changed_on_both))
                is Action.Skip -> planRow(a.key, a.key, root, ChangeAction.Skip, Filter.Skipped, sized[a.key], skipReason(a.reason))
                else -> null
            }
            if (row != null) rows += row
        }
        rows.sortWith(compareBy<PlanRow>({ actionOrder(it.action) }, { it.key.lowercase() }))
        val counts = Filter.entries.associateWith { f ->
            if (f == Filter.All) rows.count { it.filter != Filter.Skipped } else rows.count { it.filter == f }
        }
        val summary = preview.plan.summary.toKotlin()
        return Plan(summary, rows, counts, firstSync = pair.lastSuccessAt == null, notes = (summary.warnings + preview.notes).distinct())
    }

    private fun planRow(key: String, nameFrom: String, root: String, action: ChangeAction, filter: Filter, size: Long?, reason: String, isDir: Boolean = false): PlanRow {
        val name = nameFrom.substringAfterLast('/')
        return PlanRow("${action.name}:$key", key, name, pathOf(root, key), action, filter, FileKind.of(name, isDir), size, reason)
    }

    private fun uploadReason(a: Action.Upload): String {
        val r = a.reason.lowercase()
        return when {
            r.startsWith("conflict") -> Strings.get(R.string.pair_reason_both_phone_wins)
            r.contains("missing on drive") -> Strings.get(R.string.pair_reason_missing_on_drive)
            r.startsWith("re-uploading") -> Strings.get(R.string.pair_reason_deleted_on_drive_copied_back)
            r.contains("overwriting drive change") -> Strings.get(R.string.pair_reason_drive_changed_phone_wins)
            a.existingRemoteId != null -> Strings.get(R.string.pair_reason_changed_on_phone)
            else -> Strings.get(R.string.pair_reason_new)
        }
    }

    private fun downloadReason(a: Action.Download): String {
        val r = a.reason.lowercase()
        return when {
            r.startsWith("conflict") -> Strings.get(R.string.pair_reason_both_drive_wins)
            r.startsWith("restoring") -> Strings.get(R.string.pair_reason_deleted_on_phone_copied_back)
            r.contains("after local delete") -> Strings.get(R.string.pair_reason_drive_changed_after_phone_delete)
            r.contains("missing on device") -> Strings.get(R.string.pair_reason_missing_on_phone)
            r.contains("overwriting device change") -> Strings.get(R.string.pair_reason_phone_changed_drive_wins)
            a.overwrites -> Strings.get(R.string.pair_reason_changed_on_drive)
            else -> Strings.get(R.string.pair_reason_new_on_drive)
        }
    }

    /** [onDrive] says where the file was deleted (the reason it goes to the bin on the other side): Drive or the phone. */
    private fun deleteReason(raw: String, isDir: Boolean, onDrive: Boolean): String = Strings.get(
        when {
            raw.contains("replaced by a regular file", ignoreCase = true) -> R.string.pair_reason_replaced_on_drive
            raw.contains("mirror", ignoreCase = true) -> when {
                isDir && onDrive -> R.string.pair_reason_folder_not_on_drive
                isDir -> R.string.pair_reason_folder_not_on_phone
                onDrive -> R.string.pair_reason_not_on_drive
                else -> R.string.pair_reason_not_on_phone
            }
            else -> when {
                isDir && onDrive -> R.string.pair_reason_folder_deleted_on_drive
                isDir -> R.string.pair_reason_folder_deleted_on_phone
                onDrive -> R.string.pair_reason_deleted_on_drive
                else -> R.string.pair_reason_deleted_on_phone
            }
        },
    )

    private fun actionOrder(a: ChangeAction): Int = when (a) {
        ChangeAction.Conflict -> 0
        ChangeAction.Bin -> 1
        ChangeAction.Upload -> 2
        ChangeAction.Download -> 3
        ChangeAction.Rename -> 4
        ChangeAction.Skip -> 5
    }

    /** The folder rows are relative to: "Work" for /storage/emulated/0/Documents/Work. */
    private fun rootName(pair: FolderPairEntity): String = Fmt.humanLocalPath(pair.localPath).substringAfterLast(" / ")

    private fun pathOf(root: String, key: String): String = key.substringBeforeLast('/', "").let { if (it.isEmpty()) root else "$root/$it" }

    // ------------------------------------------------------------------ live log

    private fun runIdOf(i: LiveInputs): String? {
        val start = i.start ?: return null
        return i.progress?.runId ?: i.last?.takeIf { it.startedAt >= start - START_SLACK_MS }?.id
    }

    private fun buildLive(i: LiveInputs, start: Long, events: List<SyncEventEntity>, open: List<ConflictEntity>, blocked: PlanSummary?): Live {
        val runId = runIdOf(i)
        val run = i.last?.takeIf { runId != null && it.id == runId }
        val root = i.pair?.let { rootName(it) }.orEmpty()
        val openByKey = open.associate { it.key to it.id }
        val known = sizes
        val done = ArrayList<DoneRow>()
        val waiting = ArrayList<DoneRow>()
        val failed = ArrayList<DoneRow>()
        for (e in events.asReversed()) {
            if (e.runId != runId) continue
            val key = e.path ?: continue
            val name = key.substringAfterLast('/')
            val path = pathOf(root, key)
            if (e.action == "ERROR") {
                failed += DoneRow(e.id, ChangeAction.Skip, name, path, FileKind.of(name), null, e.detail ?: e.message)
                continue
            }
            val change = changeOf(e) ?: continue
            if (change.action == ChangeAction.Skip) continue
            val conflict = change.action == ChangeAction.Conflict
            val row = DoneRow(e.id, change.action, name, path, FileKind.of(name), known[key]?.let { Fmt.bytes(it) }, change.reason, if (conflict) openByKey[key] else null)
            if (conflict) waiting += row else done += row
        }
        val p = i.progress
        val pr = i.pair
        val progress = if (p != null && pr != null) PairStatusResolver.model(p, pr) else null
        return Live(
            startedAt = start, runId = runId, progress = progress, run = run,
            done = done.take(DONE_CAP), doneTotal = done.size, waiting = waiting, failed = failed,
            blocked = if (run?.status == RunStatus.BLOCKED) blocked else null,
        )
    }

    private fun decode(s: String): PlanSummary? = runCatching { json.decodeFromString(PlanSummary.serializer(), s) }.getOrNull()

    private fun hintOf(t: Throwable): String {
        val e = AppError.wrap(t)
        return e.message?.takeIf { e is AppError.NotFound && it.isNotBlank() } ?: e.userHint
    }

    companion object {
        private const val DONE_CAP = 100
        private const val START_SLACK_MS = 2_000L
    }
}

@Composable
fun DryRunScreen(nav: AppNav, pairId: String) {
    val vm: DryRunViewModel = hiltViewModel()
    val since by vm.liveSince.collectAsStateWithLifecycle()
    val pair by vm.pair.collectAsStateWithLifecycle()
    val snackbar = remember { SnackbarHostState() }
    LaunchedEffect(vm) { vm.messages.collect { snackbar.showMessage(it) } }

    val start = since
    if (start == null) {
        val preview by vm.preview.collectAsStateWithLifecycle()
        DryRunPreview(nav, vm, preview, pair, snackbar)
    } else {
        val live by vm.live.collectAsStateWithLifecycle()
        DryRunLive(nav, vm, pairId, live ?: DryRunViewModel.Live(start), pair, snackbar)
    }
}

// ------------------------------------------------------------------ preview

@Composable
private fun DryRunPreview(nav: AppNav, vm: DryRunViewModel, preview: DryRunViewModel.Preview, pair: FolderPairEntity?, snackbar: SnackbarHostState) {
    var searching by rememberSaveable { mutableStateOf(false) }
    var query by rememberSaveable { mutableStateOf("") }
    var filter by rememberSaveable { mutableStateOf(DryRunViewModel.Filter.All) }
    val ready = preview as? DryRunViewModel.Preview.Ready
    val firstSync = ready?.plan?.firstSync ?: (pair != null && pair.lastSuccessAt == null)
    val searchLabel = stringResource(R.string.pair_dryrun_search_changes)

    AppScreen(
        title = stringResource(if (firstSync) R.string.pair_dryrun_title_first_sync else R.string.pair_dryrun_title_changes),
        subtitle = pair?.name,
        onBack = { nav.back() },
        actions = {
            if (ready != null && !searching) {
                IconButton(onClick = { searching = true }) { Icon(Sym.Search, contentDescription = searchLabel) }
            }
        },
        bottomBar = {
            if (ready != null) {
                if (ready.plan.stopsForDeletions) {
                    BottomActionBar(hint = stringResource(R.string.pair_dryrun_nothing_until_decide)) {
                        TextButton(onClick = { nav.back() }, modifier = Modifier.testTag("dryrun_cancel")) { Text(stringResource(R.string.common_cancel)) }
                    }
                } else {
                    BottomActionBar {
                        TextButton(onClick = { nav.back() }, modifier = Modifier.testTag("dryrun_cancel")) { Text(stringResource(R.string.common_cancel)) }
                        Button(onClick = { vm.startSync() }, modifier = Modifier.testTag("dryrun_start")) { ButtonLabel(stringResource(R.string.pair_dryrun_start_sync), Sym.Sync) }
                    }
                }
            }
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        when (preview) {
            is DryRunViewModel.Preview.Loading -> DryRunComparing(preview.startedAt, preview.phase, preview.checked, Modifier.padding(padding))
            is DryRunViewModel.Preview.Failed -> EmptyState(Sym.SyncProblem, stringResource(R.string.pair_dryrun_compare_failed), preview.hint, Modifier.padding(padding)) {
                EmptyStateAction(stringResource(R.string.common_try_again), onClick = { vm.loadPreview() })
            }
            DryRunViewModel.Preview.Missing -> EmptyState(Sym.Folder, stringResource(R.string.pair_removed_title), stringResource(R.string.pair_removed_body), Modifier.padding(padding)) {
                EmptyStateAction(stringResource(R.string.common_back), onClick = { nav.back() })
            }
            is DryRunViewModel.Preview.Ready -> DryRunPlan(
                plan = preview.plan,
                pairName = pair?.name.orEmpty(),
                searching = searching,
                query = query,
                onQuery = { query = it },
                onCloseSearch = { searching = false; query = "" },
                filter = filter,
                onFilter = { filter = it },
                onReview = { nav.reviewDeletions(vm.pairId) },
                modifier = Modifier.padding(padding),
            )
        }
    }
}

/** "Comparing folders…" with the time it has taken so far. */
@Composable
private fun DryRunComparing(startedAt: Long, phase: Phase?, checked: Int, modifier: Modifier) {
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(startedAt) {
        while (true) {
            delay(1_000)
            now = System.currentTimeMillis()
        }
    }
    Box(modifier.fillMaxSize().padding(horizontal = Space.s32), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(Space.s12)) {
            StatusChip(SyncStatus.Syncing, label = stringResource(R.string.pair_dryrun_comparing))
            DsLinearProgress(null, Modifier.widthIn(max = 280.dp))
            val step = when {
                phase == Phase.PLANNING && checked > 0 -> pluralStringResource(R.plurals.components_progress_comparing_checked, checked, Fmt.count(checked))
                phase == null || phase == Phase.PREPARING -> null
                else -> PairUi.phaseLabel(phase)
            }
            if (step != null) Text(step, style = MaterialTheme.typography.bodyMedium.tabular, textAlign = TextAlign.Center)
            Text(durationText(now - startedAt), style = MaterialTheme.typography.bodyMedium.tabular, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Text(
                stringResource(R.string.pair_dryrun_preview_changes_nothing),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
            )
        }
    }
}

@Composable
private fun DryRunPlan(
    plan: DryRunViewModel.Plan,
    pairName: String,
    searching: Boolean,
    query: String,
    onQuery: (String) -> Unit,
    onCloseSearch: () -> Unit,
    filter: DryRunViewModel.Filter,
    onFilter: (DryRunViewModel.Filter) -> Unit,
    onReview: () -> Unit,
    modifier: Modifier,
) {
    val s = plan.summary
    val visible = remember(plan, filter, query) {
        val q = query.trim()
        plan.rows.filter { r ->
            val inFilter = if (filter == DryRunViewModel.Filter.All) r.filter != DryRunViewModel.Filter.Skipped else r.filter == filter
            inFilter && (q.isEmpty() || r.name.contains(q, ignoreCase = true) || r.path.contains(q, ignoreCase = true))
        }
    }
    val chips = DryRunViewModel.Filter.entries
        .filter { f -> f == DryRunViewModel.Filter.All || f == filter || (plan.counts[f] ?: 0) > 0 }
        .map { f -> ChipModel(stringResource(f.label), selected = f == filter, count = plan.counts[f] ?: 0) { onFilter(f) } }

    LazyColumn(modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = Space.s24)) {
        if (searching) item(key = "search") { SearchField(query, onQuery, stringResource(R.string.pair_dryrun_search_changes), onCloseSearch) }
        item(key = "summary") {
            Column(
                Modifier.padding(start = Space.s16, end = Space.s16, top = Space.s8, bottom = Space.s8),
                verticalArrangement = Arrangement.spacedBy(Space.s16),
            ) {
                if (plan.stopsForDeletions) {
                    StatusBanner(
                        SyncStatus.Approval,
                        StatusText.approvalTitle(s),
                        body = StatusText.approvalBody(pairName, s),
                        primary = UiAction(StatusText.approvalAction(s)) { onReview() },
                    )
                }
                ChangeSummary(unchanged = s.unchanged, items = dryRunSummaryItems(s))
            }
        }
        item(key = "chips") { FilterChipRow(chips) }
        if (visible.isEmpty()) {
            item(key = "empty") {
                Text(
                    if (plan.changes == 0 && query.isBlank() && filter == DryRunViewModel.Filter.All) stringResource(R.string.pair_dryrun_already_match)
                    else stringResource(R.string.pair_dryrun_no_match),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(Space.s16),
                )
            }
        }
        items(visible, key = { it.id }) { r ->
            ChangeRow(r.action, r.name, r.path, kind = r.kind, size = r.size?.let { Fmt.bytes(it) }, reason = r.reason)
        }
        if (plan.notes.isNotEmpty()) {
            item(key = "notes") { SectionHeader(stringResource(R.string.pair_dryrun_notes)) }
            items(plan.notes) { w -> Callout(w, Modifier.padding(horizontal = Space.s16, vertical = Space.s4), tone = CalloutTone.Warning) }
        }
    }
}

/** What stays leads (in [ChangeSummary]); then to Drive, to phone, to recycle bin, conflicts. */
private fun dryRunSummaryItems(s: PlanSummary): List<SummaryItem> {
    val toPhone = s.downloads + s.exports
    // "Then delete" methods move what they sent to a bin after the transfer (E2E 2026-10).
    val toBin = s.deleteLocal + s.deleteRemote + s.movedAfterTransfer
    return listOf(
        SummaryItem(ChangeAction.Upload, s.uploads, if (s.uploads > 0) Fmt.bytes(s.uploadBytes) else null),
        SummaryItem(ChangeAction.Download, toPhone, if (toPhone > 0) Fmt.bytes(s.downloadBytes) else null),
        SummaryItem(ChangeAction.Bin, toBin),
        SummaryItem(ChangeAction.Conflict, s.conflicts, label = Strings.get(R.string.pair_dryrun_filter_conflicts)),
    )
}

// ------------------------------------------------------------------ live log

@Composable
private fun DryRunLive(nav: AppNav, vm: DryRunViewModel, pairId: String, live: DryRunViewModel.Live, pair: FolderPairEntity?, snackbar: SnackbarHostState) {
    val notStarted = live.progress == null && live.run?.finishedAt == null
    var online by remember { mutableStateOf(true) }
    LaunchedEffect(notStarted) {
        if (!notStarted) {
            online = true
        } else {
            while (true) {
                online = vm.online()
                delay(2_000)
            }
        }
    }
    val paused = pair?.paused == true
    val runId = live.runId
    val openPairLabel = stringResource(R.string.pair_dryrun_open_pair)
    val syncDetailsLabel = stringResource(R.string.pair_dryrun_sync_details)
    val menu = buildList {
        add(MenuAction(openPairLabel, Sym.FolderOpen) { nav.pairDetail(pairId) })
        if (runId != null) add(MenuAction(syncDetailsLabel, Sym.List) { nav.runDetail(runId) })
    }

    AppScreen(
        title = pair?.name ?: stringResource(R.string.pair_dryrun_title_sync),
        subtitle = listOfNotNull(pair?.let { PairDirection.of(it.mode).title }, stringResource(R.string.pair_dryrun_started_at, Fmt.clock(live.startedAt))).joinToString(" · "),
        onBack = { nav.back() },
        actions = { OverflowMenu(menu) },
        bottomBar = {
            when {
                live.finished -> BottomActionBar {
                    if (live.run?.status == RunStatus.CANCELLED && paused) {
                        FilledTonalButton(onClick = { vm.resume() }) { ButtonLabel(stringResource(R.string.common_resume), Sym.PlayArrow) }
                    }
                    Button(onClick = { nav.back() }, modifier = Modifier.testTag("run_done")) { Text(stringResource(R.string.common_done)) }
                }
                notStarted && paused -> BottomActionBar {
                    FilledTonalButton(onClick = { vm.resume() }) { ButtonLabel(stringResource(R.string.common_resume), Sym.PlayArrow) }
                    Button(onClick = { nav.back() }, modifier = Modifier.testTag("run_done")) { Text(stringResource(R.string.common_done)) }
                }
                else -> BottomActionBar(hint = stringResource(R.string.pair_dryrun_can_leave)) {
                    FilledTonalButton(onClick = { vm.pause() }) { ButtonLabel(stringResource(R.string.common_pause), Sym.Pause) }
                }
            }
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        LazyColumn(Modifier.padding(padding).fillMaxSize(), contentPadding = PaddingValues(bottom = Space.s24)) {
            item(key = "head") {
                Column(
                    Modifier.padding(start = Space.s16, end = Space.s16, top = Space.s8, bottom = Space.s8),
                    verticalArrangement = Arrangement.spacedBy(Space.s16),
                ) {
                    DryRunLiveHead(
                        live = live, pair = pair, paused = paused, online = online,
                        onReview = { nav.reviewDeletions(pairId) },
                        onPreview = { vm.previewAgain() },
                    )
                }
            }
            item(key = "done-h") { SectionHeader(stringResource(R.string.pair_dryrun_section_done)) }
            if (live.done.isEmpty()) {
                item(key = "done-empty") {
                    Text(
                        stringResource(if (live.finished) R.string.pair_dryrun_nothing_changed else R.string.pair_dryrun_files_show_up),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(horizontal = Space.s16, vertical = Space.s8),
                    )
                }
            }
            items(live.done, key = { "d${it.id}" }) { r ->
                ChangeRow(r.action, r.name, r.path, kind = r.kind, size = r.size, reason = r.reason)
            }
            if (runId != null && live.doneTotal > live.done.size) {
                item(key = "done-more") {
                    TextButton(onClick = { nav.runDetail(runId) }, modifier = Modifier.padding(horizontal = Space.s8)) {
                        Text(pluralStringResource(R.plurals.pair_dryrun_see_all, live.doneTotal, Fmt.count(live.doneTotal)))
                    }
                }
            }
            if (live.waiting.isNotEmpty()) {
                item(key = "wait-h") { SectionHeader(stringResource(R.string.common_waiting_for_you)) }
                items(live.waiting, key = { "w${it.id}" }) { r ->
                    val id = r.conflictId
                    ChangeRow(
                        r.action, r.name, r.path, kind = r.kind, size = r.size, reason = r.reason,
                        onClick = if (id != null) ({ nav.conflict(id) }) else null,
                    )
                }
            }
            if (live.failed.isNotEmpty()) {
                item(key = "fail-h") { SectionHeader(stringResource(R.string.pair_dryrun_couldnt_sync)) }
                items(live.failed, key = { "f${it.id}" }) { r -> DryRunFailedRow(r) }
            }
        }
    }
}

/** The top of the live log: progress while running, then the result. */
@Composable
private fun DryRunLiveHead(live: DryRunViewModel.Live, pair: FolderPairEntity?, paused: Boolean, online: Boolean, onReview: () -> Unit, onPreview: () -> Unit) {
    val m = live.progress
    val run = live.run
    val variant = MaterialTheme.colorScheme.onSurfaceVariant
    when {
        m != null && m.scanning -> {
            StatusChip(SyncStatus.Syncing, label = stringResource(R.string.pair_dryrun_comparing))
            DsLinearProgress(null)
        }
        m != null -> {
            StatusChip(SyncStatus.Syncing, progress = m.value ?: 0f)
            SyncProgress(m)
        }
        live.finished && run != null -> DryRunResult(run, pair, live.blocked, onReview, onPreview)
        paused -> {
            StatusChip(SyncStatus.Waiting, label = stringResource(R.string.common_paused), icon = Sym.Pause)
            Text(stringResource(R.string.pair_dryrun_paused_body), style = MaterialTheme.typography.bodyMedium, color = variant)
        }
        !online -> {
            StatusChip(SyncStatus.Waiting, label = stringResource(R.string.pair_dryrun_waiting_connection), icon = Sym.WifiOff)
            Text(stringResource(R.string.pair_dryrun_starts_when_online), style = MaterialTheme.typography.bodyMedium, color = variant)
        }
        else -> {
            StatusChip(SyncStatus.Syncing, label = stringResource(R.string.pair_dryrun_comparing))
            DsLinearProgress(null)
        }
    }
}

/** How the run ended: its status, what it did, and the one next step when it stopped to ask. */
@Composable
private fun DryRunResult(run: SyncRunEntity, pair: FolderPairEntity?, blocked: PlanSummary?, onReview: () -> Unit, onPreview: () -> Unit) {
    val cs = MaterialTheme.colorScheme
    val shape = MaterialTheme.shapes.large
    val name = pair?.name ?: stringResource(R.string.pair_this_pair)
    val badge = runBadge(run.status)
    val title = when (run.status) {
        RunStatus.SUCCESS -> stringResource(R.string.pair_dryrun_result_in_sync, name)
        RunStatus.PARTIAL -> stringResource(R.string.pair_dryrun_result_partial)
        RunStatus.FAILED -> StatusText.failedTitle(name, ErrorKind.parse(pair?.lastErrorKind))
        RunStatus.BLOCKED -> StatusText.approvalTitle(blocked)
        RunStatus.CANCELLED -> stringResource(if (pair?.paused == true) R.string.common_paused else R.string.common_stopped)
        RunStatus.SKIPPED -> stringResource(R.string.pair_dryrun_result_skipped)
        RunStatus.RUNNING -> stringResource(R.string.common_syncing)
    }
    val body = when (run.status) {
        RunStatus.SUCCESS -> run.finishedAt?.let { stringResource(R.string.pair_dryrun_result_finished_at, atTime(it)) }
        RunStatus.PARTIAL -> pluralStringResource(R.plurals.pair_dryrun_result_partial_body, run.errors, Fmt.count(run.errors))
        RunStatus.BLOCKED -> StatusText.approvalBody(name, blocked, run.message)
        RunStatus.CANCELLED -> stringResource(R.string.pair_dryrun_result_cancelled_body)
        else -> run.message
    }
    val counts = runCounts(run)
    Column(
        Modifier.fillMaxWidth().clip(shape).background(Ds.lab.card).padding(Space.s16),
        verticalArrangement = Arrangement.spacedBy(Space.s8),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            StatusChip(badge.status, label = badge.label, icon = badge.icon)
            Spacer(Modifier.weight(1f))
            val finishedAt = run.finishedAt
            if (finishedAt != null) {
                Text(stringResource(R.string.pair_dryrun_took, durationText(finishedAt - run.startedAt)), style = MaterialTheme.typography.bodySmall.tabular, color = cs.onSurfaceVariant)
            }
        }
        Text(title, style = MaterialTheme.typography.titleMedium, color = cs.onSurface)
        if (!body.isNullOrBlank()) Text(body, style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant)
        if (counts.isNotEmpty()) Text(counts, style = MaterialTheme.typography.bodyMedium.tabular, color = cs.onSurface)
        if (run.status == RunStatus.BLOCKED) {
            FilledTonalButton(onClick = if (blocked?.blockedKind == "first-merge") onPreview else onReview) { Text(StatusText.approvalAction(blocked)) }
        }
    }
}

/** A file this run couldn't sync: the reason and the Failed state (icon and word). */
@Composable
private fun DryRunFailedRow(row: DryRunViewModel.DoneRow) {
    val cs = MaterialTheme.colorScheme
    val failed = Ds.status.failed.content
    Row(
        Modifier.fillMaxWidth().heightIn(min = 72.dp).padding(horizontal = Space.s16, vertical = Space.s8),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.s12),
    ) {
        Box(Modifier.size(40.dp).clip(MaterialTheme.shapes.small).background(cs.surfaceContainerHighest), contentAlignment = Alignment.Center) {
            Icon(row.kind.icon, contentDescription = null, tint = cs.onSurfaceVariant, modifier = Modifier.size(20.dp))
        }
        Column(Modifier.weight(1f)) {
            Text(row.name, style = MaterialTheme.typography.bodyLarge, color = cs.onSurface, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(row.reason, style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant, maxLines = 2, overflow = TextOverflow.Ellipsis)
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s4)) {
            Icon(Sym.ErrorFill, contentDescription = null, tint = failed, modifier = Modifier.size(16.dp))
            Text(SyncStatus.Failed.label, style = MaterialTheme.typography.labelMedium, color = failed)
        }
    }
}
