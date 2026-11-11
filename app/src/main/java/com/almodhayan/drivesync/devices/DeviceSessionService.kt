package com.almodhayan.drivesync.devices

import android.app.Notification
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.scheduler.SyncNotifications
import com.almodhayan.drivesync.ui.MainActivity
import dagger.hilt.android.AndroidEntryPoint
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import javax.inject.Inject

/**
 * "Sync with my devices now" (SRS FR-N12): keeps the phone in the device group while the person
 * wants it to, with a notification that says so and a Stop button. It ends by itself after
 * [MAX_MS] so a forgotten session doesn't keep the radio busy.
 */
@AndroidEntryPoint
class DeviceSessionService : Service() {
    @Inject lateinit var manager: DeviceSyncManager
    @Inject lateinit var notifications: SyncNotifications

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var work: Job? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            stopNow()
            return START_NOT_STICKY
        }
        notifications.ensureChannels()
        try {
            startForeground(ID, notification(), ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } catch (e: Exception) {
            // Android refused a foreground start (from the background): nothing to keep alive.
            stopSelf()
            return START_NOT_STICKY
        }
        manager.enter(Window.SESSION)
        if (work?.isActive != true) {
            work = scope.launch {
                // Pairs run as soon as the phone has joined; the manager keeps them going after.
                withTimeoutOrNull(30_000) { manager.open.first { it } }
                if (manager.open.value) manager.syncDevicePairs()
                delay(MAX_MS)
                stopNow()
            }
        }
        return START_NOT_STICKY
    }

    override fun onTimeout(startId: Int, fgsType: Int) = stopNow()

    private fun stopNow() {
        manager.leave(Window.SESSION)
        work?.cancel()
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    override fun onDestroy() {
        manager.leave(Window.SESSION)
        scope.cancel()
        super.onDestroy()
    }

    private fun notification(): Notification {
        val open = PendingIntent.getActivity(
            this, ID, Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP).putExtra(MainActivity.EXTRA_ROUTE, "devices"),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val stop = PendingIntent.getService(this, ID + 1, Intent(this, DeviceSessionService::class.java).setAction(ACTION_STOP), PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        // Private with a generic public line, like every other notification (security review S1).
        return notifications.privateBuilder(SyncNotifications.CH_PROGRESS)
            .setContentTitle(Strings.get(R.string.devices_session_title))
            .setContentText(Strings.get(R.string.devices_session_text))
            .setOngoing(true).setSilent(true).setOnlyAlertOnce(true)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .setContentIntent(open)
            .addAction(0, Strings.get(R.string.devices_session_stop), stop)
            .build()
    }

    companion object {
        const val ID = 1003
        const val ACTION_STOP = "com.kahflabs.wasl.devices.STOP_SESSION"
        private const val MAX_MS = 30 * 60_000L

        fun start(context: Context) {
            ContextCompat.startForegroundService(context, Intent(context, DeviceSessionService::class.java))
        }

        fun stop(context: Context) {
            context.startService(Intent(context, DeviceSessionService::class.java).setAction(ACTION_STOP))
        }
    }
}
