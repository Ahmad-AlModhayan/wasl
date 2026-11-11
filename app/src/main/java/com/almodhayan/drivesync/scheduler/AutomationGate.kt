package com.almodhayan.drivesync.scheduler

import java.security.MessageDigest
import java.security.SecureRandom

/**
 * Who may trigger what through the exported [SyncIntentReceiver]. Any app on the phone can send it
 * a broadcast, so automation commands need the user's "Allow automation" setting plus the per-install
 * secret they copied into Tasker. Boot broadcasts stay open: only the system sends BOOT_COMPLETED and
 * MY_PACKAGE_REPLACED, and QUICKBOOT_POWERON only re-arms the schedule, which is harmless.
 * Notification actions never come through here; they use the non-exported [InternalActionReceiver].
 */
object AutomationGate {
    const val EXTRA_SECRET = "secret"

    val SYSTEM_ACTIONS = setOf(
        "android.intent.action.BOOT_COMPLETED",
        "android.intent.action.MY_PACKAGE_REPLACED",
        "android.intent.action.QUICKBOOT_POWERON",
    )

    val AUTOMATION_ACTIONS = setOf(
        SyncIntentReceiver.ACTION_SYNC_ALL, SyncIntentReceiver.ACTION_SYNC_PAIR, SyncIntentReceiver.ACTION_PAUSE_ALL,
        SyncIntentReceiver.ACTION_RESUME_ALL, SyncIntentReceiver.ACTION_CANCEL, SyncIntentReceiver.ACTION_RESCHEDULE,
    )

    fun allows(action: String?, presentedSecret: String?, enabled: Boolean, secret: String): Boolean = when {
        action in SYSTEM_ACTIONS -> true
        action !in AUTOMATION_ACTIONS -> false
        !enabled || secret.isBlank() || presentedSecret.isNullOrEmpty() -> false
        // Constant-time, so response timing does not reveal how much of a guess was right.
        else -> MessageDigest.isEqual(presentedSecret.toByteArray(Charsets.UTF_8), secret.toByteArray(Charsets.UTF_8))
    }

    fun newSecret(): String = ByteArray(16).also { SecureRandom().nextBytes(it) }.joinToString("") { "%02x".format(it) }
}
