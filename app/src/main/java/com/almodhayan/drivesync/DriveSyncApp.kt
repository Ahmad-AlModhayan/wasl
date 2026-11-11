package com.almodhayan.drivesync

import android.app.Application
import androidx.hilt.work.HiltWorkerFactory
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import androidx.work.Configuration
import com.almodhayan.drivesync.backup.ConfigBackup
import com.almodhayan.drivesync.backup.schedulePeriodic
import com.almodhayan.drivesync.widget.WidgetUpdater
import com.almodhayan.drivesync.ondemand.PinManager
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import com.almodhayan.drivesync.drive.BandwidthLimiter
import com.almodhayan.drivesync.scheduler.MonitorPolicy
import com.almodhayan.drivesync.scheduler.MonitorService
import com.almodhayan.drivesync.scheduler.SyncNotifications
import com.almodhayan.drivesync.scheduler.SyncScheduler
import com.almodhayan.drivesync.engine.WaslRuntime
import com.almodhayan.drivesync.devices.DeviceSyncManager
import com.almodhayan.drivesync.devices.DeviceWindowWorker
import dagger.hilt.android.HiltAndroidApp
import kotlinx.coroutines.CoroutineExceptionHandler
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.launch
import javax.inject.Inject

@HiltAndroidApp
class DriveSyncApp : Application(), Configuration.Provider {
    @Inject lateinit var workerFactory: HiltWorkerFactory
    @Inject lateinit var scheduler: SyncScheduler
    @Inject lateinit var notifications: SyncNotifications
    @Inject lateinit var settings: SettingsRepository
    @Inject lateinit var configBackup: ConfigBackup
    @Inject lateinit var pinManager: PinManager
    @Inject lateinit var db: Db
    @Inject lateinit var runtime: WaslRuntime
    @Inject lateinit var devices: DeviceSyncManager

    // A failing background task is logged; it must not crash the app on every start.
    val appScope = CoroutineScope(SupervisorJob() + Dispatchers.Default + CoroutineExceptionHandler { _, t -> android.util.Log.w("DriveSync", "Background task failed", t) })

    /** Anything still marked running from before this moment belongs to a process Android killed. */
    private val processStartedAt = System.currentTimeMillis()

    override val workManagerConfiguration: Configuration
        get() = Configuration.Builder().setWorkerFactory(workerFactory).setMinimumLoggingLevel(android.util.Log.INFO).build()

    override fun onCreate() {
        super.onCreate()
        com.almodhayan.drivesync.core.Strings.init(this)
        notifications.ensureChannels()
        appScope.launch {
            // Opening the engine closes runs a killed process left open; pick their work up again now
            // rather than at the next check.
            runtime.engine.runs(null, 50u)
                .filter { it.message == STOPPED_BEFORE_FINISHING && it.endedAt != null && it.endedAt!! >= processStartedAt - 60_000 }
                .map { it.pairId }.toSet()
                .forEach { scheduler.syncNow(it, "resume", manual = false) }
        }
        appScope.launch { scheduler.ensurePeriodic() }
        // One owner for the status service: it runs whenever a pair is on, so creating, resuming or
        // restoring a pair starts it without every screen having to remember to.
        appScope.launch {
            db.pairs().observeAll().map { MonitorPolicy.shouldRun(it) }
                .distinctUntilChanged()
                .collect { run -> if (run) MonitorService.start(this@DriveSyncApp) else MonitorService.stop(this@DriveSyncApp) }
        }
        // Android refuses foreground-service starts from the background; retry whenever the app is opened.
        ProcessLifecycleOwner.get().lifecycle.addObserver(object : DefaultLifecycleObserver {
            override fun onStart(owner: LifecycleOwner) { MonitorService.startIfEnabled(this@DriveSyncApp) }
        })
        appScope.launch {
            settings.settings.collect { BandwidthLimiter.bytesPerSecond = it.bandwidthLimitKBps.toLong() * 1024 }
        }
        // Device sync (FR-N12): the phone takes part while the app is open, during a session the
        // person started, and while charging on Wi-Fi when allowed.
        devices.start()
        appScope.launch {
            settings.settings.map { it.devicesWhileCharging }.distinctUntilChanged()
                .collect { DeviceWindowWorker.schedule(this@DriveSyncApp, it) }
        }
        configBackup.schedulePeriodic(this)
        WidgetUpdater.start(this, appScope)
        pinManager.start(appScope)
        pinManager.ensurePeriodic(this)
    }
}

/** The engine's message on a run a killed process left open. */
private const val STOPPED_BEFORE_FINISHING = "Stopped before it finished. The next sync carries on."
