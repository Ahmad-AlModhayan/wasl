package com.almodhayan.drivesync.ui.settings

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.annotation.RequiresApi
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
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.data.db.ConflictPolicy
import com.almodhayan.drivesync.data.db.DocsFormat
import com.almodhayan.drivesync.data.prefs.AppSettings
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.common.AppScreen
import com.almodhayan.drivesync.ui.common.LoadingState
import com.almodhayan.drivesync.ui.components.DsDialog
import com.almodhayan.drivesync.ui.components.DsSegmentedButton
import com.almodhayan.drivesync.ui.components.DsTextField
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.GroupItem
import com.almodhayan.drivesync.ui.components.GroupPos
import com.almodhayan.drivesync.ui.components.SectionHeader
import com.almodhayan.drivesync.ui.components.SettingControl
import com.almodhayan.drivesync.ui.components.SettingRow
import com.almodhayan.drivesync.ui.components.Side
import com.almodhayan.drivesync.ui.components.StatusIcon
import com.almodhayan.drivesync.ui.components.StorageBar
import com.almodhayan.drivesync.ui.components.StorageModel
import com.almodhayan.drivesync.ui.components.showMessage
import com.almodhayan.drivesync.ui.components.showUndo
import com.almodhayan.drivesync.ui.theme.Ds
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Size
import com.almodhayan.drivesync.ui.theme.Sym
import com.almodhayan.drivesync.ui.theme.SyncStatus
import kotlinx.coroutines.launch

private val intervalOptions = listOf(15, 30, 60, 120, 240, 360, 720, 1440)
private val pollOptions = listOf(1, 2, 5, 10, 15, 30, 60)
private val batteryOptions = listOf(0, 10, 15, 20, 30, 50)
private val concurrencyOptions = listOf(1, 2, 3, 4, 5, 6)
private val bandwidthOptions = listOf(0, 256, 512, 1024, 2048, 5120, 10240, 20480)
private val binOptions = listOf(7, 14, 30, 60, 90)
private val versionOptions = listOf(1, 3, 5, 10, 20)
private val historyOptions = listOf(7, 30, 90, 180, 365)
private val cacheOptions = listOf(128, 256, 512, 1024, 2048, 4096, 8192)
private val themeOptions = listOf("system" to R.string.settings_theme_system, "light" to R.string.settings_theme_light, "dark" to R.string.settings_theme_dark)

private sealed interface SettingsDialog {
    data object IntervalPick : SettingsDialog
    data object PollPick : SettingsDialog
    data object BatteryPick : SettingsDialog
    data object ConcurrencyPick : SettingsDialog
    data object BandwidthPick : SettingsDialog
    data object DeleteLimitEdit : SettingsDialog
    data object ConflictPick : SettingsDialog
    data object BinPick : SettingsDialog
    data object VersionsPick : SettingsDialog
    data object HistoryPick : SettingsDialog
    data object NameEdit : SettingsDialog
    data object DocsPick : SettingsDialog
    data object CachePick : SettingsDialog
    data object AutomationInfo : SettingsDialog
    data object PrivacyInfo : SettingsDialog
    data object LicencesInfo : SettingsDialog
    data object ChooseAccount : SettingsDialog
    data class ConfirmDisconnect(val account: AccountSummary) : SettingsDialog
}

/**
 * Settings (top level; the bottom navigation bar belongs to MainActivity). Account card, then the
 * defaults for new pairs, the safety rules, appearance, Google Docs, notifications, storage and
 * backup, about, and the one red row: Disconnect Google account.
 */
@Composable
fun SettingsScreen(nav: AppNav, vm: SettingsViewModel = hiltViewModel()) {
    val settings by vm.settings.collectAsStateWithLifecycle()
    val accounts by vm.accounts.collectAsStateWithLifecycle()
    val pairCount by vm.pairCount.collectAsStateWithLifecycle()
    val allPaused by vm.allPaused.collectAsStateWithLifecycle()
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    val scrollBehavior = TopAppBarDefaults.pinnedScrollBehavior()
    var dialog by remember { mutableStateOf<SettingsDialog?>(null) }

    LaunchedEffect(vm) {
        vm.messages.collect { m ->
            scope.launch {
                val undo = m.undo
                if (undo != null) snackbar.showUndo(m.text, undo) else snackbar.showMessage(m.text)
            }
        }
    }

    AppScreen(
        title = stringResource(R.string.common_settings),
        onBack = null,
        snackbarHost = { SnackbarHost(snackbar) },
        scrollBehavior = scrollBehavior,
    ) { padding ->
        val s = settings
        if (s == null) {
            Box(Modifier.fillMaxSize().padding(padding)) { LoadingState() }
        } else {
            SettingsList(
                s = s,
                accounts = accounts,
                pairCount = pairCount,
                allPaused = allPaused,
                padding = padding,
                vm = vm,
                nav = nav,
                open = { dialog = it },
            )
        }
    }

    val s = settings ?: return
    val close = { dialog = null }
    when (val d = dialog) {
        null -> Unit
        SettingsDialog.IntervalPick -> ChoiceDialog(
            title = stringResource(R.string.common_check_for_changes),
            intro = stringResource(R.string.settings_interval_intro),
            choices = withCurrent(intervalOptions, s.globalIntervalMinutes).map { Choice(it, intervalText(it)) },
            selected = s.globalIntervalMinutes, onSelect = vm::setInterval, onDismiss = close,
        )
        SettingsDialog.PollPick -> ChoiceDialog(
            title = stringResource(R.string.settings_poll_title),
            intro = stringResource(R.string.settings_poll_intro),
            choices = withCurrent(pollOptions, s.remotePollMinutes).map { Choice(it, intervalText(it)) },
            selected = s.remotePollMinutes, onSelect = { m -> vm.update { it.copy(remotePollMinutes = m) } }, onDismiss = close,
        )
        SettingsDialog.BatteryPick -> ChoiceDialog(
            title = stringResource(R.string.common_minimum_battery),
            intro = stringResource(R.string.settings_battery_intro),
            choices = withCurrent(batteryOptions, s.minBatteryDefault).map { Choice(it, batteryText(it)) },
            selected = s.minBatteryDefault, onSelect = { v -> vm.update { it.copy(minBatteryDefault = v) } }, onDismiss = close,
        )
        SettingsDialog.ConcurrencyPick -> ChoiceDialog(
            title = stringResource(R.string.settings_concurrency_title),
            intro = stringResource(R.string.settings_concurrency_intro),
            choices = withCurrent(concurrencyOptions, s.concurrency).map { Choice(it, concurrencyText(it), recommended = it == 3) },
            selected = s.concurrency, onSelect = { v -> vm.update { it.copy(concurrency = v) } }, onDismiss = close,
        )
        SettingsDialog.BandwidthPick -> ChoiceDialog(
            title = stringResource(R.string.settings_bandwidth_title),
            intro = stringResource(R.string.settings_bandwidth_intro),
            choices = withCurrent(bandwidthOptions, s.bandwidthLimitKBps).map { Choice(it, bandwidthText(it)) },
            selected = s.bandwidthLimitKBps, onSelect = { v -> vm.update { it.copy(bandwidthLimitKBps = v) } }, onDismiss = close,
        )
        SettingsDialog.DeleteLimitEdit -> DeleteLimitDialog(
            files = s.massDeleteMinFiles, percent = s.massDeleteThresholdPercent,
            onSave = { f, p -> vm.update { it.copy(massDeleteMinFiles = f, massDeleteThresholdPercent = p) } }, onDismiss = close,
        )
        SettingsDialog.ConflictPick -> ChoiceDialog(
            title = stringResource(R.string.settings_conflict_title),
            intro = stringResource(R.string.settings_conflict_intro),
            choices = ConflictPolicy.entries.map { Choice(it, conflictTitle(it), conflictDescription(it), recommended = it == ConflictPolicy.KEEP_BOTH) },
            selected = s.conflictPolicyDefault, onSelect = { p -> vm.update { it.copy(conflictPolicyDefault = p) } }, onDismiss = close,
        )
        SettingsDialog.BinPick -> ChoiceDialog(
            title = stringResource(R.string.settings_bin_title),
            intro = stringResource(R.string.settings_bin_intro),
            choices = withCurrent(binOptions, s.trashRetentionDays).map { Choice(it, daysText(it), recommended = it == 30) },
            selected = s.trashRetentionDays, onSelect = { v -> vm.update { it.copy(trashRetentionDays = v) } }, onDismiss = close,
        )
        SettingsDialog.VersionsPick -> ChoiceDialog(
            title = stringResource(R.string.settings_versions_title),
            intro = stringResource(R.string.settings_versions_intro),
            choices = withCurrent(versionOptions, s.localVersionsKept).map { Choice(it, versionsText(it), recommended = it == 5) },
            selected = s.localVersionsKept, onSelect = { v -> vm.update { it.copy(localVersionsKept = v) } }, onDismiss = close,
        )
        SettingsDialog.HistoryPick -> ChoiceDialog(
            title = stringResource(R.string.common_sync_history),
            intro = stringResource(R.string.settings_history_intro),
            choices = withCurrent(historyOptions, s.historyRetentionDays).map { Choice(it, daysText(it)) },
            selected = s.historyRetentionDays, onSelect = { v -> vm.update { it.copy(historyRetentionDays = v) } }, onDismiss = close,
        )
        SettingsDialog.NameEdit -> NameDialog(current = s.deviceName, onSave = { n -> vm.update { it.copy(deviceName = n) } }, onDismiss = close)
        SettingsDialog.DocsPick -> ChoiceDialog(
            title = stringResource(R.string.settings_docs_title),
            intro = stringResource(R.string.settings_docs_intro),
            choices = DocsFormat.entries.map { Choice(it, docsTitle(it), docsDescription(it), recommended = it == DocsFormat.OFFICE) },
            selected = s.docsFormatDefault, onSelect = { f -> vm.update { it.copy(docsFormatDefault = f) } }, onDismiss = close,
        )
        SettingsDialog.CachePick -> ChoiceDialog(
            title = stringResource(R.string.settings_cache_title),
            intro = stringResource(R.string.settings_cache_intro),
            choices = withCurrent(cacheOptions, s.onDemandCacheMb).map { Choice(it, cacheText(it)) },
            selected = s.onDemandCacheMb, onSelect = { v -> vm.update { it.copy(onDemandCacheMb = v) } }, onDismiss = close,
        )
        SettingsDialog.AutomationInfo -> AutomationDialog(onDismiss = close)
        SettingsDialog.PrivacyInfo -> PrivacyDialog(onDismiss = close)
        SettingsDialog.LicencesInfo -> LicencesDialog(onDismiss = close)
        SettingsDialog.ChooseAccount -> ContentDialog(title = stringResource(R.string.settings_choose_account_title), onDismiss = close) {
            accounts.forEach { a ->
                AccountOption(a, onClick = { dialog = SettingsDialog.ConfirmDisconnect(a) })
            }
        }
        is SettingsDialog.ConfirmDisconnect -> DsDialog(
            title = stringResource(R.string.settings_disconnect_title, d.account.email),
            body = disconnectBody(d.account.pairCount),
            confirmLabel = stringResource(R.string.settings_disconnect),
            onConfirm = { vm.disconnect(d.account.email); dialog = null },
            onDismiss = close,
            icon = Sym.Logout,
            danger = true,
        )
    }
}

@Composable
private fun SettingsList(
    s: AppSettings,
    accounts: List<AccountSummary>,
    pairCount: Int,
    allPaused: Boolean,
    padding: PaddingValues,
    vm: SettingsViewModel,
    nav: AppNav,
    open: (SettingsDialog) -> Unit,
) {
    val cs = MaterialTheme.colorScheme
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(top = padding.calculateTopPadding() + Space.s4, bottom = padding.calculateBottomPadding() + Space.s24)) {
        item(key = "account") {
            AccountCard(accounts, onManage = { nav.accounts() }, modifier = Modifier.padding(horizontal = Space.s16).padding(bottom = Space.s4))
        }

        // ---------------------------------------------------------------- sync defaults
        item(key = "h-sync") { SectionHeader(stringResource(R.string.settings_section_sync_defaults)) }
        item(key = "pause-all") { GroupItem(GroupPos.First) {
            SettingRow(
                title = stringResource(R.string.settings_pause_all), icon = Sym.Pause,
                summary = when {
                    pairCount == 0 -> stringResource(R.string.settings_no_pairs_yet)
                    allPaused -> stringResource(R.string.settings_pause_all_on)
                    else -> null
                },
                control = SettingControl.Toggle(allPaused) { vm.setAllPaused(it) },
                enabled = pairCount > 0,
            )
        } }
        item(key = "wifi") { GroupItem(GroupPos.Middle) {
            SettingRow(
                title = stringResource(R.string.settings_wifi_only), icon = Sym.Wifi,
                summary = if (s.wifiOnlyDefault) stringResource(R.string.settings_for_new_pairs) else stringResource(R.string.settings_wifi_only_off),
                control = SettingControl.Toggle(s.wifiOnlyDefault) { v -> vm.update { it.copy(wifiOnlyDefault = v) } },
            )
        } }
        item(key = "interval") { GroupItem(GroupPos.Middle) {
            SettingRow(
                title = stringResource(R.string.common_check_for_changes), icon = Sym.Schedule,
                summary = stringResource(R.string.settings_interval_summary),
                control = SettingControl.Value(intervalText(s.globalIntervalMinutes)),
                onClick = { open(SettingsDialog.IntervalPick) },
            )
        } }
        item(key = "instant") { GroupItem(GroupPos.Middle) {
            SettingRow(
                title = stringResource(R.string.settings_instant), icon = Sym.Update,
                summary = if (s.instantMonitoring) stringResource(R.string.settings_instant_on) else stringResource(R.string.settings_instant_off),
                control = SettingControl.Toggle(s.instantMonitoring) { vm.setInstantMonitoring(it) },
            )
        } }
        item(key = "poll") { GroupItem(GroupPos.Middle) {
            SettingRow(
                title = stringResource(R.string.settings_poll_title), icon = Sym.Cloud,
                summary = if (s.instantMonitoring) stringResource(R.string.settings_poll_summary) else stringResource(R.string.settings_poll_needs_instant),
                control = SettingControl.Value(intervalText(s.remotePollMinutes)),
                enabled = s.instantMonitoring,
                onClick = { open(SettingsDialog.PollPick) },
            )
        } }
        item(key = "charging") { GroupItem(GroupPos.Middle) {
            SettingRow(
                title = stringResource(R.string.settings_charging), icon = Sym.Bolt,
                summary = if (s.requireChargingDefault) stringResource(R.string.settings_for_new_pairs) else stringResource(R.string.settings_charging_off),
                control = SettingControl.Toggle(s.requireChargingDefault) { v -> vm.update { it.copy(requireChargingDefault = v) } },
            )
        } }
        item(key = "battery") { GroupItem(GroupPos.Middle) {
            SettingRow(
                title = stringResource(R.string.common_minimum_battery), icon = Sym.BatteryAlert,
                summary = stringResource(R.string.settings_battery_summary),
                control = SettingControl.Value(batteryText(s.minBatteryDefault)),
                onClick = { open(SettingsDialog.BatteryPick) },
            )
        } }
        item(key = "concurrency") { GroupItem(GroupPos.Middle) {
            SettingRow(
                title = stringResource(R.string.settings_concurrency_title), icon = Sym.SwapVert,
                summary = stringResource(R.string.settings_concurrency_summary),
                control = SettingControl.Value(concurrencyText(s.concurrency)),
                onClick = { open(SettingsDialog.ConcurrencyPick) },
            )
        } }
        item(key = "bandwidth") { GroupItem(GroupPos.Last) {
            SettingRow(
                title = stringResource(R.string.settings_bandwidth_title), icon = Sym.DataUsage,
                summary = stringResource(R.string.settings_bandwidth_summary),
                control = SettingControl.Value(bandwidthText(s.bandwidthLimitKBps)),
                onClick = { open(SettingsDialog.BandwidthPick) },
            )
        } }

        // ---------------------------------------------------------------- devices (FR-N12)
        item(key = "h-devices") { SectionHeader(stringResource(R.string.settings_section_devices)) }
        item(key = "devices") { GroupItem(GroupPos.First) {
            SettingRow(
                title = stringResource(R.string.devices_title), icon = Sym.Dns,
                summary = stringResource(R.string.settings_devices_summary),
                onClick = { nav.devices() },
                modifier = Modifier.testTag("settings-devices"),
            )
        } }
        item(key = "devices-charging") { GroupItem(GroupPos.Last) {
            SettingRow(
                title = stringResource(R.string.devices_charging), icon = Sym.Bolt,
                summary = stringResource(if (s.devicesWhileCharging) R.string.devices_charging_on else R.string.devices_charging_off),
                control = SettingControl.Toggle(s.devicesWhileCharging) { v -> vm.update { it.copy(devicesWhileCharging = v) } },
            )
        } }

        // ---------------------------------------------------------------- safety
        item(key = "h-safety") { SectionHeader(stringResource(R.string.settings_section_safety)) }
        item(key = "guard") { GroupItem(GroupPos.First) {
            SettingRow(
                title = stringResource(R.string.settings_guard), icon = Sym.FrontHand,
                summary = if (s.massDeleteGuardDefault) deleteLimitText(s) else stringResource(R.string.settings_guard_off),
                control = SettingControl.Toggle(s.massDeleteGuardDefault) { v -> vm.update { it.copy(massDeleteGuardDefault = v) } },
            )
        } }
        item(key = "guard-limit") { GroupItem(GroupPos.Middle) {
            SettingRow(
                title = stringResource(R.string.settings_guard_limit), icon = Sym.Rule,
                summary = stringResource(R.string.settings_guard_limit_summary),
                control = SettingControl.Value(pluralStringResource(R.plurals.settings_guard_limit_value, s.massDeleteMinFiles, Fmt.count(s.massDeleteMinFiles), s.massDeleteThresholdPercent)),
                onClick = { open(SettingsDialog.DeleteLimitEdit) },
            )
        } }
        item(key = "conflicts") { GroupItem(GroupPos.Middle) {
            SettingRow(
                title = stringResource(R.string.settings_conflict_title), icon = Sym.CallSplit,
                summary = conflictSummary(s.conflictPolicyDefault),
                control = SettingControl.Chevron,
                onClick = { open(SettingsDialog.ConflictPick) },
            )
        } }
        item(key = "bin") { GroupItem(GroupPos.Middle) {
            SettingRow(
                title = stringResource(R.string.settings_bin_title), icon = Sym.AutoDelete,
                summary = if (s.trashRetentionDays < 30) stringResource(R.string.settings_bin_short) else stringResource(R.string.settings_bin_summary),
                control = SettingControl.Value(daysText(s.trashRetentionDays)),
                onClick = { open(SettingsDialog.BinPick) },
            )
        } }
        item(key = "versions") { GroupItem(GroupPos.Middle) {
            SettingRow(
                title = stringResource(R.string.settings_versions_title), icon = Sym.History,
                summary = stringResource(R.string.settings_versions_summary),
                control = SettingControl.Value(versionsText(s.localVersionsKept)),
                onClick = { open(SettingsDialog.VersionsPick) },
            )
        } }
        item(key = "history") { GroupItem(GroupPos.Last) {
            SettingRow(
                title = stringResource(R.string.common_sync_history), icon = Sym.List,
                summary = stringResource(R.string.settings_history_summary),
                control = SettingControl.Value(daysText(s.historyRetentionDays)),
                onClick = { open(SettingsDialog.HistoryPick) },
            )
        } }

        // ---------------------------------------------------------------- appearance
        item(key = "h-appearance") { SectionHeader(stringResource(R.string.settings_section_appearance)) }
        item(key = "dynamic") { GroupItem(GroupPos.First) {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                SettingRow(
                    title = stringResource(R.string.settings_dynamic_color), icon = Sym.Palette,
                    summary = if (s.dynamicColor) stringResource(R.string.settings_dynamic_color_on) else stringResource(R.string.settings_dynamic_color_off),
                    control = SettingControl.Toggle(s.dynamicColor) { v -> vm.update { it.copy(dynamicColor = v) } },
                )
            } else {
                SettingRow(title = stringResource(R.string.settings_dynamic_color), icon = Sym.Palette, summary = stringResource(R.string.settings_dynamic_color_needs), control = SettingControl.None, enabled = false)
            }
        } }
        item(key = "theme") { GroupItem(GroupPos.Middle) {
            Column(Modifier.fillMaxWidth().padding(horizontal = Space.s16, vertical = Space.s12)) {
                Row(Modifier.heightIn(min = 24.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s16)) {
                    Icon(Sym.DarkMode, contentDescription = null, tint = cs.onSurfaceVariant)
                    Text(stringResource(R.string.settings_theme), style = MaterialTheme.typography.bodyLarge, color = cs.onSurface)
                }
                Spacer(Modifier.height(Space.s12))
                DsSegmentedButton(
                    items = themeOptions.map { stringResource(it.second) },
                    selectedIndex = themeOptions.indexOfFirst { it.first == s.theme }.coerceAtLeast(0),
                    onSelect = { i -> vm.update { it.copy(theme = themeOptions[i].first) } },
                    modifier = Modifier.padding(start = Size.icon + Space.s16),
                )
            }
        } }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            item(key = "language") { GroupItem(GroupPos.Middle) {
                val context = LocalContext.current
                val locale = LocalConfiguration.current.locales[0]
                SettingRow(
                    title = stringResource(R.string.settings_language), icon = Sym.TravelExplore,
                    summary = stringResource(R.string.settings_language_summary),
                    control = SettingControl.Value(locale.getDisplayLanguage(locale).replaceFirstChar { it.titlecase(locale) }),
                    onClick = { openAppLanguageSettings(context) },
                )
            } }
        }
        item(key = "device-name") { GroupItem(GroupPos.Last) {
            SettingRow(
                title = stringResource(R.string.settings_phone_name), icon = Sym.Mobile,
                summary = stringResource(R.string.settings_phone_name_summary),
                control = SettingControl.Value(s.deviceName.ellipsize(18)),
                onClick = { open(SettingsDialog.NameEdit) },
            )
        } }

        // ---------------------------------------------------------------- google docs
        item(key = "h-docs") { SectionHeader(stringResource(R.string.settings_section_docs)) }
        item(key = "docs") { GroupItem(GroupPos.Single) {
            SettingRow(
                title = stringResource(R.string.settings_docs_export), icon = Sym.Description,
                summary = stringResource(R.string.settings_docs_export_summary),
                control = SettingControl.Value(docsShort(s.docsFormatDefault)),
                onClick = { open(SettingsDialog.DocsPick) },
            )
        } }

        // ---------------------------------------------------------------- notifications
        item(key = "h-notifications") { SectionHeader(stringResource(R.string.settings_section_notifications)) }
        item(key = "n-progress") { GroupItem(GroupPos.First) {
            SettingRow(
                title = stringResource(R.string.settings_notify_progress), icon = Sym.Notifications,
                summary = if (s.notifyProgress) stringResource(R.string.settings_notify_progress_on) else stringResource(R.string.settings_notify_progress_off),
                control = SettingControl.Toggle(s.notifyProgress) { v -> vm.update { it.copy(notifyProgress = v) } },
            )
        } }
        item(key = "n-success") { GroupItem(GroupPos.Middle) {
            SettingRow(
                title = stringResource(R.string.settings_notify_success), icon = Sym.TaskAlt,
                summary = if (s.notifyOnSuccess) stringResource(R.string.settings_notify_success_on) else stringResource(R.string.settings_off),
                control = SettingControl.Toggle(s.notifyOnSuccess) { v -> vm.update { it.copy(notifyOnSuccess = v) } },
            )
        } }
        item(key = "n-problems") { GroupItem(GroupPos.Last) {
            SettingRow(
                title = stringResource(R.string.settings_notify_problems), icon = Sym.Report,
                summary = if (s.notifyOnError) stringResource(R.string.settings_notify_problems_on) else stringResource(R.string.settings_notify_problems_off),
                control = SettingControl.Toggle(s.notifyOnError) { v -> vm.update { it.copy(notifyOnError = v) } },
            )
        } }

        // ---------------------------------------------------------------- storage & backup
        item(key = "h-storage") { SectionHeader(stringResource(R.string.settings_section_storage)) }
        item(key = "auto-backup") { GroupItem(GroupPos.First) {
            SettingRow(
                title = stringResource(R.string.settings_auto_backup), icon = Sym.CloudUpload,
                summary = if (s.autoBackupConfig) stringResource(R.string.settings_auto_backup_on) else stringResource(R.string.settings_auto_backup_off),
                control = SettingControl.Toggle(s.autoBackupConfig) { v -> vm.update { it.copy(autoBackupConfig = v) } },
            )
        } }
        item(key = "backup") { GroupItem(GroupPos.Middle) {
            SettingRow(
                title = stringResource(R.string.settings_backup), icon = Sym.Save,
                summary = s.lastBackupAt.takeIf { it > 0 }?.let { stringResource(R.string.settings_backup_last, Fmt.relativeLower(it)) } ?: stringResource(R.string.settings_backup_summary),
                onClick = { nav.backup() },
            )
        } }
        item(key = "cache") { GroupItem(GroupPos.Middle) {
            SettingRow(
                title = stringResource(R.string.settings_cache_title), icon = Sym.Storage,
                summary = stringResource(R.string.settings_cache_summary),
                control = SettingControl.Value(cacheText(s.onDemandCacheMb)),
                onClick = { open(SettingsDialog.CachePick) },
            )
        } }
        item(key = "vaults") { GroupItem(GroupPos.Middle) {
            SettingRow(title = stringResource(R.string.settings_vaults), icon = Sym.Lock, summary = stringResource(R.string.settings_vaults_summary), onClick = { nav.vaults() })
        } }
        item(key = "browse") { GroupItem(GroupPos.Last) {
            val first = accounts.firstOrNull()
            SettingRow(
                title = stringResource(R.string.settings_browse_drive), icon = Sym.FolderOpen,
                summary = if (first == null) stringResource(R.string.settings_browse_drive_needs_account) else stringResource(R.string.settings_browse_drive_summary, first.email),
                enabled = first != null,
                onClick = { if (first != null) nav.driveBrowser(first.email) },
            )
        } }

        // ---------------------------------------------------------------- about
        item(key = "h-about") { SectionHeader(stringResource(R.string.settings_section_about)) }
        item(key = "history-runs") { GroupItem(GroupPos.First) {
            SettingRow(title = stringResource(R.string.common_sync_history), icon = Sym.History, summary = stringResource(R.string.settings_sync_history_summary), onClick = { nav.history() })
        } }
        item(key = "diagnostics") { GroupItem(GroupPos.Middle) {
            SettingRow(title = stringResource(R.string.common_diagnostics), icon = Sym.MonitorHeart, summary = stringResource(R.string.settings_diagnostics_summary), onClick = { nav.diagnostics() })
        } }
        item(key = "automation") { GroupItem(GroupPos.Middle) {
            SettingRow(
                title = stringResource(R.string.common_automation), icon = Sym.Terminal, modifier = Modifier.testTag("settings_automation"),
                summary = if (s.automationEnabled) stringResource(R.string.settings_automation_on) else stringResource(R.string.settings_automation_off),
                onClick = { open(SettingsDialog.AutomationInfo) },
            )
        } }
        item(key = "privacy") { GroupItem(GroupPos.Middle) {
            SettingRow(title = stringResource(R.string.settings_privacy), icon = Sym.Shield, summary = stringResource(R.string.settings_privacy_summary), onClick = { open(SettingsDialog.PrivacyInfo) })
        } }
        item(key = "licences") { GroupItem(GroupPos.Middle) {
            SettingRow(title = stringResource(R.string.settings_licences), icon = Sym.Article, summary = stringResource(R.string.settings_licences_summary), onClick = { open(SettingsDialog.LicencesInfo) })
        } }
        item(key = "about") { GroupItem(GroupPos.Last) {
            SettingRow(title = stringResource(R.string.settings_about_app), icon = Sym.Info, onClick = { nav.about() })
        } }

        // ---------------------------------------------------------------- the one red row
        if (accounts.isNotEmpty()) {
            item(key = "gap-disconnect") { Spacer(Modifier.height(Space.s24)) }
            item(key = "disconnect") { GroupItem(GroupPos.Single) {
                SettingRow(
                    title = stringResource(R.string.settings_disconnect_account), icon = Sym.Logout,
                    summary = if (accounts.size == 1) accounts.first().email else pluralStringResource(R.plurals.settings_accounts_count, accounts.size, accounts.size),
                    control = SettingControl.None,
                    danger = true,
                    onClick = { open(if (accounts.size == 1) SettingsDialog.ConfirmDisconnect(accounts.first()) else SettingsDialog.ChooseAccount) },
                )
            } }
        }
    }
}

// -------------------------------------------------------------------------------------- account card

@Composable
private fun AccountCard(accounts: List<AccountSummary>, onManage: () -> Unit, modifier: Modifier = Modifier) {
    val cs = MaterialTheme.colorScheme
    val shape = MaterialTheme.shapes.large
    Column(
        modifier.fillMaxWidth().clip(shape).background(Ds.lab.card).padding(Space.s16),
        verticalArrangement = Arrangement.spacedBy(Space.s12),
    ) {
        if (accounts.isEmpty()) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s12)) {
                Box(Modifier.size(40.dp).clip(CircleShape).background(cs.tertiaryContainer), contentAlignment = Alignment.Center) {
                    Icon(Sym.AccountCircle, contentDescription = null, tint = cs.onTertiaryContainer)
                }
                Column(Modifier.weight(1f)) {
                    Text(stringResource(R.string.settings_no_account), style = MaterialTheme.typography.titleMedium, color = cs.onSurface)
                    Text(stringResource(R.string.settings_no_account_summary), style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant)
                }
            }
            OutlinedButton(onClick = onManage, contentPadding = PaddingValues(horizontal = Space.s16)) { Text(stringResource(R.string.settings_add_account)) }
        } else {
            accounts.forEachIndexed { i, a ->
                if (i > 0) HorizontalDivider(color = cs.outlineVariant)
                AccountBlock(a)
            }
            OutlinedButton(onClick = onManage, contentPadding = PaddingValues(horizontal = Space.s16)) { Text(stringResource(R.string.settings_manage_accounts)) }
        }
    }
}

@Composable
private fun AccountBlock(a: AccountSummary) {
    val cs = MaterialTheme.colorScheme
    Column(verticalArrangement = Arrangement.spacedBy(Space.s12)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s12)) {
            Avatar(a.initial)
            Column(Modifier.weight(1f)) {
                Text(a.name ?: stringResource(R.string.common_google_account), style = MaterialTheme.typography.titleMedium, color = cs.onSurface, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(a.email, style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        if (a.needsSignIn) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s8)) {
                StatusIcon(SyncStatus.Failed, size = 16.dp)
                Text(stringResource(R.string.settings_sign_in_again), style = MaterialTheme.typography.bodySmall, color = Ds.status.failed.content)
            }
        }
        val limit = a.storageLimit
        val used = a.storageUsed
        when {
            limit != null && limit > 0 && used != null -> {
                val pair = a.pairBytes.coerceIn(0L, used.coerceAtLeast(0L))
                StorageBar(StorageModel(Side.Drive, total = limit, pair = pair, other = (used - pair).coerceAtLeast(0L), label = stringResource(R.string.common_drive_storage), pairLabel = stringResource(R.string.common_your_pairs)), compact = true)
            }
            used != null -> Text(stringResource(R.string.settings_storage_no_limit, Fmt.bytes(used)), style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant)
            else -> Text(stringResource(R.string.settings_storage_not_checked), style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant)
        }
    }
}

@Composable
private fun Avatar(initial: String) {
    val cs = MaterialTheme.colorScheme
    Box(Modifier.size(40.dp).clip(CircleShape).background(cs.tertiaryContainer), contentAlignment = Alignment.Center) {
        Text(initial, style = MaterialTheme.typography.titleMedium, color = cs.onTertiaryContainer)
    }
}

/** A tappable account row inside the "Disconnect which account?" chooser. */
@Composable
private fun AccountOption(a: AccountSummary, onClick: () -> Unit) {
    val cs = MaterialTheme.colorScheme
    Row(
        Modifier.fillMaxWidth().clip(MaterialTheme.shapes.small).clickable(role = Role.Button, onClick = onClick).heightIn(min = 56.dp).padding(vertical = Space.s8),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.s12),
    ) {
        Avatar(a.initial)
        Column(Modifier.weight(1f)) {
            Text(a.email, style = MaterialTheme.typography.bodyLarge, color = cs.onSurface, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(if (a.pairCount == 0) stringResource(R.string.settings_no_pairs) else pluralStringResource(R.plurals.settings_pairs, a.pairCount, a.pairCount), style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant)
        }
    }
}

// -------------------------------------------------------------------------------------- edit dialogs

@Composable
private fun DeleteLimitDialog(files: Int, percent: Int, onSave: (Int, Int) -> Unit, onDismiss: () -> Unit) {
    var f by remember { mutableStateOf(files.toString()) }
    var p by remember { mutableStateOf(percent.toString()) }
    val fv = f.toIntOrNull()?.takeIf { it in 1..100_000 }
    val pv = p.toIntOrNull()?.takeIf { it in 1..100 }
    ContentDialog(
        title = stringResource(R.string.settings_guard_limit),
        icon = Sym.FrontHand,
        onDismiss = onDismiss,
        confirmLabel = stringResource(R.string.settings_guard_save),
        confirmEnabled = fv != null && pv != null,
        onConfirm = { if (fv != null && pv != null) { onSave(fv, pv); onDismiss() } },
    ) {
        Text(
            stringResource(R.string.settings_guard_body),
            style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        DsTextField(
            value = f, onValueChange = { f = it.filter(Char::isDigit).take(6) }, label = stringResource(R.string.settings_guard_files),
            error = if (fv == null) stringResource(R.string.settings_guard_files_error) else null,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number, imeAction = ImeAction.Next),
        )
        DsTextField(
            value = p, onValueChange = { p = it.filter(Char::isDigit).take(3) }, label = stringResource(R.string.settings_guard_percent),
            error = if (pv == null) stringResource(R.string.settings_guard_percent_error) else null,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number, imeAction = ImeAction.Done),
        )
    }
}

@Composable
private fun NameDialog(current: String, onSave: (String) -> Unit, onDismiss: () -> Unit) {
    var name by remember { mutableStateOf(current) }
    val clean = name.trim()
    val fallbackName = stringResource(R.string.settings_phone_name_fallback)
    ContentDialog(
        title = stringResource(R.string.settings_phone_name),
        icon = Sym.Mobile,
        onDismiss = onDismiss,
        confirmLabel = stringResource(R.string.settings_phone_name_save),
        confirmEnabled = clean.isNotEmpty(),
        onConfirm = { if (clean.isNotEmpty()) { onSave(clean); onDismiss() } },
    ) {
        Text(
            stringResource(R.string.settings_phone_name_body, clean.ifEmpty { fallbackName }),
            style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        DsTextField(
            value = name, onValueChange = { name = it.take(60) }, label = stringResource(R.string.settings_phone_name),
            error = if (clean.isEmpty()) stringResource(R.string.settings_phone_name_error) else null,
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
        )
    }
}

// -------------------------------------------------------------------------------------- words

private fun intervalText(minutes: Int): String = Fmt.interval(minutes).replaceFirstChar { it.uppercase() }

@Composable
private fun batteryText(percent: Int): String =
    if (percent <= 0) stringResource(R.string.settings_battery_any) else stringResource(R.string.settings_battery_percent, percent)

@Composable
private fun concurrencyText(n: Int): String = stringResource(R.string.settings_concurrency_value, n)

@Composable
private fun daysText(n: Int): String = pluralStringResource(R.plurals.settings_days, n, n)

@Composable
private fun versionsText(n: Int): String = pluralStringResource(R.plurals.settings_versions, n, n)

@Composable
private fun bandwidthText(kbps: Int): String = when {
    kbps <= 0 -> stringResource(R.string.settings_bandwidth_none)
    kbps < 1024 -> stringResource(R.string.settings_bandwidth_kbps, kbps)
    kbps % 1024 == 0 -> stringResource(R.string.settings_bandwidth_mbps, (kbps / 1024).toString())
    else -> stringResource(R.string.settings_bandwidth_mbps, String.format(java.util.Locale.US, "%.1f", kbps / 1024f))
}

@Composable
private fun cacheText(mb: Int): String =
    if (mb >= 1024 && mb % 1024 == 0) stringResource(R.string.settings_cache_gb, mb / 1024) else stringResource(R.string.settings_cache_mb, Fmt.count(mb))

@Composable
private fun deleteLimitText(s: AppSettings): String =
    pluralStringResource(R.plurals.settings_guard_summary, s.massDeleteMinFiles, Fmt.count(s.massDeleteMinFiles), s.massDeleteThresholdPercent)

@Composable
private fun conflictTitle(p: ConflictPolicy): String = stringResource(
    when (p) {
        ConflictPolicy.KEEP_BOTH -> R.string.common_keep_both
        ConflictPolicy.NEWER_WINS -> R.string.settings_conflict_newer
        ConflictPolicy.LOCAL_WINS -> R.string.settings_conflict_local
        ConflictPolicy.REMOTE_WINS -> R.string.settings_conflict_remote
        ConflictPolicy.SKIP -> R.string.settings_conflict_skip
    },
)

@Composable
private fun conflictDescription(p: ConflictPolicy): String = stringResource(
    when (p) {
        ConflictPolicy.KEEP_BOTH -> R.string.settings_conflict_keep_both_description
        ConflictPolicy.NEWER_WINS -> R.string.settings_conflict_newer_description
        ConflictPolicy.LOCAL_WINS -> R.string.settings_conflict_local_description
        ConflictPolicy.REMOTE_WINS -> R.string.settings_conflict_remote_description
        ConflictPolicy.SKIP -> R.string.settings_conflict_skip_description
    },
)

@Composable
private fun conflictSummary(p: ConflictPolicy): String = stringResource(
    when (p) {
        ConflictPolicy.KEEP_BOTH -> R.string.settings_conflict_keep_both_summary
        ConflictPolicy.NEWER_WINS -> R.string.settings_conflict_newer_summary
        ConflictPolicy.LOCAL_WINS -> R.string.settings_conflict_local_summary
        ConflictPolicy.REMOTE_WINS -> R.string.settings_conflict_remote_summary
        ConflictPolicy.SKIP -> R.string.settings_conflict_skip_summary
    },
)

@Composable
private fun docsTitle(f: DocsFormat): String = when (f) {
    DocsFormat.OFFICE -> stringResource(R.string.settings_docs_office)
    DocsFormat.PDF -> "PDF"
}

@Composable
private fun docsDescription(f: DocsFormat): String = when (f) {
    DocsFormat.OFFICE -> stringResource(R.string.settings_docs_office_description)
    DocsFormat.PDF -> stringResource(R.string.settings_docs_pdf_description)
}

/** Format names, the same in every language. */
private fun docsShort(f: DocsFormat): String = when (f) {
    DocsFormat.OFFICE -> "Office"
    DocsFormat.PDF -> "PDF"
}

@Composable
private fun disconnectBody(pairs: Int): String =
    if (pairs == 0) stringResource(R.string.settings_disconnect_body_none) else pluralStringResource(R.plurals.settings_disconnect_body, pairs, pairs)

/** The phone's own screen for this app's language (Android 13+), which lists the locales the APK ships. */
@RequiresApi(Build.VERSION_CODES.TIRAMISU)
private fun openAppLanguageSettings(context: Context) {
    val intent = Intent(Settings.ACTION_APP_LOCALE_SETTINGS, Uri.fromParts("package", context.packageName, null))
    runCatching { context.startActivity(intent) }
}

private fun String.ellipsize(max: Int): String = if (length <= max) this else take(max - 1).trimEnd() + "…"
