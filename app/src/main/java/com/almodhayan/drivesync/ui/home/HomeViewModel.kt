package com.almodhayan.drivesync.ui.home

import com.almodhayan.drivesync.sync.Conditions
import androidx.compose.runtime.Immutable
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.actions.PairActions
import com.almodhayan.drivesync.actions.StorageStats
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.db.AccountEntity
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.ConflictEntity
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.db.PairTotals
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import com.almodhayan.drivesync.sync.PairProgress
import com.almodhayan.drivesync.sync.PlanSummary
import com.almodhayan.drivesync.sync.SyncStateHolder
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.PairCardModel
import com.almodhayan.drivesync.ui.components.PairDirection
import com.almodhayan.drivesync.ui.components.Side
import com.almodhayan.drivesync.ui.components.StorageModel
import com.almodhayan.drivesync.ui.onboarding.DeviceChecks
import com.almodhayan.drivesync.ui.status.PairStatusResolver
import com.almodhayan.drivesync.ui.status.PairStatusUi
import com.almodhayan.drivesync.ui.theme.SyncStatus
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import kotlinx.serialization.json.Json
import java.util.concurrent.ConcurrentHashMap
import javax.inject.Inject

/** One pair as Home shows it: the resolved status, its loud state (if any) and the card model. */
@Immutable
data class HomePair(
    val pair: FolderPairEntity,
    val status: PairStatusUi,
    val loud: LoudSpec?,
    val card: PairCardModel,
)

@Immutable
data class HomeUiState(
    val loading: Boolean = true,
    /** The time the relative times were computed against. */
    val now: Long = 0L,
    val hasAccounts: Boolean = false,
    val pairs: List<HomePair> = emptyList(),
    /** The single banner: the most urgent loud state of the app, else a health problem. */
    val banner: LoudSpec? = null,
    /** Every pair synced and nothing running: Home becomes the summary. */
    val quiet: Boolean = false,
    val checkedAt: Long? = null,
    val phoneStorage: StorageModel? = null,
    val driveStorage: List<StorageModel> = emptyList(),
)

@HiltViewModel
class HomeViewModel @Inject constructor(
    private val db: Db,
    settingsRepo: SettingsRepository,
    syncState: SyncStateHolder,
    private val actions: PairActions,
    private val storageStats: StorageStats,
    private val removal: PairRemoval,
    private val json: Json,
    private val conditions: Conditions,
) : ViewModel() {

    /** All files access is system state, not a Flow: the screen re-checks it on every resume. */
    private val filesAccess = MutableStateFlow(DeviceChecks.hasAllFilesAccess())

    /** StatFs results per folder, refreshed at most every [VOLUME_TTL_MS] (progress ticks rebuild the state often). */
    private val volumes = ConcurrentHashMap<String, Pair<Long, StorageStats.Volume?>>()

    private val blocked: Flow<Map<String, PlanSummary>> = db.pairs().observeAll()
        .map { list -> list.filter { it.pendingApproval }.map { it.id } }
        .distinctUntilChanged()
        .flatMapLatest { ids ->
            if (ids.isEmpty()) flowOf(emptyMap<String, PlanSummary>())
            else combine(ids.map { db.blockedPlans().observe(it) }) { rows ->
                rows.filterNotNull().mapNotNull { b -> decode(b.summaryJson)?.let { b.pairId to it } }.toMap()
            }
        }

    private class Inputs(
        val accounts: List<AccountEntity>,
        val pairs: List<FolderPairEntity>,
        val conflicts: List<ConflictEntity>,
        val totals: List<PairTotals>,
        val intervalMinutes: Int,
    )

    private val inputs: Flow<Inputs> = combine(
        db.accounts().observeAll(),
        db.pairs().observeAll(),
        db.conflicts().observeOpen(),
        db.fileStates().observePairTotals(),
        settingsRepo.settings,
    ) { accounts, pairs, conflicts, totals, settings -> Inputs(accounts, pairs, conflicts, totals, settings.globalIntervalMinutes) }

    /** Keeps "4 min ago" honest while nothing else changes. */
    private val clock: Flow<Long> = flow {
        while (true) {
            emit(System.currentTimeMillis())
            delay(CLOCK_TICK_MS)
        }
    }

    val state: StateFlow<HomeUiState> = combine(inputs, syncState.running, blocked, filesAccess, clock) { i, running, blockedMap, files, now ->
        build(i, running, blockedMap, files, now)
    }
        .flowOn(Dispatchers.IO)
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), HomeUiState())

    /** The newest removed pair, while Undo is still on offer. */
    val pendingUndo: StateFlow<PairRemoval.Removed?> = removal.pending

    private val _messages = MutableSharedFlow<String>(extraBufferCapacity = 8)
    /** Short confirmations and failures for the snackbar. */
    val messages: SharedFlow<String> = _messages

    fun refreshDevice() { filesAccess.value = DeviceChecks.hasAllFilesAccess() }

    fun syncAll() = io { actions.syncAll() }
    fun syncNow(pairId: String) = io { actions.syncNow(pairId) }
    fun pause(pairId: String) = io { actions.pause(pairId) }
    fun resume(pairId: String) = io { actions.resume(pairId) }
    fun turnOn(pairId: String) = io { actions.setEnabled(pairId, true) }

    /** Removes the pair; Home then offers Undo through [pendingUndo]. */
    fun remove(pairId: String) = io { removal.remove(pairId) }
    fun undoRemove(r: PairRemoval.Removed) = io { removal.restore(r) }
    fun consumeUndo(r: PairRemoval.Removed) = removal.consume(r)

    private fun io(block: suspend () -> Unit) {
        viewModelScope.launch(Dispatchers.IO) {
            try {
                block()
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _messages.tryEmit(AppError.wrap(t).userHint)
            }
        }
    }

    // ------------------------------------------------------------------ state

    private fun build(i: Inputs, running: Map<String, PairProgress>, blockedMap: Map<String, PlanSummary>, files: Boolean, now: Long): HomeUiState {
        val conflictsByPair = i.conflicts.groupBy { it.pairId }
        val totalsByPair = i.totals.associateBy { it.pairId }
        val accountsById = i.accounts.associateBy { it.id }

        val items = i.pairs.map { pair ->
            val open = conflictsByPair[pair.id].orEmpty()
            val summary = blockedMap[pair.id]
            val live = if (pair.enabled && !pair.paused) conditions.blockReason(pair) else null
            val st = PairStatusResolver.resolve(pair, running[pair.id], open.size, summary, now, live)
            val card = PairCardModel(
                name = pair.name,
                direction = PairDirection.of(pair.mode),
                status = st.status,
                statusLabel = st.label,
                statusIcon = st.icon,
                conditions = PairStatusResolver.conditions(pair, i.intervalMinutes),
                lastSync = st.lastSync,
                phonePath = Fmt.humanLocalPath(pair.localPath),
                drivePath = Fmt.humanRemotePath(pair.remotePath),
                progress = st.progress,
                storage = pairStorage(pair, totalsByPair[pair.id], accountsById[pair.accountId]),
            )
            HomePair(pair, st, LoudStates.forPair(pair, st, summary, open.size, open.firstOrNull()?.id), card)
        }

        // Health problems get the banner only when no pair is loud.
        val health = buildList {
            // Only pairs on file paths need All files access; SAF pairs have their own grant.
            if (!files && i.pairs.any { !it.localPath.startsWith("content://") }) add(LoudStates.filesAccessMissing())
            i.accounts.filter { !it.authError.isNullOrBlank() }.forEach { a ->
                add(LoudStates.signInNeeded(a, i.pairs.count { it.accountId == a.id }, onlyAccount = i.accounts.size == 1))
            }
        }
        val banner = LoudStates.mostUrgent(items.mapNotNull { it.loud }) ?: health.firstOrNull()
        val quiet = items.isNotEmpty() && banner == null && running.isEmpty() && items.all { it.status.status == SyncStatus.Synced }

        val phone = volumeFor(null)?.let { v ->
            val mine = i.totals.sumOf { it.localBytes }
            StorageModel(Side.Phone, v.total, mine, (v.used - mine).coerceAtLeast(0), pairLabel = Strings.get(R.string.common_your_pairs))
        }
        val drives = i.accounts.mapNotNull { a ->
            val limit = a.storageLimit
            if (limit == null || limit <= 0) return@mapNotNull null
            val mine = i.pairs.filter { it.accountId == a.id }.sumOf { totalsByPair[it.id]?.remoteBytes ?: 0L }
            StorageModel(Side.Drive, limit, mine, ((a.storageUsed ?: 0L) - mine).coerceAtLeast(0), label = if (i.accounts.size > 1) a.id else null, pairLabel = Strings.get(R.string.common_your_pairs))
        }

        return HomeUiState(
            loading = false,
            now = now,
            hasAccounts = i.accounts.isNotEmpty(),
            pairs = items,
            banner = banner,
            quiet = quiet,
            checkedAt = i.pairs.mapNotNull { it.lastRunAt }.maxOrNull(),
            phoneStorage = phone,
            driveStorage = drives,
        )
    }

    /** The side the pair fills: Drive for Phone → Drive, phone for Drive → Phone, both for Two-way. */
    private fun pairStorage(pair: FolderPairEntity, totals: PairTotals?, account: AccountEntity?): List<StorageModel> {
        val direction = PairDirection.of(pair.mode)
        val out = ArrayList<StorageModel>(2)
        if (direction != PairDirection.PhoneToDrive) {
            volumeFor(pair.localPath)?.let { v ->
                val mine = totals?.localBytes ?: 0L
                out += StorageModel(Side.Phone, v.total, mine, (v.used - mine).coerceAtLeast(0))
            }
        }
        if (direction != PairDirection.DriveToPhone && account != null) {
            val limit = account.storageLimit
            if (limit != null && limit > 0) {
                val mine = totals?.remoteBytes ?: 0L
                out += StorageModel(Side.Drive, limit, mine, ((account.storageUsed ?: 0L) - mine).coerceAtLeast(0))
            }
        }
        return out
    }

    /** [path] null = internal storage. */
    private fun volumeFor(path: String?): StorageStats.Volume? {
        val key = path ?: ""
        val now = System.currentTimeMillis()
        volumes[key]?.let { (at, v) -> if (now - at < VOLUME_TTL_MS) return v }
        val v = if (path == null) storageStats.phoneVolume() else storageStats.phoneVolume(path)
        volumes[key] = now to v
        return v
    }

    private fun decode(summaryJson: String): PlanSummary? =
        runCatching { json.decodeFromString(PlanSummary.serializer(), summaryJson) }.getOrNull()

    private companion object {
        const val VOLUME_TTL_MS = 30_000L
        const val CLOCK_TICK_MS = 30_000L
    }
}
