package com.almodhayan.drivesync.drive

import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Strings
import kotlinx.serialization.json.Json
import okhttp3.Response

object DriveErrors {
    private val json = Json { ignoreUnknownKeys = true; isLenient = true }

    fun fromResponse(response: Response, bodyText: String?): AppError {
        val code = response.code
        val parsed = bodyText?.let { runCatching { json.decodeFromString(ApiErrorEnvelope.serializer(), it) }.getOrNull() }?.error
        val reason = parsed?.errors?.firstOrNull()?.reason ?: parsed?.status
        val message = parsed?.message ?: response.message.ifBlank { "HTTP $code" }
        val retryAfter = response.header("Retry-After")?.toLongOrNull()?.times(1000)
        return when {
            code == 401 -> AppError.Auth(Strings.get(R.string.drive_error_token_rejected, message))
            code == 403 && reason in setOf("userRateLimitExceeded", "rateLimitExceeded", "dailyLimitExceeded", "sharingRateLimitExceeded") ->
                AppError.RateLimited(retryAfter ?: 0L, message)
            code == 429 -> AppError.RateLimited(retryAfter ?: 0L, message)
            code == 403 && (reason == "storageQuotaExceeded" || reason == "quotaExceeded" || message.contains("storage quota", true)) ->
                AppError.StorageQuota(message)
            code == 403 && reason == "insufficientPermissions" -> AppError.Auth(Strings.get(R.string.drive_error_missing_permission, message))
            code == 403 -> AppError.Http(code, reason, message)
            code == 404 -> AppError.NotFound(message)
            code == 412 -> AppError.Http(code, reason, message)
            code >= 500 -> AppError.Http(code, reason, Strings.get(R.string.drive_error_server, message))
            else -> AppError.Http(code, reason, message)
        }
    }
}
