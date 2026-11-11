package com.almodhayan.drivesync.ui.activity

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.actions.ConflictDetails
import com.almodhayan.drivesync.actions.ConflictResolver
import com.almodhayan.drivesync.actions.ConflictUndo
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.common.AppScreen
import com.almodhayan.drivesync.ui.common.EmptyState
import com.almodhayan.drivesync.ui.common.EmptyStateAction
import com.almodhayan.drivesync.ui.common.LoadingState
import com.almodhayan.drivesync.ui.components.BottomActionBar
import com.almodhayan.drivesync.ui.components.ConflictCard
import com.almodhayan.drivesync.ui.components.ConflictChoice
import com.almodhayan.drivesync.ui.components.ConflictSideInfo
import com.almodhayan.drivesync.ui.components.SettingControl
import com.almodhayan.drivesync.ui.components.SettingRow
import com.almodhayan.drivesync.ui.components.showMessage
import com.almodhayan.drivesync.ui.components.showUndo
import com.almodhayan.drivesync.ui.theme.Opacity
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import javax.inject.Inject
import com.almodhayan.drivesync.ui.theme.Ds

/**
 * Conflict compare (Screen06Problems, "Conflict: compare, then choose"): both copies side by side, then
 * three choices with Keep both selected by default. Resolving shows a snackbar with Undo.
 */
@HiltViewModel
class ConflictViewModel @Inject constructor(
    handle: SavedStateHandle,
    private val resolver: ConflictResolver,
    private val db: Db,
) : ViewModel() {

    val conflictId: String = handle.get<String>("conflictId").orEmpty()

    sealed interface Ui {
        data object Loading : Ui
        data class Gone(val title: String, val body: String) : Ui
        data class Ready(val details: ConflictDetails) : Ui
    }

    /** A choice was made; the snackbar's Undo can put it back. */
    data class Resolved(val undo: ConflictUndo, val message: String, val body: String)

    private val _ui = MutableStateFlow<Ui>(Ui.Loading)
    val ui: StateFlow<Ui> = _ui

    private val _choice = MutableStateFlow(ConflictChoice.KeepBoth)
    val choice: StateFlow<ConflictChoice> = _choice

    private val _busy = MutableStateFlow(false)
    val busy: StateFlow<Boolean> = _busy

    private val _resolved = MutableStateFlow<Resolved?>(null)
    val resolved: StateFlow<Resolved?> = _resolved

    private val _messages = Channel<String>(Channel.BUFFERED)
    val messages: Flow<String> = _messages.receiveAsFlow()

    /** "1 of 2" among the pair's open conflicts; null when it is the only one. */
    val position: StateFlow<Pair<Int, Int>?> = _ui.flatMapLatest { u ->
        val pairId = (u as? Ui.Ready)?.details?.conflict?.pairId
        if (pairId == null) {
            flowOf<Pair<Int, Int>?>(null)
        } else {
            db.conflicts().observeOpenForPair(pairId).map { list ->
                val i = list.indexOfFirst { it.id == conflictId }
                if (i < 0 || list.size < 2) null else (i + 1) to list.size
            }
        }
    }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    init { load() }

    fun load() {
        viewModelScope.launch {
            val d = try {
                withContext(Dispatchers.IO) { resolver.details(conflictId) }
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _ui.value = Ui.Gone(Strings.get(R.string.activity_conflict_couldnt_open), hintOf(t))
                return@launch
            }
            _ui.value = when {
                d == null -> Ui.Gone(Strings.get(R.string.activity_conflict_resolved_title), Strings.get(R.string.activity_conflict_resolved_body))
                d.pair == null -> Ui.Gone(Strings.get(R.string.activity_conflict_pair_removed_title), Strings.get(R.string.activity_conflict_pair_removed_body))
                d.conflict.resolved -> Ui.Gone(Strings.get(R.string.activity_conflict_resolved_title), Strings.get(R.string.activity_conflict_resolved_named_body, d.name))
                else -> Ui.Ready(d)
            }
        }
    }

    fun choose(c: ConflictChoice) {
        if (!_busy.value && _resolved.value == null) _choice.value = c
    }

    fun resolve() {
        val d = (_ui.value as? Ui.Ready)?.details ?: return
        if (_busy.value || _resolved.value != null) return
        val c = _choice.value
        _busy.value = true
        viewModelScope.launch {
            try {
                val undo = withContext(Dispatchers.IO) { resolver.resolve(conflictId, c) }
                _resolved.value = Resolved(undo, resolvedMessage(undo.choice, d.name), resolvedBody(undo.choice))
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _messages.send(hintOf(t))
            } finally {
                _busy.value = false
            }
        }
    }

    /** Undo from the snackbar: puts both copies back as they were and reopens the conflict. */
    fun undo() {
        val r = _resolved.value ?: return
        viewModelScope.launch {
            try {
                withContext(Dispatchers.IO) { resolver.undo(r.undo) }
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _messages.send(hintOf(t))
            }
            _resolved.value = null
            load()
        }
    }

    private fun resolvedMessage(c: ConflictChoice, name: String): String = when (c) {
        ConflictChoice.KeepBoth -> Strings.get(R.string.activity_conflict_kept_both, name)
        ConflictChoice.KeepPhone -> Strings.get(R.string.activity_conflict_kept_phone, name)
        ConflictChoice.KeepDrive -> Strings.get(R.string.activity_conflict_kept_drive, name)
    }

    private fun resolvedBody(c: ConflictChoice): String = when (c) {
        ConflictChoice.KeepBoth -> Strings.get(R.string.activity_conflict_kept_both_body)
        ConflictChoice.KeepPhone -> Strings.get(R.string.activity_conflict_kept_phone_body)
        ConflictChoice.KeepDrive -> Strings.get(R.string.activity_conflict_kept_drive_body)
    }

    private fun hintOf(t: Throwable): String {
        val e = AppError.wrap(t)
        return e.message?.takeIf { e is AppError.NotFound && it.isNotBlank() } ?: e.userHint
    }
}

@Composable
fun ConflictScreen(nav: AppNav, conflictId: String) {
    val vm: ConflictViewModel = hiltViewModel()
    val ui by vm.ui.collectAsStateWithLifecycle()
    val choice by vm.choice.collectAsStateWithLifecycle()
    val busy by vm.busy.collectAsStateWithLifecycle()
    val resolved by vm.resolved.collectAsStateWithLifecycle()
    val position by vm.position.collectAsStateWithLifecycle()
    val snackbar = remember { SnackbarHostState() }

    LaunchedEffect(vm) { vm.messages.collect { snackbar.showMessage(it) } }

    // After a choice: Undo for 10 s, then back to where the person came from.
    val done = resolved
    LaunchedEffect(done) {
        if (done != null) {
            var undone = false
            snackbar.showUndo(done.message, onUndo = {
                undone = true
                vm.undo()
            })
            if (!undone) nav.back()
        }
    }

    val details = (ui as? ConflictViewModel.Ui.Ready)?.details
    val pair = details?.pair
    val remoteId = details?.conflict?.remoteFileId
    val openHistory: (() -> Unit)? =
        if (details != null && pair != null && remoteId != null) {
            { nav.revisions(pair.accountId, remoteId, details.name, pair.id, details.conflict.key) }
        } else {
            null
        }
    val pos = position
    val subtitle = pair?.name?.let { n -> if (pos != null) stringResource(R.string.activity_conflict_subtitle_position, n, pos.first, pos.second) else n }

    AppScreen(
        title = stringResource(R.string.common_conflict),
        subtitle = subtitle,
        onBack = { nav.back() },
        actions = {
            if (openHistory != null && done == null) {
                IconButton(onClick = openHistory) { Icon(Sym.History, contentDescription = stringResource(R.string.common_version_history)) }
            }
        },
        bottomBar = {
            if (details != null && done == null) {
                BottomActionBar(hint = stringResource(R.string.activity_conflict_applies_to_file)) {
                    Button(onClick = { vm.resolve() }, enabled = !busy) { Text(conflictButtonLabel(choice)) }
                }
            }
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        when (val u = ui) {
            ConflictViewModel.Ui.Loading -> LoadingState()
            is ConflictViewModel.Ui.Gone -> EmptyState(Sym.TaskAlt, u.title, u.body, Modifier.padding(padding)) {
                EmptyStateAction(stringResource(R.string.common_back), onClick = { nav.back() })
            }
            is ConflictViewModel.Ui.Ready ->
                if (done != null) {
                    EmptyState(Sym.TaskAlt, done.message, done.body, Modifier.padding(padding))
                } else {
                    ConflictCompare(u.details, choice, busy, onChoose = { vm.choose(it) }, onHistory = openHistory, modifier = Modifier.padding(padding))
                }
        }
    }
}

@Composable
private fun ConflictCompare(
    d: ConflictDetails,
    choice: ConflictChoice,
    busy: Boolean,
    onChoose: (ConflictChoice) -> Unit,
    onHistory: (() -> Unit)?,
    modifier: Modifier,
) {
    val pair = d.pair
    val copyName = d.conflict.localCopyRel?.substringAfterLast('/')
    // Keeping one side moves the phone's conflict copy; both choices need it to still be there.
    val phoneCopyThere = d.phone != null && copyName != null
    val path = if (pair != null) conflictPaths(pair.localPath, pair.remotePath, d.folder) else d.folder
    Column(modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = Space.s24)) {
        Column(
            Modifier.padding(start = Space.s16, end = Space.s16, top = Space.s8, bottom = Space.s8),
            verticalArrangement = Arrangement.spacedBy(Space.s12),
        ) {
            ConflictCard(
                name = d.name,
                path = path,
                phone = d.phone ?: ConflictSideInfo(stringResource(R.string.activity_conflict_not_found), stringResource(R.string.activity_conflict_no_longer_on_phone)),
                drive = d.drive ?: ConflictSideInfo(stringResource(R.string.activity_conflict_not_found), stringResource(R.string.activity_conflict_not_on_phone_yet)),
                chosen = choice,
            )
            Column(Modifier.selectableGroup(), verticalArrangement = Arrangement.spacedBy(Space.s12)) {
                ConflictOption(
                    title = stringResource(R.string.common_keep_both),
                    text = if (copyName != null) stringResource(R.string.activity_conflict_keep_both_copy_named, copyName) else stringResource(R.string.activity_conflict_kept_both_body),
                    selected = choice == ConflictChoice.KeepBoth,
                    enabled = !busy,
                    recommended = true,
                ) { onChoose(ConflictChoice.KeepBoth) }
                ConflictOption(
                    title = stringResource(R.string.activity_conflict_keep_phone_version),
                    text = stringResource(if (phoneCopyThere) R.string.activity_conflict_drive_copy_to_bin else R.string.activity_conflict_phone_copy_gone),
                    selected = choice == ConflictChoice.KeepPhone,
                    enabled = phoneCopyThere && !busy,
                ) { onChoose(ConflictChoice.KeepPhone) }
                ConflictOption(
                    title = stringResource(R.string.activity_conflict_keep_drive_version),
                    text = stringResource(if (phoneCopyThere) R.string.activity_conflict_phone_copy_to_bin else R.string.activity_conflict_phone_copy_gone),
                    selected = choice == ConflictChoice.KeepDrive,
                    enabled = phoneCopyThere && !busy,
                ) { onChoose(ConflictChoice.KeepDrive) }
            }
        }
        if (onHistory != null) {
            SettingRow(
                stringResource(R.string.common_version_history),
                icon = Sym.History,
                summary = stringResource(R.string.activity_conflict_version_history_summary),
                control = SettingControl.Chevron,
                onClick = onHistory,
            )
        }
    }
}

/** One choice as a radio card; "Safest" marks Keep both. */
@Composable
private fun ConflictOption(title: String, text: String, selected: Boolean, enabled: Boolean, recommended: Boolean = false, onSelect: () -> Unit) {
    val cs = MaterialTheme.colorScheme
    val shape = MaterialTheme.shapes.large
    Row(
        Modifier.fillMaxWidth()
            .clip(shape)
            .background(if (selected) cs.primaryContainer else Ds.lab.card)
            .then(if (selected) Modifier.border(2.dp, cs.primary, shape) else Modifier)
            .selectable(selected = selected, enabled = enabled, role = Role.RadioButton, onClick = onSelect)
            .padding(Space.s16)
            .alpha(if (enabled) 1f else Opacity.disabledContent),
        horizontalArrangement = Arrangement.spacedBy(Space.s12),
    ) {
        Icon(
            if (selected) Sym.RadioButtonChecked else Sym.RadioButtonUnchecked,
            contentDescription = null,
            tint = if (selected) cs.primary else cs.onSurfaceVariant,
        )
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(Space.s2)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s8)) {
                Text(title, style = MaterialTheme.typography.titleSmall, color = if (selected) cs.onPrimaryContainer else cs.onSurface)
                if (recommended) {
                    Surface(shape = RoundedCornerShape(50), color = cs.primary) {
                        Text(
                            stringResource(R.string.activity_conflict_safest),
                            style = MaterialTheme.typography.labelSmall,
                            color = cs.onPrimary,
                            modifier = Modifier.padding(horizontal = Space.s8, vertical = Space.s2),
                        )
                    }
                }
            }
            Text(text, style = MaterialTheme.typography.bodyMedium, color = if (selected) cs.onPrimaryContainer else cs.onSurfaceVariant)
        }
    }
}

@Composable
private fun conflictButtonLabel(c: ConflictChoice): String = when (c) {
    ConflictChoice.KeepBoth -> stringResource(R.string.common_keep_both)
    ConflictChoice.KeepPhone -> stringResource(R.string.activity_conflict_keep_phone)
    ConflictChoice.KeepDrive -> stringResource(R.string.activity_conflict_keep_drive)
}

/** "Documents/Work ↔ My Drive/Work": where the file lives on each side. */
private fun conflictPaths(localPath: String, remotePath: String, folder: String): String {
    val sub = if (folder.isNotEmpty()) "/$folder" else ""
    val phone = (localPath.removePrefix("/storage/emulated/0").trim('/') + sub).trim('/').ifEmpty { Strings.get(R.string.activity_internal_storage) }
    val drive = (remotePath.trim('/') + sub).trim('/').ifEmpty { Strings.get(R.string.drive_my_drive) }
    return "$phone ↔ $drive"
}
