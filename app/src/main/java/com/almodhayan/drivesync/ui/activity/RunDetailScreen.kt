package com.almodhayan.drivesync.ui.activity

import android.content.ClipData
import android.content.ClipboardManager
import androidx.annotation.StringRes
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.core.TimeFmt
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.EventLevel
import com.almodhayan.drivesync.data.db.SyncEventEntity
import com.almodhayan.drivesync.data.db.SyncRunEntity
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.common.AppScreen
import com.almodhayan.drivesync.ui.common.EmptyState
import com.almodhayan.drivesync.ui.common.EmptyStateAction
import com.almodhayan.drivesync.ui.common.KeyValueRow
import com.almodhayan.drivesync.ui.common.LoadingState
import com.almodhayan.drivesync.ui.common.SectionCard
import com.almodhayan.drivesync.ui.components.ChangeAction
import com.almodhayan.drivesync.ui.components.ChangeRow
import com.almodhayan.drivesync.ui.components.ChipModel
import com.almodhayan.drivesync.ui.components.FilterChipRow
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.StatusChip
import com.almodhayan.drivesync.ui.components.showMessage
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import com.almodhayan.drivesync.ui.theme.tabular
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import javax.inject.Inject

/** One sync: how it ended, what it changed (as change rows), what went wrong, and the full log. */
@HiltViewModel
class RunDetailViewModel @Inject constructor(handle: SavedStateHandle, private val db: Db) : ViewModel() {

    val runId: String = handle.get<String>("runId").orEmpty()

    enum class View(@StringRes val label: Int) {
        Changes(R.string.activity_run_view_changes),
        Problems(R.string.activity_run_view_problems),
        Log(R.string.activity_run_view_log),
    }

    @Immutable
    data class Change(val id: Long, val action: ChangeAction, val name: String, val path: String, val reason: String)

    data class Ui(
        val run: SyncRunEntity?,
        val pairName: String?,
        val pairExists: Boolean,
        val events: List<SyncEventEntity>,
        val changes: List<Change>,
        val problems: List<SyncEventEntity>,
        val view: View,
    )

    val view = MutableStateFlow(View.Changes)

    // The run's own row changes (it finishes) without new events, so any change to the runs table re-reads it.
    private val data = combine(db.events().observeForRun(runId), db.runs().observeRecent(1)) { events, _ ->
        val run = db.runs().get(runId)
        val pair = run?.let { db.pairs().get(it.pairId) }
        Triple(run, pair, events)
    }

    val ui: StateFlow<Ui?> = combine(data, view) { (run, pair, events), v ->
        val root = pair?.let { Fmt.humanLocalPath(it.localPath).substringAfterLast(" / ") }
        val changes = events.mapNotNull { e ->
            val c = changeOf(e) ?: return@mapNotNull null
            val key = e.path ?: return@mapNotNull null
            val parent = key.substringBeforeLast('/', "")
            val path = when {
                root == null -> parent
                parent.isEmpty() -> root
                else -> "$root/$parent"
            }
            Change(e.id, c.action, key.substringAfterLast('/'), path, c.reason)
        }
        Ui(run, pair?.name, pair != null, events, changes, events.filter { it.level != EventLevel.INFO }, v)
    }.flowOn(Dispatchers.IO).stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)
}

@Composable
fun RunDetailScreen(nav: AppNav, runId: String) {
    val vm: RunDetailViewModel = hiltViewModel()
    val ui by vm.ui.collectAsStateWithLifecycle()
    val context = LocalContext.current
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    var expanded by remember { mutableStateOf(setOf<Long>()) }

    val u = ui
    val record = u?.run
    val copyLogLabel = stringResource(R.string.activity_run_copy_log)
    val viewLabels = RunDetailViewModel.View.entries.associateWith { stringResource(it.label) }
    AppScreen(
        title = u?.pairName ?: stringResource(R.string.activity_run_title_fallback),
        subtitle = record?.let { "${Fmt.dayAndTime(it.startedAt)} · ${triggerText(it.trigger)}" },
        onBack = { nav.back() },
        actions = {
            if (u != null && u.events.isNotEmpty()) {
                IconButton(onClick = {
                    val header = record?.let { r -> "${u.pairName ?: r.pairId} · ${TimeFmt.human(r.startedAt)} · ${runBadge(r.status).label} · ${runCounts(r)}" }
                    val lines = if (u.view == RunDetailViewModel.View.Problems) u.problems else u.events
                    context.getSystemService(ClipboardManager::class.java)?.setPrimaryClip(ClipData.newPlainText(Strings.get(R.string.activity_run_clip_label), eventsToText(lines, header)))
                    scope.launch { snackbar.showMessage(Strings.plural(R.plurals.activity_run_copied_lines, lines.size, Fmt.count(lines.size))) }
                }) { Icon(Sym.ContentCopy, contentDescription = copyLogLabel) }
            }
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        when {
            u == null -> LoadingState()
            record == null -> EmptyState(
                Sym.History,
                stringResource(R.string.activity_run_gone_title),
                stringResource(R.string.activity_run_gone_body),
                Modifier.padding(padding),
            ) {
                EmptyStateAction(stringResource(R.string.common_back), onClick = { nav.back() })
            }
            else -> LazyColumn(Modifier.padding(padding).fillMaxSize(), contentPadding = PaddingValues(top = Space.s8, bottom = Space.s24)) {
                item(key = "summary") { RunSummaryCard(record, u.pairExists) { nav.pairDetail(record.pairId) } }
                item(key = "chips") {
                    FilterChipRow(
                        RunDetailViewModel.View.entries.map { v ->
                            val count = when (v) {
                                RunDetailViewModel.View.Changes -> u.changes.size
                                RunDetailViewModel.View.Problems -> u.problems.size
                                RunDetailViewModel.View.Log -> u.events.size
                            }
                            ChipModel(viewLabels.getValue(v), u.view == v, count) { vm.view.value = v }
                        },
                        Modifier.padding(top = Space.s12),
                    )
                }
                when (u.view) {
                    RunDetailViewModel.View.Changes -> {
                        if (u.changes.isEmpty()) item(key = "none") { RunDetailNote(stringResource(R.string.activity_run_no_changes)) }
                        items(u.changes, key = { "c${it.id}" }) { c -> ChangeRow(c.action, c.name, c.path, reason = c.reason) }
                    }
                    RunDetailViewModel.View.Problems -> {
                        if (u.problems.isEmpty()) item(key = "none") { RunDetailNote(stringResource(R.string.activity_run_no_problems)) }
                        items(u.problems, key = { "p${it.id}" }) { e ->
                            EventRow(e, expanded = e.id in expanded, onToggle = { expanded = if (e.id in expanded) expanded - e.id else expanded + e.id })
                        }
                    }
                    RunDetailViewModel.View.Log -> {
                        if (u.events.isEmpty()) item(key = "none") { RunDetailNote(stringResource(R.string.activity_run_nothing_logged)) }
                        items(u.events, key = { "l${it.id}" }) { e ->
                            EventRow(e, expanded = e.id in expanded, onToggle = { expanded = if (e.id in expanded) expanded - e.id else expanded + e.id })
                        }
                    }
                }
            }
        }
    }
}

/** How the sync ended, how long it took, and what it moved. */
@Composable
private fun RunSummaryCard(r: SyncRunEntity, pairExists: Boolean, onOpenPair: () -> Unit) {
    val badge = runBadge(r.status)
    val cs = MaterialTheme.colorScheme
    SectionCard(modifier = Modifier.padding(horizontal = Space.s16)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            StatusChip(badge.status, label = badge.label, icon = badge.icon)
            Spacer(Modifier.weight(1f))
            val finishedAt = r.finishedAt
            Text(
                if (finishedAt != null) stringResource(R.string.activity_run_took, durationText(finishedAt - r.startedAt)) else stringResource(R.string.activity_run_still_going),
                style = MaterialTheme.typography.bodySmall.tabular,
                color = cs.onSurfaceVariant,
            )
        }
        KeyValueRow(stringResource(R.string.common_to_drive), countAndBytes(r.uploaded, r.bytesUp))
        KeyValueRow(stringResource(R.string.common_to_phone), countAndBytes(r.downloaded, r.bytesDown))
        KeyValueRow(stringResource(R.string.activity_run_to_bin), Fmt.count(r.deletedLocal + r.deletedRemote))
        if (r.conflicts > 0) KeyValueRow(stringResource(R.string.activity_run_conflicts), Fmt.count(r.conflicts))
        if (r.skipped > 0) KeyValueRow(stringResource(R.string.common_skipped), Fmt.count(r.skipped))
        if (r.errors > 0) KeyValueRow(stringResource(R.string.activity_action_couldnt_sync), Fmt.count(r.errors))
        val message = r.message
        if (!message.isNullOrBlank()) Text(message, style = MaterialTheme.typography.bodyMedium, color = cs.onSurface)
        if (pairExists) {
            TextButton(onClick = onOpenPair) { ButtonLabel(stringResource(R.string.activity_run_open_pair), Sym.FolderOpen) }
        }
    }
}

@Composable
private fun RunDetailNote(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(Space.s16),
    )
}

private fun countAndBytes(n: Int, bytes: Long): String = if (bytes > 0) "${Fmt.count(n)} · ${Fmt.bytes(bytes)}" else Fmt.count(n)
