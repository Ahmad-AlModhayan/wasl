@file:OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)

package com.almodhayan.drivesync.ui.pair

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.db.PairTotals
import com.almodhayan.drivesync.data.db.RunStatus
import com.almodhayan.drivesync.data.db.SyncRunEntity
import com.almodhayan.drivesync.sync.StatusText
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.Routes
import com.almodhayan.drivesync.ui.common.AppScreen
import com.almodhayan.drivesync.ui.common.EmptyState
import com.almodhayan.drivesync.ui.common.EmptyStateAction
import com.almodhayan.drivesync.ui.common.LoadingState
import com.almodhayan.drivesync.ui.components.ChangeRow
import com.almodhayan.drivesync.ui.components.ChipSize
import com.almodhayan.drivesync.ui.components.ChipVariant
import com.almodhayan.drivesync.ui.components.DsDialog
import com.almodhayan.drivesync.ui.components.DsListItem
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.MenuAction
import com.almodhayan.drivesync.ui.components.OverflowMenu
import com.almodhayan.drivesync.ui.components.PairDirection
import com.almodhayan.drivesync.ui.components.PathRow
import com.almodhayan.drivesync.ui.components.SectionHeader
import com.almodhayan.drivesync.ui.components.Side
import com.almodhayan.drivesync.ui.components.StatusBanner
import com.almodhayan.drivesync.ui.components.StatusIcon
import com.almodhayan.drivesync.ui.components.SyncProgress
import com.almodhayan.drivesync.ui.components.UiAction
import com.almodhayan.drivesync.ui.components.showMessage
import com.almodhayan.drivesync.ui.components.showUndo
import com.almodhayan.drivesync.ui.home.LoudTarget
import com.almodhayan.drivesync.ui.home.perform
import com.almodhayan.drivesync.ui.onboarding.DeviceChecks
import com.almodhayan.drivesync.ui.status.PairStatusResolver
import com.almodhayan.drivesync.ui.status.PairStatusUi
import com.almodhayan.drivesync.ui.theme.Size
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import com.almodhayan.drivesync.ui.theme.SyncStatus
import com.almodhayan.drivesync.ui.theme.tabular
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import com.almodhayan.drivesync.actions.StorageStats
import com.almodhayan.drivesync.ui.components.StatusChip as DsStatusChip
import com.almodhayan.drivesync.ui.devices.PairDevicesSection

/**
 * The pair screen, opened from a card: its state, live progress and log while it runs, the one fix
 * when it is loud, both folders, the everyday actions, open conflicts, recent syncs and details.
 */
@Composable
fun PairDetailScreen(nav: AppNav, pairId: String) {
    val vm: PairDetailViewModel = hiltViewModel()
    val ui by vm.ui.collectAsStateWithLifecycle()
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }
    val scrollBehavior = TopAppBarDefaults.pinnedScrollBehavior()
    var confirmReset by rememberSaveable { mutableStateOf(false) }
    var removing by remember { mutableStateOf(false) }
    LaunchedEffect(vm) { vm.messages.collect { snackbar.showMessage(it) } }

    val pair = ui.pair
    val progress = ui.progress
    val direction = pair?.let { PairDirection.of(it.mode) }
    val subtitle = when {
        direction == null -> null
        progress != null -> "${direction.title} · ${stringResource(R.string.pair_dryrun_started_at, Fmt.clock(progress.startedAt))}"
        else -> direction.title
    }

    val noAppForPage = stringResource(R.string.pair_detail_no_app_for_page)
    val noFilesApp = stringResource(R.string.pair_detail_no_files_app)
    val noAppForDrive = stringResource(R.string.pair_detail_no_app_for_drive)
    val pausedName = pair?.name?.let { stringResource(R.string.pair_detail_paused_name, it) }
    val message: (String) -> Unit = { text -> scope.launch { snackbar.showMessage(text) } }
    val runAction: (LoudTarget) -> Unit = { target ->
        if (!target.perform(nav, context, retry = { vm.syncNow() })) message(noAppForPage)
    }
    val pauseWithUndo: () -> Unit = {
        vm.pause()
        if (pausedName != null) scope.launch { snackbar.showUndo(pausedName, onUndo = { vm.resume() }) }
    }
    val removePair: () -> Unit = {
        removing = true
        scope.launch {
            val removed = vm.remove()
            if (!removed) {
                removing = false
                return@launch
            }
            // Home offers Undo; go back to the list of pairs wherever we came from.
            if (!nav.controller.popBackStack(Routes.HOME, inclusive = false)) nav.back()
        }
    }
    val menu: List<MenuAction> = if (pair == null) emptyList() else listOf(
        MenuAction(stringResource(R.string.pair_selective_sync), Sym.Checklist) { nav.selective(pairId) },
        MenuAction(stringResource(R.string.pair_detail_open_phone_folder), Sym.FolderOpen) {
            if (!vm.openPhoneFolder(context, pair.localPath)) message(noFilesApp)
        },
        MenuAction(stringResource(R.string.pair_detail_open_in_drive), Sym.OpenInNew) {
            if (!DeviceChecks.openUrl(context, pdDriveFolderUrl(pair.remoteFolderId))) message(noAppForDrive)
        },
        MenuAction(stringResource(R.string.pair_detail_reset_state), Sym.History) { confirmReset = true },
        MenuAction(stringResource(R.string.common_remove_pair), Sym.Delete, destructive = true) { removePair() },
    )

    AppScreen(
        title = pair?.name ?: "",
        onBack = { nav.back() },
        subtitle = subtitle,
        scrollBehavior = scrollBehavior,
        snackbarHost = { SnackbarHost(snackbar) },
        actions = { if (pair != null && !removing) OverflowMenu(menu, contentDescription = stringResource(R.string.pair_detail_more_for, pair.name)) },
    ) { padding ->
        when {
            ui.loading || removing -> Box(Modifier.fillMaxSize().padding(padding)) { if (!removing) LoadingState() }
            pair == null -> EmptyState(
                icon = Sym.Folder,
                title = stringResource(R.string.pair_removed_title),
                body = stringResource(R.string.pair_detail_removed_body),
                modifier = Modifier.padding(padding),
            ) { EmptyStateAction(stringResource(R.string.pair_detail_back_to_pairs), onClick = { nav.back() }) }
            else -> PdContent(
                ui = ui,
                pair = pair,
                padding = padding,
                onAction = runAction,
                onSyncNow = vm::syncNow,
                onPause = pauseWithUndo,
                onResume = vm::resume,
                onTurnOn = vm::turnOn,
                onPreview = { nav.dryRun(pairId) },
                onEdit = { nav.pairEdit(pairId) },
                onConflict = { nav.conflict(it) },
                onRun = { nav.runDetail(it) },
                onHistory = { nav.history() },
                onMessage = message,
            )
        }
    }

    if (confirmReset && pair != null) {
        DsDialog(
            title = stringResource(R.string.pair_detail_reset_title),
            body = stringResource(R.string.pair_detail_reset_body, pair.name),
            confirmLabel = stringResource(R.string.pair_detail_reset_state),
            onConfirm = {
                confirmReset = false
                vm.resetSyncState()
            },
            onDismiss = { confirmReset = false },
            icon = Sym.History,
            danger = false,
        )
    }
}

@Composable
private fun PdContent(
    ui: PairDetailUi,
    pair: FolderPairEntity,
    padding: PaddingValues,
    onAction: (LoudTarget) -> Unit,
    onSyncNow: () -> Unit,
    onPause: () -> Unit,
    onResume: () -> Unit,
    onTurnOn: () -> Unit,
    onPreview: () -> Unit,
    onEdit: () -> Unit,
    onConflict: (String) -> Unit,
    onRun: (String) -> Unit,
    onHistory: () -> Unit,
    onMessage: (String) -> Unit,
) {
    val st = ui.status ?: return
    val progress = ui.progress
    val running = progress != null
    val progressModel = remember(progress, pair.mode) { progress?.let { PairStatusResolver.model(it, pair) } }
    // During a run its conflicts show under "Waiting for you"; don't list them twice.
    val liveKeys = ui.waiting.map { it.key }.toSet()
    val conflicts = if (running) ui.conflicts.filter { it.key !in liveKeys } else ui.conflicts

    LazyColumn(Modifier.fillMaxSize().padding(padding), contentPadding = PaddingValues(bottom = Space.s24)) {
        item(key = "status") { PdStatusHeader(st) }
        if (progressModel != null) {
            item(key = "progress") {
                SyncProgress(progressModel, Modifier.padding(start = Space.s16, end = Space.s16, bottom = Space.s8), onPause = onPause)
            }
        }

        // Live log of the current run, grouped Done / Waiting for you.
        if (running && ui.done.isNotEmpty()) {
            item(key = "done-h") { SectionHeader(stringResource(R.string.pair_dryrun_section_done)) }
            items(ui.done, key = { "done-${it.id}" }) { c -> ChangeRow(c.action, c.name, c.folder, reason = c.reason) }
            if (ui.doneTotal > ui.done.size) {
                item(key = "done-more") { PdNote(pluralStringResource(R.plurals.pair_detail_earlier_changes, ui.doneTotal - ui.done.size, Fmt.count(ui.doneTotal - ui.done.size))) }
            }
        }
        if (running && ui.waiting.isNotEmpty()) {
            item(key = "waiting-h") { SectionHeader(stringResource(R.string.common_waiting_for_you)) }
            items(ui.waiting, key = { "wait-${it.id}" }) { c ->
                val conflictId = ui.conflicts.firstOrNull { it.key == c.key }?.id
                ChangeRow(c.action, c.name, c.folder, reason = c.reason, onClick = conflictId?.let { id -> { onConflict(id) } })
            }
        }

        ui.loud?.let { loud ->
            item(key = "banner") {
                StatusBanner(
                    status = loud.status,
                    title = loud.title,
                    modifier = Modifier.padding(start = Space.s16, end = Space.s16, top = Space.s8),
                    body = loud.body,
                    primary = UiAction(loud.actionLabel) { onAction(loud.target) },
                )
            }
        }
        item(key = "paths") { PdPaths(pair, ui.totals, ui.runs) }
        item(key = "actions") {
            PdActions(pair, running, onSyncNow = onSyncNow, onPause = onPause, onResume = onResume, onTurnOn = onTurnOn, onPreview = onPreview, onEdit = onEdit)
        }
        // Device sync (FR-N12): share this pair with your devices, or join another device's folder.
        item(key = "devices") { PairDevicesSection(onMessage = onMessage) }

        if (conflicts.isNotEmpty()) {
            item(key = "conflicts-h") { SectionHeader(pluralStringResource(R.plurals.pair_n_conflicts, conflicts.size, Fmt.count(conflicts.size))) }
            items(conflicts, key = { "c-${it.id}" }) { c ->
                val folder = c.key.substringBeforeLast('/', "")
                DsListItem(
                    headline = c.key.substringAfterLast('/'),
                    supporting = if (folder.isEmpty()) stringResource(R.string.pair_detail_conflict_both_kept) else stringResource(R.string.pair_detail_conflict_both_kept_in, folder),
                    leading = { Box(Modifier.size(Size.circleButton), contentAlignment = Alignment.Center) { StatusIcon(SyncStatus.Conflict, size = Size.icon) } },
                    meta = Fmt.relative(c.detectedAt, ui.now),
                    chevron = true,
                    onClick = { onConflict(c.id) },
                )
            }
        }

        item(key = "runs-h") { SectionHeader(stringResource(R.string.pair_detail_recent_syncs)) }
        if (ui.runs.isEmpty()) {
            item(key = "runs-empty") { PdNote(stringResource(R.string.pair_detail_no_syncs)) }
        } else {
            items(ui.runs, key = { "r-${it.id}" }) { r -> PdRunItem(r, onClick = { onRun(r.id) }) }
        }
        item(key = "history") {
            TextButton(onClick = onHistory, modifier = Modifier.padding(horizontal = Space.s8)) { Text(stringResource(R.string.pair_detail_all_history)) }
        }

        // More than the design shows: kept, after its sections.
        item(key = "details-h") { SectionHeader(stringResource(R.string.pair_detail_details)) }
        item(key = "d-account") { DsListItem(headline = stringResource(R.string.common_google_account), supporting = pair.accountId, icon = Sym.AccountCircle) }
        item(key = "d-when") {
            DsListItem(headline = stringResource(R.string.pair_detail_when_it_syncs), supporting = ui.conditions.replaceFirstChar { it.uppercase() }, icon = Sym.Schedule)
        }
        if (pair.encrypted) {
            item(key = "d-vault") { DsListItem(headline = stringResource(R.string.pair_detail_encrypted), supporting = stringResource(R.string.pair_detail_encrypted_body), icon = Sym.Lock) }
        }
        if (pair.driveId != null) {
            item(key = "d-shared") { DsListItem(headline = stringResource(R.string.pair_detail_shared_drive), supporting = stringResource(R.string.pair_detail_shared_drive_body), icon = Sym.Group) }
        }
        if (pair.pendingLocalChanges > 0) {
            item(key = "d-pending") {
                DsListItem(headline = stringResource(R.string.pair_detail_pending_local), supporting = Fmt.files(pair.pendingLocalChanges), icon = Sym.Mobile)
            }
        }
    }
}

/** The status as icon + word, when it last synced, and the one-line reason for quiet waits. */
@Composable
private fun PdStatusHeader(st: PairStatusUi) {
    val cs = MaterialTheme.colorScheme
    Column(Modifier.fillMaxWidth().padding(horizontal = Space.s16, vertical = Space.s8), verticalArrangement = Arrangement.spacedBy(Space.s4)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s8)) {
            DsStatusChip(st.status, label = st.label, progress = st.progress?.value, icon = st.icon)
            if (st.lastSync != null) Text(st.lastSync, style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant, maxLines = 1)
        }
        if (!st.loud && st.progress == null && st.detail != null) {
            Text(st.detail, style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant)
        }
    }
}

@Composable
private fun PdPaths(pair: FolderPairEntity, totals: PairTotals?, runs: List<SyncRunEntity>) {
    val notYet = if (pair.lastSuccessAt == null) stringResource(R.string.common_not_synced_yet) else Fmt.filesAndSize(0, 0)
    // Free space on the phone's side, as the desktop shows per pair (#103); StatFs can touch slow storage.
    val free by produceState<Long?>(null, pair.localPath) {
        value = withContext(Dispatchers.IO) {
            val path = if (pair.localPath.startsWith("content://")) Fmt.safFilePath(pair.localPath) else pair.localPath
            path?.let { StorageStats().phoneVolume(it)?.free }
        }
    }
    val phoneMeta = listOfNotNull(
        totals?.let { Fmt.filesAndSize(it.files, it.localBytes) } ?: notYet,
        free?.let { stringResource(R.string.pair_detail_free, Fmt.bytes(it)) },
    ).joinToString(" · ")
    // What the last finished sync left out (size limit, Docs exports and the like), as the desktop shows it.
    val skipped = runs.firstOrNull { it.status == RunStatus.SUCCESS || it.status == RunStatus.PARTIAL }?.skipped ?: 0
    Column(Modifier.fillMaxWidth().padding(start = Space.s16, end = Space.s16, top = Space.s8), verticalArrangement = Arrangement.spacedBy(Space.s8)) {
        PathRow(Side.Phone, Fmt.humanLocalPath(pair.localPath), meta = phoneMeta)
        PathRow(Side.Drive, Fmt.humanRemotePath(pair.remotePath), meta = totals?.let { Fmt.filesAndSize(it.files, it.remoteBytes) } ?: notYet)
        if (skipped > 0) {
            Text(
                stringResource(R.string.pair_detail_skipped_last, Fmt.count(skipped)),
                style = MaterialTheme.typography.bodySmall.tabular,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

@Composable
private fun PdActions(
    pair: FolderPairEntity,
    running: Boolean,
    onSyncNow: () -> Unit,
    onPause: () -> Unit,
    onResume: () -> Unit,
    onTurnOn: () -> Unit,
    onPreview: () -> Unit,
    onEdit: () -> Unit,
) {
    // Text-only buttons: an icon beside each word only repeated it (0.4.1).
    FlowRow(
        Modifier.fillMaxWidth().padding(start = Space.s16, end = Space.s16, top = Space.s16),
        horizontalArrangement = Arrangement.spacedBy(Space.s8),
        verticalArrangement = Arrangement.spacedBy(Space.s8),
    ) {
        Button(onClick = onSyncNow, enabled = !running, modifier = Modifier.testTag("pair_sync_now")) { Text(stringResource(R.string.common_sync_now), maxLines = 1) }
        when {
            !pair.enabled -> OutlinedButton(onClick = onTurnOn) { Text(stringResource(R.string.pair_detail_turn_on), maxLines = 1) }
            pair.paused -> OutlinedButton(onClick = onResume, modifier = Modifier.testTag("pair_resume")) { Text(stringResource(R.string.common_resume), maxLines = 1) }
            else -> OutlinedButton(onClick = onPause, modifier = Modifier.testTag("pair_pause")) { Text(stringResource(R.string.common_pause), maxLines = 1) }
        }
        OutlinedButton(onClick = onPreview) { Text(stringResource(R.string.pair_detail_preview), maxLines = 1) }
        OutlinedButton(onClick = onEdit, modifier = Modifier.testTag("pair_edit")) { Text(stringResource(R.string.pair_detail_edit), maxLines = 1) }
    }
}

@Composable
private fun PdNote(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.bodySmall.tabular,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(horizontal = Space.s16, vertical = Space.s8),
    )
}

/** A recent sync as a full-bleed list row: status icon, when, what changed, the status word. */
@Composable
private fun PdRunItem(r: SyncRunEntity, onClick: () -> Unit) {
    val s = pdRunStatus(r)
    DsListItem(
        modifier = Modifier.testTag("run_row:" + r.trigger),
        headline = Fmt.dayAndTime(r.startedAt),
        supporting = runCountsText(r),
        leading = { Box(Modifier.size(Size.circleButton), contentAlignment = Alignment.Center) { StatusIcon(s.status, size = Size.icon, icon = s.icon) } },
        meta = s.label,
        chevron = true,
        onClick = onClick,
    )
}

// ------------------------------------------------------------------ shared with History and Run detail

private class PdRunStatus(val status: SyncStatus, val label: String, val icon: ImageVector?)

/** A finished (or running) sync in the six-state words. */
private fun pdRunStatus(r: SyncRunEntity): PdRunStatus = when (r.status) {
    RunStatus.SUCCESS -> PdRunStatus(SyncStatus.Synced, SyncStatus.Synced.label, null)
    RunStatus.RUNNING -> PdRunStatus(SyncStatus.Syncing, SyncStatus.Syncing.label, null)
    RunStatus.PARTIAL -> PdRunStatus(SyncStatus.Failed, if (r.errors > 0) Strings.plural(R.plurals.pair_run_n_failed, r.errors, Fmt.count(r.errors)) else SyncStatus.Failed.label, null)
    RunStatus.FAILED -> PdRunStatus(SyncStatus.Failed, SyncStatus.Failed.label, null)
    RunStatus.BLOCKED -> PdRunStatus(SyncStatus.Approval, SyncStatus.Approval.label, null)
    RunStatus.SKIPPED -> StatusText.waiting(r.message).let { (label, icon) -> PdRunStatus(SyncStatus.Waiting, label, Sym.named(icon)) }
    RunStatus.CANCELLED -> PdRunStatus(SyncStatus.Waiting, Strings.get(R.string.common_stopped), Sym.Pause)
}

/** One history row: when, what changed, and the status as icon + word. Used by the History screen. */
@Composable
fun RunRow(r: SyncRunEntity, pairName: String? = null, onClick: () -> Unit) {
    val s = pdRunStatus(r)
    val cs = MaterialTheme.colorScheme
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick).heightIn(min = Size.touchTarget).padding(vertical = Space.s8),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.s12),
    ) {
        Column(Modifier.weight(1f)) {
            Text(
                (pairName?.let { "$it · " } ?: "") + Fmt.dayAndTime(r.startedAt),
                style = MaterialTheme.typography.bodyLarge, color = cs.onSurface, maxLines = 1, overflow = TextOverflow.Ellipsis,
            )
            Text(runCountsText(r), style = MaterialTheme.typography.bodySmall.tabular, color = cs.onSurfaceVariant, maxLines = 2, overflow = TextOverflow.Ellipsis)
        }
        DsStatusChip(s.status, label = s.label, icon = s.icon, variant = ChipVariant.Plain, size = ChipSize.Sm)
    }
}

/** What started a sync, in plain words. */
fun triggerLabel(trigger: String): String = when (trigger.lowercase()) {
    "manual" -> Strings.get(R.string.pair_trigger_manual)
    "periodic" -> Strings.get(R.string.pair_trigger_scheduled)
    "instant", "local-change", "local" -> Strings.get(R.string.pair_trigger_phone_change)
    "remote", "remote-change" -> Strings.get(R.string.pair_trigger_drive_change)
    "boot" -> Strings.get(R.string.pair_trigger_after_restart)
    "share" -> Strings.get(R.string.pair_trigger_shared_file)
    "tile" -> Strings.get(R.string.pair_trigger_quick_tile)
    "widget" -> Strings.get(R.string.pair_trigger_widget)
    "automation", "intent" -> Strings.get(R.string.common_automation)
    "approved" -> Strings.get(R.string.pair_trigger_after_ok)
    "resume" -> Strings.get(R.string.pair_trigger_resumed)
    else -> trigger.replaceFirstChar { it.uppercase() }
}

/** "34 to Drive (48 MB) · 12 to phone · 3 to recycle bin · 2 conflicts · Scheduled · 1 min 12 s". */
fun runCountsText(r: SyncRunEntity): String {
    val parts = buildList {
        if (r.uploaded > 0) add(
            if (r.bytesUp > 0) Strings.get(R.string.pair_run_to_drive_bytes, Fmt.count(r.uploaded), Fmt.bytes(r.bytesUp))
            else Strings.get(R.string.pair_run_to_drive, Fmt.count(r.uploaded)),
        )
        if (r.downloaded > 0) add(
            if (r.bytesDown > 0) Strings.get(R.string.pair_run_to_phone_bytes, Fmt.count(r.downloaded), Fmt.bytes(r.bytesDown))
            else Strings.get(R.string.pair_run_to_phone, Fmt.count(r.downloaded)),
        )
        val binned = r.deletedLocal + r.deletedRemote
        if (binned > 0) add(Strings.get(R.string.pair_run_to_bin, Fmt.count(binned)))
        if (r.conflicts > 0) add(Strings.plural(R.plurals.pair_n_conflicts, r.conflicts, Fmt.count(r.conflicts)))
        if (r.skipped > 0) add(Strings.get(R.string.pair_run_skipped, Fmt.count(r.skipped)))
        if (r.errors > 0) add(Strings.plural(R.plurals.pair_run_n_failed, r.errors, Fmt.count(r.errors)))
    }
    val counts = if (parts.isEmpty()) (if (r.status == RunStatus.SUCCESS) Strings.get(R.string.pair_run_no_changes) else "") else parts.joinToString(" · ")
    val duration = r.finishedAt?.let { pdDuration(it - r.startedAt) }
    val tail = listOfNotNull(triggerLabel(r.trigger), duration, r.message?.takeIf { it.isNotBlank() && r.status != RunStatus.SUCCESS })
    return listOf(counts, tail.joinToString(" · ")).filter { it.isNotEmpty() }.joinToString(" · ")
}

/** "12 s", "1 min 12 s", "1 h 5 min". */
private fun pdDuration(ms: Long): String? {
    if (ms < 0) return null
    val s = ms / 1000
    return when {
        s < 60 -> Strings.get(R.string.pair_run_duration_s, s)
        s < 3600 -> if (s % 60 == 0L) Strings.get(R.string.pair_run_duration_min, s / 60) else Strings.get(R.string.pair_run_duration_min_s, s / 60, s % 60)
        else -> if ((s % 3600) / 60 == 0L) Strings.get(R.string.pair_run_duration_h, s / 3600) else Strings.get(R.string.pair_run_duration_h_min, s / 3600, (s % 3600) / 60)
    }
}

private fun pdDriveFolderUrl(id: String) = "https://drive.google.com/drive/folders/$id"
