package com.almodhayan.drivesync.scheduler

import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.db.RunStatus
import com.almodhayan.drivesync.sync.PairProgress
import com.almodhayan.drivesync.sync.Phase
import com.almodhayan.drivesync.ui.components.Fmt

data class StatusInput(
    val now: Long,
    val pairs: List<FolderPairEntity>,
    val running: Map<String, PairProgress>,
    /** When each running pair last made progress; older than [StatusLine.STALL_AFTER_MS] reads as stalled. */
    val lastProgressAt: Map<String, Long>,
    val instantMonitoring: Boolean,
    val globalIntervalMinutes: Int,
    /** Pair id → what holds it back right now (offline, not on Wi‑Fi…), for pairs that may run. */
    val liveBlocks: Map<String, String> = emptyMap(),
)

data class Status(val title: String, val text: String, val fraction: Float? = null)

/** The always-on status notification: what Wasl is doing, in one short title and one line. */
object StatusLine {
    const val STALL_AFTER_MS = 10 * 60_000L

    fun of(i: StatusInput): Status {
        val names = i.pairs.associate { it.id to it.name }
        val running = i.running.values.sortedBy { it.startedAt }
        if (running.isNotEmpty()) return syncing(i, running, names)

        val active = i.pairs.filter { it.enabled }
        if (active.isNotEmpty() && active.all { it.paused }) return Status(Strings.get(R.string.common_paused), Strings.get(R.string.notif_status_all_paused))
        val unpaused = active.filter { !it.paused }
        unpaused.firstOrNull { it.pendingApproval || it.lastStatus == RunStatus.FAILED || it.lastStatus == RunStatus.BLOCKED }?.let {
            return Status(Strings.get(R.string.notif_status_needs_attention), Strings.get(R.string.notif_status_pair_reason, it.name, it.lastError ?: Strings.get(R.string.sync_approval_title)))
        }
        // Held right now by its conditions: no run happens offline, so no run recorded why.
        unpaused.firstOrNull { i.liveBlocks[it.id] != null }?.let {
            return Status(Strings.get(R.string.notif_status_waiting), Strings.get(R.string.notif_status_pair_reason, it.name, i.liveBlocks.getValue(it.id)))
        }
        // Held by its rules, or stopped part way (by Android or by hand): there is work left, so say why.
        unpaused.firstOrNull { (it.lastStatus == RunStatus.SKIPPED || it.lastStatus == RunStatus.CANCELLED) && !it.lastError.isNullOrBlank() }?.let {
            return Status(Strings.get(R.string.notif_status_waiting), Strings.get(R.string.notif_status_pair_reason, it.name, it.lastError.orEmpty()))
        }
        return Status(Strings.get(if (i.instantMonitoring) R.string.notif_status_waiting_for_changes else R.string.notif_up_to_date), nextCheck(i, unpaused))
    }

    private fun syncing(i: StatusInput, running: List<PairProgress>, names: Map<String, String>): Status {
        val first = running.first()
        val name = names[first.pairId] ?: Strings.get(R.string.notif_status_a_pair)
        running.firstOrNull { p -> (i.lastProgressAt[p.pairId] ?: p.startedAt) <= i.now - STALL_AFTER_MS }?.let { p ->
            val mins = (i.now - (i.lastProgressAt[p.pairId] ?: p.startedAt)) / 60_000
            return Status(Strings.get(R.string.notif_status_stalled_title, names[p.pairId] ?: name), Strings.plural(R.plurals.notif_status_stalled_text, mins.toInt()))
        }
        if (running.size > 1) return Status(Strings.plural(R.plurals.notif_status_syncing_more, running.size - 1, name, running.size - 1), Strings.plural(R.plurals.notif_status_pairs_at_once, running.size))
        val transferring = first.phase == Phase.TRANSFERRING || first.phase == Phase.DELETING
        if (!transferring) return Status(Strings.get(R.string.notif_status_syncing_pair, name), Strings.get(R.string.notif_comparing_folders))
        val pct = (first.fraction * 100).toInt()
        val left = Fmt.eta(etaMs(first, i.now)) ?: Strings.get(R.string.notif_status_done_of_total, Fmt.count(first.doneActions), Fmt.files(first.totalActions))
        return Status(Strings.get(R.string.notif_status_syncing_pair_percent, name, pct), left, first.fraction)
    }

    /** Earliest scheduled check over the active pairs: last run plus that pair's interval. */
    private fun nextCheck(i: StatusInput, pairs: List<FolderPairEntity>): String {
        val next = pairs.minOfOrNull { p ->
            val interval = (if (p.intervalMinutes > 0) p.intervalMinutes else i.globalIntervalMinutes) * 60_000L
            (p.lastRunAt ?: i.now) + interval
        } ?: return Strings.get(R.string.notif_status_no_pairs)
        return if (next <= i.now + 60_000) Strings.get(R.string.notif_status_next_check_soon) else Strings.get(R.string.notif_status_next_check_at, Fmt.clock(next))
    }

    private fun etaMs(p: PairProgress, now: Long): Long? {
        val elapsed = now - p.startedAt
        val f = p.fraction
        if (f <= 0.02f || elapsed < 5_000) return null
        return (elapsed / f * (1 - f)).toLong()
    }
}
