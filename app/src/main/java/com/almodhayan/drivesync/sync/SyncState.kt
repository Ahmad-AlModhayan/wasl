package com.almodhayan.drivesync.sync

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import java.util.concurrent.ConcurrentHashMap
import javax.inject.Inject
import javax.inject.Singleton

enum class Phase { PREPARING, LISTING_REMOTE, SCANNING_LOCAL, PLANNING, TRANSFERRING, DELETING, FINISHING }

data class PairProgress(
    val pairId: String,
    val runId: String,
    val phase: Phase = Phase.PREPARING,
    val totalActions: Int = 0,
    val doneActions: Int = 0,
    val bytesTotal: Long = 0,
    val bytesDone: Long = 0,
    val currentFile: String? = null,
    val startedAt: Long = System.currentTimeMillis(),
    val errors: Int = 0,
    /** Files compared by content while planning; it moves while a big first sync is being checked. */
    val checked: Int = 0,
) {
    val fraction: Float get() = when {
        bytesTotal > 0 -> (bytesDone.toFloat() / bytesTotal).coerceIn(0f, 1f)
        totalActions > 0 -> (doneActions.toFloat() / totalActions).coerceIn(0f, 1f)
        else -> 0f
    }
}

/** Live progress of running pairs, observed by the UI, the notification and the widget. */
@Singleton
class SyncStateHolder @Inject constructor() {
    private val _running = MutableStateFlow<Map<String, PairProgress>>(emptyMap())
    val running: StateFlow<Map<String, PairProgress>> = _running
    private val followUps = ConcurrentHashMap.newKeySet<String>()

    /** A change arrived while [pairId] was syncing: run it once more when this run ends. */
    fun requestFollowUp(pairId: String) { followUps += pairId }
    fun takeFollowUp(pairId: String): Boolean = followUps.remove(pairId)

    fun start(p: PairProgress) = _running.update { it + (p.pairId to p) }
    fun update(pairId: String, f: (PairProgress) -> PairProgress) = _running.update { m -> m[pairId]?.let { m + (pairId to f(it)) } ?: m }
    fun finish(pairId: String) = _running.update { it - pairId }
    fun isRunning(pairId: String) = _running.value.containsKey(pairId)
    val anyRunning: Boolean get() = _running.value.isNotEmpty()
}
