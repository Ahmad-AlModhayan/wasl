package com.almodhayan.drivesync.ui.drive

import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.db.DriveNodeEntity
import com.almodhayan.drivesync.drive.FOLDER_MIME
import com.almodhayan.drivesync.drive.GOOGLE_APPS_PREFIX
import com.almodhayan.drivesync.ondemand.OnDemandConstants
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.common.AppScreen
import com.almodhayan.drivesync.ui.common.EmptyState
import com.almodhayan.drivesync.ui.common.EmptyStateAction
import com.almodhayan.drivesync.ui.common.LoadingState
import com.almodhayan.drivesync.ui.components.Callout
import com.almodhayan.drivesync.ui.components.CalloutTone
import com.almodhayan.drivesync.ui.components.DsDivider
import com.almodhayan.drivesync.ui.components.DsLinearProgress
import com.almodhayan.drivesync.ui.components.DsListItem
import com.almodhayan.drivesync.ui.components.DsSegmentedButton
import com.almodhayan.drivesync.ui.components.DsTextField
import com.almodhayan.drivesync.ui.components.FileKind
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.ItemTone
import com.almodhayan.drivesync.ui.components.NavIcon
import com.almodhayan.drivesync.ui.components.PathBreadcrumb
import com.almodhayan.drivesync.ui.components.UiAction
import com.almodhayan.drivesync.ui.components.showMessage
import com.almodhayan.drivesync.ui.components.showUndo
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import kotlinx.coroutines.launch

/**
 * Browse one account's Drive: My Drive, Shared with me and (when there are any) shared drives,
 * with keep-offline, version history, save to Downloads, and pairing a folder with the phone.
 */
@Composable
fun DriveBrowserScreen(nav: AppNav, accountId: String, folderId: String?) {
    val vm: DriveBrowserViewModel = hiltViewModel()
    val ui by vm.ui.collectAsStateWithLifecycle()
    val failure by vm.failure.collectAsStateWithLifecycle()
    val context = LocalContext.current
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    var searching by remember { mutableStateOf(false) }
    var sheetNode by remember { mutableStateOf<DriveNodeEntity?>(null) }
    val noFilesApp = stringResource(R.string.drive_no_files_app)
    val noDriveApp = stringResource(R.string.drive_no_drive_app)

    LaunchedEffect(vm) {
        vm.notes.collect { n ->
            scope.launch {
                val undo = n.undo
                if (undo != null) snackbar.showUndo(n.text, undo) else snackbar.showMessage(n.text)
            }
        }
    }
    fun exitSearch() { searching = false; vm.clearSearch() }
    BackHandler(enabled = searching || ui.crumbs.size > 1) {
        if (searching) exitSearch() else vm.up()
    }

    AppScreen(
        title = stringResource(if (searching) R.string.drive_search_drive else R.string.drive_browse_drive),
        subtitle = vm.accountId.ifBlank { accountId },
        onBack = { if (searching) exitSearch() else nav.back() },
        navIcon = if (searching) NavIcon.Close else NavIcon.Back,
        snackbarHost = { SnackbarHost(snackbar) },
        actions = {
            if (!searching) IconButton(onClick = { searching = true }) { Icon(Sym.Search, contentDescription = stringResource(R.string.drive_search_drive)) }
            IconButton(onClick = { vm.refresh() }) { Icon(Sym.Sync, contentDescription = stringResource(R.string.drive_refresh_folder)) }
            IconButton(onClick = {
                if (!startSafely(context, OnDemandConstants.openFilesIntent(vm.accountId))) vm.note(noFilesApp)
            }) { Icon(Sym.FolderOpen, contentDescription = stringResource(R.string.drive_open_in_files)) }
        },
    ) { padding ->
        Column(Modifier.padding(padding).fillMaxSize()) {
            if (searching) {
                DsTextField(
                    value = ui.query, onValueChange = { vm.setQuery(it) }, label = stringResource(R.string.drive_search_label),
                    leading = Sym.Search,
                    modifier = Modifier.padding(horizontal = Space.s16, vertical = Space.s8),
                )
                if (ui.indexing) {
                    Callout(
                        text = stringResource(R.string.drive_search_indexing),
                        tone = CalloutTone.Info,
                        modifier = Modifier.padding(horizontal = Space.s16, vertical = Space.s4),
                    )
                }
            } else {
                val places = ui.places
                DsSegmentedButton(
                    items = places.map { stringResource(it.labelRes) },
                    selectedIndex = places.indexOf(ui.place).coerceAtLeast(0),
                    onSelect = { i -> vm.openPlace(places[i]) },
                    modifier = Modifier.padding(start = Space.s16, end = Space.s16, top = Space.s4, bottom = Space.s8),
                )
                if (ui.crumbs.isNotEmpty()) {
                    PathBreadcrumb(
                        rootLabel = ui.crumbs.first().name,
                        rootIcon = Sym.Cloud,
                        parts = ui.crumbs.drop(1).map { it.name },
                        onCrumb = { i -> vm.goTo(i + 1) },
                    )
                }
                if (ui.indexing) {
                    Callout(
                        text = stringResource(if (ui.place == DriveBrowserViewModel.Place.SharedWithMe) R.string.drive_indexing_shared else R.string.drive_indexing_folders),
                        tone = CalloutTone.Info,
                        modifier = Modifier.padding(horizontal = Space.s16, vertical = Space.s4),
                    )
                }
            }
            ui.busy?.let { b ->
                Column(Modifier.fillMaxWidth().padding(horizontal = Space.s16, vertical = Space.s8)) {
                    Text(b, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    DsLinearProgress(null, Modifier.padding(top = Space.s4))
                }
            }
            failure?.let { f ->
                val tryAgain = stringResource(R.string.common_try_again)
                val dismiss = stringResource(R.string.drive_dismiss)
                Callout(
                    text = f.hint, title = f.title, tone = CalloutTone.Danger,
                    action = f.retry?.let { retry -> UiAction(tryAgain, Sym.Sync) { vm.dismissFailure(); retry() } }
                        ?: UiAction(dismiss) { vm.dismissFailure() },
                    modifier = Modifier.padding(horizontal = Space.s16, vertical = Space.s4),
                )
            }
            DsDivider()

            val list = if (searching) ui.results else ui.items
            Box(Modifier.fillMaxSize()) {
                when {
                    searching && ui.query.isBlank() -> Text(
                        stringResource(R.string.drive_search_prompt),
                        style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(Space.s16),
                    )
                    searching && list != null && list.isEmpty() -> EmptyState(Sym.Search, stringResource(R.string.drive_no_matches), stringResource(R.string.drive_no_matches_body, ui.query))
                    !searching && ui.loading -> LoadingState(stringResource(R.string.drive_listing))
                    !searching && ui.error != null -> EmptyState(Sym.CloudOff, stringResource(R.string.drive_list_failed), ui.error) {
                        EmptyStateAction(stringResource(R.string.common_try_again), onClick = { vm.refresh() })
                    }
                    !searching && list != null && list.isEmpty() -> EmptyState(
                        if (ui.place == DriveBrowserViewModel.Place.SharedWithMe && ui.crumbs.size == 1) Sym.FolderShared else Sym.Folder,
                        emptyTitle(ui),
                    )
                    list != null -> LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = Space.s16)) {
                        items(list, key = { it.id }) { node ->
                            NodeRow(
                                node = node,
                                pinned = node.id in ui.pinned,
                                driveRoot = !searching && ui.atDriveList,
                                onClick = {
                                    if (isFolderLike(node)) {
                                        if (searching) exitSearch()
                                        vm.open(node)
                                    } else sheetNode = node
                                },
                                onMore = { sheetNode = node },
                            )
                        }
                    }
                    else -> LoadingState(stringResource(R.string.drive_searching))
                }
            }
        }
    }

    sheetNode?.let { node ->
        val cs = MaterialTheme.colorScheme
        ModalBottomSheet(onDismissRequest = { sheetNode = null }, containerColor = cs.surfaceContainerHigh) {
            val pinned = node.id in ui.pinned
            val folder = isFolderLike(node)
            Column(Modifier.navigationBarsPadding().padding(bottom = Space.s16)) {
                DsListItem(headline = node.name, supporting = nodeSubtitle(node), icon = nodeIcon(node), tone = if (folder) ItemTone.Drive else ItemTone.None)
                DsDivider()
                if (!ui.atDriveList) {
                    SheetAction(Sym.CloudDownload, stringResource(if (pinned) R.string.drive_stop_keeping_offline else R.string.drive_keep_offline)) { sheetNode = null; vm.togglePin(node) }
                }
                if (folder) {
                    SheetAction(Sym.FolderOpen, stringResource(R.string.drive_open_folder)) { sheetNode = null; vm.open(node) }
                    SheetAction(Sym.SyncAlt, stringResource(R.string.drive_pair_with_phone_folder)) {
                        sheetNode = null
                        val t = vm.pairTarget(node)
                        if (t != null) nav.pairEditWithDriveFolder(vm.accountId, t.folderId, t.path, t.driveId) else nav.pairEdit(null, vm.accountId)
                    }
                } else {
                    SheetAction(Sym.History, stringResource(R.string.common_version_history)) { sheetNode = null; nav.revisions(vm.accountId, node.id, node.name) }
                    SheetAction(
                        Sym.Download,
                        stringResource(if (node.mimeType.startsWith(GOOGLE_APPS_PREFIX)) R.string.drive_save_office_copy else R.string.drive_save_copy),
                        enabled = ui.busy == null,
                    ) { sheetNode = null; vm.download(node) }
                }
                SheetAction(Sym.OpenInNew, stringResource(R.string.drive_open_in_google_drive)) {
                    sheetNode = null
                    val url = if (folder) "https://drive.google.com/drive/folders/${node.shortcutTargetId ?: node.id}" else "https://drive.google.com/file/d/${node.id}/view"
                    if (!startSafely(context, Intent(Intent.ACTION_VIEW, Uri.parse(url)))) vm.note(noDriveApp)
                }
            }
        }
    }
}

@Composable
private fun SheetAction(icon: ImageVector, label: String, enabled: Boolean = true, onClick: () -> Unit) {
    DsListItem(headline = label, icon = icon, enabled = enabled, onClick = onClick)
}

@Composable
private fun NodeRow(node: DriveNodeEntity, pinned: Boolean, driveRoot: Boolean, onClick: () -> Unit, onMore: () -> Unit) {
    val folder = isFolderLike(node)
    DsListItem(
        headline = node.name,
        supporting = if (driveRoot) stringResource(R.string.common_shared_drive) else nodeSubtitle(node),
        icon = if (driveRoot) Sym.FolderSharedFill else nodeIcon(node),
        tone = if (folder) ItemTone.Drive else ItemTone.None,
        meta = if (pinned) stringResource(R.string.drive_offline) else null,
        trailing = {
            if (!driveRoot) IconButton(onClick = onMore) { Icon(Sym.MoreVert, contentDescription = stringResource(R.string.components_more_for, node.name)) }
        },
        chevron = driveRoot,
        onClick = onClick,
        onLongClick = if (driveRoot) null else onMore,
    )
}

private fun isFolderLike(node: DriveNodeEntity): Boolean = node.isFolder || (node.isShortcut && node.shortcutTargetMime == FOLDER_MIME)

private fun nodeIcon(node: DriveNodeEntity): ImageVector = when {
    isFolderLike(node) -> if (node.shared) Sym.FolderSharedFill else Sym.FolderFill
    node.mimeType.startsWith(GOOGLE_APPS_PREFIX) -> Sym.Description
    else -> FileKind.of(node.name).icon
}

private fun googleTypeLabel(mime: String): String = when (mime) {
    "application/vnd.google-apps.document" -> Strings.get(R.string.drive_type_doc)
    "application/vnd.google-apps.spreadsheet" -> Strings.get(R.string.drive_type_sheet)
    "application/vnd.google-apps.presentation" -> Strings.get(R.string.drive_type_slides)
    "application/vnd.google-apps.form" -> Strings.get(R.string.drive_type_form)
    "application/vnd.google-apps.drawing" -> Strings.get(R.string.drive_type_drawing)
    "application/vnd.google-apps.shortcut" -> Strings.get(R.string.drive_shortcut)
    else -> Strings.get(R.string.drive_type_google_file)
}

/** "Shared · 3 people see changes", "2.4 MB · Sep 28", "Google Doc · Yesterday". */
private fun nodeSubtitle(node: DriveNodeEntity): String {
    val parts = ArrayList<String>()
    when {
        node.isShortcut -> parts += Strings.get(R.string.drive_shortcut)
        node.isFolder && node.shared -> {
            val others = (node.permissionCount ?: 0) - 1
            parts += if (others >= 1) Strings.plural(R.plurals.drive_shared_with_people, others, others) else Strings.get(R.string.drive_shared)
        }
        node.isFolder -> parts += Strings.get(R.string.drive_folder)
        node.mimeType.startsWith(GOOGLE_APPS_PREFIX) -> parts += googleTypeLabel(node.mimeType)
        node.size != null -> parts += Fmt.bytes(node.size)
    }
    if (node.modifiedTime > 0) parts += Fmt.relative(node.modifiedTime)
    return parts.joinToString(" · ")
}

private fun emptyTitle(ui: DriveBrowserViewModel.Ui): String = when {
    ui.crumbs.size > 1 -> Strings.get(R.string.drive_empty_folder)
    ui.place == DriveBrowserViewModel.Place.SharedWithMe -> Strings.get(R.string.drive_empty_shared_with_me)
    ui.place == DriveBrowserViewModel.Place.SharedDrives -> Strings.get(R.string.drive_empty_shared_drives)
    else -> Strings.get(R.string.drive_empty_my_drive)
}

private fun startSafely(context: Context, intent: Intent): Boolean = try {
    context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    true
} catch (e: Exception) {
    false
}
