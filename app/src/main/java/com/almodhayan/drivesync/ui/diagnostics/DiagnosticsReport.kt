package com.almodhayan.drivesync.ui.diagnostics

import android.os.Build
import androidx.work.WorkInfo
import com.almodhayan.drivesync.BuildConfig
import com.almodhayan.drivesync.core.TimeFmt
import com.almodhayan.drivesync.data.db.SyncEventEntity
import com.almodhayan.drivesync.scheduler.MonitorService
import com.almodhayan.drivesync.ui.onboarding.VendorGuide
import java.util.concurrent.TimeUnit

/**
 * Plain-text report and WorkInfo formatting; pure functions so they are easy to test. The report is
 * for the developer, so it stays in English whatever the app's language.
 */
object DiagnosticsReport {

    /** Maps WorkManager/JobScheduler stop reasons (API 31+) to text. */
    fun stopReasonText(code: Int): String = when (code) {
        WorkInfo.STOP_REASON_NOT_STOPPED, 0 -> "not stopped"
        1 -> "cancelled by the app"
        2 -> "preempted by higher-priority work"
        3 -> "timed out (ran longer than allowed)"
        4 -> "device state changed (Doze, screen off, …)"
        5 -> "battery became low"
        6 -> "charger was disconnected"
        7 -> "network connection was lost"
        8 -> "device stopped being idle"
        9 -> "storage became low"
        10 -> "background execution quota exhausted (app standby bucket)"
        11 -> "background restriction: the app is restricted in battery settings"
        12 -> "app standby bucket too restrictive"
        13 -> "stopped by the user (force stop / swiped away)"
        14 -> "system needed the resources"
        15 -> "estimated app launch time changed"
        16 -> "foreground service time limit reached (Android 15 dataSync cap)"
        WorkInfo.STOP_REASON_UNKNOWN -> "unknown"
        else -> "code $code"
    }

    fun stopReasonOf(w: WorkInfo): Int =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) w.stopReason else WorkInfo.STOP_REASON_NOT_STOPPED

    fun isStopped(w: WorkInfo): Boolean {
        val r = stopReasonOf(w)
        return r != WorkInfo.STOP_REASON_NOT_STOPPED && r != 0
    }

    /** "periodic every 60 min" / "one-shot". */
    fun kindOf(w: WorkInfo): String {
        val p = w.periodicityInfo
        return if (p != null) "periodic every ${TimeUnit.MILLISECONDS.toMinutes(p.repeatIntervalMillis)} min" else "one-shot"
    }

    fun tagsOf(w: WorkInfo): String = w.tags.map { it.substringAfterLast('.') }.sorted().joinToString(", ")

    fun nextRunOf(w: WorkInfo): String? {
        if (w.state != WorkInfo.State.ENQUEUED) return null
        val next = w.nextScheduleTimeMillis
        if (next == Long.MAX_VALUE || next <= 0L) return null
        return TimeFmt.human(next)
    }

    fun workInfoSummary(w: WorkInfo): String = buildString {
        append(w.state.name)
        append(" · ").append(kindOf(w))
        append(" · ").append(tagsOf(w))
        if (w.runAttemptCount > 0) append(" · attempt ${w.runAttemptCount}")
        nextRunOf(w)?.let { append(" · next $it") }
        if (isStopped(w)) append(" · last stop: ").append(stopReasonText(stopReasonOf(w)))
    }

    /** "ab***@gmail.com": enough to tell accounts apart without pasting the address. */
    fun maskEmail(email: String): String {
        val at = email.indexOf('@')
        if (at <= 0) return "***"
        val user = email.substring(0, at)
        val domain = email.substring(at)
        return user.take(2) + "***" + domain
    }

    private fun eventLine(e: SyncEventEntity): String =
        "${TimeFmt.human(e.at)} ${e.level} ${e.action}: ${e.message}" + (e.path?.substringAfterLast('/')?.let { " [$it]" } ?: "")

    fun build(state: DiagnosticsUiState): String = buildString {
        val d = state.device
        val s = state.settings
        val vendor = VendorGuide.current()
        appendLine("Wasl diagnostics report")
        appendLine("Generated: ${TimeFmt.human(System.currentTimeMillis())}")
        appendLine("App: ${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})")
        appendLine("Device: ${Build.MANUFACTURER} ${Build.MODEL} (brand ${Build.BRAND}), Android ${Build.VERSION.RELEASE} (API ${Build.VERSION.SDK_INT})")
        appendLine("Vendor guidance: ${vendor?.label ?: "none (stock-like)"}")
        appendLine()
        appendLine("Checks")
        if (d != null) {
            appendLine("- All files access: ${if (d.allFilesAccess) "OK" else "MISSING"}")
            appendLine("- Notifications: ${if (d.notifications) "OK" else "MISSING"}")
            appendLine("- Battery optimization ignored: ${if (d.batteryUnrestricted) "OK" else "NO"}")
            appendLine("- Location (Wi-Fi rules): ${if (!state.ssidRulesUsed) "not needed" else if (d.fineLocation) "OK" else "MISSING"}")
        }
        appendLine("- Instant sync service: ${if (s?.instantMonitoring == true) (if (state.monitorRunning) "running" else "ENABLED BUT NOT RUNNING") else "off"} (MonitorService.running=${MonitorService.running})")
        state.network?.let { n ->
            appendLine("- Network: online=${n.online}, wifi=${n.wifi}, metered=${n.metered}, vpn=${n.vpn}, roaming=${n.roaming}, charging=${n.charging}, battery=${n.battery}%")
        }
        appendLine("- Unreachable pair folders: ${state.unreachable.size}")
        appendLine("- Background jobs: ${state.workInfos.size}")
        state.workInfos.forEach { appendLine("    ${workInfoSummary(it)}") }
        appendLine("- Last 24 hours: ${state.summary.runs} syncs, ${state.summary.filesMoved} files moved, ${state.summary.errors} errors")
        appendLine()
        appendLine("Device sync")
        val info = state.devices
        if (!state.devicesOpen || info == null) {
            appendLine("- not running (app closed, no session, not charging on Wi-Fi), whileCharging=${s?.devicesWhileCharging ?: false}")
        } else {
            appendLine("- running, listening on ${info.listen ?: "unknown"}, whileCharging=${s?.devicesWhileCharging ?: false}")
            info.devices.filter { !it.thisDevice }.forEach { dev ->
                // Device IDs only: names may be personal.
                appendLine("- ${dev.id}: ${if (dev.connected) "reachable, path=${dev.path?.name?.lowercase() ?: "unknown"}" else "not reachable, path=none"}")
            }
        }
        appendLine()
        if (s != null) {
            appendLine("Settings")
            appendLine("- interval ${s.globalIntervalMinutes} min, instant=${s.instantMonitoring}, remote poll ${s.remotePollMinutes} min")
            appendLine("- defaults: wifiOnly=${s.wifiOnlyDefault}, requireCharging=${s.requireChargingDefault}, minBattery=${s.minBatteryDefault}%")
            appendLine("- concurrency ${s.concurrency}, bandwidth ${if (s.bandwidthLimitKBps == 0) "unlimited" else "${s.bandwidthLimitKBps} KB/s"}")
            appendLine("- trash ${s.trashRetentionDays} d, history ${s.historyRetentionDays} d, mass-delete ≥${s.massDeleteMinFiles} files & ≥${s.massDeleteThresholdPercent}%")
            appendLine("- conflicts ${s.conflictPolicyDefault.name}, docs ${s.docsFormatDefault.name}, notifications progress=${s.notifyProgress} success=${s.notifyOnSuccess} error=${s.notifyOnError}")
            appendLine("- theme ${s.theme}, dynamicColor=${s.dynamicColor}, autoBackup=${s.autoBackupConfig}, onDemandCache ${s.onDemandCacheMb} MB, batteryHintDismissed=${s.batteryHintDismissed}")
            appendLine("- new pairs: deleteGuard=${s.massDeleteGuardDefault}, maxFile ${s.maxFileSizeDefaultMb} MB; phone versions kept ${s.localVersionsKept}")
            appendLine()
        }
        appendLine("Accounts (${state.accounts.size})")
        state.accounts.forEach { a ->
            val check = state.checks[a.id]
            val quota = if (a.storageUsed != null) " storage=${a.storageUsed}/${a.storageLimit ?: "unlimited"}" else ""
            appendLine(
                "- ${maskEmail(a.id)}: indexed=${a.lastCrawlAt != null}, lastDelta=${a.lastDeltaAt?.let(TimeFmt::human) ?: "never"}, " +
                    "authError=${if (a.authError.isNullOrBlank()) "none" else "yes"},$quota connectivity=" +
                    (check?.let { (if (it.ok) "OK" else "FAIL: ${it.hint ?: "unknown"}") + " at ${TimeFmt.human(it.at)}" } ?: "not tested"),
            )
        }
        appendLine()
        appendLine("Folder pairs (${state.pairs.size})")
        state.pairs.forEach { p ->
            appendLine(
                "- ${p.name}: ${p.mode.name}, enabled=${p.enabled}, paused=${p.paused}${p.pauseReason?.let { " ($it)" } ?: ""}, instant=${p.instantSync}, " +
                    "wifiOnly=${p.wifiOnly}, charging=${p.requireCharging}, minBattery=${p.minBatteryPercent}, ssidRules=${p.ssidAllow.isNotBlank() || p.ssidDeny.isNotBlank()}, " +
                    "lastStatus=${p.lastStatus?.name ?: "never"}${p.lastRunAt?.let { " at ${TimeFmt.human(it)}" } ?: ""}" +
                    (p.lastError?.let { ", lastError=${it.lineSequence().first().take(120)}" } ?: "") +
                    (if (p.pendingApproval) ", PENDING APPROVAL" else ""),
            )
        }
        appendLine()
        appendLine("Recent problems (${state.problems.size})")
        if (state.problems.isEmpty()) appendLine("- none") else state.problems.forEach { appendLine("- ${eventLine(it)}") }
    }
}
