package com.almodhayan.drivesync.core

import android.app.PendingIntent
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.crypto.detailText
import com.kahflabs.wasl.ffi.WaslException

/** Error taxonomy shared by the Drive client, the engine and the UI. */
sealed class AppError(message: String, cause: Throwable? = null) : Exception(message, cause) {
    /** Sign-in / consent is required. [resolution] can be launched by the UI. */
    class Auth(message: String, val resolution: PendingIntent? = null, cause: Throwable? = null) : AppError(message, cause)
    class Network(message: String, cause: Throwable? = null) : AppError(message, cause)
    class RateLimited(val retryAfterMs: Long, message: String) : AppError(message)
    class StorageQuota(message: String) : AppError(message)
    class NotFound(message: String) : AppError(message)
    class Permission(message: String) : AppError(message)
    class Http(val code: Int, val reason: String?, message: String) : AppError(message)
    class Io(message: String, cause: Throwable? = null) : AppError(message, cause)
    class Integrity(message: String) : AppError(message)
    /** The pair's encryption vault has no stored keys on this device. */
    class Locked(message: String) : AppError(message)
    class Cancelled : AppError("Cancelled")
    class Unknown(message: String, cause: Throwable? = null) : AppError(message, cause)

    /** Whether a retry later (without user action) could succeed. */
    val isTransient: Boolean
        get() = when (this) {
            is Network, is RateLimited -> true
            is Http -> code >= 500
            else -> false
        }

    /** Short, user-facing explanation with the fix. */
    val userHint: String
        get() = when (this) {
            is Auth -> Strings.get(R.string.core_error_hint_auth)
            is Network -> Strings.get(R.string.core_error_hint_network)
            is RateLimited -> Strings.get(R.string.core_error_hint_rate_limited, retryAfterMs / 1000)
            is StorageQuota -> Strings.get(R.string.core_error_hint_quota)
            is NotFound -> Strings.get(R.string.core_error_hint_not_found)
            is Permission -> Strings.get(R.string.core_error_hint_permission)
            is Http -> reason?.let { Strings.get(R.string.core_error_hint_http_reason, code, it) } ?: Strings.get(R.string.core_error_hint_http, code)
            is Io -> Strings.get(R.string.core_error_hint_io, message.orEmpty())
            is Integrity -> Strings.get(R.string.core_error_hint_integrity, message.orEmpty())
            is Locked -> message?.takeIf { it.isNotBlank() } ?: Strings.get(R.string.core_error_hint_locked)
            is Cancelled -> Strings.get(R.string.core_error_hint_cancelled)
            is Unknown -> message?.takeIf { it.isNotBlank() } ?: Strings.get(R.string.core_error_hint_unknown)
        }

    companion object {
        fun wrap(t: Throwable): AppError = when (t) {
            is AppError -> t
            is kotlinx.coroutines.CancellationException -> Cancelled()
            is java.net.UnknownHostException, is java.net.SocketTimeoutException, is java.net.SocketException,
            is javax.net.ssl.SSLException, is java.io.InterruptedIOException -> Network(t.message ?: Strings.get(R.string.core_error_network_fallback), t)
            is java.io.IOException -> Io(t.message ?: Strings.get(R.string.core_error_io_fallback), t)
            // libwasl's sentence, without the generated "detail=" its message starts with (UX audit 2026-10).
            is WaslException -> Unknown(t.detailText(), t)
            else -> Unknown(t.message ?: t::class.java.simpleName, t)
        }
    }
}
