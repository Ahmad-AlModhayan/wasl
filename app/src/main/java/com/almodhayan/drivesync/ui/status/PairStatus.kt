package com.almodhayan.drivesync.ui.status

import androidx.compose.runtime.Immutable
import androidx.compose.ui.graphics.vector.ImageVector
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.actions.PairActions
import com.almodhayan.drivesync.core.ErrorKind
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.db.RunStatus
import com.almodhayan.drivesync.sync.PairProgress
import com.almodhayan.drivesync.sync.Phase
import com.almodhayan.drivesync.sync.PlanSummary
import com.almodhayan.drivesync.sync.StatusText
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.SyncProgressModel
import com.almodhayan.drivesync.ui.components.TransferDirection
import com.almodhayan.drivesync.ui.components.PairDirection
import com.almodhayan.drivesync.ui.theme.Sym
import com.almodhayan.drivesync.ui.theme.SyncStatus

/** One pair's state as every screen shows it. */
@Immutable
data class PairStatusUi(
    val status: SyncStatus,
    val label: String,                 // the chip word: "Synced", "Syncing 42%", "Waiting for Wi‑Fi", "Needs approval", "2 conflicts", "Failed"
    val icon: ImageVector?,            // override for waiting (bolt, schedule, pause)
    val lastSync: String?,             // "4 min ago", "Synced yesterday"
    val detail: String?,               // one plain sentence: the reason, the error hint
    val retrying: Boolean,             // a transient failure retrying on its own (never badged)
    val errorKind: ErrorKind?,
    val progress: SyncProgressModel?,
) {
    val loud: Boolean get() = status.loud
}

/**
 * Resolves the six states for a pair with the design's precedence:
 * Needs approval → Failed → Conflict → Syncing → Waiting → Synced.
 */
object PairStatusResolver {
    /**
     * [liveBlock] is what holds the pair back right now (offline, not on Wi‑Fi…), from
     * `Conditions.blockReason`. Without it an offline phone runs nothing, records nothing, and kept
     * saying "Synced" while files waited (E2E 2026-10).
     */
    fun resolve(pair: FolderPairEntity, progress: PairProgress?, openConflicts: Int, blocked: PlanSummary? = null, now: Long = System.currentTimeMillis(), liveBlock: String? = null): PairStatusUi {
        val kind = ErrorKind.parse(pair.lastErrorKind)
        val lastSync = pair.lastSuccessAt?.let { Fmt.relative(it, now) }
        val failedRun = pair.lastStatus == RunStatus.FAILED || pair.lastStatus == RunStatus.PARTIAL
        val transient = failedRun && kind != null && kind.transient && pair.consecutiveFailures < StatusText.FAILED_AFTER
        // The engine pauses with "Your Google Drive is full"; older builds said "storage".
        val quotaPaused = pair.paused && pair.pauseReason != null &&
            (pair.pauseReason.contains("is full", ignoreCase = true) || pair.pauseReason.contains("storage", ignoreCase = true))
        return when {
            pair.pendingApproval -> PairStatusUi(SyncStatus.Approval, SyncStatus.Approval.label, null, syncedText(pair, now),
                StatusText.approvalBody(pair.name, blocked, pair.pendingApprovalSummary), false, kind, null)
            (failedRun && !transient) || quotaPaused -> {
                val k = if (quotaPaused) ErrorKind.QUOTA else kind
                PairStatusUi(SyncStatus.Failed, SyncStatus.Failed.label, null, pair.lastSuccessAt?.let { Strings.get(R.string.status_last_synced, Fmt.relativeLower(it, now)) },
                    // For these kinds the engine's own message says what happened and what to do (a
                    // vault refusal, a damaged download); the generic line would hide it.
                    pair.lastError.takeIf { k == ErrorKind.INTEGRITY || k == ErrorKind.IO || k == ErrorKind.OTHER }
                        ?: StatusText.failedWhy(k) ?: pair.lastError ?: pair.pauseReason, false, k, null)
            }
            openConflicts > 0 -> PairStatusUi(SyncStatus.Conflict, Strings.plural(R.plurals.status_conflicts, openConflicts), null, lastSync,
                StatusText.CONFLICT_BODY, false, null, progress?.let { model(it, pair) })
            progress != null -> {
                val m = model(progress, pair)
                PairStatusUi(SyncStatus.Syncing, if (m.scanning) Strings.get(R.string.status_comparing_folders) else Strings.get(R.string.status_syncing_percent, Fmt.percent(m.value ?: 0f)), null, null, null, false, null, m)
            }
            pair.holdForPreview -> PairStatusUi(SyncStatus.Waiting, Strings.get(R.string.status_waiting_for_preview), Sym.Visibility, null,
                Strings.get(R.string.status_waiting_for_preview_detail), false, null, null)
            !pair.enabled -> PairStatusUi(SyncStatus.Waiting, Strings.get(R.string.status_turned_off), Sym.Pause, syncedText(pair, now),
                Strings.get(R.string.status_turned_off_detail), false, null, null)
            pair.paused -> PairStatusUi(SyncStatus.Waiting, Strings.get(R.string.common_paused), Sym.Pause, syncedText(pair, now),
                pair.pauseReason?.takeIf { it != PairActions.PAUSED_BY_YOU }, false, null, null)
            transient -> PairStatusUi(SyncStatus.Waiting, Strings.get(R.string.status_retrying_soon), Sym.Schedule, syncedText(pair, now), pair.lastError, true, kind, null)
            liveBlock != null -> {
                val (label, icon) = StatusText.waiting(liveBlock)
                PairStatusUi(SyncStatus.Waiting, label, Sym.named(icon), syncedText(pair, now), liveBlock, false, null, null)
            }
            pair.lastStatus == RunStatus.SKIPPED && pair.lastError != null -> {
                val (label, icon) = StatusText.waiting(pair.lastError)
                PairStatusUi(SyncStatus.Waiting, label, Sym.named(icon), syncedText(pair, now), pair.lastError, false, null, null)
            }
            pair.lastSuccessAt == null -> PairStatusUi(SyncStatus.Waiting, Strings.get(R.string.common_not_synced_yet), Sym.Schedule, null, null, false, null, null)
            else -> PairStatusUi(SyncStatus.Synced, SyncStatus.Synced.label, null, lastSync, null, false, null, null)
        }
    }

    fun model(p: PairProgress, pair: FolderPairEntity): SyncProgressModel {
        val scanning = p.phase == Phase.PREPARING || p.phase == Phase.LISTING_REMOTE || p.phase == Phase.SCANNING_LOCAL || p.phase == Phase.PLANNING
        val elapsed = System.currentTimeMillis() - p.startedAt
        val f = p.fraction
        val eta = if (!scanning && f > 0.02f && elapsed > 5_000) Fmt.eta((elapsed / f * (1 - f)).toLong()) else null
        val dir = when (PairDirection.of(pair.mode)) { PairDirection.PhoneToDrive -> TransferDirection.Up; PairDirection.DriveToPhone -> TransferDirection.Down; else -> TransferDirection.Both }
        return SyncProgressModel(
            scanning = scanning, value = if (scanning) null else f, done = p.doneActions, total = p.totalActions,
            doneBytes = p.bytesDone, totalBytes = p.bytesTotal, direction = dir, current = p.currentFile?.substringAfterLast('/'), eta = eta,
        )
    }

    /** "Wi‑Fi only · every 6 hours" for the card subtitle. */
    fun conditions(pair: FolderPairEntity, globalIntervalMinutes: Int): String = buildList {
        if (pair.wifiOnly) add(Strings.get(R.string.status_wifi_only))
        if (pair.requireCharging) add(Strings.get(R.string.status_while_charging))
        val interval = if (pair.intervalMinutes > 0) pair.intervalMinutes else globalIntervalMinutes
        if (pair.instantSync) add(Strings.get(R.string.status_when_files_change)) else add(Fmt.interval(interval))
    }.joinToString(" · ")

    /** Problems badge: approvals + conflicts + failures (never transient retries). */
    fun problemCount(pairs: List<FolderPairEntity>, conflictsByPair: Map<String, Int>): Int =
        pairs.count { p -> resolve(p, null, 0).status.let { it == SyncStatus.Approval || it == SyncStatus.Failed } } + conflictsByPair.values.sum()

    private fun syncedText(pair: FolderPairEntity, now: Long): String? = pair.lastSuccessAt?.let { Strings.get(R.string.status_synced_when, Fmt.relativeLower(it, now)) }
}
