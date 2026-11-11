@file:OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)

package com.almodhayan.drivesync.ui.components

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.expandHorizontally
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkHorizontally
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FilterChipDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LargeTopAppBar
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.TopAppBarScrollBehavior
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.foundation.layout.widthIn
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.annotation.StringRes
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.ui.theme.Ds
import com.almodhayan.drivesync.ui.theme.MonoMedium
import com.almodhayan.drivesync.ui.theme.Motion
import com.almodhayan.drivesync.ui.theme.Opacity
import com.almodhayan.drivesync.ui.theme.Size
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import com.almodhayan.drivesync.ui.theme.tabular
import kotlinx.coroutines.withTimeoutOrNull
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.SwitchDefaults
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.draw.shadow
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.layout.fillMaxHeight
import com.almodhayan.drivesync.ui.theme.RowTitle

// ------------------------------------------------------------------ top app bar

enum class NavIcon { Back, Close, None }

/**
 * Screen header on the grouped page (no container colour, no tonal tint). Root tabs show the mark and a
 * bold title; pushed screens show a plain back arrow; editors and pickers a plain close icon.
 * Subtitle names the pair the screen is about.
 */
@Composable
fun DsTopAppBar(
    title: String,
    modifier: Modifier = Modifier,
    subtitle: String? = null,
    navIcon: NavIcon = NavIcon.Back,
    onNav: () -> Unit = {},
    logo: Boolean = false,
    large: Boolean = false,
    scrollBehavior: TopAppBarScrollBehavior? = null,
    actions: @Composable RowScope.() -> Unit = {},
) {
    val cs = MaterialTheme.colorScheme
    val colors = TopAppBarDefaults.topAppBarColors(containerColor = cs.surface, scrolledContainerColor = cs.surface)
    val root = navIcon == NavIcon.None
    val nav: @Composable () -> Unit = {
        when (navIcon) {
            // A plain icon, not a filled circle: the bar carries the title, not another shape (0.4.1).
            NavIcon.Back -> IconButton(onClick = onNav, modifier = Modifier.padding(start = Space.s4)) { Icon(Sym.ArrowBack, contentDescription = stringResource(R.string.common_back)) }
            NavIcon.Close -> IconButton(onClick = onNav, modifier = Modifier.padding(start = Space.s4)) { Icon(Sym.Close, contentDescription = stringResource(R.string.common_close)) }
            NavIcon.None -> if (logo) DriveSyncMark(Modifier.padding(start = Space.s16, end = Space.s4), size = 28.dp, contentDescription = null)
        }
    }
    if (large) {
        LargeTopAppBar(
            title = {
                Column {
                    Text(title, maxLines = 2, overflow = TextOverflow.Ellipsis, modifier = Modifier.semantics { heading() })
                    if (subtitle != null) Text(subtitle, style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            },
            modifier = modifier, navigationIcon = nav, actions = actions, scrollBehavior = scrollBehavior,
            colors = TopAppBarDefaults.largeTopAppBarColors(containerColor = cs.surface, scrolledContainerColor = cs.surface),
        )
    } else {
        TopAppBar(
            title = {
                Column {
                    Text(
                        title,
                        style = if (root) MaterialTheme.typography.headlineMedium else MaterialTheme.typography.titleLarge,
                        maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.semantics { heading() },
                    )
                    if (subtitle != null) Text(subtitle, style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            },
            modifier = modifier, navigationIcon = nav, actions = actions, scrollBehavior = scrollBehavior, colors = colors,
        )
    }
}

/** 40dp circle icon button (48dp hit area): grey fill by default, `tinted` for the one accent action in a bar. */
@Composable
fun CircleIconButton(icon: ImageVector, contentDescription: String, onClick: () -> Unit, modifier: Modifier = Modifier, tinted: Boolean = false, enabled: Boolean = true) {
    val cs = MaterialTheme.colorScheme
    FilledIconButton(
        onClick = onClick,
        modifier = modifier,
        enabled = enabled,
        shape = CircleShape,
        colors = IconButtonDefaults.filledIconButtonColors(
            containerColor = if (tinted) cs.primaryContainer else Ds.lab.fill,
            contentColor = if (tinted) cs.primary else cs.onSurface,
        ),
    ) { Icon(icon, contentDescription = contentDescription, modifier = Modifier.size(Size.iconRow)) }
}

// ------------------------------------------------------------------ navigation bar

/** The four top-level destinations. */
enum class TopDestination(@StringRes val labelRes: Int) {
    Pairs(R.string.components_nav_pairs), Problems(R.string.components_nav_problems), Bin(R.string.components_nav_recycle_bin), Settings(R.string.common_settings);

    val label: String get() = Strings.get(labelRes)

    /** One word for the tab bar. */
    @get:StringRes
    val tabLabelRes: Int get() = if (this == Bin) R.string.components_nav_bin else labelRes
    val tabLabel: String get() = Strings.get(tabLabelRes)

    fun icon(selected: Boolean): ImageVector = when (this) {
        Pairs -> if (selected) Sym.FolderCopyFill else Sym.FolderCopy
        Problems -> if (selected) Sym.ReportFill else Sym.Report
        Bin -> if (selected) Sym.DeleteFill else Sym.Delete
        Settings -> if (selected) Sym.SettingsFill else Sym.Settings
    }
}

/**
 * The floating capsule tab bar: 66dp, 16 from the sides and 20 above the gesture area, on the raised card
 * with a soft wide shadow. The active destination is a filled icon in a primary-container pill. Only
 * Problems ever badges.
 */
@Composable
fun DsNavigationBar(current: TopDestination?, problems: Int, onSelect: (TopDestination) -> Unit, modifier: Modifier = Modifier) {
    val cs = MaterialTheme.colorScheme
    val shape = MaterialTheme.shapes.extraLarge
    val needYou = pluralStringResource(R.plurals.components_nav_problems_need_you, problems, problems)
    Box(modifier.fillMaxWidth().navigationBarsPadding().padding(start = Space.s16, end = Space.s16, top = Space.s8, bottom = Space.s20)) {
        Row(
            Modifier.fillMaxWidth().height(66.dp)
                .shadow(elevation = 12.dp, shape = shape, ambientColor = Color.Black.copy(alpha = 0.12f), spotColor = Color.Black.copy(alpha = 0.12f))
                .clip(shape).background(Ds.lab.cardRaised)
                .selectableGroup()
                .padding(6.dp),
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            TopDestination.entries.forEach { d ->
                val selected = d == current
                val fg = if (selected) cs.onPrimaryContainer else cs.onSurfaceVariant
                Column(
                    Modifier.weight(1f).fillMaxHeight().clip(CircleShape)
                        .background(if (selected) cs.primaryContainer else Color.Transparent)
                        .selectable(selected = selected, role = Role.Tab, onClick = { onSelect(d) })
                        .testTag("nav_${d.name.lowercase()}")
                        .semantics { if (d == TopDestination.Problems && problems > 0) stateDescription = needYou },
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.Center,
                ) {
                    if (d == TopDestination.Problems && problems > 0) {
                        BadgedBox(badge = { Badge { Text(if (problems > 99) "99+" else problems.toString(), style = MaterialTheme.typography.labelSmall.tabular) } }) {
                            Icon(d.icon(selected), contentDescription = null, tint = fg, modifier = Modifier.size(24.dp))
                        }
                    } else Icon(d.icon(selected), contentDescription = null, tint = fg, modifier = Modifier.size(24.dp))
                    Text(stringResource(d.tabLabelRes), style = MaterialTheme.typography.labelSmall, color = fg, maxLines = 1, overflow = TextOverflow.Ellipsis, textAlign = TextAlign.Center)
                }
            }
        }
    }
}

// ------------------------------------------------------------------ breadcrumb, segmented, chips

/** Where you are inside a picker; every crumb goes back to that level. Anchored to the end. */
@Composable
fun PathBreadcrumb(rootLabel: String, rootIcon: ImageVector, parts: List<String>, onCrumb: (Int) -> Unit, modifier: Modifier = Modifier) {
    val state = rememberLazyListState()
    val all = listOf(rootLabel) + parts
    LaunchedEffect(all.size) { if (all.isNotEmpty()) state.scrollToItem((all.size - 1).coerceAtLeast(0)) }
    val cs = MaterialTheme.colorScheme
    LazyRow(modifier.fillMaxWidth(), state = state, contentPadding = PaddingValues(horizontal = Space.s16, vertical = Space.s8), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(2.dp)) {
        itemsIndexed(all) { i, label ->
            if (i > 0) Icon(Sym.ChevronRight, contentDescription = null, tint = cs.outline, modifier = Modifier.size(18.dp))
            val current = i == all.lastIndex
            val goTo = stringResource(R.string.components_breadcrumb_go_to, label)
            Row(
                Modifier.clip(MaterialTheme.shapes.small)
                    .then(if (current) Modifier.background(Ds.lab.fill) else Modifier)
                    .clickable(enabled = !current, onClickLabel = goTo) { onCrumb(i - 1) }
                    .padding(start = if (i == 0) 4.dp else 8.dp, end = 8.dp, top = 6.dp, bottom = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                if (i == 0) Icon(rootIcon, contentDescription = null, tint = if (current) cs.onSurface else cs.onSurfaceVariant, modifier = Modifier.size(18.dp))
                Text(label, style = MaterialTheme.typography.labelLarge, color = if (current) cs.onSurface else cs.onSurfaceVariant, maxLines = 1)
            }
        }
    }
}

/** Two or three places: Internal storage / SD card, My Drive / Shared with me. */
@Composable
fun DsSegmentedButton(items: List<String>, selectedIndex: Int, onSelect: (Int) -> Unit, modifier: Modifier = Modifier) {
    val cs = MaterialTheme.colorScheme
    SingleChoiceSegmentedButtonRow(modifier.fillMaxWidth()) {
        items.forEachIndexed { i, label ->
            SegmentedButton(
                selected = i == selectedIndex,
                onClick = { onSelect(i) },
                shape = SegmentedButtonDefaults.itemShape(i, items.size),
                colors = SegmentedButtonDefaults.colors(activeContainerColor = cs.primaryContainer, activeContentColor = cs.onPrimaryContainer, activeBorderColor = cs.outlineVariant, inactiveContainerColor = Ds.lab.card, inactiveBorderColor = cs.outlineVariant),
                label = { Text(label, maxLines = 1, overflow = TextOverflow.Ellipsis) },
            )
        }
    }
}

@Immutable
data class ChipModel(val label: String, val selected: Boolean, val count: Int? = null, val icon: ImageVector? = null, val onClick: () -> Unit)

/** Filter chips with counts, one row, horizontally scrollable, never wrapping. */
@Composable
fun FilterChipRow(chips: List<ChipModel>, modifier: Modifier = Modifier) {
    val cs = MaterialTheme.colorScheme
    Row(modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = Space.s16, vertical = Space.s4), horizontalArrangement = Arrangement.spacedBy(Space.s8)) {
        chips.forEach { c ->
            FilterChip(
                selected = c.selected,
                onClick = c.onClick,
                label = {
                    Text(c.label, maxLines = 1)
                    if (c.count != null) { Spacer(Modifier.size(Space.s8)); Text(Fmt.count(c.count), style = MaterialTheme.typography.labelLarge.tabular, color = if (c.selected) cs.onPrimaryContainer else cs.onSurfaceVariant) }
                },
                leadingIcon = when {
                    c.selected -> { { Icon(Sym.Check, contentDescription = null, modifier = Modifier.size(18.dp)) } }
                    c.icon != null -> { { Icon(c.icon, contentDescription = null, modifier = Modifier.size(18.dp)) } }
                    else -> null
                },
                shape = CircleShape,
                border = null,
                colors = FilterChipDefaults.filterChipColors(containerColor = Ds.lab.fill, selectedContainerColor = cs.primaryContainer, selectedLabelColor = cs.onPrimaryContainer, selectedLeadingIconColor = cs.onPrimaryContainer, labelColor = cs.onSurfaceVariant),
            )
        }
    }
}

/** Small pill, e.g. "Current" on a version. */
@Composable
fun Tag(text: String, modifier: Modifier = Modifier) {
    Surface(modifier, shape = CircleShape, color = MaterialTheme.colorScheme.primaryContainer) {
        Text(text, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onPrimaryContainer, modifier = Modifier.heightIn(min = Size.tag).padding(horizontal = Space.s8, vertical = 2.dp))
    }
}

// ------------------------------------------------------------------ settings rows

sealed interface SettingControl {
    data class Toggle(val checked: Boolean, val onChange: (Boolean) -> Unit) : SettingControl
    data class Value(val text: String) : SettingControl
    data object Chevron : SettingControl
    data object None : SettingControl
}

/** Where a row sits in a grouped section: corners round only at the group's ends, hairlines between. */
enum class GroupPos { None, Single, First, Middle, Last }

/**
 * One row of a grouped section (Modhayan Labs › Row): white card 16 from the screen edge, 26dp corners at
 * the group's ends, 0.5dp hairlines inset to the text. Works per LazyColumn item, so a section can be a run
 * of items. [GroupPos.None] draws nothing (content stays on the page).
 */
@Composable
fun GroupItem(pos: GroupPos, modifier: Modifier = Modifier, inset: androidx.compose.ui.unit.Dp = 64.dp, content: @Composable () -> Unit) {
    if (pos == GroupPos.None) { Box(modifier) { content() }; return }
    val r = 26.dp
    val top = pos == GroupPos.Single || pos == GroupPos.First
    val bottom = pos == GroupPos.Single || pos == GroupPos.Last
    val shape = RoundedCornerShape(topStart = if (top) r else 0.dp, topEnd = if (top) r else 0.dp, bottomStart = if (bottom) r else 0.dp, bottomEnd = if (bottom) r else 0.dp)
    val sep = Ds.lab.separator
    Column(modifier.fillMaxWidth().padding(horizontal = Space.s16).clip(shape).background(Ds.lab.card)) {
        if (!top) Box(Modifier.fillMaxWidth().padding(start = inset).height(0.5.dp).background(sep))
        content()
    }
}

/**
 * A settings row: icon (no tile), title (500), the summary that states the current effect ("Off: mobile
 * data allowed"), then a value, chevron or switch. The whole row toggles a switch. `danger` only for
 * Disconnect.
 */
@Composable
fun SettingRow(
    title: String,
    modifier: Modifier = Modifier,
    icon: ImageVector? = null,
    summary: String? = null,
    control: SettingControl = SettingControl.Chevron,
    danger: Boolean = false,
    enabled: Boolean = true,
    onClick: (() -> Unit)? = null,
) {
    val cs = MaterialTheme.colorScheme
    val titleColor = if (danger) cs.error else cs.onSurface
    val rowMod = when (control) {
        is SettingControl.Toggle -> Modifier.toggleable(value = control.checked, enabled = enabled, role = Role.Switch, onValueChange = control.onChange)
        else -> if (onClick != null && enabled) Modifier.clickable(onClick = onClick) else Modifier
    }
    Row(
        modifier.fillMaxWidth().then(rowMod).heightIn(min = if (summary != null) Size.listTwoLine else Size.listOneLine)
            .padding(horizontal = Space.s16, vertical = Space.s12).alpha(if (enabled) 1f else Opacity.disabledContent),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.s16),
    ) {
        if (icon != null) {
            // The icon alone, no tile behind it; the 32dp box keeps titles aligned between rows.
            Box(Modifier.size(Size.iconTile), contentAlignment = Alignment.Center) {
                Icon(icon, contentDescription = null, tint = if (danger) cs.error else cs.onSurfaceVariant, modifier = Modifier.size(Size.iconRow))
            }
        }
        Column(Modifier.weight(1f)) {
            Text(title, style = RowTitle, color = titleColor)
            if (summary != null) Text(summary, style = MaterialTheme.typography.bodySmall.tabular, color = cs.onSurfaceVariant)
        }
        when (control) {
            is SettingControl.Toggle -> Switch(
                checked = control.checked, onCheckedChange = null, enabled = enabled,
                colors = SwitchDefaults.colors(
                    checkedTrackColor = cs.primary, checkedThumbColor = Ds.lab.knob, checkedBorderColor = cs.primary,
                    uncheckedTrackColor = Ds.lab.fill, uncheckedThumbColor = cs.outline, uncheckedBorderColor = cs.outline,
                ),
            )
            // Capped so a long value ("Every 5 minutes") wraps in its own column instead of pushing the title onto two lines.
            is SettingControl.Value -> Text(
                control.text,
                style = MaterialTheme.typography.bodyMedium,
                color = cs.onSurfaceVariant,
                textAlign = TextAlign.End,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.widthIn(max = Size.settingValueMax),
            )
            SettingControl.Chevron -> Icon(Sym.ChevronRight, contentDescription = null, tint = cs.outline)
            SettingControl.None -> {}
        }
    }
}

/** Small grey header above a grouped section, aligned with the row text (label-medium). */
@Composable
fun SectionHeader(text: String, modifier: Modifier = Modifier, action: (@Composable () -> Unit)? = null) {
    Row(modifier.fillMaxWidth().padding(start = Space.s32, end = Space.s16, top = Space.s24, bottom = Space.s8), verticalAlignment = Alignment.CenterVertically) {
        Text(text, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.weight(1f).semantics { heading() })
        if (action != null) action()
    }
}

/** 0.5dp hairline in the separator colour. */
@Composable
fun DsDivider(modifier: Modifier = Modifier) {
    Box(modifier.fillMaxWidth().padding(horizontal = Space.s16).height(0.5.dp).background(Ds.lab.separator))
}

// ------------------------------------------------------------------ text field

/** Filled field for names and patterns. Errors say what to do ("Give this pair a name"). */
@Composable
fun DsTextField(
    value: String,
    onValueChange: (String) -> Unit,
    label: String,
    modifier: Modifier = Modifier,
    helper: String? = null,
    error: String? = null,
    mono: Boolean = false,
    leading: ImageVector? = null,
    trailing: (@Composable () -> Unit)? = null,
    singleLine: Boolean = true,
    minLines: Int = 1,
    keyboardOptions: KeyboardOptions = KeyboardOptions.Default,
    keyboardActions: KeyboardActions = KeyboardActions.Default,
) {
    val cs = MaterialTheme.colorScheme
    TextField(
        value = value,
        onValueChange = onValueChange,
        modifier = modifier.fillMaxWidth(),
        label = { Text(label) },
        leadingIcon = leading?.let { { Icon(it, contentDescription = null) } },
        trailingIcon = trailing,
        isError = error != null,
        supportingText = (error ?: helper)?.let { msg ->
            {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    if (error != null) Icon(Sym.ErrorFill, contentDescription = null, modifier = Modifier.size(14.dp))
                    Text(msg)
                }
            }
        },
        textStyle = if (mono) MonoMedium.copy(color = cs.onSurface) else MaterialTheme.typography.bodyLarge,
        singleLine = singleLine,
        minLines = minLines,
        keyboardOptions = keyboardOptions,
        keyboardActions = keyboardActions,
        shape = MaterialTheme.shapes.medium,
        colors = TextFieldDefaults.colors(
            focusedContainerColor = Ds.lab.fill, unfocusedContainerColor = Ds.lab.fill, errorContainerColor = Ds.lab.fill, disabledContainerColor = Ds.lab.fill,
            focusedIndicatorColor = Color.Transparent, unfocusedIndicatorColor = Color.Transparent, errorIndicatorColor = Color.Transparent, disabledIndicatorColor = Color.Transparent,
        ),
    )
}

// ------------------------------------------------------------------ dialog

/**
 * Only irreversible actions and large batches reach a dialog (Delete forever, Disconnect). The confirm
 * button repeats the verb from the title; the body says where files are removed from.
 */
@Composable
fun DsDialog(
    title: String,
    body: String,
    confirmLabel: String,
    onConfirm: () -> Unit,
    onDismiss: () -> Unit,
    icon: ImageVector? = null,
    danger: Boolean = false,
    dismissLabel: String = stringResource(R.string.common_cancel),
) {
    val cs = MaterialTheme.colorScheme
    AlertDialog(
        onDismissRequest = onDismiss,
        icon = icon?.let { { Icon(it, contentDescription = null, tint = if (danger) cs.error else cs.secondary) } },
        title = { Text(title, style = MaterialTheme.typography.headlineSmall) },
        text = { Text(body, style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant) },
        confirmButton = {
            Button(onClick = onConfirm, colors = if (danger) ButtonDefaults.buttonColors(containerColor = cs.error, contentColor = cs.onError) else ButtonDefaults.buttonColors()) { Text(confirmLabel) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(dismissLabel) } },
        containerColor = Ds.lab.cardRaised,
        shape = MaterialTheme.shapes.extraLarge,
    )
}

// ------------------------------------------------------------------ onboarding

enum class PermissionState { Needed, Next, Granted }

/** One permission: what it is, why in one line, its state. `Next` is the step to do now. Optional never blocks. */
@Composable
fun PermissionStep(icon: ImageVector, title: String, why: String, state: PermissionState, onAllow: () -> Unit, modifier: Modifier = Modifier, optional: Boolean = false, cta: String = stringResource(R.string.components_permission_allow), enabled: Boolean = true) {
    val cs = MaterialTheme.colorScheme
    val shape = MaterialTheme.shapes.large
    val granted = state == PermissionState.Granted
    val described = stringResource(R.string.components_permission_button_described, cta, title)
    Row(
        modifier.fillMaxWidth().clip(shape).background(Ds.lab.card)
            .then(if (state == PermissionState.Next) Modifier.border(2.dp, cs.primary, shape) else Modifier)
            .padding(Space.s16),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.s12),
    ) {
        Box(
            Modifier.align(Alignment.Top).size(40.dp).clip(CircleShape).background(if (granted) Ds.status.synced.container else cs.primaryContainer),
            contentAlignment = Alignment.Center,
        ) { Icon(icon, contentDescription = null, tint = if (granted) Ds.status.synced.onContainer else cs.primary) }
        // Title, chip, reason and button stack in the card's full width: beside the text, the button
        // squeezed the reason to a third of the card and the "Optional" chip to a letter a line (UX audit 2026-10).
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            FlowRow(horizontalArrangement = Arrangement.spacedBy(Space.s8), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(title, style = MaterialTheme.typography.titleSmall, color = cs.onSurface, modifier = Modifier.align(Alignment.CenterVertically))
                if (optional) {
                    Text(stringResource(R.string.common_optional), style = MaterialTheme.typography.labelSmall, color = cs.onSurfaceVariant, maxLines = 1, softWrap = false,
                        modifier = Modifier.align(Alignment.CenterVertically).clip(CircleShape).background(Ds.lab.fill).padding(horizontal = 8.dp, vertical = 1.dp))
                }
            }
            Text(why, style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant)
            when (state) {
                PermissionState.Granted -> Row(Modifier.padding(top = Space.s4), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    Icon(Sym.CheckCircleFill, contentDescription = null, tint = Ds.status.synced.content, modifier = Modifier.size(20.dp))
                    Text(stringResource(R.string.components_permission_allowed), style = MaterialTheme.typography.labelMedium, color = Ds.status.synced.content)
                }
                // Named with the row's title: three buttons that all read "Allow" told TalkBack nothing (#150 M6).
                PermissionState.Next -> Button(onClick = onAllow, enabled = enabled, modifier = Modifier.padding(top = Space.s8).heightIn(min = 40.dp).semantics { contentDescription = described }, contentPadding = PaddingValues(horizontal = 16.dp)) { Text(cta) }
                PermissionState.Needed -> OutlinedButton(onClick = onAllow, enabled = enabled, modifier = Modifier.padding(top = Space.s8).heightIn(min = 40.dp).semantics { contentDescription = described }, contentPadding = PaddingValues(horizontal = 16.dp)) { Text(cta) }
            }
        }
    }
}

// ------------------------------------------------------------------ bottom action bar, FAB

/**
 * Bottom action bar on surface-container: at most two buttons, optionally one line of body-small
 * explaining what the primary action does. `plain` drops the ground (onboarding).
 */
@Composable
fun BottomActionBar(modifier: Modifier = Modifier, hint: String? = null, plain: Boolean = false, content: @Composable RowScope.() -> Unit) {
    Surface(modifier.fillMaxWidth(), color = MaterialTheme.colorScheme.surface) {
        Row(
            Modifier.navigationBarsPadding().padding(horizontal = Space.s16, vertical = Space.s12).padding(bottom = if (plain) Space.s4 else 0.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(Space.s8, Alignment.End),
        ) {
            if (hint != null) Text(hint, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.weight(1f))
            content()
        }
    }
}

/**
 * Extended FAB that collapses to its icon on scroll. 48dp tall ([Size.fab]) with a 20dp icon and
 * label-large text: about 15% smaller than Material's 56dp, still a full touch target (0.4.2).
 */
@Composable
fun DsFab(text: String, icon: ImageVector, expanded: Boolean, onClick: () -> Unit, modifier: Modifier = Modifier) {
    Surface(
        onClick = onClick,
        // Collapsed, the text leaves the tree: keep the button's name for TalkBack (E2E 2026-10).
        modifier = modifier.semantics { contentDescription = text; role = Role.Button },
        shape = CircleShape,
        color = MaterialTheme.colorScheme.primary,
        contentColor = MaterialTheme.colorScheme.onPrimary,
        shadowElevation = 6.dp,
    ) {
        Row(
            Modifier.height(Size.fab).defaultMinSize(minWidth = Size.fab).padding(horizontal = 14.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.Center,
        ) {
            Icon(icon, contentDescription = null, modifier = Modifier.size(Size.iconFab))
            AnimatedVisibility(expanded, enter = fadeIn() + expandHorizontally(), exit = fadeOut() + shrinkHorizontally()) {
                Text(text, style = MaterialTheme.typography.labelLarge, maxLines = 1, modifier = Modifier.padding(start = Space.s8, end = Space.s4))
            }
        }
    }
}

// ------------------------------------------------------------------ undo snackbar

/**
 * Undo snackbar for reversible actions: names what happened and where ("Restored 12 files to Camera"),
 * stays 10s, one at a time. Never for errors that need action (those go to Problems).
 */
suspend fun SnackbarHostState.showUndo(message: String, onUndo: () -> Unit, actionLabel: String = Strings.get(R.string.common_undo)) {
    currentSnackbarData?.dismiss()
    val result = withTimeoutOrNull(Motion.SnackbarUndoMs) {
        showSnackbar(message, actionLabel = actionLabel, withDismissAction = false, duration = SnackbarDuration.Indefinite)
    }
    if (result == null) currentSnackbarData?.dismiss()
    if (result == SnackbarResult.ActionPerformed) onUndo()
}

/** Plain confirmation without an action. */
suspend fun SnackbarHostState.showMessage(message: String) {
    currentSnackbarData?.dismiss()
    showSnackbar(message, duration = SnackbarDuration.Short)
}

/** No-op insets helper for nested scaffolds that must not re-apply system bar padding. */
val NoInsets = WindowInsets(0, 0, 0, 0)
