package com.almodhayan.drivesync.scheduler

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.sync.PairProgress
import com.almodhayan.drivesync.sync.Phase
import com.almodhayan.drivesync.ui.MainActivity
import com.almodhayan.drivesync.ui.components.Fmt
import dagger.hilt.android.qualifiers.ApplicationContext
import javax.inject.Inject
import javax.inject.Singleton

/**
 * Syncing is a silent, ongoing notification with Pause and a progress bar. Loud states (needs approval,
 * conflicts, failed) post a default-priority notification whose actions match the in-app ones.
 * Synced and waiting post nothing (unless the person turned success notifications on).
 */
@Singleton
class SyncNotifications @Inject constructor(@ApplicationContext private val context: Context) {
    private val nm get() = context.getSystemService(NotificationManager::class.java)
    private val brand = 0xFF1F3D6B.toInt()

    fun ensureChannels() {
        nm.createNotificationChannel(NotificationChannel(CH_PROGRESS, Strings.get(R.string.notif_channel_progress), NotificationManager.IMPORTANCE_LOW).apply { setShowBadge(false); description = Strings.get(R.string.notif_channel_progress_description) })
        // Low, not min, so the status line shows in the shade. A channel's importance can't be raised later,
        // so the old min-importance "monitor" channel is replaced by a new one.
        nm.deleteNotificationChannel(CH_MONITOR_OLD)
        nm.createNotificationChannel(NotificationChannel(CH_STATUS, Strings.get(R.string.notif_channel_status), NotificationManager.IMPORTANCE_LOW).apply { setShowBadge(false); description = Strings.get(R.string.notif_channel_status_description) })
        nm.createNotificationChannel(NotificationChannel(CH_RESULT, Strings.get(R.string.notif_channel_result), NotificationManager.IMPORTANCE_LOW).apply { description = Strings.get(R.string.notif_channel_result_description) })
        nm.createNotificationChannel(NotificationChannel(CH_ATTENTION, Strings.get(R.string.notif_channel_attention), NotificationManager.IMPORTANCE_DEFAULT).apply { description = Strings.get(R.string.notif_channel_attention_description) })
    }

    private fun openApp(route: String? = null): PendingIntent {
        val i = Intent(context, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP
            if (route != null) putExtra(MainActivity.EXTRA_ROUTE, route)
        }
        return PendingIntent.getActivity(context, (route ?: "home").hashCode(), i, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    }

    private fun broadcast(action: String, pairId: String? = null): PendingIntent {
        val i = Intent(context, InternalActionReceiver::class.java).setAction(action).apply { if (pairId != null) putExtra("pairId", pairId) }
        return PendingIntent.getBroadcast(context, (action + (pairId ?: "")).hashCode(), i, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    }

    /**
     * Private on the lock screen: pair names, folders and file counts show only once unlocked; the
     * lock screen gets a generic line per channel (security review 2026-10).
     */
    private fun base(channel: String) = NotificationCompat.Builder(context, channel)
        .setSmallIcon(R.drawable.ic_stat_drive_sync)
        .setColor(brand)
        .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
        .setPublicVersion(publicVersion(channel))

    /**
     * The same private builder for notifications posted elsewhere (the device session's foreground
     * service), so none of them can show more on the lock screen than these.
     */
    fun privateBuilder(channel: String): NotificationCompat.Builder = base(channel)

    private fun publicVersion(channel: String): Notification {
        val text = when (channel) {
            CH_PROGRESS -> R.string.notif_public_syncing
            CH_ATTENTION -> R.string.notif_public_attention
            CH_RESULT -> R.string.notif_public_result
            else -> R.string.notif_public_status
        }
        return NotificationCompat.Builder(context, channel)
            .setSmallIcon(R.drawable.ic_stat_drive_sync)
            .setColor(brand)
            .setContentTitle(Strings.get(text))
            .build()
    }

    fun progress(progress: Collection<PairProgress>, names: Map<String, String>): Notification {
        val b = base(CH_PROGRESS)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .setContentIntent(openApp(progress.singleOrNull()?.let { "pair:${it.pairId}" } ?: "home"))
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .addAction(0, Strings.get(R.string.common_pause), broadcast(InternalActionReceiver.ACTION_PAUSE_RUNNING))
            .addAction(0, Strings.get(R.string.notif_action_open), openApp("home"))
        when {
            progress.isEmpty() -> b.setContentTitle(Strings.get(R.string.notif_progress_getting_ready)).setProgress(0, 0, true)
            progress.size == 1 -> {
                val p = progress.first()
                b.setContentTitle(names[p.pairId] ?: Strings.get(R.string.common_syncing))
                if (p.phase == Phase.TRANSFERRING || p.phase == Phase.DELETING) {
                    val left = Fmt.eta(etaMs(p))
                    val done = Fmt.count(p.doneActions); val total = Fmt.count(p.totalActions)
                    b.setContentText(left?.let { Strings.get(R.string.notif_progress_count_eta, done, total, it.replaceFirstChar { c -> c.lowercase() }) }
                        ?: Strings.get(R.string.notif_progress_count, done, total))
                    b.setProgress(1000, (p.fraction * 1000).toInt(), false)
                } else {
                    b.setContentText(Strings.get(R.string.notif_comparing_folders)).setProgress(0, 0, true)
                }
            }
            else -> b.setContentTitle(Strings.plural(R.plurals.notif_progress_syncing_pairs, progress.size)).setContentText(progress.mapNotNull { names[it.pairId] }.joinToString(Strings.get(R.string.notif_list_separator))).setProgress(0, 0, true)
        }
        return b.build()
    }

    private fun etaMs(p: PairProgress): Long? {
        val elapsed = System.currentTimeMillis() - p.startedAt
        val f = p.fraction
        if (f <= 0.02f || elapsed < 5_000) return null
        return (elapsed / f * (1 - f)).toLong()
    }

    /** The always-on status line (the status service's foreground notification). */
    fun status(s: Status): Notification = base(CH_STATUS)
        .setContentTitle(s.title)
        .setContentText(s.text)
        .setOngoing(true).setSilent(true).setShowWhen(false).setOnlyAlertOnce(true)
        .setCategory(NotificationCompat.CATEGORY_STATUS)
        .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
        .setContentIntent(openApp())
        // Android 14+ lets people swipe even an ongoing notification away; the status service puts it
        // straight back when told it was dismissed.
        .setDeleteIntent(broadcast(InternalActionReceiver.ACTION_STATUS_DISMISSED))
        .apply { s.fraction?.let { setProgress(1000, (it * 1000).toInt(), false) } }
        .build()

    fun postStatus(s: Status) = post(ID_MONITOR, status(s))

    /** Needs approval: "Waiting for your OK" with Review and the safe alternative. */
    fun approval(pairId: String, title: String, body: String, reviewLabel: String, restoreLabel: String?) {
        val b = base(CH_ATTENTION).setContentTitle(title).setContentText(body).setStyle(NotificationCompat.BigTextStyle().bigText(body))
            .setAutoCancel(true).setContentIntent(openApp("review:$pairId"))
            .addAction(0, Strings.get(R.string.sync_action_review), openApp("review:$pairId"))
        if (restoreLabel != null) b.addAction(0, restoreLabel, broadcast(InternalActionReceiver.ACTION_RESTORE_DELETIONS, pairId))
        post(idFor("approval", pairId), b.build())
    }

    fun conflicts(pairId: String, title: String, body: String) {
        val b = base(CH_ATTENTION).setContentTitle(title).setContentText(body).setAutoCancel(true)
            .setContentIntent(openApp("problems")).addAction(0, Strings.get(R.string.sync_action_review), openApp("problems"))
        post(idFor("conflict", pairId), b.build())
    }

    fun failed(pairId: String, title: String, body: String, fixLabel: String) {
        val b = base(CH_ATTENTION).setContentTitle(title).setContentText(body).setStyle(NotificationCompat.BigTextStyle().bigText(body))
            .setAutoCancel(true).setContentIntent(openApp("problems")).addAction(0, fixLabel, openApp("problems"))
        post(idFor("failed", pairId), b.build())
    }

    /** Optional quiet note (Settings › Notify when a sync finishes). */
    fun synced(pairId: String, title: String, body: String) {
        val b = base(CH_RESULT).setContentTitle(title).setContentText(body).setAutoCancel(true).setSilent(true).setContentIntent(openApp("pair:$pairId"))
        post(idFor("synced", pairId), b.build())
    }

    fun clearFor(pairId: String) { listOf("approval", "conflict", "failed").forEach { nm.cancel(idFor(it, pairId)) } }

    fun cancel(id: Int) = nm.cancel(id)

    private fun post(id: Int, n: Notification) { if (canPost()) nm.notify(id, n) }

    private fun idFor(kind: String, pairId: String) = ID_RESULT_BASE + ("$kind:$pairId".hashCode() and 0xffff)

    fun canPost(): Boolean =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU)
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
        else NotificationManagerCompat.from(context).areNotificationsEnabled()

    companion object {
        const val CH_PROGRESS = "progress"
        const val CH_STATUS = "status"
        private const val CH_MONITOR_OLD = "monitor"
        const val CH_RESULT = "result"
        const val CH_ATTENTION = "attention"
        const val ID_PROGRESS = 1001
        const val ID_MONITOR = 1002
        const val ID_RESULT_BASE = 2000
    }
}
