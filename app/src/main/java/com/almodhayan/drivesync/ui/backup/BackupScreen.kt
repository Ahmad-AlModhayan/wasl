package com.almodhayan.drivesync.ui.backup

import android.content.Context
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.annotation.PluralsRes
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilledTonalButton
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
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.backup.ConfigBackup
import com.almodhayan.drivesync.migration.DriveSyncImport
import com.almodhayan.drivesync.backup.ConfigSnapshot
import com.almodhayan.drivesync.backup.DriveBackupInfo
import com.almodhayan.drivesync.backup.ImportMode
import com.almodhayan.drivesync.backup.ImportPlan
import com.almodhayan.drivesync.backup.ImportReport
import com.almodhayan.drivesync.backup.scheduleSoon
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.db.AccountEntity
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.prefs.AppSettings
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.common.AppScreen
import com.almodhayan.drivesync.ui.components.Callout
import com.almodhayan.drivesync.ui.components.CalloutTone
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.SectionHeader
import com.almodhayan.drivesync.ui.components.SettingControl
import com.almodhayan.drivesync.ui.components.SettingRow
import com.almodhayan.drivesync.ui.components.UiAction
import com.almodhayan.drivesync.ui.components.showMessage
import com.almodhayan.drivesync.ui.settings.ChoiceOption
import com.almodhayan.drivesync.ui.settings.ContentDialog
import com.almodhayan.drivesync.ui.theme.Ds
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import dagger.hilt.android.lifecycle.HiltViewModel
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.time.LocalDate
import javax.inject.Inject

@HiltViewModel
class BackupViewModel @Inject constructor(
    @ApplicationContext private val context: Context,
    private val backup: ConfigBackup,
    private val settings: SettingsRepository,
    private val driveSync: DriveSyncImport,
    db: Db,
) : ViewModel() {

    sealed class DriveState {
        data object Loading : DriveState()
        data object None : DriveState()
        data class Found(val info: DriveBackupInfo) : DriveState()
        data class Failed(val hint: String) : DriveState()
    }

    /** A parsed backup waiting for the user to pick Merge or Replace. Both plans are precomputed for the dialog. */
    class PendingImport(val source: String, val snapshot: ConfigSnapshot, val merge: ImportPlan, val replace: ImportPlan)

    /** What failed, why, and the one fix: run the same action again. */
    class Failure(val title: String, val hint: String, val retry: () -> Unit)

    val accounts: StateFlow<List<AccountEntity>> = db.accounts().observeAll().stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), emptyList())
    val config: StateFlow<AppSettings?> = settings.settings.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    private val _drive = MutableStateFlow<Map<String, DriveState>>(emptyMap())
    val drive: StateFlow<Map<String, DriveState>> = _drive.asStateFlow()

    /** Non-null while a backup/restore/export/import runs; the text is shown with a spinner. */
    private val _busy = MutableStateFlow<String?>(null)
    val busy: StateFlow<String?> = _busy.asStateFlow()

    private val _message = MutableStateFlow<String?>(null)
    val message: StateFlow<String?> = _message.asStateFlow()

    private val _failure = MutableStateFlow<Failure?>(null)
    val failure: StateFlow<Failure?> = _failure.asStateFlow()

    private val _pending = MutableStateFlow<PendingImport?>(null)
    val pending: StateFlow<PendingImport?> = _pending.asStateFlow()

    private val _report = MutableStateFlow<ImportReport?>(null)
    val report: StateFlow<ImportReport?> = _report.asStateFlow()

    init {
        viewModelScope.launch {
            accounts.collect { list -> list.forEach { if (_drive.value[it.id] == null) refreshDrive(it.id) } }
        }
    }

    fun refreshDrive(accountId: String) {
        _drive.update { it + (accountId to DriveState.Loading) }
        viewModelScope.launch {
            val state = try {
                backup.listDriveBackups(accountId)?.let { DriveState.Found(it) } ?: DriveState.None
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                DriveState.Failed(AppError.wrap(t).userHint)
            }
            _drive.update { it + (accountId to state) }
        }
    }

    fun backupNow(accountId: String) = launchBusy(Strings.get(R.string.backup_busy_backing_up, accountId), Strings.get(R.string.backup_fail_backup, accountId)) {
        val info = backup.backupToDrive(accountId)
        _drive.update { it + (accountId to DriveState.Found(info)) }
        _message.value = Strings.get(R.string.backup_done_backup, accountId)
    }

    fun prepareRestoreFromDrive(accountId: String) = launchBusy(Strings.get(R.string.backup_busy_downloading, accountId), Strings.get(R.string.backup_fail_download)) {
        stage(Strings.get(R.string.backup_source_drive, accountId), backup.downloadFromDrive(accountId))
    }

    fun exportTo(uri: Uri) = launchBusy(Strings.get(R.string.backup_busy_saving), Strings.get(R.string.backup_fail_save)) {
        val json = backup.export()
        withContext(Dispatchers.IO) {
            val out = context.contentResolver.openOutputStream(uri, "wt") ?: throw AppError.Io(Strings.get(R.string.backup_error_open_write))
            out.use { it.write(json.toByteArray(Charsets.UTF_8)) }
        }
        _message.value = Strings.get(R.string.backup_saved)
    }

    fun prepareImportFrom(uri: Uri) = launchBusy(Strings.get(R.string.backup_busy_reading), Strings.get(R.string.backup_fail_read)) {
        val json = withContext(Dispatchers.IO) {
            val input = context.contentResolver.openInputStream(uri) ?: throw AppError.Io(Strings.get(R.string.backup_error_open_read))
            input.use { it.readBytes() }.toString(Charsets.UTF_8)
        }
        stage(Strings.get(R.string.backup_source_file), json)
    }

    /** Drive Sync 1.1.5's "Export for Wasl" file (SRS FR-K2): pairs keep their sync records. */
    fun importDriveSync(uri: Uri) = launchBusy(Strings.get(R.string.migration_busy), Strings.get(R.string.migration_fail)) {
        val r = driveSync.import(uri)
        _report.value = r.config
        val missing = if (r.binFailed > 0) " " + Strings.plural(R.plurals.migration_bin_missing, r.binFailed) else ""
        _message.value = Strings.plural(R.plurals.migration_done, r.pairs) + missing
    }

    private suspend fun stage(source: String, json: String) {
        val snapshot = backup.parse(json)
        _pending.value = PendingImport(source, snapshot, backup.preview(snapshot, ImportMode.MERGE), backup.preview(snapshot, ImportMode.REPLACE))
    }

    fun confirmImport(mode: ImportMode) {
        val p = _pending.value ?: return
        _pending.value = null
        launchBusy(Strings.get(if (mode == ImportMode.REPLACE) R.string.backup_busy_replacing else R.string.backup_busy_merging), Strings.get(R.string.backup_fail_restore)) {
            _report.value = backup.import(p.snapshot, mode)
        }
    }

    fun dismissPending() { _pending.value = null }
    fun dismissReport() { _report.value = null }
    fun dismissMessage() { _message.value = null }
    fun dismissFailure() { _failure.value = null }

    fun setAutoBackup(enabled: Boolean) {
        viewModelScope.launch {
            settings.update { it.copy(autoBackupConfig = enabled) }
            if (enabled) backup.scheduleSoon(context)
        }
    }

    private fun launchBusy(label: String, failTitle: String, block: suspend () -> Unit) {
        if (_busy.value != null) return
        _busy.value = label
        _failure.value = null
        viewModelScope.launch {
            try {
                block()
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _failure.value = Failure(failTitle, AppError.wrap(t).userHint) { launchBusy(label, failTitle, block) }
            } finally {
                _busy.value = null
            }
        }
    }
}

@Composable
fun BackupScreen(nav: AppNav) {
    val vm: BackupViewModel = hiltViewModel()
    val accounts by vm.accounts.collectAsStateWithLifecycle()
    val cfg by vm.config.collectAsStateWithLifecycle()
    val drive by vm.drive.collectAsStateWithLifecycle()
    val busy by vm.busy.collectAsStateWithLifecycle()
    val message by vm.message.collectAsStateWithLifecycle()
    val failure by vm.failure.collectAsStateWithLifecycle()
    val pending by vm.pending.collectAsStateWithLifecycle()
    val report by vm.report.collectAsStateWithLifecycle()
    val cs = MaterialTheme.colorScheme

    val exportLauncher = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/json")) { uri ->
        if (uri != null) vm.exportTo(uri)
    }
    val driveSyncLauncher = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) vm.importDriveSync(uri)
    }
    val importLauncher = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) vm.prepareImportFrom(uri)
    }
    val snackbar = remember { SnackbarHostState() }
    LaunchedEffect(message) {
        val m = message ?: return@LaunchedEffect
        snackbar.showMessage(m)
        vm.dismissMessage()
    }
    val scrollBehavior = TopAppBarDefaults.pinnedScrollBehavior()

    AppScreen(title = stringResource(R.string.backup_title), onBack = { nav.back() }, snackbarHost = { SnackbarHost(snackbar) }, scrollBehavior = scrollBehavior) { padding ->
        Column(Modifier.fillMaxSize().padding(padding).verticalScroll(rememberScrollState()).padding(bottom = Space.s24)) {
            busy?.let { BusyRow(it) }
            failure?.let { f ->
                Callout(
                    text = f.hint, title = f.title, tone = CalloutTone.Danger,
                    action = UiAction(stringResource(R.string.common_try_again), Sym.Sync) { f.retry() },
                    modifier = Modifier.padding(horizontal = Space.s16, vertical = Space.s8),
                )
            }

            SectionHeader(stringResource(R.string.backup_section_google_drive))
            Text(
                stringResource(R.string.backup_drive_intro),
                style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = Space.s16, vertical = Space.s4),
            )
            if (accounts.isEmpty()) {
                Callout(
                    text = stringResource(R.string.backup_add_account_hint), tone = CalloutTone.Info,
                    action = UiAction(stringResource(R.string.backup_add_account)) { nav.accounts() },
                    modifier = Modifier.padding(horizontal = Space.s16, vertical = Space.s8),
                )
            }
            accounts.forEach { account ->
                DriveBackupCard(
                    account = account,
                    state = drive[account.id],
                    disabled = busy != null,
                    onBackup = { vm.backupNow(account.id) },
                    onRestore = { vm.prepareRestoreFromDrive(account.id) },
                    onRefresh = { vm.refreshDrive(account.id) },
                    modifier = Modifier.padding(horizontal = Space.s16, vertical = Space.s8),
                )
            }
            SettingRow(
                title = stringResource(R.string.backup_auto_title), icon = Sym.Schedule,
                summary = stringResource(if (cfg?.autoBackupConfig != false) R.string.backup_auto_on else R.string.backup_auto_off),
                control = SettingControl.Toggle(cfg?.autoBackupConfig ?: true) { vm.setAutoBackup(it) },
                enabled = cfg != null,
            )
            SettingRow(
                title = stringResource(R.string.backup_last_from_phone), icon = Sym.History,
                summary = cfg?.lastBackupAt?.takeIf { it > 0 }?.let { Fmt.dayAndTime(it) } ?: stringResource(R.string.backup_never),
                control = SettingControl.None,
            )

            SectionHeader(stringResource(R.string.backup_section_file))
            SettingRow(
                title = stringResource(R.string.backup_export_title), icon = Sym.Download,
                summary = stringResource(R.string.backup_export_summary),
                enabled = busy == null,
                onClick = { exportLauncher.launch("drivesync-config-${LocalDate.now()}.json") },
            )
            SettingRow(
                title = stringResource(R.string.backup_import_title), icon = Sym.FolderOpen,
                summary = stringResource(R.string.backup_import_summary),
                enabled = busy == null,
                onClick = { importLauncher.launch(arrayOf("application/json")) },
            )

            SectionHeader(stringResource(R.string.migration_section))
            SettingRow(
                title = stringResource(R.string.migration_import_title), icon = Sym.FolderOpen,
                summary = stringResource(R.string.migration_import_summary),
                enabled = busy == null,
                onClick = { driveSyncLauncher.launch(arrayOf("application/zip", "application/octet-stream")) },
            )

            SectionHeader(stringResource(R.string.backup_section_holds))
            Column(Modifier.padding(horizontal = Space.s16, vertical = Space.s4), verticalArrangement = Arrangement.spacedBy(Space.s8)) {
                Bullet(stringResource(R.string.backup_holds_pairs))
                Bullet(stringResource(R.string.backup_holds_settings))
                Bullet(stringResource(R.string.backup_holds_vaults))
                Bullet(stringResource(R.string.backup_holds_offline))
                Bullet(stringResource(R.string.backup_holds_emails))
                Callout(
                    text = stringResource(R.string.backup_holds_never),
                    tone = CalloutTone.Info,
                    modifier = Modifier.padding(top = Space.s8),
                )
            }
        }
    }

    pending?.let { p -> ImportModeDialog(p, onConfirm = { vm.confirmImport(it) }, onDismiss = vm::dismissPending) }
    report?.let { r ->
        ImportReportDialog(
            r,
            onDismiss = vm::dismissReport,
            onAccounts = { vm.dismissReport(); nav.accounts() },
            onVaults = { vm.dismissReport(); nav.vaults() },
        )
    }
}

@Composable
private fun BusyRow(label: String) {
    Row(
        Modifier.fillMaxWidth().padding(horizontal = Space.s16, vertical = Space.s8).semantics(mergeDescendants = true) { liveRegion = LiveRegionMode.Polite },
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.s12),
    ) {
        CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp, color = Ds.status.syncing.content)
        Text(label, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
private fun DriveBackupCard(
    account: AccountEntity,
    state: BackupViewModel.DriveState?,
    disabled: Boolean,
    onBackup: () -> Unit,
    onRestore: () -> Unit,
    onRefresh: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val cs = MaterialTheme.colorScheme
    val shape = MaterialTheme.shapes.large
    Column(
        modifier.fillMaxWidth().clip(shape).background(Ds.lab.card).padding(Space.s16),
        verticalArrangement = Arrangement.spacedBy(Space.s12),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s12)) {
            Box(Modifier.size(40.dp).clip(CircleShape).background(cs.tertiaryContainer), contentAlignment = Alignment.Center) {
                Icon(Sym.Cloud, contentDescription = null, tint = cs.onTertiaryContainer)
            }
            Column(Modifier.weight(1f)) {
                Text(account.id, style = MaterialTheme.typography.titleMedium, color = cs.onSurface, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(
                    when (state) {
                        null, BackupViewModel.DriveState.Loading -> stringResource(R.string.backup_checking_drive)
                        BackupViewModel.DriveState.None -> stringResource(R.string.backup_none_in_drive)
                        is BackupViewModel.DriveState.Found -> {
                            val last = state.info.modifiedTime?.let { stringResource(R.string.backup_last_backup_at, Fmt.dayAndTime(it)) }
                                ?: stringResource(R.string.backup_last_backup_unknown)
                            state.info.size?.let { "$last · ${Fmt.bytes(it)}" } ?: last
                        }
                        is BackupViewModel.DriveState.Failed -> stringResource(R.string.backup_check_failed)
                    },
                    style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant,
                )
            }
        }
        if (state is BackupViewModel.DriveState.Failed) {
            Callout(text = state.hint, tone = CalloutTone.Danger, action = UiAction(stringResource(R.string.backup_check_again), Sym.Sync) { onRefresh() })
        }
        Row(horizontalArrangement = Arrangement.spacedBy(Space.s8)) {
            FilledTonalButton(onClick = onBackup, enabled = !disabled) { Text(stringResource(R.string.backup_back_up_now)) }
            OutlinedButton(onClick = onRestore, enabled = !disabled && state is BackupViewModel.DriveState.Found) { Text(stringResource(R.string.common_restore)) }
        }
    }
}

@Composable
private fun Bullet(text: String) {
    val cs = MaterialTheme.colorScheme
    Row(horizontalArrangement = Arrangement.spacedBy(Space.s12), verticalAlignment = Alignment.Top) {
        Box(Modifier.padding(top = 8.dp).size(6.dp).clip(CircleShape).background(cs.onSurfaceVariant))
        Text(text, style = MaterialTheme.typography.bodyMedium, color = cs.onSurface, modifier = Modifier.weight(1f))
    }
}

/** Pairs listed by name in the import dialog; the rest are counted. */
private const val MAX_PAIR_LINES = 8

@Composable
private fun ImportModeDialog(p: BackupViewModel.PendingImport, onConfirm: (ImportMode) -> Unit, onDismiss: () -> Unit) {
    var mode by remember { mutableStateOf(ImportMode.MERGE) }
    val snap = p.snapshot
    val plan = if (mode == ImportMode.MERGE) p.merge else p.replace
    val cs = MaterialTheme.colorScheme
    ContentDialog(
        title = stringResource(R.string.backup_restore_title),
        icon = Sym.History,
        onDismiss = onDismiss,
        confirmLabel = stringResource(if (mode == ImportMode.REPLACE) R.string.backup_replace_pairs else R.string.backup_merge),
        onConfirm = { onConfirm(mode) },
        danger = mode == ImportMode.REPLACE,
    ) {
        val device = snap.deviceName.ifBlank { stringResource(R.string.backup_another_phone) }
        val from = if (snap.exportedAt > 0) {
            stringResource(R.string.backup_restore_from_dated, p.source, device, Fmt.dayAndTime(snap.exportedAt))
        } else {
            stringResource(R.string.backup_restore_from, p.source, device)
        }
        val contents = stringResource(
            R.string.backup_restore_contents,
            countText(R.plurals.backup_count_pairs, snap.pairs.size),
            countText(R.plurals.backup_count_accounts, snap.accounts.size),
            countText(R.plurals.backup_count_vaults, snap.vaults.size),
            countText(R.plurals.backup_count_offline_files, snap.pins.size),
        )
        val mergeLabel = stringResource(R.string.backup_merge)
        val replaceLabel = stringResource(R.string.backup_replace)
        val mergeDescription = stringResource(
            R.string.backup_merge_description,
            countText(R.plurals.backup_count_new_pairs, p.merge.pairsAdded),
            countText(R.plurals.backup_count_pairs, p.merge.pairsUpdated),
        )
        val removing = p.replace.pairIdsToRemove.size
        val replaceDescription = pluralStringResource(
            R.plurals.backup_replace_description, removing,
            Fmt.count(removing), countText(R.plurals.backup_count_pairs, p.replace.pairsAdded),
        )
        Text(
            "$from $contents",
            style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant,
        )
        Column(Modifier.selectableGroup()) {
            ChoiceOption(
                label = mergeLabel, selected = mode == ImportMode.MERGE, onClick = { mode = ImportMode.MERGE }, recommended = true,
                description = mergeDescription,
            )
            ChoiceOption(
                label = replaceLabel, selected = mode == ImportMode.REPLACE, onClick = { mode = ImportMode.REPLACE },
                description = replaceDescription,
            )
        }
        // Where each pair will sync and how, so a crafted backup cannot hide a pair (security review 2026-10).
        if (plan.pairsToUpsert.isNotEmpty()) {
            Column(verticalArrangement = Arrangement.spacedBy(Space.s4)) {
                Text(stringResource(R.string.backup_pairs_heading), style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant)
                plan.pairsToUpsert.take(MAX_PAIR_LINES).forEach { pair ->
                    Text(
                        stringResource(R.string.backup_pair_line, Fmt.humanLocalPath(pair.localPath), Fmt.humanRemotePath(pair.remotePath).ifBlank { pair.remoteFolderId }, pair.mode.label),
                        style = MaterialTheme.typography.bodySmall, color = cs.onSurface,
                    )
                }
                val more = plan.pairsToUpsert.size - MAX_PAIR_LINES
                if (more > 0) Text(stringResource(R.string.backup_pairs_more, Fmt.count(more)), style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant)
            }
        }
        Text(
            stringResource(R.string.backup_settings_apply),
            style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant,
        )
        if (plan.accountsToSignIn.isNotEmpty()) {
            Callout(
                text = stringResource(R.string.backup_not_signed_in, plan.accountsToSignIn.joinToString(", ")),
                tone = CalloutTone.Warning,
            )
        }
        if (plan.skipped.isNotEmpty()) {
            Callout(text = stringResource(R.string.backup_left_out, plan.skipped.joinToString("; ")), tone = CalloutTone.Warning)
        }
        plan.warnings.forEach { Text(it, style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant) }
    }
}

@Composable
private fun ImportReportDialog(r: ImportReport, onDismiss: () -> Unit, onAccounts: () -> Unit, onVaults: () -> Unit) {
    val signIn = r.accountsToSignIn.isNotEmpty()
    ContentDialog(
        title = stringResource(R.string.backup_restore_finished),
        icon = Sym.TaskAlt,
        onDismiss = onDismiss,
        confirmLabel = when {
            signIn -> stringResource(R.string.backup_sign_in)
            r.needsVaultPassphrase -> stringResource(R.string.backup_unlock_vaults)
            else -> null
        },
        onConfirm = { if (signIn) onAccounts() else onVaults() },
        dismissLabel = stringResource(R.string.common_done),
    ) {
        r.lines().forEach { Text(it, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurface) }
        if (signIn && r.needsVaultPassphrase) {
            Callout(text = stringResource(R.string.backup_vaults_need_passphrases), tone = CalloutTone.Info, action = UiAction(stringResource(R.string.backup_open_vaults)) { onVaults() })
        }
    }
}

/** "1 pair", "3 pairs": [id] is a plurals resource whose first argument is the count. */
@Composable
private fun countText(@PluralsRes id: Int, n: Int): String = pluralStringResource(id, n, Fmt.count(n))
