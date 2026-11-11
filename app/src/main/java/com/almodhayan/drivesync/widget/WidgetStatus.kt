package com.almodhayan.drivesync.widget

import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.db.RunStatus
import com.almodhayan.drivesync.sync.PairProgress
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter

/** What the home screen widget shows. Pure data, computed by [WidgetStatusBuilder]. */
data class WidgetStatus(val statusLine: String, val problems: Int, val syncing: Boolean) {
    val problemsLine: String? get() = if (problems > 0) Strings.plural(R.plurals.widget_problems, problems, problems.toString()) else null
}

object WidgetStatusBuilder {
    fun build(pairs: List<FolderPairEntity>, running: Map<String, PairProgress>, openConflicts: Int, now: Long = System.currentTimeMillis()): WidgetStatus {
        val problems = openConflicts + pairs.count { it.lastStatus == RunStatus.FAILED || it.lastStatus == RunStatus.PARTIAL || it.pendingApproval }
        val current = running.values.minByOrNull { it.startedAt }
        val line = when {
            current != null -> {
                val name = pairs.firstOrNull { it.id == current.pairId }?.name ?: Strings.get(R.string.widget_folder_pair)
                val pct = (current.fraction * 100).toInt().toString()
                if (running.size > 1) Strings.get(R.string.widget_syncing_pair_more, name, pct, (running.size - 1).toString())
                else Strings.get(R.string.widget_syncing_pair, name, pct)
            }
            pairs.isEmpty() -> Strings.get(R.string.widget_no_pairs)
            else -> pairs.mapNotNull { it.lastSuccessAt }.maxOrNull()?.let { Strings.get(R.string.widget_synced_at, formatTime(it, now)) } ?: Strings.get(R.string.common_not_synced_yet)
        }
        return WidgetStatus(line, problems, current != null)
    }

    /** Absolute time so the text never goes stale between widget refreshes: "14:32", "yesterday 09:10", "Sep 3, 18:00". */
    fun formatTime(ts: Long, now: Long, zone: ZoneId = ZoneId.systemDefault()): String {
        val t = Instant.ofEpochMilli(ts).atZone(zone)
        val today = Instant.ofEpochMilli(now).atZone(zone).toLocalDate()
        val pattern = when {
            t.toLocalDate() == today -> "HH:mm"
            t.toLocalDate() == today.minusDays(1) -> return Strings.get(R.string.widget_yesterday_at, DateTimeFormatter.ofPattern("HH:mm").format(t))
            t.year == today.year -> "MMM d, HH:mm"
            else -> "yyyy-MM-dd HH:mm"
        }
        return DateTimeFormatter.ofPattern(pattern).format(t)
    }
}
