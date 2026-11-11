package com.almodhayan.drivesync.ui.revisions

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.actions.FileVersions
import com.almodhayan.drivesync.actions.VersionEntry
import com.almodhayan.drivesync.actions.VersionHistory
import com.almodhayan.drivesync.actions.VersionRef
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.TrashReason
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import com.almodhayan.drivesync.drive.DriveApi
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.activity.openUrl
import com.almodhayan.drivesync.ui.common.AppScreen
import com.almodhayan.drivesync.ui.common.EmptyState
import com.almodhayan.drivesync.ui.common.EmptyStateAction
import com.almodhayan.drivesync.ui.common.LoadingState
import com.almodhayan.drivesync.ui.components.DsLinearProgress
import com.almodhayan.drivesync.ui.components.FileKind
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.MenuAction
import com.almodhayan.drivesync.ui.components.OverflowMenu
import com.almodhayan.drivesync.ui.components.VersionItem
import com.almodhayan.drivesync.ui.components.showMessage
import com.almodhayan.drivesync.ui.components.showUndo
import com.almodhayan.drivesync.ui.theme.MonoSmall
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
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import javax.inject.Inject
import com.almodhayan.drivesync.ui.theme.Ds

/**
 * File version history (Screen08VersionHistory): a header card for the file, then one timeline, newest
 * first, merging Drive's revisions with the versions this phone replaced. Restore never deletes: the
 * replaced version goes to the recycle bin, and the snackbar's Undo brings it back.
 */
@HiltViewModel
class RevisionsViewModel @Inject constructor(
    handle: SavedStateHandle,
    private val history: VersionHistory,
    private val api: DriveApi,
    private val db: Db,
    settings: SettingsRepository,
) : ViewModel() {

    val accountId: String = handle.get<String>("accountId").orEmpty()
    val fileId: String = handle.get<String>("fileId").orEmpty()
    val name: String = handle.get<String>("name")?.takeIf { it.isNotBlank() } ?: Strings.get(R.string.revisions_file)
    val pairId: String? = handle.get<String>("pairId")?.takeIf { it.isNotBlank() }
    val key: String? = handle.get<String>("key")?.takeIf { it.isNotBlank() }

    sealed interface Ui {
        data object Loading : Ui
        data class Failed(val hint: String) : Ui
        data class Ready(val versions: FileVersions) : Ui
    }

    sealed interface Event {
        /** [undoSince] is set when a replaced version went to the bin, so Undo can bring it back. */
        data class Restored(val message: String, val undoSince: Long?) : Event
        data class Kept(val message: String, val entry: VersionEntry, val keep: Boolean) : Event
        data class Message(val text: String) : Event
    }

    private val _ui = MutableStateFlow<Ui>(Ui.Loading)
    val ui: StateFlow<Ui> = _ui

    /** What is happening right now ("Restoring the version from Today 09:15…"); null when idle. */
    private val _busy = MutableStateFlow<String?>(null)
    val busy: StateFlow<String?> = _busy

    private val _events = Channel<Event>(Channel.BUFFERED)
    val events: Flow<Event> = _events.receiveAsFlow()

    /** How many replaced versions this phone keeps per file (Settings › Safety). */
    val localKept: StateFlow<Int> = settings.settings.map { it.localVersionsKept }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), 5)

    init { load() }

    fun load(quiet: Boolean = false) {
        viewModelScope.launch {
            if (!quiet) _ui.value = Ui.Loading
            try {
                val v = withContext(Dispatchers.IO) { history.load(accountId, fileId, name, pairId, key) }
                _ui.value = Ui.Ready(v)
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                val hint = hintOf(t)
                if (quiet && _ui.value is Ui.Ready) {
                    _events.send(Event.Message(hint))
                } else {
                    _ui.value = Ui.Failed(hint)
                }
            }
        }
    }

    private fun work(label: String, block: suspend () -> Unit) {
        if (_busy.value != null) return
        _busy.value = label
        viewModelScope.launch {
            try {
                block()
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _events.send(Event.Message(hintOf(t)))
            } finally {
                _busy.value = null
            }
        }
    }

    /** Makes [e] current: on the phone now, on Drive at the next sync. */
    fun restore(e: VersionEntry) {
        val pid = pairId ?: return
        val k = key ?: return
        val time = Fmt.dayAndTime(e.at)
        work(Strings.get(R.string.revisions_restoring, time)) {
            val since = System.currentTimeMillis()
            withContext(Dispatchers.IO) { history.restore(accountId, fileId, pid, k, e.ref) }
            val replaced = withContext(Dispatchers.IO) { db.trash().forKey(pid, k) }
                .any { it.reason == TrashReason.RESTORED_VERSION && it.createdAt >= since }
            _events.send(
                Event.Restored(
                    if (replaced) Strings.get(R.string.revisions_restored_replaced_in_bin, time) else Strings.get(R.string.revisions_restored, time),
                    undoSince = if (replaced) since else null,
                ),
            )
            load(quiet = true)
        }
    }

    /** Undo for Restore: the version it replaced (the newest one in the bin since then) becomes current again. */
    fun undoRestore(since: Long) {
        val pid = pairId ?: return
        val k = key ?: return
        work(Strings.get(R.string.revisions_putting_back)) {
            val replaced = withContext(Dispatchers.IO) { db.trash().forKey(pid, k) }
                .firstOrNull { it.reason == TrashReason.RESTORED_VERSION && it.createdAt >= since }
            if (replaced != null) {
                withContext(Dispatchers.IO) { history.restore(accountId, fileId, pid, k, VersionRef.Local(replaced.id)) }
                load(quiet = true)
            }
        }
    }

    /** Saves [e] beside the file (or to Downloads for files outside a pair). */
    fun saveCopy(e: VersionEntry) {
        work(Strings.get(R.string.revisions_saving_copy)) {
            val f = withContext(Dispatchers.IO) { history.saveCopy(accountId, fileId, name, pairId, key, e.ref, e.at) }
            _events.send(Event.Message(if (pairId != null) Strings.get(R.string.revisions_saved_beside, f.name) else Strings.get(R.string.revisions_saved_to_downloads, f.name)))
        }
    }

    /** Drive's "Keep forever" for one of its revisions. */
    fun setKeepForever(e: VersionEntry, keep: Boolean, announce: Boolean = true) {
        val revisionId = (e.ref as? VersionRef.Drive)?.revisionId ?: return
        val time = Fmt.dayAndTime(e.at)
        work(if (keep) Strings.get(R.string.revisions_keeping, time) else Strings.get(R.string.revisions_updating, time)) {
            withContext(Dispatchers.IO) { api.keepRevision(accountId, fileId, revisionId, keep) }
            if (announce) {
                _events.send(
                    Event.Kept(
                        if (keep) Strings.get(R.string.revisions_kept_forever_message, time) else Strings.get(R.string.revisions_not_kept_message, time),
                        e, keep,
                    ),
                )
            }
            load(quiet = true)
        }
    }

    private fun hintOf(t: Throwable): String {
        val e = AppError.wrap(t)
        return e.message?.takeIf { e is AppError.NotFound && it.isNotBlank() } ?: e.userHint
    }
}

@Composable
fun RevisionsScreen(nav: AppNav, accountId: String, fileId: String, name: String, pairId: String?, key: String?) {
    val vm: RevisionsViewModel = hiltViewModel()
    val ui by vm.ui.collectAsStateWithLifecycle()
    val busy by vm.busy.collectAsStateWithLifecycle()
    val kept by vm.localKept.collectAsStateWithLifecycle()
    val context = LocalContext.current
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    val noDriveApp = stringResource(R.string.revisions_no_app_for_drive)

    LaunchedEffect(vm) {
        vm.events.collect { e ->
            when (e) {
                is RevisionsViewModel.Event.Restored -> {
                    val since = e.undoSince
                    scope.launch {
                        if (since != null) snackbar.showUndo(e.message, onUndo = { vm.undoRestore(since) }) else snackbar.showMessage(e.message)
                    }
                }
                is RevisionsViewModel.Event.Kept -> scope.launch {
                    snackbar.showUndo(e.message, onUndo = { vm.setKeepForever(e.entry, !e.keep, announce = false) })
                }
                is RevisionsViewModel.Event.Message -> scope.launch { snackbar.showMessage(e.text) }
            }
        }
    }

    AppScreen(
        title = stringResource(R.string.revisions_title),
        onBack = { nav.back() },
        actions = {
            IconButton(onClick = {
                val url = "https://drive.google.com/file/d/${vm.fileId}/view"
                if (!openUrl(context, url)) {
                    scope.launch { snackbar.showMessage(noDriveApp) }
                }
            }) { Icon(Sym.OpenInNew, contentDescription = stringResource(R.string.revisions_open_in_drive)) }
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        when (val u = ui) {
            RevisionsViewModel.Ui.Loading -> LoadingState(stringResource(R.string.revisions_loading))
            is RevisionsViewModel.Ui.Failed -> EmptyState(Sym.History, stringResource(R.string.revisions_load_failed), u.hint, Modifier.padding(padding)) {
                EmptyStateAction(stringResource(R.string.common_try_again), onClick = { vm.load() })
            }
            is RevisionsViewModel.Ui.Ready -> {
                val v = u.versions
                LazyColumn(Modifier.padding(padding).fillMaxSize(), contentPadding = PaddingValues(top = Space.s8, bottom = Space.s24)) {
                    item(key = "head") { RevisionsHeader(v, kept, inPair = vm.pairId != null) }
                    val working = busy
                    if (working != null) {
                        item(key = "busy") {
                            Column(Modifier.padding(horizontal = Space.s16, vertical = Space.s8), verticalArrangement = Arrangement.spacedBy(Space.s8)) {
                                Text(working, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                                DsLinearProgress(null)
                            }
                        }
                    }
                    if (v.entries.isEmpty()) {
                        item(key = "none") {
                            Text(
                                stringResource(R.string.revisions_none),
                                style = MaterialTheme.typography.bodyMedium,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                modifier = Modifier.padding(horizontal = Space.s16, vertical = Space.s8),
                            )
                        }
                    }
                    itemsIndexed(v.entries, key = { _, e -> versionKey(e.ref) }) { i, e ->
                        RevisionsRow(
                            e = e,
                            first = i == 0,
                            last = i == v.entries.lastIndex,
                            canRestore = v.canRestore,
                            onRestore = { vm.restore(e) },
                            onSaveCopy = { vm.saveCopy(e) },
                            onKeep = if (e.ref is VersionRef.Drive) ({ keep: Boolean -> vm.setKeepForever(e, keep) }) else null,
                        )
                    }
                }
            }
        }
    }
}

/** The file: kind tile, name, where it lives, its pair and size, and how long versions are kept. */
@Composable
private fun RevisionsHeader(v: FileVersions, kept: Int, inPair: Boolean) {
    val cs = MaterialTheme.colorScheme
    val shape = MaterialTheme.shapes.large
    Column(
        Modifier.padding(horizontal = Space.s16).fillMaxWidth().clip(shape).background(Ds.lab.card).padding(Space.s16),
        verticalArrangement = Arrangement.spacedBy(Space.s12),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s12)) {
            Box(
                Modifier.size(40.dp).clip(MaterialTheme.shapes.small).background(cs.surfaceContainerHighest),
                contentAlignment = Alignment.Center,
            ) { Icon(FileKind.of(v.name).icon, contentDescription = null, tint = cs.onSurfaceVariant) }
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(Space.s2)) {
                Text(v.name, style = MaterialTheme.typography.titleMedium, color = cs.onSurface, maxLines = 2, overflow = TextOverflow.Ellipsis)
                Text(v.path, style = MonoSmall, color = cs.onSurfaceVariant, maxLines = 2, overflow = TextOverflow.Ellipsis)
                val meta = listOfNotNull(v.pairName, v.size?.let { Fmt.bytes(it) }).joinToString(" · ")
                if (meta.isNotEmpty()) Text(meta, style = MaterialTheme.typography.bodySmall.tabular, color = cs.onSurfaceVariant)
            }
        }
        Text(
            if (inPair) pluralStringResource(R.plurals.revisions_drive_and_phone_keep, kept, kept) else stringResource(R.string.revisions_drive_keeps),
            style = MaterialTheme.typography.bodySmall,
            color = cs.onSurfaceVariant,
        )
    }
}

/** One version on the timeline; Drive versions also get a menu for Keep forever. */
@Composable
private fun RevisionsRow(
    e: VersionEntry,
    first: Boolean,
    last: Boolean,
    canRestore: Boolean,
    onRestore: () -> Unit,
    onSaveCopy: () -> Unit,
    onKeep: ((Boolean) -> Unit)?,
) {
    val time = Fmt.dayAndTime(e.at)
    val keptForever = stringResource(R.string.revisions_kept_forever)
    val note = listOfNotNull(e.note, if (e.keepForever) keptForever else null).joinToString(" · ").ifEmpty { null }
    Box(Modifier.fillMaxWidth()) {
        VersionItem(
            whenText = time,
            source = e.source,
            size = Fmt.bytes(e.size),
            modifier = if (onKeep != null) Modifier.padding(end = 40.dp) else Modifier,
            note = note,
            current = e.current,
            first = first,
            last = last,
            onRestore = if (canRestore) onRestore else null,
            onSaveCopy = onSaveCopy,
        )
        if (onKeep != null) {
            OverflowMenu(
                listOf(MenuAction(if (e.keepForever) stringResource(R.string.revisions_stop_keeping_forever) else stringResource(R.string.revisions_keep_forever), Sym.Lock) { onKeep(!e.keepForever) }),
                Modifier.align(Alignment.TopEnd),
                contentDescription = stringResource(R.string.revisions_more_for, time),
            )
        }
    }
}

private fun versionKey(ref: VersionRef): String = when (ref) {
    is VersionRef.Drive -> "d:${ref.revisionId}"
    is VersionRef.Local -> "l:${ref.trashId}"
}
