package com.almodhayan.drivesync.ui.theme

import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color

// Modhayan Labs family system (Modhayan Labs design system › Color). Drive Sync takes the lab's own
// accent, ink navy (ml-product-lab): it is a lab tool, not a registry product with an accent of its own.
// Neutrals, status and hues are the family's and never change per product.

val LabLight = lightColorScheme(
    primary = Color(0xFF1F3D6B),                 // ml-product-lab
    onPrimary = Color(0xFFFFFFFF),
    primaryContainer = Color(0xFFE4EBF6),
    onPrimaryContainer = Color(0xFF142C52),
    inversePrimary = Color(0xFF8EB0E6),
    secondary = Color(0xFF5F5C57),
    onSecondary = Color(0xFFFFFFFF),
    secondaryContainer = Color(0xFFECEBE7),
    onSecondaryContainer = Color(0xFF1F1E1B),
    tertiary = Color(0xFF9A6A1F),
    onTertiary = Color(0xFFFFFFFF),
    tertiaryContainer = Color(0xFFF8ECD7),
    onTertiaryContainer = Color(0xFF4A3209),
    error = Color(0xFFD0342C),
    onError = Color(0xFFFFFFFF),
    errorContainer = Color(0xFFFCE8E6),
    onErrorContainer = Color(0xFF8C1D18),
    background = Color(0xFFF5F4F0),
    onBackground = Color(0xFF1B1A17),
    surface = Color(0xFFF5F4F0),                 // the grouped page
    onSurface = Color(0xFF1B1A17),
    surfaceVariant = Color(0xFFECEAE4),
    onSurfaceVariant = Color(0xFF6B675F),
    surfaceDim = Color(0xFFE9E7E1),
    surfaceBright = Color(0xFFFBFAF8),
    surfaceContainerLowest = Color(0xFFFFFFFF),
    surfaceContainerLow = Color(0xFFFFFFFF),     // = ml-color-card, so any Material card lands on the card colour
    surfaceContainer = Color(0xFFEFEDE8),
    surfaceContainerHigh = Color(0xFFFFFFFF),    // menus and dialogs: ml-color-card-raised
    surfaceContainerHighest = Color(0xFFEEECE6), // = ml-color-fill: fields and tracks
    inverseSurface = Color(0xFF2B2925),
    inverseOnSurface = Color(0xFFF5F4F0),
    outline = Color(0xFF8A857C),
    outlineVariant = Color(0xFFE5E2DB),          // = ml-color-separator
    scrim = Color(0xFF000000),
    surfaceTint = Color.Transparent,             // never tonal elevation
)

val LabDark = darkColorScheme(
    primary = Color(0xFF8EB0E6),
    onPrimary = Color(0xFF0B1A30),
    primaryContainer = Color(0xFF1A2740),
    onPrimaryContainer = Color(0xFFCBDAF3),
    inversePrimary = Color(0xFF1F3D6B),
    secondary = Color(0xFFB9B4AB),
    onSecondary = Color(0xFF23211E),
    secondaryContainer = Color(0xFF2A2925),
    onSecondaryContainer = Color(0xFFE3DFD8),
    tertiary = Color(0xFFEBC07A),
    onTertiary = Color(0xFF3A2603),
    tertiaryContainer = Color(0xFF3A2C17),
    onTertiaryContainer = Color(0xFFF6DDB2),
    error = Color(0xFFFF8A80),
    onError = Color(0xFF4A0703),
    errorContainer = Color(0xFF3D1A17),
    onErrorContainer = Color(0xFFFFD3CE),
    background = Color(0xFF0E0D0B),
    onBackground = Color(0xFFF2EFE9),
    surface = Color(0xFF0E0D0B),
    onSurface = Color(0xFFF2EFE9),
    surfaceVariant = Color(0xFF2A2824),
    onSurfaceVariant = Color(0xFFA29D94),
    surfaceDim = Color(0xFF0E0D0B),
    surfaceBright = Color(0xFF2E2C28),
    surfaceContainerLowest = Color(0xFF0A0908),
    surfaceContainerLow = Color(0xFF1B1A17),     // = ml-color-card
    surfaceContainer = Color(0xFF211F1C),
    surfaceContainerHigh = Color(0xFF24221E),    // = ml-color-card-raised
    surfaceContainerHighest = Color(0xFF2A2824), // = ml-color-fill
    inverseSurface = Color(0xFFF2EFE9),
    inverseOnSurface = Color(0xFF2B2925),
    outline = Color(0xFF7D786F),
    outlineVariant = Color(0xFF2E2C28),
    scrim = Color(0xFF000000),
    surfaceTint = Color.Transparent,
)

/** A categorical hue: the glyph colour and its wash (icon tiles, tags). */
@Immutable
data class Hue(val ink: Color, val wash: Color)

/** The family's roles that Material has no slot for (ml-color-*). Never dynamic. */
@Immutable
data class LabColors(
    val card: Color,
    val cardRaised: Color,
    val fill: Color,
    val fillStrong: Color,
    val thumb: Color,
    val knob: Color,
    val separator: Color,
    val glass: Color,
    val coral: Hue,
    val saffron: Hue,
    val honey: Hue,
    val sea: Hue,
    val indigo: Hue,
    val lavender: Hue,
    val slate: Hue,
)

val LabLightColors = LabColors(
    card = Color(0xFFFFFFFF), cardRaised = Color(0xFFFFFFFF), fill = Color(0xFFEEECE6), fillStrong = Color(0xFFE3E0D9),
    thumb = Color(0xFFFFFFFF), knob = Color(0xFFFFFFFF), separator = Color(0xFFE5E2DB), glass = Color(0xC7FFFFFF),
    coral = Hue(Color(0xFFD5594E), Color(0xFFFFDAD6)),
    saffron = Hue(Color(0xFFC46A00), Color(0xFFFFDCC3)),
    honey = Hue(Color(0xFF8A6A00), Color(0xFFFDEAB2)),
    sea = Hue(Color(0xFF0A6B8F), Color(0xFFD4ECF8)),
    indigo = Hue(Color(0xFF4A4B9E), Color(0xFFE3E2FF)),
    lavender = Hue(Color(0xFF9B4F92), Color(0xFFFBDCF3)),
    slate = Hue(Color(0xFF56626E), Color(0xFFE6EAEE)),
)

val LabDarkColors = LabColors(
    card = Color(0xFF1B1A17), cardRaised = Color(0xFF24221E), fill = Color(0xFF2A2824), fillStrong = Color(0xFF34322D),
    thumb = Color(0xFF4A4741), knob = Color(0xFFFFFFFF), separator = Color(0xFF2E2C28), glass = Color(0x99262420),
    coral = Hue(Color(0xFFFF8073), Color(0xFF5A2C27)),
    saffron = Hue(Color(0xFFFFA85D), Color(0xFF55300F)),
    honey = Hue(Color(0xFFEAB400), Color(0xFF493704)),
    sea = Hue(Color(0xFF7CC8EC), Color(0xFF173A4A)),
    indigo = Hue(Color(0xFFA1A3FC), Color(0xFF363753)),
    lavender = Hue(Color(0xFFE19AD6), Color(0xFF4B3147)),
    slate = Hue(Color(0xFFA7B3BF), Color(0xFF262B30)),
)

val LocalLabColors = staticCompositionLocalOf { LabLightColors }

/** Logo colours: the mark in the lab accent, and the app tile (cream mark on navy). Never follow dynamic colour. */
@Immutable
data class BrandColors(
    val logoInk: Color,   // the mark and wordmark on plain surfaces
    val tile: Color,      // app icon ground
    val tileMark: Color,  // the mark on the tile
)

val BrandColorsLight = BrandColors(logoInk = Color(0xFF1F3D6B), tile = Color(0xFF1F3D6B), tileMark = Color(0xFFF5F3F0))
val BrandColorsDark = BrandColors(logoInk = Color(0xFF8EB0E6), tile = Color(0xFF1F3D6B), tileMark = Color(0xFFF5F3F0))

val LocalBrandColors = staticCompositionLocalOf { BrandColorsLight }
