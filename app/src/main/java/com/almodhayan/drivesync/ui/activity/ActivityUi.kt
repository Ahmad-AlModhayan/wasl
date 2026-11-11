package com.almodhayan.drivesync.ui.activity

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.core.TimeFmt
import com.almodhayan.drivesync.data.db.EventLevel
import com.almodhayan.drivesync.data.db.RunStatus
import com.almodhayan.drivesync.data.db.SyncEventEntity
import com.almodhayan.drivesync.data.db.SyncRunEntity
import com.almodhayan.drivesync.ui.components.ChangeAction
import com.almodhayan.drivesync.ui.components.DsTextField
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.LogLevel
import com.almodhayan.drivesync.ui.components.LogLine
import com.almodhayan.drivesync.ui.components.LogLineRow
import com.almodhayan.drivesync.ui.theme.Ds
import com.almodhayan.drivesync.ui.theme.MonoSmall
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import com.almodhayan.drivesync.ui.theme.SyncStatus
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale

// Shared words and rows for the activity screens (status, history, run details, problems) and the
// dry run's live log. Everything here uses the design system's vocabulary: phone and Drive, the
// recycle bin, "couldn't sync" instead of "error".

// ------------------------------------------------------------------ event levels

/** Icon for an event level: info outline, warning and error filled (status icons are filled). */
fun levelIcon(level: EventLevel): ImageVector = when (level) {
    EventLevel.INFO -> Sym.Info
    EventLevel.WARN -> Sym.WarningFill
    EventLevel.ERROR -> Sym.ErrorFill
}

/** Status colours for event levels: warnings use needs-approval amber, errors the failed red. */
@Composable
fun levelColor(level: EventLevel): Color = when (level) {
    EventLevel.INFO -> MaterialTheme.colorScheme.onSurfaceVariant
    EventLevel.WARN -> Ds.status.approval.content
    EventLevel.ERROR -> Ds.status.failed.content
}

fun logLevel(level: EventLevel): LogLevel = when (level) {
    EventLevel.INFO -> LogLevel.INFO
    EventLevel.WARN -> LogLevel.WARN
    EventLevel.ERROR -> LogLevel.ERROR
}

/** Event actions in the app's words. */
fun actionLabel(action: String): String = when (action.uppercase()) {
    "UPLOAD" -> Strings.get(R.string.common_to_drive)
    "DOWNLOAD", "EXPORT" -> Strings.get(R.string.common_to_phone)
    "DELETE_LOCAL" -> Strings.get(R.string.activity_action_removed_on_phone)
    "DELETE_REMOTE" -> Strings.get(R.string.activity_action_removed_on_drive)
    "CONFLICT" -> Strings.get(R.string.common_conflict)
    "SKIP" -> Strings.get(R.string.common_skipped)
    "ERROR" -> Strings.get(R.string.activity_action_couldnt_sync)
    "INFO" -> Strings.get(R.string.activity_action_note)
    "MKDIR" -> Strings.get(R.string.common_new_folder)
    "BLOCKED" -> Strings.get(R.string.activity_action_stopped_to_ask)
    else -> action.lowercase().replaceFirstChar { it.uppercase() }
}

// ------------------------------------------------------------------ times

private val clockSeconds: DateTimeFormatter = DateTimeFormatter.ofPattern("HH:mm:ss", Locale.US)
/** "Sep 28" / "28 سبتمبر": the app language's short date (same pattern as [Fmt.relative]). */
private val shortDate: DateTimeFormatter
    get() = DateTimeFormatter.ofPattern(Strings.get(R.string.fmt_pattern_date), Locale.forLanguageTag(Strings.get(R.string.fmt_locale)))

/** "10:42:13" for a log line. */
fun logClock(epochMs: Long): String = clockSeconds.format(Instant.ofEpochMilli(epochMs).atZone(ZoneId.systemDefault()))

/** Time for a line in a list that spans days: "10:42" today, "Sep 28" before. */
fun logTime(epochMs: Long): String {
    val zone = ZoneId.systemDefault()
    val then = Instant.ofEpochMilli(epochMs).atZone(zone)
    return if (then.toLocalDate() == LocalDate.now(zone)) Fmt.clock(epochMs) else shortDate.format(then)
}

/** For running text: "at 10:40", "yesterday at 10:40", "tomorrow at 06:00", "on Sep 28". */
fun atTime(epochMs: Long): String {
    val zone = ZoneId.systemDefault()
    val day = Instant.ofEpochMilli(epochMs).atZone(zone).toLocalDate()
    val today = LocalDate.now(zone)
    return when (day) {
        today -> Strings.get(R.string.activity_at_time, Fmt.clock(epochMs))
        today.minusDays(1) -> Strings.get(R.string.activity_yesterday_at_time, Fmt.clock(epochMs))
        today.plusDays(1) -> Strings.get(R.string.activity_tomorrow_at_time, Fmt.clock(epochMs))
        else -> Strings.get(R.string.activity_on_date, shortDate.format(Instant.ofEpochMilli(epochMs).atZone(zone)))
    }
}

/** "42 s", "3 min 12 s", "1 h 5 min". */
fun durationText(ms: Long): String {
    val s = (ms / 1000).coerceAtLeast(0)
    return when {
        s < 60 -> Strings.get(R.string.activity_duration_seconds, s.toInt())
        s < 3600 -> Strings.get(R.string.activity_duration_minutes_seconds, (s / 60).toInt(), (s % 60).toInt())
        else -> Strings.get(R.string.activity_duration_hours_minutes, (s / 3600).toInt(), ((s % 3600) / 60).toInt())
    }
}

// ------------------------------------------------------------------ log lines

/** One event as a log line: the file name, then what happened. */
fun logLineOf(e: SyncEventEntity, time: String = logClock(e.at)): LogLine {
    val what = listOfNotNull(e.path?.substringAfterLast('/')?.takeIf { it.isNotEmpty() }, e.message.takeIf { it.isNotBlank() }).joinToString(": ")
    return LogLine(time, logLevel(e.level), what, e.detail)
}

/** One sync event as a log line; tap to show the full path, message and detail. */
@Composable
fun EventRow(e: SyncEventEntity, expanded: Boolean, onToggle: () -> Unit, showTime: Boolean = true) {
    val line = logLineOf(e).let { if (showTime) it else it.copy(time = "") }
    Column(
        Modifier.fillMaxWidth()
            .clickable(onClickLabel = stringResource(if (expanded) R.string.activity_show_less else R.string.activity_show_all), onClick = onToggle)
            .heightIn(min = 48.dp)
            .padding(horizontal = Space.s16, vertical = Space.s8),
        verticalArrangement = Arrangement.Center,
    ) {
        LogLineRow(line)
        if (expanded) {
            Text(
                listOfNotNull(e.path, e.message.takeIf { it.isNotBlank() }, e.detail).joinToString("\n"),
                style = MonoSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(top = Space.s4),
            )
        }
    }
}

/** Plain-text export of an event list (for the clipboard). */
fun eventsToText(events: List<SyncEventEntity>, header: String? = null): String = buildString {
    if (header != null) { append(header); append('\n') }
    for (e in events) {
        append(TimeFmt.human(e.at)); append("  ")
        append(e.level.name.padEnd(5)); append("  ")
        append(e.action.padEnd(13)); append("  ")
        e.path?.let { append(it); append("  —  ") }
        append(e.message)
        e.detail?.let { append("  [").append(it).append(']') }
        append('\n')
    }
}

// ------------------------------------------------------------------ runs

/** The status chip for one sync: the six states, with words for how a run ended. */
data class RunBadge(val status: SyncStatus, val label: String, val icon: ImageVector? = null)

fun runBadge(status: RunStatus): RunBadge = when (status) {
    RunStatus.RUNNING -> RunBadge(SyncStatus.Syncing, Strings.get(R.string.common_syncing))
    RunStatus.SUCCESS -> RunBadge(SyncStatus.Synced, SyncStatus.Synced.label)
    RunStatus.PARTIAL -> RunBadge(SyncStatus.Failed, Strings.get(R.string.activity_run_partly_synced))
    RunStatus.FAILED -> RunBadge(SyncStatus.Failed, SyncStatus.Failed.label)
    RunStatus.BLOCKED -> RunBadge(SyncStatus.Approval, Strings.get(R.string.activity_action_stopped_to_ask))
    RunStatus.SKIPPED -> RunBadge(SyncStatus.Waiting, Strings.get(R.string.activity_run_didnt_start), Sym.Schedule)
    RunStatus.CANCELLED -> RunBadge(SyncStatus.Waiting, Strings.get(R.string.common_stopped), Sym.Pause)
}

/** "34 to Drive · 12 to phone · 3 to the recycle bin · 2 conflicts", or "No changes". */
fun runCounts(r: SyncRunEntity): String {
    val parts = buildList {
        if (r.uploaded > 0) add(Strings.get(R.string.activity_count_to_drive, Fmt.count(r.uploaded)))
        if (r.downloaded > 0) add(Strings.get(R.string.activity_count_to_phone, Fmt.count(r.downloaded)))
        val removed = r.deletedLocal + r.deletedRemote
        if (removed > 0) add(Strings.get(R.string.activity_count_to_bin, Fmt.count(removed)))
        if (r.conflicts > 0) add(Strings.plural(R.plurals.activity_count_conflicts, r.conflicts, Fmt.count(r.conflicts)))
        if (r.errors > 0) add(Strings.get(R.string.activity_count_couldnt_sync, Fmt.count(r.errors)))
    }
    return when {
        parts.isNotEmpty() -> parts.joinToString(" · ")
        r.status == RunStatus.SUCCESS -> Strings.get(R.string.activity_no_changes)
        else -> ""
    }
}

/** What started a sync, in plain words. */
fun triggerText(trigger: String): String = when (trigger.lowercase()) {
    "manual" -> Strings.get(R.string.activity_trigger_manual)
    "periodic" -> Strings.get(R.string.activity_trigger_periodic)
    "instant", "local-change", "local" -> Strings.get(R.string.activity_trigger_local)
    "remote", "remote-change" -> Strings.get(R.string.activity_trigger_remote)
    "boot" -> Strings.get(R.string.activity_trigger_boot)
    "share" -> Strings.get(R.string.activity_trigger_share)
    "tile" -> Strings.get(R.string.activity_trigger_tile)
    "widget" -> Strings.get(R.string.activity_trigger_widget)
    "automation", "intent" -> Strings.get(R.string.common_automation)
    "approved" -> Strings.get(R.string.activity_trigger_approved)
    "restored", "restore" -> Strings.get(R.string.activity_trigger_restore)
    "conflict", "conflict-undo" -> Strings.get(R.string.activity_trigger_conflict)
    "resume" -> Strings.get(R.string.activity_trigger_resume)
    "restore-version", "version-copy" -> Strings.get(R.string.activity_trigger_version)
    else -> trigger.replaceFirstChar { it.uppercase() }
}

// ------------------------------------------------------------------ changes

/** A file event as a change row. */
data class EventChange(val action: ChangeAction, val reason: String)

/** What a file event did, in the app's words. Null for notes, folders and errors. */
fun changeOf(e: SyncEventEntity): EventChange? {
    if (e.path.isNullOrEmpty()) return null
    val m = e.message
    return when (e.action.uppercase()) {
        "UPLOAD" -> EventChange(ChangeAction.Upload, Strings.get(R.string.activity_change_uploaded))
        "DOWNLOAD", "EXPORT" -> EventChange(ChangeAction.Download, Strings.get(R.string.activity_change_downloaded))
        "DELETE_LOCAL" -> EventChange(
            ChangeAction.Bin,
            when {
                m.startsWith("Removed from device after upload") -> Strings.get(R.string.activity_change_removed_after_upload)
                m.startsWith("Removed folder") -> Strings.get(R.string.activity_change_empty_folder_removed)
                else -> Strings.get(R.string.activity_change_moved_to_bin)
            },
        )
        "DELETE_REMOTE" -> EventChange(ChangeAction.Bin, Strings.get(R.string.activity_change_moved_to_drive_bin))
        "CONFLICT" -> EventChange(ChangeAction.Conflict, Strings.get(R.string.activity_change_both_kept))
        "SKIP" -> EventChange(ChangeAction.Skip, skipReason(m))
        else -> null
    }
}

/** Planner and executor skip reasons, in plain words. */
fun skipReason(raw: String): String {
    val r = raw.lowercase()
    return when {
        r.contains("size limit") -> Strings.get(R.string.activity_skip_size_limit)
        r.contains("already uploaded once") -> Strings.get(R.string.activity_skip_deleted_on_drive)
        r.contains("already downloaded once") -> Strings.get(R.string.activity_skip_removed_from_phone)
        r.contains("google docs file ignored") -> Strings.get(R.string.activity_skip_google_doc)
        r.contains("no export format") -> Strings.get(R.string.activity_skip_no_export_format)
        r.contains("modified on both sides") -> Strings.get(R.string.activity_skip_changed_on_both)
        r.contains("folder not empty") -> Strings.get(R.string.activity_skip_folder_not_empty)
        else -> raw.replaceFirstChar { it.uppercase() }
    }
}

// ------------------------------------------------------------------ small shared pieces

/** Icon (18dp) and label inside a button. */
@Composable
internal fun RowScope.ButtonLabel(text: String, icon: ImageVector? = null) {
    if (icon != null) {
        Icon(icon, contentDescription = null, modifier = Modifier.size(18.dp))
        Spacer(Modifier.size(Space.s8))
    }
    Text(text, maxLines = 1, overflow = TextOverflow.Ellipsis)
}

/** The search row under a top app bar. It takes focus when it opens. */
@Composable
internal fun SearchField(query: String, onQuery: (String) -> Unit, label: String, onClose: () -> Unit, modifier: Modifier = Modifier) {
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
    DsTextField(
        value = query,
        onValueChange = onQuery,
        label = label,
        modifier = modifier.padding(horizontal = Space.s16, vertical = Space.s8).focusRequester(focus),
        leading = Sym.Search,
        trailing = { IconButton(onClick = onClose) { Icon(Sym.Close, contentDescription = stringResource(R.string.activity_close_search)) } },
    )
}

/** Opens a web link; false when no app on the phone can open it. */
internal fun openUrl(context: Context, url: String): Boolean = try {
    context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    true
} catch (e: ActivityNotFoundException) {
    false
}

/** Android's "All files access" page for this app (the fix for lost folder access). */
internal fun openAllFilesAccess(context: Context): Boolean {
    val candidates = listOf(
        Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, Uri.parse("package:${context.packageName}")),
        Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION),
    )
    for (i in candidates) {
        try {
            context.startActivity(i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            return true
        } catch (e: ActivityNotFoundException) {
            // try the next one
        }
    }
    return false
}

/** Drive's storage page, for "Free up Drive space". */
internal const val DRIVE_STORAGE_URL = "https://drive.google.com/settings/storage"
