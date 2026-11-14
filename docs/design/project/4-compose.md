# Compose implementation

How the tokens map onto Jetpack Compose with Material 3 (`androidx.compose.material3`). Values are generated from `tokens.json`; if they disagree, the tokens win.

## Theme entry point

```kotlin
@Composable
fun DriveSyncTheme(
    darkTheme: Boolean = isSystemInDarkTheme(),
    dynamicColor: Boolean = true,          // Settings › Colours from wallpaper
    content: @Composable () -> Unit,
) {
    val context = LocalContext.current
    val colorScheme = when {
        dynamicColor && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ->
            if (darkTheme) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
        darkTheme -> BrandDark
        else -> BrandLight
    }
    // Status colours are fixed: never derived from the scheme, never harmonised.
    val status = if (darkTheme) StatusDark else StatusLight
    CompositionLocalProvider(LocalStatusColors provides status) {
        MaterialTheme(
            colorScheme = colorScheme,
            typography = DriveSyncTypography,
            shapes = DriveSyncShapes,
            motionScheme = MotionScheme.standard(),   // calm: never the expressive scheme
            content = content,
        )
    }
}
```

`motionScheme` exists in recent Material 3 releases; on older versions drop the argument and keep the durations and easings below.

## Brand fallback colours

```kotlin
val BrandLight = lightColorScheme(
    primary = Color(0xFF004530),
    onPrimary = Color(0xFFFFFFFF),
    primaryContainer = Color(0xFF1B5E45),
    onPrimaryContainer = Color(0xFF94D5B5),
    inversePrimary = Color(0xFF93D4B5),
    secondary = Color(0xFF4D6357),
    onSecondary = Color(0xFFFFFFFF),
    secondaryContainer = Color(0xFFCFE9D9),
    onSecondaryContainer = Color(0xFF092016),
    tertiary = Color(0xFF0D4250),
    onTertiary = Color(0xFFFFFFFF),
    tertiaryContainer = Color(0xFF2B5968),
    onTertiaryContainer = Color(0xFFA0CEE0),
    error = Color(0xFFBA1A1A),
    onError = Color(0xFFFFFFFF),
    errorContainer = Color(0xFFFFDAD6),
    onErrorContainer = Color(0xFF93000A),
    surface = Color(0xFFF8FAF6),
    surfaceDim = Color(0xFFD8DBD7),
    surfaceBright = Color(0xFFF8FAF6),
    surfaceContainerLowest = Color(0xFFFFFFFF),
    surfaceContainerLow = Color(0xFFF2F4F0),
    surfaceContainer = Color(0xFFECEEEA),
    surfaceContainerHigh = Color(0xFFE7E9E5),
    surfaceContainerHighest = Color(0xFFE1E3DF),
    onSurface = Color(0xFF191C1A),
    onSurfaceVariant = Color(0xFF404943),
    surfaceVariant = Color(0xFFDBE5DD),
    inverseSurface = Color(0xFF2E312F),
    inverseOnSurface = Color(0xFFEFF1ED),
    outline = Color(0xFF707973),
    outlineVariant = Color(0xFFBFC9C2),
    scrim = Color(0xFF000000),
    background = Color(0xFFF8FAF6),
    onBackground = Color(0xFF191C1A),
    surfaceTint = Color(0xFF004530),
)

val BrandDark = darkColorScheme(
    primary = Color(0xFF93D4B5),
    onPrimary = Color(0xFF003826),
    primaryContainer = Color(0xFF1B5E45),
    onPrimaryContainer = Color(0xFF94D5B5),
    inversePrimary = Color(0xFF296A50),
    secondary = Color(0xFFB3CCBE),
    onSecondary = Color(0xFF1F352B),
    secondaryContainer = Color(0xFF384E43),
    onSecondaryContainer = Color(0xFFCFE9D9),
    tertiary = Color(0xFFA0CEDF),
    onTertiary = Color(0xFF003543),
    tertiaryContainer = Color(0xFF2B5968),
    onTertiaryContainer = Color(0xFFA0CEE0),
    error = Color(0xFFFFB4AB),
    onError = Color(0xFF690005),
    errorContainer = Color(0xFF93000A),
    onErrorContainer = Color(0xFFFFDAD6),
    surface = Color(0xFF111412),
    surfaceDim = Color(0xFF111412),
    surfaceBright = Color(0xFF373A37),
    surfaceContainerLowest = Color(0xFF0C0F0D),
    surfaceContainerLow = Color(0xFF191C1A),
    surfaceContainer = Color(0xFF1D201E),
    surfaceContainerHigh = Color(0xFF272B28),
    surfaceContainerHighest = Color(0xFF323633),
    onSurface = Color(0xFFE1E3DF),
    onSurfaceVariant = Color(0xFFBFC9C2),
    surfaceVariant = Color(0xFF404943),
    inverseSurface = Color(0xFFE1E3DF),
    inverseOnSurface = Color(0xFF2E312F),
    outline = Color(0xFF8A938C),
    outlineVariant = Color(0xFF404943),
    scrim = Color(0xFF000000),
    background = Color(0xFF111412),
    onBackground = Color(0xFFE1E3DF),
    surfaceTint = Color(0xFF93D4B5),
)
```

## Status colours

```kotlin
@Immutable data class StatusColor(val content: Color, val container: Color, val onContainer: Color)

@Immutable data class StatusColors(
    val synced: StatusColor, val syncing: StatusColor, val waiting: StatusColor,
    val approval: StatusColor, val conflict: StatusColor, val failed: StatusColor,
)

val StatusLight = StatusColors(
    synced = StatusColor(Color(0xFF2A6B37), Color(0xFFC8ECC6), Color(0xFF003913)),
    syncing = StatusColor(Color(0xFF185EAF), Color(0xFFD6E3FF), Color(0xFF003062)),
    waiting = StatusColor(Color(0xFF57605B), Color(0xFFE0E3DF), Color(0xFF2A322D)),
    approval = StatusColor(Color(0xFF7F5600), Color(0xFFFFDEAE), Color(0xFF432C00)),
    conflict = StatusColor(Color(0xFF7F4A8B), Color(0xFFFDD6FF), Color(0xFF4C1A59)),
    failed = StatusColor(Color(0xFFBA1A1A), Color(0xFFFFDAD6), Color(0xFF690005)),
)

val StatusDark = StatusColors(
    synced = StatusColor(Color(0xFF93D698), Color(0xFF244227), Color(0xFFAEF3B2)),
    syncing = StatusColor(Color(0xFFA9C7FF), Color(0xFF213C63), Color(0xFFD6E3FF)),
    waiting = StatusColor(Color(0xFFBFC9C2), Color(0xFF383C39), Color(0xFFDBE5DD)),
    approval = StatusColor(Color(0xFFFFBA3E), Color(0xFF513600), Color(0xFFFFDEAE)),
    conflict = StatusColor(Color(0xFFF0B0FA), Color(0xFF4D3252), Color(0xFFFDD6FF)),
    failed = StatusColor(Color(0xFFFFB4AB), Color(0xFF701E18), Color(0xFFFFDAD6)),
)

val LocalStatusColors = staticCompositionLocalOf { StatusLight }

enum class SyncStatus(val icon: ImageVector, @StringRes val label: Int, val loud: Boolean) {
    Synced(Icons.CloudDone, R.string.status_synced, loud = false),
    Syncing(Icons.Sync, R.string.status_syncing, loud = false),
    Waiting(Icons.Wifi, R.string.status_waiting_wifi, loud = false),
    Approval(Icons.FrontHand, R.string.status_needs_approval, loud = true),
    Conflict(Icons.CallSplit, R.string.status_conflict, loud = true),
    Failed(Icons.Error, R.string.status_failed, loud = true),
}
```

`Icons.*` here are Material Symbols Rounded vector assets imported into the project (`res/drawable`); the legacy `material-icons-extended` set doesn't include every symbol.

## Type

```kotlin
@OptIn(ExperimentalTextApi::class)
private fun figtree(weight: Int) = Font(
    R.font.figtree_variable, FontWeight(weight),
    variationSettings = FontVariation.Settings(FontVariation.weight(weight)),
)
val Figtree = FontFamily(figtree(400), figtree(500), figtree(600))
val JetBrainsMono = FontFamily(Font(R.font.jetbrains_mono_variable, FontWeight(450)))

val DriveSyncTypography = Typography(
    displaySmall   = TextStyle(fontFamily = Figtree, fontWeight = FontWeight(500), fontSize = 36.sp, lineHeight = 44.sp, letterSpacing = (-0.25).sp),
    headlineMedium = TextStyle(fontFamily = Figtree, fontWeight = FontWeight(600), fontSize = 28.sp, lineHeight = 36.sp, letterSpacing = (-0.2).sp),
    headlineSmall  = TextStyle(fontFamily = Figtree, fontWeight = FontWeight(600), fontSize = 24.sp, lineHeight = 32.sp),
    titleLarge     = TextStyle(fontFamily = Figtree, fontWeight = FontWeight(600), fontSize = 22.sp, lineHeight = 28.sp),
    titleMedium    = TextStyle(fontFamily = Figtree, fontWeight = FontWeight(600), fontSize = 16.sp, lineHeight = 24.sp, letterSpacing = 0.1.sp),
    titleSmall     = TextStyle(fontFamily = Figtree, fontWeight = FontWeight(600), fontSize = 14.sp, lineHeight = 20.sp, letterSpacing = 0.1.sp),
    bodyLarge      = TextStyle(fontFamily = Figtree, fontWeight = FontWeight(400), fontSize = 16.sp, lineHeight = 24.sp, letterSpacing = 0.15.sp),
    bodyMedium     = TextStyle(fontFamily = Figtree, fontWeight = FontWeight(400), fontSize = 14.sp, lineHeight = 20.sp, letterSpacing = 0.2.sp),
    bodySmall      = TextStyle(fontFamily = Figtree, fontWeight = FontWeight(400), fontSize = 12.sp, lineHeight = 16.sp, letterSpacing = 0.3.sp),
    labelLarge     = TextStyle(fontFamily = Figtree, fontWeight = FontWeight(600), fontSize = 14.sp, lineHeight = 20.sp, letterSpacing = 0.1.sp),
    labelMedium    = TextStyle(fontFamily = Figtree, fontWeight = FontWeight(600), fontSize = 12.sp, lineHeight = 16.sp, letterSpacing = 0.4.sp),
    labelSmall     = TextStyle(fontFamily = Figtree, fontWeight = FontWeight(600), fontSize = 11.sp, lineHeight = 16.sp, letterSpacing = 0.5.sp),
)
val MonoMedium = TextStyle(fontFamily = JetBrainsMono, fontSize = 13.sp, lineHeight = 20.sp)
val MonoSmall  = TextStyle(fontFamily = JetBrainsMono, fontSize = 11.sp, lineHeight = 16.sp)
val Tabular = SpanStyle(fontFeatureSettings = "tnum")
```

Display large/medium and headline large are left at Material defaults: the app doesn't use them. The fonts ship in the APK (no downloadable-font request).

## Shape, spacing, motion

```kotlin
val DriveSyncShapes = Shapes(
    extraSmall = RoundedCornerShape(4.dp), small = RoundedCornerShape(8.dp), medium = RoundedCornerShape(12.dp),
    large = RoundedCornerShape(16.dp), extraLarge = RoundedCornerShape(28.dp),
)
object Space { val s2 = 2.dp; val s4 = 4.dp; val s8 = 8.dp; val s12 = 12.dp; val s16 = 16.dp; val s20 = 20.dp; val s24 = 24.dp; val s32 = 32.dp; val s48 = 48.dp; val s64 = 64.dp }
object Motion {
    const val Short = 150; const val Medium = 300; const val Long = 450; const val Progress = 600; const val SyncTurn = 2000
    val Standard = CubicBezierEasing(0.2f, 0f, 0f, 1f)
    val EmphasizedDecelerate = CubicBezierEasing(0.05f, 0.7f, 0.1f, 1f)
    val EmphasizedAccelerate = CubicBezierEasing(0.3f, 0f, 0.8f, 0.15f)
}
```

Folder-pair cards use `OutlinedCard` with `containerColor = surfaceContainerLow` and `shape = shapes.large`. Read `Settings.Global.ANIMATOR_DURATION_SCALE`: at 0, stop the syncing icon's `rememberInfiniteTransition`.

## Component map

| Design system | Compose |
|---|---|
| Button (filled, tonal, outlined, text, danger) | `Button`, `FilledTonalButton`, `OutlinedButton`, `TextButton`, `Button(colors = ButtonDefaults.buttonColors(containerColor = error))` |
| Fab | `ExtendedFloatingActionButton(expanded = !scrolled)` |
| Chip | `FilterChip` |
| SegmentedButton | `SingleChoiceSegmentedButtonRow` |
| StatusChip | Custom `Row` in a `Surface(shape = shapes.small, color = status.container)`; plain variant without the Surface |
| StatusBanner, Callout | Custom `Surface(shape = shapes.medium)` |
| SyncProgress | `LinearProgressIndicator(progress = { value }, color = status.syncing.content, trackColor = status.syncing.container)` |
| StorageBar | Custom `Canvas` (segments with 2dp gaps) + legend `Row` |
| FolderPairCard | `OutlinedCard` |
| SyncModeOption | `Card` + `RadioButton` in a `Modifier.selectableGroup()` |
| ListItem, SettingRow, HealthCheckRow | `ListItem` (+ `Switch` as trailing content) |
| TopAppBar | `TopAppBar` / `LargeTopAppBar` with `exitUntilCollapsedScrollBehavior` |
| NavigationBar | `NavigationBar` + `BadgedBox` |
| Dialog | `AlertDialog(icon = …)` |
| Snackbar | `SnackbarHost`, `SnackbarDuration.Long` for Undo |
| PathBreadcrumb | `LazyRow` of `TextButton`s |
| ConflictCard, VersionItem, ChangeRow, ChangeSummary, LogView, PermissionStep | Custom, from the primitives above |

## App icon resources

Copy `assets/Logos/android/` into `res/drawable/` (the three layers and `ic_stat_drive_sync.xml`) and `res/mipmap-anydpi-v26/ic_launcher.xml`. Use `R.drawable.ic_stat_drive_sync` as `setSmallIcon` for every notification channel.
