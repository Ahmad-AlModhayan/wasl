@file:OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)

package com.almodhayan.drivesync.ui.components

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedCard
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.annotation.StringRes
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.db.SyncMode
import com.almodhayan.drivesync.ui.theme.Ds
import com.almodhayan.drivesync.ui.theme.MonoMedium
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import com.almodhayan.drivesync.ui.theme.SyncStatus
import com.almodhayan.drivesync.ui.theme.tabular

/** The two sides are always "phone" and "Drive". */
enum class Side(@StringRes val labelRes: Int) {
    Phone(R.string.common_phone), Drive(R.string.common_drive);
    val label: String get() = Strings.get(labelRes)
    val icon: ImageVector get() = if (this == Phone) Sym.Mobile else Sym.Cloud
}

/** The three directions the UI offers. Mirror/move variants are options of the one-way directions. */
enum class PairDirection(@StringRes val titleRes: Int) {
    TwoWay(R.string.components_direction_two_way), PhoneToDrive(R.string.components_direction_phone_to_drive), DriveToPhone(R.string.components_direction_drive_to_phone);

    val title: String get() = Strings.get(titleRes)

    val modeIcon: ImageVector get() = when (this) { TwoWay -> Sym.SyncAlt; PhoneToDrive -> Sym.CloudUpload; DriveToPhone -> Sym.CloudDownload }
    val arrow: ImageVector get() = when (this) { TwoWay -> Sym.SwapHoriz; PhoneToDrive -> Sym.ArrowForward; DriveToPhone -> Sym.ArrowBack }

    companion object {
        fun of(mode: SyncMode): PairDirection = when (mode) {
            SyncMode.TWO_WAY -> TwoWay
            SyncMode.UPLOAD_ONLY, SyncMode.UPLOAD_THEN_DELETE, SyncMode.UPLOAD_MIRROR -> PhoneToDrive
            SyncMode.DOWNLOAD_ONLY, SyncMode.DOWNLOAD_THEN_DELETE, SyncMode.DOWNLOAD_MIRROR -> DriveToPhone
        }
    }
}

// ------------------------------------------------------------------ StorageBar

@Immutable
data class StorageModel(val side: Side, val total: Long, val pair: Long, val other: Long, val label: String? = null, val pairLabel: String = Strings.get(R.string.components_storage_this_pair))

/**
 * How full one side is and how much of it this pair uses: three segments split by 2dp gaps, and the
 * header always states free of total in words. Below 10% free the bar turns to the low-space colour.
 */
@Composable
fun StorageBar(model: StorageModel, modifier: Modifier = Modifier, compact: Boolean = false) {
    val total = model.total.coerceAtLeast(1)
    val pair = model.pair.coerceIn(0, total)
    val other = model.other.coerceIn(0, total - pair)
    val free = (total - pair - other).coerceAtLeast(0)
    val low = free.toDouble() / total < 0.10
    val name = model.label ?: stringResource(model.side.labelRes)
    val cs = MaterialTheme.colorScheme
    val pairColor = if (low) Ds.status.approval.content else cs.primary
    val otherColor = if (low) Ds.status.approval.content.copy(alpha = 0.55f) else cs.outline
    val freeColor = cs.surfaceContainerHighest
    val a11y = stringResource(R.string.components_storage_a11y, name, model.pairLabel.lowercase(), Fmt.bytes(pair), Fmt.bytes(other), Fmt.bytes(free), Fmt.bytes(total))
    Column(modifier.fillMaxWidth().clearAndSetSemantics { contentDescription = a11y }, verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Icon(model.side.icon, contentDescription = null, tint = cs.onSurfaceVariant, modifier = Modifier.size(16.dp))
            Spacer(Modifier.size(Space.s4))
            Text(name, style = MaterialTheme.typography.bodySmall.copy(fontWeight = FontWeight(600)), color = cs.onSurface)
            Spacer(Modifier.weight(1f))
            if (low) {
                Icon(Sym.WarningFill, contentDescription = null, tint = Ds.status.approval.content, modifier = Modifier.size(14.dp))
                Spacer(Modifier.size(Space.s2))
                Text(stringResource(R.string.components_storage_low_space), style = MaterialTheme.typography.bodySmall.copy(fontWeight = FontWeight(600)), color = Ds.status.approval.content)
                // Without a gap "Low space" ran straight into "x free of y".
                Spacer(Modifier.size(Space.s8))
            }
            Text(stringResource(R.string.components_storage_free_of, Fmt.bytes(free), Fmt.bytes(total)), style = MaterialTheme.typography.bodySmall.tabular, color = cs.onSurfaceVariant)
        }
        Canvas(Modifier.fillMaxWidth().height(if (compact) 6.dp else 8.dp).clip(RoundedCornerShape(50))) {
            val gap = 2.dp.toPx()
            val minSeg = 3.dp.toPx()
            val w = size.width
            var pairW = if (pair > 0) maxOf(minSeg, (w * pair / total).toFloat()) else 0f
            var otherW = if (other > 0) maxOf(minSeg, (w * other / total).toFloat()) else 0f
            val gaps = (if (pairW > 0) gap else 0f) + (if (otherW > 0) gap else 0f)
            val overflow = pairW + otherW + gaps - w
            if (overflow > 0) { val scale = (w - gaps) / (pairW + otherW); pairW *= scale; otherW *= scale }
            var x = 0f
            if (pairW > 0) { drawRect(pairColor, Offset(x, 0f), Size(pairW, size.height)); x += pairW + gap }
            if (otherW > 0) { drawRect(otherColor, Offset(x, 0f), Size(otherW, size.height)); x += otherW + gap }
            if (x < w) drawRoundRect(freeColor, Offset(x, 0f), Size(w - x, size.height), cornerRadius = CornerRadius(size.height / 2))
        }
        if (!compact) {
            FlowRow(horizontalArrangement = Arrangement.spacedBy(Space.s16), verticalArrangement = Arrangement.spacedBy(Space.s4)) {
                LegendKey(pairColor, stringResource(R.string.components_storage_legend_pair, model.pairLabel, Fmt.bytes(pair)))
                LegendKey(otherColor, stringResource(R.string.components_storage_legend_other, Fmt.bytes(other)))
                LegendKey(freeColor, stringResource(R.string.components_storage_legend_free, Fmt.bytes(free)))
            }
        }
    }
}

@Composable
private fun LegendKey(color: Color, text: String) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Box(Modifier.size(8.dp).clip(RoundedCornerShape(2.dp)).background(color))
        Text(text, style = MaterialTheme.typography.bodySmall.tabular, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

// ------------------------------------------------------------------ PathRow

/**
 * One side of a pair: which side, which folder, how much is in it. Both sides share one quiet grey
 * circle behind the icon; they differ by icon and label, not by accent colour (0.4.1).
 */
@Composable
fun PathRow(
    side: Side,
    path: String?,
    modifier: Modifier = Modifier,
    meta: String? = null,
    error: String? = null,
    dense: Boolean = false,
    mono: Boolean = false,
    onPick: (() -> Unit)? = null,
) {
    val cs = MaterialTheme.colorScheme
    val iconBg = Ds.lab.fill
    val iconFg = cs.onSurfaceVariant
    val pickLabel = stringResource(if (side == Side.Phone) R.string.components_choose_phone_folder else R.string.components_choose_drive_folder)
    val base = modifier.testTag("path_${side.name.lowercase()}").fillMaxWidth().clip(MaterialTheme.shapes.small)
        .then(if (dense) Modifier else Modifier.background(cs.surfaceContainerLowest))
        .then(if (onPick != null) Modifier.clickable(onClickLabel = pickLabel, onClick = onPick) else Modifier)
        .padding(horizontal = Space.s12, vertical = if (dense) Space.s8 else Space.s12)
    Row(base, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s12)) {
        Box(Modifier.size(if (dense) 28.dp else 36.dp).clip(CircleShape).background(iconBg), contentAlignment = Alignment.Center) {
            Icon(side.icon, contentDescription = null, tint = iconFg, modifier = Modifier.size(if (dense) 18.dp else 20.dp))
        }
        Column(Modifier.weight(1f)) {
            Text(stringResource(side.labelRes), style = MaterialTheme.typography.labelSmall, color = cs.onSurfaceVariant)
            if (path.isNullOrEmpty()) {
                Text(stringResource(R.string.components_choose_a_folder), style = MaterialTheme.typography.bodyMedium.copy(fontWeight = FontWeight(600)), color = cs.primary)
            } else {
                Text(path, style = if (mono) MonoMedium else MaterialTheme.typography.bodyMedium, color = cs.onSurface, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            if (meta != null) Text(meta, style = MaterialTheme.typography.bodySmall.tabular, color = cs.onSurfaceVariant, modifier = Modifier.testTag("path_${side.name.lowercase()}_meta"))
            if (error != null) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    Icon(Sym.ErrorFill, contentDescription = null, tint = cs.error, modifier = Modifier.size(14.dp))
                    Text(error, style = MaterialTheme.typography.bodySmall, color = cs.error)
                }
            }
        }
        if (onPick != null) Icon(Sym.ChevronRight, contentDescription = null, tint = cs.onSurfaceVariant)
    }
}

// ------------------------------------------------------------------ FolderPairCard

@Immutable
data class MenuAction(val label: String, val icon: ImageVector? = null, val destructive: Boolean = false, val onClick: () -> Unit)

@Immutable
data class PairCardModel(
    val name: String,
    val direction: PairDirection,
    val status: SyncStatus,
    val statusLabel: String? = null,
    val statusIcon: ImageVector? = null,
    val conditions: String? = null,
    val lastSync: String? = null,
    val phonePath: String,
    val drivePath: String,
    val progress: SyncProgressModel? = null,
    val storage: List<StorageModel> = emptyList(),
)

/**
 * The Home unit. Quiet states show the plain chip; loud states the tonal chip plus exactly one action.
 * Tap opens the pair; the overflow menu holds Sync now, Pause, Preview, Edit, Remove pair.
 */
@Composable
fun FolderPairCard(
    model: PairCardModel,
    modifier: Modifier = Modifier,
    onClick: (() -> Unit)? = null,
    menu: List<MenuAction> = emptyList(),
    action: UiAction? = null,
    onPause: (() -> Unit)? = null,
) {
    val cs = MaterialTheme.colorScheme
    val content: @Composable () -> Unit = {
        val directionTitle = stringResource(model.direction.titleRes)
        Column(Modifier.padding(Space.s16), verticalArrangement = Arrangement.spacedBy(Space.s12)) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.Top) {
                Column(Modifier.weight(1f).padding(top = Space.s2)) {
                    Text(model.name, style = MaterialTheme.typography.titleMedium, color = cs.onSurface, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(
                        directionTitle + (model.conditions?.let { " · $it" } ?: ""),
                        style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis,
                    )
                }
                // The 48dp button's own inset would push the dots inwards; pull it to the card's edge like the design.
                if (menu.isNotEmpty()) OverflowMenu(menu, contentDescription = stringResource(R.string.components_more_for, model.name), modifier = Modifier.offset(x = 12.dp, y = (-8).dp))
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s8)) {
                StatusChip(
                    model.status,
                    label = model.statusLabel,
                    progress = if (model.progress == null) null else model.progress.value,
                    icon = model.statusIcon,
                )
                if (model.lastSync != null) Text(model.lastSync, style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant, maxLines = 1)
            }
            if (model.progress != null) SyncProgress(model.progress, onPause = onPause)
            // The two folders sit on the card itself: no grey panel inside the card (0.4.1).
            Box(Modifier.fillMaxWidth()) {
                Column {
                    PathRow(Side.Phone, model.phonePath, dense = true)
                    PathRow(Side.Drive, model.drivePath, dense = true)
                }
                Box(
                    // Centred under the 28dp side icons (12dp row inset + 14dp radius − 10dp).
                    Modifier.align(Alignment.CenterStart).offset(x = 16.dp).size(20.dp).clip(CircleShape).background(Ds.lab.card)
                        .semantics { contentDescription = directionTitle },
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(model.direction.arrow, contentDescription = null, tint = cs.onSurfaceVariant, modifier = Modifier.size(18.dp).rotate(90f))
                }
            }
            if (model.storage.isNotEmpty()) {
                Column(verticalArrangement = Arrangement.spacedBy(Space.s12)) { model.storage.forEach { StorageBar(it, compact = true) } }
            }
            if (action != null) {
                Row(horizontalArrangement = Arrangement.spacedBy(Space.s8)) {
                    FilledTonalButton(onClick = action.onClick, modifier = Modifier.testTag("pair_card_action")) { ButtonContent(action) }
                }
            }
        }
    }
    // A white card on the grey page: no border, no shadow (Modhayan Labs › Card).
    val colors = CardDefaults.cardColors(containerColor = Ds.lab.card)
    if (onClick != null) Card(onClick = onClick, modifier = modifier.fillMaxWidth(), shape = MaterialTheme.shapes.large, colors = colors) { content() }
    else Card(modifier = modifier.fillMaxWidth(), shape = MaterialTheme.shapes.large, colors = colors) { content() }
}

/** The ⋮ button with its menu. */
@Composable
fun OverflowMenu(items: List<MenuAction>, modifier: Modifier = Modifier, contentDescription: String = stringResource(R.string.components_more_options)) {
    var open by remember { mutableStateOf(false) }
    Box(modifier) {
        IconButton(onClick = { open = true }) { Icon(Sym.MoreVert, contentDescription = contentDescription) }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            items.forEach { item ->
                DropdownMenuItem(
                    text = { Text(item.label, color = if (item.destructive) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurface) },
                    leadingIcon = item.icon?.let { ic -> { Icon(ic, contentDescription = null, tint = if (item.destructive) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant) } },
                    onClick = { open = false; item.onClick() },
                )
            }
        }
    }
}

// ------------------------------------------------------------------ SyncModeOption

/**
 * A radio card for one direction, with what it copies and what it never touches. Always show all three,
 * in order: Two-way (recommended), Phone → Drive, Drive → Phone.
 */
@Composable
fun SyncModeOption(direction: PairDirection, selected: Boolean, description: String, onSelect: () -> Unit, modifier: Modifier = Modifier, recommended: Boolean = false) {
    val cs = MaterialTheme.colorScheme
    val shape = MaterialTheme.shapes.large
    Row(
        modifier.fillMaxWidth().clip(shape)
            .background(if (selected) cs.primaryContainer else Ds.lab.card)
            .then(if (selected) Modifier.border(2.dp, cs.primary, shape) else Modifier)
            .selectable(selected = selected, role = Role.RadioButton, onClick = onSelect)
            .padding(Space.s16),
        horizontalArrangement = Arrangement.spacedBy(Space.s12),
        verticalAlignment = Alignment.Top,
    ) {
        Icon(if (selected) Sym.RadioButtonChecked else Sym.RadioButtonUnchecked, contentDescription = null, tint = if (selected) cs.primary else cs.onSurfaceVariant)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            FlowRow(horizontalArrangement = Arrangement.spacedBy(Space.s8), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(stringResource(direction.titleRes), style = MaterialTheme.typography.titleMedium, color = cs.onSurface, modifier = Modifier.align(Alignment.CenterVertically))
                if (recommended) {
                    Surface(shape = RoundedCornerShape(50), color = cs.primary, modifier = Modifier.align(Alignment.CenterVertically)) {
                        Text(stringResource(R.string.common_recommended), style = MaterialTheme.typography.labelSmall, color = cs.onPrimary, modifier = Modifier.padding(horizontal = Space.s8, vertical = Space.s2))
                    }
                }
            }
            Text(description, style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant)
        }
        Row(
            Modifier.clip(RoundedCornerShape(50)).background(Ds.lab.fill).padding(horizontal = 8.dp, vertical = 6.dp).clearAndSetSemantics { },
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            Icon(Sym.Mobile, contentDescription = null, tint = cs.primary, modifier = Modifier.size(20.dp))
            Icon(direction.arrow, contentDescription = null, tint = cs.onSurfaceVariant, modifier = Modifier.size(18.dp))
            Icon(Sym.Cloud, contentDescription = null, tint = cs.tertiary, modifier = Modifier.size(20.dp))
        }
    }
}

// ------------------------------------------------------------------ Callout

enum class CalloutTone { Info, Warning, Danger, Success }

/** Inline guidance under the setting that causes it: what will happen, in numbers, and the undo. */
@Composable
fun Callout(text: String, modifier: Modifier = Modifier, tone: CalloutTone = CalloutTone.Info, title: String? = null, action: UiAction? = null) {
    val cs = MaterialTheme.colorScheme
    val st = Ds.status
    val (bg, fg, iconTint, icon) = when (tone) {
        CalloutTone.Info -> Quad(st.syncing.container, st.syncing.onContainer, st.syncing.content, Sym.InfoFill)
        CalloutTone.Warning -> Quad(st.approval.container, st.approval.onContainer, st.approval.content, Sym.WarningFill)
        CalloutTone.Danger -> Quad(st.failed.container, st.failed.onContainer, st.failed.content, Sym.ErrorFill)
        CalloutTone.Success -> Quad(st.synced.container, st.synced.onContainer, st.synced.content, Sym.CheckCircleFill)
    }
    Surface(modifier.fillMaxWidth(), shape = MaterialTheme.shapes.medium, color = bg, contentColor = fg) {
        Row(Modifier.padding(horizontal = Space.s16, vertical = Space.s12), horizontalArrangement = Arrangement.spacedBy(Space.s12)) {
            Icon(icon, contentDescription = null, tint = iconTint, modifier = Modifier.padding(top = 2.dp).size(20.dp))
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                if (title != null) Text(title, style = MaterialTheme.typography.titleSmall, color = fg)
                Text(text, style = MaterialTheme.typography.bodyMedium, color = fg)
                if (action != null) {
                    TextButton(onClick = action.onClick, modifier = Modifier.offset(x = (-8).dp).heightIn(min = com.almodhayan.drivesync.ui.theme.Size.buttonCompact), colors = androidx.compose.material3.ButtonDefaults.textButtonColors(contentColor = fg)) { ButtonContent(action) }
                }
            }
        }
    }
}

private data class Quad(val bg: Color, val fg: Color, val tint: Color, val icon: ImageVector)
