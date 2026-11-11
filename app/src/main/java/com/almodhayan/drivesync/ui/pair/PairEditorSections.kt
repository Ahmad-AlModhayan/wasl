package com.almodhayan.drivesync.ui.pair

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Slider
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.DpOffset
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.LifecycleResumeEffect
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.db.ConflictPolicy
import com.almodhayan.drivesync.data.db.DocsFormat
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.prefs.AppSettings
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.components.Callout
import com.almodhayan.drivesync.ui.components.CalloutTone
import com.almodhayan.drivesync.ui.components.ChipModel
import com.almodhayan.drivesync.ui.components.DsTextField
import com.almodhayan.drivesync.ui.components.FilterChipRow
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.PairDirection
import com.almodhayan.drivesync.ui.components.SectionHeader
import com.almodhayan.drivesync.ui.components.SettingControl
import com.almodhayan.drivesync.ui.components.SettingRow
import com.almodhayan.drivesync.ui.components.UiAction
import com.almodhayan.drivesync.ui.theme.Ds
import com.almodhayan.drivesync.ui.theme.MonoMedium
import com.almodhayan.drivesync.ui.theme.Motion
import com.almodhayan.drivesync.ui.theme.Size
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import com.almodhayan.drivesync.ui.theme.tabular

private typealias EditorState = PairEditorViewModel.State

// ------------------------------------------------------------------ deletions

@Composable
internal fun DeletionsSection(state: EditorState, vm: PairEditorViewModel) {
    val d = state.draft ?: return
    val binDays = state.cfg.trashRetentionDays
    SectionHeader(
        when (state.direction) {
            PairDirection.TwoWay -> stringResource(R.string.pair_del_header)
            PairDirection.PhoneToDrive -> stringResource(R.string.pair_del_header_upload)
            PairDirection.DriveToPhone -> stringResource(R.string.pair_del_header_download)
        },
    )
    val turnOffMirror = stringResource(R.string.pair_del_turn_off_mirror)
    when (state.direction) {
        PairDirection.TwoWay -> SettingRow(
            title = stringResource(R.string.pair_del_sync_deletions),
            icon = Sym.Delete,
            summary = if (d.propagateDeletions) pluralStringResource(R.plurals.pair_del_sync_deletions_on, binDays, binDays)
            else stringResource(R.string.pair_del_sync_deletions_off),
            control = SettingControl.Toggle(d.propagateDeletions) { v -> vm.update { it.copy(propagateDeletions = v) } },
        )
        PairDirection.PhoneToDrive -> {
            SettingRow(
                title = stringResource(R.string.pair_del_mirror),
                icon = Sym.Delete,
                summary = stringResource(if (state.mirror) R.string.pair_del_mirror_upload_on else R.string.pair_del_mirror_upload_off),
                control = SettingControl.Toggle(state.mirror) { vm.setMirror(it) },
            )
            if (state.mirror) {
                RowCallout(
                    stringResource(R.string.pair_del_mirror_upload_warning),
                    CalloutTone.Danger,
                    title = stringResource(R.string.pair_del_mirror_upload_warning_title),
                    action = UiAction(turnOffMirror) { vm.setMirror(false) },
                )
            }
            SettingRow(
                title = stringResource(R.string.pair_del_move_upload),
                icon = Sym.DriveFileMove,
                summary = stringResource(if (state.moveAfter) R.string.pair_del_move_upload_on else R.string.pair_del_move_upload_off),
                control = SettingControl.Toggle(state.moveAfter) { vm.setMoveAfter(it) },
            )
            if (state.moveAfter) {
                RowCallout(
                    stringResource(R.string.pair_del_move_upload_warning),
                    CalloutTone.Warning,
                    title = stringResource(R.string.pair_del_move_upload_warning_title),
                    action = UiAction(stringResource(R.string.pair_del_keep_on_phone)) { vm.setMoveAfter(false) },
                )
            }
        }
        PairDirection.DriveToPhone -> {
            SettingRow(
                title = stringResource(R.string.pair_del_mirror),
                icon = Sym.Delete,
                summary = stringResource(if (state.mirror) R.string.pair_del_mirror_download_on else R.string.pair_del_mirror_download_off),
                control = SettingControl.Toggle(state.mirror) { vm.setMirror(it) },
            )
            if (state.mirror) {
                RowCallout(
                    pluralStringResource(R.plurals.pair_del_mirror_download_warning, binDays, binDays),
                    CalloutTone.Danger,
                    title = stringResource(R.string.pair_del_mirror_download_warning_title),
                    action = UiAction(turnOffMirror) { vm.setMirror(false) },
                )
            }
            SettingRow(
                title = stringResource(R.string.pair_del_move_download),
                icon = Sym.DriveFileMove,
                summary = stringResource(if (state.moveAfter) R.string.pair_del_move_download_on else R.string.pair_del_move_download_off),
                control = SettingControl.Toggle(state.moveAfter) { vm.setMoveAfter(it) },
            )
            if (state.moveAfter) {
                RowCallout(
                    stringResource(R.string.pair_del_move_download_warning),
                    CalloutTone.Warning,
                    title = stringResource(R.string.pair_del_move_download_warning_title),
                    action = UiAction(stringResource(R.string.pair_del_keep_on_drive)) { vm.setMoveAfter(false) },
                )
            }
        }
    }
    if (state.deletionsPossible) {
        SettingRow(
            title = stringResource(R.string.pair_del_guard),
            icon = Sym.FrontHand,
            summary = if (d.massDeleteGuard) pluralStringResource(R.plurals.pair_del_guard_on, state.cfg.massDeleteMinFiles, state.cfg.massDeleteMinFiles, state.cfg.massDeleteThresholdPercent)
            else stringResource(R.string.pair_del_guard_off),
            control = SettingControl.Toggle(d.massDeleteGuard) { v -> vm.update { it.copy(massDeleteGuard = v) } },
        )
        if (!d.massDeleteGuard) {
            RowCallout(
                stringResource(R.string.pair_del_guard_off_warning),
                CalloutTone.Warning,
                title = stringResource(R.string.pair_del_guard_off_warning_title),
                action = UiAction(stringResource(R.string.pair_del_guard_turn_on)) { vm.update { it.copy(massDeleteGuard = true) } },
            )
        }
    }
}

// ------------------------------------------------------------------ when to sync

@Composable
internal fun WhenSection(state: EditorState, vm: PairEditorViewModel) {
    val d = state.draft ?: return
    var checkDialog by rememberSaveable { mutableStateOf(false) }
    var moreOpen by rememberSaveable { mutableStateOf(false) }
    SectionHeader(stringResource(R.string.pair_when_header))
    SettingRow(
        title = stringResource(R.string.pair_when_wifi_only),
        icon = Sym.Wifi,
        summary = when {
            d.wifiOnly -> null
            d.allowMetered -> stringResource(R.string.pair_when_wifi_only_off_metered)
            else -> stringResource(R.string.pair_when_wifi_only_off_unmetered)
        },
        control = SettingControl.Toggle(d.wifiOnly) { vm.setWifiOnly(it) },
    )
    val firstBytes = state.firstSyncBytes
    if (!d.wifiOnly && d.allowMetered && state.firstSync && firstBytes != null && firstBytes > PairEditorViewModel.LARGE_FIRST_SYNC) {
        RowCallout(
            stringResource(R.string.pair_when_large_first_sync),
            CalloutTone.Warning,
            title = stringResource(R.string.pair_when_large_first_sync_title, Fmt.bytes(firstBytes)),
            action = UiAction(stringResource(R.string.pair_when_turn_on_wifi_only)) { vm.setWifiOnly(true) },
        )
    }
    SettingRow(
        title = stringResource(R.string.pair_when_charging_only),
        icon = Sym.Bolt,
        control = SettingControl.Toggle(d.requireCharging) { v -> vm.update { it.copy(requireCharging = v) } },
    )
    SettingRow(
        title = stringResource(R.string.common_check_for_changes),
        icon = Sym.Schedule,
        summary = checkSummary(d, state.cfg),
        control = SettingControl.Chevron,
        onClick = { checkDialog = true },
    )
    ExpandRow(title = stringResource(R.string.pair_when_more_conditions), summary = moreSummary(d), expanded = moreOpen, onToggle = { moreOpen = !moreOpen }, icon = Sym.Tune, tag = "editor_more_conditions")
    if (moreOpen) MoreConditions(d, vm)
    if (checkDialog) {
        CheckForChangesDialog(
            d = d,
            cfg = state.cfg,
            onInstant = { v -> vm.update { it.copy(instantSync = v) } },
            onInterval = { m -> vm.update { it.copy(intervalMinutes = m) } },
            onDismiss = { checkDialog = false },
        )
    }
}

/** "When files change, and every 6 hours". */
private fun checkSummary(d: FolderPairEntity, cfg: AppSettings): String {
    val minutes = if (d.intervalMinutes > 0) d.intervalMinutes else cfg.globalIntervalMinutes
    val interval = Fmt.interval(minutes)
    val every = if (d.intervalMinutes <= 0) Strings.get(R.string.pair_when_interval_app_default, interval) else interval
    return if (d.instantSync && com.almodhayan.drivesync.core.SafPaths.watchable(d.localPath)) Strings.get(R.string.pair_when_on_change_and, every) else every.replaceFirstChar { it.uppercase() }
}

private fun moreSummary(d: FolderPairEntity): String {
    val parts = buildList {
        if (d.allowRoaming) add(Strings.get(R.string.pair_when_more_roaming))
        if (d.requireVpn) add(Strings.get(R.string.pair_when_more_vpn))
        if (d.minBatteryPercent > 0) add(Strings.get(R.string.pair_when_more_battery, d.minBatteryPercent))
        d.ssidAllowList().size.takeIf { it > 0 }?.let { add(Strings.plural(R.plurals.pair_when_more_only_on_networks, it, Fmt.count(it))) }
        d.ssidDenyList().size.takeIf { it > 0 }?.let { add(Strings.plural(R.plurals.pair_when_more_never_on_networks, it, Fmt.count(it))) }
    }
    return if (parts.isEmpty()) Strings.get(R.string.pair_when_more_summary_empty)
    else parts.joinToString(Strings.get(R.string.pair_list_separator)).replaceFirstChar { it.uppercase() }
}

@Composable
private fun MoreConditions(d: FolderPairEntity, vm: PairEditorViewModel) {
    val context = LocalContext.current
    val cs = MaterialTheme.colorScheme
    var locationGranted by remember { mutableStateOf(hasLocation(context)) }
    val requestLocation = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { locationGranted = it }
    LifecycleResumeEffect(Unit) { locationGranted = hasLocation(context); onPauseOrDispose { } }

    SettingRow(
        title = stringResource(R.string.pair_when_metered),
        icon = Sym.DataUsage,
        summary = when {
            d.wifiOnly -> stringResource(R.string.pair_when_metered_wifi_only)
            d.allowMetered -> stringResource(R.string.pair_when_metered_on)
            else -> stringResource(R.string.pair_when_metered_off)
        },
        control = SettingControl.Toggle(d.allowMetered) { v -> vm.update { it.copy(allowMetered = v) } },
        enabled = !d.wifiOnly,
    )
    SettingRow(
        title = stringResource(R.string.pair_when_roaming),
        icon = Sym.CellTower,
        summary = stringResource(if (d.allowRoaming) R.string.pair_when_roaming_on else R.string.pair_when_roaming_off),
        control = SettingControl.Toggle(d.allowRoaming) { v -> vm.update { it.copy(allowRoaming = v) } },
    )
    SettingRow(
        title = stringResource(R.string.pair_when_vpn),
        modifier = Modifier.testTag("editor_require_vpn"),
        icon = Sym.Shield,
        summary = stringResource(if (d.requireVpn) R.string.pair_when_vpn_on else R.string.pair_when_vpn_off),
        control = SettingControl.Toggle(d.requireVpn) { v -> vm.update { it.copy(requireVpn = v) } },
    )
    BatteryRow(d.minBatteryPercent) { v -> vm.update { it.copy(minBatteryPercent = v) } }
    Column(
        Modifier.padding(start = Space.s16, end = Space.s16, top = Space.s4, bottom = Space.s8),
        verticalArrangement = Arrangement.spacedBy(Space.s8),
    ) {
        DsTextField(
            value = d.ssidAllow,
            onValueChange = { v -> vm.update { it.copy(ssidAllow = v) } },
            label = stringResource(R.string.pair_when_ssid_allow),
            helper = stringResource(R.string.pair_when_ssid_allow_helper),
            leading = Sym.Wifi,
        )
        DsTextField(
            value = d.ssidDeny,
            onValueChange = { v -> vm.update { it.copy(ssidDeny = v) } },
            label = stringResource(R.string.pair_when_ssid_deny),
            helper = stringResource(R.string.pair_when_ssid_deny_helper),
            leading = Sym.WifiOff,
        )
        val hasLists = d.ssidAllow.isNotBlank() || d.ssidDeny.isNotBlank()
        if (!locationGranted) {
            if (hasLists) {
                Callout(
                    stringResource(R.string.pair_when_location_needed),
                    tone = CalloutTone.Warning,
                    title = stringResource(R.string.pair_when_location_needed_title),
                    action = UiAction(stringResource(R.string.pair_when_allow_location)) { requestLocation.launch(Manifest.permission.ACCESS_FINE_LOCATION) },
                )
            } else {
                Text(
                    stringResource(R.string.pair_when_location_note),
                    style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant,
                )
            }
        }
        Text(stringResource(R.string.pair_when_sync_now_ignores), style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant)
    }
}

private fun hasLocation(context: Context): Boolean =
    ContextCompat.checkSelfPermission(context, Manifest.permission.ACCESS_FINE_LOCATION) == PackageManager.PERMISSION_GRANTED

@Composable
private fun BatteryRow(percent: Int, onChange: (Int) -> Unit) {
    val cs = MaterialTheme.colorScheme
    val batteryTitle = stringResource(R.string.common_minimum_battery)
    Column(Modifier.fillMaxWidth().padding(start = Space.s16, end = Space.s16, top = Space.s12)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s16)) {
            Icon(Sym.BatteryAlert, contentDescription = null, tint = cs.onSurfaceVariant)
            Column(Modifier.weight(1f)) {
                Text(batteryTitle, style = MaterialTheme.typography.bodyLarge, color = cs.onSurface)
                Text(
                    if (percent <= 0) stringResource(R.string.pair_when_battery_off) else stringResource(R.string.pair_when_battery_on, percent),
                    style = MaterialTheme.typography.bodyMedium.tabular, color = cs.onSurfaceVariant,
                )
            }
            Text(stringResource(R.string.pair_percent, percent), style = MaterialTheme.typography.labelLarge.tabular, color = cs.onSurface)
        }
        Slider(
            value = percent.toFloat(),
            onValueChange = { v -> onChange((Math.round(v / 5f) * 5).coerceIn(0, 100)) },
            valueRange = 0f..100f,
            steps = 19,
            // Lines the slider up with the text, past the 24dp icon and its 16dp gap.
            modifier = Modifier.padding(start = Size.icon + Space.s16).semantics { contentDescription = batteryTitle },
        )
    }
}

/** "When files change" plus one interval; changes apply to the draft at once. */
@Composable
private fun CheckForChangesDialog(d: FolderPairEntity, cfg: AppSettings, onInstant: (Boolean) -> Unit, onInterval: (Int) -> Unit, onDismiss: () -> Unit) {
    val cs = MaterialTheme.colorScheme
    val choices = PairEditorViewModel.INTERVAL_CHOICES.let { base ->
        if (d.intervalMinutes > 0 && d.intervalMinutes !in base) (base + d.intervalMinutes).sorted() else base
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        icon = { Icon(Sym.Schedule, contentDescription = null, tint = cs.secondary) },
        title = { Text(stringResource(R.string.common_check_for_changes), style = MaterialTheme.typography.headlineSmall) },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState())) {
                // A folder another app provides can't be watched: say so instead of offering it (#133).
                val watchable = com.almodhayan.drivesync.core.SafPaths.watchable(d.localPath)
                Row(
                    Modifier.fillMaxWidth()
                        .toggleable(value = d.instantSync && watchable, enabled = watchable, role = Role.Switch, onValueChange = onInstant)
                        .heightIn(min = Size.listOneLine)
                        .padding(vertical = Space.s8),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(Space.s16),
                ) {
                    Column(Modifier.weight(1f)) {
                        Text(stringResource(R.string.pair_when_on_change), style = MaterialTheme.typography.bodyLarge, color = cs.onSurface)
                        Text(
                            stringResource(
                                when {
                                    !watchable -> R.string.pair_when_on_change_other_app
                                    cfg.instantMonitoring -> R.string.pair_when_on_change_on
                                    else -> R.string.pair_when_on_change_needs_instant
                                },
                            ),
                            style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant,
                        )
                    }
                    Switch(
                        checked = d.instantSync && watchable,
                        onCheckedChange = null,
                        enabled = watchable,
                        thumbContent = if (d.instantSync && watchable) { { Icon(Sym.Check, contentDescription = null, modifier = Modifier.size(16.dp)) } } else null,
                    )
                }
                Text(
                    stringResource(R.string.pair_when_and_schedule),
                    style = MaterialTheme.typography.titleSmall, color = cs.primary,
                    modifier = Modifier.padding(top = Space.s12, bottom = Space.s4),
                )
                Column(Modifier.selectableGroup()) {
                    choices.forEach { m ->
                        val selected = d.intervalMinutes.coerceAtLeast(0) == m
                        val label = if (m == 0) stringResource(R.string.pair_when_app_default_choice, Fmt.interval(cfg.globalIntervalMinutes)) else PairUi.intervalLabel(m)
                        Row(
                            Modifier.fillMaxWidth()
                                .selectable(selected = selected, role = Role.RadioButton, onClick = { onInterval(m) })
                                .heightIn(min = Size.touchTarget),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(Space.s12),
                        ) {
                            RadioButton(selected = selected, onClick = null)
                            Text(label, style = MaterialTheme.typography.bodyLarge, color = cs.onSurface)
                        }
                    }
                }
                Text(
                    stringResource(R.string.pair_when_android_may_delay),
                    style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant,
                    modifier = Modifier.padding(top = Space.s8),
                )
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.common_done)) } },
        containerColor = cs.surfaceContainerHigh,
        shape = MaterialTheme.shapes.extraLarge,
    )
}

// ------------------------------------------------------------------ skip

@Composable
internal fun SkipSection(state: EditorState, vm: PairEditorViewModel, nav: AppNav, isNew: Boolean) {
    val d = state.draft ?: return
    var help by rememberSaveable { mutableStateOf(false) }
    SectionHeader(stringResource(R.string.pair_skip_header))
    val patternHelp = stringResource(R.string.pair_skip_pattern_help)
    DsTextField(
        value = state.skipText,
        onValueChange = vm::setSkipText,
        label = stringResource(R.string.pair_skip_matching),
        modifier = Modifier.padding(horizontal = Space.s16),
        helper = stringResource(R.string.pair_skip_matching_helper),
        error = state.skipProblem,
        mono = true,
        leading = Sym.FilterAlt,
        trailing = { IconButton(onClick = { help = true }) { Icon(Sym.Help, contentDescription = patternHelp) } },
        singleLine = false,
    )
    FilterChipRow(
        listOf(
            ChipModel(label = stringResource(R.string.pair_skip_hidden_files), selected = !d.syncHidden, onClick = vm::toggleHidden),
            ChipModel(label = sizeChipLabel(d.maxFileSizeBytes), selected = d.maxFileSizeBytes > 0, onClick = vm::toggleSizeLimit),
            ChipModel(label = stringResource(R.string.pair_skip_videos), selected = state.videosSkipped, onClick = vm::toggleVideos),
        ),
        Modifier.padding(top = Space.s4),
    )
    if (!isNew) {
        SettingRow(
            title = stringResource(R.string.pair_skip_choose_subfolders),
            icon = Sym.Checklist,
            summary = subfolderSummary(d),
            control = SettingControl.Chevron,
            onClick = { nav.selective(d.id) },
        )
    }
    if (help) GlobHelpSheet(onDismiss = { help = false })
}

/** "Over 2 GB", or the custom limit when one is set. */
private fun sizeChipLabel(bytes: Long): String =
    if (bytes <= 0 || bytes == PairEditorViewModel.DEFAULT_SIZE_LIMIT) Strings.get(R.string.pair_skip_over_2gb)
    else Strings.get(R.string.pair_skip_over_size, Fmt.bytes(bytes).replace(".0 ", " "))

private fun subfolderSummary(d: FolderPairEntity): String = when (val n = d.excludedSubfolderList().size) {
    0 -> Strings.get(R.string.pair_skip_all_subfolders)
    else -> Strings.plural(R.plurals.pair_skip_subfolders_skipped, n, Fmt.count(n))
}

// ------------------------------------------------------------------ advanced

@Composable
internal fun AdvancedSection(state: EditorState, vm: PairEditorViewModel, nav: AppNav, isNew: Boolean, open: Boolean, onOpen: (Boolean) -> Unit) {
    val d = state.draft ?: return
    val cs = MaterialTheme.colorScheme
    val stateText = stringResource(if (open) R.string.pair_expanded else R.string.pair_collapsed)
    SectionHeader(
        stringResource(R.string.pair_adv_header),
        modifier = Modifier
            .testTag("editor_advanced")
            .clickable(onClickLabel = stringResource(if (open) R.string.pair_adv_hide else R.string.pair_adv_show)) { onOpen(!open) }
            .semantics { stateDescription = stateText }
            .heightIn(min = 48.dp),
        action = { ExpandArrow(open, cs.primary) },
    )
    if (!open) return

    MenuRow(
        title = stringResource(R.string.pair_adv_conflict_policy),
        value = conflictLabel(d.conflictPolicy),
        options = ConflictPolicy.entries,
        selected = d.conflictPolicy,
        optionLabel = ::conflictLabel,
        onSelect = { p -> vm.update { it.copy(conflictPolicy = p) } },
        icon = Sym.CallSplit,
    )
    val downloads = d.mode.downloads
    SettingRow(
        title = stringResource(R.string.pair_adv_docs),
        icon = Sym.Description,
        summary = when {
            !downloads -> stringResource(R.string.pair_adv_docs_no_downloads)
            d.docsExport -> stringResource(R.string.pair_adv_docs_on)
            else -> stringResource(R.string.pair_adv_docs_off)
        },
        control = SettingControl.Toggle(d.docsExport) { v -> vm.update { it.copy(docsExport = v) } },
        enabled = downloads,
    )
    MenuRow(
        title = stringResource(R.string.pair_adv_docs_as),
        value = docsLabel(d.docsFormat),
        options = DocsFormat.entries,
        selected = d.docsFormat,
        optionLabel = ::docsLabel,
        onSelect = { f -> vm.update { it.copy(docsFormat = f) } },
        icon = Sym.Article,
        enabled = downloads && d.docsExport,
    )
    SettingRow(
        title = stringResource(R.string.pair_adv_mtime),
        icon = Sym.History,
        summary = stringResource(if (d.preserveMtime) R.string.pair_adv_mtime_on else R.string.pair_adv_mtime_off),
        control = SettingControl.Toggle(d.preserveMtime) { v -> vm.update { it.copy(preserveMtime = v) } },
    )
    Column(
        Modifier.padding(horizontal = Space.s16, vertical = Space.s8),
        verticalArrangement = Arrangement.spacedBy(Space.s8),
    ) {
        DsTextField(
            value = state.includeText,
            onValueChange = vm::setIncludeText,
            label = stringResource(R.string.pair_adv_include),
            helper = stringResource(R.string.pair_adv_include_helper),
            error = state.includeProblem,
            mono = true,
            leading = Sym.Rule,
            singleLine = false,
        )
        DsTextField(
            value = state.sizeMbText,
            onValueChange = vm::setSizeMb,
            label = stringResource(R.string.pair_adv_size_limit),
            helper = stringResource(R.string.pair_adv_size_limit_helper),
            leading = Sym.DataUsage,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
        )
    }
    SettingRow(
        title = stringResource(R.string.pair_adv_encrypt),
        icon = Sym.Lock,
        summary = stringResource(if (d.encrypted) R.string.pair_adv_encrypt_on else R.string.pair_adv_encrypt_off),
        control = SettingControl.Toggle(d.encrypted) { v -> vm.update { it.copy(encrypted = v) } },
    )
    if (d.encrypted) {
        val vault = state.vaults.firstOrNull { it.id == d.vaultId }
        MenuRow(
            title = stringResource(R.string.pair_adv_vault),
            value = vault?.name ?: stringResource(if (state.vaults.isEmpty()) R.string.pair_adv_no_vaults else R.string.pair_adv_choose_vault),
            options = state.vaults,
            selected = vault,
            optionLabel = { it.name },
            onSelect = { v -> vm.update { it.copy(vaultId = v.id) } },
            icon = Sym.Key,
            extraLabel = stringResource(R.string.pair_adv_manage_vaults),
            onExtra = { nav.vaults() },
        )
        if (state.showErrors) state.vaultProblem?.let { RowCallout(it, CalloutTone.Danger) }
        RowCallout(
            stringResource(R.string.pair_adv_vault_empty_folder),
            CalloutTone.Warning,
            title = stringResource(R.string.pair_adv_vault_empty_folder_title),
        )
    }
    if (state.encryptionChanged && state.hasSynced) {
        RowCallout(stringResource(R.string.pair_adv_encryption_changed), CalloutTone.Info)
    }
    if (!isNew) {
        SettingRow(
            title = stringResource(R.string.common_remove_pair),
            modifier = Modifier.testTag("editor_remove"),
            icon = Sym.Delete,
            summary = stringResource(R.string.pair_adv_remove_summary),
            control = SettingControl.None,
            onClick = vm::removePair,
        )
    }
}

private fun conflictLabel(p: ConflictPolicy): String = when (p) {
    ConflictPolicy.KEEP_BOTH -> Strings.get(R.string.pair_conflict_keep_both)
    ConflictPolicy.NEWER_WINS -> Strings.get(R.string.pair_conflict_newer)
    ConflictPolicy.LOCAL_WINS -> Strings.get(R.string.pair_conflict_phone)
    ConflictPolicy.REMOTE_WINS -> Strings.get(R.string.pair_conflict_drive)
    ConflictPolicy.SKIP -> Strings.get(R.string.pair_conflict_skip)
}

private fun docsLabel(f: DocsFormat): String = when (f) {
    DocsFormat.OFFICE -> Strings.get(R.string.pair_docs_office)
    DocsFormat.PDF -> Strings.get(R.string.pair_docs_pdf)
}

// ------------------------------------------------------------------ shared rows

/** A setting whose value is picked from a menu: the summary shows the current value. */
@Composable
internal fun <T> MenuRow(
    title: String,
    value: String,
    options: List<T>,
    selected: T?,
    optionLabel: (T) -> String,
    onSelect: (T) -> Unit,
    icon: ImageVector? = null,
    enabled: Boolean = true,
    extraLabel: String? = null,
    onExtra: (() -> Unit)? = null,
) {
    var open by remember { mutableStateOf(false) }
    val selectedLabel = stringResource(R.string.pair_selected)
    Box(Modifier.fillMaxWidth()) {
        SettingRow(title = title, icon = icon, summary = value, control = SettingControl.Chevron, enabled = enabled, onClick = { open = true })
        DropdownMenu(expanded = open, onDismissRequest = { open = false }, offset = DpOffset(56.dp, 0.dp)) {
            options.forEach { option ->
                DropdownMenuItem(
                    text = { Text(optionLabel(option)) },
                    onClick = { open = false; onSelect(option) },
                    trailingIcon = if (option == selected) { { Icon(Sym.Check, contentDescription = selectedLabel) } } else null,
                )
            }
            if (extraLabel != null && onExtra != null) {
                DropdownMenuItem(
                    text = { Text(extraLabel) },
                    onClick = { open = false; onExtra() },
                    leadingIcon = { Icon(Sym.OpenInNew, contentDescription = null) },
                )
            }
        }
    }
}

/** A row that shows or hides the settings below it. */
@Composable
private fun ExpandRow(title: String, summary: String?, expanded: Boolean, onToggle: () -> Unit, icon: ImageVector? = null, tag: String) {
    val cs = MaterialTheme.colorScheme
    val stateText = stringResource(if (expanded) R.string.pair_expanded else R.string.pair_collapsed)
    Row(
        Modifier.fillMaxWidth().testTag(tag)
            .clickable(onClickLabel = stringResource(if (expanded) R.string.pair_hide_x else R.string.pair_show_x, title), onClick = onToggle)
            .semantics { stateDescription = stateText }
            .heightIn(min = 72.dp)
            .padding(horizontal = Space.s16, vertical = Space.s12),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.s16),
    ) {
        if (icon != null) Icon(icon, contentDescription = null, tint = cs.onSurfaceVariant)
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.bodyLarge, color = cs.onSurface)
            if (summary != null) Text(summary, style = MaterialTheme.typography.bodyMedium.tabular, color = cs.onSurfaceVariant)
        }
        ExpandArrow(expanded, cs.onSurfaceVariant)
    }
}

@Composable
private fun ExpandArrow(expanded: Boolean, tint: Color) {
    val angle by animateFloatAsState(
        targetValue = if (expanded) 180f else 0f,
        animationSpec = tween(durationMillis = if (Ds.reduceMotion) 0 else Motion.Short, easing = Motion.Standard),
        label = "expand",
    )
    Icon(Sym.KeyboardArrowDown, contentDescription = null, tint = tint, modifier = Modifier.rotate(angle))
}

/** A callout under a full-width setting row. */
@Composable
private fun RowCallout(text: String, tone: CalloutTone, title: String? = null, action: UiAction? = null) {
    Callout(text, Modifier.padding(start = Space.s16, end = Space.s16, bottom = Space.s8), tone = tone, title = title, action = action)
}

// ------------------------------------------------------------------ pattern help

@Composable
fun GlobHelpSheet(onDismiss: () -> Unit) {
    val cs = MaterialTheme.colorScheme
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        containerColor = cs.surfaceContainerHigh,
        shape = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp),
    ) {
        Column(
            Modifier.padding(horizontal = Space.s24).padding(bottom = Space.s32).verticalScroll(rememberScrollState()),
            verticalArrangement = Arrangement.spacedBy(Space.s12),
        ) {
            Text(stringResource(R.string.pair_glob_title), style = MaterialTheme.typography.titleLarge, color = cs.onSurface)
            Text(
                stringResource(R.string.pair_glob_intro),
                style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant,
            )
            HelpRow("?", stringResource(R.string.pair_glob_question))
            HelpRow("*", stringResource(R.string.pair_glob_star))
            HelpRow("**", stringResource(R.string.pair_glob_double_star))
            HelpRow("[abc]", stringResource(R.string.pair_glob_class))
            HelpRow("[!abc]", stringResource(R.string.pair_glob_negated_class))
            HelpRow("{2020..2026}", stringResource(R.string.pair_glob_range))
            HelpRow("{jpg,png}", stringResource(R.string.pair_glob_alternatives))
            HelpRow("/Camera/*", stringResource(R.string.pair_glob_leading_slash))
            HelpRow(".thumbnails/", stringResource(R.string.pair_glob_trailing_slash))
            Text(stringResource(R.string.pair_glob_examples), style = MaterialTheme.typography.titleSmall, color = cs.primary, modifier = Modifier.padding(top = Space.s8))
            HelpRow("*.tmp", stringResource(R.string.pair_glob_example_tmp))
            HelpRow("/Backups/**", stringResource(R.string.pair_glob_example_backups))
            HelpRow("~\$*", stringResource(R.string.pair_glob_example_office_lock))
            HelpRow("*.part", stringResource(R.string.pair_glob_example_part))
            Text(
                stringResource(R.string.pair_glob_footer),
                style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant,
            )
        }
    }
}

@Composable
private fun HelpRow(token: String, meaning: String) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Space.s12), verticalAlignment = Alignment.Top) {
        Text(token, style = MonoMedium, color = MaterialTheme.colorScheme.onSurface, modifier = Modifier.padding(top = 1.dp))
        Text(meaning, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.weight(1f))
    }
}
