package com.almodhayan.drivesync.scheduler

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import dagger.hilt.android.AndroidEntryPoint
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import javax.inject.Inject

/**
 * The app's own actions: notification buttons (Pause, Restore on phone) and the schedule re-arm
 * after a cancel. Not exported, so only this app, or the system firing this app's PendingIntents,
 * can reach it; that is why these need no secret, unlike [SyncIntentReceiver].
 */
@AndroidEntryPoint
class InternalActionReceiver : BroadcastReceiver() {
    @Inject lateinit var scheduler: SyncScheduler
    @Inject lateinit var pairActions: com.almodhayan.drivesync.actions.PairActions

    override fun onReceive(context: Context, intent: Intent) {
        val pending = goAsync()
        CoroutineScope(Dispatchers.IO).launch {
            try {
                when (intent.action) {
                    ACTION_PAUSE_RUNNING -> pairActions.pauseRunning()
                    ACTION_RESTORE_DELETIONS -> intent.getStringExtra("pairId")?.let { RestoreDeletionsWorker.enqueue(context, it) }
                    ACTION_RESCHEDULE -> scheduler.ensurePeriodic()
                    ACTION_STATUS_DISMISSED -> MonitorService.requestRepost()
                }
            } finally { pending.finish() }
        }
    }

    companion object {
        const val ACTION_PAUSE_RUNNING = "com.kahflabs.wasl.internal.PAUSE_RUNNING"
        const val ACTION_RESTORE_DELETIONS = "com.kahflabs.wasl.internal.RESTORE_DELETIONS"
        const val ACTION_RESCHEDULE = "com.kahflabs.wasl.internal.RESCHEDULE"
        const val ACTION_STATUS_DISMISSED = "com.kahflabs.wasl.internal.STATUS_DISMISSED"
    }
}
