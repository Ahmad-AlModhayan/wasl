package com.almodhayan.drivesync.ui.components

import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.core.humanBytes
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale

/** Number and time formatting from the content rules: digit grouping (1,204), units with a space (2.4 MB). */
object Fmt {
    fun count(n: Int): String = String.format(Locale.US, "%,d", n)
    fun count(n: Long): String = String.format(Locale.US, "%,d", n)
    /** Isolated left-to-right, so Arabic text shows "225 GB", not "GB 225" (E2E 2026-10). */
    fun bytes(b: Long): String = "⁦${b.humanBytes()}⁩"
    fun files(n: Int): String = Strings.plural(R.plurals.fmt_files, n, count(n))
    fun filesAndSize(n: Int, bytes: Long): String = "${files(n)} · ${bytes(bytes)}"
    fun percent(fraction: Float): String = "${(fraction * 100).toInt().coerceIn(0, 100)}%"

    private val time: DateTimeFormatter = DateTimeFormatter.ofPattern("HH:mm", Locale.US)

    /** Day and month names follow the app's language; digits stay Western (java.time's default decimal style). */
    private fun pattern(id: Int): DateTimeFormatter =
        DateTimeFormatter.ofPattern(Strings.get(id), Locale.forLanguageTag(Strings.get(R.string.fmt_locale)))

    /** "Just now", "4 min ago", "3 h ago", "Yesterday", "Mon", "Sep 28", "Sep 28, 2025". */
    fun relative(epochMs: Long, now: Long = System.currentTimeMillis()): String = relative(epochMs, now, lower = false)

    /** Lowercase form for running text: "synced 4 min ago", "synced yesterday". Day and month names keep their capitals. */
    fun relativeLower(epochMs: Long, now: Long = System.currentTimeMillis()): String = relative(epochMs, now, lower = true)

    private fun relative(epochMs: Long, now: Long, lower: Boolean): String {
        val diff = now - epochMs
        if (diff < 60_000) return Strings.get(if (lower) R.string.fmt_just_now_lower else R.string.common_just_now)
        if (diff < 3_600_000) (diff / 60_000).toInt().let { return Strings.plural(R.plurals.fmt_minutes_ago, it, it) }
        val zone = ZoneId.systemDefault()
        val then = Instant.ofEpochMilli(epochMs).atZone(zone)
        val today = Instant.ofEpochMilli(now).atZone(zone).toLocalDate()
        val d = then.toLocalDate()
        if (diff < 12 * 3_600_000L || d == today) (diff / 3_600_000).toInt().let { return Strings.plural(R.plurals.fmt_hours_ago, it, it) }
        if (d == today.minusDays(1)) return Strings.get(if (lower) R.string.fmt_yesterday_lower else R.string.common_yesterday)
        if (d.isAfter(today.minusDays(7))) return pattern(R.string.fmt_pattern_day).format(then)
        return pattern(if (d.year == today.year) R.string.fmt_pattern_date else R.string.fmt_pattern_date_year).format(then)
    }

    /** "Today 14:03", "Yesterday 09:15", "Sep 28, 14:03". */
    fun dayAndTime(epochMs: Long, now: Long = System.currentTimeMillis()): String {
        val zone = ZoneId.systemDefault()
        val then = Instant.ofEpochMilli(epochMs).atZone(zone)
        val today = LocalDate.now(zone)
        return when (then.toLocalDate()) {
            today -> Strings.get(R.string.fmt_today_at, time.format(then))
            today.minusDays(1) -> Strings.get(R.string.fmt_yesterday_at, time.format(then))
            else -> pattern(R.string.fmt_pattern_date_time).format(then)
        }
    }

    fun clock(epochMs: Long): String = time.format(Instant.ofEpochMilli(epochMs).atZone(ZoneId.systemDefault()))

    /** "About 6 min left"; null when under a minute or unknown. */
    fun eta(remainingMs: Long?): String? {
        if (remainingMs == null || remainingMs < 60_000) return null
        val min = ((remainingMs + 30_000) / 60_000).toInt()
        return if (min < 60) Strings.plural(R.plurals.fmt_eta_minutes, min, min)
        else ((min + 30) / 60).let { Strings.plural(R.plurals.fmt_eta_hours, it, it) }
    }

    /** "every 6 hours", "every 15 minutes". */
    fun interval(minutes: Int): String = when {
        minutes % 1440 == 0 -> (minutes / 1440).let { Strings.plural(R.plurals.fmt_every_days, it, it) }
        minutes % 60 == 0 -> (minutes / 60).let { Strings.plural(R.plurals.fmt_every_hours, it, it) }
        else -> Strings.plural(R.plurals.fmt_every_minutes, minutes, minutes)
    }

    /** Human path for cards: "DCIM / Camera" from "/storage/emulated/0/DCIM/Camera". */
    fun humanLocalPath(absolute: String): String {
        if (absolute.startsWith("content://")) return safFilePath(absolute)?.let { humanLocalPath(it) } ?: Strings.get(R.string.fmt_chosen_folder)
        val rel = absolute.removePrefix("/storage/emulated/0").trim('/')
        val sd = Regex("^/?storage/([0-9A-F]{4}-[0-9A-F]{4})/?(.*)$").find(absolute)
        return when {
            sd != null -> sd.groupValues[2].trim('/', ' ').let { if (it.isEmpty()) Strings.get(R.string.fmt_sd_card) else Strings.get(R.string.fmt_sd_card_path, it.replace("/", " / ")) }
            rel.isEmpty() -> Strings.get(R.string.fmt_internal_storage)
            else -> rel.replace("/", " / ")
        }
    }

    /**
     * The file path a folder chosen in the system picker stands for ("primary:DCIM/Camera" is
     * /storage/emulated/0/DCIM/Camera), or null when another app provides it. Only for display and
     * overlap checks: the engine always goes through SAF for such a folder.
     */
    fun safFilePath(treeUri: String): String? = com.almodhayan.drivesync.core.SafPaths.filePath(treeUri)

    /** Human Drive path: "My Drive / Photos / Pixel". */
    fun humanRemotePath(path: String): String = path.trim('/').replace("/", " / ")
}
