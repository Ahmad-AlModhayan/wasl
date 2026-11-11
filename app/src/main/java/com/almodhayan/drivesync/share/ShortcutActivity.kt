package com.almodhayan.drivesync.share

import android.content.Context
import android.os.Bundle
import android.widget.Toast
import androidx.activity.ComponentActivity
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.scheduler.SyncScheduler
import com.almodhayan.drivesync.widget.WidgetUpdater
import dagger.hilt.android.AndroidEntryPoint
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import javax.inject.Inject

/**
 * Invisible activity behind the static launcher shortcuts (res/xml/shortcuts.xml). Reads the "action"
 * extra, does the work and finishes at once. Uses a translucent theme and is excluded from recents.
 */
@AndroidEntryPoint
class ShortcutActivity : ComponentActivity() {
    @Inject lateinit var scheduler: SyncScheduler
    @Inject lateinit var db: Db

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val app = applicationContext
        when (intent?.getStringExtra(EXTRA_ACTION)) {
            ACTION_SYNC_ALL -> {
                scheduler.syncNow(null, "shortcut", manual = true)
                Toast.makeText(app, app.getString(R.string.share_shortcut_syncing_all), Toast.LENGTH_SHORT).show()
            }
            ACTION_PAUSE_ALL -> {
                val reason = app.getString(R.string.share_shortcut_paused_reason)
                updatePairs(app, app.getString(R.string.share_shortcut_paused_all)) { p -> db.pairs().setPaused(p.id, true, reason) }
            }
            ACTION_RESUME_ALL -> updatePairs(app, app.getString(R.string.share_shortcut_resumed_all)) { p -> db.pairs().setPaused(p.id, false, null) }
            else -> Toast.makeText(app, app.getString(R.string.share_shortcut_unknown), Toast.LENGTH_SHORT).show()
        }
        finish()
    }

    /** Runs after finish(): the database and the toast use the application context, not this activity. */
    private fun updatePairs(app: Context, doneText: String, change: suspend (FolderPairEntity) -> Unit) {
        val database = db
        val failedText = app.getString(R.string.share_shortcut_failed)
        CoroutineScope(Dispatchers.IO).launch {
            val ok = runCatching { database.pairs().getAll().forEach { change(it) } }.isSuccess
            WidgetUpdater.updateAll(app)
            withContext(Dispatchers.Main) {
                Toast.makeText(app, if (ok) doneText else failedText, Toast.LENGTH_SHORT).show()
            }
        }
    }

    companion object {
        const val EXTRA_ACTION = "action"
        const val ACTION_SYNC_ALL = "sync_all"
        const val ACTION_PAUSE_ALL = "pause_all"
        const val ACTION_RESUME_ALL = "resume_all"
    }
}
