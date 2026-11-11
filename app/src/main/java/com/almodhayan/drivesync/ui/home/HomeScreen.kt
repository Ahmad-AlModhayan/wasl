package com.almodhayan.drivesync.ui.home

import android.Manifest
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.almodhayan.drivesync.ui.onboarding.DeviceChecks
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.common.AppScreen
import com.almodhayan.drivesync.ui.common.EmptyState
import com.almodhayan.drivesync.ui.common.LoadingState
import com.almodhayan.drivesync.ui.components.ChipVariant
import com.almodhayan.drivesync.ui.components.DsFab
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.FolderPairCard
import com.almodhayan.drivesync.ui.components.MenuAction
import com.almodhayan.drivesync.ui.components.PairDirection
import com.almodhayan.drivesync.ui.components.SectionHeader
import com.almodhayan.drivesync.ui.components.StatusBanner
import com.almodhayan.drivesync.ui.components.StatusChip
import com.almodhayan.drivesync.ui.components.StorageBar
import com.almodhayan.drivesync.ui.components.StorageModel
import com.almodhayan.drivesync.ui.components.UiAction
import com.almodhayan.drivesync.ui.components.showMessage
import com.almodhayan.drivesync.ui.components.showUndo
import com.almodhayan.drivesync.ui.onboarding.rememberResumeTick
import com.almodhayan.drivesync.ui.theme.Ds
import com.almodhayan.drivesync.ui.theme.Size
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import com.almodhayan.drivesync.ui.theme.tabular
import kotlinx.coroutines.launch

/** Room under the list for the FAB (48dp + 16dp margin + 16dp air). */
private val FabClearance = Size.fab + Space.s32

/**
 * Pairs (Home). Busy: one banner for the app's most urgent loud state, then a card per pair.
 * Quiet (every pair synced): a one-line summary, the storage bars and a compact list.
 * Top-level route: MainActivity draws the navigation bar.
 */
@Composable
fun HomeScreen(nav: AppNav, vm: HomeViewModel = hiltViewModel()) {
    val state by vm.state.collectAsStateWithLifecycle()
    val pendingUndo by vm.pendingUndo.collectAsStateWithLifecycle()
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }
    val listState = rememberLazyListState()
    val scrollBehavior = TopAppBarDefaults.pinnedScrollBehavior()
    val fabExpanded by remember { derivedStateOf { listState.firstVisibleItemIndex == 0 } }
    val cantOpenText = stringResource(R.string.home_cant_open_page)
    val syncNowText = stringResource(R.string.common_sync_now)
    val turnOnText = stringResource(R.string.home_turn_on)
    val resumeText = stringResource(R.string.common_resume)
    val pauseText = stringResource(R.string.common_pause)
    val previewText = stringResource(R.string.home_preview)
    val editText = stringResource(R.string.home_edit)
    val removePairText = stringResource(R.string.common_remove_pair)

    // All files access can change in Android settings while we are away.
    val tick = rememberResumeTick()
    LaunchedEffect(tick) { vm.refreshDevice() }

    // Installs that finished onboarding without the notification row never saw Android's
    // question, so the status notification stayed off silently (E2E 2026-10). Ask once per launch
    // while pairs exist; after a refusal Android itself stops showing it, and Diagnostics remains.
    val notificationLauncher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {}
    var notificationAsked by rememberSaveable { mutableStateOf(false) }
    val hasPairs = state.pairs.isNotEmpty()
    LaunchedEffect(hasPairs) {
        if (hasPairs && !notificationAsked && DeviceChecks.needsNotificationRuntimePermission() &&
            !DeviceChecks.status(context).notifications
        ) {
            notificationAsked = true
            notificationLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }
    LaunchedEffect(vm) { vm.messages.collect { snackbar.showMessage(it) } }
    // A pair removed here or on the pair screen: offer Undo once.
    LaunchedEffect(pendingUndo) {
        val removed = pendingUndo ?: return@LaunchedEffect
        if (System.currentTimeMillis() - removed.removedAt <= PairRemoval.UNDO_WINDOW_MS) {
            snackbar.showUndo(Strings.get(R.string.home_removed_undo, removed.pair.name), onUndo = { vm.undoRemove(removed) })
        }
        vm.consumeUndo(removed)
    }

    val runAction: (LoudTarget) -> Unit = { target ->
        if (!target.perform(nav, context, retry = vm::syncNow)) {
            scope.launch { snackbar.showMessage(cantOpenText) }
        }
    }
    val pauseWithUndo: (FolderPairEntity) -> Unit = { p ->
        vm.pause(p.id)
        scope.launch { snackbar.showUndo(Strings.get(R.string.home_paused_pair, p.name), onUndo = { vm.resume(p.id) }) }
    }
    val menuFor: (HomePair) -> List<MenuAction> = { hp ->
        val p = hp.pair
        buildList {
            add(MenuAction(syncNowText, Sym.Sync) { vm.syncNow(p.id) })
            when {
                !p.enabled -> add(MenuAction(turnOnText, Sym.PlayArrow) { vm.turnOn(p.id) })
                p.paused -> add(MenuAction(resumeText, Sym.PlayArrow) { vm.resume(p.id) })
                else -> add(MenuAction(pauseText, Sym.Pause) { pauseWithUndo(p) })
            }
            add(MenuAction(previewText, Sym.Visibility) { nav.dryRun(p.id) })
            add(MenuAction(editText, Sym.Edit) { nav.pairEdit(p.id) })
            add(MenuAction(removePairText, Sym.Delete, destructive = true) { vm.remove(p.id) })
        }
    }

    AppScreen(
        title = stringResource(R.string.app_name),
        onBack = null,
        logo = true,
        scrollBehavior = scrollBehavior,
        actions = {
            // Shown only with something to sync: disabled, it looked the same and did nothing (UX audit 2026-10).
            if (state.pairs.isNotEmpty()) {
                IconButton(onClick = vm::syncAll, modifier = Modifier.testTag("home_sync_all")) { Icon(Sym.Sync, contentDescription = stringResource(R.string.home_sync_all_now)) }
            }
            IconButton(onClick = { nav.accounts() }, modifier = Modifier.testTag("home_account")) { Icon(Sym.AccountCircle, contentDescription = stringResource(R.string.home_account)) }
        },
        floatingActionButton = {
            if (state.hasAccounts && state.pairs.isNotEmpty()) {
                DsFab(text = stringResource(R.string.home_new_pair), icon = Sym.Add, expanded = fabExpanded, onClick = { nav.pairEdit(null) }, modifier = Modifier.testTag("home_new_pair"))
            }
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        val modifier = Modifier.fillMaxSize().padding(padding)
        when {
            state.loading -> Box(modifier) { LoadingState() }
            !state.hasAccounts -> EmptyHome(
                modifier = modifier,
                banner = state.banner,
                onBanner = runAction,
                icon = Sym.AccountCircle,
                title = stringResource(R.string.home_empty_account_title),
                body = stringResource(R.string.home_empty_account_body),
                actionLabel = stringResource(R.string.home_empty_account_action),
                onAction = { nav.accounts() },
            )
            state.pairs.isEmpty() -> EmptyHome(
                modifier = modifier,
                banner = state.banner,
                onBanner = runAction,
                icon = Sym.CreateNewFolder,
                title = stringResource(R.string.home_empty_pairs_title),
                body = stringResource(R.string.home_empty_pairs_body),
                actionLabel = stringResource(R.string.home_new_pair),
                onAction = { nav.pairEdit(null) },
            )
            state.quiet -> QuietHome(state, listState, modifier, onOpen = { nav.pairDetail(it) })
            else -> BusyHome(
                state = state,
                listState = listState,
                modifier = modifier,
                onAction = runAction,
                onOpen = { nav.pairDetail(it) },
                menuFor = menuFor,
                onPause = pauseWithUndo,
            )
        }
    }
}

// ------------------------------------------------------------------ busy: banner + cards

@Composable
private fun BusyHome(
    state: HomeUiState,
    listState: LazyListState,
    modifier: Modifier,
    onAction: (LoudTarget) -> Unit,
    onOpen: (String) -> Unit,
    menuFor: (HomePair) -> List<MenuAction>,
    onPause: (FolderPairEntity) -> Unit,
) {
    LazyColumn(
        modifier = modifier,
        state = listState,
        contentPadding = PaddingValues(start = Space.s16, end = Space.s16, top = Space.s4, bottom = FabClearance),
        verticalArrangement = Arrangement.spacedBy(Space.s12),
    ) {
        state.banner?.let { banner -> item(key = "banner") { LoudBanner(banner, onAction) } }
        items(state.pairs, key = { it.pair.id }) { hp ->
            FolderPairCard(
                model = hp.card,
                modifier = Modifier.testTag("pair_card:" + hp.pair.name),
                onClick = { onOpen(hp.pair.id) },
                menu = menuFor(hp),
                action = hp.loud?.let { loud -> UiAction(loud.actionLabel) { onAction(loud.target) } },
                onPause = { onPause(hp.pair) },
            )
        }
    }
}

/** The one screen-level banner, with the fix as its filled button. */
@Composable
private fun LoudBanner(spec: LoudSpec, onAction: (LoudTarget) -> Unit, modifier: Modifier = Modifier) {
    StatusBanner(
        status = spec.status,
        title = spec.title,
        modifier = modifier,
        body = spec.body,
        primary = UiAction(spec.actionLabel) { onAction(spec.target) },
    )
}

// ------------------------------------------------------------------ quiet: summary + compact list

@Composable
private fun QuietHome(state: HomeUiState, listState: LazyListState, modifier: Modifier, onOpen: (String) -> Unit) {
    LazyColumn(modifier = modifier, state = listState, contentPadding = PaddingValues(top = Space.s4, bottom = FabClearance)) {
        item(key = "summary") {
            Column(Modifier.padding(horizontal = Space.s16), verticalArrangement = Arrangement.spacedBy(Space.s16)) {
                QuietHeader(state.pairs.size, state.checkedAt, state.now)
                if (state.phoneStorage != null || state.driveStorage.isNotEmpty()) StorageSummaryCard(state.phoneStorage, state.driveStorage)
            }
        }
        item(key = "pairs-header") { SectionHeader(stringResource(R.string.home_pairs_header), Modifier.padding(top = Space.s12)) }
        items(state.pairs, key = { it.pair.id }) { hp -> CompactPairRow(hp, state.now, onClick = { onOpen(hp.pair.id) }) }
    }
}

@Composable
private fun QuietHeader(pairCount: Int, checkedAt: Long?, now: Long) {
    val cs = MaterialTheme.colorScheme
    val synced = Ds.status.synced
    val subtitle = if (checkedAt != null) {
        pluralStringResource(R.plurals.home_quiet_pairs_checked, pairCount, Fmt.count(pairCount), Fmt.relativeLower(checkedAt, now))
    } else {
        pluralStringResource(R.plurals.home_quiet_pairs, pairCount, Fmt.count(pairCount))
    }
    Row(
        Modifier.fillMaxWidth().padding(start = Space.s4, end = Space.s4, top = Space.s8).semantics(mergeDescendants = true) {},
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.s12),
    ) {
        Box(Modifier.size(48.dp).clip(CircleShape).background(synced.container), contentAlignment = Alignment.Center) {
            Icon(Sym.CloudDoneFill, contentDescription = null, tint = synced.content, modifier = Modifier.size(26.dp))
        }
        Column {
            Text(stringResource(R.string.home_all_in_sync), style = MaterialTheme.typography.titleLarge, color = cs.onSurface, modifier = Modifier.semantics { heading() })
            Text(subtitle, style = MaterialTheme.typography.bodyMedium.tabular, color = cs.onSurfaceVariant)
        }
    }
}

/** Phone storage with all pairs, then Drive storage per account. */
@Composable
private fun StorageSummaryCard(phone: StorageModel?, drives: List<StorageModel>) {
    val cs = MaterialTheme.colorScheme
    val shape = MaterialTheme.shapes.large
    Column(
        Modifier.fillMaxWidth().clip(shape).background(Ds.lab.card).padding(Space.s16),
        verticalArrangement = Arrangement.spacedBy(Space.s12),
    ) {
        if (phone != null) StorageBar(phone)
        drives.forEach { StorageBar(it) }
    }
}

/** Dense row: mode tile, name, "from → to", icon-only status (with its word as the description) and when. */
@Composable
private fun CompactPairRow(item: HomePair, now: Long, onClick: () -> Unit) {
    val cs = MaterialTheme.colorScheme
    val direction = item.card.direction
    val route = stringResource(
        when (direction) {
            PairDirection.TwoWay -> R.string.home_route_two_way
            PairDirection.PhoneToDrive -> R.string.home_route_to_drive
            PairDirection.DriveToPhone -> R.string.home_route_to_phone
        },
        item.card.phonePath, item.card.drivePath,
    )
    val rowDescription = stringResource(R.string.home_pair_row_description, direction.title, item.card.phonePath, item.card.drivePath)
    Row(
        Modifier.fillMaxWidth().testTag("pair_card:" + item.pair.name)
            .clickable(onClickLabel = stringResource(R.string.home_open_pair, item.pair.name), onClick = onClick)
            .padding(horizontal = Space.s16, vertical = Space.s12),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.s12),
    ) {
        Box(Modifier.size(40.dp).clip(MaterialTheme.shapes.medium).background(cs.surfaceContainerHighest), contentAlignment = Alignment.Center) {
            Icon(direction.modeIcon, contentDescription = null, tint = cs.onSurfaceVariant, modifier = Modifier.size(Size.iconRow))
        }
        Column(Modifier.weight(1f)) {
            Text(item.pair.name, style = MaterialTheme.typography.titleMedium, color = cs.onSurface, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(
                route,
                style = MaterialTheme.typography.bodyMedium,
                color = cs.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.semantics { contentDescription = rowDescription },
            )
        }
        Column(horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(Space.s2)) {
            StatusChip(item.status.status, label = item.status.label, icon = item.status.icon, variant = ChipVariant.Icon)
            item.pair.lastSuccessAt?.let {
                Text(Fmt.relative(it, now), style = MaterialTheme.typography.bodySmall.tabular, color = cs.onSurfaceVariant, maxLines = 1)
            }
        }
    }
}

// ------------------------------------------------------------------ empty states

@Composable
private fun EmptyHome(
    modifier: Modifier,
    banner: LoudSpec?,
    onBanner: (LoudTarget) -> Unit,
    icon: ImageVector,
    title: String,
    body: String,
    actionLabel: String,
    onAction: () -> Unit,
) {
    Column(modifier) {
        if (banner != null) LoudBanner(banner, onBanner, Modifier.padding(start = Space.s16, end = Space.s16, top = Space.s4))
        EmptyState(icon = icon, title = title, body = body, modifier = Modifier.weight(1f)) {
            Button(onClick = onAction, contentPadding = ButtonDefaults.ButtonWithIconContentPadding, modifier = Modifier.testTag("home_empty_action")) {
                Icon(Sym.Add, contentDescription = null, modifier = Modifier.size(Size.iconSmall))
                Spacer(Modifier.width(Space.s8))
                Text(actionLabel)
            }
        }
    }
}
