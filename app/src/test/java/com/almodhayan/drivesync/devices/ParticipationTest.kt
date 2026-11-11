package com.almodhayan.drivesync.devices

import com.kahflabs.wasl.ffi.core.Route
import com.kahflabs.wasl.ffi.core.RunStatus
import com.kahflabs.wasl.ffi.engine.RunOutcome
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** When the phone takes part in device sync (FR-N12), and how a pair runs by its route. */
class ParticipationTest {
    private fun outcome(status: RunStatus, message: String? = null) =
        RunOutcome(runId = "r", status = status, message = message, summary = null, errorKind = null, blocker = null)

    @Test fun `charging window needs the setting, the charger and unmetered wifi`() {
        assertTrue(Participation.chargingWindow(enabled = true, charging = true, wifi = true, metered = false))
        assertFalse(Participation.chargingWindow(enabled = false, charging = true, wifi = true, metered = false))
        assertFalse(Participation.chargingWindow(enabled = true, charging = false, wifi = true, metered = false))
        assertFalse(Participation.chargingWindow(enabled = true, charging = true, wifi = false, metered = false))
        // A metered Wi-Fi (a phone's hotspot) is not the window.
        assertFalse(Participation.chargingWindow(enabled = true, charging = true, wifi = true, metered = true))
    }

    @Test fun `device sync is open while any window is`() {
        assertFalse(Participation.shouldOpen(emptySet()))
        for (w in Window.entries) assertTrue(w.name, Participation.shouldOpen(setOf(w)))
        assertTrue(Participation.shouldOpen(setOf(Window.APP_OPEN, Window.SESSION)))
    }

    @Test fun `drive pairs never touch devices`() {
        assertEquals(Participation.RunPlan(drive = true, devices = false, waiting = false), Participation.plan(Route.DRIVE, devicesOpen = true))
        assertEquals(Participation.RunPlan(drive = true, devices = false, waiting = false), Participation.plan(Route.DRIVE, devicesOpen = false))
    }

    @Test fun `drive and devices pairs run through Drive and add devices while open`() {
        assertEquals(Participation.RunPlan(drive = true, devices = true, waiting = false), Participation.plan(Route.DRIVE_AND_DEVICES, devicesOpen = true))
        // Outside the windows Drive carries on alone; nothing waits.
        assertEquals(Participation.RunPlan(drive = true, devices = false, waiting = false), Participation.plan(Route.DRIVE_AND_DEVICES, devicesOpen = false))
    }

    @Test fun `devices pairs run with devices while open and wait otherwise`() {
        assertEquals(Participation.RunPlan(drive = false, devices = true, waiting = false), Participation.plan(Route.DEVICES, devicesOpen = true))
        assertEquals(Participation.RunPlan(drive = false, devices = false, waiting = true), Participation.plan(Route.DEVICES, devicesOpen = false))
    }

    @Test fun `a failed devices part makes a successful drive run partial`() {
        val merged = Participation.merge(outcome(RunStatus.SUCCESS), outcome(RunStatus.FAILED, "couldn't reach laptop"), "Devices")
        assertEquals(RunStatus.PARTIAL, merged.status)
        assertEquals("Devices: couldn't reach laptop", merged.message)
        assertEquals("r", merged.runId)
    }

    @Test fun `merging keeps drive's own status and message first`() {
        val merged = Participation.merge(outcome(RunStatus.FAILED, "Drive said no."), outcome(RunStatus.SUCCESS, "2 change(s) here"), "Devices")
        assertEquals(RunStatus.FAILED, merged.status)
        assertEquals("Drive said no. Devices: 2 change(s) here", merged.message)
        val quiet = Participation.merge(outcome(RunStatus.SUCCESS, "ok"), outcome(RunStatus.SUCCESS), "Devices")
        assertEquals(RunStatus.SUCCESS, quiet.status)
        assertEquals("ok", quiet.message)
    }

    @Test fun `waiting names the other devices`() {
        assertNull(Participation.waitingFor(emptyList(), ", "))
        assertNull(Participation.waitingFor(listOf(" ", ""), ", "))
        assertEquals("Laptop", Participation.waitingFor(listOf("Laptop"), ", "))
        assertEquals("Laptop, Desktop", Participation.waitingFor(listOf("Laptop", " Desktop ", "Laptop"), ", "))
    }
}
