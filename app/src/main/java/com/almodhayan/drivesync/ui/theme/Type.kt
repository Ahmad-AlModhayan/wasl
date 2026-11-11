package com.almodhayan.drivesync.ui.theme

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.runtime.Composable
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.LineHeightStyle
import androidx.compose.ui.unit.sp
import com.almodhayan.drivesync.R

/**
 * Readex Pro for everything people read, in English and Arabic: one family drawn for both scripts,
 * so mixed text stays one voice. Bundled, never downloaded (res/font, OFL). The bundled weights add
 * tabular digits (`tnum`), which Readex Pro doesn't ship, for [Tabular].
 */
val ReadexPro = FontFamily(
    Font(R.font.readex_pro_regular, FontWeight.Normal),
    Font(R.font.readex_pro_medium, FontWeight.Medium),
    Font(R.font.readex_pro_semibold, FontWeight.SemiBold),
    Font(R.font.readex_pro_bold, FontWeight.Bold),
)

/** IBM Plex Mono for exact paths, patterns, versions and the log. */
val PlexMono = FontFamily(
    Font(R.font.ibm_plex_mono_regular, FontWeight.Normal),
    Font(R.font.ibm_plex_mono_medium, FontWeight.Medium),
)

private val Trim = LineHeightStyle(LineHeightStyle.Alignment.Center, LineHeightStyle.Trim.None)

private fun FontFamily.s(size: Int, line: Int, weight: FontWeight) =
    TextStyle(fontFamily = this, fontSize = size.sp, lineHeight = line.sp, fontWeight = weight, lineHeightStyle = Trim)

/**
 * The family's interface scale: bold titles, regular text, no tracking. Stepped down about 12% in 0.4.2
 * (a Kahf Labs decision, 2026-11): body text never below 14, secondary text 13. Readex Pro's tall x-height and open
 * Arabic forms keep both scripts legible at these sizes.
 */
fun labTypography(family: FontFamily): Typography = with(family) { Typography(
    displayLarge = s(57, 64, FontWeight.SemiBold),
    displayMedium = s(40, 50, FontWeight.SemiBold),
    displaySmall = s(32, 44, FontWeight.Bold),
    headlineLarge = s(28, 38, FontWeight.Bold),
    headlineMedium = s(25, 34, FontWeight.Bold),
    headlineSmall = s(20, 28, FontWeight.Bold),
    titleLarge = s(18, 26, FontWeight.Bold),
    titleMedium = s(15, 22, FontWeight.SemiBold),
    titleSmall = s(14, 20, FontWeight.SemiBold),
    bodyLarge = s(15, 24, FontWeight.Normal),
    bodyMedium = s(14, 22, FontWeight.Normal),
    bodySmall = s(13, 18, FontWeight.Normal),
    labelLarge = s(15, 22, FontWeight.SemiBold),
    labelMedium = s(13, 18, FontWeight.SemiBold),
    labelSmall = s(11, 16, FontWeight.SemiBold),
) }

val LabTypography = labTypography(ReadexPro)

/** Settings-row titles: title-medium at 500. */
val RowTitle: TextStyle
    @Composable get() = MaterialTheme.typography.titleMedium.copy(fontWeight = FontWeight.Medium)

/** Paths shown in full, exclude patterns (ml-typescale-data). */
val MonoMedium = TextStyle(fontFamily = PlexMono, fontWeight = FontWeight.Medium, fontSize = 12.sp, lineHeight = 18.sp)

/** Log lines. */
val MonoSmall = TextStyle(fontFamily = PlexMono, fontWeight = FontWeight.Normal, fontSize = 12.sp, lineHeight = 16.sp)

/** Every number that can change uses tabular figures (the bundled Readex Pro's `tnum`). */
val Tabular = SpanStyle(fontFeatureSettings = "tnum")

val TextStyle.tabular: TextStyle get() = copy(fontFeatureSettings = "tnum")
