package com.almodhayan.drivesync.ui.components

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.addPathNodes
import androidx.compose.ui.graphics.vector.rememberVectorPainter
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.ui.theme.Ds

// Geometry from docs/design/project/components/src/mark-paths.js (24-unit grid, round caps and joins).
private val MARK = listOf(
    "M4.40 17.80 A1.7 1.7 0 0 1 2.70 16.10 L2.70 7.70 A1.7 1.7 0 0 1 4.40 6.00 L7.05 6.00",
    "M13.60 6.00 A4.6 4.6 0 0 1 18.20 10.69 A3.6 3.6 0 0 1 17.40 17.80 L10.95 17.80",
    "M11.05 7.25 L6.45 2.60 L6.45 7.25 Z M6.95 16.55 L11.55 21.20 L11.55 16.55 Z",
)
private val NOTIF = listOf(
    "M4.40 17.80 A1.7 1.7 0 0 1 2.70 16.10 L2.70 7.70 A1.7 1.7 0 0 1 4.40 6.00 L6.80 6.00",
    "M13.60 6.00 A4.6 4.6 0 0 1 18.20 10.69 A3.6 3.6 0 0 1 17.40 17.80 L11.20 17.80",
    "M11.00 7.40 L6.20 2.40 L6.20 7.40 Z M7.00 16.40 L11.80 21.40 L11.80 16.40 Z",
)
private const val WORDMARK = "M3.29 -28H13.48Q16.21 -28 18.43 -27.12Q20.66 -26.23 22.24 -24.49Q23.83 -22.74 24.69 -20.12Q25.55 -17.49 25.55 -14Q25.55 -10.51 24.69 -7.88Q23.83 -5.26 22.24 -3.51Q20.66 -1.77 18.43 -0.88Q16.21 0 13.48 0H3.29ZM13.48 -4.69Q16.45 -4.69 18.19 -6.44Q19.94 -8.18 19.94 -11.79V-16.21Q19.94 -19.82 18.19 -21.56Q16.45 -23.31 13.48 -23.31H8.58V-4.69Z M30.61 0V-20.94H35.74V-16.61H35.94Q36.14 -17.45 36.56 -18.23Q36.99 -19.01 37.67 -19.62Q38.35 -20.22 39.29 -20.58Q40.23 -20.94 41.48 -20.94H42.6V-16.09H41Q38.39 -16.09 37.07 -15.32Q35.74 -14.56 35.74 -12.84V0Z M48.94 -23.99Q47.34 -23.99 46.63 -24.71Q45.93 -25.43 45.93 -26.56V-27.36Q45.93 -28.48 46.63 -29.2Q47.34 -29.93 48.94 -29.93Q50.5 -29.93 51.23 -29.2Q51.95 -28.48 51.95 -27.36V-26.56Q51.95 -25.43 51.23 -24.71Q50.5 -23.99 48.94 -23.99ZM46.37 -20.94H51.51V0H46.37Z M61.94 0 54.96 -20.94H60.01L62.94 -11.91L64.99 -4.45H65.27L67.31 -11.91L70.16 -20.94H75.05L68.03 0Z M86.61 0.48Q84.28 0.48 82.46 -0.3Q80.63 -1.08 79.37 -2.51Q78.1 -3.93 77.42 -5.96Q76.74 -7.98 76.74 -10.51Q76.74 -13 77.4 -15Q78.06 -17.01 79.31 -18.43Q80.55 -19.86 82.36 -20.64Q84.16 -21.42 86.45 -21.42Q88.89 -21.42 90.7 -20.58Q92.5 -19.74 93.67 -18.29Q94.83 -16.85 95.41 -14.94Q95.99 -13.04 95.99 -10.87V-9.19H82.07V-8.66Q82.07 -6.38 83.36 -4.99Q84.64 -3.61 87.17 -3.61Q89.09 -3.61 90.32 -4.41Q91.54 -5.21 92.5 -6.46L95.27 -3.37Q93.99 -1.56 91.76 -0.54Q89.54 0.48 86.61 0.48ZM86.53 -17.57Q84.48 -17.57 83.28 -16.21Q82.07 -14.84 82.07 -12.68V-12.36H90.66V-12.72Q90.66 -14.88 89.6 -16.23Q88.53 -17.57 86.53 -17.57Z M119.02 0.48Q115.41 0.48 112.9 -0.8Q110.4 -2.09 108.59 -4.17L112.16 -7.62Q113.6 -5.94 115.39 -5.05Q117.17 -4.17 119.34 -4.17Q121.79 -4.17 123.03 -5.23Q124.28 -6.3 124.28 -8.1Q124.28 -9.51 123.47 -10.39Q122.67 -11.27 120.46 -11.67L117.82 -12.07Q109.43 -13.4 109.43 -20.22Q109.43 -22.1 110.13 -23.63Q110.84 -25.15 112.16 -26.23Q113.48 -27.32 115.35 -27.9Q117.21 -28.48 119.58 -28.48Q122.75 -28.48 125.12 -27.44Q127.48 -26.4 129.17 -24.35L125.56 -20.94Q124.52 -22.22 123.03 -23.03Q121.55 -23.83 119.3 -23.83Q117.01 -23.83 115.87 -22.97Q114.73 -22.1 114.73 -20.54Q114.73 -18.93 115.65 -18.17Q116.57 -17.41 118.62 -17.05L121.23 -16.57Q125.48 -15.81 127.5 -13.86Q129.53 -11.91 129.53 -8.42Q129.53 -6.42 128.83 -4.79Q128.13 -3.17 126.78 -1.99Q125.44 -0.8 123.49 -0.16Q121.55 0.48 119.02 0.48Z M147.34 -20.94H152.15L143.73 3.73Q143.33 4.85 142.83 5.68Q142.33 6.5 141.64 7.02Q140.96 7.54 140 7.78Q139.04 8.02 137.75 8.02H134.66V3.93H138.44L139.4 1L131.9 -20.94H136.99L140.64 -9.87L142.01 -4.57H142.25L143.69 -9.87Z M155.52 0V-20.94H160.66V-17.45H160.86Q161.5 -19.13 162.89 -20.28Q164.27 -21.42 166.72 -21.42Q169.97 -21.42 171.69 -19.3Q173.42 -17.17 173.42 -13.24V0H168.28V-12.72Q168.28 -14.96 167.48 -16.09Q166.68 -17.21 164.83 -17.21Q164.03 -17.21 163.29 -16.99Q162.54 -16.77 161.96 -16.35Q161.38 -15.93 161.02 -15.3Q160.66 -14.68 160.66 -13.84V0Z M187.38 0.48Q185.05 0.48 183.24 -0.28Q181.44 -1.04 180.21 -2.49Q178.99 -3.93 178.35 -5.96Q177.71 -7.98 177.71 -10.51Q177.71 -13.04 178.35 -15.04Q178.99 -17.05 180.21 -18.47Q181.44 -19.9 183.24 -20.66Q185.05 -21.42 187.38 -21.42Q190.54 -21.42 192.61 -20.02Q194.68 -18.61 195.6 -16.13L191.39 -14.24Q191.03 -15.56 190.04 -16.43Q189.06 -17.29 187.38 -17.29Q185.21 -17.29 184.15 -15.93Q183.08 -14.56 183.08 -12.36V-8.54Q183.08 -6.34 184.15 -4.99Q185.21 -3.65 187.38 -3.65Q189.22 -3.65 190.22 -4.59Q191.23 -5.54 191.79 -6.94L195.72 -5.05Q194.68 -2.29 192.55 -0.9Q190.42 0.48 187.38 0.48Z"
private const val WORDMARK_WIDTH = 196.72f
private const val WORDMARK_CAP = 28f

private fun ImageVector.Builder.markPaths(color: Color, notification: Boolean): ImageVector.Builder {
    val p = if (notification) NOTIF else MARK
    val w = if (notification) 2.8f else 2.5f
    addPath(addPathNodes(p[0]), stroke = SolidColor(color), strokeLineWidth = w, strokeLineCap = StrokeCap.Round, strokeLineJoin = StrokeJoin.Round)
    addPath(addPathNodes(p[1]), stroke = SolidColor(color), strokeLineWidth = w, strokeLineCap = StrokeCap.Round, strokeLineJoin = StrokeJoin.Round)
    addPath(addPathNodes(p[2]), fill = SolidColor(color), stroke = SolidColor(color), strokeLineWidth = 0.9f, strokeLineJoin = StrokeJoin.Round)
    return this
}

fun markVector(color: Color, notification: Boolean = false): ImageVector =
    ImageVector.Builder("drive-sync-mark", 24.dp, 24.dp, 24f, 24f).markPaths(color, notification).build()

/**
 * The mark: one loop, phone corner on the left, cloud on the right, two harpoon arrows (⇌).
 * The lab accent (`logo-ink`) on surfaces; never follows dynamic colour. Below 20dp the heavier notification geometry is used.
 */
@Composable
fun DriveSyncMark(modifier: Modifier = Modifier, size: Dp = 28.dp, color: Color = Ds.brand.logoInk, contentDescription: String? = stringResource(R.string.app_name)) {
    val notification = size < 20.dp
    val vector = remember(color, notification) { markVector(color, notification) }
    Image(painter = rememberVectorPainter(vector), contentDescription = contentDescription, modifier = modifier.size(size))
}

/** Mark + outlined IBM Plex Sans SemiBold wordmark (the family's lockup rule). Use where the app isn't already named (About, onboarding). */
@Composable
fun DriveSyncLockup(modifier: Modifier = Modifier, height: Dp = 32.dp, markColor: Color = Ds.brand.logoInk, textColor: Color = MaterialTheme.colorScheme.onSurface) {
    val ms = 2.2f
    val mbox = 24f * ms
    val base = mbox / 2f + WORDMARK_CAP / 2f
    val tx = mbox + 10f
    val w = tx + WORDMARK_WIDTH + 2f
    val vector = remember(markColor, textColor) {
        ImageVector.Builder("drive-sync-lockup", (w / mbox * 24f).dp, 24.dp, w, mbox)
            .addGroup(scaleX = ms, scaleY = ms).markPaths(markColor, false).clearGroup()
            .addGroup(translationX = tx, translationY = base)
            .addPath(addPathNodes(WORDMARK), fill = SolidColor(textColor))
            .clearGroup()
            .build()
    }
    Image(painter = rememberVectorPainter(vector), contentDescription = stringResource(R.string.app_name), modifier = modifier.height(height).aspectRatio(w / mbox))
}

/** The app icon as drawn in-app (onboarding welcome): cream mark on the navy tile, corner 15/64 like the lab tile. */
@Composable
fun AppIconTile(modifier: Modifier = Modifier, size: Dp = 72.dp) {
    Box(
        modifier.size(size).clip(RoundedCornerShape(percent = 23)).background(Ds.brand.tile),
        contentAlignment = Alignment.Center,
    ) {
        DriveSyncMark(size = size * 0.717f, color = Ds.brand.tileMark, contentDescription = stringResource(R.string.app_name))
    }
}
