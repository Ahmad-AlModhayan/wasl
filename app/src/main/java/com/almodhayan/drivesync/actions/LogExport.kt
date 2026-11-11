package com.almodhayan.drivesync.actions

import android.content.Context
import android.content.Intent
import com.almodhayan.drivesync.BuildConfig
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Hashing
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.EventLevel
import com.almodhayan.drivesync.data.db.SyncEventEntity
import com.almodhayan.drivesync.ui.components.LogLevel
import com.almodhayan.drivesync.ui.components.LogLine
import dagger.hilt.android.qualifiers.ApplicationContext
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale
import javax.inject.Inject
import javax.inject.Singleton

/** The sync log for Diagnostics: lines for the LogView and an export with file names hidden by default. */
@Singleton
class LogExport @Inject constructor(@ApplicationContext private val context: Context, private val db: Db) {
    private val clock = DateTimeFormatter.ofPattern("HH:mm:ss", Locale.US).withZone(ZoneId.systemDefault())
    private val full = DateTimeFormatter.ofPattern("yyyy-MM-dd HH:mm:ss", Locale.US).withZone(ZoneId.systemDefault())

    fun toLine(e: SyncEventEntity, pairName: String? = null, hideNames: Boolean = false): LogLine {
        val level = when (e.level) {
            EventLevel.ERROR -> LogLevel.ERROR
            EventLevel.WARN -> LogLevel.WARN
            EventLevel.INFO -> if (e.action in DONE_ACTIONS) LogLevel.OK else LogLevel.INFO
        }
        val path = e.path?.let { if (hideNames) redact(it) else it }
        val msg = buildString {
            if (pairName != null) append(pairName).append(" · ")
            if (path != null) append(path).append(": ")
            append(e.message)
        }
        return LogLine(clock.format(Instant.ofEpochMilli(e.at)), level, msg, e.detail)
    }

    suspend fun lines(since: Long, pairId: String? = null, errorsOnly: Boolean = false): List<LogLine> {
        val names = db.pairs().getAll().associate { it.id to it.name }
        return db.events().since(since)
            .filter { pairId == null || it.pairId == pairId }
            .filter { !errorsOnly || it.level != EventLevel.INFO }
            .asReversed()
            .map { toLine(it, names[it.pairId]) }
    }

    /** Plain-text export; file and folder names are replaced by stable placeholders unless [includeNames]. */
    suspend fun export(since: Long, includeNames: Boolean = false): String {
        val names = db.pairs().getAll().associate { it.id to it.name }
        val sb = StringBuilder()
        sb.append("Wasl ${BuildConfig.VERSION_NAME} log, exported ${full.format(Instant.now())}\n")
        sb.append(if (includeNames) "File names included.\n\n" else "File names hidden.\n\n")
        for (e in db.events().since(since)) {
            val pair = names[e.pairId]?.let { if (includeNames) it else "pair-" + Hashing.sha256(it.toByteArray()).take(6) } ?: "removed pair"
            val path = e.path?.let { if (includeNames) it else redact(it) }
            sb.append(full.format(Instant.ofEpochMilli(e.at))).append(' ').append(e.level.name.padEnd(5)).append(' ').append(e.action.padEnd(13)).append(' ')
                .append(pair).append(path?.let { " $it" } ?: "").append(" — ").append(if (includeNames) e.message else redactMessage(e.message))
            if (!e.detail.isNullOrBlank() && !e.detail.startsWith("md5:")) sb.append(" (").append(e.detail).append(')')
            sb.append('\n')
        }
        return sb.toString()
    }

    fun shareIntent(text: String): Intent = Intent.createChooser(
        Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_SUBJECT, context.getString(R.string.actions_log_subject)).putExtra(Intent.EXTRA_TEXT, text),
        context.getString(R.string.actions_export_log),
    ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

    /** "Work/Invoices/Invoice 0941.pdf" → "folder-3fa1/folder-9c2e/file-77b0.pdf". */
    fun redact(path: String): String = path.split('/').let { parts ->
        parts.mapIndexed { i, p ->
            val h = Hashing.sha256(p.toByteArray()).take(4)
            if (i == parts.lastIndex) {
                val ext = p.substringAfterLast('.', "").takeIf { it.length in 1..5 }
                "file-$h" + (ext?.let { ".$it" } ?: "")
            } else "folder-$h"
        }.joinToString("/")
    }

    private fun redactMessage(m: String): String = m.replace(Regex("\"[^\"]+\""), "\"…\"")

    companion object {
        private val DONE_ACTIONS = setOf("UPLOAD", "DOWNLOAD", "EXPORT", "MKDIR", "DELETE_LOCAL", "DELETE_REMOTE")
    }
}
