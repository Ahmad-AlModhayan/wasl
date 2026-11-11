package com.almodhayan.drivesync.ui.pair

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.actions.DeletionReview
import com.almodhayan.drivesync.actions.DeletionReviewModel
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.sync.PlanSummary
import com.almodhayan.drivesync.sync.StatusText
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.Routes
import com.almodhayan.drivesync.ui.activity.ButtonLabel
import com.almodhayan.drivesync.ui.activity.atTime
import com.almodhayan.drivesync.ui.common.AppScreen
import com.almodhayan.drivesync.ui.common.EmptyState
import com.almodhayan.drivesync.ui.common.EmptyStateAction
import com.almodhayan.drivesync.ui.common.LoadingState
import com.almodhayan.drivesync.ui.components.BottomActionBar
import com.almodhayan.drivesync.ui.components.DsLinearProgress
import com.almodhayan.drivesync.ui.components.DsListItem
import com.almodhayan.drivesync.ui.components.FileKind
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.SectionHeader
import com.almodhayan.drivesync.ui.components.Side
import com.almodhayan.drivesync.ui.components.StatusBanner
import com.almodhayan.drivesync.ui.components.showMessage
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import com.almodhayan.drivesync.ui.theme.SyncStatus
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import javax.inject.Inject

/**
 * "Review deletions" (Screen05DryRun, Needs approval): a sync stopped before deleting more than the limit.
 * Shows the count, the share of the folder and every file, with two recoverable answers.
 */
@HiltViewModel
class ReviewDeletionsViewModel @Inject constructor(
    handle: SavedStateHandle,
    private val db: Db,
    private val review: DeletionReview,
    private val json: Json,
) : ViewModel() {

    val pairId: String = handle.get<String>("pairId").orEmpty()

    sealed interface Ui {
        data object Loading : Ui
        data class Failed(val hint: String) : Ui
        /** The stop was for a first sync; that review is the preview. */
        data object FirstSync : Ui
        /** The plan no longer stops. [stillMarked] when the pair still shows Needs approval. */
        data class NoLongerBlocked(val stillMarked: Boolean) : Ui
        data class Ready(val model: DeletionReviewModel) : Ui
    }

    enum class Busy { Restoring, Approving }

    sealed interface Event {
        data class Restored(val count: Int, val toPhone: Boolean) : Event
        data object Approved : Event
        data class Failed(val hint: String) : Event
    }

    private val _ui = MutableStateFlow<Ui>(Ui.Loading)
    val ui: StateFlow<Ui> = _ui

    private val _busy = MutableStateFlow<Busy?>(null)
    val busy: StateFlow<Busy?> = _busy

    /** How many files came back, once Restore finished. */
    private val _restored = MutableStateFlow<Int?>(null)
    val restored: StateFlow<Int?> = _restored

    private val _events = Channel<Event>(Channel.BUFFERED)
    val events: Flow<Event> = _events.receiveAsFlow()

    val pairName: StateFlow<String?> = db.pairs().observe(pairId).map { it?.name }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    /** When the sync stopped to ask (the list header's "Deleted at 10:40"). */
    val stoppedAt: StateFlow<Long?> = db.blockedPlans().observe(pairId).map { it?.createdAt }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    init { load() }

    fun load() {
        viewModelScope.launch {
            _ui.value = Ui.Loading
            try {
                val stored = withContext(Dispatchers.IO) { db.blockedPlans().get(pairId) }?.let { decode(it.summaryJson) }
                if (stored?.blockedKind == "first-merge") {
                    _ui.value = Ui.FirstSync
                    return@launch
                }
                val m = withContext(Dispatchers.IO) { review.load(pairId) }
                val kind = m.summary.blockedKind
                _ui.value = when {
                    kind == "first-merge" -> Ui.FirstSync
                    (kind == "deletions" || kind == "wipe") && m.items.isNotEmpty() -> Ui.Ready(m)
                    else -> Ui.NoLongerBlocked(m.pair.pendingApproval)
                }
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _ui.value = Ui.Failed(hintOf(t))
            }
        }
    }

    /** Copies every file back to the side that lost it. */
    fun restore() {
        val m = (_ui.value as? Ui.Ready)?.model ?: return
        if (_busy.value != null || _restored.value != null) return
        _busy.value = Busy.Restoring
        viewModelScope.launch {
            try {
                val n = withContext(Dispatchers.IO) { review.restore(pairId) }
                _restored.value = n
                _events.send(Event.Restored(n, toPhone = m.summary.blockedSide != "phone"))
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _events.send(Event.Failed(hintOf(t)))
            } finally {
                _busy.value = null
            }
        }
    }

    /** Lets the deletions through once (to the recycle bin / Drive's bin for 30 days) and syncs now. */
    fun approve() {
        if (_busy.value != null || _restored.value != null) return
        _busy.value = Busy.Approving
        viewModelScope.launch {
            try {
                withContext(Dispatchers.IO) { review.approve(pairId) }
                _events.send(Event.Approved)
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _events.send(Event.Failed(hintOf(t)))
            } finally {
                _busy.value = null
            }
        }
    }

    private fun decode(s: String): PlanSummary? = runCatching { json.decodeFromString(PlanSummary.serializer(), s) }.getOrNull()

    private fun hintOf(t: Throwable): String {
        val e = AppError.wrap(t)
        return e.message?.takeIf { e is AppError.NotFound && it.isNotBlank() } ?: e.userHint
    }
}

@Composable
fun ReviewDeletionsScreen(nav: AppNav, pairId: String) {
    val vm: ReviewDeletionsViewModel = hiltViewModel()
    val ui by vm.ui.collectAsStateWithLifecycle()
    val busy by vm.busy.collectAsStateWithLifecycle()
    val restored by vm.restored.collectAsStateWithLifecycle()
    val stoppedAt by vm.stoppedAt.collectAsStateWithLifecycle()
    val pairName by vm.pairName.collectAsStateWithLifecycle()
    val snackbar = remember { SnackbarHostState() }

    LaunchedEffect(vm) {
        vm.events.collect { e ->
            when (e) {
                is ReviewDeletionsViewModel.Event.Restored -> {
                    snackbar.showMessage(Strings.get(if (e.toPhone) R.string.pair_review_restored_to_phone else R.string.pair_review_restored_to_drive, Fmt.files(e.count)))
                    nav.back()
                }
                ReviewDeletionsViewModel.Event.Approved -> reviewBackToPair(nav, pairId)
                is ReviewDeletionsViewModel.Event.Failed -> snackbar.showMessage(e.hint)
            }
        }
    }

    val u = ui
    if (u is ReviewDeletionsViewModel.Ui.FirstSync) {
        // A first sync's review is its preview.
        LaunchedEffect(Unit) {
            nav.back()
            nav.dryRun(pairId)
        }
    }
    val ready = u as? ReviewDeletionsViewModel.Ui.Ready

    AppScreen(
        title = stringResource(R.string.pair_review_title),
        subtitle = ready?.model?.pair?.name ?: pairName,
        onBack = { nav.back() },
        bottomBar = {
            if (ready != null) {
                ReviewDeletionsBar(
                    model = ready.model, busy = busy, restored = restored,
                    onRestore = { vm.restore() }, onDelete = { vm.approve() }, onDone = { nav.back() },
                )
            }
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        when (u) {
            ReviewDeletionsViewModel.Ui.Loading -> Box(Modifier.padding(padding)) { LoadingState(stringResource(R.string.pair_dryrun_comparing)) }
            ReviewDeletionsViewModel.Ui.FirstSync -> Box(Modifier.padding(padding)) { LoadingState(stringResource(R.string.pair_review_opening_first_sync)) }
            is ReviewDeletionsViewModel.Ui.Failed -> EmptyState(Sym.SyncProblem, stringResource(R.string.pair_review_check_failed), u.hint, Modifier.padding(padding)) {
                EmptyStateAction(stringResource(R.string.common_try_again), onClick = { vm.load() })
            }
            is ReviewDeletionsViewModel.Ui.NoLongerBlocked -> EmptyState(
                Sym.TaskAlt,
                stringResource(R.string.pair_review_nothing_title),
                stringResource(R.string.pair_review_nothing_body),
                Modifier.padding(padding),
            ) {
                Row(horizontalArrangement = Arrangement.spacedBy(Space.s8)) {
                    TextButton(onClick = { nav.back() }) { Text(stringResource(R.string.common_back)) }
                    if (u.stillMarked) Button(onClick = { vm.approve() }) { ButtonLabel(stringResource(R.string.common_sync_now), Sym.Sync) }
                }
            }
            is ReviewDeletionsViewModel.Ui.Ready -> ReviewDeletionsList(u.model, stoppedAt, Modifier.padding(padding))
        }
    }
}

@Composable
private fun ReviewDeletionsList(m: DeletionReviewModel, stoppedAt: Long?, modifier: Modifier) {
    val s = m.summary
    // blockedSide is where files would be deleted: "drive" means they're gone from the phone.
    val fromDrive = s.blockedSide == "phone"
    val n = s.blockedCount.takeIf { it > 0 } ?: (if (fromDrive) m.onPhone else m.onDrive)
    val title = when {
        s.blockedKind == "wipe" -> stringResource(if (fromDrive) R.string.pair_review_drive_empty else R.string.pair_review_phone_empty)
        fromDrive -> stringResource(R.string.pair_review_deleted_on_drive, Fmt.files(n))
        else -> stringResource(R.string.pair_review_deleted_on_phone, Fmt.files(n))
    }
    val why = when {
        s.blockedKind == "wipe" ->
            stringResource(if (fromDrive) R.string.pair_review_wipe_phone else R.string.pair_review_wipe_drive)
        s.blockedPercent > m.limitPercent ->
            stringResource(R.string.pair_review_over_percent, s.blockedPercent, m.limitPercent)
        else -> pluralStringResource(R.plurals.pair_review_over_files, m.limitFiles, Fmt.count(m.limitFiles))
    }
    val safe = stringResource(if (fromDrive) R.string.pair_review_phone_has_all else R.string.pair_review_drive_has_all)
    val header = listOfNotNull(stoppedAt?.let { stringResource(R.string.pair_review_deleted_at, atTime(it)) }, Fmt.files(m.onDrive + m.onPhone), Fmt.bytes(m.bytes)).joinToString(" · ")
    val mixed = m.items.map { it.deleteOn }.distinct().size > 1
    val root = Fmt.humanLocalPath(m.pair.localPath).substringAfterLast(" / ")
    val restoreLabel = StatusText.restoreAction(s) ?: stringResource(R.string.pair_review_restore_on_phone)
    val deleteLabel = StatusText.confirmDeleteAction(s) ?: stringResource(R.string.pair_review_delete_on_drive)
    val hint = stringResource(if (fromDrive) R.string.pair_review_hint_from_drive else R.string.pair_review_hint_from_phone, deleteLabel, restoreLabel)
    val wholeFolder = stringResource(R.string.pair_review_whole_folder)
    val goneFromPhone = stringResource(R.string.pair_review_gone_from_phone)
    val goneFromDrive = stringResource(R.string.pair_review_gone_from_drive)

    LazyColumn(modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = Space.s16)) {
        item(key = "banner") {
            StatusBanner(
                SyncStatus.Approval, title,
                Modifier.padding(start = Space.s16, end = Space.s16, top = Space.s8),
                body = stringResource(R.string.pair_review_body, why, safe),
            )
        }
        item(key = "header") { SectionHeader(header) }
        items(m.items, key = { "${it.deleteOn.name}:${it.key}" }) { d ->
            val where = if (d.folder.isEmpty()) root else "$root/${d.folder}"
            DsListItem(
                headline = d.name,
                supporting = if (d.isDir) "$wholeFolder · ${Fmt.files(d.files)} · ${Fmt.bytes(d.bytes)}" else "$where · ${Fmt.bytes(d.bytes)}",
                icon = if (d.isDir) Sym.FolderFill else FileKind.of(d.name).icon,
                thumb = !d.isDir,
                meta = if (mixed) (if (d.deleteOn == Side.Drive) goneFromPhone else goneFromDrive) else null,
            )
        }
        item(key = "hint") {
            Text(
                hint,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = Space.s16, vertical = Space.s8),
            )
        }
    }
}

/** Two equal answers, both recoverable; progress while one runs; Done once the files are back. */
@Composable
private fun ReviewDeletionsBar(
    model: DeletionReviewModel,
    busy: ReviewDeletionsViewModel.Busy?,
    restored: Int?,
    onRestore: () -> Unit,
    onDelete: () -> Unit,
    onDone: () -> Unit,
) {
    val s = model.summary
    when {
        restored != null -> BottomActionBar(hint = stringResource(R.string.pair_review_restored, Fmt.files(restored))) {
            Button(onClick = onDone, modifier = Modifier.testTag("review_done")) { Text(stringResource(R.string.common_done)) }
        }
        busy != null -> BottomActionBar {
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(Space.s8)) {
                Text(
                    if (busy == ReviewDeletionsViewModel.Busy.Restoring) stringResource(R.string.pair_review_restoring, Fmt.files(model.onDrive + model.onPhone))
                    else stringResource(R.string.pair_review_starting_sync),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurface,
                )
                DsLinearProgress(null)
            }
        }
        else -> BottomActionBar {
            OutlinedButton(onClick = onRestore, modifier = Modifier.weight(1f).testTag("review_restore"), contentPadding = PaddingValues(horizontal = Space.s12)) {
                ButtonLabel(StatusText.restoreAction(s) ?: stringResource(R.string.pair_review_restore_on_phone), Sym.History)
            }
            // Error-coloured: of the two answers, this is the one that deletes.
            Button(
                onClick = onDelete,
                modifier = Modifier.weight(1f),
                contentPadding = PaddingValues(horizontal = Space.s12),
                colors = ButtonDefaults.buttonColors(containerColor = MaterialTheme.colorScheme.error, contentColor = MaterialTheme.colorScheme.onError),
            ) {
                ButtonLabel(StatusText.confirmDeleteAction(s) ?: stringResource(R.string.pair_review_delete_on_drive), Sym.Delete)
            }
        }
    }
}

/** Leaves this screen for the pair, without stacking a second pair screen on top of an existing one. */
private fun reviewBackToPair(nav: AppNav, pairId: String) {
    val prev = nav.controller.previousBackStackEntry
    val prevIsPair = prev != null && prev.destination.route == Routes.PAIR_DETAIL && prev.arguments?.getString("pairId") == pairId
    nav.back()
    if (!prevIsPair) nav.pairDetail(pairId)
}
