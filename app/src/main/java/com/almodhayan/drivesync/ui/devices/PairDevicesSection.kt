package com.almodhayan.drivesync.ui.devices

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.LayoutDirection
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.ui.components.DsTextField
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.GroupItem
import com.almodhayan.drivesync.ui.components.GroupPos
import com.almodhayan.drivesync.ui.components.SectionHeader
import com.almodhayan.drivesync.ui.components.SettingControl
import com.almodhayan.drivesync.ui.components.SettingRow
import com.almodhayan.drivesync.ui.components.copySensitiveText
import com.almodhayan.drivesync.ui.settings.ContentDialog
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import com.kahflabs.wasl.ffi.core.Route

/**
 * A pair's Devices section: "Sync with my devices" (shares the pair with the group under a code),
 * its code to copy, "Use a code from another device", and the wait or last result with devices.
 */
@Composable
fun PairDevicesSection(onMessage: (String) -> Unit, vm: PairDevicesViewModel = hiltViewModel()) {
    val ui by vm.ui.collectAsStateWithLifecycle()
    val context = LocalContext.current
    var entering by rememberSaveable { mutableStateOf(false) }
    var choosing by rememberSaveable { mutableStateOf(false) }
    LaunchedEffect(vm) { vm.messages.collect { onMessage(it) } }
    // A pair without a Drive folder keeps route "devices" when unshared; its code says whether it is shared.
    val shared = if (ui.open) ui.code != null else ui.route != Route.DRIVE
    val copied = stringResource(R.string.devices_pair_copied)

    Column {
        SectionHeader(stringResource(R.string.devices_pair_section))
        GroupItem(GroupPos.First) {
            SettingRow(
                title = stringResource(R.string.devices_pair_switch), icon = Sym.SyncAlt,
                summary = when {
                    !ui.open -> ui.waiting ?: stringResource(R.string.devices_pair_unavailable)
                    shared && ui.code != null -> stringResource(R.string.devices_pair_code, ltr(ui.code!!))
                    shared -> null
                    ui.hasDrive -> stringResource(R.string.devices_pair_off)
                    else -> stringResource(R.string.devices_pair_off_no_drive)
                },
                control = SettingControl.Toggle(shared) { vm.setShared(it) },
                enabled = ui.open,
                modifier = Modifier.testTag("pair-devices-switch"),
            )
        }
        // Who the pair is shared with, and choosing them (docs/INTRODUCTIONS.md §3.2).
        val sharing = ui.sharing
        if (shared && ui.open && sharing != null) {
            GroupItem(GroupPos.Middle) {
                SettingRow(
                    title = stringResource(R.string.intro_choose), icon = Sym.Mobile,
                    summary = sharedWithText(sharing.allDevices, sharing.devices, ui.others) +
                        (if (ui.waitingFor.isEmpty()) "" else " · " + stringResource(R.string.devices_waiting_for, ui.waitingFor.joinToString(", "))),
                    onClick = { choosing = true },
                )
            }
        }
        if (shared && ui.open && ui.code != null) {
            GroupItem(GroupPos.Middle) {
                SettingRow(
                    title = stringResource(R.string.devices_pairing_copy_code), icon = Sym.ContentCopy,
                    control = SettingControl.None,
                    onClick = { copySensitiveText(context, "Wasl share code", ui.code!!); onMessage(copied) },
                )
            }
        }
        GroupItem(if (ui.last == null) GroupPos.Last else GroupPos.Middle) {
            SettingRow(
                title = stringResource(R.string.devices_pair_use_code), icon = Sym.Link,
                summary = stringResource(R.string.devices_pair_use_code_summary),
                enabled = ui.open,
                onClick = { entering = true },
            )
        }
        ui.last?.let { last ->
            GroupItem(GroupPos.Last) {
                val stamp = stringResource(if (last.ok) R.string.devices_pair_last_ok else R.string.devices_pair_last_failed, Fmt.relativeLower(last.at))
                // A skipped run synced nothing: it says why, never "Synced with devices" (#148).
                val text = when {
                    last.noPeer -> stringResource(R.string.devices_pair_no_peer)
                    last.skipped -> last.message.orEmpty()
                    else -> stamp
                }
                SettingRow(
                    title = text,
                    icon = when {
                        last.skipped -> Sym.Info
                        last.ok -> Sym.TaskAlt
                        else -> Sym.SyncProblem
                    },
                    summary = last.message?.takeIf { !last.ok && !last.skipped },
                    control = SettingControl.None,
                )
            }
        }
    }

    val sharing = ui.sharing
    if (choosing && sharing != null) {
        val newHint = stringResource(R.string.intro_new)
        ChoiceDialog(
            title = stringResource(R.string.intro_choose_title),
            body = stringResource(R.string.intro_choose_body),
            choices = listOf(Choice(ALL_DEVICES, stringResource(R.string.intro_all_devices), stringResource(R.string.intro_all_devices_hint), sharing.allDevices)) +
                ui.others.map { d -> Choice(d.id, d.name, if (d.isNew()) newHint else null, d.id in sharing.devices) },
            confirmLabel = stringResource(R.string.intro_choose_confirm),
            onConfirm = { picked -> choosing = false; vm.shareWith(ALL_DEVICES in picked, (picked - ALL_DEVICES).toList()) },
            onDismiss = { choosing = false },
        )
    }

    if (entering) {
        var code by rememberSaveable { mutableStateOf("") }
        ContentDialog(
            title = stringResource(R.string.devices_pair_use_code),
            onDismiss = { entering = false },
            confirmLabel = stringResource(R.string.devices_pair_use),
            confirmEnabled = code.isNotBlank(),
            onConfirm = { entering = false; vm.useCode(code) },
            icon = Sym.Link,
        ) {
            Text(stringResource(R.string.devices_pair_use_code_body), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            CompositionLocalProvider(LocalLayoutDirection provides LayoutDirection.Ltr) {
                DsTextField(value = code, onValueChange = { code = it }, label = stringResource(R.string.devices_pair_code_label), mono = true, modifier = Modifier.padding(top = Space.s4))
            }
        }
    }
}
