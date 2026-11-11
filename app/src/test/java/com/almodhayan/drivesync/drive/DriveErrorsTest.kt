package com.almodhayan.drivesync.drive

import com.almodhayan.drivesync.core.AppError
import okhttp3.Protocol
import okhttp3.Request
import okhttp3.Response
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class DriveErrorsTest {

    private fun response(code: Int, message: String = "Err", vararg headers: Pair<String, String>): Response {
        val b = Response.Builder()
            .request(Request.Builder().url("https://www.googleapis.com/drive/v3/files").build())
            .protocol(Protocol.HTTP_1_1)
            .code(code)
            .message(message)
        headers.forEach { (k, v) -> b.header(k, v) }
        return b.build()
    }

    private fun body(reason: String?, message: String = "Something went wrong", code: Int = 403, status: String? = null): String {
        val errors = if (reason == null) "" else ""","errors":[{"domain":"usageLimits","reason":"$reason","message":"$message"}]"""
        val st = if (status == null) "" else ""","status":"$status""""
        return """{"error":{"code":$code,"message":"$message"$errors$st}}"""
    }

    private fun map(code: Int, bodyText: String?, vararg headers: Pair<String, String>, message: String = "Err"): AppError =
        DriveErrors.fromResponse(response(code, message, *headers), bodyText)

    // ---------------------------------------------------------------- 401 / 403

    @Test
    fun unauthorized_isAuth() {
        val e = map(401, body("authError", "Invalid Credentials", code = 401))
        assertTrue(e is AppError.Auth)
        assertTrue(e.message!!.contains("Invalid Credentials"))
        assertNull((e as AppError.Auth).resolution)
    }

    @Test
    fun forbidden_rateLimitReasons_areRateLimited() {
        for (reason in listOf("userRateLimitExceeded", "rateLimitExceeded", "dailyLimitExceeded", "sharingRateLimitExceeded")) {
            val e = map(403, body(reason))
            assertTrue("$reason should be RateLimited", e is AppError.RateLimited)
            assertEquals(0L, (e as AppError.RateLimited).retryAfterMs)
            assertTrue(e.isTransient)
        }
    }

    @Test
    fun forbidden_rateLimit_honoursRetryAfterSeconds() {
        val e = map(403, body("userRateLimitExceeded"), "Retry-After" to "3") as AppError.RateLimited
        assertEquals(3000L, e.retryAfterMs)
    }

    @Test
    fun forbidden_storageQuotaReasons_areStorageQuota() {
        assertTrue(map(403, body("storageQuotaExceeded")) is AppError.StorageQuota)
        assertTrue(map(403, body("quotaExceeded")) is AppError.StorageQuota)
        // reason missing but the message mentions the storage quota
        assertTrue(map(403, body(null, "The user's Drive storage quota has been exceeded.")) is AppError.StorageQuota)
    }

    @Test
    fun forbidden_insufficientPermissions_isAuth() {
        val e = map(403, body("insufficientPermissions", "Insufficient Permission"))
        assertTrue(e is AppError.Auth)
        assertTrue(e.message!!.startsWith("Missing Drive permission"))
    }

    @Test
    fun forbidden_otherReason_isHttp403() {
        val e = map(403, body("domainPolicy", "Blocked by admin")) as AppError.Http
        assertEquals(403, e.code)
        assertEquals("domainPolicy", e.reason)
        assertEquals("Blocked by admin", e.message)
        assertTrue(!e.isTransient)
    }

    @Test
    fun reason_fallsBackToStatusWhenErrorsListIsMissing() {
        val e = map(403, body(null, "denied", status = "PERMISSION_DENIED")) as AppError.Http
        assertEquals("PERMISSION_DENIED", e.reason)
    }

    // ---------------------------------------------------------------- 404 / 412 / 429 / 5xx

    @Test
    fun notFound_isNotFound() {
        val e = map(404, body("notFound", "File not found: abc", code = 404))
        assertTrue(e is AppError.NotFound)
        assertEquals("File not found: abc", e.message)
    }

    @Test
    fun preconditionFailed_isHttp412() {
        val e = map(412, body("conditionNotMet", code = 412)) as AppError.Http
        assertEquals(412, e.code)
        assertEquals("conditionNotMet", e.reason)
    }

    @Test
    fun tooManyRequests_isRateLimitedWithRetryAfter() {
        val e = map(429, body("rateLimitExceeded", code = 429), "Retry-After" to "7") as AppError.RateLimited
        assertEquals(7000L, e.retryAfterMs)
        // 429 is rate limiting even without a parseable body
        assertTrue(map(429, "<html>slow down</html>") is AppError.RateLimited)
    }

    @Test
    fun retryAfter_thatIsNotSeconds_isIgnored() {
        val e = map(429, null, "Retry-After" to "Wed, 21 Oct 2026 07:28:00 GMT") as AppError.RateLimited
        assertEquals(0L, e.retryAfterMs)
    }

    @Test
    fun serverErrors_areTransientHttp() {
        for (code in listOf(500, 502, 503, 504)) {
            val e = map(code, body("backendError", "Backend Error", code = code)) as AppError.Http
            assertEquals(code, e.code)
            assertEquals("backendError", e.reason)
            assertTrue(e.message!!.startsWith("Google Drive server error:"))
            assertTrue(e.isTransient)
        }
    }

    @Test
    fun otherClientErrors_areNonTransientHttp() {
        val e = map(400, body("invalid", "Invalid Value", code = 400)) as AppError.Http
        assertEquals(400, e.code)
        assertEquals("invalid", e.reason)
        assertEquals("Invalid Value", e.message)
        assertTrue(!e.isTransient)
    }

    // ---------------------------------------------------------------- body parsing

    @Test
    fun malformedBody_fallsBackToStatusMessage() {
        val e = map(500, "<html>Bad Gateway</html>", message = "Internal Server Error") as AppError.Http
        assertNull(e.reason)
        assertEquals("Google Drive server error: Internal Server Error", e.message)
    }

    @Test
    fun missingBodyAndBlankStatusMessage_usesHttpCode() {
        val e = map(418, null, message = "") as AppError.Http
        assertEquals("HTTP 418", e.message)
        assertEquals("Google Drive returned HTTP 418.", e.userHint)
    }

    @Test
    fun unknownJsonFields_areIgnored() {
        val text = """{"error":{"code":404,"message":"nope","details":[{"x":1}],"errors":[{"reason":"notFound","extra":true}]},"other":1}"""
        val e = map(404, text)
        assertTrue(e is AppError.NotFound)
        assertEquals("nope", e.message)
    }
}
