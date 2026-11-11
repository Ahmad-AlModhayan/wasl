package com.almodhayan.drivesync.ui.components

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.snap
import androidx.compose.animation.core.tween
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.ui.theme.Ds
import com.almodhayan.drivesync.ui.theme.MonoSmall
import com.almodhayan.drivesync.ui.theme.Motion
import com.almodhayan.drivesync.ui.theme.Size
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import com.almodhayan.drivesync.ui.theme.tabular

/** Status-syncing coloured progress. Determinate values tween over 600ms and never run backwards. */
@Composable
fun DsLinearProgress(value: Float?, modifier: Modifier = Modifier) {
    val c = Ds.status.syncing
    if (value == null) {
        LinearProgressIndicator(modifier.fillMaxWidth().height(Size.progressTrack), color = c.content, trackColor = c.container, strokeCap = StrokeCap.Round)
        return
    }
    val high = remember { mutableFloatStateOf(0f) }
    val target = value.coerceIn(0f, 1f).let { if (it < high.floatValue && it > 0f) high.floatValue else it }
    high.floatValue = target
    val animated by animateFloatAsState(target, if (Ds.reduceMotion) snap() else tween(Motion.Progress, easing = Motion.Standard), label = "progress")
    LinearProgressIndicator(
        progress = { animated.coerceAtLeast(0.02f) },
        modifier = modifier.fillMaxWidth().height(Size.progressTrack),
        color = c.content,
        trackColor = c.container,
        strokeCap = StrokeCap.Round,
    )
}

enum class TransferDirection { Up, Down, Both }

@Immutable
data class SyncProgressModel(
    val scanning: Boolean,
    val value: Float? = null,
    val done: Int = 0,
    val total: Int = 0,
    val doneBytes: Long = 0,
    val totalBytes: Long = 0,
    val direction: TransferDirection = TransferDirection.Both,
    val current: String? = null,
    val eta: String? = null,
    val checked: Int = 0,
)

/**
 * Live progress: counts, bytes, current file, time left and Pause. Indeterminate only while
 * comparing folders, and it says so.
 */
@Composable
fun SyncProgress(model: SyncProgressModel, modifier: Modifier = Modifier, onPause: (() -> Unit)? = null) {
    val progressRes = when (model.direction) {
        TransferDirection.Up -> R.string.components_progress_uploading
        TransferDirection.Down -> R.string.components_progress_downloading
        TransferDirection.Both -> R.string.components_progress_syncing
    }
    val headline = when {
        !model.scanning -> stringResource(progressRes, Fmt.count(model.done), Fmt.count(model.total))
        model.checked > 0 -> pluralStringResource(R.plurals.components_progress_comparing_checked, model.checked, Fmt.count(model.checked))
        else -> stringResource(R.string.components_progress_comparing)
    }
    Column(modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(Space.s8)) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Space.s8)) {
            Text(
                headline,
                style = MaterialTheme.typography.bodyMedium.tabular,
                color = MaterialTheme.colorScheme.onSurface,
                modifier = Modifier.weight(1f),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (!model.scanning && model.totalBytes > 0) {
                Text(stringResource(R.string.components_progress_bytes, Fmt.bytes(model.doneBytes), Fmt.bytes(model.totalBytes)), style = MaterialTheme.typography.bodyMedium.tabular, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1)
            }
        }
        DsLinearProgress(if (model.scanning) null else model.value)
        if (model.current != null || model.eta != null || onPause != null) {
            Row(Modifier.fillMaxWidth().offset(y = (-4).dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s8)) {
                Text(
                    buildAnnotatedString {
                        // Time left first: a long file name ellipsizes at the end instead of hiding it.
                        if (model.eta != null) append(model.eta)
                        if (model.current != null && model.eta != null) append(" · ")
                        if (model.current != null) withStyle(MonoSmall.toSpanStyle()) { append(model.current) }
                    },
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                if (onPause != null) {
                    TextButton(onClick = onPause, modifier = Modifier.heightIn(min = Size.buttonCompact)) {
                        Icon(Sym.PauseFill, contentDescription = null, modifier = Modifier.size(18.dp))
                        androidx.compose.foundation.layout.Spacer(Modifier.size(Space.s8))
                        Text(stringResource(R.string.common_pause))
                    }
                }
            }
        }
    }
}
