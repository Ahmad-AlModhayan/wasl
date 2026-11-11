package com.almodhayan.drivesync.ui.accounts

import android.app.Activity
import android.app.PendingIntent
import android.content.Intent
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.almodhayan.drivesync.backup.ImportPlanner
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.db.AccountEntity
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.drive.DriveApi
import com.almodhayan.drivesync.drive.DriveAuth
import com.almodhayan.drivesync.drive.DriveNodeCache
import com.almodhayan.drivesync.scheduler.SyncScheduler
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import javax.inject.Inject

data class AccountRow(
    val account: AccountEntity,
    val pairCount: Int,
    /** What this account's pairs hold on Drive (sum of the pairs' baselines). */
    val pairBytes: Long,
    /** A crawl started by this screen is running right now. */
    val indexing: Boolean,
    val indexError: String?,
    val busy: Boolean,
)

data class AccountsUiState(
    val loading: Boolean = true,
    val rows: List<AccountRow> = emptyList(),
    val signingIn: Boolean = false,
)

/** Google consent screen the UI must launch with StartIntentSenderForResult. */
data class PendingResolution(val email: String?, val intent: PendingIntent)

@HiltViewModel
class AccountsViewModel @Inject constructor(
    private val db: Db,
    private val auth: DriveAuth,
    private val api: DriveApi,
    private val cache: DriveNodeCache,
    private val scheduler: SyncScheduler,
) : ViewModel() {

    private val indexing = MutableStateFlow<Set<String>>(emptySet())
    private val indexErrors = MutableStateFlow<Map<String, String>>(emptyMap())
    private val busy = MutableStateFlow<Set<String>>(emptySet())
    private val signingIn = MutableStateFlow(false)

    private val stored = combine(db.accounts().observeAll(), db.pairs().observeAll(), db.fileStates().observePairTotals()) { accounts, pairs, totals ->
        val bytesByPair = totals.associate { it.pairId to it.remoteBytes }
        accounts.map { a ->
            val mine = pairs.filter { it.accountId == a.id }
            Triple(a, mine.size, mine.sumOf { bytesByPair[it.id] ?: 0L })
        }
    }

    val state: StateFlow<AccountsUiState> = combine(stored, indexing, indexErrors, busy, signingIn) { accounts, indexingSet, errors, busySet, s ->
        val rows = accounts.map { (a, pairCount, pairBytes) ->
            AccountRow(a, pairCount, pairBytes, a.id in indexingSet, errors[a.id], a.id in busySet)
        }
        AccountsUiState(loading = false, rows = rows, signingIn = s)
    }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), AccountsUiState())

    /** Short messages for a snackbar. */
    private val _messages = MutableSharedFlow<String>(extraBufferCapacity = 16)
    val messages: SharedFlow<String> = _messages

    private val _resolution = MutableStateFlow<PendingResolution?>(null)
    val resolution: StateFlow<PendingResolution?> = _resolution

    /** Configuration problem (DEVELOPER_ERROR) shown as a banner instead of a snackbar. */
    private val _setupError = MutableStateFlow<String?>(null)
    val setupError: StateFlow<String?> = _setupError
    fun dismissSetupError() { _setupError.value = null }

    private var pendingEmail: String? = null

    // ------------------------------------------------------------------ add / re-authorize

    fun addAccount() = startAuthorization(null)
    fun reauthorize(email: String) = startAuthorization(email)

    private fun startAuthorization(email: String?) {
        if (signingIn.value) return
        signingIn.value = true
        viewModelScope.launch {
            try {
                when (val outcome = withContext(Dispatchers.IO) { auth.authorize(email) }) {
                    is DriveAuth.Outcome.Token -> { completeSignIn(email, outcome.accessToken); signingIn.value = false }
                    is DriveAuth.Outcome.NeedsResolution -> { pendingEmail = email; _resolution.value = PendingResolution(email, outcome.pendingIntent) }
                }
            } catch (t: Throwable) {
                if (t is CancellationException) throw t
                signingIn.value = false
                reportAuthFailure(t)
            }
        }
    }

    /** The UI calls this right after launching the consent intent so it is not launched twice. */
    fun resolutionLaunched() { _resolution.value = null }

    fun onResolutionLaunchFailed(@Suppress("UNUSED_PARAMETER") t: Throwable) {
        pendingEmail = null
        signingIn.value = false
        _messages.tryEmit(Strings.get(R.string.accounts_msg_signin_screen_failed))
    }

    /** Result of the consent screen launched with StartIntentSenderForResult. */
    fun onAuthResult(resultCode: Int, data: Intent?) {
        val email = pendingEmail
        pendingEmail = null
        if (resultCode != Activity.RESULT_OK) {
            signingIn.value = false
            _messages.tryEmit(Strings.get(R.string.accounts_msg_signin_cancelled))
            return
        }
        viewModelScope.launch {
            try {
                when (val outcome = auth.resultFromIntent(email, data)) {
                    is DriveAuth.Outcome.Token -> completeSignIn(email, outcome.accessToken)
                    is DriveAuth.Outcome.NeedsResolution -> {
                        pendingEmail = email
                        _resolution.value = PendingResolution(email, outcome.pendingIntent)
                        return@launch
                    }
                }
            } catch (t: Throwable) {
                if (t is CancellationException) throw t
                reportAuthFailure(t)
            }
            signingIn.value = false
        }
    }

    /** Token in hand: resolve the email, persist the account, start the initial Drive crawl, arm the scheduler. */
    private suspend fun completeSignIn(expectedEmail: String?, token: String) = withContext(Dispatchers.IO) {
        val about = api.aboutWithToken(token)
        val email = about.user?.emailAddress?.trim()?.takeIf { it.isNotEmpty() }
            ?: throw AppError.Auth(Strings.get(R.string.accounts_no_email))
        auth.cache(email, token)
        val existing = db.accounts().get(email)
        val entity = (existing ?: AccountEntity(id = email)).copy(
            displayName = about.user?.displayName ?: existing?.displayName,
            photoUrl = about.user?.photoLink ?: existing?.photoUrl,
            storageUsed = about.storageQuota?.usage?.toLongOrNull() ?: existing?.storageUsed,
            storageLimit = about.storageQuota?.limit?.toLongOrNull(),
            authError = null,
        )
        db.accounts().upsert(entity)
        // Pairs restored from a configuration backup wait (disabled + paused) until their account signs in.
        val restoreReason = ImportPlanner.pauseReasonForMissingAccount(email)
        val resumed = db.pairs().byAccount(email).filter { it.pauseReason == restoreReason }
        resumed.forEach {
            db.pairs().setEnabled(it.id, true)
            db.pairs().setPaused(it.id, false, null)
        }
        indexErrors.update { it - email }
        startIndexing(email)
        runCatching { scheduler.ensurePeriodic() }
        val mismatch = expectedEmail != null && !expectedEmail.equals(email, ignoreCase = true)
        val resumedNote = if (resumed.isEmpty()) null else Strings.plural(R.plurals.accounts_msg_restored_pairs, resumed.size, resumed.size)
        val signedIn = when {
            mismatch -> Strings.get(R.string.accounts_msg_added_other, email, expectedEmail ?: "")
            existing == null -> Strings.get(R.string.accounts_msg_added, email)
            else -> Strings.get(R.string.accounts_msg_signed_in_again, email)
        }
        _messages.tryEmit(listOfNotNull(signedIn, resumedNote).joinToString(" "))
    }

    // ------------------------------------------------------------------ indexing

    /** Initial (or repeated) crawl of the Drive metadata cache. The row shows "Indexing Drive…" while it runs. */
    fun startIndexing(email: String) {
        if (email in indexing.value) return
        indexing.update { it + email }
        viewModelScope.launch(Dispatchers.IO) {
            try {
                cache.refresh(email)
                indexErrors.update { it - email }
            } catch (t: Throwable) {
                if (t is CancellationException) {
                    // The screen was closed mid-crawl: finish it in the background instead of losing it.
                    scheduler.indexAccount(email)
                    throw t
                }
                val err = AppError.wrap(t)
                indexErrors.update { it + (email to err.userHint) }
                if (err is AppError.Auth) db.accounts().setAuthError(email, err.message ?: Strings.get(R.string.accounts_reauth_required))
            } finally {
                indexing.update { it - email }
            }
        }
    }

    // ------------------------------------------------------------------ refresh / remove

    /** Re-reads quota and profile from Drive. An expired sign-in flips the row into the auth-error state. */
    fun refresh(email: String) = withBusy(email) {
        try {
            val about = api.about(email)
            db.accounts().setQuota(email, about.storageQuota?.usage?.toLongOrNull(), about.storageQuota?.limit?.toLongOrNull())
            db.accounts().setAuthError(email, null)
            val a = db.accounts().get(email)
            val name = about.user?.displayName
            if (a != null && name != null && name != a.displayName) {
                db.accounts().upsert(a.copy(displayName = name, photoUrl = about.user?.photoLink ?: a.photoUrl))
            }
            _messages.tryEmit(Strings.get(R.string.accounts_msg_updated, email))
        } catch (t: Throwable) {
            if (t is CancellationException) throw t
            val err = AppError.wrap(t)
            if (err is AppError.Auth) {
                db.accounts().setAuthError(email, err.message ?: Strings.get(R.string.accounts_reauth_required))
                val res = err.resolution
                if (res != null) {
                    pendingEmail = email
                    signingIn.value = true
                    _resolution.value = PendingResolution(email, res)
                    return@withBusy
                }
            }
            _messages.tryEmit(err.userHint)
        }
    }

    /**
     * Withdraws Wasl's access at Google (best effort, S2-4), then deletes the account, all of its
     * folder pairs (baseline, conflicts, blocked plans) and the cached Drive index. Files are
     * untouched. A failed revocation never keeps the account: the message says so, and where to
     * withdraw it by hand.
     */
    fun remove(email: String) = withBusy(email) {
        val revoked = auth.revoke(email)
        val pairs = db.pairs().byAccount(email)
        for (p in pairs) {
            db.fileStates().clearPair(p.id)
            db.conflicts().clearPair(p.id)
            db.blockedPlans().delete(p.id)
            db.pairs().delete(p.id)
        }
        db.nodes().clearAccount(email)
        db.accounts().delete(email)
        auth.invalidate(email)
        indexErrors.update { it - email }
        val access = Strings.get(if (revoked) R.string.accounts_msg_access_revoked else R.string.accounts_msg_access_not_revoked)
        _messages.tryEmit(Strings.get(R.string.accounts_msg_disconnected, email) + " " + access)
    }

    private fun withBusy(email: String, block: suspend () -> Unit) {
        if (email in busy.value) return
        busy.update { it + email }
        viewModelScope.launch(Dispatchers.IO) {
            try { block() } finally { busy.update { it - email } }
        }
    }

    private fun reportAuthFailure(t: Throwable) {
        val err = AppError.wrap(t)
        val text = if (err is AppError.Auth) (err.message ?: err.userHint) else err.userHint
        if (text.contains("DEVELOPER_ERROR")) _setupError.value = text else _messages.tryEmit(text)
    }
}
