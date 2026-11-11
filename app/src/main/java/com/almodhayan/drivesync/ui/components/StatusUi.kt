package com.almodhayan.drivesync.ui.components

import com.almodhayan.drivesync.ui.theme.Size
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.ui.theme.Ds
import com.almodhayan.drivesync.ui.theme.Motion
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.SyncStatus
import com.almodhayan.drivesync.ui.theme.tabular

/** A labelled action: buttons in banners, callouts and cards. */
@Immutable
data class UiAction(val label: String, val icon: ImageVector? = null, val onClick: () -> Unit)

enum class ChipVariant { Auto, Tonal, Plain, Icon }
enum class ChipSize { Md, Sm }

/** Fixed label for a state, with the counted and percentage forms the status system allows. */
fun statusLabel(status: SyncStatus, progress: Float? = null, count: Int = 0, label: String? = null): String = label ?: when {
    status == SyncStatus.Syncing && progress != null -> Strings.get(R.string.components_status_syncing_percent, Fmt.percent(progress))
    status == SyncStatus.Conflict && count > 1 -> Strings.plural(R.plurals.components_status_conflicts, count, count)
    status == SyncStatus.Failed && count > 1 -> Strings.plural(R.plurals.components_status_failed, count, count)
    else -> status.label
}

/** The status glyph in its colour. Syncing turns once per 2s (stands still when animations are off). */
@Composable
fun StatusIcon(status: SyncStatus, modifier: Modifier = Modifier, size: Dp = 20.dp, icon: ImageVector? = null, contentDescription: String? = null) {
    val tint = Ds.status.of(status).content
    val glyph = icon ?: status.icon
    val spin = status == SyncStatus.Syncing && icon == null && !Ds.reduceMotion
    val rotation = if (spin) {
        val t = rememberInfiniteTransition(label = "sync-turn")
        t.animateFloat(0f, -360f, infiniteRepeatable(tween(Motion.SyncTurn, easing = LinearEasing)), label = "sync-turn").value
    } else 0f
    Icon(glyph, contentDescription = contentDescription, tint = tint, modifier = modifier.size(size).rotate(rotation))
}

/**
 * The single visual for a sync state (Modhayan Labs › StatusChip): tone + filled icon + label in a capsule,
 * 30dp (24dp compact). Plain is kept for inline text; icon-only is for dense rows and always carries the
 * full label as its content description.
 */
@Composable
fun StatusChip(
    status: SyncStatus,
    modifier: Modifier = Modifier,
    label: String? = null,
    progress: Float? = null,
    count: Int = 0,
    variant: ChipVariant = ChipVariant.Auto,
    size: ChipSize = ChipSize.Md,
    icon: ImageVector? = null,
) {
    val text = statusLabel(status, progress, count, label)
    val colors = Ds.status.of(status)
    val v = if (variant == ChipVariant.Auto) ChipVariant.Tonal else variant
    val iconSize = if (size == ChipSize.Sm) 16.dp else 18.dp
    when (v) {
        ChipVariant.Icon -> StatusIcon(status, modifier.semantics { contentDescription = text }, size = if (size == ChipSize.Sm) 18.dp else 20.dp, icon = icon)
        ChipVariant.Plain -> Row(
            modifier.semantics(mergeDescendants = true) { liveRegion = LiveRegionMode.Polite },
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            StatusIcon(status, size = iconSize, icon = icon)
            Text(text, style = MaterialTheme.typography.labelMedium.tabular, color = colors.content, maxLines = 1)
        }
        else -> Surface(modifier.semantics(mergeDescendants = true) { liveRegion = LiveRegionMode.Polite }, shape = CircleShape, color = colors.container) {
            Row(
                Modifier.height(if (size == ChipSize.Sm) 24.dp else 30.dp).padding(start = if (size == ChipSize.Sm) 8.dp else 10.dp, end = if (size == ChipSize.Sm) 10.dp else 12.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(if (size == ChipSize.Sm) 4.dp else 6.dp),
            ) {
                StatusIcon(status, size = iconSize, icon = icon)
                Text(text, style = MaterialTheme.typography.labelMedium.tabular, color = colors.onContainer, maxLines = 1)
            }
        }
    }
}

/**
 * One per screen, for the single most urgent state: what happened to the files, what is safe, what to do.
 * The filled button inverts to on-status-container on status-container.
 */
@Composable
fun StatusBanner(
    status: SyncStatus,
    title: String,
    modifier: Modifier = Modifier,
    body: String? = null,
    primary: UiAction? = null,
    secondary: UiAction? = null,
    icon: ImageVector? = null,
) {
    val c = Ds.status.of(status)
    Surface(
        modifier.fillMaxWidth().semantics(mergeDescendants = false) { liveRegion = if (status == SyncStatus.Failed) LiveRegionMode.Assertive else LiveRegionMode.Polite },
        shape = MaterialTheme.shapes.medium,
        color = c.container,
        contentColor = c.onContainer,
    ) {
        Row(Modifier.padding(Space.s16), horizontalArrangement = Arrangement.spacedBy(Space.s12)) {
            StatusIcon(status, Modifier.padding(top = 1.dp), size = 24.dp, icon = icon)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(Space.s4)) {
                Text(title, style = MaterialTheme.typography.titleMedium, color = c.onContainer)
                if (body != null) Text(body, style = MaterialTheme.typography.bodyMedium, color = c.onContainer)
                if (primary != null || secondary != null) {
                    Row(Modifier.padding(top = Space.s8), horizontalArrangement = Arrangement.spacedBy(Space.s8), verticalAlignment = Alignment.CenterVertically) {
                        if (primary != null) {
                            Button(
                                onClick = primary.onClick,
                                colors = ButtonDefaults.buttonColors(containerColor = c.onContainer, contentColor = c.container),
                                contentPadding = ButtonDefaults.SmallButtonContentPadding(primary.icon != null),
                                modifier = Modifier.heightIn(min = Size.buttonCompact),
                            ) { ButtonContent(primary) }
                        }
                        if (secondary != null) {
                            TextButton(onClick = secondary.onClick, colors = ButtonDefaults.textButtonColors(contentColor = c.onContainer), modifier = Modifier.heightIn(min = Size.buttonCompact)) { ButtonContent(secondary) }
                        }
                    }
                }
            }
        }
    }
}

/** Icon (18dp) + label, for buttons built from a [UiAction]. */
@Composable
fun RowScope.ButtonContent(action: UiAction) {
    if (action.icon != null) {
        Icon(action.icon, contentDescription = null, modifier = Modifier.size(18.dp))
        androidx.compose.foundation.layout.Spacer(Modifier.size(Space.s8))
    }
    Text(action.label, maxLines = 1, overflow = TextOverflow.Ellipsis)
}

@Suppress("FunctionName")
private fun ButtonDefaults.SmallButtonContentPadding(hasIcon: Boolean) =
    androidx.compose.foundation.layout.PaddingValues(start = if (hasIcon) 12.dp else 16.dp, end = 16.dp, top = 0.dp, bottom = 0.dp)

/** Hides a decorative element from accessibility. */
fun Modifier.decorative(): Modifier = clearAndSetSemantics { }
