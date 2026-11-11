package com.almodhayan.drivesync.ui.trash

import androidx.activity.compose.BackHandler
import androidx.annotation.StringRes
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
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
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.actions.BinItem
import com.almodhayan.drivesync.actions.RecycleBin
import com.almodhayan.drivesync.actions.RestoreResult
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.TrashEntity
import com.almodhayan.drivesync.data.db.TrashReason
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.activity.ButtonLabel
import com.almodhayan.drivesync.ui.activity.SearchField
import com.almodhayan.drivesync.ui.common.AppScreen
import com.almodhayan.drivesync.ui.common.EmptyState
import com.almodhayan.drivesync.ui.common.LoadingState
import com.almodhayan.drivesync.ui.components.BottomActionBar
import com.almodhayan.drivesync.ui.components.ChipModel
import com.almodhayan.drivesync.ui.components.DsDialog
import com.almodhayan.drivesync.ui.components.DsListItem
import com.almodhayan.drivesync.ui.components.FilterChipRow
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.MenuAction
import com.almodhayan.drivesync.ui.components.NavIcon
import com.almodhayan.drivesync.ui.components.OverflowMenu
import com.almodhayan.drivesync.ui.components.SectionHeader
import com.almodhayan.drivesync.ui.components.Side
import com.almodhayan.drivesync.ui.components.showMessage
import com.almodhayan.drivesync.ui.components.showUndo
import com.almodhayan.drivesync.ui.theme.Ds
import com.almodhayan.drivesync.ui.theme.Size
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import com.almodhayan.drivesync.ui.theme.tabular
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.time.LocalDate
import java.time.ZoneId
import javax.inject.Inject

/**
 * Recycle bin (Screen07RecycleBin): everything sync removed or replaced, from either side, for 30 days.
 * Tap restores (with Undo); touch and hold selects several; Delete forever is the only dialog.
 */
@HiltViewModel
class TrashViewModel @Inject constructor(
    private val bin: RecycleBin,
    private val db: Db,
    settings: SettingsRepository,
) : ViewModel() {

    enum class Filter(@StringRes private val labelRes: Int) {
        All(R.string.trash_filter_all), Phone(R.string.common_phone), Drive(R.string.common_drive);

        val label: String get() = Strings.get(labelRes)
    }

    /** Where "Version history" leads for a version this phone replaced. */
    data class VersionLink(val accountId: String, val fileId: String, val pairId: String, val key: String, val name: String)

    @Immutable
    data class Entry(val item: BinItem, val today: Boolean, val versions: VersionLink?) {
        val id: String get() = item.entry.id
    }

    data class Ui(val rows: List<Entry>, val phoneBytes: Long, val driveBytes: Long, val retentionDays: Int)

    sealed interface Event {
        data class Restored(val result: RestoreResult, val message: String) : Event
        data class Message(val text: String) : Event
    }

    val ui: StateFlow<Ui?> = combine(bin.observe(), db.pairs().observeAll(), settings.settings) { items, pairs, cfg ->
        build(items, pairs.associate { it.id to it.accountId }, cfg.trashRetentionDays)
    }.flowOn(Dispatchers.IO).stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    private val _selected = MutableStateFlow<Set<String>>(emptySet())
    val selected: StateFlow<Set<String>> = _selected

    private val _busy = MutableStateFlow(false)
    val busy: StateFlow<Boolean> = _busy

    private val _events = Channel<Event>(Channel.BUFFERED)
    val events: Flow<Event> = _events.receiveAsFlow()

    fun toggle(id: String) {
        _selected.update { if (id in it) it - id else it + id }
    }

    fun selectAll(ids: Collection<String>) {
        _selected.value = ids.toSet()
    }

    fun clearSelection() {
        _selected.value = emptySet()
    }

    /** Puts files back where they were. A newer file in the way goes into the bin first, so nothing is lost. */
    fun restore(ids: List<String>) {
        if (ids.isEmpty() || _busy.value) return
        val byId = ui.value?.rows.orEmpty().associate { it.id to it.item }
        _busy.value = true
        viewModelScope.launch {
            try {
                val result = withContext(Dispatchers.IO) { bin.restore(ids) }
                _selected.value = emptySet()
                _events.send(restoredEvent(result, byId))
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _events.send(Event.Message(AppError.wrap(t).userHint))
            } finally {
                _busy.value = false
            }
        }
    }

    /** Undo from the snackbar: the restored files go back into the bin. */
    fun undoRestore(restored: RestoreResult) {
        viewModelScope.launch {
            try {
                withContext(Dispatchers.IO) { bin.undoRestore(restored) }
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _events.send(Event.Message(AppError.wrap(t).userHint))
            }
        }
    }

    /** Irreversible: removes the files from the phone's bin and from Drive's bin. */
    fun deleteForever(ids: List<String>, all: Boolean) {
        if (_busy.value) return
        _busy.value = true
        viewModelScope.launch {
            try {
                val n = withContext(Dispatchers.IO) { if (all) bin.emptyAll() else bin.deleteForever(ids) }
                _selected.value = emptySet()
                _events.send(Event.Message(Strings.plural(R.plurals.trash_deleted_forever, n, Fmt.count(n))))
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _events.send(Event.Message(AppError.wrap(t).userHint))
            } finally {
                _busy.value = false
            }
        }
    }

    private suspend fun build(items: List<BinItem>, accountByPair: Map<String, String>, retentionDays: Int): Ui {
        val zone = ZoneId.systemDefault()
        val startOfToday = LocalDate.now(zone).atStartOfDay(zone).toInstant().toEpochMilli()
        val remoteIds = HashMap<String, String?>()
        val rows = items.map { b ->
            val t = b.entry
            val key = t.syncKey
            val account = accountByPair[t.pairId]
            val replaced = t.reason == TrashReason.OVERWRITTEN || t.reason == TrashReason.RESTORED_VERSION
            val link = if (replaced && key != null && account != null) {
                val rid = remoteIds.getOrPut(t.pairId + "|" + key) { db.fileStates().get(t.pairId, key)?.remoteId }
                rid?.let { VersionLink(account, it, t.pairId, key, b.name) }
            } else {
                null
            }
            Entry(b, t.createdAt >= startOfToday, link)
        }
        var phone = 0L
        var drive = 0L
        for (b in items) if (b.side == Side.Phone) phone += b.entry.size else drive += b.entry.size
        return Ui(rows, phone, drive, retentionDays)
    }

    private fun restoredEvent(result: RestoreResult, byId: Map<String, BinItem>): Event {
        val failed = result.failed
        if (result.restored.isEmpty()) {
            val first = failed.firstOrNull()
            val head = if (failed.size == 1) {
                Strings.get(R.string.trash_restore_failed_one, first?.let { byId[it.first.id]?.name } ?: Strings.get(R.string.trash_the_file))
            } else {
                Strings.plural(R.plurals.trash_restore_failed_many, failed.size, Fmt.count(failed.size))
            }
            val why = first?.second.orEmpty()
            return Event.Message(listOf(head, why).filter { it.isNotBlank() }.joinToString(" "))
        }
        val ok = result.restored.mapNotNull { byId[it.id] }
        val all = ok.size == result.restored.size
        val base = when {
            all && ok.size == 1 -> Strings.get(R.string.trash_restored_one_to, ok[0].name, ok[0].folder)
            all && ok.map { it.folder }.distinct().size == 1 -> Strings.plural(R.plurals.trash_restored_many_to, ok.size, Fmt.count(ok.size), ok[0].folder)
            else -> Strings.plural(R.plurals.trash_restored_many, result.restored.size, Fmt.count(result.restored.size))
        }
        val tail = if (failed.isEmpty()) "" else ". " + Strings.plural(R.plurals.trash_restore_some_failed, failed.size, Fmt.count(failed.size))
        return Event.Restored(result, base + tail)
    }
}

/** Files waiting for the Delete forever confirmation. */
private data class TrashConfirm(val ids: List<String>, val all: Boolean)

@Composable
fun TrashScreen(nav: AppNav) {
    val vm: TrashViewModel = hiltViewModel()
    val ui by vm.ui.collectAsStateWithLifecycle()
    val selectedRaw by vm.selected.collectAsStateWithLifecycle()
    val busy by vm.busy.collectAsStateWithLifecycle()
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    var searching by rememberSaveable { mutableStateOf(false) }
    var query by rememberSaveable { mutableStateOf("") }
    var filter by rememberSaveable { mutableStateOf(TrashViewModel.Filter.All) }
    var confirm by remember { mutableStateOf<TrashConfirm?>(null) }

    LaunchedEffect(vm) {
        vm.events.collect { e ->
            when (e) {
                is TrashViewModel.Event.Restored -> scope.launch { snackbar.showUndo(e.message, onUndo = { vm.undoRestore(e.result) }) }
                is TrashViewModel.Event.Message -> scope.launch { snackbar.showMessage(e.text) }
            }
        }
    }

    val rows = ui?.rows.orEmpty()
    val byId = remember(rows) { rows.associateBy { it.id } }
    val selected = remember(selectedRaw, byId) { selectedRaw.filterTo(LinkedHashSet()) { it in byId } }
    val selecting = selected.isNotEmpty()
    BackHandler(enabled = selecting) { vm.clearSelection() }

    val visible = remember(rows, filter, query) {
        val q = query.trim()
        rows.filter { r ->
            val b = r.item
            val inFilter = when (filter) {
                TrashViewModel.Filter.All -> true
                TrashViewModel.Filter.Phone -> b.side == Side.Phone
                TrashViewModel.Filter.Drive -> b.side == Side.Drive
            }
            inFilter && (q.isEmpty() || b.name.contains(q, ignoreCase = true) || b.pairName.contains(q, ignoreCase = true) || b.folder.contains(q, ignoreCase = true))
        }
    }
    val today = remember(visible) { visible.filter { it.today } }
    val earlier = remember(visible) { visible.filter { !it.today } }
    val cs = MaterialTheme.colorScheme
    val searchLabel = stringResource(R.string.trash_search)
    val emptyBinLabel = stringResource(R.string.trash_empty_bin)

    AppScreen(
        title = if (selecting) pluralStringResource(R.plurals.trash_selected, selected.size, Fmt.count(selected.size)) else stringResource(R.string.trash_title),
        onBack = if (selecting) ({ vm.clearSelection() }) else null,
        navIcon = NavIcon.Close,
        actions = {
            // Delete forever lives in the bottom bar only, next to Restore.
            if (selecting) {
                IconButton(onClick = { vm.selectAll(visible.map { it.id }) }) { Icon(Sym.SelectAll, contentDescription = stringResource(R.string.trash_select_all)) }
            } else if (rows.isNotEmpty()) {
                if (!searching) {
                    IconButton(onClick = { searching = true }) { Icon(Sym.Search, contentDescription = searchLabel) }
                }
                OverflowMenu(
                    listOf(MenuAction(emptyBinLabel, Sym.DeleteForever, destructive = true) { confirm = TrashConfirm(rows.map { it.id }, all = true) }),
                )
            }
        },
        bottomBar = {
            if (selecting) {
                BottomActionBar {
                    TextButton(
                        onClick = { confirm = TrashConfirm(selected.toList(), all = false) },
                        enabled = !busy,
                        colors = ButtonDefaults.textButtonColors(contentColor = cs.error),
                    ) { ButtonLabel(stringResource(R.string.trash_delete_forever), Sym.DeleteForever) }
                    Button(onClick = { vm.restore(selected.toList()) }, enabled = !busy) {
                        ButtonLabel(stringResource(R.string.trash_restore_count, Fmt.count(selected.size)), Sym.History)
                    }
                }
            }
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        val u = ui
        when {
            u == null -> LoadingState()
            u.rows.isEmpty() -> EmptyState(
                Sym.Delete,
                stringResource(R.string.trash_empty_title),
                pluralStringResource(R.plurals.trash_empty_body, u.retentionDays, Fmt.count(u.retentionDays)),
                Modifier.padding(padding),
            )
            else -> LazyColumn(Modifier.padding(padding).fillMaxSize(), contentPadding = PaddingValues(bottom = Space.s24)) {
                if (searching && !selecting) {
                    item(key = "search") {
                        SearchField(query, { query = it }, searchLabel, onClose = { searching = false; query = "" })
                    }
                }
                item(key = "intro") {
                    Text(
                        trashIntro(u),
                        style = MaterialTheme.typography.bodyMedium,
                        color = cs.onSurfaceVariant,
                        modifier = Modifier.padding(horizontal = Space.s16, vertical = Space.s8),
                    )
                }
                item(key = "chips") {
                    FilterChipRow(
                        TrashViewModel.Filter.entries.map { f ->
                            val count = when (f) {
                                TrashViewModel.Filter.All -> rows.size
                                TrashViewModel.Filter.Phone -> rows.count { it.item.side == Side.Phone }
                                TrashViewModel.Filter.Drive -> rows.count { it.item.side == Side.Drive }
                            }
                            val icon = when (f) {
                                TrashViewModel.Filter.All -> null
                                TrashViewModel.Filter.Phone -> Sym.Mobile
                                TrashViewModel.Filter.Drive -> Sym.Cloud
                            }
                            ChipModel(f.label, f == filter, count, icon = icon) { filter = f }
                        },
                    )
                }
                if (visible.isEmpty()) {
                    item(key = "none") {
                        Text(
                            stringResource(R.string.trash_no_match),
                            style = MaterialTheme.typography.bodyMedium,
                            color = cs.onSurfaceVariant,
                            modifier = Modifier.padding(Space.s16),
                        )
                    }
                }
                if (today.isNotEmpty()) {
                    item(key = "h-today") { SectionHeader(stringResource(R.string.trash_today)) }
                    items(today, key = { it.id }) { r ->
                        TrashEntryRow(
                            r, isSelected = r.id in selected, selecting = selecting,
                            onTap = { if (selecting) vm.toggle(r.id) else vm.restore(listOf(r.id)) },
                            onHold = { vm.toggle(r.id) },
                            onVersions = r.versions?.let { v -> { nav.revisions(v.accountId, v.fileId, v.name, v.pairId, v.key) } },
                        )
                    }
                }
                if (earlier.isNotEmpty()) {
                    item(key = "h-earlier") { SectionHeader(stringResource(R.string.trash_earlier)) }
                    items(earlier, key = { it.id }) { r ->
                        TrashEntryRow(
                            r, isSelected = r.id in selected, selecting = selecting,
                            onTap = { if (selecting) vm.toggle(r.id) else vm.restore(listOf(r.id)) },
                            onHold = { vm.toggle(r.id) },
                            onVersions = r.versions?.let { v -> { nav.revisions(v.accountId, v.fileId, v.name, v.pairId, v.key) } },
                        )
                    }
                }
            }
        }
    }

    confirm?.let { c ->
        val n = c.ids.size
        val sides = c.ids.mapNotNull { byId[it]?.item?.side }.toSet()
        val where = stringResource(
            when (sides) {
                setOf(Side.Phone) -> R.string.trash_where_phone
                setOf(Side.Drive) -> R.string.trash_where_drive
                else -> R.string.trash_where_both
            },
        )
        DsDialog(
            title = pluralStringResource(R.plurals.trash_delete_title, n, Fmt.count(n)),
            body = pluralStringResource(R.plurals.trash_delete_body, n, Fmt.count(n), where),
            confirmLabel = stringResource(R.string.trash_delete_forever),
            onConfirm = {
                confirm = null
                vm.deleteForever(c.ids, c.all)
            },
            onDismiss = { confirm = null },
            icon = Sym.DeleteForever,
            danger = true,
        )
    }
}

/** One file in the bin: the side it came from, its pair and folder, and the days it has left. */
@Composable
private fun TrashEntryRow(
    row: TrashViewModel.Entry,
    isSelected: Boolean,
    selecting: Boolean,
    onTap: () -> Unit,
    onHold: () -> Unit,
    onVersions: (() -> Unit)?,
) {
    val b = row.item
    val versionHistory = stringResource(R.string.common_version_history)
    val moreFor = stringResource(R.string.trash_more_for, b.name)
    DsListItem(
        headline = b.name,
        modifier = Modifier.semantics { this.selected = isSelected },
        supporting = "${b.pairName} · ${b.folder}",
        leading = { TrashThumb(b.side, isSelected) },
        trailing = {
            Row(verticalAlignment = Alignment.CenterVertically) {
                TrashDaysLeft(b.daysLeft)
                if (onVersions != null && !selecting) {
                    OverflowMenu(listOf(MenuAction(versionHistory, Sym.History, onClick = onVersions)), contentDescription = moreFor)
                }
            }
        },
        selected = isSelected,
        onClick = onTap,
        onLongClick = onHold,
    )
}

/** The side the file came from (phone in primary, Drive in tertiary), or a check when selected. */
@Composable
private fun TrashThumb(side: Side, isSelected: Boolean) {
    val cs = MaterialTheme.colorScheme
    Box(
        Modifier.size(40.dp).clip(MaterialTheme.shapes.small).background(if (isSelected) cs.primary else cs.surfaceContainerHighest),
        contentAlignment = Alignment.Center,
    ) {
        if (isSelected) {
            Icon(Sym.Check, contentDescription = null, tint = cs.onPrimary, modifier = Modifier.size(Size.iconRow))
        } else {
            Icon(
                side.icon,
                contentDescription = stringResource(if (side == Side.Phone) R.string.trash_was_on_phone else R.string.trash_was_on_drive),
                tint = if (side == Side.Phone) cs.primary else cs.tertiary,
                modifier = Modifier.size(Size.iconRow),
            )
        }
    }
}

/** "21 days left"; amber and heavier at 3 or fewer. */
@Composable
private fun TrashDaysLeft(days: Int) {
    val soon = days <= 3
    Text(
        if (days == 0) stringResource(R.string.trash_last_day) else pluralStringResource(R.plurals.trash_days_left, days, Fmt.count(days)),
        style = MaterialTheme.typography.bodySmall.tabular.copy(fontWeight = if (soon) FontWeight(600) else FontWeight(400)),
        color = if (soon) Ds.status.approval.content else MaterialTheme.colorScheme.onSurfaceVariant,
        maxLines = 1,
    )
}

private fun trashIntro(u: TrashViewModel.Ui): String {
    val keep = if (u.retentionDays == RecycleBin.DRIVE_BIN_DAYS) {
        Strings.plural(R.plurals.trash_intro_keep, u.retentionDays, Fmt.count(u.retentionDays))
    } else {
        Strings.get(
            R.string.trash_intro_keep_split,
            Strings.plural(R.plurals.settings_days, u.retentionDays, u.retentionDays),
            Strings.plural(R.plurals.settings_days, RecycleBin.DRIVE_BIN_DAYS, RecycleBin.DRIVE_BIN_DAYS),
        )
    }
    val use = buildList {
        if (u.phoneBytes > 0) add(Strings.get(R.string.trash_use_phone, Fmt.bytes(u.phoneBytes)))
        if (u.driveBytes > 0) add(Strings.get(R.string.trash_use_drive, Fmt.bytes(u.driveBytes)))
    }
    val using = when (use.size) {
        0 -> ""
        1 -> " " + Strings.get(R.string.trash_using_one, use[0])
        else -> " " + Strings.get(R.string.trash_using_two, use[0], use[1])
    }
    return "$keep$using " + Strings.get(R.string.trash_tap_to_restore)
}
