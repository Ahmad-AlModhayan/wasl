package com.almodhayan.drivesync.ui

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Routes another app may hand MainActivity: only what the app's own notifications and widget send (REQ-0012). */
class ExternalRoutesTest {
    private val id = "3f2b9c1e-7a4d-4e8b-9c0a-1d2e3f4a5b6c"

    @Test
    fun allowsTheRoutesTheAppItselfSends() {
        for (r in listOf("home", "problems", "pair:$id", "review:$id")) assertTrue(r, ExternalRoutes.allowed(r))
    }

    @Test
    fun refusesOtherNamedScreens() {
        for (r in listOf("settings", "accounts", "trash", "status", "history", "conflict:$id")) assertFalse(r, ExternalRoutes.allowed(r))
    }

    @Test
    fun refusesIdsThatWouldInjectAnotherRoute() {
        for (r in listOf("pair:x/edit", "review:$id/../edit", "pair:$id?accountId=evil", "pair:", "review:" + "a".repeat(65))) {
            assertFalse(r, ExternalRoutes.allowed(r))
        }
    }

    @Test
    fun refusesMissingRoutes() {
        assertFalse(ExternalRoutes.allowed(null))
        assertFalse(ExternalRoutes.allowed(""))
    }
}
