package com.almodhayan.drivesync.scheduler

import com.almodhayan.drivesync.sync.Conditions
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.database.ContentObserver
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.provider.MediaStore
import androidx.core.content.ContextCompat
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import com.almodhayan.drivesync.drive.DriveNodeCache
import com.almodhayan.drivesync.fs.FileWatcher
import dagger.hilt.android.AndroidEntryPoint
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineExceptionHandler
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.filter
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.merge
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import com.almodhayan.drivesync.sync.PairProgress
import com.almodhayan.drivesync.sync.Phase
import com.almodhayan.drivesync.sync.SyncStateHolder
import java.io.File
import java.util.concurrent.ConcurrentHashMap
import javax.inject.Inject

/**
 * Long-lived foreground service that (1) keeps the status notification current: what is syncing
 * and how far along, or whether Drive Sync is waiting, paused, stalled or needs you, and (2) while
 * Instant sync is on, watches local folders (inotify + MediaStore) and polls the Drive Changes API
 * so both directions feel instant. Runs whenever at least one pair is turned on.
 */
@AndroidEntryPoint
class MonitorService : Service() {
    @Inject lateinit var db: Db
    @Inject lateinit var settings: SettingsRepository
    @Inject lateinit var scheduler: SyncScheduler
    @Inject lateinit var notifications: SyncNotifications
    @Inject lateinit var cache: DriveNodeCache
    @Inject lateinit var state: SyncStateHolder
    @Inject lateinit var conditions: Conditions

    // This service runs all the time and Android restarts it after a crash, so one bad read must never
    // take the process down (it would crash again on every restart).
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO + CoroutineExceptionHandler { _, t -> android.util.Log.w(TAG, "Status service task failed", t) })
    // Changed on a background thread, read by the MediaStore observer on the main thread.
    private val watchers = ConcurrentHashMap<String, FileWatcher>()
    /** The pairs that sync when files change, as last set; read by the MediaStore observer. */
    @Volatile private var watched: List<FolderPairEntity> = emptyList()
    private val pending = HashMap<String, Job>()
    private var mediaObserver: ContentObserver? = null
    private val lastProgressAt = HashMap<String, Long>()
    private val lastSignature = HashMap<String, List<Any>>()

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        running = true
        notifications.ensureChannels()
        try {
            startForeground(SyncNotifications.ID_MONITOR, notifications.status(Status(Strings.get(R.string.app_name), Strings.get(R.string.notif_status_checking))), ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        } catch (e: Exception) {
            // Android 12+ can refuse when it restarts this service in the background. Stop quietly; the
            // next app start or pair change starts it again.
            android.util.Log.w(TAG, "Could not show the status notification now: ${e.message}")
            running = false
            stopSelf()
            return
        }
        scope.launch {
            combine(db.pairs().observeAll(), settings.settings) { pairs, cfg -> pairs.filter { MonitorPolicy.watches(cfg.instantMonitoring, it) } }
                .collectLatest { runCatching { rebuildWatchers(it) }.onFailure { e -> android.util.Log.w(TAG, "Could not watch folders", e) } }
        }
        scope.launch { statusLoop() }
        scope.launch { remotePollLoop() }
        registerMediaObserver()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int = START_STICKY

    override fun onDestroy() {
        running = false
        watchers.values.forEach { it.stop() }; watchers.clear()
        mediaObserver?.let { contentResolver.unregisterContentObserver(it) }
        scope.cancel()
        super.onDestroy()
    }

    private fun rebuildWatchers(pairs: List<FolderPairEntity>) {
        watched = pairs
        val wanted = pairs.associateBy { it.id }
        watchers.keys.filter { it !in wanted }.forEach { watchers.remove(it)?.stop() }
        for (p in pairs) {
            if (watchers.containsKey(p.id)) continue
            val root = MonitorPolicy.watchPath(p)?.let(::File) ?: continue
            if (!root.isDirectory) continue
            val w = FileWatcher(root) { changedPath -> onLocalChange(p.id, changedPath) }
            runCatching { w.start() }.onSuccess { watchers[p.id] = w }
        }
    }

    private fun registerMediaObserver() {
        val obs = object : ContentObserver(Handler(Looper.getMainLooper())) {
            override fun onChange(selfChange: Boolean) { onMediaChange() }
        }
        contentResolver.registerContentObserver(MediaStore.Files.getContentUri("external"), true, obs)
        mediaObserver = obs
    }

    private fun onMediaChange() {
        // MediaStore does not say which file changed; debounce and let the scan decide per pair.
        for (id in MonitorPolicy.mediaTargets(watched, watchers.keys)) schedule(id, DEBOUNCE_MEDIA_MS)
    }

    private fun onLocalChange(pairId: String, path: String) {
        if (path.endsWith(".drivesync-part")) return
        schedule(pairId, DEBOUNCE_MS)
    }

    @Synchronized
    private fun schedule(pairId: String, delayMs: Long) {
        pending[pairId]?.cancel()
        pending[pairId] = scope.launch {
            delay(delayMs)
            val pair = db.pairs().get(pairId) ?: return@launch
            if (!pair.mode.uploads && delayMs == DEBOUNCE_MS) return@launch  // download-only pairs ignore local edits
            scheduler.syncNow(pairId, "instant", manual = false)
        }
    }

    /**
     * Keeps the status line current: every second while syncing, on any start or stop, else every 15 s.
     * It is always there: dismissed, it comes straight back, and it is re-posted every minute anyway.
     */
    private suspend fun statusLoop() {
        var posted: Status? = null
        var postedAt = 0L
        while (scope.isActive) {
            val running = state.running.value
            try {
                val now = System.currentTimeMillis()
                trackProgress(running, now)
                val cfg = settings.current()
                val pairs = db.pairs().getAll()
                val live = pairs.filter { it.enabled && !it.paused }.mapNotNull { p -> conditions.blockReason(p)?.let { p.id to it } }.toMap()
                val status = StatusLine.of(StatusInput(now, pairs, running, HashMap(lastProgressAt), cfg.instantMonitoring, cfg.globalIntervalMinutes, live))
                if (status != posted || now - postedAt >= REPOST_EVERY_MS) { notifications.postStatus(status); posted = status; postedAt = now }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                android.util.Log.w(TAG, "Could not update the status notification", e)
            }
            if (running.isNotEmpty()) {
                delay(1_000)
                if (repost.tryReceive().isSuccess) posted = null
            } else {
                val dismissed = withTimeoutOrNull(15_000) {
                    merge(state.running.filter { it.isNotEmpty() }.map { false }, repost.receiveAsFlow().map { true }).first()
                }
                if (dismissed == true) posted = null
            }
        }
    }

    /** Remembers when each running pair last moved, so a run that stops moving reads as stalled. */
    private fun trackProgress(running: Map<String, PairProgress>, now: Long) {
        lastProgressAt.keys.retainAll(running.keys)
        lastSignature.keys.retainAll(running.keys)
        for ((id, p) in running) {
            val sig = listOf(p.phase, p.doneActions, p.bytesDone, p.checked)
            if (lastSignature[id] != sig) { lastSignature[id] = sig; lastProgressAt[id] = now }
        }
    }

    private suspend fun remotePollLoop() {
        while (scope.isActive) {
            val minutes = runCatching { settings.current().remotePollMinutes }.getOrDefault(5)
            delay(minutes.coerceAtLeast(1) * 60_000L)
            try {
                if (!settings.current().instantMonitoring) continue
                val pairs = db.pairs().getActive().filter { it.instantSync && it.mode.downloads }
                for (accountId in pairs.map { it.accountId }.distinct()) {
                    val changed = try { cache.refresh(accountId) } catch (e: CancellationException) { throw e } catch (e: Exception) { false }
                    if (changed) pairs.filter { it.accountId == accountId }.forEach { scheduler.syncNow(it.id, "remote-change", manual = false) }
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                android.util.Log.w(TAG, "Could not check Drive for changes", e)
            }
        }
    }

    companion object {
        @Volatile var running: Boolean = false
            private set
        private const val TAG = "MonitorService"
        private const val REPOST_EVERY_MS = 60_000L
        private val repost = Channel<Unit>(Channel.CONFLATED)

        /** The status notification was dismissed: show it again now. */
        fun requestRepost() { repost.trySend(Unit) }
        private const val DEBOUNCE_MS = 4_000L
        private const val DEBOUNCE_MEDIA_MS = 15_000L

        fun startIfEnabled(context: Context) {
            CoroutineScope(Dispatchers.IO).launch {
                val enabled = runCatching {
                    val entry = dagger.hilt.android.EntryPointAccessors.fromApplication(context.applicationContext, MonitorEntryPoint::class.java)
                    MonitorPolicy.shouldRun(entry.db().pairs().getAll())
                }.getOrDefault(false)
                if (enabled) start(context)
            }
        }

        /** Android 12+ refuses foreground-service starts from the background; the next app start retries. */
        fun start(context: Context) {
            if (running) return
            runCatching { ContextCompat.startForegroundService(context, Intent(context, MonitorService::class.java)) }
                .onFailure { android.util.Log.w("MonitorService", "Could not start the status service now: ${it.message}") }
        }

        fun stop(context: Context) { context.stopService(Intent(context, MonitorService::class.java)) }
    }

    @dagger.hilt.EntryPoint
    @dagger.hilt.InstallIn(dagger.hilt.components.SingletonComponent::class)
    interface MonitorEntryPoint {
        fun settings(): SettingsRepository
        fun db(): Db
    }
}
