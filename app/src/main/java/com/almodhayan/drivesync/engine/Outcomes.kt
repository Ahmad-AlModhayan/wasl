package com.almodhayan.drivesync.engine

import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.ErrorKind
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.sync.Blockers
import com.almodhayan.drivesync.sync.RunOutcome
import com.kahflabs.wasl.ffi.core.Blocker
import com.kahflabs.wasl.ffi.engine.Trigger
import com.kahflabs.wasl.ffi.engine.RunOutcome as WRunOutcome

/** The engine's message for a run abandoned by its watchdog (FR-B4). Engine text: compare, don't show. */
const val STALLED = "Stopped after 15 minutes without progress. Trying again shortly."

/**
 * A run's message as the person reads it: the engine's fixed messages in the app's language, any
 * other (free-form) engine text as the engine sent it.
 */
fun engineMessageText(message: String?): String? = when {
    message == STALLED -> Strings.get(R.string.engine_stalled)
    // An empty note clears a pair's wait (device sync, which the engine does not record as a run).
    message.isNullOrBlank() -> null
    else -> message
}

internal fun WRunOutcome.toKotlin(): RunOutcome = RunOutcome(
    runId = runId,
    status = status.toKotlin(),
    message = message,
    summary = summary?.toKotlin(),
    errorKind = errorKind?.let { ErrorKind.parse(it.name) },
)

/** What a pair is waiting for, as the status line says it. */
fun Blocker.text(): String = when (this) {
    Blocker.Offline -> Blockers.text(Blockers.Kind.OFFLINE)
    Blocker.WaitingForWifi -> Blockers.text(Blockers.Kind.WIFI)
    Blocker.MeteredConnection -> Blockers.text(Blockers.Kind.METERED)
    Blocker.Roaming -> Blockers.text(Blockers.Kind.ROAMING)
    Blocker.WaitingForVpn -> Blockers.text(Blockers.Kind.VPN)
    Blocker.WaitingForCharger -> Blockers.text(Blockers.Kind.CHARGER)
    is Blocker.BatteryBelow -> Blockers.text(Blockers.Kind.BATTERY, percent.toInt())
    Blocker.WifiNotAllowed -> Blockers.text(Blockers.Kind.SSID_NOT_ALLOWED)
    Blocker.WifiDenied -> Blockers.text(Blockers.Kind.SSID_DENIED)
}

/**
 * The engine's trigger for the app's trigger names. Manual runs only need a connection; automation
 * and background runs respect the pair's conditions (FR-P4).
 */
fun triggerFor(name: String, manual: Boolean): Trigger = when {
    name == "preview" -> Trigger.PREVIEW
    manual -> Trigger.MANUAL
    name == "automation" -> Trigger.AUTOMATION
    name in setOf("instant", "remote-change", "local-change", "follow-up", "share", "shortcut") -> Trigger.INSTANT
    else -> Trigger.SCHEDULE
}
