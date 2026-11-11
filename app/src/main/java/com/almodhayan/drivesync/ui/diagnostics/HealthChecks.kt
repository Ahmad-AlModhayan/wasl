package com.almodhayan.drivesync.ui.diagnostics

import androidx.work.WorkInfo
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.sync.StatusText
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.HealthState
import com.almodhayan.drivesync.ui.devices.ltr
import uniffi.wasl_devices.LinkPath
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId

/** The one fix a failing or warning check offers. */
enum class HealthFix { GRANT_ACCESS, CHOOSE_FOLDER, ADD_ACCOUNT, SIGN_IN, BATTERY, REARM, LOCATION, NOTIFICATIONS, FREE_SPACE }

/** One row of the Health section. */
data class HealthItem(
    val key: String,
    val state: HealthState,
    val title: String,
    val detail: String,
    val fix: HealthFix? = null,
    val fixLabel: String? = null,
    val pairId: String? = null,
)

/**
 * The six reasons a sync stops (folder access, Google account, battery, background work, network,
 * Drive storage), in that order, each in plain words with at most one fix. Notifications are added
 * only when they are blocked: they don't stop a sync, but you wouldn't hear about one that stopped.
 */
object HealthChecks {
    /** Opens the account's storage page, where large files can be found and removed. */
    const val DRIVE_STORAGE_URL = "https://drive.google.com/drive/quota"

    fun build(s: DiagnosticsUiState, vendorLabel: String? = null): List<HealthItem> = buildList<HealthItem> {
        val d = s.device

        // 1 · folder access
        val turnedOn = s.pairs.filter { it.enabled }
        // Only pairs on a path need all files access; folders picked with Android's picker don't
        // (UX audit 2026-10: with none, "pairs can't read their phone folders" was untrue).
        val pathPairs = turnedOn.any { !it.localPath.startsWith("content://") }
        if (d != null && !d.allFilesAccess && !pathPairs) {
            add(HealthItem("folders", HealthState.Ok, Strings.get(R.string.diagnostics_folder_access), Strings.get(R.string.diagnostics_folder_access_not_needed)))
        } else if (d != null && !d.allFilesAccess) {
            add(HealthItem("folders", HealthState.Fail, Strings.get(R.string.diagnostics_folder_access), Strings.get(R.string.diagnostics_folder_access_off),
                HealthFix.GRANT_ACCESS, StatusText.fixLabel(StatusText.Fix.GRANT_ACCESS)))
        } else {
            val reachable = turnedOn.size - s.unreachable.size
            add(HealthItem("folders", HealthState.Ok, Strings.get(R.string.diagnostics_folder_access),
                if (turnedOn.isEmpty()) Strings.get(R.string.diagnostics_folder_access_allowed)
                else Strings.plural(R.plurals.diagnostics_folders_reachable, turnedOn.size, reachable, turnedOn.size)))
            s.unreachable.forEach { p ->
                add(HealthItem("folder-${p.id}", HealthState.Fail, Fmt.humanLocalPath(p.localPath),
                    Strings.get(R.string.diagnostics_folder_gone, p.name), HealthFix.CHOOSE_FOLDER,
                    StatusText.fixLabel(StatusText.Fix.CHOOSE_FOLDER), pairId = p.id))
            }
        }

        // 2 · Google account
        val signedOut = s.accounts.filter { !it.authError.isNullOrBlank() }
        when {
            s.accounts.isEmpty() -> add(HealthItem("account", HealthState.Fail, Strings.get(R.string.common_google_account), Strings.get(R.string.diagnostics_no_account),
                HealthFix.ADD_ACCOUNT, Strings.get(R.string.diagnostics_add_account)))
            signedOut.isNotEmpty() -> add(HealthItem("account", HealthState.Fail, Strings.get(R.string.common_google_account),
                if (signedOut.size == 1) Strings.get(R.string.diagnostics_account_sign_in_again, signedOut.first().id)
                else Strings.plural(R.plurals.diagnostics_accounts_sign_in_again, signedOut.size),
                HealthFix.SIGN_IN, StatusText.fixLabel(StatusText.Fix.SIGN_IN)))
            else -> {
                val checkedAt = s.accounts.mapNotNull { s.checks[it.id] }.filter { it.ok }.maxOfOrNull { it.at }
                val who = if (s.accounts.size == 1) Strings.get(R.string.diagnostics_signed_in_as, s.accounts.first().id)
                else Strings.plural(R.plurals.diagnostics_accounts_signed_in, s.accounts.size)
                val text = listOfNotNull(who, checkedAt?.let { Strings.get(R.string.diagnostics_checked_lower, Fmt.clock(it)) }).joinToString(" · ")
                add(HealthItem("account", HealthState.Ok, Strings.get(R.string.common_google_account), text))
            }
        }

        // 3 · battery
        if (d != null) {
            if (d.batteryUnrestricted) {
                add(HealthItem("battery", HealthState.Ok, Strings.get(R.string.diagnostics_battery), Strings.get(R.string.diagnostics_battery_ok)))
            } else {
                add(HealthItem("battery", HealthState.Warn, Strings.get(R.string.diagnostics_battery),
                    vendorLabel?.let { Strings.get(R.string.diagnostics_battery_optimised_vendor, it) } ?: Strings.get(R.string.diagnostics_battery_optimised),
                    HealthFix.BATTERY, Strings.get(R.string.diagnostics_fix)))
            }
        }

        // 4 · background work
        val periodic = s.workInfos.filter { it.periodicityInfo != null && !it.state.isFinished }
        val instantExpected = s.settings?.instantMonitoring == true && s.pairs.any { it.enabled && !it.paused && it.instantSync }
        when {
            periodic.isEmpty() -> add(HealthItem("background", HealthState.Fail, Strings.get(R.string.diagnostics_background_work),
                Strings.get(R.string.diagnostics_no_scheduled_checks), HealthFix.REARM, Strings.get(R.string.diagnostics_rearm)))
            instantExpected && !s.monitorRunning -> add(HealthItem("background", HealthState.Warn, Strings.get(R.string.diagnostics_background_work),
                Strings.get(R.string.diagnostics_instant_stopped), HealthFix.REARM, Strings.get(R.string.diagnostics_rearm)))
            else -> {
                val next = periodic.mapNotNull { nextRun(it) }.minOrNull()
                val stopped = s.workInfos.firstOrNull { DiagnosticsReport.isStopped(it) }
                val parts = buildList<String> {
                    add(next?.let { Strings.get(R.string.diagnostics_next_scheduled_check, clockOrDay(it)) } ?: Strings.get(R.string.diagnostics_scheduled_checks_on))
                    if (instantExpected) add(Strings.get(R.string.diagnostics_instant_sync_on))
                    if (stopped != null) add(Strings.get(R.string.diagnostics_last_stopped, stopReasonLabel(DiagnosticsReport.stopReasonOf(stopped))))
                }
                add(HealthItem("background", HealthState.Ok, Strings.get(R.string.diagnostics_background_work), parts.joinToString(" · ")))
            }
        }

        // 5 · network
        val network = Strings.get(R.string.common_network)
        val n = s.network
        val wifiOnlyWaiting = s.pairs.any { it.enabled && !it.paused && it.wifiOnly }
        when {
            s.ssidRulesUsed && d != null && !d.fineLocation -> add(HealthItem("network", HealthState.Warn, network,
                Strings.get(R.string.diagnostics_wifi_rules_need_location), HealthFix.LOCATION, Strings.get(R.string.diagnostics_allow_location)))
            n == null -> add(HealthItem("network", HealthState.Wait, network, Strings.get(R.string.diagnostics_checking)))
            !n.online -> add(HealthItem("network", HealthState.Wait, network, Strings.get(R.string.diagnostics_offline)))
            n.wifi -> add(HealthItem("network", HealthState.Ok, network,
                buildList<String> {
                    add(Strings.get(R.string.diagnostics_wifi))
                    if (n.metered) add(Strings.get(R.string.diagnostics_metered))
                    if (n.vpn) add(Strings.get(R.string.diagnostics_vpn))
                }.joinToString(" · ")))
            wifiOnlyWaiting -> add(HealthItem("network", HealthState.Wait, network, Strings.get(R.string.diagnostics_mobile_data_wifi_waiting)))
            else -> add(HealthItem("network", HealthState.Ok, network,
                buildList<String> {
                    add(Strings.get(R.string.diagnostics_mobile_data))
                    if (n.roaming) add(Strings.get(R.string.diagnostics_roaming))
                    if (n.vpn) add(Strings.get(R.string.diagnostics_vpn))
                }.joinToString(" · ")))
        }

        // 6 · Drive storage, one row per account
        val storage = Strings.get(R.string.common_drive_storage)
        s.accounts.forEach { a ->
            val prefix = if (s.accounts.size > 1) "${a.id}: " else ""
            val used = a.storageUsed
            val limit = a.storageLimit
            val key = "storage-${a.id}"
            when {
                used == null -> add(HealthItem(key, HealthState.Wait, storage,
                    prefix + Strings.get(if (s.checking) R.string.diagnostics_checking else R.string.diagnostics_not_checked_yet)))
                limit == null || limit <= 0 -> add(HealthItem(key, HealthState.Ok, storage, prefix + Strings.get(R.string.diagnostics_storage_no_limit, Fmt.bytes(used))))
                used >= limit -> add(HealthItem(key, HealthState.Fail, storage, prefix + Strings.get(R.string.diagnostics_storage_full),
                    HealthFix.FREE_SPACE, StatusText.fixLabel(StatusText.Fix.FREE_SPACE)))
                (limit - used) * 10 < limit -> add(HealthItem(key, HealthState.Warn, storage,
                    prefix + Strings.get(R.string.diagnostics_storage_low, Fmt.bytes(limit - used), Fmt.bytes(limit)),
                    HealthFix.FREE_SPACE, StatusText.fixLabel(StatusText.Fix.FREE_SPACE)))
                else -> add(HealthItem(key, HealthState.Ok, storage, prefix + Strings.get(R.string.diagnostics_storage_used, Fmt.bytes(used), Fmt.bytes(limit))))
            }
        }

        // 7 · device sync (FR-B7): whether the phone takes part, then each device's reachability and path
        addAll(devices(s))

        // notifications: only when blocked
        if (d != null && !d.notifications) {
            add(HealthItem("notifications", HealthState.Warn, Strings.get(R.string.diagnostics_notifications), Strings.get(R.string.diagnostics_notifications_off),
                HealthFix.NOTIFICATIONS, Strings.get(R.string.diagnostics_allow)))
        }
    }

    /** Device sync rows: one for the phone, one per other device with how it is reached (LAN, direct, hole punched, none). */
    fun devices(s: DiagnosticsUiState): List<HealthItem> = buildList {
        val title = Strings.get(R.string.diagnostics_devices)
        val info = s.devices
        if (!s.devicesOpen || info == null) {
            add(HealthItem("devices", HealthState.Wait, title, Strings.get(R.string.diagnostics_devices_off)))
            return@buildList
        }
        val others = info.devices.filter { !it.thisDevice }
        val listen = info.listen
        add(HealthItem("devices", HealthState.Ok, title, when {
            others.isEmpty() -> Strings.get(R.string.diagnostics_devices_none)
            listen == null -> Strings.get(R.string.devices_taking_part)
            else -> Strings.get(R.string.diagnostics_devices_listening, ltr(listen))
        }))
        others.forEach { d ->
            val (state, text) = when {
                !d.connected || d.path == null -> HealthState.Wait to Strings.get(R.string.diagnostics_device_none)
                d.path == LinkPath.LAN -> HealthState.Ok to Strings.get(R.string.diagnostics_device_lan)
                d.path == LinkPath.DIRECT -> HealthState.Ok to Strings.get(R.string.diagnostics_device_direct)
                else -> HealthState.Ok to Strings.get(R.string.diagnostics_device_punched)
            }
            add(HealthItem("device-${d.id}", state, d.name, text))
        }
    }

    /** Why Android stopped background work, in the app's language (the report keeps the English of [DiagnosticsReport.stopReasonText]). */
    fun stopReasonLabel(code: Int): String = when (code) {
        WorkInfo.STOP_REASON_NOT_STOPPED, 0 -> Strings.get(R.string.diagnostics_stop_not_stopped)
        1 -> Strings.get(R.string.diagnostics_stop_cancelled_by_app)
        2 -> Strings.get(R.string.diagnostics_stop_preempted)
        3 -> Strings.get(R.string.diagnostics_stop_timeout)
        4 -> Strings.get(R.string.diagnostics_stop_device_state)
        5 -> Strings.get(R.string.diagnostics_stop_battery_low)
        6 -> Strings.get(R.string.diagnostics_stop_charger)
        7 -> Strings.get(R.string.diagnostics_stop_network)
        8 -> Strings.get(R.string.diagnostics_stop_not_idle)
        9 -> Strings.get(R.string.diagnostics_stop_storage_low)
        10 -> Strings.get(R.string.diagnostics_stop_quota)
        11 -> Strings.get(R.string.diagnostics_stop_restricted)
        12 -> Strings.get(R.string.diagnostics_stop_bucket)
        13 -> Strings.get(R.string.diagnostics_stop_user)
        14 -> Strings.get(R.string.diagnostics_stop_system)
        15 -> Strings.get(R.string.diagnostics_stop_launch_time)
        16 -> Strings.get(R.string.diagnostics_stop_fgs_limit)
        WorkInfo.STOP_REASON_UNKNOWN -> Strings.get(R.string.diagnostics_stop_unknown)
        else -> Strings.get(R.string.diagnostics_stop_code, code)
    }

    private fun nextRun(w: WorkInfo): Long? {
        if (w.state != WorkInfo.State.ENQUEUED) return null
        val next = w.nextScheduleTimeMillis
        return if (next == Long.MAX_VALUE || next <= 0L) null else next
    }

    /** "14:30" today, otherwise "Tomorrow 09:15" / "Oct 4, 09:15". */
    private fun clockOrDay(epochMs: Long): String {
        val zone = ZoneId.systemDefault()
        val day = Instant.ofEpochMilli(epochMs).atZone(zone).toLocalDate()
        val today = LocalDate.now(zone)
        return when (day) {
            today -> Fmt.clock(epochMs)
            today.plusDays(1) -> Strings.get(R.string.diagnostics_tomorrow_clock, Fmt.clock(epochMs))
            else -> Fmt.dayAndTime(epochMs)
        }
    }
}
