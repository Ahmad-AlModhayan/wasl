@file:OptIn(androidx.compose.ui.ExperimentalComposeUiApi::class)

package com.almodhayan.drivesync.ui.pair

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Environment
import android.provider.DocumentsContract
import android.provider.Settings
import android.widget.Toast
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.testTagsAsResourceId
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.res.stringResource
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.db.RunStatus
import com.almodhayan.drivesync.data.db.SyncMode
import com.almodhayan.drivesync.data.prefs.AppSettings
import com.almodhayan.drivesync.drive.DriveNodeCache
import com.almodhayan.drivesync.sync.PairProgress
import com.almodhayan.drivesync.sync.Phase
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.Routes
import com.almodhayan.drivesync.ui.components.Callout
import com.almodhayan.drivesync.ui.components.CalloutTone
import com.almodhayan.drivesync.ui.components.DsDialog
import com.almodhayan.drivesync.ui.components.DsLinearProgress
import com.almodhayan.drivesync.ui.components.DsTextField
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.PathBreadcrumb
import com.almodhayan.drivesync.ui.components.SyncProgress
import com.almodhayan.drivesync.ui.components.SyncProgressModel
import com.almodhayan.drivesync.ui.components.UiAction
import com.almodhayan.drivesync.ui.theme.Ds
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import com.almodhayan.drivesync.ui.theme.SyncStatus
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import java.io.File
import com.almodhayan.drivesync.ui.components.StatusChip as DsStatusChip

/** Result keys the picker screens write into the editor's SavedStateHandle (see Nav.kt). */
object PickerKeys {
    const val LOCAL_PATH = "localPath"
    const val REMOTE_FOLDER_ID = "remoteFolderId"
    const val REMOTE_PATH = "remotePath"
    const val DRIVE_ID = "driveId"
}

/**
 * Presentation helpers shared by the pair, activity, trash, revision and Drive screens, in the
 * design system's words: the two sides are "phone" and "Drive", states use the six status words.
 */
object PairUi {

    fun statusLabel(s: RunStatus?): String = when (s) {
        null -> Strings.get(R.string.common_not_synced_yet)
        RunStatus.RUNNING -> Strings.get(R.string.common_syncing)
        RunStatus.SUCCESS -> Strings.get(R.string.common_synced)
        RunStatus.PARTIAL -> Strings.get(R.string.pair_status_partly_synced)
        RunStatus.FAILED -> Strings.get(R.string.pair_status_failed)
        RunStatus.BLOCKED -> Strings.get(R.string.pair_status_needs_approval)
        RunStatus.SKIPPED -> Strings.get(R.string.common_skipped)
        RunStatus.CANCELLED -> Strings.get(R.string.common_stopped)
    }

    /** Fixed status colours (never dynamic colour). */
    @Composable
    fun statusColor(s: RunStatus?): Color = when (s) {
        RunStatus.SUCCESS -> Ds.status.synced.content
        RunStatus.RUNNING -> Ds.status.syncing.content
        RunStatus.BLOCKED -> Ds.status.approval.content
        RunStatus.FAILED, RunStatus.PARTIAL -> Ds.status.failed.content
        else -> Ds.status.waiting.content
    }

    fun phaseLabel(p: Phase): String = when (p) {
        Phase.PREPARING -> Strings.get(R.string.pair_phase_preparing)
        Phase.LISTING_REMOTE -> Strings.get(R.string.pair_phase_listing_remote)
        Phase.SCANNING_LOCAL -> Strings.get(R.string.pair_phase_scanning_local)
        Phase.PLANNING -> Strings.get(R.string.pair_phase_planning)
        Phase.TRANSFERRING -> Strings.get(R.string.pair_phase_transferring)
        Phase.DELETING -> Strings.get(R.string.pair_phase_deleting)
        Phase.FINISHING -> Strings.get(R.string.pair_phase_finishing)
    }

    fun effectiveIntervalMinutes(pair: FolderPairEntity, cfg: AppSettings): Int =
        if (pair.intervalMinutes > 0) pair.intervalMinutes else cfg.globalIntervalMinutes

    /** Epoch millis of the next periodic run, or null when the pair is not scheduled. */
    fun nextRunEstimate(pair: FolderPairEntity, cfg: AppSettings): Long? {
        if (!pair.enabled || pair.paused) return null
        val last = pair.lastRunAt ?: return null
        return last + effectiveIntervalMinutes(pair, cfg) * 60_000L
    }

    /** "Every 6 hours"; 0 follows the app's interval. */
    fun intervalLabel(minutes: Int): String = when {
        minutes <= 0 -> Strings.get(R.string.pair_interval_app_default)
        minutes < 60 -> Strings.plural(R.plurals.pair_interval_every_minutes, minutes)
        minutes % 1440 == 0 -> Strings.plural(R.plurals.pair_interval_every_days, minutes / 1440)
        minutes % 60 == 0 -> Strings.plural(R.plurals.pair_interval_every_hours, minutes / 60)
        else -> Strings.plural(R.plurals.pair_interval_every_minutes, minutes)
    }

    fun duration(ms: Long): String {
        if (ms < 0) return "—"
        val s = ms / 1000
        return when {
            s < 60 -> Strings.get(R.string.pair_duration_seconds, s)
            s < 3600 -> Strings.get(R.string.pair_duration_minutes_seconds, s / 60, s % 60)
            else -> Strings.get(R.string.pair_duration_hours_minutes, s / 3600, (s % 3600) / 60)
        }
    }

    /** What a destructive mode does, in plain words; null when the mode never deletes. */
    fun modeWarning(mode: SyncMode): String? = when (mode) {
        SyncMode.UPLOAD_MIRROR -> Strings.get(R.string.pair_mode_warning_upload_mirror)
        SyncMode.DOWNLOAD_MIRROR -> Strings.get(R.string.pair_mode_warning_download_mirror)
        SyncMode.UPLOAD_THEN_DELETE -> Strings.get(R.string.pair_mode_warning_upload_then_delete)
        SyncMode.DOWNLOAD_THEN_DELETE -> Strings.get(R.string.pair_mode_warning_download_then_delete)
        else -> null
    }

    /** One-line status shared with cards and lists. */
    fun statusLine(pair: FolderPairEntity): String = when {
        pair.pendingApproval -> Strings.get(R.string.pair_status_line_needs_approval, pair.pendingApprovalSummary ?: Strings.get(R.string.pair_status_line_approval_default))
        pair.paused -> pair.pauseReason?.let { Strings.get(R.string.pair_status_line_paused_reason, it) } ?: Strings.get(R.string.common_paused)
        !pair.enabled -> Strings.get(R.string.pair_status_turned_off)
        pair.lastStatus == null -> Strings.get(R.string.common_not_synced_yet)
        else -> {
            val base = statusLabel(pair.lastStatus) + (pair.lastRunAt?.let { " · " + Fmt.relative(it) } ?: "")
            if (pair.lastStatus != RunStatus.SUCCESS && !pair.lastError.isNullOrBlank()) "$base · ${pair.lastError}" else base
        }
    }

    fun driveFolderUrl(id: String) = "https://drive.google.com/drive/folders/$id"
    fun driveFileUrl(id: String) = "https://drive.google.com/file/d/$id/view"

    fun openUrl(context: Context, url: String) {
        try {
            context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        } catch (e: ActivityNotFoundException) {
            Toast.makeText(context, context.getString(R.string.pair_toast_no_app_for_link), Toast.LENGTH_LONG).show()
        }
    }

    /**
     * Opens [localPath] in the system Files app through the external storage documents provider.
     * [volumes] come from LocalFs.volumes(); the first one is the primary volume.
     */
    fun openDeviceFolder(context: Context, localPath: String, volumes: List<File>) {
        try {
            @Suppress("DEPRECATION")
            val primary = Environment.getExternalStorageDirectory()
            val volume = (listOf(primary) + volumes).firstOrNull { localPath == it.path || localPath.startsWith(it.path + "/") }
            val volumeId = if (volume == null || volume.path == primary.path) "primary" else volume.name
            val rel = if (volume == null) localPath.trimStart('/') else localPath.removePrefix(volume.path).trimStart('/')
            val uri = DocumentsContract.buildDocumentUri("com.android.externalstorage.documents", "$volumeId:$rel")
            val intent = Intent(Intent.ACTION_VIEW).setDataAndType(uri, DocumentsContract.Document.MIME_TYPE_DIR).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            context.startActivity(intent)
        } catch (t: Throwable) {
            Toast.makeText(context, context.getString(R.string.pair_toast_no_files_app), Toast.LENGTH_LONG).show()
        }
    }
}

/** Pops the current screen and shows the pair detail, without stacking a second detail screen on top of an existing one. */
fun AppNav.showPairDetailReplacing(pairId: String) {
    val prev = controller.previousBackStackEntry
    val prevIsDetail = prev?.destination?.route == Routes.PAIR_DETAIL && prev.arguments?.getString("pairId") == pairId
    back()
    if (!prevIsDetail) pairDetail(pairId)
}

/** Opens the system page where the person grants "All files access" to this app. */
fun openAllFilesAccess(context: Context) {
    try {
        context.startActivity(Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, Uri.parse("package:" + context.packageName)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    } catch (t: Throwable) {
        try { context.startActivity(Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) } catch (_: Throwable) {}
    }
}

// ------------------------------------------------------------------ legacy composables, restyled

/** A run result as a status chip: icon and word, quiet or loud by the status system. */
@Composable
fun StatusChip(status: RunStatus?, label: String = PairUi.statusLabel(status)) {
    val s = when (status) {
        RunStatus.SUCCESS -> SyncStatus.Synced
        RunStatus.RUNNING -> SyncStatus.Syncing
        RunStatus.BLOCKED -> SyncStatus.Approval
        RunStatus.FAILED, RunStatus.PARTIAL -> SyncStatus.Failed
        RunStatus.SKIPPED, RunStatus.CANCELLED, null -> SyncStatus.Waiting
    }
    val pausedWord = stringResource(R.string.common_paused)
    val icon: ImageVector? = when {
        status == RunStatus.CANCELLED || label.startsWith(pausedWord) -> Sym.Pause
        status == RunStatus.SKIPPED || status == null -> Sym.Schedule
        else -> null
    }
    DsStatusChip(s, label = label, icon = icon)
}

/** Live progress of a running pair: counts, bytes, current file and time left. */
@Composable
fun ProgressBlock(p: PairProgress, modifier: Modifier = Modifier) {
    val scanning = p.phase == Phase.PREPARING || p.phase == Phase.LISTING_REMOTE || p.phase == Phase.SCANNING_LOCAL || p.phase == Phase.PLANNING
    val elapsed = System.currentTimeMillis() - p.startedAt
    val f = p.fraction
    val eta = if (!scanning && f > 0.02f && elapsed > 5_000) Fmt.eta((elapsed / f * (1 - f)).toLong()) else null
    SyncProgress(
        SyncProgressModel(
            scanning = scanning, value = if (scanning) null else f, done = p.doneActions, total = p.totalActions,
            doneBytes = p.bytesDone, totalBytes = p.bytesTotal, current = p.currentFile?.substringAfterLast('/'), eta = eta,
        ),
        modifier,
    )
}

enum class MessageKind { INFO, WARNING, ERROR }

/** Inline guidance; now drawn as the design system's [Callout]. */
@Composable
fun MessageCard(text: String, kind: MessageKind = MessageKind.INFO, modifier: Modifier = Modifier, actionLabel: String? = null, onAction: (() -> Unit)? = null) {
    val tone = when (kind) {
        MessageKind.INFO -> CalloutTone.Info
        MessageKind.WARNING -> CalloutTone.Warning
        MessageKind.ERROR -> CalloutTone.Danger
    }
    Callout(text, modifier, tone = tone, action = if (actionLabel != null && onAction != null) UiAction(actionLabel, onClick = onAction) else null)
}

/** Confirmation for irreversible actions; now drawn as the design system's [DsDialog]. */
@Composable
fun ConfirmDialog(
    title: String,
    text: String,
    confirmLabel: String,
    onConfirm: () -> Unit,
    onDismiss: () -> Unit,
    destructive: Boolean = true,
) {
    DsDialog(title = title, body = text, confirmLabel = confirmLabel, onConfirm = { onDismiss(); onConfirm() }, onDismiss = onDismiss, danger = destructive)
}

/**
 * Breadcrumb strip for folder browsers, drawn as [PathBreadcrumb]. With [onHome] the strip starts at
 * [homeLabel] and every entry of [items] is a level below it; without it the first item is the root.
 */
@Composable
fun Breadcrumbs(items: List<String>, onClick: (Int) -> Unit, onHome: (() -> Unit)? = null, homeLabel: String = stringResource(R.string.pair_storage), rootIcon: ImageVector = Sym.Folder) {
    if (onHome != null) {
        PathBreadcrumb(rootLabel = homeLabel, rootIcon = rootIcon, parts = items, onCrumb = { i -> if (i < 0) onHome() else onClick(i) })
    } else if (items.isNotEmpty()) {
        PathBreadcrumb(rootLabel = items.first(), rootIcon = rootIcon, parts = items.drop(1), onCrumb = { i -> onClick(i + 1) })
    }
}

/**
 * Name prompt for a new folder. [onCreate] returns true to close the dialog; callers that create the
 * folder asynchronously return false and close it themselves.
 */
@Composable
fun NewFolderDialog(onDismiss: () -> Unit, onCreate: (String) -> Boolean, error: String? = null, busy: Boolean = false) {
    var name by rememberSaveable { mutableStateOf("") }
    val cs = MaterialTheme.colorScheme
    val folderNameLabel = stringResource(R.string.pair_folder_name)
    AlertDialog(
        onDismissRequest = { if (!busy) onDismiss() },
        modifier = Modifier.semantics { testTagsAsResourceId = true },   // dialogs are their own window
        icon = { Icon(Sym.CreateNewFolder, contentDescription = null, tint = cs.secondary) },
        title = { Text(stringResource(R.string.common_new_folder), style = MaterialTheme.typography.headlineSmall) },
        text = {
            val focus = remember { FocusRequester() }
            LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
            Column(verticalArrangement = Arrangement.spacedBy(Space.s12)) {
                DsTextField(
                    value = name, onValueChange = { name = it }, label = folderNameLabel, error = error,
                    modifier = Modifier.focusRequester(focus).testTag("new_folder_name"),
                    keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences, imeAction = ImeAction.Done),
                    // Done on the keyboard does what Create does, under the same conditions.
                    keyboardActions = KeyboardActions(onDone = { if (!busy && name.isNotBlank() && onCreate(name.trim())) onDismiss() }),
                )
                if (busy) DsLinearProgress(null)
            }
        },
        confirmButton = {
            Button(onClick = { if (onCreate(name.trim())) onDismiss() }, enabled = !busy && name.isNotBlank(), modifier = Modifier.testTag("new_folder_create")) { Text(stringResource(R.string.pair_create_folder)) }
        },
        dismissButton = { TextButton(onClick = onDismiss, enabled = !busy) { Text(stringResource(R.string.common_cancel)) } },
        containerColor = cs.surfaceContainerHigh,
        shape = MaterialTheme.shapes.extraLarge,
    )
}

// ------------------------------------------------------------------ folder sizes

/** Files and bytes inside one folder. [capped] means counting stopped early, so [files] is a floor. */
data class FolderCount(val files: Int, val bytes: Long, val capped: Boolean = false) {
    /** "2,612 files · 23.4 GB", "10,000+ files", "Empty". */
    val text: String
        get() = when {
            capped -> Strings.plural(R.plurals.pair_files_at_least, files, Fmt.count(files))
            files == 0 -> Strings.get(R.string.pair_folder_empty)
            else -> Fmt.filesAndSize(files, bytes)
        }
}

/** Counts the files under [root], skipping hidden ones unless [includeHidden]; stops at [cap] files. Cancellable. */
internal suspend fun countPhoneFolder(root: File, includeHidden: Boolean, cap: Int = Int.MAX_VALUE): FolderCount {
    var files = 0
    var bytes = 0L
    val stack = ArrayDeque<File>()
    stack.addLast(root)
    while (stack.isNotEmpty()) {
        currentCoroutineContext().ensureActive()
        val dir = stack.removeLast()
        val kids = dir.listFiles() ?: continue
        for (f in kids) {
            if (!includeHidden && f.name.startsWith(".")) continue
            if (f.isDirectory) {
                stack.addLast(f)
            } else if (f.isFile) {
                files++
                bytes += f.length()
                if (files >= cap) return FolderCount(files, bytes, capped = true)
            }
        }
    }
    return FolderCount(files, bytes)
}

/**
 * Subtree totals from the Drive metadata cache, memoised per folder so a whole picker level costs one
 * walk. Not thread-safe: use it from one coroutine at a time.
 */
internal class DriveTotals(private val snap: DriveNodeCache.Snapshot) {
    private val memo = HashMap<String, FolderCount>()

    /** Whether the cache knows this folder at all (uncrawled shared drives are unknown, not empty). */
    fun knows(id: String): Boolean = snap.node(id) != null || snap.childrenOf(id).isNotEmpty()

    fun of(id: String): FolderCount {
        memo[id]?.let { return it }
        val stack = ArrayDeque<String>()
        val opened = HashSet<String>()
        stack.addLast(id)
        while (stack.isNotEmpty()) {
            val cur = stack.last()
            if (memo.containsKey(cur)) { stack.removeLast(); continue }
            if (opened.add(cur)) {
                for (n in snap.childrenOf(cur)) {
                    if (n.isFolder && !n.trashed && !memo.containsKey(n.id) && n.id !in opened) stack.addLast(n.id)
                }
            } else {
                stack.removeLast()
                var files = 0
                var bytes = 0L
                for (n in snap.childrenOf(cur)) {
                    if (n.trashed) continue
                    if (n.isFolder) {
                        memo[n.id]?.let { files += it.files; bytes += it.bytes }
                    } else if (!n.isShortcut) {
                        files++
                        bytes += n.size ?: 0L
                    }
                }
                memo[cur] = FolderCount(files, bytes)
            }
        }
        return memo[id] ?: FolderCount(0, 0)
    }

    companion object {
        /** Ancestor folder ids of [id] in the cache, nearest first; the My Drive root id ends the list. */
        fun ancestors(snap: DriveNodeCache.Snapshot, id: String): List<String> {
            val out = ArrayList<String>()
            var cur = snap.node(id)?.parentId
            var guard = 0
            while (cur != null && guard++ < 256) {
                out += cur
                cur = snap.node(cur)?.parentId
            }
            return out
        }
    }
}

/** Keeps pair names short enough for a list row's meta slot. */
internal fun shortName(name: String, max: Int = 20): String = if (name.length > max) name.take(max - 1).trimEnd() + "…" else name

// ------------------------------------------------------------------ skip and include patterns

/**
 * Patterns are stored one per line and edited as a comma-separated list. Commas inside [..] classes
 * and {..} alternatives belong to the pattern; a stored pattern with a bare comma is shown as [,] so
 * it survives the round trip unchanged.
 */
internal object PatternText {
    fun toUi(patterns: List<String>): String = patterns.joinToString(", ") { escapeCommas(it) }

    fun parse(text: String): List<String> {
        val out = ArrayList<String>()
        val cur = StringBuilder()
        var inClass = false
        var braces = 0
        fun flush() {
            val p = cur.toString().trim()
            if (p.isNotEmpty()) out += p
            cur.setLength(0)
        }
        for (c in text) {
            if (c == '\n' || c == '\r') {
                flush(); inClass = false; braces = 0
            } else if (c == ',' && !inClass && braces == 0) {
                flush()
            } else {
                if (c == '[' && !inClass) inClass = true
                else if (c == ']' && inClass) inClass = false
                else if (c == '{' && !inClass) braces++
                else if (c == '}' && !inClass && braces > 0) braces--
                cur.append(c)
            }
        }
        flush()
        return out
    }

    private fun escapeCommas(p: String): String {
        val sb = StringBuilder()
        var inClass = false
        var braces = 0
        for (c in p) {
            if (c == ',' && !inClass && braces == 0) {
                sb.append("[,]")
            } else {
                if (c == '[' && !inClass) inClass = true
                else if (c == ']' && inClass) inClass = false
                else if (c == '{' && !inClass) braces++
                else if (c == '}' && !inClass && braces > 0) braces--
                sb.append(c)
            }
        }
        return sb.toString()
    }
}
