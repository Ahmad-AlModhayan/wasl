package com.almodhayan.drivesync.ui.activity

import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.RunStatus
import com.almodhayan.drivesync.data.db.SyncRunEntity
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.common.AppScreen
import com.almodhayan.drivesync.ui.common.EmptyState
import com.almodhayan.drivesync.ui.common.EmptyStateAction
import com.almodhayan.drivesync.ui.common.LoadingState
import com.almodhayan.drivesync.ui.components.ChipModel
import com.almodhayan.drivesync.ui.components.ChipSize
import com.almodhayan.drivesync.ui.components.DsDialog
import com.almodhayan.drivesync.ui.components.DsListItem
import com.almodhayan.drivesync.ui.components.FilterChipRow
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.MenuAction
import com.almodhayan.drivesync.ui.components.OverflowMenu
import com.almodhayan.drivesync.ui.components.StatusChip
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import javax.inject.Inject

/** Every sync, newest first, filterable by pair and by how it ended. */
@HiltViewModel
class HistoryViewModel @Inject constructor(private val db: Db) : ViewModel() {

    data class Entry(val run: SyncRunEntity, val pairName: String)
    data class PairOption(val id: String, val name: String)
    data class Ui(
        val rows: List<Entry>,
        val total: Int,
        val pairs: List<PairOption>,
        val statusCounts: List<Pair<RunStatus, Int>>,
        val pairFilter: String?,
        val statusFilter: RunStatus?,
    )

    val pairFilter = MutableStateFlow<String?>(null)
    val statusFilter = MutableStateFlow<RunStatus?>(null)

    val ui: StateFlow<Ui?> = combine(db.runs().observeRecent(LIMIT), db.pairs().observeAll(), pairFilter, statusFilter) { runs, pairs, pf, sf ->
        val removedName = Strings.get(R.string.activity_removed_pair)
        val names = pairs.associate { it.id to it.name }
        val options = LinkedHashMap<String, String>()
        pairs.forEach { options[it.id] = it.name }
        runs.forEach { if (!options.containsKey(it.pairId)) options[it.pairId] = removedName }
        val inPair = runs.filter { pf == null || it.pairId == pf }
        val rows = inPair.filter { sf == null || it.status == sf }.map { Entry(it, names[it.pairId] ?: removedName) }
        val counts = inPair.groupingBy { it.status }.eachCount().toList().sortedBy { it.first.ordinal }
        Ui(rows, runs.size, options.map { PairOption(it.key, it.value) }, counts, pf, sf)
    }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    /** Deletes the run records and their logs. Which files are in sync is kept. */
    fun clearHistory() {
        viewModelScope.launch {
            withContext(Dispatchers.IO) {
                val now = System.currentTimeMillis()
                db.runs().deleteOlderThan(now)
                db.events().deleteOlderThan(now)
            }
        }
    }

    companion object {
        private const val LIMIT = 200
    }
}

@Composable
fun HistoryScreen(nav: AppNav) {
    val vm: HistoryViewModel = hiltViewModel()
    val ui by vm.ui.collectAsStateWithLifecycle()
    var confirmClear by remember { mutableStateOf(false) }
    val clearLabel = stringResource(R.string.activity_clear_history)
    val allPairsLabel = stringResource(R.string.activity_history_all_pairs)
    val anyResultLabel = stringResource(R.string.activity_history_any_result)

    val u = ui
    AppScreen(
        title = stringResource(R.string.activity_history_title),
        onBack = { nav.back() },
        actions = {
            if ((u?.total ?: 0) > 0) {
                OverflowMenu(listOf(MenuAction(clearLabel, Sym.DeleteForever, destructive = true) { confirmClear = true }))
            }
        },
    ) { padding ->
        when {
            u == null -> LoadingState()
            u.total == 0 -> EmptyState(Sym.History, stringResource(R.string.activity_history_empty_title), stringResource(R.string.activity_history_empty_body), Modifier.padding(padding))
            else -> LazyColumn(Modifier.padding(padding).fillMaxSize(), contentPadding = PaddingValues(bottom = Space.s24)) {
                if (u.pairs.size > 1) {
                    item(key = "f-pairs") {
                        FilterChipRow(
                            listOf(ChipModel(allPairsLabel, u.pairFilter == null) { vm.pairFilter.value = null }) +
                                u.pairs.map { p -> ChipModel(p.name, u.pairFilter == p.id) { vm.pairFilter.value = if (u.pairFilter == p.id) null else p.id } },
                        )
                    }
                }
                if (u.statusCounts.size > 1 || u.statusFilter != null) {
                    item(key = "f-status") {
                        FilterChipRow(
                            listOf(ChipModel(anyResultLabel, u.statusFilter == null, u.statusCounts.sumOf { it.second }) { vm.statusFilter.value = null }) +
                                u.statusCounts.map { (s, n) ->
                                    ChipModel(runBadge(s).label, u.statusFilter == s, n) { vm.statusFilter.value = if (u.statusFilter == s) null else s }
                                },
                        )
                    }
                }
                if (u.rows.isEmpty()) {
                    item(key = "none") {
                        Text(
                            stringResource(R.string.activity_history_no_match),
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.padding(Space.s16),
                        )
                        // A way out of an empty filter result.
                        EmptyStateAction(stringResource(R.string.activity_history_clear_filters), onClick = {
                            vm.pairFilter.value = null
                            vm.statusFilter.value = null
                        }, modifier = Modifier.padding(horizontal = Space.s16))
                    }
                }
                items(u.rows, key = { it.run.id }) { row -> HistoryRow(row) { nav.runDetail(row.run.id) } }
            }
        }
    }

    if (confirmClear) {
        DsDialog(
            title = stringResource(R.string.activity_clear_history_title),
            body = stringResource(R.string.activity_clear_history_body),
            confirmLabel = clearLabel,
            onConfirm = {
                confirmClear = false
                vm.clearHistory()
            },
            onDismiss = { confirmClear = false },
            icon = Sym.DeleteForever,
            danger = true,
        )
    }
}

/** One sync: the pair, when, what it did, and how it ended. */
@Composable
private fun HistoryRow(row: HistoryViewModel.Entry, onClick: () -> Unit) {
    val r = row.run
    val badge = runBadge(r.status)
    DsListItem(
        headline = row.pairName,
        supporting = listOf(Fmt.dayAndTime(r.startedAt), runCounts(r)).filter { it.isNotEmpty() }.joinToString(" · "),
        trailing = { StatusChip(badge.status, label = badge.label, icon = badge.icon, size = ChipSize.Sm) },
        onClick = onClick,
    )
}
