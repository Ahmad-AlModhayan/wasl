package com.almodhayan.drivesync.ui.theme

import android.os.Build
import android.provider.Settings
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.remember
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.platform.LocalContext

/** True when Android's "Remove animations" is on: all motion stops, words and values still carry the state. */
val LocalReduceMotion = staticCompositionLocalOf { false }

/**
 * The Modhayan Labs family theme with the lab accent.
 *
 * @param theme "system", "light" or "dark" (Settings › Theme).
 * @param dynamicColor Settings › Device colours. When on, Material You replaces the four accent roles only;
 *   neutrals, status, hues and the logo stay fixed.
 */
@Composable
fun DriveSyncTheme(theme: String = "system", dynamicColor: Boolean = false, content: @Composable () -> Unit) {
    val dark = when (theme) { "light" -> false; "dark" -> true; else -> isSystemInDarkTheme() }
    val context = LocalContext.current
    val base = if (dark) LabDark else LabLight
    val scheme: ColorScheme = if (dynamicColor && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
        val d = if (dark) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
        base.copy(primary = d.primary, onPrimary = d.onPrimary, primaryContainer = d.primaryContainer, onPrimaryContainer = d.onPrimaryContainer)
    } else base
    val reduceMotion = remember(context) {
        runCatching { Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }.getOrDefault(false)
    }
    CompositionLocalProvider(
        LocalStatusColors provides if (dark) StatusDark else StatusLight,
        LocalBrandColors provides if (dark) BrandColorsDark else BrandColorsLight,
        LocalLabColors provides if (dark) LabDarkColors else LabLightColors,
        LocalReduceMotion provides reduceMotion,
    ) {
        MaterialTheme(colorScheme = scheme, typography = LabTypography, shapes = LabShapes, content = content)
    }
}

/** Shorthand for the design system's non-Material roles. */
object Ds {
    val status: StatusColors @Composable @ReadOnlyComposable get() = LocalStatusColors.current
    val brand: BrandColors @Composable @ReadOnlyComposable get() = LocalBrandColors.current
    val lab: LabColors @Composable @ReadOnlyComposable get() = LocalLabColors.current
    val reduceMotion: Boolean @Composable @ReadOnlyComposable get() = LocalReduceMotion.current
}
