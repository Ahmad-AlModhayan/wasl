package com.almodhayan.drivesync.scheduler

import android.content.Context
import android.content.pm.ServiceInfo
import androidx.hilt.work.HiltWorker
import androidx.work.CoroutineWorker
import androidx.work.ForegroundInfo
import androidx.work.WorkerParameters
import com.almodhayan.drivesync.BuildConfig
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.RunStatus
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import com.almodhayan.drivesync.engine.STALLED
import com.almodhayan.drivesync.engine.WaslRuntime
import com.almodhayan.drivesync.engine.engineMessageText
import com.almodhayan.drivesync.engine.text
import com.almodhayan.drivesync.engine.toKotlin
import com.almodhayan.drivesync.engine.triggerFor
import com.almodhayan.drivesync.sync.SyncStateHolder
import com.almodhayan.drivesync.devices.DeviceSyncManager
import com.almodhayan.drivesync.devices.Participation
import com.kahflabs.wasl.ffi.core.Route
import dagger.assisted.Assisted
import dagger.assisted.AssistedInject
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.sample
import kotlinx.coroutines.launch

/**
 * Runs due folder pairs. Uses a dataSync foreground service for the transfer window; Android 15
 * caps that at 6h/day so long initial syncs are resumed by the next periodic run (the executor
 * commits the baseline per file, nothing is lost).
 */
@HiltWorker
class SyncWorker @AssistedInject constructor(
    @Assisted appContext: Context,
    @Assisted params: WorkerParameters,
    private val db: Db,
    private val runtime: WaslRuntime,
    private val settings: SettingsRepository,
    private val notifications: SyncNotifications,
    private val state: SyncStateHolder,
    private val devices: DeviceSyncManager,
) : CoroutineWorker(appContext, params) {

    override suspend fun getForegroundInfo(): ForegroundInfo = foregroundInfo()

    private suspend fun foregroundInfo(): ForegroundInfo {
        val names = db.pairs().getAll().associate { it.id to it.name }
        val n = notifications.progress(state.running.value.values, names)
        return ForegroundInfo(SyncNotifications.ID_PROGRESS, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
    }

    override suspend fun doWork(): Result = coroutineScope {
        val trigger = inputData.getString(KEY_TRIGGER) ?: "periodic"
        val manual = inputData.getBoolean(KEY_MANUAL, false)
        val onlyPair = inputData.getString(KEY_PAIR_ID)
        val cfg = settings.current()
        notifications.ensureChannels()
        runCatching { setForeground(foregroundInfo()) }

        val pairs = (if (onlyPair != null) listOfNotNull(db.pairs().get(onlyPair)) else db.pairs().getActive())
            .filter { it.enabled && (!it.paused || manual) }
        if (pairs.isEmpty()) return@coroutineScope Result.success()

        val names = pairs.associate { it.id to it.name }
        var updater: Job? = null
        if (cfg.notifyProgress) {
            updater = launch {
                state.running.sample(750).collect { runCatching { setForeground(ForegroundInfo(SyncNotifications.ID_PROGRESS, notifications.progress(it.values, names), ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)) } }
            }
        }
        var anyFailure = false
        var anyStall = false
        try {
            for (listed in pairs) {
                if (isStopped) break
                var runTrigger = trigger
                var rounds = 0
                do {
                    // Read the pair again: an earlier pair may have synced for hours, and this one may
                    // have been paused, turned off or pointed at another folder since.
                    val pair = db.pairs().get(listed.id)?.takeIf { it.enabled && (!it.paused || manual) } ?: break
                    val interval = (if (pair.intervalMinutes > 0) pair.intervalMinutes else cfg.globalIntervalMinutes) * 60_000L
                    if (rounds == 0 && !RunPolicy.isDue(pair, runTrigger, manual, System.currentTimeMillis(), interval)) break
                    // By its route (FR-N12): Drive, the devices the phone is connected to, or both.
                    val route = runCatching { runtime.engine.pair(pair.id)?.route }.getOrNull() ?: Route.DRIVE
                    val plan = Participation.plan(route, devices.open.value)
                    if (plan.waiting) {
                        devices.recordWaiting(pair.id)
                        break
                    }
                    if (!plan.drive) {
                        if (plan.devices) devices.syncPair(pair.id)
                        break
                    }
                    var result = try {
                        runtime.engine.sync(pair.id, triggerFor(runTrigger, manual))
                    } catch (e: CancellationException) {
                        runtime.engine.cancel(pair.id)
                        throw e
                    } catch (t: Throwable) {
                        // One pair's crash must not stop the other pairs from syncing.
                        // The pair's id, not its name; the stack only in debug builds, as messages can carry paths (security review 2026-10).
                        android.util.Log.w("SyncWorker", "Sync of pair ${pair.id} failed unexpectedly: ${t.javaClass.simpleName}", t.takeIf { BuildConfig.DEBUG })
                        anyFailure = true
                        break
                    } finally {
                        state.finish(pair.id)
                        runtime.changed()
                    }
                    // The pair's conditions held it back: say why on its card, as 1.1.4 did.
                    val blocker = result.blocker
                    if (blocker != null) {
                        db.pairs().setRunResult(pair.id, pair.lastRunAt ?: 0L, RunStatus.SKIPPED, blocker.text())
                        break
                    }
                    if (plan.devices && pair.enabled) {
                        devices.syncPair(pair.id)?.let { result = Participation.merge(result, it, Strings.get(R.string.devices_outcome_label)) }
                    }
                    val outcome = result.toKotlin()
                    notifyOutcome(pair, outcome, cfg.notifyOnSuccess, cfg.notifyOnError)
                    if (outcome.status == RunStatus.FAILED || outcome.status == RunStatus.PARTIAL) anyFailure = true
                    if (outcome.message == STALLED) anyStall = true
                    // A change that arrived while this pair was syncing was not part of this run: run it
                    // once more now instead of leaving it for the next scheduled check.
                    val again = outcome.status != RunStatus.SKIPPED && state.takeFollowUp(pair.id) &&
                        (outcome.status == RunStatus.SUCCESS || outcome.status == RunStatus.PARTIAL)
                    runTrigger = "follow-up"
                    rounds++
                } while (again && rounds < 3 && !isStopped)
            }
        } finally {
            updater?.cancel()
        }
        if (RunPolicy.shouldRetry(anyFailure, anyStall, manual, runAttemptCount)) Result.retry() else Result.success()
    }

    private suspend fun notifyOutcome(pair: com.almodhayan.drivesync.data.db.FolderPairEntity, outcome: com.almodhayan.drivesync.sync.RunOutcome, onSuccess: Boolean, onError: Boolean) {
        val text = com.almodhayan.drivesync.sync.StatusText
        when (outcome.status) {
            RunStatus.BLOCKED -> {
                val s = outcome.summary
                notifications.approval(pair.id, text.approvalTitle(s), text.approvalBody(pair.name, s, engineMessageText(outcome.message)), text.approvalAction(s), text.restoreAction(s))
            }
            RunStatus.FAILED, RunStatus.PARTIAL -> {
                val fresh = db.pairs().get(pair.id)
                val kind = com.almodhayan.drivesync.core.ErrorKind.parse(fresh?.lastErrorKind)
                val loud = kind == null || !kind.transient || (fresh?.consecutiveFailures ?: 0) >= text.FAILED_AFTER
                if (onError && loud) notifications.failed(pair.id, text.failedTitle(pair.name, kind), engineMessageText(outcome.message) ?: Strings.get(R.string.notif_failed_body_fallback), text.fixLabel(text.fixFor(kind)))
            }
            RunStatus.SUCCESS -> {
                notifications.clearFor(pair.id)
                val conflicts = db.conflicts().openForPair(pair.id).size
                if (conflicts > 0 && (outcome.summary?.conflicts ?: 0) > 0) notifications.conflicts(pair.id, text.conflictTitle(conflicts, pair.name), text.CONFLICT_BODY)
                else if (onSuccess && outcome.summary?.isEmpty == false) notifications.synced(pair.id, Strings.get(R.string.notif_synced_title, pair.name), describe(outcome.summary))
            }
            else -> {}
        }
    }

    private fun describe(s: com.almodhayan.drivesync.sync.PlanSummary): String = buildList {
        if (s.uploads > 0) add(Strings.plural(R.plurals.notif_synced_uploaded, s.uploads))
        if (s.downloads + s.exports > 0) add(Strings.plural(R.plurals.notif_synced_downloaded, s.downloads + s.exports))
        if (s.deleteLocal + s.deleteRemote > 0) add(Strings.plural(R.plurals.notif_synced_deleted, s.deleteLocal + s.deleteRemote))
        if (s.conflicts > 0) add(Strings.plural(R.plurals.notif_synced_conflicts, s.conflicts))
    }.joinToString(Strings.get(R.string.notif_list_separator)).ifEmpty { Strings.get(R.string.notif_up_to_date) }

    companion object {
        const val KEY_TRIGGER = "trigger"
        const val KEY_MANUAL = "manual"
        const val KEY_PAIR_ID = "pairId"
    }
}
