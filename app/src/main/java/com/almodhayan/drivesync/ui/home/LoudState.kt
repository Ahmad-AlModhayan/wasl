package com.almodhayan.drivesync.ui.home

import android.content.Context
import androidx.compose.runtime.Immutable
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.db.AccountEntity
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.sync.PlanSummary
import com.almodhayan.drivesync.sync.StatusText
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.TopDestination
import com.almodhayan.drivesync.ui.onboarding.DeviceChecks
import com.almodhayan.drivesync.ui.status.PairStatusUi
import com.almodhayan.drivesync.ui.theme.SyncStatus

/**
 * Where a loud state's one action goes. Plain data, so view models decide it without a Context and
 * the screen runs it with [perform]. Shared by Home (banner and card action) and the pair screen.
 */
sealed interface LoudTarget {
    data class ReviewDeletions(val pairId: String) : LoudTarget
    data class Preview(val pairId: String) : LoudTarget
    data class OpenConflict(val conflictId: String) : LoudTarget
    data object Problems : LoudTarget
    data object SignIn : LoudTarget
    data object GrantAccess : LoudTarget
    data object FreeSpace : LoudTarget
    data class ChooseFolder(val pairId: String) : LoudTarget
    data object UnlockVault : LoudTarget
    data class Retry(val pairId: String) : LoudTarget
}

/** One loud state, in the shared words of [StatusText]: what happened, the facts, and the one fix. */
@Immutable
data class LoudSpec(
    val status: SyncStatus,
    val title: String,
    val body: String?,
    val actionLabel: String,
    val target: LoudTarget,
)

object LoudStates {
    const val DRIVE_STORAGE_URL = "https://drive.google.com/settings/storage"

    /** The loud state of one pair (approval, failed or conflict), or null when the pair is quiet. */
    fun forPair(pair: FolderPairEntity, st: PairStatusUi, blocked: PlanSummary?, openConflicts: Int, firstConflictId: String?): LoudSpec? =
        when (st.status) {
            SyncStatus.Approval -> LoudSpec(
                status = SyncStatus.Approval,
                title = StatusText.approvalTitle(blocked),
                body = StatusText.approvalBody(pair.name, blocked, pair.pendingApprovalSummary),
                actionLabel = StatusText.approvalAction(blocked),
                target = if (blocked?.blockedKind == "deletions" || blocked?.blockedKind == "wipe") LoudTarget.ReviewDeletions(pair.id) else LoudTarget.Preview(pair.id),
            )
            SyncStatus.Failed -> {
                val fix = StatusText.fixFor(st.errorKind)
                LoudSpec(
                    status = SyncStatus.Failed,
                    title = StatusText.failedTitle(pair.name, st.errorKind),
                    body = st.detail,
                    actionLabel = StatusText.fixLabel(fix),
                    target = fixTarget(fix, pair.id),
                )
            }
            SyncStatus.Conflict -> {
                val n = openConflicts.coerceAtLeast(1)
                LoudSpec(
                    status = SyncStatus.Conflict,
                    title = StatusText.conflictTitle(n, pair.name),
                    body = StatusText.CONFLICT_BODY,
                    actionLabel = StatusText.conflictAction(n),
                    target = if (n == 1 && firstConflictId != null) LoudTarget.OpenConflict(firstConflictId) else LoudTarget.Problems,
                )
            }
            else -> null
        }

    fun fixTarget(fix: StatusText.Fix, pairId: String): LoudTarget = when (fix) {
        StatusText.Fix.SIGN_IN -> LoudTarget.SignIn
        StatusText.Fix.GRANT_ACCESS -> LoudTarget.GrantAccess
        StatusText.Fix.FREE_SPACE -> LoudTarget.FreeSpace
        StatusText.Fix.CHOOSE_FOLDER -> LoudTarget.ChooseFolder(pairId)
        StatusText.Fix.UNLOCK_VAULT -> LoudTarget.UnlockVault
        StatusText.Fix.RETRY -> LoudTarget.Retry(pairId)
    }

    /** Health check: without All files access no pair can reach its phone folder. */
    fun filesAccessMissing(): LoudSpec = LoudSpec(
        status = SyncStatus.Failed,
        title = Strings.get(R.string.home_files_access_title),
        body = Strings.get(R.string.home_files_access_body),
        actionLabel = Strings.get(R.string.home_files_access_action),
        target = LoudTarget.GrantAccess,
    )

    /** Health check: Google asked for a new sign-in on [account]. */
    fun signInNeeded(account: AccountEntity, pairCount: Int, onlyAccount: Boolean): LoudSpec = LoudSpec(
        status = SyncStatus.Failed,
        title = if (onlyAccount) Strings.get(R.string.home_sign_in_title_only) else Strings.get(R.string.home_sign_in_title_account, account.id),
        body = if (pairCount == 0) {
            Strings.get(R.string.home_sign_in_body_no_pairs, account.id)
        } else {
            Strings.plural(R.plurals.home_sign_in_body_pairs, pairCount, Fmt.count(pairCount), account.id)
        },
        actionLabel = StatusText.fixLabel(StatusText.Fix.SIGN_IN),
        target = LoudTarget.SignIn,
    )

    /** Needs approval → Failed → Conflict; the first one in list order wins a tie. */
    fun mostUrgent(specs: List<LoudSpec>): LoudSpec? = specs.minByOrNull { it.status.urgency }
}

/**
 * Runs a loud state's action. [retry] starts a sync of one pair (PairActions.syncNow).
 * Returns false when nothing on the phone could open the page (no browser, no settings screen).
 */
fun LoudTarget.perform(nav: AppNav, context: Context, retry: (String) -> Unit): Boolean {
    when (this) {
        is LoudTarget.ReviewDeletions -> nav.reviewDeletions(pairId)
        is LoudTarget.Preview -> nav.dryRun(pairId)
        is LoudTarget.OpenConflict -> nav.conflict(conflictId)
        LoudTarget.Problems -> nav.top(TopDestination.Problems)
        LoudTarget.SignIn -> nav.accounts()
        LoudTarget.GrantAccess -> return DeviceChecks.openAllFilesAccess(context)
        LoudTarget.FreeSpace -> return DeviceChecks.openUrl(context, LoudStates.DRIVE_STORAGE_URL)
        is LoudTarget.ChooseFolder -> nav.pairEdit(pairId)
        LoudTarget.UnlockVault -> nav.vaults()
        is LoudTarget.Retry -> retry(pairId)
    }
    return true
}
