package com.almodhayan.drivesync.sync

import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.ErrorKind
import com.almodhayan.drivesync.core.Strings

/**
 * The words for states, shared by screens and notifications (docs/design/project/2-status-system.md):
 * titles say what happened to the files, bodies reassure with facts, buttons are verb-first with counts.
 * Never "error" in a title; never blame the person.
 */
object StatusText {
    // ---- needs approval ----
    fun approvalTitle(s: PlanSummary?): String =
        Strings.get(if (s?.blockedKind == "first-merge") R.string.sync_approval_title_first_merge else R.string.sync_approval_title)

    fun approvalBody(pairName: String, s: PlanSummary?, fallback: String? = null): String = when (s?.blockedKind) {
        "deletions" -> if (s.blockedSide == "drive") Strings.plural(R.plurals.sync_approval_body_deleted_on_phone, s.blockedCount, s.blockedCount, pairName)
        else Strings.plural(R.plurals.sync_approval_body_deleted_on_drive, s.blockedCount, s.blockedCount, pairName)
        "wipe" -> if (s.blockedSide == "drive") Strings.get(R.string.sync_approval_body_wiped_on_phone, pairName)
        else Strings.get(R.string.sync_approval_body_wiped_on_drive, pairName)
        "first-merge" -> Strings.get(R.string.sync_approval_body_first_merge, pairName, s.localFiles, s.remoteFiles)
        else -> fallback ?: Strings.get(R.string.sync_approval_body_fallback)
    }

    fun approvalAction(s: PlanSummary?): String = when (s?.blockedKind) {
        "deletions", "wipe" -> Strings.plural(R.plurals.sync_approval_action_review_deletions, s.blockedCount)
        "first-merge" -> Strings.get(R.string.sync_approval_action_review_first_sync)
        else -> Strings.get(R.string.sync_action_review)
    }

    /** The safe alternative for a deletion stop: "Restore on phone" when the phone lost files, else "Restore on Drive". */
    fun restoreAction(s: PlanSummary?): String? = when (s?.blockedKind) {
        "deletions", "wipe" -> Strings.get(if (s.blockedSide == "drive") R.string.sync_action_restore_on_phone else R.string.sync_action_restore_on_drive)
        else -> null
    }

    fun confirmDeleteAction(s: PlanSummary?): String? = when (s?.blockedKind) {
        "deletions", "wipe" -> Strings.get(if (s.blockedSide == "drive") R.string.sync_action_delete_on_drive else R.string.sync_action_delete_on_phone)
        else -> null
    }

    // ---- conflicts ----
    fun conflictTitle(n: Int, pairName: String): String = Strings.plural(R.plurals.sync_conflict_title, n.coerceAtLeast(1), n, pairName)
    val CONFLICT_BODY: String get() = Strings.get(R.string.sync_conflict_body)
    fun conflictAction(n: Int): String = Strings.plural(R.plurals.sync_conflict_action, n.coerceAtLeast(1), n)

    // ---- failed ----
    fun failedTitle(pairName: String, kind: ErrorKind?): String = when (kind) {
        ErrorKind.AUTH -> Strings.get(R.string.sync_failed_title_auth, pairName)
        ErrorKind.PERMISSION, ErrorKind.IO -> Strings.get(R.string.sync_failed_title_unreachable, pairName)
        ErrorKind.QUOTA -> Strings.get(R.string.sync_failed_title_quota)
        ErrorKind.NOT_FOUND -> Strings.get(R.string.sync_failed_title_not_found, pairName)
        ErrorKind.LOCKED -> Strings.get(R.string.sync_failed_title_locked, pairName)
        ErrorKind.NETWORK, ErrorKind.RATE_LIMIT, ErrorKind.SERVER -> Strings.get(R.string.sync_failed_title_keeps_stopping, pairName)
        else -> Strings.get(R.string.sync_failed_title_other, pairName)
    }

    /** Why it failed, in plain words (the raw error stays in Diagnostics). */
    fun failedWhy(kind: ErrorKind?): String? = when (kind) {
        ErrorKind.AUTH -> Strings.get(R.string.sync_failed_why_auth)
        ErrorKind.PERMISSION -> Strings.get(R.string.sync_failed_why_permission)
        ErrorKind.QUOTA -> Strings.get(R.string.sync_failed_why_quota)
        ErrorKind.NOT_FOUND -> Strings.get(R.string.sync_failed_why_not_found)
        ErrorKind.LOCKED -> Strings.get(R.string.sync_failed_why_locked)
        ErrorKind.IO -> Strings.get(R.string.sync_failed_why_io)
        ErrorKind.INTEGRITY -> Strings.get(R.string.sync_failed_why_integrity)
        ErrorKind.NETWORK -> Strings.get(R.string.sync_failed_why_network)
        ErrorKind.RATE_LIMIT -> Strings.get(R.string.sync_failed_why_rate_limit)
        ErrorKind.SERVER -> Strings.get(R.string.sync_failed_why_server)
        else -> null
    }

    enum class Fix { SIGN_IN, GRANT_ACCESS, FREE_SPACE, CHOOSE_FOLDER, UNLOCK_VAULT, RETRY }

    fun fixFor(kind: ErrorKind?): Fix = when (kind) {
        ErrorKind.AUTH -> Fix.SIGN_IN
        ErrorKind.PERMISSION -> Fix.GRANT_ACCESS
        ErrorKind.QUOTA -> Fix.FREE_SPACE
        ErrorKind.NOT_FOUND -> Fix.CHOOSE_FOLDER
        ErrorKind.LOCKED -> Fix.UNLOCK_VAULT
        else -> Fix.RETRY
    }

    fun fixLabel(fix: Fix): String = Strings.get(when (fix) {
        Fix.SIGN_IN -> R.string.sync_fix_sign_in
        Fix.GRANT_ACCESS -> R.string.sync_fix_grant_access
        Fix.FREE_SPACE -> R.string.sync_fix_free_space
        Fix.CHOOSE_FOLDER -> R.string.sync_fix_choose_folder
        Fix.UNLOCK_VAULT -> R.string.sync_fix_unlock_vault
        Fix.RETRY -> R.string.sync_fix_retry
    })

    /** Transient failures retry on their own; after this many in a row they count as Failed. */
    const val FAILED_AFTER = 3

    // ---- waiting ----
    /**
     * Label and Material Symbols icon name for a condition the person set. [reason] is the stored
     * blocker text: in the app's language (see Blocker.text) or, from older versions, in English.
     */
    fun waiting(reason: String?): Pair<String, String> {
        val r = reason.orEmpty()
        val label = when (Blockers.kindOf(r)) {
            Blockers.Kind.CHARGER -> R.string.sync_waiting_charger to "bolt"
            Blockers.Kind.BATTERY -> R.string.sync_waiting_battery to "bolt"
            Blockers.Kind.VPN -> R.string.sync_waiting_vpn to "shield"
            Blockers.Kind.ROAMING -> R.string.sync_waiting_roaming to "cell_tower"
            Blockers.Kind.OFFLINE -> R.string.sync_waiting_connection to "wifi"
            Blockers.Kind.SSID_NOT_ALLOWED, Blockers.Kind.SSID_DENIED -> R.string.sync_waiting_your_wifi to "wifi"
            Blockers.Kind.METERED, Blockers.Kind.WIFI, null -> R.string.sync_waiting_wifi to "wifi"
        }
        return Strings.get(label.first) to label.second
    }
}
