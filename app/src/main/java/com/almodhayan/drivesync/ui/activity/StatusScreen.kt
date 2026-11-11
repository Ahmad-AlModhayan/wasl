package com.almodhayan.drivesync.ui.activity

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedCard
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import androidx.work.WorkInfo
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.actions.PairActions
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.scheduler.SyncScheduler
import com.almodhayan.drivesync.sync.SyncStateHolder
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.common.AppScreen
import com.almodhayan.drivesync.ui.common.EmptyState
import com.almodhayan.drivesync.ui.common.EmptyStateAction
import com.almodhayan.drivesync.ui.common.LoadingState
import com.almodhayan.drivesync.ui.components.DsListItem
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.SectionHeader
import com.almodhayan.drivesync.ui.components.StatusChip
import com.almodhayan.drivesync.ui.components.SyncProgress
import com.almodhayan.drivesync.ui.components.SyncProgressModel
import com.almodhayan.drivesync.ui.components.showUndo
import com.almodhayan.drivesync.ui.status.PairStatusResolver
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import com.almodhayan.drivesync.ui.theme.SyncStatus
import com.almodhayan.drivesync.ui.theme.tabular
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import javax.inject.Inject
import com.almodhayan.drivesync.ui.theme.Ds

/** What is syncing right now and what is queued, with Pause per pair and Stop all. */
@HiltViewModel
class StatusViewModel @Inject constructor(
    db: Db,
    stateHolder: SyncStateHolder,
    private val scheduler: SyncScheduler,
    private val pairActions: PairActions,
) : ViewModel() {

    data class Running(val pairId: String, val name: String, val startedAt: Long, val model: SyncProgressModel)
    data class Queued(val id: String, val title: String, val detail: String, val periodic: Boolean)
    data class Ui(val running: List<Running>, val queued: List<Queued>)

    val ui: StateFlow<Ui?> = combine(stateHolder.running, db.pairs().observeAll(), scheduler.observeWorkInfos()) { running, pairs, infos ->
        val byId = pairs.associateBy { it.id }
        Ui(
            running = running.values.sortedBy { it.startedAt }.mapNotNull { p ->
                byId[p.pairId]?.let { pair -> Running(p.pairId, pair.name, p.startedAt, PairStatusResolver.model(p, pair)) }
            },
            queued = infos.filter { it.state == WorkInfo.State.ENQUEUED || it.state == WorkInfo.State.BLOCKED }.map { queuedOf(it) },
        )
    }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    fun pause(pairId: String) {
        viewModelScope.launch {
            try {
                withContext(Dispatchers.IO) { pairActions.pause(pairId) }
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                // The run carries on; the card still offers Pause.
            }
        }
    }

    private var stopped: Ui? = null

    /** Stops every running and queued sync. Files synced so far stay synced; the schedule comes back on its own. */
    fun stopAll() {
        stopped = ui.value
        scheduler.cancelAll()
    }

    /** Undo for Stop all: the pairs that were running start again, and a waiting one-off sync becomes a sync of every pair. */
    fun undoStopAll() {
        val s = stopped ?: return
        stopped = null
        s.running.forEach { scheduler.syncNow(it.pairId) }
        if (s.queued.any { !it.periodic }) scheduler.syncNow()
    }

    private fun queuedOf(w: WorkInfo): Queued {
        val periodic = w.periodicityInfo
        val title = if (periodic != null) {
            Strings.get(R.string.activity_status_scheduled_sync, Fmt.interval((periodic.repeatIntervalMillis / 60_000).toInt()))
        } else {
            Strings.get(R.string.activity_status_sync_waiting)
        }
        val state = when {
            w.state == WorkInfo.State.BLOCKED -> Strings.get(R.string.activity_status_waiting_other_sync)
            periodic != null -> Strings.get(R.string.activity_status_waiting_for_time)
            else -> Strings.get(R.string.activity_status_waiting_to_start)
        }
        val next = w.nextScheduleTimeMillis.takeIf { it != Long.MAX_VALUE && it > 0 }?.let { Strings.get(R.string.activity_status_next, atTime(it)) }
        return Queued(w.id.toString(), title, listOfNotNull(state, next).joinToString(" · "), periodic != null)
    }
}

@Composable
fun StatusScreen(nav: AppNav) {
    val vm: StatusViewModel = hiltViewModel()
    val ui by vm.ui.collectAsStateWithLifecycle()
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(Unit) {
        while (true) {
            delay(1_000)
            now = System.currentTimeMillis()
        }
    }

    val u = ui
    val busy = u != null && (u.running.isNotEmpty() || u.queued.isNotEmpty())
    AppScreen(
        title = stringResource(R.string.activity_sync_status),
        onBack = { nav.back() },
        actions = {
            if (busy) {
                TextButton(onClick = {
                    vm.stopAll()
                    scope.launch { snackbar.showUndo(Strings.get(R.string.activity_status_stopped_all), onUndo = vm::undoStopAll) }
                }) { Text(stringResource(R.string.activity_status_stop_all)) }
            }
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        when {
            u == null -> LoadingState()
            !busy -> EmptyState(
                Sym.CloudDone,
                stringResource(R.string.activity_status_empty_title),
                stringResource(R.string.activity_status_empty_body),
                Modifier.padding(padding),
            ) {
                EmptyStateAction(stringResource(R.string.activity_status_see_history), onClick = { nav.history() })
            }
            else -> LazyColumn(Modifier.padding(padding).fillMaxSize(), contentPadding = PaddingValues(top = Space.s8, bottom = Space.s24)) {
                items(u.running, key = { it.pairId }) { r ->
                    StatusRunningCard(r, now, onOpen = { nav.pairDetail(r.pairId) }, onPause = { vm.pause(r.pairId) })
                }
                if (u.queued.isNotEmpty()) {
                    item(key = "h-queued") { SectionHeader(stringResource(R.string.activity_status_queued)) }
                    items(u.queued, key = { it.id }) { q -> DsListItem(headline = q.title, supporting = q.detail, icon = Sym.Schedule) }
                    item(key = "queued-hint") {
                        Text(
                            stringResource(R.string.activity_status_queued_hint),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.padding(horizontal = Space.s16, vertical = Space.s8),
                        )
                    }
                }
            }
        }
    }
}

/** One running pair: name and time so far, a chip while comparing, progress with Pause. Tap opens the pair. */
@Composable
private fun StatusRunningCard(r: StatusViewModel.Running, now: Long, onOpen: () -> Unit, onPause: () -> Unit) {
    val cs = MaterialTheme.colorScheme
    Card(
        onClick = onOpen,
        modifier = Modifier.padding(horizontal = Space.s16, vertical = Space.s4).fillMaxWidth(),
        shape = MaterialTheme.shapes.large,
        colors = CardDefaults.cardColors(containerColor = Ds.lab.card),
    ) {
        Column(Modifier.padding(Space.s16), verticalArrangement = Arrangement.spacedBy(Space.s12)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s8)) {
                Text(r.name, style = MaterialTheme.typography.titleMedium, color = cs.onSurface, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                Text(durationText(now - r.startedAt), style = MaterialTheme.typography.bodySmall.tabular, color = cs.onSurfaceVariant)
            }
            // While files move, the progress line below already says "Syncing 3 of 10"; the chip only names the scan.
            if (r.model.scanning) {
                StatusChip(SyncStatus.Syncing, label = stringResource(R.string.status_comparing_folders))
            }
            SyncProgress(r.model, onPause = onPause)
        }
    }
}
