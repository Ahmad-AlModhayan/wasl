package com.almodhayan.drivesync.widget

import android.content.Context
import androidx.glance.appwidget.updateAll
import com.almodhayan.drivesync.data.db.RunStatus
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.conflate
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.merge
import kotlinx.coroutines.launch

/**
 * Refreshes every placed [SyncWidget]. Cheap when no widget exists. Call [updateAll] from
 * SyncWorker after runs, or simply call [start] once from Application.onCreate and it keeps the
 * widget in step with sync progress, pair status and open conflicts on its own.
 */
object WidgetUpdater {
    suspend fun updateAll(context: Context) {
        runCatching { SyncWidget().updateAll(context.applicationContext) }
    }

    /** Fire-and-forget variant for callers without a coroutine. */
    fun requestUpdate(context: Context) {
        val app = context.applicationContext
        CoroutineScope(Dispatchers.Default).launch { updateAll(app) }
    }

    /**
     * Observes [com.almodhayan.drivesync.sync.SyncStateHolder.running] (every 5% of progress),
     * the pairs table (name, last run, status, pending approval) and the open conflict count, and
     * refreshes the widget at most every 1.5 s while anything changes.
     */
    fun start(context: Context, scope: CoroutineScope) {
        val app = context.applicationContext
        val entry = widgetEntryPoint(app)
        scope.launch {
            val progress = entry.syncState().running
                .map { m -> m.values.map { it.pairId to (it.fraction * 20).toInt() } }
                .distinctUntilChanged()
                .map { }
            val pairs = entry.db().pairs().observeAll()
                .map { list -> list.map { PairKey(it.id, it.name, it.lastRunAt, it.lastStatus, it.pendingApproval) } }
                .distinctUntilChanged()
                .map { }
            val conflicts = entry.db().conflicts().observeOpenCount().distinctUntilChanged().map { }
            merge(progress, pairs, conflicts).conflate().collect {
                updateAll(app)
                delay(1_500)
            }
        }
    }

    private data class PairKey(val id: String, val name: String, val lastRunAt: Long?, val lastStatus: RunStatus?, val pendingApproval: Boolean)
}
