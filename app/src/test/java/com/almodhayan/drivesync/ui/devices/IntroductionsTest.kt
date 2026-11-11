package com.almodhayan.drivesync.ui.devices

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.wasl_devices.DeviceInfo
import uniffi.wasl_devices.OfferInfo
import uniffi.wasl_devices.OfferStatus
import uniffi.wasl_devices.PairSharing

/** Introductions on the phone (docs/INTRODUCTIONS.md): what counts as new, who added it, which offers show. */
class IntroductionsTest {
    private fun dev(id: String, name: String, me: Boolean = false, by: String? = null, known: Boolean = true) =
        DeviceInfo(id = id, name = name, thisDevice = me, connected = false, path = null, addedAt = 0uL, addedBy = by, known = known)

    private fun offer(name: String, status: OfferStatus) =
        OfferInfo(share = "ab".repeat(16), fromDevice = "WASL-A", fromName = "Laptop", name = name, files = 3uL, bytes = 1000uL, firstSeen = 0L, status = status)

    private val me = dev("WASL-ME", "Galaxy A54", me = true)
    private val laptop = dev("WASL-A", "Laptop")
    private val tablet = dev("WASL-T", "Tablet", by = "WASL-A", known = false)

    @Test
    fun aDeviceIsNewOnlyWhenAnotherAddedItAndItIsNotKeptHere() {
        assertTrue(tablet.isNew())
        assertFalse(tablet.copy(known = true).isNew())
        assertFalse(laptop.isNew())
        assertFalse(me.copy(known = false).isNew())
    }

    @Test
    fun whoAddedADeviceIsNamedWhenThisPhoneKnowsIt() {
        assertEquals("Laptop", addedByName(tablet, listOf(me, laptop, tablet)))
        assertEquals("WASL-GONE", addedByName(tablet.copy(addedBy = "WASL-GONE"), listOf(me, tablet)))
        assertNull(addedByName(laptop, listOf(me, laptop)))
    }

    @Test
    fun onlyOffersWaitingForAnAnswerShow() {
        val shown = waiting(listOf(offer("Camera", OfferStatus.NEW), offer("Old", OfferStatus.DECLINED), offer("Taken", OfferStatus.ACCEPTED)))
        assertEquals(listOf("Camera"), shown.map { it.name })
    }

    @Test
    fun aSharedPairWaitsForItsDevicesOnlyWhenNoneIsConnected() {
        val others = listOf(laptop, tablet)
        val toLaptop = PairSharing(share = "s", allDevices = false, devices = listOf("WASL-A"))
        assertEquals(listOf("Laptop"), waitingFor(toLaptop, others))
        assertEquals(emptyList<String>(), waitingFor(toLaptop, listOf(laptop.copy(connected = true), tablet)))
        // All devices: waits only while every one of them is away.
        val toAll = PairSharing(share = "s", allDevices = true, devices = emptyList())
        assertEquals(listOf("Laptop", "Tablet"), waitingFor(toAll, others))
        assertEquals(emptyList<String>(), waitingFor(toAll, listOf(laptop, tablet.copy(connected = true))))
        assertEquals(emptyList<String>(), waitingFor(null, others))
        assertEquals(emptyList<String>(), waitingFor(PairSharing(share = "s", allDevices = false, devices = emptyList()), others))
    }
}
