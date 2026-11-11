package com.almodhayan.drivesync.ui.pair

import androidx.compose.foundation.ScrollState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.platform.LocalContext
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.LocalViewModelStoreOwner
import androidx.navigation.NavBackStackEntry
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.common.AppScreen
import com.almodhayan.drivesync.ui.common.EmptyState
import com.almodhayan.drivesync.ui.common.EmptyStateAction
import com.almodhayan.drivesync.ui.common.LoadingState
import com.almodhayan.drivesync.ui.components.BottomActionBar
import com.almodhayan.drivesync.ui.components.Callout
import com.almodhayan.drivesync.ui.components.CalloutTone
import com.almodhayan.drivesync.ui.components.DsTextField
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.NavIcon
import com.almodhayan.drivesync.ui.components.PairDirection
import com.almodhayan.drivesync.ui.components.PathRow
import com.almodhayan.drivesync.ui.components.SectionHeader
import com.almodhayan.drivesync.ui.components.Side
import com.almodhayan.drivesync.ui.components.SyncModeOption
import com.almodhayan.drivesync.ui.components.UiAction
import com.almodhayan.drivesync.ui.components.showUndo
import com.almodhayan.drivesync.ui.theme.Size
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import kotlinx.coroutines.flow.MutableStateFlow

/**
 * Pair editor (design screen 3): Name → Folders → Sync mode → Deletions → When to sync → Skip, then
 * Advanced. Warnings sit under the setting that causes them. The filled button previews; an overlap
 * with another pair blocks both Save and the preview.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun PairEditorScreen(nav: AppNav, pairId: String?, accountId: String?) {
    val vm: PairEditorViewModel = hiltViewModel()
    val state by vm.state.collectAsStateWithLifecycle()
    val snackbar = remember { SnackbarHostState() }
    val scroll = rememberScrollState()
    val scrollBehavior = TopAppBarDefaults.pinnedScrollBehavior()
    var advancedOpen by rememberSaveable { mutableStateOf(false) }

    PickerResults(nav, ready = state.draft != null, vm = vm)
    LifecycleResumeEffect(Unit) { vm.recheck(); onPauseOrDispose { } }

    val saved = state.saved
    LaunchedEffect(saved) {
        if (saved != null) {
            vm.dismissSaved()
            nav.back()
            if (saved.after == PairEditorViewModel.After.Preview) nav.dryRun(saved.id)
        }
    }
    LaunchedEffect(state.failedSaves) {
        if (state.failedSaves > 0) {
            if (state.includeProblem != null || state.vaultProblem != null) advancedOpen = true
            scroll.animateScrollTo(0)
        }
    }
    val removedText = state.draft?.name?.ifBlank { null }?.let { stringResource(R.string.pair_editor_removed_name, it) }
        ?: stringResource(R.string.pair_editor_removed_the_pair)
    LaunchedEffect(state.removed) {
        if (state.removed) {
            var undone = false
            snackbar.showUndo(removedText, onUndo = { undone = true; vm.undoRemove() })
            if (!undone) nav.home()
        }
    }

    val title = if (vm.isNew) stringResource(R.string.pair_editor_title_new) else state.original?.name?.ifBlank { null } ?: stringResource(R.string.pair_editor_title_edit)
    AppScreen(
        title = title,
        onBack = { nav.back() },
        navIcon = NavIcon.Close,
        scrollBehavior = scrollBehavior,
        snackbarHost = { SnackbarHost(snackbar) },
        actions = {
            if (state.draft != null && !state.removed) {
                TextButton(onClick = { vm.save(PairEditorViewModel.After.Close) }, enabled = !state.blocked && !state.saving, modifier = Modifier.testTag("editor_save")) { Text(stringResource(R.string.common_save)) }
            }
        },
        bottomBar = {
            if (state.draft != null && !state.removed) {
                BottomActionBar(
                    hint = when {
                        state.blocked -> stringResource(R.string.pair_editor_hint_fix_overlap)
                        vm.isNew -> stringResource(R.string.pair_editor_hint_nothing_before_preview)
                        else -> null
                    },
                ) {
                    Button(onClick = { vm.save(PairEditorViewModel.After.Preview) }, enabled = !state.blocked && !state.saving, modifier = Modifier.testTag("editor_preview")) {
                        Icon(Sym.Visibility, contentDescription = null, modifier = Modifier.size(Size.iconSmall))
                        Spacer(Modifier.size(Space.s8))
                        Text(stringResource(if (vm.isNew) R.string.pair_dryrun_title_first_sync else R.string.pair_dryrun_title_changes))
                    }
                }
            }
        },
    ) { padding ->
        when {
            !state.loaded -> Box(Modifier.padding(padding)) { LoadingState(stringResource(R.string.pair_loading)) }
            state.missing -> EmptyState(Sym.Error, stringResource(R.string.pair_editor_missing_title), stringResource(R.string.pair_editor_missing_body), Modifier.padding(padding)) {
                EmptyStateAction(stringResource(R.string.common_back), onClick = { nav.back() })
            }
            state.removed -> EmptyState(Sym.Delete, stringResource(R.string.pair_editor_removed_title), stringResource(R.string.pair_editor_removed_body), Modifier.padding(padding)) {
                EmptyStateAction(stringResource(R.string.common_back), onClick = { nav.back() })
            }
            else -> EditorContent(
                state = state, vm = vm, nav = nav, scroll = scroll,
                advancedOpen = advancedOpen, onAdvanced = { advancedOpen = it },
                modifier = Modifier.padding(padding).consumeWindowInsets(padding).imePadding(),
            )
        }
    }
}

/** Reads the pickers' results from this screen's own back stack entry (convention in Nav.kt). */
@Composable
private fun PickerResults(nav: AppNav, ready: Boolean, vm: PairEditorViewModel) {
    val owner = LocalViewModelStoreOwner.current
    val entry = remember(owner) { owner as? NavBackStackEntry ?: nav.controller.currentBackStackEntry }
    val handle = entry?.savedStateHandle
    val localFlow = remember(handle) { handle?.getStateFlow<String?>(PickerKeys.LOCAL_PATH, null) ?: MutableStateFlow<String?>(null) }
    val remoteFlow = remember(handle) { handle?.getStateFlow<String?>(PickerKeys.REMOTE_FOLDER_ID, null) ?: MutableStateFlow<String?>(null) }
    val pickedLocal by localFlow.collectAsStateWithLifecycle()
    val pickedRemote by remoteFlow.collectAsStateWithLifecycle()
    LaunchedEffect(pickedLocal, ready) {
        val path = pickedLocal ?: return@LaunchedEffect
        if (!ready) return@LaunchedEffect
        vm.onLocalPicked(path)
        handle?.set<String>(PickerKeys.LOCAL_PATH, null)
    }
    LaunchedEffect(pickedRemote, ready) {
        val id = pickedRemote ?: return@LaunchedEffect
        if (!ready) return@LaunchedEffect
        vm.onRemotePicked(id, handle?.get<String>(PickerKeys.REMOTE_PATH) ?: "", handle?.get<String>(PickerKeys.DRIVE_ID) ?: "")
        handle?.set<String>(PickerKeys.REMOTE_FOLDER_ID, null)
        handle?.set<String>(PickerKeys.REMOTE_PATH, null)
        handle?.set<String>(PickerKeys.DRIVE_ID, null)
    }
}

@Composable
private fun EditorContent(
    state: PairEditorViewModel.State,
    vm: PairEditorViewModel,
    nav: AppNav,
    scroll: ScrollState,
    advancedOpen: Boolean,
    onAdvanced: (Boolean) -> Unit,
    modifier: Modifier,
) {
    val d = state.draft ?: return
    Column(modifier.fillMaxSize().verticalScroll(scroll).padding(bottom = Space.s24)) {
        // A folder another device offered (docs/INTRODUCTIONS.md §3.3): say what saving does.
        vm.offer?.let {
            Callout(stringResource(R.string.intro_editor_offer_body), Modifier.padding(start = Space.s16, end = Space.s16, top = Space.s8), tone = CalloutTone.Info, title = stringResource(R.string.intro_editor_offer_title, it.name))
        }
        if (state.showErrors && state.problems.isNotEmpty()) {
            Callout(
                state.problems.joinToString(" ") { it.trimEnd('.') + "." },
                Modifier.padding(start = Space.s16, end = Space.s16, top = Space.s8),
                tone = CalloutTone.Danger,
                title = stringResource(R.string.pair_editor_cant_save_yet),
            )
        }
        state.saveError?.let {
            Callout(it, Modifier.padding(start = Space.s16, end = Space.s16, top = Space.s8), tone = CalloutTone.Danger, title = stringResource(R.string.pair_editor_save_failed))
        }
        DsTextField(
            value = d.name,
            onValueChange = vm::setName,
            label = stringResource(R.string.pair_editor_name),
            modifier = Modifier.padding(start = Space.s16, end = Space.s16, top = Space.s8),
            error = if (state.showErrors) state.nameProblem else null,
            keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
        )
        FoldersSection(state, vm, nav)
        SyncModeSection(state, vm)
        DeletionsSection(state, vm)
        WhenSection(state, vm)
        SkipSection(state, vm, nav, isNew = vm.isNew)
        AdvancedSection(state, vm, nav, isNew = vm.isNew, open = advancedOpen, onOpen = onAdvanced)
    }
}

// ------------------------------------------------------------------ folders

@Composable
private fun FoldersSection(state: PairEditorViewModel.State, vm: PairEditorViewModel, nav: AppNav) {
    val d = state.draft ?: return
    val context = LocalContext.current
    // SAF mode (SRS FR-R3): the system picker grants one folder, no All files access needed.
    val safPicker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri -> uri?.let(vm::onSafPicked) }
    SectionHeader(stringResource(R.string.pair_editor_folders))
    val chooseAccount = stringResource(R.string.pair_editor_choose_account)
    val chooseAnother = stringResource(R.string.pair_editor_choose_another_folder)
    if (state.accounts.size > 1) {
        MenuRow(
            title = stringResource(R.string.common_google_account),
            value = d.accountId.ifBlank { chooseAccount },
            options = state.accounts,
            selected = state.accounts.firstOrNull { it.id == d.accountId },
            optionLabel = { it.id },
            onSelect = { vm.setAccount(it.id) },
            icon = Sym.AccountCircle,
        )
    }
    Column(Modifier.padding(horizontal = Space.s16), verticalArrangement = Arrangement.spacedBy(Space.s8)) {
        if (state.accounts.isEmpty()) {
            Callout(stringResource(R.string.pair_editor_add_account_body), tone = CalloutTone.Warning, action = UiAction(stringResource(R.string.pair_editor_add_account)) { nav.accounts() })
        }
        PathRow(
            side = Side.Phone,
            path = d.localPath.takeIf { it.isNotBlank() }?.let { Fmt.humanLocalPath(it) },
            meta = when {
                d.localPath.isBlank() -> null
                state.phoneCounting -> stringResource(R.string.pair_editor_counting_files)
                else -> state.phoneCount?.text
            },
            error = if (state.showErrors) state.phoneProblem else state.phoneLiveProblem,
            // Android's own picker is the default (SRS FR-R3): one folder, no All files access.
            onPick = { safPicker.launch(null) },
        )
        val pathFolder = d.localPath.isNotBlank() && !d.localPath.startsWith("content://")
        if (!state.hasAllFilesAccess && pathFolder) {
            Callout(
                stringResource(R.string.pair_editor_no_access_body),
                tone = CalloutTone.Warning,
                action = UiAction(stringResource(R.string.pair_allow_all_files_access)) { openAllFilesAccess(context) },
            )
        }
        TextButton(onClick = { nav.localPicker(d.localPath.takeIf { pathFolder }) }) {
            Text(stringResource(if (pathFolder) R.string.pair_editor_path_choose_another else R.string.pair_editor_path_choose_instead))
        }
        state.phoneOverlap?.let { o ->
            Callout(overlapText(o), tone = CalloutTone.Warning, title = stringResource(R.string.pair_editor_overlaps, o.pairName), action = UiAction(chooseAnother) { nav.localPicker() })
        }
        PathRow(
            side = Side.Drive,
            path = if (d.remoteFolderId.isBlank()) null else Fmt.humanRemotePath(d.remotePath.ifBlank { Strings.get(R.string.pair_editor_drive_folder) }),
            meta = when {
                d.remoteFolderId.isBlank() -> null
                state.driveCounting -> stringResource(R.string.pair_editor_counting_files)
                else -> state.drive?.text
            },
            error = if (state.showErrors) state.driveProblem else null,
            onPick = { if (d.accountId.isBlank()) vm.showErrors() else nav.remotePicker(d.accountId) },
        )
        state.driveOverlap?.let { o ->
            Callout(overlapText(o), tone = CalloutTone.Warning, title = stringResource(R.string.pair_editor_overlaps, o.pairName), action = UiAction(chooseAnother) { nav.remotePicker(d.accountId) })
        }
        driveNotEmptyText(state)?.let { Callout(it, tone = CalloutTone.Info) }
        if (state.foldersChanged && state.syncingNow) {
            // The engine refuses this save until the run ends (#104); say so before Save, not after.
            Callout(stringResource(R.string.pair_editor_folders_syncing, d.name.ifBlank { state.original?.name.orEmpty() }), tone = CalloutTone.Warning)
        } else if (state.foldersChanged && (state.hasSynced || state.sharedWithDevices)) {
            val text = stringResource(R.string.pair_editor_folders_changed)
            Callout(if (state.sharedWithDevices) text + " " + stringResource(R.string.pair_editor_folders_changed_shared) else text, tone = CalloutTone.Info)
        }
    }
}

private fun overlapText(o: PairEditorViewModel.Overlap): String =
    Strings.get(R.string.pair_editor_overlap_body, o.path)

/** Only before a first sync, and only where the promise "deletes nothing" holds. */
private fun driveNotEmptyText(state: PairEditorViewModel.State): String? {
    if (!state.firstSync || !state.driveHasFiles || state.mirror) return null
    return when (state.direction) {
        PairDirection.TwoWay -> Strings.get(R.string.pair_editor_drive_not_empty_two_way)
        PairDirection.PhoneToDrive -> Strings.get(R.string.pair_editor_drive_not_empty_upload)
        PairDirection.DriveToPhone -> null
    }
}

// ------------------------------------------------------------------ sync mode

@Composable
private fun SyncModeSection(state: PairEditorViewModel.State, vm: PairEditorViewModel) {
    val selected = state.direction
    SectionHeader(stringResource(R.string.pair_editor_sync_mode))
    Column(Modifier.padding(horizontal = Space.s16).selectableGroup(), verticalArrangement = Arrangement.spacedBy(Space.s8)) {
        PairDirection.entries.forEach { dir ->
            SyncModeOption(
                direction = dir,
                selected = selected == dir,
                description = modeDescription(dir, state),
                onSelect = { vm.setDirection(dir) },
                recommended = dir == PairDirection.TwoWay,
            )
        }
    }
}

/** Each card says what it copies and what it never changes; the selected card reflects its options. */
private fun modeDescription(dir: PairDirection, state: PairEditorViewModel.State): String {
    val selected = state.direction == dir
    return when (dir) {
        PairDirection.TwoWay ->
            if (selected && state.draft?.propagateDeletions == false) Strings.get(R.string.pair_mode_two_way_restore)
            else Strings.get(R.string.pair_mode_two_way)
        PairDirection.PhoneToDrive -> when {
            selected && state.moveAfter -> Strings.get(R.string.pair_mode_upload_move)
            selected && state.mirror -> Strings.get(R.string.pair_mode_upload_mirror)
            else -> Strings.get(R.string.pair_mode_upload)
        }
        PairDirection.DriveToPhone -> when {
            selected && state.moveAfter -> Strings.get(R.string.pair_mode_download_move)
            selected && state.mirror -> Strings.get(R.string.pair_mode_download_mirror)
            else -> Strings.get(R.string.pair_mode_download)
        }
    }
}
