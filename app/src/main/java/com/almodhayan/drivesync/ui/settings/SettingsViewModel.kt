package com.almodhayan.drivesync.ui.settings

import android.content.Context
import androidx.compose.runtime.Immutable
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.actions.PairActions
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.prefs.AppSettings
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import com.almodhayan.drivesync.drive.DriveAuth
import com.almodhayan.drivesync.scheduler.SyncScheduler
import dagger.hilt.android.lifecycle.HiltViewModel
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import javax.inject.Inject

/** One Google account as the Settings account card shows it. */
@Immutable
data class AccountSummary(
    val email: String,
    val name: String?,
    val storageLimit: Long?,
    val storageUsed: Long?,
    /** Bytes the account's pairs keep in Drive (sum of the sync baseline's Drive sizes). */
    val pairBytes: Long,
    val pairCount: Int,
    val needsSignIn: Boolean,
) {
    val initial: String get() = ((name?.trim()?.firstOrNull() ?: email.firstOrNull()) ?: '?').uppercaseChar().toString()
}

/** A snackbar request. [undo] is set for reversible actions. */
class SettingsMessage(val text: String, val undo: (() -> Unit)? = null)

@HiltViewModel
class SettingsViewModel @Inject constructor(
    @ApplicationContext private val context: Context,
    private val repo: SettingsRepository,
    private val scheduler: SyncScheduler,
    private val db: Db,
    private val pairActions: PairActions,
    private val auth: DriveAuth,
) : ViewModel() {

    val settings: StateFlow<AppSettings?> = repo.settings.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    val accounts: StateFlow<List<AccountSummary>> = combine(
        db.accounts().observeAll(), db.pairs().observeAll(), db.fileStates().observePairTotals(),
    ) { accounts, pairs, totals ->
        val bytesByPair = totals.associate { it.pairId to it.remoteBytes }
        accounts.map { a ->
            val mine = pairs.filter { it.accountId == a.id }
            AccountSummary(
                email = a.id,
                name = a.displayName?.takeIf { it.isNotBlank() },
                storageLimit = a.storageLimit,
                storageUsed = a.storageUsed,
                pairBytes = mine.sumOf { bytesByPair[it.id] ?: 0L },
                pairCount = mine.size,
                needsSignIn = !a.authError.isNullOrBlank(),
            )
        }
    }.flowOn(Dispatchers.Default).stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), emptyList())

    val pairCount: StateFlow<Int> = db.pairs().observeAll().map { it.size }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), 0)

    val allPaused: StateFlow<Boolean> = pairActions.observeAllPaused().stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), false)

    private val _messages = MutableSharedFlow<SettingsMessage>(extraBufferCapacity = 8)
    val messages: SharedFlow<SettingsMessage> = _messages

    /** Serializes read-modify-write updates so two quick toggles cannot overwrite each other. */
    private val writeLock = Mutex()

    fun update(block: (AppSettings) -> AppSettings) {
        viewModelScope.launch { writeLock.withLock { repo.update(block) } }
    }

    fun setInterval(minutes: Int) {
        viewModelScope.launch {
            writeLock.withLock { repo.update { it.copy(globalIntervalMinutes = minutes) } }
            runCatching { scheduler.reschedulePeriodic() }
        }
    }

    fun setInstantMonitoring(enabled: Boolean) {
        viewModelScope.launch {
            // The status service keeps running either way; it starts or stops its folder watchers itself.
            writeLock.withLock { repo.update { it.copy(instantMonitoring = enabled) } }
        }
    }

    /** Pause all syncing on/off, with an Undo that puts every pair back exactly as it was. */
    fun setAllPaused(pause: Boolean) {
        viewModelScope.launch(Dispatchers.IO) {
            try {
                val before = db.pairs().getAll()
                if (before.isEmpty()) return@launch
                if (pause) {
                    pairActions.pauseAll()
                    _messages.emit(SettingsMessage(Strings.plural(R.plurals.settings_paused_pairs, before.size)) { restore(before) })
                } else {
                    val paused = before.filter { it.paused }
                    pairActions.resumeAll()
                    _messages.emit(SettingsMessage(Strings.plural(R.plurals.settings_resumed_pairs, paused.size)) { restore(before) })
                }
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _messages.emit(SettingsMessage(AppError.wrap(t).userHint))
            }
        }
    }

    private fun restore(before: List<FolderPairEntity>) {
        viewModelScope.launch(Dispatchers.IO) {
            try {
                val now = db.pairs().getAll().associateBy { it.id }
                for (p in before) {
                    val cur = now[p.id] ?: continue
                    when {
                        p.paused && (!cur.paused || cur.pauseReason != p.pauseReason) -> pairActions.pause(p.id, p.pauseReason ?: PairActions.PAUSED_BY_YOU)
                        !p.paused && cur.paused -> pairActions.resume(p.id)
                    }
                }
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _messages.emit(SettingsMessage(AppError.wrap(t).userHint))
            }
        }
    }

    /**
     * Disconnects a Google account the way the Accounts screen removes one: every pair of the account
     * (with its sync records), the cached Drive index, the account and its token. Files are untouched.
     */
    fun disconnect(email: String) {
        viewModelScope.launch(Dispatchers.IO) {
            try {
                val pairs = db.pairs().byAccount(email)
                for (p in pairs) pairActions.remove(p.id)
                db.nodes().clearAccount(email)
                db.accounts().delete(email)
                auth.invalidate(email)
                _messages.emit(SettingsMessage(if (pairs.isEmpty()) Strings.get(R.string.settings_disconnected, email) else Strings.plural(R.plurals.settings_disconnected_and_removed, pairs.size, email, pairs.size)))
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _messages.emit(SettingsMessage(AppError.wrap(t).userHint))
            }
        }
    }
}
