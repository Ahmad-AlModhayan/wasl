package com.almodhayan.drivesync.scheduler

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log
import androidx.work.WorkManager
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import dagger.hilt.android.AndroidEntryPoint
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import javax.inject.Inject

/**
 * Automation surface (Tasker, MacroDroid, adb), plus boot. Exported, so every command is checked by
 * [AutomationGate]: Settings › Allow automation must be on and the broadcast must carry the secret.
 *   am broadcast -a com.kahflabs.wasl.SYNC_ALL --es secret <secret> -n com.kahflabs.wasl/.scheduler.SyncIntentReceiver
 *   am broadcast -a com.kahflabs.wasl.SYNC_PAIR --es pairId <id> --es secret <secret> -n ...
 *   am broadcast -a com.kahflabs.wasl.PAUSE_ALL / RESUME_ALL / CANCEL / RESCHEDULE --es secret <secret> -n ...
 */
@AndroidEntryPoint
class SyncIntentReceiver : BroadcastReceiver() {
    @Inject lateinit var scheduler: SyncScheduler
    @Inject lateinit var settings: SettingsRepository
    @Inject lateinit var db: Db
    @Inject lateinit var pairActions: com.almodhayan.drivesync.actions.PairActions

    override fun onReceive(context: Context, intent: Intent) {
        val pending = goAsync()
        CoroutineScope(Dispatchers.IO).launch {
            try {
                val action = intent.action
                val cfg = settings.current()
                if (!AutomationGate.allows(action, intent.getStringExtra(AutomationGate.EXTRA_SECRET), cfg.automationEnabled, cfg.automationSecret)) {
                    Log.w(TAG, "Ignored $action: automation is off or the secret does not match")
                    return@launch
                }
                when (action) {
                    // Automation runs keep each pair's Wi-Fi, VPN, roaming and battery rules and skip paused pairs.
                    ACTION_SYNC_ALL -> scheduler.syncNow(null, "intent", manual = false)
                    ACTION_SYNC_PAIR -> intent.getStringExtra("pairId")?.let { scheduler.syncNow(it, "intent", manual = false) }
                    ACTION_PAUSE_ALL -> db.pairs().getAll().forEach { pairActions.pause(it.id, Strings.get(R.string.notif_paused_by_automation)) }
                    ACTION_RESUME_ALL -> pairActions.resumeAll()
                    ACTION_CANCEL -> WorkManager.getInstance(context).cancelAllWorkByTag(SyncScheduler.TAG_SYNC).also { scheduler.ensurePeriodic() }
                    ACTION_RESCHEDULE -> scheduler.ensurePeriodic()
                    in AutomationGate.SYSTEM_ACTIONS -> {
                        scheduler.ensurePeriodic()
                        MonitorService.startIfEnabled(context)
                    }
                }
            } finally { pending.finish() }
        }
    }

    companion object {
        private const val TAG = "SyncIntentReceiver"
        const val ACTION_SYNC_ALL = "com.kahflabs.wasl.SYNC_ALL"
        const val ACTION_SYNC_PAIR = "com.kahflabs.wasl.SYNC_PAIR"
        const val ACTION_PAUSE_ALL = "com.kahflabs.wasl.PAUSE_ALL"
        const val ACTION_RESUME_ALL = "com.kahflabs.wasl.RESUME_ALL"
        const val ACTION_CANCEL = "com.kahflabs.wasl.CANCEL"
        const val ACTION_RESCHEDULE = "com.kahflabs.wasl.RESCHEDULE"
    }
}
