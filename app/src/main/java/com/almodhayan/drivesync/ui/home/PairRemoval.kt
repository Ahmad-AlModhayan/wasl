package com.almodhayan.drivesync.ui.home

import com.almodhayan.drivesync.actions.PairActions
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.engine.EngineData
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import java.util.concurrent.ConcurrentHashMap
import javax.inject.Inject
import javax.inject.Singleton

/**
 * "Remove pair" with Undo. The pair disappears from every screen at once and stops syncing; the
 * engine removes it (with its baseline, history and conflicts) only when the Undo window closes, so
 * Undo brings it back exactly as it was. Files on the phone and on Drive are never touched. If the
 * app is closed within the window the pair simply stays.
 */
@Singleton
class PairRemoval @Inject constructor(
    private val db: Db,
    private val engine: EngineData,
    private val actions: PairActions,
) {
    class Removed(val pair: FolderPairEntity, val removedAt: Long)

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val deletions = ConcurrentHashMap<String, Job>()

    private val _pending = MutableStateFlow<Removed?>(null)
    val pending: StateFlow<Removed?> = _pending

    /** Hides and stops the pair; returns the Undo token, or null when it no longer exists. */
    suspend fun remove(pairId: String): Removed? {
        val pair = db.pairs().get(pairId) ?: return null
        actions.cancel(pairId)
        engine.hide(pairId)
        val removed = Removed(pair, System.currentTimeMillis())
        deletions[pairId] = scope.launch {
            delay(UNDO_WINDOW_MS)
            actions.remove(pairId)
            engine.unhide(pairId)
            deletions.remove(pairId)
        }
        _pending.value = removed
        return removed
    }

    /** Undo: the pair comes back exactly as it was, with its baseline. */
    suspend fun restore(r: Removed) {
        deletions.remove(r.pair.id)?.cancel()
        engine.unhide(r.pair.id)
        consume(r)
    }

    /** The Undo offer for [r] was shown (or expired). */
    fun consume(r: Removed) {
        _pending.compareAndSet(r, null)
    }

    companion object {
        /** How long a removed pair can still be brought back. */
        const val UNDO_WINDOW_MS = 15_000L
    }
}
