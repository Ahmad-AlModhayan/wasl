package com.almodhayan.drivesync.drive

import com.almodhayan.drivesync.core.AppError
import org.junit.Assert.assertTrue
import org.junit.Test

/** A passing Play services problem while fetching a token is a network error, not a sign-in problem. */
class DriveAuthErrorsTest {
    @Test
    fun networkAndTimeoutStatuses_areRetriedAsNetworkErrors() {
        listOf(7, 8, 15, 17).forEach { code ->
            assertTrue("status $code", DriveAuth.errorFor(code, "x") is AppError.Network)
        }
    }

    @Test
    fun cancelledOrMisconfiguredSignIn_staysASignInProblem() {
        assertTrue(DriveAuth.errorFor(16, "x") is AppError.Auth)
        assertTrue(DriveAuth.errorFor(10, "x") is AppError.Auth)
        assertTrue(DriveAuth.errorFor(4, "x") is AppError.Auth)
    }
}
