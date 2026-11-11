package com.almodhayan.drivesync.ui.theme

import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Shapes
import androidx.compose.ui.unit.dp

/**
 * Family corners (Modhayan Labs › Layout, shape and motion): extra-small 6 (badges) · small 10 (icon tiles)
 * · medium 16 (fields, banners, path boxes) · large 26 (cards, grouped sections) · extra-large 32 (sheets,
 * dialogs, the tab bar). Buttons, chips and switches are capsules (CircleShape). Rounded rectangles stand in
 * for the continuous corners until androidx.graphics.shapes is added.
 */
val LabShapes = Shapes(
    extraSmall = RoundedCornerShape(6.dp),
    small = RoundedCornerShape(10.dp),
    medium = RoundedCornerShape(16.dp),
    large = RoundedCornerShape(26.dp),
    extraLarge = RoundedCornerShape(32.dp),
)

/** 4dp grid. Names are the dp value. Cards sit 16 from the edge with 16 inside; 24 between sections. */
object Space {
    val s2 = 2.dp; val s4 = 4.dp; val s8 = 8.dp; val s12 = 12.dp; val s16 = 16.dp
    val s20 = 20.dp; val s24 = 24.dp; val s32 = 32.dp; val s48 = 48.dp; val s64 = 64.dp
}

object Size {
    val touchTarget = 48.dp
    val icon = 24.dp
    val iconSmall = 18.dp
    /** Settings-row and circle-button icons: a step under [icon], so they sit lighter beside 15sp row titles. */
    val iconRow = 22.dp
    val iconTile = 32.dp
    val circleButton = 40.dp
    val button = 52.dp
    val buttonSmall = 36.dp
    /** Least height of a compact action button (cards, banners, rows). A floor, never a fixed height: the label grows with the font size (#149). */
    val buttonCompact = 32.dp
    /** Least height of a [com.almodhayan.drivesync.ui.components.Tag]. */
    val tag = 20.dp
    val chip = 32.dp
    val statusChip = 30.dp
    val statusChipCompact = 24.dp
    val appBar = 64.dp
    val tabBar = 66.dp
    val listOneLine = 56.dp
    val listTwoLine = 68.dp
    /** Widest a settings row's value may grow before it wraps, so the title keeps its line. */
    val settingValueMax = 96.dp
    val fab = 48.dp
    val iconFab = 20.dp
    val storageBar = 8.dp
    val progressTrack = 4.dp
    val hairline = 0.5.dp
}

object Motion {
    const val Short = 150
    const val Medium = 300
    const val Long = 450
    const val Progress = 600
    const val SyncTurn = 2000
    const val SnackbarUndoMs = 10_000L
    val Standard = CubicBezierEasing(0.2f, 0f, 0f, 1f)
    val Decelerate = CubicBezierEasing(0f, 0f, 0f, 1f)
    val Accelerate = CubicBezierEasing(0.3f, 0f, 1f, 1f)
    val EmphasizedDecelerate = CubicBezierEasing(0.05f, 0.7f, 0.1f, 1f)
    val EmphasizedAccelerate = CubicBezierEasing(0.3f, 0f, 0.8f, 0.15f)
}

object Opacity {
    const val disabledContent = 0.38f
    const val disabledContainer = 0.12f
    const val scrim = 0.32f
}
