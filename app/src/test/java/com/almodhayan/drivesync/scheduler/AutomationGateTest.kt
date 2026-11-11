package com.almodhayan.drivesync.scheduler

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** Who may trigger what through the exported [SyncIntentReceiver] (REQ-0005). */
class AutomationGateTest {
    private val secret = "0123456789abcdef0123456789abcdef"

    private fun allows(action: String?, presented: String?, enabled: Boolean = true, configured: String = secret) =
        AutomationGate.allows(action, presented, enabled, configured)

    @Test
    fun systemBootBroadcasts_needNeitherTheSettingNorTheSecret() {
        for (a in listOf("android.intent.action.BOOT_COMPLETED", "android.intent.action.MY_PACKAGE_REPLACED", "android.intent.action.QUICKBOOT_POWERON")) {
            assertTrue(a, allows(a, presented = null, enabled = false, configured = ""))
        }
    }

    @Test
    fun automationIsRefusedWhileTheSettingIsOff_evenWithTheRightSecret() {
        assertFalse(allows(SyncIntentReceiver.ACTION_PAUSE_ALL, secret, enabled = false))
    }

    @Test
    fun automationIsRefusedWithoutASecret() {
        assertFalse(allows(SyncIntentReceiver.ACTION_SYNC_ALL, presented = null))
        assertFalse(allows(SyncIntentReceiver.ACTION_SYNC_ALL, presented = ""))
    }

    @Test
    fun automationIsRefusedWithTheWrongSecret() {
        assertFalse(allows(SyncIntentReceiver.ACTION_SYNC_PAIR, "0123456789abcdef0123456789abcdee"))
        assertFalse(allows(SyncIntentReceiver.ACTION_SYNC_PAIR, secret.substring(0, 16)))
    }

    @Test
    fun automationIsRefusedWhenNoSecretWasEverCreated() {
        assertFalse(allows(SyncIntentReceiver.ACTION_SYNC_ALL, presented = "", configured = ""))
    }

    @Test
    fun everyDocumentedCommandIsAllowedWithTheRightSecret() {
        for (a in listOf(
            SyncIntentReceiver.ACTION_SYNC_ALL, SyncIntentReceiver.ACTION_SYNC_PAIR, SyncIntentReceiver.ACTION_PAUSE_ALL,
            SyncIntentReceiver.ACTION_RESUME_ALL, SyncIntentReceiver.ACTION_CANCEL, SyncIntentReceiver.ACTION_RESCHEDULE,
        )) assertTrue(a, allows(a, secret))
    }

    @Test
    fun notificationOnlyActionsAreNeverAcceptedFromOutside() {
        assertFalse(allows(InternalActionReceiver.ACTION_PAUSE_RUNNING, secret))
        assertFalse(allows(InternalActionReceiver.ACTION_RESTORE_DELETIONS, secret))
        assertFalse(allows("com.almodhayan.drivesync.SOMETHING_ELSE", secret))
        assertFalse(allows(null, secret))
    }

    @Test
    fun newSecrets_are128BitHexAndUnique() {
        val a = AutomationGate.newSecret()
        val b = AutomationGate.newSecret()
        assertEquals(32, a.length)
        assertTrue(a, a.all { it in '0'..'9' || it in 'a'..'f' })
        assertNotEquals(a, b)
    }
}
