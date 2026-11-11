package com.almodhayan.drivesync.ui.devices

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Size
import com.almodhayan.drivesync.ui.theme.Sym
import uniffi.wasl_devices.DeviceInfo
import uniffi.wasl_devices.OfferInfo
import uniffi.wasl_devices.OfferStatus

// Introductions (docs/INTRODUCTIONS.md) on the phone: a device another one added waits as "new" until
// the person keeps it here; folders other devices offer are accepted (a new pair, with its preview) or
// declined; a shared pair's devices are chosen; a device just paired can be offered folders. Offers are
// data from other devices, shown as text and never acted on without a tap.

/** A device another one added, not kept on this phone yet: it gets nothing shared with all devices. */
fun DeviceInfo.isNew(): Boolean = !thisDevice && !known

/** The device that added [d], by name when this phone knows it. */
fun addedByName(d: DeviceInfo, all: List<DeviceInfo>): String? =
    d.addedBy?.let { by -> all.firstOrNull { it.id == by }?.name ?: by }

/** The offers still waiting for an answer. */
fun waiting(offers: List<OfferInfo>): List<OfferInfo> = offers.filter { it.status == OfferStatus.NEW }

/** "All my devices", or the names of the devices a pair is shared with. */
@Composable
fun sharedWithText(all: Boolean, ids: List<String>, devices: List<DeviceInfo>): String {
    val names = ids.map { id -> devices.firstOrNull { it.id == id }?.name ?: id }.joinToString(", ")
    return when {
        all && names.isNotEmpty() -> stringResource(R.string.intro_with_all_and, names)
        // #148: "all your devices" means nothing yet when there is no other device.
        all && devices.none { !it.thisDevice } -> stringResource(R.string.devices_pair_no_devices)
        all -> stringResource(R.string.intro_with_all)
        names.isNotEmpty() -> stringResource(R.string.intro_with_names, names)
        else -> stringResource(R.string.intro_with_none)
    }
}

/** Under a new device's name: who added it, and Keep beside the row's Remove. */
@Composable
fun NewDeviceNote(name: String, by: String?, onKeep: () -> Unit) {
    val cs = MaterialTheme.colorScheme
    Column(
        Modifier.fillMaxWidth().padding(start = Space.s16 + 40.dp + Space.s16, end = Space.s16, bottom = Space.s12),
        verticalArrangement = Arrangement.spacedBy(Space.s8),
    ) {
        Text(
            if (by != null) stringResource(R.string.intro_added_by, by) else stringResource(R.string.intro_added_elsewhere),
            style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant,
        )
        Button(onClick = onKeep) {
            Icon(Sym.Check, contentDescription = null, modifier = Modifier.size(18.dp))
            Text(stringResource(R.string.intro_keep_for, name), modifier = Modifier.padding(start = Space.s8))
        }
    }
}

/** "Laptop offers Projects", its size, Accept and Decline. */
@Composable
fun OfferRow(o: OfferInfo, onAccept: () -> Unit, onDecline: () -> Unit) {
    val cs = MaterialTheme.colorScheme
    Column(Modifier.fillMaxWidth().padding(horizontal = Space.s16, vertical = Space.s12), verticalArrangement = Arrangement.spacedBy(Space.s8)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s16)) {
            Icon(Sym.Folder, contentDescription = null, tint = cs.onSurfaceVariant)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(stringResource(R.string.intro_offers, o.fromName, o.name), style = MaterialTheme.typography.bodyLarge, color = cs.onSurface)
                Text(
                    pluralStringResource(R.plurals.intro_offer_size, o.files.toInt().coerceAtLeast(0), o.files.toLong(), Fmt.bytes(o.bytes.toLong())),
                    style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant,
                )
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(Space.s8), modifier = Modifier.padding(start = Size.icon + Space.s16)) {
            Button(onClick = onAccept) { Text(stringResource(R.string.intro_accept)) }
            OutlinedButton(onClick = onDecline) { Text(stringResource(R.string.intro_decline)) }
        }
    }
}

/** One line in a [ChoiceDialog]. */
data class Choice(val value: String, val label: String, val hint: String? = null, val checked: Boolean = false)

/** A dialog of checkboxes; [onConfirm] gets the values ticked. */
@Composable
fun ChoiceDialog(title: String, body: String, choices: List<Choice>, confirmLabel: String, onConfirm: (Set<String>) -> Unit, onDismiss: () -> Unit) {
    val ticked = remember(choices) { mutableStateListOf(*choices.filter { it.checked }.map { it.value }.toTypedArray()) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title, style = MaterialTheme.typography.headlineSmall) },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(Space.s4)) {
                Text(body, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(bottom = Space.s8))
                choices.forEach { c ->
                    val on = c.value in ticked
                    Row(
                        Modifier.fillMaxWidth()
                            .toggleable(value = on, role = Role.Checkbox, onValueChange = { if (it) ticked.add(c.value) else ticked.remove(c.value) })
                            .padding(vertical = Space.s4),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(Space.s8),
                    ) {
                        Checkbox(checked = on, onCheckedChange = null)
                        Column {
                            Text(c.label, style = MaterialTheme.typography.bodyLarge)
                            c.hint?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
                        }
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = { onConfirm(ticked.toSet()) }) { Text(confirmLabel) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.common_cancel)) } },
    )
}

/** The value standing for "All my devices" in the device chooser. */
const val ALL_DEVICES = "*"
