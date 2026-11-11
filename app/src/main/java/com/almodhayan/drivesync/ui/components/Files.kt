package com.almodhayan.drivesync.ui.components

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.annotation.StringRes
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.ui.theme.Ds
import com.almodhayan.drivesync.ui.theme.MonoMedium
import com.almodhayan.drivesync.ui.theme.MonoSmall
import com.almodhayan.drivesync.ui.theme.Opacity
import com.almodhayan.drivesync.ui.theme.Size
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import com.almodhayan.drivesync.ui.theme.SyncStatus
import com.almodhayan.drivesync.ui.theme.tabular

// ------------------------------------------------------------------ file kinds

enum class FileKind {
    Image, Video, Audio, Pdf, Doc, Text, Folder, Shared, Archive, Other;

    val icon: ImageVector
        get() = when (this) {
            Image -> Sym.Image; Video -> Sym.Movie; Audio -> Sym.MusicNote; Pdf -> Sym.PictureAsPdf
            Doc -> Sym.Description; Text -> Sym.Article; Folder -> Sym.Folder; Shared -> Sym.FolderShared
            Archive -> Sym.FolderCopy; Other -> Sym.Description
        }

    companion object {
        private val images = setOf("jpg", "jpeg", "png", "gif", "webp", "heic", "heif", "bmp", "svg", "dng", "raw", "cr2", "nef", "arw", "avif", "tif", "tiff")
        private val videos = setOf("mp4", "mov", "mkv", "avi", "webm", "3gp", "m4v", "wmv", "flv", "mts")
        private val audio = setOf("mp3", "m4a", "aac", "flac", "wav", "ogg", "opus", "wma", "amr", "mid")
        private val docs = setOf("doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "rtf", "pages", "numbers", "key", "csv", "epub")
        private val text = setOf("txt", "md", "json", "xml", "html", "htm", "log", "yaml", "yml", "kt", "java", "py", "js", "ts", "c", "cpp", "h", "sh", "ini", "conf")
        private val archives = setOf("zip", "rar", "7z", "tar", "gz", "bz2", "xz", "apk")

        fun of(name: String, isDir: Boolean = false): FileKind {
            if (isDir) return Folder
            val ext = name.substringAfterLast('.', "").lowercase()
            return when (ext) {
                in images -> Image; in videos -> Video; in audio -> Audio; "pdf" -> Pdf; in docs -> Doc
                in text -> Text; in archives -> Archive; else -> Other
            }
        }

        val videoExtensions: Set<String> get() = videos
    }
}

enum class ItemTone { None, Folder, Drive }

// ------------------------------------------------------------------ ListItem

/**
 * The one row used by pickers, the recycle bin and simple lists. Disabled rows stay visible: the content
 * dims, the reason stays fully readable.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun DsListItem(
    headline: String,
    modifier: Modifier = Modifier,
    supporting: String? = null,
    icon: ImageVector? = null,
    leading: (@Composable () -> Unit)? = null,
    thumb: Boolean = false,
    mono: Boolean = false,
    meta: String? = null,
    trailing: (@Composable () -> Unit)? = null,
    chevron: Boolean = false,
    selected: Boolean = false,
    enabled: Boolean = true,
    disabledReason: String? = null,
    tone: ItemTone = ItemTone.None,
    iconTint: Color? = null,
    onClick: (() -> Unit)? = null,
    onLongClick: (() -> Unit)? = null,
) {
    val cs = MaterialTheme.colorScheme
    val dim = if (enabled) 1f else Opacity.disabledContent
    val clickable = enabled && (onClick != null || onLongClick != null)
    // On the selected tint, both text levels use the container's own on-colour (onSurfaceVariant is too weak there).
    val fg = if (selected) cs.onSecondaryContainer else cs.onSurface
    val fgVariant = if (selected) cs.onSecondaryContainer else cs.onSurfaceVariant
    Row(
        modifier.fillMaxWidth()
            .then(if (selected) Modifier.background(cs.secondaryContainer) else Modifier)
            .then(if (clickable) Modifier.combinedClickable(onClick = { onClick?.invoke() }, onLongClick = onLongClick, role = Role.Button) else Modifier)
            .heightIn(min = 56.dp)
            .padding(horizontal = Space.s16, vertical = Space.s8),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.s16),
    ) {
        if (leading != null) leading()
        else if (icon != null) {
            val tint = iconTint ?: when (tone) { ItemTone.Folder -> cs.primary; ItemTone.Drive -> cs.tertiary; ItemTone.None -> cs.onSurfaceVariant }
            Box(
                Modifier.size(40.dp).alpha(dim).then(if (thumb) Modifier.clip(MaterialTheme.shapes.small).background(cs.surfaceContainerHighest) else Modifier),
                contentAlignment = Alignment.Center,
            ) { Icon(icon, contentDescription = null, tint = tint) }
        }
        Column(Modifier.weight(1f)) {
            Text(headline, style = MaterialTheme.typography.bodyLarge, color = fg, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.alpha(dim))
            if (supporting != null) {
                Text(supporting, style = if (mono) MonoMedium else MaterialTheme.typography.bodyMedium.tabular, color = fgVariant, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.alpha(dim))
            }
            if (!enabled && disabledReason != null) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s4)) {
                    Icon(Sym.Block, contentDescription = null, tint = cs.onSurfaceVariant, modifier = Modifier.size(14.dp))
                    Text(disabledReason, style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant)
                }
            }
        }
        if (meta != null) Text(meta, style = MaterialTheme.typography.bodySmall.tabular, color = fgVariant, textAlign = TextAlign.End, maxLines = 2)
        if (trailing != null) trailing()
        if (chevron && enabled) Icon(Sym.ChevronRight, contentDescription = null, tint = cs.onSurfaceVariant)
    }
}

// ------------------------------------------------------------------ dry-run rows

enum class ChangeAction(@StringRes val labelRes: Int) {
    Upload(R.string.common_to_drive), Download(R.string.common_to_phone), Bin(R.string.components_action_to_bin),
    Conflict(R.string.common_conflict), Skip(R.string.common_skipped), Rename(R.string.components_action_move);

    val label: String get() = Strings.get(labelRes)

    val icon: ImageVector
        get() = when (this) {
            Upload -> Sym.ArrowUpward; Download -> Sym.ArrowDownward; Bin -> Sym.Delete
            Conflict -> Sym.CallSplit; Skip -> Sym.DoNotDisturbOn; Rename -> Sym.DriveFileMove
        }

    @Composable
    fun color(): Color = when (this) {
        Upload -> MaterialTheme.colorScheme.primary
        Download -> MaterialTheme.colorScheme.tertiary
        Bin -> Ds.status.approval.content
        Conflict -> Ds.status.conflict.content
        Skip, Rename -> MaterialTheme.colorScheme.onSurfaceVariant
    }
}

/** One planned or completed action. The action is a word as well as an arrow. */
@Composable
fun ChangeRow(action: ChangeAction, name: String, path: String, modifier: Modifier = Modifier, kind: FileKind = FileKind.of(name), size: String? = null, reason: String? = null, onClick: (() -> Unit)? = null) {
    val cs = MaterialTheme.colorScheme
    Row(
        modifier.fillMaxWidth().then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier).heightIn(min = 72.dp).padding(horizontal = Space.s16, vertical = Space.s8),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.s12),
    ) {
        Box(Modifier.size(40.dp).clip(MaterialTheme.shapes.small).background(cs.surfaceContainerHighest), contentAlignment = Alignment.Center) {
            Icon(kind.icon, contentDescription = null, tint = cs.onSurfaceVariant, modifier = Modifier.size(20.dp))
        }
        Column(Modifier.weight(1f)) {
            Text(name, style = MaterialTheme.typography.bodyLarge, color = cs.onSurface, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(
                buildAnnotatedString {
                    if (reason != null) append("$reason · ")
                    withStyle(MonoSmall.toSpanStyle()) { append(path.ifEmpty { "/" }) }
                },
                style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis,
            )
        }
        Column(horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(Space.s2)) {
            val color = action.color()
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s4)) {
                Icon(action.icon, contentDescription = null, tint = color, modifier = Modifier.size(16.dp))
                Text(stringResource(action.labelRes), style = MaterialTheme.typography.labelMedium, color = color)
            }
            if (size != null) Text(size, style = MaterialTheme.typography.bodySmall.tabular, color = cs.onSurfaceVariant)
        }
    }
}

@Immutable
data class SummaryItem(val action: ChangeAction, val count: Int, val size: String? = null, val label: String? = null)

/** The dry-run headline: what stays untouched first (the reassurance), then four tiles. Zero tiles stay, dimmed. */
@Composable
fun ChangeSummary(unchanged: Int, items: List<SummaryItem>, modifier: Modifier = Modifier) {
    val cs = MaterialTheme.colorScheme
    Column(modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(Space.s16)) {
        Column {
            Text(Fmt.count(unchanged), style = MaterialTheme.typography.displaySmall.tabular, color = cs.onSurface)
            Text(pluralStringResource(R.plurals.components_unchanged_files, unchanged), style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant)
        }
        items.chunked(2).forEach { row ->
            Row(Modifier.fillMaxWidth().height(IntrinsicSize.Min), horizontalArrangement = Arrangement.spacedBy(Space.s8)) {
                row.forEach { SummaryTile(it, Modifier.weight(1f).fillMaxHeight()) }
                if (row.size == 1) Spacer(Modifier.weight(1f))
            }
        }
    }
}

@Composable
private fun SummaryTile(item: SummaryItem, modifier: Modifier) {
    val cs = MaterialTheme.colorScheme
    val zero = item.count == 0
    val label = item.label ?: stringResource(item.action.labelRes)
    val a11y = stringResource(R.string.components_summary_a11y, Fmt.count(item.count), label) + (item.size?.let { ", $it" } ?: "")
    Row(
        modifier.clip(MaterialTheme.shapes.medium).background(Ds.lab.card).padding(Space.s12)
            .testTag("summary_${item.action.name.lowercase()}")
            .semantics(mergeDescendants = true) { contentDescription = a11y },
        horizontalArrangement = Arrangement.spacedBy(Space.s8),
    ) {
        Box(Modifier.size(32.dp).clip(CircleShape).background(cs.surfaceContainerHighest), contentAlignment = Alignment.Center) {
            Icon(item.action.icon, contentDescription = null, tint = item.action.color(), modifier = Modifier.size(18.dp))
        }
        Column {
            Text(Fmt.count(item.count), style = MaterialTheme.typography.titleLarge.tabular.copy(fontWeight = if (zero) FontWeight(400) else FontWeight(600)), color = if (zero) cs.onSurfaceVariant else cs.onSurface)
            Text(label, style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant)
            if (item.size != null) Text(item.size, style = MaterialTheme.typography.bodySmall.tabular, color = cs.onSurfaceVariant)
        }
    }
}

// ------------------------------------------------------------------ conflicts

@Immutable
data class ConflictSideInfo(val whenText: String, val meta: String)

enum class ConflictChoice { KeepBoth, KeepPhone, KeepDrive }

/** Both copies side by side, with the three safe choices. Keep both is the default and first. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun ConflictCard(
    name: String,
    path: String,
    phone: ConflictSideInfo,
    drive: ConflictSideInfo,
    modifier: Modifier = Modifier,
    kind: FileKind = FileKind.of(name),
    chosen: ConflictChoice? = null,
    onChoose: ((ConflictChoice) -> Unit)? = null,
) {
    val cs = MaterialTheme.colorScheme
    Column(
        modifier.fillMaxWidth().clip(MaterialTheme.shapes.large).background(Ds.lab.card).padding(Space.s16),
        verticalArrangement = Arrangement.spacedBy(Space.s12),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s12)) {
            Box(Modifier.size(40.dp).clip(MaterialTheme.shapes.small).background(cs.surfaceContainerHighest), contentAlignment = Alignment.Center) {
                Icon(kind.icon, contentDescription = null, tint = cs.onSurfaceVariant, modifier = Modifier.size(Size.iconRow))
            }
            Column(Modifier.weight(1f)) {
                Text(name, style = MaterialTheme.typography.titleMedium, color = cs.onSurface, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(path, style = MonoSmall, color = cs.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        StatusChip(SyncStatus.Conflict, label = stringResource(R.string.components_conflict_changed_both), size = ChipSize.Sm, modifier = Modifier.offset(x = 52.dp, y = (-4).dp))
        Row(Modifier.fillMaxWidth().height(IntrinsicSize.Min), horizontalArrangement = Arrangement.spacedBy(Space.s8)) {
            ConflictSideBox(Side.Phone, stringResource(R.string.components_conflict_on_phone), phone, chosen == ConflictChoice.KeepPhone, Modifier.weight(1f).fillMaxHeight())
            ConflictSideBox(Side.Drive, stringResource(R.string.components_conflict_on_drive), drive, chosen == ConflictChoice.KeepDrive, Modifier.weight(1f).fillMaxHeight())
        }
        if (onChoose != null) {
            FlowRow(horizontalArrangement = Arrangement.spacedBy(Space.s8), verticalArrangement = Arrangement.spacedBy(Space.s8)) {
                FilledTonalButton(onClick = { onChoose(ConflictChoice.KeepBoth) }, modifier = Modifier.heightIn(min = Size.buttonCompact)) { Text(stringResource(R.string.common_keep_both)) }
                OutlinedButton(onClick = { onChoose(ConflictChoice.KeepPhone) }, modifier = Modifier.heightIn(min = Size.buttonCompact)) { Text(stringResource(R.string.components_conflict_keep_phone)) }
                OutlinedButton(onClick = { onChoose(ConflictChoice.KeepDrive) }, modifier = Modifier.heightIn(min = Size.buttonCompact)) { Text(stringResource(R.string.components_conflict_keep_drive)) }
            }
            Text(stringResource(R.string.components_conflict_bin_note), style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant)
        }
    }
}

@Composable
private fun ConflictSideBox(side: Side, over: String, info: ConflictSideInfo, chosen: Boolean, modifier: Modifier) {
    val cs = MaterialTheme.colorScheme
    Column(
        modifier.clip(MaterialTheme.shapes.medium).background(Ds.lab.fill)
            .then(if (chosen) Modifier.border(2.dp, cs.primary, MaterialTheme.shapes.medium) else Modifier)
            .padding(Space.s12),
        verticalArrangement = Arrangement.spacedBy(Space.s2),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s4)) {
            Icon(side.icon, contentDescription = null, tint = cs.onSurfaceVariant, modifier = Modifier.size(16.dp))
            Text(over.uppercase(), style = MaterialTheme.typography.labelSmall, color = cs.onSurfaceVariant)
        }
        Text(info.whenText, style = MaterialTheme.typography.titleSmall, color = cs.onSurface)
        Text(info.meta, style = MaterialTheme.typography.bodySmall.tabular, color = cs.onSurfaceVariant)
    }
}

// ------------------------------------------------------------------ version history

enum class VersionSource(@StringRes val labelRes: Int) {
    Phone(R.string.components_version_edited_phone), Drive(R.string.components_version_edited_drive),
    Conflict(R.string.components_version_conflict_copy), Restored(R.string.components_version_restored);

    val label: String get() = Strings.get(labelRes)

    val icon: ImageVector get() = when (this) { Phone -> Sym.Mobile; Drive -> Sym.Cloud; Conflict -> Sym.CallSplit; Restored -> Sym.History }
}

/** One entry of a file's timeline, newest first. The current version has the filled dot and no actions. */
@Composable
fun VersionItem(
    whenText: String,
    source: VersionSource,
    size: String,
    modifier: Modifier = Modifier,
    note: String? = null,
    current: Boolean = false,
    first: Boolean = false,
    last: Boolean = false,
    onRestore: (() -> Unit)? = null,
    onSaveCopy: (() -> Unit)? = null,
) {
    val cs = MaterialTheme.colorScheme
    Row(modifier.fillMaxWidth().height(IntrinsicSize.Min).padding(horizontal = Space.s16), horizontalArrangement = Arrangement.spacedBy(Space.s12)) {
        val rail = cs.outlineVariant
        val dotFill = if (current) cs.primary else cs.surface
        val dotStroke = if (current) cs.primary else cs.outline
        Canvas(Modifier.width(24.dp).fillMaxHeight()) {
            val x = this.size.width / 2
            val top = if (first) 14.dp.toPx() else 0f
            val bottom = if (last) 14.dp.toPx() else this.size.height
            if (bottom > top) drawLine(rail, Offset(x, top), Offset(x, bottom), strokeWidth = 2.dp.toPx())
            val r = (if (current) 7.dp else 6.dp).toPx()
            val cy = (if (current) 7.dp else 8.dp).toPx() + r
            drawCircle(dotFill, r, Offset(x, cy))
            drawCircle(dotStroke, r - 1.dp.toPx(), Offset(x, cy), style = Stroke(2.dp.toPx()))
        }
        Column(Modifier.weight(1f).padding(top = Space.s4, bottom = Space.s16)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s8)) {
                Text(whenText, style = MaterialTheme.typography.titleMedium, color = cs.onSurface)
                if (current) Tag(stringResource(R.string.components_version_current))
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s4)) {
                Icon(source.icon, contentDescription = null, tint = cs.onSurfaceVariant, modifier = Modifier.size(16.dp))
                Text("${stringResource(source.labelRes)} · $size" + (note?.let { " · $it" } ?: ""), style = MaterialTheme.typography.bodyMedium.tabular, color = cs.onSurfaceVariant)
            }
            if (!current && (onRestore != null || onSaveCopy != null)) {
                Row(Modifier.offset(x = (-8).dp).padding(top = Space.s4), horizontalArrangement = Arrangement.spacedBy(Space.s4)) {
                    if (onRestore != null) TextButton(onClick = onRestore, modifier = Modifier.heightIn(min = Size.buttonCompact)) {
                        Icon(Sym.History, contentDescription = null, modifier = Modifier.size(18.dp)); Spacer(Modifier.size(Space.s8)); Text(stringResource(R.string.common_restore))
                    }
                    if (onSaveCopy != null) TextButton(onClick = onSaveCopy, modifier = Modifier.heightIn(min = Size.buttonCompact)) {
                        Icon(Sym.ContentCopy, contentDescription = null, modifier = Modifier.size(18.dp)); Spacer(Modifier.size(Space.s8)); Text(stringResource(R.string.components_version_save_copy))
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ diagnostics

enum class HealthState(@StringRes val wordRes: Int) {
    Ok(R.string.common_ok), Warn(R.string.components_health_check), Fail(R.string.components_health_problem), Wait(R.string.components_health_waiting);

    val word: String get() = Strings.get(wordRes)
}

/** One check, its finding in plain words, and the one fix. Only failing or warning rows get an action. */
@Composable
fun HealthCheckRow(state: HealthState, title: String, detail: String, modifier: Modifier = Modifier, actionLabel: String? = null, onAction: (() -> Unit)? = null) {
    val st = Ds.status
    val (icon, tint) = when (state) {
        HealthState.Ok -> Sym.CheckCircleFill to st.synced.content
        HealthState.Warn -> Sym.WarningFill to st.approval.content
        HealthState.Fail -> Sym.ErrorFill to st.failed.content
        HealthState.Wait -> Sym.ScheduleFill to st.waiting.content
    }
    Row(
        modifier.fillMaxWidth().padding(horizontal = Space.s16, vertical = Space.s12),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.s16),
    ) {
        Icon(icon, contentDescription = stringResource(state.wordRes), tint = tint, modifier = Modifier.size(Size.iconRow))
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurface)
            Text(detail, style = MaterialTheme.typography.bodyMedium.tabular, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        if (actionLabel != null && onAction != null && state != HealthState.Ok) {
            FilledTonalButton(onClick = onAction, modifier = Modifier.heightIn(min = Size.buttonCompact)) { Text(actionLabel) }
        }
    }
}

// ------------------------------------------------------------------ log

enum class LogLevel { OK, INFO, WARN, ERROR }

@Immutable
data class LogLine(val time: String, val level: LogLevel, val message: String, val detail: String? = null)

/** One log line: time, level as a word, message (truncated; tap to see it in full). */
@Composable
fun LogLineRow(line: LogLine, modifier: Modifier = Modifier, onClick: (() -> Unit)? = null) {
    val cs = MaterialTheme.colorScheme
    val levelColor = when (line.level) {
        LogLevel.OK -> Ds.status.synced.content
        LogLevel.WARN -> Ds.status.approval.content
        LogLevel.ERROR -> Ds.status.failed.content
        LogLevel.INFO -> cs.onSurfaceVariant
    }
    Row(modifier.fillMaxWidth().then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier), horizontalArrangement = Arrangement.spacedBy(Space.s4)) {
        Text(line.time, style = MonoSmall, color = cs.onSurfaceVariant, modifier = Modifier.width(58.dp), maxLines = 1)
        Text(line.level.name, style = MonoSmall, color = levelColor, modifier = Modifier.width(44.dp), maxLines = 1)
        Text(line.message, style = MonoSmall, color = cs.onSurface, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
    }
}

/** The sync log on surface-container-highest. For long logs, put [LogLineRow]s in a LazyColumn instead. */
@Composable
fun LogView(lines: List<LogLine>, modifier: Modifier = Modifier, onLineClick: ((LogLine) -> Unit)? = null) {
    Column(
        modifier.fillMaxWidth().padding(horizontal = Space.s16).clip(MaterialTheme.shapes.medium).background(MaterialTheme.colorScheme.surfaceContainerHighest).padding(Space.s12),
        verticalArrangement = Arrangement.spacedBy(Space.s2),
    ) {
        if (lines.isEmpty()) Text(stringResource(R.string.components_log_empty), style = MonoSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        lines.forEach { l -> LogLineRow(l, onClick = onLineClick?.let { { it(l) } }) }
    }
}
