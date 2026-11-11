@file:OptIn(androidx.compose.ui.ExperimentalComposeUiApi::class)

package com.almodhayan.drivesync.ui.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.testTagsAsResourceId
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.DialogProperties
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.ui.components.Tag
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym

/*
 * Dialog building blocks for Settings, Diagnostics, About, Backup, Vaults and the share sheet.
 * They follow the DsDialog look (surface-container-high, shape-xl, headline-small title, verb-first
 * buttons) but take content, which DsDialog doesn't.
 */

/**
 * A dialog with free content (choices, a switch, fields). Recoverable choices and forms only:
 * irreversible confirmations use [com.almodhayan.drivesync.ui.components.DsDialog].
 * Pass `confirmLabel = null` for a dialog that only closes.
 */
@Composable
fun ContentDialog(
    title: String,
    onDismiss: () -> Unit,
    confirmLabel: String? = null,
    onConfirm: () -> Unit = {},
    confirmEnabled: Boolean = true,
    danger: Boolean = false,
    dismissLabel: String? = stringResource(R.string.common_cancel),
    icon: ImageVector? = null,
    dismissOnOutside: Boolean = true,
    content: @Composable ColumnScope.() -> Unit,
) {
    val cs = MaterialTheme.colorScheme
    AlertDialog(
        onDismissRequest = onDismiss,
        modifier = Modifier.semantics { testTagsAsResourceId = true },   // dialogs are their own window
        icon = icon?.let { ic -> { Icon(ic, contentDescription = null, tint = if (danger) cs.error else cs.secondary) } },
        title = { Text(title, style = MaterialTheme.typography.headlineSmall, color = cs.onSurface) },
        text = {
            Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(Space.s12)) { content() }
        },
        confirmButton = {
            if (confirmLabel != null) {
                Button(
                    onClick = onConfirm,
                    enabled = confirmEnabled,
                    colors = if (danger) ButtonDefaults.buttonColors(containerColor = cs.error, contentColor = cs.onError) else ButtonDefaults.buttonColors(),
                ) { Text(confirmLabel) }
            }
        },
        dismissButton = dismissLabel?.let { label -> { TextButton(onClick = onDismiss) { Text(label) } } },
        containerColor = cs.surfaceContainerHigh,
        shape = MaterialTheme.shapes.extraLarge,
        properties = DialogProperties(dismissOnClickOutside = dismissOnOutside, dismissOnBackPress = dismissOnOutside),
    )
}

/** One option of a [ChoiceDialog]. */
@Immutable
data class Choice<T>(val value: T, val label: String, val description: String? = null, val recommended: Boolean = false)

/** Radio list. Choosing an option applies it and closes the dialog; Cancel keeps the current value. */
@Composable
fun <T> ChoiceDialog(
    title: String,
    choices: List<Choice<T>>,
    selected: T,
    onSelect: (T) -> Unit,
    onDismiss: () -> Unit,
    intro: String? = null,
) {
    ContentDialog(title = title, onDismiss = onDismiss) {
        if (intro != null) Text(intro, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Column(Modifier.selectableGroup()) {
            choices.forEach { c ->
                ChoiceOption(
                    label = c.label,
                    description = c.description,
                    selected = c.value == selected,
                    recommended = c.recommended,
                    onClick = { onSelect(c.value); onDismiss() },
                )
            }
        }
    }
}

/** A radio row for dialogs: Material Symbols radio, label, optional description and "Recommended" tag. */
@Composable
fun ChoiceOption(label: String, selected: Boolean, onClick: () -> Unit, description: String? = null, recommended: Boolean = false) {
    val cs = MaterialTheme.colorScheme
    Row(
        Modifier.fillMaxWidth().clip(MaterialTheme.shapes.small)
            .selectable(selected = selected, role = Role.RadioButton, onClick = onClick)
            .heightIn(min = 48.dp)
            .padding(vertical = Space.s8),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.s12),
    ) {
        Icon(
            if (selected) Sym.RadioButtonChecked else Sym.RadioButtonUnchecked,
            contentDescription = null,
            tint = if (selected) cs.primary else cs.onSurfaceVariant,
            modifier = Modifier.align(if (description != null) Alignment.Top else Alignment.CenterVertically),
        )
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s8)) {
                Text(label, style = MaterialTheme.typography.bodyLarge, color = cs.onSurface, modifier = Modifier.weight(1f, fill = false))
                if (recommended) Tag(stringResource(R.string.common_recommended))
            }
            if (description != null) Text(description, style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant)
        }
    }
}

/** A switch row for dialogs. The whole row toggles; the text states what it does. */
@Composable
fun DialogSwitch(title: String, checked: Boolean, onChange: (Boolean) -> Unit, summary: String? = null) {
    val cs = MaterialTheme.colorScheme
    Row(
        Modifier.fillMaxWidth().clip(MaterialTheme.shapes.small)
            .toggleable(value = checked, role = Role.Switch, onValueChange = onChange)
            .heightIn(min = 56.dp)
            .padding(vertical = Space.s8),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.s16),
    ) {
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.bodyLarge, color = cs.onSurface)
            if (summary != null) Text(summary, style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant)
        }
        Switch(
            checked = checked,
            onCheckedChange = null,
            thumbContent = if (checked) { { Icon(Sym.Check, contentDescription = null, modifier = Modifier.size(16.dp)) } } else null,
        )
    }
}

/** Options plus the current value when it isn't one of them (values set by older versions stay visible). */
internal fun withCurrent(options: List<Int>, current: Int): List<Int> = (options + current).distinct().sorted()
