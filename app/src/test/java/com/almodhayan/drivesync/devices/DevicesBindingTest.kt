package com.almodhayan.drivesync.devices

import com.almodhayan.drivesync.engine.toEngine
import com.kahflabs.wasl.ffi.DeviceEvents
import com.kahflabs.wasl.ffi.DevicesInit
import com.kahflabs.wasl.ffi.EngineInit
import com.kahflabs.wasl.ffi.Platform
import com.kahflabs.wasl.ffi.SafBridge
import com.kahflabs.wasl.ffi.TokenSource
import com.kahflabs.wasl.ffi.VaultAccess
import com.kahflabs.wasl.ffi.WaslDevices
import com.kahflabs.wasl.ffi.WaslEngine
import com.kahflabs.wasl.ffi.core.DeviceState
import com.kahflabs.wasl.ffi.core.Route
import com.kahflabs.wasl.ffi.core.RunStatus
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import uniffi.wasl_devices.PairingStage
import java.io.File
import java.util.concurrent.CopyOnWriteArrayList
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey

/**
 * Device sync through the Kotlin binding (FR-N1, N2, N12): two phones with the app's key store
 * pair over 127.0.0.1, confirm the four words, share a pair and sync a file. Pins the generated
 * types and callbacks the app relies on.
 */
class DevicesBindingTest {
    @get:Rule val tmp = TemporaryFolder()

    private class Tokens : TokenSource {
        override fun accessToken(account: String): String = "tok"
        override fun invalidate(account: String) {}
    }

    private class Device : Platform {
        override fun deviceState() = DeviceState(online = true, wifi = true, metered = false, vpn = false, roaming = false, ssid = null, charging = true, batteryPercent = 100u)
        override fun saf(): SafBridge? = null
        override fun vaultAccess(vaultId: String): VaultAccess? = null
    }

    private class SoftKey : SeedWrapKey {
        val k: SecretKey = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
        override fun key(): SecretKey = k
    }

    private class Events : DeviceEvents {
        val changed = java.util.concurrent.atomic.AtomicInteger()
        val pairs = CopyOnWriteArrayList<String>()
        override fun changed() { changed.incrementAndGet() }
        override fun pairChanged(pairId: String) { pairs += pairId }
    }

    private class Phone(val engine: WaslEngine, val devices: WaslDevices, val root: File, val events: Events, val keyFile: File)

    private val phones = ArrayList<Phone>()

    private suspend fun phone(name: String, key: SoftKey = SoftKey(), dir: File = tmp.newFolder(name)): Phone {
        val root = File(dir, "Shared").apply { mkdirs() }
        val engine = WaslEngine(
            EngineInit(
                dbPath = File(dir, "wasl.db").path, binDir = File(dir, "bin").path, stagingDir = File(dir, "staging").path,
                deviceName = name, driveBaseUrl = "http://127.0.0.1:9/drive/v3", driveUploadUrl = "http://127.0.0.1:9/upload/drive/v3",
            ),
            Tokens(), Device(), null,
        )
        if (engine.pair("p") == null) {
            val base = com.almodhayan.drivesync.data.db.FolderPairEntity(id = "p", accountId = "", name = "Shared", localPath = root.path, remoteFolderId = "", remotePath = "")
            engine.createPair(base.toEngine(null).copy(route = Route.DEVICES))
            // A new pair waits for its preview (ADR-0001); these phones have started theirs.
            engine.setPairFlags(base.id, null, null, false)
        }
        val events = Events()
        val keyFile = File(dir, "devices/device.key")
        val devices = WaslDevices.open(
            engine,
            DevicesInit(dataDir = File(dir, "devices").apply { mkdirs() }.path, deviceName = name, port = 0u, bindAddress = "127.0.0.1", discover = false),
            DeviceSeedFile(keyFile, key),
            events,
        )
        return Phone(engine, devices, root, events, keyFile).also { phones += it }
    }

    private suspend fun until(p: Phone, stage: PairingStage): PairingStage {
        repeat(200) {
            val s = p.devices.pairingStatus()
            if (s.stage == stage || s.stage == PairingStage.FAILED) return s.stage
            delay(50)
        }
        return p.devices.pairingStatus().stage
    }

    @After fun close() {
        phones.forEach { runCatching { it.devices.shutdown() }; runCatching { it.devices.destroy() }; runCatching { it.engine.close() } }
    }

    @Test fun `the device key is kept wrapped and the phone keeps its id`() = runBlocking {
        val key = SoftKey()
        val dir = tmp.newFolder("one")
        val first = phone("Pixel", key, dir)
        val me = first.devices.info().devices.single { it.thisDevice }
        assertEquals("Pixel", me.name)
        assertTrue(me.id.startsWith("WASL-"))
        assertEquals(12 + 32 + 16, first.keyFile.length().toInt())
        first.devices.shutdown(); first.devices.destroy(); first.engine.close(); phones.clear()

        val again = phone("Pixel", key, dir)
        assertEquals(me.id, again.devices.info().devices.single { it.thisDevice }.id)
    }

    @Test fun `two phones pair, share a pair and sync a file`() = runBlocking {
        val a = phone("Pixel")
        val b = phone("Galaxy")
        val code = a.devices.invite().code!!
        b.devices.join(code)
        assertEquals(PairingStage.CONFIRM, until(a, PairingStage.CONFIRM))
        assertEquals(PairingStage.CONFIRM, until(b, PairingStage.CONFIRM))
        val words = a.devices.pairingStatus().words
        assertEquals(4, words.size)
        assertEquals(words, b.devices.pairingStatus().words)
        a.devices.confirmPairing(true)
        b.devices.confirmPairing(true)
        assertEquals(PairingStage.PAIRED, until(a, PairingStage.PAIRED))
        assertEquals(PairingStage.PAIRED, until(b, PairingStage.PAIRED))
        assertEquals(2, a.devices.info().devices.size)
        assertTrue("the screens heard about it", a.events.changed.get() > 0)

        val share = a.devices.sharePair("p", null)
        assertEquals(share, b.devices.sharePair("p", share))
        assertEquals(share, b.devices.shareCode("p"))
        assertEquals(Route.DEVICES, a.engine.pair("p")!!.route)
        b.devices.connect(a.devices.info().listen!!)
        a.devices.connect(b.devices.info().listen!!)
        assertTrue(a.devices.info().devices.any { !it.thisDevice && it.connected && it.path != null })

        File(a.root, "photo.jpg").writeBytes("pixels".toByteArray())
        val r = a.devices.syncPair("p")
        assertEquals(r.message, RunStatus.SUCCESS, r.status)
        repeat(100) { if (b.events.pairs.isEmpty()) delay(50) }
        assertEquals(listOf("p"), b.events.pairs.toList())
        b.devices.syncPair("p")
        assertArrayEquals("pixels".toByteArray(), File(b.root, "photo.jpg").readBytes())

        b.devices.unsharePair("p")
        assertEquals(null, b.devices.shareCode("p"))
    }
}
