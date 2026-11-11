package com.almodhayan.drivesync.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import com.kahflabs.wasl.ffi.WaslException
import java.io.EOFException
import java.io.FileNotFoundException
import java.io.IOException
import java.io.InterruptedIOException
import java.net.ConnectException
import java.net.SocketException
import java.net.SocketTimeoutException
import java.net.UnknownHostException
import java.util.concurrent.CancellationException
import javax.net.ssl.SSLException
import javax.net.ssl.SSLHandshakeException

class AppErrorTest {

    // ---------------------------------------------------------------- wrap

    @Test
    fun wrap_returnsAppErrorsUnchanged() {
        val e = AppError.NotFound("gone")
        assertSame(e, AppError.wrap(e))
    }

    @Test
    fun wrap_mapsCancellation() {
        assertTrue(AppError.wrap(CancellationException("stop")) is AppError.Cancelled)
        // kotlinx.coroutines.CancellationException is a typealias of the JDK one
        assertTrue(AppError.wrap(kotlinx.coroutines.CancellationException("stop")) is AppError.Cancelled)
    }

    @Test
    fun wrap_showsLibwaslSentenceWithoutGeneratedPrefix() {
        val e = AppError.wrap(WaslException.Other("That is not a Wasl pairing code."))
        assertEquals("That is not a Wasl pairing code.", e.userHint)
    }

    @Test
    fun wrap_showsTheEnginesIsSyncingRefusal() {
        // updatePair refuses a change of folders during a sync (#101, #104); the editor shows this sentence.
        val sentence = "Photos is syncing. Stop it or let it finish, then change its folders."
        assertEquals(sentence, AppError.wrap(WaslException.Other(sentence)).userHint)
    }

    @Test
    fun wrap_mapsConnectivityFailuresToNetwork() {
        val causes = listOf<Throwable>(
            UnknownHostException("www.googleapis.com"),
            SocketTimeoutException("timeout"),
            ConnectException("Connection refused"),
            SSLException("handshake"),
            SSLHandshakeException("bad cert"),
            InterruptedIOException("interrupted"),
            SocketException("Connection reset"),
        )
        for (t in causes) {
            val e = AppError.wrap(t)
            assertTrue("${t::class.java.simpleName} should map to Network, got ${e::class.java.simpleName}", e is AppError.Network)
            assertSame(t, e.cause)
            assertTrue(e.isTransient)
        }
    }

    @Test
    fun wrap_keepsTheOriginalMessageForNetworkErrors() {
        assertEquals("www.googleapis.com", AppError.wrap(UnknownHostException("www.googleapis.com")).message)
        assertEquals("network error", AppError.wrap(UnknownHostException()).message)
    }

    @Test
    fun wrap_mapsOtherIoExceptionsToIo() {
        val e = AppError.wrap(IOException("disk full"))
        assertTrue(e is AppError.Io)
        assertEquals("disk full", e.message)
        assertFalse(e.isTransient)
        assertTrue(AppError.wrap(FileNotFoundException("x")) is AppError.Io)
        assertTrue(AppError.wrap(EOFException()) is AppError.Io)
        assertEquals("I/O error", AppError.wrap(IOException()).message)
    }

    @Test
    fun wrap_mapsEverythingElseToUnknown() {
        val e = AppError.wrap(IllegalStateException("boom"))
        assertTrue(e is AppError.Unknown)
        assertEquals("boom", e.message)
        assertNotNull(e.cause)
        // without a message the class name is used
        assertEquals("RuntimeException", AppError.wrap(RuntimeException()).message)
    }

    // ---------------------------------------------------------------- isTransient

    @Test
    fun isTransient_onlyForNetworkRateLimitAndServerErrors() {
        assertTrue(AppError.Network("n").isTransient)
        assertTrue(AppError.RateLimited(0, "r").isTransient)
        assertTrue(AppError.Http(500, null, "h").isTransient)
        assertTrue(AppError.Http(503, "backendError", "h").isTransient)

        assertFalse(AppError.Http(499, null, "h").isTransient)
        assertFalse(AppError.Http(403, "domainPolicy", "h").isTransient)
        assertFalse(AppError.Http(412, null, "h").isTransient)
        assertFalse(AppError.Auth("a").isTransient)
        assertFalse(AppError.StorageQuota("s").isTransient)
        assertFalse(AppError.NotFound("n").isTransient)
        assertFalse(AppError.Permission("p").isTransient)
        assertFalse(AppError.Io("i").isTransient)
        assertFalse(AppError.Integrity("i").isTransient)
        assertFalse(AppError.Locked("l").isTransient)
        assertFalse(AppError.Cancelled().isTransient)
        assertFalse(AppError.Unknown("u").isTransient)
    }

    // ---------------------------------------------------------------- userHint

    private fun everySubclass(): List<AppError> = listOf(
        AppError.Auth("token rejected"),
        AppError.Auth("needs consent", resolution = null, cause = IllegalStateException("x")),
        AppError.Network("dropped", UnknownHostException()),
        AppError.RateLimited(5000, "slow down"),
        AppError.StorageQuota("full"),
        AppError.NotFound("missing"),
        AppError.Permission("denied"),
        AppError.Http(500, "backendError", "server"),
        AppError.Http(418, null, "teapot"),
        AppError.Io("read failed"),
        AppError.Integrity("md5 differs"),
        AppError.Locked("vault has no keys on this device"),
        AppError.Cancelled(),
        AppError.Unknown("weird"),
    )

    @Test
    fun userHint_isNonBlankForEverySubclass() {
        for (e in everySubclass()) {
            assertTrue("${e::class.java.simpleName} has a blank hint", e.userHint.isNotBlank())
        }
    }

    @Test
    fun userHint_isNonBlankForUnknownErrorsWithoutMessage() {
        // Anonymous exception classes have an empty simpleName; the hint must still say something.
        val anonymous = object : RuntimeException() {}
        assertTrue(AppError.wrap(anonymous).userHint.isNotBlank())
        assertTrue(AppError.Unknown("").userHint.isNotBlank())
        assertTrue(AppError.Unknown("   ").userHint.isNotBlank())
    }

    @Test
    fun userHint_containsActionableDetails() {
        assertEquals("Google is rate limiting requests. Waiting 5s before retrying.", AppError.RateLimited(5000, "x").userHint)
        assertEquals("Google Drive returned HTTP 403 (rateLimitExceeded).", AppError.Http(403, "rateLimitExceeded", "x").userHint)
        assertEquals("Google Drive returned HTTP 500.", AppError.Http(500, null, "x").userHint)
        assertEquals("Local storage error: disk full", AppError.Io("disk full").userHint)
        assertEquals("Transferred data did not verify: md5 differs", AppError.Integrity("md5 differs").userHint)
        assertEquals("weird", AppError.Unknown("weird").userHint)
        assertEquals("Cancelled.", AppError.Cancelled().userHint)
        assertTrue(AppError.Auth("x").userHint.contains("re-authorize"))
        assertTrue(AppError.Permission("x").userHint.contains("All files access"))
    }

    @Test
    fun auth_carriesOptionalResolution() {
        assertNull(AppError.Auth("x").resolution)
        assertNull(AppError.Auth("x", null).cause)
    }

    @Test
    fun rateLimited_exposesRetryAfter() {
        assertEquals(2500L, AppError.RateLimited(2500, "x").retryAfterMs)
    }

    @Test
    fun http_exposesCodeAndReason() {
        val e = AppError.Http(403, "storageQuotaExceeded", "m")
        assertEquals(403, e.code)
        assertEquals("storageQuotaExceeded", e.reason)
        assertEquals("m", e.message)
    }
}
