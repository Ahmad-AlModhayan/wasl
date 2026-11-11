package com.almodhayan.drivesync.scheduler

import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.sync.SyncStateHolder
import dagger.hilt.android.AndroidEntryPoint
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.launch
import javax.inject.Inject

@AndroidEntryPoint
class SyncTileService : TileService() {
    @Inject lateinit var scheduler: SyncScheduler
    @Inject lateinit var state: SyncStateHolder
    private var scope: CoroutineScope? = null

    override fun onStartListening() {
        val s = CoroutineScope(SupervisorJob() + Dispatchers.Main)
        scope = s
        s.launch { state.running.collect { render(it.isNotEmpty()) } }
    }

    override fun onStopListening() { scope?.cancel(); scope = null }

    override fun onClick() {
        if (state.anyRunning) scheduler.cancelAll() else scheduler.syncNow(null, "tile", manual = true)
    }

    private fun render(running: Boolean) {
        val t = qsTile ?: return
        t.state = if (running) Tile.STATE_ACTIVE else Tile.STATE_INACTIVE
        t.label = Strings.get(R.string.app_name)
        t.subtitle = Strings.get(if (running) R.string.notif_tile_syncing else R.string.notif_tile_idle)
        t.updateTile()
    }
}
