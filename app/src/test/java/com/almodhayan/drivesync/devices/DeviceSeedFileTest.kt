package com.almodhayan.drivesync.devices

import com.kahflabs.wasl.ffi.WaslException
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.security.SecureRandom
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey

/**
 * The device key seed file (FR-N1): wrapped with AES-GCM, never in the clear, damaged or foreign
 * files refused. On the phone the wrap key is the Keystore's; here a software key stands in.
 */
class DeviceSeedFileTest {
    @get:Rule val tmp = TemporaryFolder()

    private class SoftKey : SeedWrapKey {
        val k: SecretKey = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
        override fun key(): SecretKey = k
    }

    private fun seed() = ByteArray(32).also { SecureRandom().nextBytes(it) }

    private fun containsRun(hay: ByteArray, needle: ByteArray, run: Int = 8): Boolean =
        (0..hay.size - run).any { i -> (0..needle.size - run).any { j -> (0 until run).all { hay[i + it] == needle[j + it] } } }

    @Test fun `no file means no key yet`() {
        assertNull(DeviceSeedFile(File(tmp.root, "devices/device.key"), SoftKey()).load())
    }

    @Test fun `a saved seed loads back and is never stored in the clear`() {
        val file = File(tmp.root, "devices/device.key")
        val store = DeviceSeedFile(file, SoftKey())
        val s = seed()
        store.save(s.copyOf())
        assertArrayEquals(s, store.load())
        val blob = file.readBytes()
        assertEquals(12 + 32 + 16, blob.size)
        assertFalse(containsRun(blob, s))
        assertFalse(File(file.parentFile, "device.key.tmp").exists())
    }

    @Test fun `saving replaces the old seed and wipes the caller's copy`() {
        val store = DeviceSeedFile(File(tmp.root, "device.key"), SoftKey())
        store.save(seed())
        val s = seed()
        val given = s.copyOf()
        store.save(given)
        assertTrue(given.all { it == 0.toByte() })
        assertArrayEquals(s, store.load())
    }

    @Test fun `only 32-byte seeds are kept`() {
        val store = DeviceSeedFile(File(tmp.root, "device.key"), SoftKey())
        assertThrows(WaslException::class.java) { store.save(ByteArray(16)) }
        assertNull(store.load())
    }

    @Test fun `a tampered file is refused`() {
        val file = File(tmp.root, "device.key")
        val store = DeviceSeedFile(file, SoftKey())
        store.save(seed())
        val blob = file.readBytes()
        blob[20] = (blob[20].toInt() xor 1).toByte()
        file.writeBytes(blob)
        assertThrows(WaslException::class.java) { store.load() }
        file.writeBytes(blob.copyOf(30))
        assertThrows(WaslException::class.java) { store.load() }
    }

    @Test fun `another phone's wrap key can't open it`() {
        val file = File(tmp.root, "device.key")
        DeviceSeedFile(file, SoftKey()).save(seed())
        val e = assertThrows(WaslException::class.java) { DeviceSeedFile(file, SoftKey()).load() }
        // The message is for people: it never carries key bytes or the provider's text.
        assertTrue(e.message.orEmpty().contains("device key"))
    }
}
