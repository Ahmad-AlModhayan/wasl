package com.almodhayan.drivesync.ui.diagnostics

import android.content.Context
import android.content.Intent
import androidx.annotation.StringRes
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import androidx.work.WorkInfo
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.actions.LogExport
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.db.AccountEntity
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.db.SyncEventEntity
import com.almodhayan.drivesync.data.prefs.AppSettings
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import com.almodhayan.drivesync.devices.DeviceSyncManager
import uniffi.wasl_devices.DevicesInfo
import com.almodhayan.drivesync.drive.DriveApi
import com.almodhayan.drivesync.scheduler.MonitorService
import com.almodhayan.drivesync.scheduler.SyncScheduler
import com.almodhayan.drivesync.sync.Conditions
import com.almodhayan.drivesync.ui.components.LogLine
import com.almodhayan.drivesync.ui.onboarding.DeviceChecks
import com.almodhayan.drivesync.ui.onboarding.DeviceStatus
import dagger.hilt.android.lifecycle.HiltViewModel
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.FlowPreview
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.catch
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.mapLatest
import kotlinx.coroutines.flow.onStart
import kotlinx.coroutines.flow.sample
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import javax.inject.Inject

/** Result of the silent call to Drive made for each account by "Run checks again". */
data class AccountCheck(val ok: Boolean, val at: Long, val auth: Boolean = false, val hint: String? = null)

/** "Last 24 hours" in three numbers. */
data class DaySummary(val runs: Int = 0, val filesMoved: Int = 0, val errors: Int = 0)

/** Log filters: All clears both; Errors and a pair can be combined. */
data class LogFilter(val errorsOnly: Boolean = false, val pairId: String? = null) {
    val isAll: Boolean get() = !errorsOnly && pairId == null
}

data class DiagnosticsUiState(
    val loading: Boolean = true,
    val settings: AppSettings? = null,
    val accounts: List<AccountEntity> = emptyList(),
    val pairs: List<FolderPairEntity> = emptyList(),
    val workInfos: List<WorkInfo> = emptyList(),
    val problems: List<SyncEventEntity> = emptyList(),
    val device: DeviceStatus? = null,
    val network: Conditions.Device? = null,
    val monitorRunning: Boolean = false,
    /** Turned-on pairs whose phone folder can't be read right now. */
    val unreachable: List<FolderPairEntity> = emptyList(),
    val checks: Map<String, AccountCheck> = emptyMap(),
    val checking: Boolean = false,
    val checkedAt: Long? = null,
    val summary: DaySummary = DaySummary(),
    /** Device sync (FR-B7): whether the phone takes part now, and each device's reachability and path. */
    val devicesOpen: Boolean = false,
    val devices: DevicesInfo? = null,
) {
    /** Location only matters when a pair limits syncing to certain Wi-Fi names. */
    val ssidRulesUsed: Boolean get() = pairs.any { it.ssidAllow.isNotBlank() || it.ssidDeny.isNotBlank() }
}

private data class Base(
    val settings: AppSettings,
    val accounts: List<AccountEntity>,
    val pairs: List<FolderPairEntity>,
    val work: List<WorkInfo>,
    val problems: List<SyncEventEntity>,
)

private data class LocalState(
    val device: DeviceStatus? = null,
    val network: Conditions.Device? = null,
    val monitorRunning: Boolean = false,
    val unreachable: Set<String> = emptySet(),
    val checks: Map<String, AccountCheck> = emptyMap(),
    val checking: Boolean = false,
    val checkedAt: Long? = null,
)

@OptIn(FlowPreview::class)
@HiltViewModel
class DiagnosticsViewModel @Inject constructor(
    @ApplicationContext private val context: Context,
    private val db: Db,
    settingsRepo: SettingsRepository,
    private val scheduler: SyncScheduler,
    private val api: DriveApi,
    private val conditions: Conditions,
    private val logExport: LogExport,
    private val deviceSync: DeviceSyncManager,
) : ViewModel() {

    private val local = MutableStateFlow(LocalState())
    private val since = MutableStateFlow(System.currentTimeMillis() - DAY_MS)

    private val _filter = MutableStateFlow(LogFilter())
    val filter: StateFlow<LogFilter> = _filter

    private val _messages = MutableSharedFlow<String>(extraBufferCapacity = 8)
    val messages: SharedFlow<String> = _messages

    private val base: Flow<Base> = combine(
        settingsRepo.settings,
        db.accounts().observeAll(),
        db.pairs().observeAll(),
        scheduler.observeWorkInfos(),
        db.events().observeProblems(5),
    ) { settings, accounts, pairs, work, problems -> Base(settings, accounts, pairs, work, problems) }

    private val summary: Flow<DaySummary> = since.flatMapLatest { s -> db.runs().observeSince(s) }
        .map { runs -> DaySummary(runs = runs.size, filesMoved = runs.sumOf { it.uploaded + it.downloaded }, errors = runs.sumOf { it.errors }) }

    private val devices: Flow<Pair<Boolean, DevicesInfo?>> = combine(deviceSync.open, deviceSync.info) { o, i -> o to i }

    val state: StateFlow<DiagnosticsUiState> = combine(base, local, summary, devices) { b, l, sum, dev ->
        DiagnosticsUiState(
            loading = false,
            settings = b.settings,
            accounts = b.accounts,
            pairs = b.pairs,
            workInfos = b.work,
            problems = b.problems,
            device = l.device,
            network = l.network,
            monitorRunning = l.monitorRunning,
            unreachable = b.pairs.filter { it.id in l.unreachable },
            checks = l.checks,
            checking = l.checking,
            checkedAt = l.checkedAt,
            summary = sum,
            devicesOpen = dev.first,
            devices = dev.second,
        )
    }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), DiagnosticsUiState())

    /** New log lines arrive while a sync runs; reload at most every 2 s instead of on every insert. */
    private val eventsTick: Flow<Long> = db.events().observeRecent(1)
        .map { it.firstOrNull()?.id ?: 0L }
        .distinctUntilChanged()
        .sample(2_000)
        .onStart { emit(-1L) }

    /** The last 24 hours of the log with the current filters; null until the first load. */
    val log: StateFlow<List<LogLine>?> = combine(since, _filter, eventsTick) { s, f, _ -> s to f }
        .mapLatest { (s, f) -> withContext(Dispatchers.IO) { logExport.lines(since = s, pairId = f.pairId, errorsOnly = f.errorsOnly) } }
        .catch { emit(emptyList()) }
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    init {
        refreshLocal()
        runChecks()
    }

    // ---------------------------------------------------------------------------------- checks

    /** Cheap checks on the phone itself: permissions, network, the instant-sync service, pair folders. Runs on every resume. */
    fun refreshLocal() {
        viewModelScope.launch(Dispatchers.IO) { refreshLocalNow() }
    }

    private suspend fun refreshLocalNow() {
        val device = DeviceChecks.status(context)
        val network = runCatching { conditions.device() }.getOrNull()
        val unreachable = db.pairs().getAll().filter { it.enabled && !reachable(it.localPath) }.map { it.id }.toSet()
        local.update { it.copy(device = device, network = network, monitorRunning = MonitorService.running, unreachable = unreachable) }
    }

    private fun reachable(path: String): Boolean = runCatching { File(path).let { it.isDirectory && it.canRead() } }.getOrDefault(false)

    /** Everything, including a silent call to Drive per account that renews sign-in status and storage numbers. */
    fun runChecks() {
        if (local.value.checking) return
        local.update { it.copy(checking = true) }
        since.value = System.currentTimeMillis() - DAY_MS
        viewModelScope.launch(Dispatchers.IO) {
            try {
                refreshLocalNow()
                val results = HashMap<String, AccountCheck>()
                for (a in db.accounts().getAll()) results[a.id] = checkAccount(a.id)
                local.update { it.copy(checks = it.checks + results, checking = false, checkedAt = System.currentTimeMillis()) }
            } catch (e: CancellationException) {
                local.update { it.copy(checking = false) }
                throw e
            } catch (t: Throwable) {
                local.update { it.copy(checking = false, checkedAt = System.currentTimeMillis()) }
                _messages.tryEmit(AppError.wrap(t).userHint)
            }
        }
    }

    private suspend fun checkAccount(email: String): AccountCheck {
        val now = System.currentTimeMillis()
        return try {
            val about = api.about(email)
            db.accounts().setQuota(email, about.storageQuota?.usage?.toLongOrNull(), about.storageQuota?.limit?.toLongOrNull())
            db.accounts().setAuthError(email, null)
            AccountCheck(ok = true, at = now)
        } catch (e: CancellationException) {
            throw e
        } catch (t: Throwable) {
            val err = AppError.wrap(t)
            if (err is AppError.Auth) db.accounts().setAuthError(email, err.message ?: Strings.get(R.string.diagnostics_sign_in_again))
            AccountCheck(ok = false, at = now, auth = err is AppError.Auth, hint = err.userHint)
        }
    }

    /** Fix for Background work: schedule the periodic check again and restart instant sync if it should run. */
    fun rearm() {
        viewModelScope.launch {
            val ok = runCatching { scheduler.ensurePeriodic() }.isSuccess
            MonitorService.startIfEnabled(context)
            _messages.tryEmit(Strings.get(if (ok) R.string.diagnostics_rearm_ok else R.string.diagnostics_rearm_failed))
            delay(1_500)
            refreshLocalNow()
        }
    }

    // ---------------------------------------------------------------------------------- log

    fun showAll() { _filter.value = LogFilter() }
    fun toggleErrors() { _filter.update { it.copy(errorsOnly = !it.errorsOnly) } }
    fun togglePair(pairId: String) { _filter.update { it.copy(pairId = if (it.pairId == pairId) null else pairId) } }

    /** Builds the export (file names hidden unless [includeNames]) and hands back the share-sheet intent. */
    fun exportLog(includeNames: Boolean, onReady: (Intent) -> Unit) {
        viewModelScope.launch {
            try {
                val text = withContext(Dispatchers.IO) { capForShare(logExport.export(since = since.value, includeNames = includeNames)) }
                onReady(logExport.shareIntent(text))
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _messages.tryEmit(AppError.wrap(t).userHint)
            }
        }
    }

    fun say(text: String) { _messages.tryEmit(text) }
    fun say(@StringRes text: Int) { say(Strings.get(text)) }

    companion object {
        const val DAY_MS = 24L * 60 * 60 * 1000

        /** A share intent carries the text through a Binder transaction (about 1 MB); keep the newest lines. */
        private const val MAX_SHARE_CHARS = 180_000

        internal fun capForShare(text: String): String {
            if (text.length <= MAX_SHARE_CHARS) return text
            val headerEnd = text.indexOf("\n\n").let { if (it < 0) 0 else it + 2 }
            val header = text.substring(0, headerEnd)
            val tail = text.takeLast(MAX_SHARE_CHARS - header.length - 80)
            val firstFull = tail.indexOf('\n').let { if (it < 0) 0 else it + 1 }
            return header + "Earlier lines left out to fit the share sheet.\n\n" + tail.substring(firstFull)
        }
    }
}
