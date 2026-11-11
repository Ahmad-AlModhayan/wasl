package com.almodhayan.drivesync.devices

import android.security.keystore.StrongBoxUnavailableException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertThrows
import org.junit.Test
import java.security.ProviderException
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import kotlin.concurrent.thread

/**
 * S4-5: a new wrap key goes into StrongBox when the phone has one and falls back to the TEE when
 * it doesn't, so a phone without StrongBox still gets a key. A failure of the TEE itself is not
 * hidden.
 */
class WrapKeyFallbackTest {
    private val key: SecretKey = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()

    @Test fun `StrongBox is tried first and kept when it works`() {
        val tried = mutableListOf<Boolean>()
        var discarded = 0
        val k = AndroidKeystoreWrapKey.generateWithFallback({ tried += it; key }, { discarded++ })
        assertSame(key, k)
        assertEquals(listOf(true), tried)
        assertEquals(0, discarded)
    }

    @Test fun `no StrongBox falls back to the TEE`() {
        val tried = mutableListOf<Boolean>()
        var discarded = 0
        val k = AndroidKeystoreWrapKey.generateWithFallback({ sb -> tried += sb; if (sb) throw StrongBoxUnavailableException() else key }, { discarded++ })
        assertSame(key, k)
        assertEquals(listOf(true, false), tried)
        assertEquals(1, discarded)
    }

    @Test fun `a StrongBox that can't hold the key falls back too`() {
        val tried = mutableListOf<Boolean>()
        val k = AndroidKeystoreWrapKey.generateWithFallback({ sb -> tried += sb; if (sb) throw ProviderException("no AES-256") else key }, {})
        assertSame(key, k)
        assertEquals(listOf(true, false), tried)
    }

    @Test fun `a failing TEE is reported`() {
        assertThrows(ProviderException::class.java) {
            AndroidKeystoreWrapKey.generateWithFallback({ throw ProviderException("keystore down") }, {})
        }
    }

    @Test fun `a key found under the alias after a failed attempt is used, never discarded`() {
        var discarded = 0
        val k = AndroidKeystoreWrapKey.generateWithFallback(
            { sb -> if (sb) throw StrongBoxUnavailableException() else error("not generated again") },
            { discarded++ },
            { key },
        )
        assertSame(key, k)
        assertEquals(0, discarded)
    }

    /**
     * Review of S2–S5: two first uses at once. The first one's StrongBox key must not be deleted
     * by the second one's fallback: both get the one key that stays under the alias.
     */
    @Test fun `concurrent first uses share one key and never delete it`() {
        val stored = AtomicReference<SecretKey?>(null)
        val attempts = AtomicInteger(0)
        val inside = CountDownLatch(1)
        val generate = { sb: Boolean ->
            // The first attempt takes a while in StrongBox; any later StrongBox attempt fails.
            if (sb && attempts.getAndIncrement() > 0) throw StrongBoxUnavailableException()
            if (sb) { inside.countDown(); Thread.sleep(200) }
            KeyGenerator.getInstance("AES").apply { init(256) }.generateKey().also { stored.set(it) }
        }
        val alias = "test-concurrent-${System.nanoTime()}"
        val got = arrayOfNulls<SecretKey>(2)
        val first = thread { got[0] = AndroidKeystoreWrapKey.getOrGenerate(alias, { stored.get() }, generate, { stored.set(null) }) }
        inside.await(5, TimeUnit.SECONDS)
        val second = thread { got[1] = AndroidKeystoreWrapKey.getOrGenerate(alias, { stored.get() }, generate, { stored.set(null) }) }
        first.join(); second.join()
        assertNotNull(stored.get())
        assertSame(stored.get(), got[0])
        assertSame(stored.get(), got[1])
        assertEquals(1, attempts.get())
    }
}
