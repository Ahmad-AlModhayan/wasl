package com.almodhayan.drivesync.drive

import android.accounts.Account
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Strings
import com.google.android.gms.auth.api.identity.AuthorizationRequest
import com.google.android.gms.auth.api.identity.AuthorizationResult
import com.google.android.gms.auth.api.identity.Identity
import com.google.android.gms.auth.api.identity.RevokeAccessRequest
import com.google.android.gms.common.api.ApiException
import com.google.android.gms.common.api.Scope
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.tasks.await
import kotlinx.coroutines.withTimeoutOrNull
import java.util.concurrent.ConcurrentHashMap
import javax.inject.Inject
import javax.inject.Singleton

/**
 * Access tokens come from Google Play services' Authorization API. Play services owns the refresh
 * token, so the app never stores long-lived credentials and background workers can obtain fresh
 * access tokens silently once the user granted consent once.
 */
/** Abstraction over token acquisition so the API client can be tested without Play services. */
interface TokenProvider {
    suspend fun accessToken(email: String, forceRefresh: Boolean = false): String
    fun invalidate(email: String)
}

@Singleton
class DriveAuth @Inject constructor(@ApplicationContext private val context: Context) : TokenProvider {

    sealed class Outcome {
        data class Token(val accessToken: String) : Outcome()
        data class NeedsResolution(val pendingIntent: PendingIntent) : Outcome()
    }

    private data class Cached(val token: String, val expiresAt: Long)
    private val cache = ConcurrentHashMap<String, Cached>()
    private val locks = ConcurrentHashMap<String, Mutex>()

    private val scopes = listOf(Scope(DRIVE_SCOPE))

    /** Start authorization for [email] (or let Play services show its account picker when null). */
    suspend fun authorize(email: String?): Outcome {
        val req = AuthorizationRequest.builder()
            .setRequestedScopes(scopes)
            .apply { if (email != null) setAccount(Account(email, "com.google")) }
            .build()
        val result = try {
            // Play services can stall (frozen by battery management); a sync must not wait on it forever.
            withTimeoutOrNull(AUTH_TIMEOUT_MS) { Identity.getAuthorizationClient(context).authorize(req).await() }
                ?: throw AppError.Network(Strings.get(R.string.drive_auth_play_services_timeout))
        } catch (e: ApiException) {
            throw errorFor(e.statusCode, e.message, e)
        }
        return toOutcome(email, result)
    }

    /** Called by the UI with the activity result of the resolution intent. */
    fun resultFromIntent(email: String?, data: Intent?): Outcome {
        val result = try {
            Identity.getAuthorizationClient(context).getAuthorizationResultFromIntent(data)
        } catch (e: ApiException) {
            throw errorFor(e.statusCode, e.message, e)
        }
        return toOutcome(email, result)
    }

    private fun toOutcome(email: String?, result: AuthorizationResult): Outcome {
        if (result.hasResolution()) {
            return Outcome.NeedsResolution(result.pendingIntent!!)
        }
        val token = result.accessToken ?: throw AppError.Auth(Strings.get(R.string.drive_auth_no_token))
        if (email != null) cache[email] = Cached(token, System.currentTimeMillis() + TOKEN_TTL_MS)
        return Outcome.Token(token)
    }

    /** Remember a token obtained interactively (e.g. right after the account picker) under its email. */
    fun cache(email: String, token: String) { cache[email] = Cached(token, System.currentTimeMillis() + TOKEN_TTL_MS) }

    /** Silent token for background use. Throws [AppError.Auth] (with resolution) when consent is needed. */
    override suspend fun accessToken(email: String, forceRefresh: Boolean): String {
        val lock = locks.getOrPut(email) { Mutex() }
        return lock.withLock {
            val c = cache[email]
            if (!forceRefresh && c != null && c.expiresAt > System.currentTimeMillis()) return@withLock c.token
            if (forceRefresh) cache.remove(email)
            when (val o = authorize(email)) {
                is Outcome.Token -> o.accessToken
                is Outcome.NeedsResolution -> throw AppError.Auth(Strings.get(R.string.drive_auth_reauthorize_for, email), o.pendingIntent)
            }
        }
    }

    override fun invalidate(email: String) { cache.remove(email) }

    /**
     * Withdraws the Drive grant this app holds for [email] at Google, so a removed account leaves
     * no standing access behind (security review S2-4): Play services keeps the refresh token, and
     * without this it would hand out tokens again on the next request. Best effort: false when Play
     * services refused, failed or did not answer in time; the caller removes the account anyway.
     */
    suspend fun revoke(email: String): Boolean {
        cache.remove(email)
        val req = RevokeAccessRequest.builder()
            .setAccount(Account(email, "com.google"))
            .setScopes(scopes)
            .build()
        return try {
            withTimeoutOrNull(REVOKE_TIMEOUT_MS) { Identity.getAuthorizationClient(context).revokeAccess(req).await(); true } ?: false
        } catch (e: Exception) {
            // A Play services task that was cancelled also lands here (as a CancellationException);
            // only this coroutine's own cancellation stops the removal.
            currentCoroutineContext().ensureActive()
            false
        }
    }

    companion object {
        const val DRIVE_SCOPE = "https://www.googleapis.com/auth/drive"
        private const val TOKEN_TTL_MS = 45L * 60 * 1000
        private const val AUTH_TIMEOUT_MS = 60_000L
        /** Removing an account waits at most this long for Google before going ahead without it. */
        private const val REVOKE_TIMEOUT_MS = 15_000L

        /**
         * Play services status codes that mean "try again later" (no network, timeouts, Play services
         * busy or restarting). They must not read as a sign-in problem: that stops the whole run and
         * asks the user to sign in again for nothing.
         */
        private val TRANSIENT = setOf(7, 8, 14, 15, 17, 19, 20, 21, 22)

        fun errorFor(statusCode: Int, message: String?, cause: Throwable? = null): AppError {
            val text = when (statusCode) {
                10 -> "DEVELOPER_ERROR (10): the OAuth client for this app's package name and signing SHA-1 is missing in Google Cloud. See docs/SETUP.md."
                12501, 16 -> Strings.get(R.string.drive_auth_cancelled)
                7 -> Strings.get(R.string.drive_auth_network)
                else -> Strings.get(R.string.drive_auth_failed, statusCode, message.orEmpty())
            }
            return if (statusCode in TRANSIENT) AppError.Network(text, cause) else AppError.Auth(text, cause = cause)
        }
    }
}
