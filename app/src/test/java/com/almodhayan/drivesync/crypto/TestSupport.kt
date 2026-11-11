package com.almodhayan.drivesync.crypto

import com.almodhayan.drivesync.core.AppError
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import java.util.Random

internal object TestSupport {
    /** Fixed keys shared with the Python reference vectors (keyEnc = 0x11 * 32, keyName = 0x22 * 32). */
    fun fixedKeys(): VaultKeys = VaultKeys(ByteArray(32) { 0x11 }, ByteArray(32) { 0x22 })

    fun otherKeys(): VaultKeys = VaultKeys(ByteArray(32) { 0x33 }, ByteArray(32) { 0x44 })

    fun randomBytes(n: Int, seed: Long = 42L): ByteArray = ByteArray(n).also { Random(seed).nextBytes(it) }

    fun hex(bytes: ByteArray): String = bytes.joinToString("") { "%02x".format(it) }

    fun unhex(s: String): ByteArray = ByteArray(s.length / 2) { s.substring(it * 2, it * 2 + 2).toInt(16).toByte() }

    inline fun assertIntegrity(block: () -> Unit): AppError.Integrity {
        try {
            block()
        } catch (e: AppError.Integrity) {
            return e
        } catch (t: Throwable) {
            fail("expected AppError.Integrity but got ${t::class.java.name}: ${t.message}")
        }
        fail("expected AppError.Integrity but nothing was thrown")
        throw IllegalStateException()
    }

    fun assertMessageContains(e: Throwable, fragment: String) {
        assertTrue("message \"${e.message}\" should contain \"$fragment\"", e.message?.contains(fragment) == true)
    }
}
