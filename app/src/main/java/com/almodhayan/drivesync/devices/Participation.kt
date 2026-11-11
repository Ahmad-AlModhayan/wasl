package com.almodhayan.drivesync.devices

import com.kahflabs.wasl.ffi.core.Route
import com.kahflabs.wasl.ffi.core.RunStatus
import com.kahflabs.wasl.ffi.engine.RunOutcome

/**
 * Why the phone takes part in device sync right now (SRS FR-N12). Device sync runs only while at
 * least one of these holds; outside them a pair synced only with devices waits.
 */
enum class Window {
    /** The app is on screen. */
    APP_OPEN,

    /** The person started "Sync with my devices now" (a foreground service with a notification). */
    SESSION,

    /** Charging on Wi-Fi, when the person allowed it in Settings. */
    CHARGING_WIFI,
}

/** The pure decisions of device sync: when the phone takes part, and how a pair runs by its route. */
object Participation {
    /**
     * Whether the charging-on-Wi-Fi window is open: only when the person turned it on, and only
     * while the phone charges on an unmetered Wi-Fi (or Ethernet) network.
     */
    fun chargingWindow(enabled: Boolean, charging: Boolean, wifi: Boolean, metered: Boolean): Boolean =
        enabled && charging && wifi && !metered

    /** Device sync is open while any window is. */
    fun shouldOpen(windows: Set<Window>): Boolean = windows.isNotEmpty()

    /** How one pair runs: through Drive, with devices, both, or it waits for the phone to take part. */
    data class RunPlan(val drive: Boolean, val devices: Boolean, val waiting: Boolean)

    fun plan(route: Route, devicesOpen: Boolean): RunPlan = when (route) {
        Route.DRIVE -> RunPlan(drive = true, devices = false, waiting = false)
        Route.DRIVE_AND_DEVICES -> RunPlan(drive = true, devices = devicesOpen, waiting = false)
        Route.DEVICES -> RunPlan(drive = false, devices = devicesOpen, waiting = !devicesOpen)
    }

    /**
     * One outcome for a pair that ran through Drive and with devices: Drive's, made partial when the
     * devices part failed, with the devices' note after it (as the desktop daemon does).
     */
    fun merge(drive: RunOutcome, devices: RunOutcome, devicesLabel: String): RunOutcome {
        val failed = devices.status == RunStatus.FAILED || devices.status == RunStatus.PARTIAL
        val status = if (failed && drive.status == RunStatus.SUCCESS) RunStatus.PARTIAL else drive.status
        val note = devices.message?.let { "$devicesLabel: $it" }
        val message = when {
            note == null -> drive.message
            drive.message == null -> note
            else -> "${drive.message} $note"
        }
        return drive.copy(status = status, message = message)
    }

    /**
     * Who a pair synced only with devices is waiting for: the other devices of the group by name,
     * or null when the phone has not learned any yet.
     */
    fun waitingFor(names: List<String>, separator: String): String? =
        names.map { it.trim() }.filter { it.isNotEmpty() }.distinct().takeIf { it.isNotEmpty() }?.joinToString(separator)
}
