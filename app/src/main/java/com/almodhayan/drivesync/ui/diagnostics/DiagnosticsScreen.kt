package com.almodhayan.drivesync.ui.diagnostics

import android.Manifest
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.common.AppScreen
import com.almodhayan.drivesync.ui.common.LoadingState
import com.almodhayan.drivesync.ui.components.BottomActionBar
import com.almodhayan.drivesync.ui.components.Callout
import com.almodhayan.drivesync.ui.components.CalloutTone
import com.almodhayan.drivesync.ui.components.ChipModel
import com.almodhayan.drivesync.ui.components.FilterChipRow
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.HealthCheckRow
import com.almodhayan.drivesync.ui.components.LogLevel
import com.almodhayan.drivesync.ui.components.LogLine
import com.almodhayan.drivesync.ui.components.LogLineRow
import com.almodhayan.drivesync.ui.components.MenuAction
import com.almodhayan.drivesync.ui.components.OverflowMenu
import com.almodhayan.drivesync.ui.components.SectionHeader
import com.almodhayan.drivesync.ui.components.UiAction
import com.almodhayan.drivesync.ui.components.showMessage
import com.almodhayan.drivesync.ui.onboarding.DeviceChecks
import com.almodhayan.drivesync.ui.onboarding.VendorGuide
import com.almodhayan.drivesync.ui.onboarding.rememberResumeTick
import com.almodhayan.drivesync.ui.settings.Automation
import com.almodhayan.drivesync.ui.settings.ContentDialog
import com.almodhayan.drivesync.ui.settings.DialogSwitch
import com.almodhayan.drivesync.ui.theme.Ds
import com.almodhayan.drivesync.ui.theme.MonoMedium
import com.almodhayan.drivesync.ui.theme.MonoSmall
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import com.almodhayan.drivesync.ui.theme.tabular
import kotlinx.coroutines.launch

/**
 * Diagnostics: plain-language health checks first, each failing row with one fix; then the last 24
 * hours and the filterable log; Export log hides file names unless you include them.
 */
@Composable
fun DiagnosticsScreen(nav: AppNav, vm: DiagnosticsViewModel = hiltViewModel()) {
    val state by vm.state.collectAsStateWithLifecycle()
    val log by vm.log.collectAsStateWithLifecycle()
    val filter by vm.filter.collectAsStateWithLifecycle()
    val context = LocalContext.current
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    val scrollBehavior = TopAppBarDefaults.pinnedScrollBehavior()
    val vendor = remember { VendorGuide.current() }
    val tick = rememberResumeTick()
    var exportOpen by remember { mutableStateOf(false) }
    var openLine by remember { mutableStateOf<LogLine?>(null) }

    LaunchedEffect(tick) { if (tick > 0) vm.refreshLocal() }
    LaunchedEffect(vm) { vm.messages.collect { m -> scope.launch { snackbar.showMessage(m) } } }

    val notificationLauncher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { vm.refreshLocal() }
    val locationLauncher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { vm.refreshLocal() }

    fun fix(item: HealthItem) {
        when (item.fix) {
            HealthFix.GRANT_ACCESS -> if (!DeviceChecks.openAllFilesAccess(context)) vm.say(R.string.diagnostics_couldnt_open_settings)
            HealthFix.CHOOSE_FOLDER -> item.pairId?.let { nav.pairEdit(it) }
            HealthFix.ADD_ACCOUNT, HealthFix.SIGN_IN -> nav.accounts()
            HealthFix.BATTERY -> if (!DeviceChecks.requestIgnoreBatteryOptimizations(context)) vm.say(R.string.diagnostics_couldnt_open_battery)
            HealthFix.REARM -> vm.rearm()
            HealthFix.LOCATION -> locationLauncher.launch(Manifest.permission.ACCESS_FINE_LOCATION)
            HealthFix.NOTIFICATIONS ->
                if (DeviceChecks.needsNotificationRuntimePermission()) notificationLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
                else DeviceChecks.openNotificationSettings(context)
            HealthFix.FREE_SPACE -> if (!startSafely(context, Intent(Intent.ACTION_VIEW, Uri.parse(HealthChecks.DRIVE_STORAGE_URL)))) vm.say(R.string.diagnostics_no_browser_storage)
            null -> Unit
        }
    }

    val copyReportLabel = stringResource(R.string.diagnostics_copy_report)
    val shareReportLabel = stringResource(R.string.diagnostics_share_report)
    val guideLabel = stringResource(R.string.diagnostics_background_guide)

    AppScreen(
        title = stringResource(R.string.common_diagnostics),
        onBack = { nav.back() },
        scrollBehavior = scrollBehavior,
        snackbarHost = { SnackbarHost(snackbar) },
        actions = {
            IconButton(onClick = { vm.runChecks() }, enabled = !state.checking) { Icon(Sym.Sync, contentDescription = stringResource(R.string.diagnostics_run_checks_again)) }
            OverflowMenu(
                listOf(
                    MenuAction(copyReportLabel, Sym.ContentCopy) {
                        Automation.copyToClipboard(context, Strings.get(R.string.diagnostics_report_label), DiagnosticsReport.build(state))
                        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) vm.say(R.string.diagnostics_copied_report)
                    },
                    MenuAction(shareReportLabel, Sym.Share) {
                        val send = Intent(Intent.ACTION_SEND).setType("text/plain")
                            .putExtra(Intent.EXTRA_SUBJECT, Strings.get(R.string.diagnostics_report_label))
                            .putExtra(Intent.EXTRA_TEXT, DiagnosticsReport.build(state))
                        if (!startSafely(context, Intent.createChooser(send, shareReportLabel))) vm.say(R.string.diagnostics_no_app_for_report)
                    },
                    MenuAction(guideLabel, Sym.OpenInNew) {
                        if (!DeviceChecks.openUrl(context, VendorGuide.dontKillMyAppUrl(vendor))) vm.say(R.string.diagnostics_no_browser_dontkillmyapp)
                    },
                ),
                contentDescription = stringResource(R.string.diagnostics_more_options),
            )
        },
        bottomBar = {
            BottomActionBar(hint = if (state.checking) stringResource(R.string.diagnostics_checking) else state.checkedAt?.let { stringResource(R.string.diagnostics_checked_at, Fmt.clock(it)) }) {
                FilledTonalButton(onClick = { exportOpen = true }) {
                    Icon(Sym.Download, contentDescription = null, modifier = Modifier.size(18.dp))
                    Spacer(Modifier.size(Space.s8))
                    Text(stringResource(R.string.diagnostics_export_log))
                }
            }
        },
    ) { padding ->
        if (state.loading || state.device == null) {
            Box(Modifier.fillMaxSize().padding(padding)) { LoadingState(stringResource(R.string.diagnostics_checking)) }
        } else {
            val health = remember(state, vendor) { HealthChecks.build(state, vendor?.label) }
            val lines = log
            LazyColumn(
                Modifier.fillMaxSize(),
                contentPadding = PaddingValues(top = padding.calculateTopPadding(), bottom = padding.calculateBottomPadding() + Space.s16),
            ) {
                item(key = "h-health") { SectionHeader(stringResource(R.string.diagnostics_health)) }
                items(health, key = { it.key }) { h ->
                    Column {
                        HealthCheckRow(
                            state = h.state, title = h.title, detail = h.detail,
                            actionLabel = h.fixLabel, onAction = if (h.fix != null) { { fix(h) } } else null,
                        )
                        if (h.key == "battery" && vendor != null) {
                            Callout(
                                text = vendor.guidance,
                                title = stringResource(R.string.diagnostics_vendor_phones, vendor.label),
                                tone = CalloutTone.Info,
                                action = UiAction(stringResource(R.string.diagnostics_open_background_settings), Sym.OpenInNew) {
                                    if (!VendorGuide.openAutostartSettings(context)) vm.say(R.string.diagnostics_opened_app_settings)
                                },
                                modifier = Modifier.padding(start = Space.s16, end = Space.s16, bottom = Space.s8),
                            )
                        }
                    }
                }

                item(key = "h-day") { SectionHeader(stringResource(R.string.diagnostics_last_24_hours)) }
                item(key = "stats") { DayStats(state.summary) }

                item(key = "h-log") { SectionHeader(stringResource(R.string.diagnostics_log)) }
                item(key = "chips") {
                    val allLabel = stringResource(R.string.diagnostics_filter_all)
                    val errorsLabel = stringResource(R.string.diagnostics_filter_errors)
                    FilterChipRow(
                        buildList<ChipModel> {
                            add(ChipModel(allLabel, selected = filter.isAll, onClick = { vm.showAll() }))
                            add(ChipModel(errorsLabel, selected = filter.errorsOnly, onClick = { vm.toggleErrors() }))
                            state.pairs.forEach { p -> add(ChipModel(p.name, selected = filter.pairId == p.id, icon = Sym.FilterList, onClick = { vm.togglePair(p.id) })) }
                        },
                        modifier = Modifier.padding(bottom = Space.s8),
                    )
                }
                when {
                    lines == null -> item(key = "log-loading") { LogNote(stringResource(R.string.diagnostics_loading_log)) }
                    lines.isEmpty() -> item(key = "log-empty") { LogNote(stringResource(if (filter.isAll) R.string.diagnostics_nothing_logged else R.string.diagnostics_no_lines_match)) }
                    else -> itemsIndexed(lines) { i, line ->
                        LogItem(line, first = i == 0, last = i == lines.lastIndex, onClick = { openLine = line })
                    }
                }
            }
        }
    }

    if (exportOpen) {
        var includeNames by remember { mutableStateOf(false) }
        ContentDialog(
            title = stringResource(R.string.diagnostics_export_log),
            icon = Sym.Download,
            onDismiss = { exportOpen = false },
            confirmLabel = stringResource(R.string.diagnostics_export_log),
            onConfirm = {
                exportOpen = false
                vm.exportLog(includeNames) { intent -> if (!startSafely(context, intent)) vm.say(R.string.diagnostics_no_app_for_log) }
            },
        ) {
            Text(
                stringResource(R.string.diagnostics_export_body),
                style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            DialogSwitch(
                title = stringResource(R.string.diagnostics_include_file_names),
                checked = includeNames,
                onChange = { includeNames = it },
                summary = stringResource(if (includeNames) R.string.diagnostics_include_names_on else R.string.diagnostics_include_names_off),
            )
        }
    }

    openLine?.let { l ->
        ContentDialog(
            title = stringResource(R.string.diagnostics_log_line),
            onDismiss = { openLine = null },
            confirmLabel = stringResource(R.string.diagnostics_copy_line),
            onConfirm = {
                Automation.copyToClipboard(context, Strings.get(R.string.diagnostics_log_line_clip_label), "${l.time} ${l.level.name} ${l.message}" + (l.detail?.let { "\n$it" } ?: ""))
                openLine = null
            },
            dismissLabel = stringResource(R.string.common_close),
        ) {
            Row(horizontalArrangement = Arrangement.spacedBy(Space.s8)) {
                Text(l.time, style = MonoSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                Text(l.level.name, style = MonoSmall, color = levelColor(l.level))
            }
            SelectionContainer { Text(l.message, style = MonoMedium, color = MaterialTheme.colorScheme.onSurface) }
            val detail = l.detail
            if (!detail.isNullOrBlank()) {
                Text(stringResource(R.string.diagnostics_detail), style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.onSurface)
                SelectionContainer { Text(detail, style = MonoSmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
            }
        }
    }
}

// -------------------------------------------------------------------------------------- pieces

@Composable
private fun DayStats(sum: DaySummary) {
    Row(
        Modifier.fillMaxWidth().padding(horizontal = Space.s16).height(IntrinsicSize.Min),
        horizontalArrangement = Arrangement.spacedBy(Space.s8),
    ) {
        Stat(Fmt.count(sum.runs), pluralStringResource(R.plurals.diagnostics_stat_syncs, sum.runs), Modifier.weight(1f).fillMaxHeight())
        Stat(Fmt.count(sum.filesMoved), pluralStringResource(R.plurals.diagnostics_stat_files_moved, sum.filesMoved), Modifier.weight(1f).fillMaxHeight())
        Stat(Fmt.count(sum.errors), pluralStringResource(R.plurals.diagnostics_stat_errors, sum.errors), Modifier.weight(1f).fillMaxHeight())
    }
}

@Composable
private fun Stat(number: String, label: String, modifier: Modifier) {
    val cs = MaterialTheme.colorScheme
    val shape = MaterialTheme.shapes.medium
    Column(
        modifier.clip(shape).background(Ds.lab.card)
            .padding(horizontal = Space.s12, vertical = Space.s12)
            .semantics(mergeDescendants = true) { contentDescription = "$number $label" },
    ) {
        Text(number, style = MaterialTheme.typography.titleLarge.tabular, color = cs.onSurface)
        Text(label, style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant)
    }
}

/** One line of the log block (surface-container-highest); the first and last round the block's corners. */
@Composable
private fun LogItem(line: LogLine, first: Boolean, last: Boolean, onClick: () -> Unit) {
    val r = 12.dp
    val shape = RoundedCornerShape(topStart = if (first) r else 0.dp, topEnd = if (first) r else 0.dp, bottomStart = if (last) r else 0.dp, bottomEnd = if (last) r else 0.dp)
    Box(
        Modifier.fillMaxWidth().padding(horizontal = Space.s16).clip(shape).background(MaterialTheme.colorScheme.surfaceContainerHighest)
            .padding(top = if (first) Space.s8 else 0.dp, bottom = if (last) Space.s8 else 0.dp),
    ) {
        Box(
            Modifier.fillMaxWidth().clickable(onClickLabel = stringResource(R.string.diagnostics_show_whole_line), onClick = onClick).heightIn(min = 32.dp).padding(horizontal = Space.s12),
            contentAlignment = Alignment.CenterStart,
        ) { LogLineRow(line) }
    }
}

@Composable
private fun LogNote(text: String) {
    Box(
        Modifier.fillMaxWidth().padding(horizontal = Space.s16).clip(MaterialTheme.shapes.medium).background(MaterialTheme.colorScheme.surfaceContainerHighest).padding(Space.s12),
    ) { Text(text, style = MonoSmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
}

@Composable
private fun levelColor(level: LogLevel) = when (level) {
    LogLevel.OK -> Ds.status.synced.content
    LogLevel.WARN -> Ds.status.approval.content
    LogLevel.ERROR -> Ds.status.failed.content
    LogLevel.INFO -> MaterialTheme.colorScheme.onSurfaceVariant
}

private fun startSafely(context: Context, intent: Intent): Boolean = try {
    context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    true
} catch (e: Exception) {
    false
}
