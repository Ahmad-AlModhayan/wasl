package com.almodhayan.drivesync.ui.vaults

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import androidx.annotation.StringRes
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.crypto.VaultManager
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.VaultEntity
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import javax.inject.Inject
import kotlin.math.ln

data class VaultRow(val vault: VaultEntity, val unlocked: Boolean, val pairsUsing: Int)

data class VaultsUiState(
    val loading: Boolean = true,
    val rows: List<VaultRow> = emptyList(),
    val busy: Boolean = false,
    val message: String? = null,
)

/** Completion callback for dialog actions: null = success, otherwise the text to show inside the dialog. */
typealias ActionResult = (error: String?) -> Unit

@HiltViewModel
class VaultsViewModel @Inject constructor(
    private val vaults: VaultManager,
    db: Db,
) : ViewModel() {
    private val busy = MutableStateFlow(false)
    private val message = MutableStateFlow<String?>(null)

    val state: StateFlow<VaultsUiState> = combine(vaults.observeVaults(), db.pairs().observeAll(), vaults.unlockedIds, busy, message) { list, pairs, unlocked, b, m ->
        VaultsUiState(
            loading = false,
            rows = list.map { v -> VaultRow(v, v.id in unlocked, pairs.count { it.vaultId == v.id }) },
            busy = b,
            message = m,
        )
    }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), VaultsUiState())

    init {
        viewModelScope.launch { runCatching { vaults.refreshUnlocked() } }
    }

    fun dismissMessage() { message.value = null }

    fun create(name: String, passphrase: String, onResult: ActionResult) = act(R.string.vaults_failed_create, onResult) {
        val v = vaults.createVault(name, passphrase)
        message.value = Strings.get(R.string.vaults_msg_created, v.name)
    }

    fun unlock(vaultId: String, passphrase: String, onResult: ActionResult) = act(R.string.vaults_failed_unlock, onResult) {
        if (!vaults.unlock(vaultId, passphrase)) throw ActionFailed(Strings.get(R.string.vaults_wrong_passphrase))
    }

    fun lock(vaultId: String) = act(R.string.vaults_failed_lock, null) {
        vaults.lock(vaultId)
        message.value = Strings.get(R.string.vaults_msg_locked)
    }

    fun delete(vaultId: String, onResult: ActionResult) = act(R.string.vaults_failed_delete, onResult) {
        vaults.deleteVault(vaultId)
        message.value = Strings.get(R.string.vaults_msg_deleted)
    }

    fun join(name: String, manifestJson: String, passphrase: String, onResult: ActionResult) = act(R.string.vaults_failed_join, onResult) {
        // Text copied from a chat, a note or a file viewer often carries words around the JSON
        // (E2E 2026-10: a stray " T5" after the closing brace made Join do nothing visible).
        val start = manifestJson.indexOf('{')
        val end = manifestJson.lastIndexOf('}')
        val json = if (start >= 0 && end > start) manifestJson.substring(start, end + 1) else manifestJson
        val v = vaults.importFromManifest(name, json, passphrase) ?: throw ActionFailed(Strings.get(R.string.vaults_wrong_passphrase_join))
        message.value = Strings.get(R.string.vaults_msg_joined, v.name)
    }

    fun manifest(vaultId: String, onResult: (text: String?, error: String?) -> Unit) {
        viewModelScope.launch {
            try {
                onResult(vaults.exportManifest(vaultId), null)
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                onResult(null, errorText(t))
            }
        }
    }

    /** Message for the dialog that started the action (not wrapped as an error hint). */
    private class ActionFailed(message: String) : Exception(message)

    private fun act(@StringRes failed: Int, onResult: ActionResult?, block: suspend () -> Unit) {
        viewModelScope.launch {
            busy.value = true
            val error: String? = try {
                block(); null
            } catch (e: CancellationException) {
                throw e
            } catch (e: ActionFailed) {
                e.message
            } catch (t: Throwable) {
                errorText(t)
            } finally {
                busy.value = false
            }
            if (onResult != null) onResult(error)
            else if (error != null) message.value = Strings.get(failed, error)
        }
    }

    private fun errorText(t: Throwable): String = when (t) {
        is AppError -> t.userHint
        else -> AppError.wrap(t).userHint
    }
}

/** Rough passphrase strength estimate for the live hint: entropy from length x alphabet size. */
object PassphraseStrength {
    const val MIN_LENGTH = VaultManager.MIN_PASSPHRASE_LENGTH

    enum class Level(@StringRes val labelRes: Int, val fraction: Float) {
        TOO_SHORT(R.string.vaults_strength_too_short, 0.1f), WEAK(R.string.vaults_strength_weak, 0.3f), FAIR(R.string.vaults_strength_fair, 0.55f),
        GOOD(R.string.vaults_strength_good, 0.8f), STRONG(R.string.vaults_strength_strong, 1f);

        val label: String get() = Strings.get(labelRes)
    }

    fun bits(p: String): Double {
        if (p.isEmpty()) return 0.0
        var alphabet = 0
        if (p.any { it.isLowerCase() }) alphabet += 26
        if (p.any { it.isUpperCase() }) alphabet += 26
        if (p.any { it.isDigit() }) alphabet += 10
        if (p.any { !it.isLetterOrDigit() && !it.isWhitespace() }) alphabet += 33
        if (p.any { it.isWhitespace() }) alphabet += 1
        if (p.any { it.code > 127 }) alphabet += 100
        if (alphabet == 0) alphabet = 1
        val raw = p.length * ln(alphabet.toDouble()) / ln(2.0)
        val distinct = p.toSet().size
        return if (distinct < p.length / 2) raw * 0.6 else raw   // heavy repetition
    }

    fun level(p: String): Level {
        if (p.length < MIN_LENGTH) return Level.TOO_SHORT
        val b = bits(p)
        return when {
            b < 40 -> Level.WEAK
            b < 60 -> Level.FAIR
            b < 80 -> Level.GOOD
            else -> Level.STRONG
        }
    }

    fun hint(p: String): String = when (level(p)) {
        Level.TOO_SHORT -> Strings.get(R.string.vaults_hint_too_short, MIN_LENGTH)
        Level.WEAK -> Strings.get(R.string.vaults_hint_weak)
        Level.FAIR -> Strings.get(R.string.vaults_hint_fair)
        Level.GOOD -> Strings.get(R.string.vaults_hint_good)
        Level.STRONG -> Strings.get(R.string.vaults_hint_strong)
    }
}
