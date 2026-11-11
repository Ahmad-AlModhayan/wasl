package com.almodhayan.drivesync.ui.devices

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.common.AppScreen
import com.almodhayan.drivesync.ui.components.Callout
import com.almodhayan.drivesync.ui.components.CalloutTone
import com.almodhayan.drivesync.ui.components.DsDialog
import com.almodhayan.drivesync.ui.components.GroupItem
import com.almodhayan.drivesync.ui.components.GroupPos
import com.almodhayan.drivesync.ui.components.SectionHeader
import com.almodhayan.drivesync.ui.components.SettingControl
import com.almodhayan.drivesync.ui.components.SettingRow
import com.almodhayan.drivesync.ui.components.showMessage
import com.almodhayan.drivesync.ui.settings.Automation
import com.almodhayan.drivesync.ui.theme.Ds
import com.almodhayan.drivesync.ui.theme.MonoSmall
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import kotlinx.coroutines.launch
import uniffi.wasl_devices.DeviceInfo
import uniffi.wasl_devices.LinkPath

/**
 * Devices (docs/DEVICE_SYNC.md, SRS FR-N12): this phone and the devices it syncs with, who is
 * connected and how, Add / Join, and when the phone takes part.
 */
@Composable
fun DevicesScreen(nav: AppNav, vm: DevicesViewModel = hiltViewModel()) {
    val ui by vm.ui.collectAsStateWithLifecycle()
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val scrollBehavior = TopAppBarDefaults.pinnedScrollBehavior()
    var removing by remember { mutableStateOf<DeviceInfo?>(null) }
    val copied = stringResource(R.string.devices_copied)
    LaunchedEffect(vm) { vm.messages.collect { scope.launch { snackbar.showMessage(it) } } }

    AppScreen(
        title = stringResource(R.string.devices_title),
        onBack = { nav.back() },
        scrollBehavior = scrollBehavior,
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        LazyColumn(
            Modifier.fillMaxSize().testTag("devices-list"),
            contentPadding = PaddingValues(top = padding.calculateTopPadding(), bottom = padding.calculateBottomPadding() + Space.s24),
        ) {
            ui.problem?.let { p ->
                item(key = "problem") {
                    Callout(p, tone = CalloutTone.Warning, modifier = Modifier.padding(start = Space.s16, end = Space.s16, top = Space.s8))
                }
            }

            item(key = "h-me") { SectionHeader(stringResource(R.string.devices_this_phone)) }
            item(key = "me") {
                GroupItem(GroupPos.Single) {
                    val me = ui.me
                    DeviceRow(
                        name = me?.name ?: stringResource(R.string.devices_this_phone),
                        id = me?.id,
                        status = when {
                            ui.open -> stringResource(R.string.devices_taking_part)
                            ui.problem != null -> stringResource(R.string.devices_not_taking_part)
                            else -> stringResource(R.string.devices_joining)
                        },
                        connected = ui.open,
                        phone = true,
                        onCopyId = me?.id?.let { id -> { Automation.copyToClipboard(context, "Wasl device ID", id); vm.say(copied) } },
                    )
                }
            }

            item(key = "h-others") { SectionHeader(stringResource(R.string.devices_your_devices)) }
            if (ui.others.isEmpty()) {
                item(key = "empty") {
                    GroupItem(GroupPos.Single) {
                        Column(Modifier.fillMaxWidth().padding(Space.s16), verticalArrangement = Arrangement.spacedBy(Space.s4)) {
                            Text(stringResource(R.string.devices_empty_title), style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.onSurface)
                            Text(stringResource(R.string.devices_empty_body), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                    }
                }
            } else {
                items(ui.others, key = { "d-${it.id}" }) { d ->
                    val pos = when {
                        ui.others.size == 1 -> GroupPos.Single
                        d == ui.others.first() -> GroupPos.First
                        d == ui.others.last() -> GroupPos.Last
                        else -> GroupPos.Middle
                    }
                    GroupItem(pos) {
                        Column {
                            DeviceRow(
                                name = d.name, id = d.id, status = if (d.isNew()) stringResource(R.string.intro_new) else connectionText(d), connected = d.connected, phone = false,
                                onRemove = { removing = d },
                            )
                            // Added by another device and not kept here yet (docs/INTRODUCTIONS.md §3.1).
                            if (d.isNew()) NewDeviceNote(d.name, addedByName(d, listOfNotNull(ui.me) + ui.others)) { vm.keep(d) }
                        }
                    }
                }
            }
            if (ui.offers.isNotEmpty()) {
                item(key = "h-offers") { SectionHeader(stringResource(R.string.intro_offers_title)) }
                items(ui.offers, key = { "o-${it.fromDevice}-${it.share}" }) { o ->
                    GroupItem(if (ui.offers.size == 1) GroupPos.Single else if (o == ui.offers.first()) GroupPos.First else if (o == ui.offers.last()) GroupPos.Last else GroupPos.Middle) {
                        OfferRow(o, onAccept = { nav.pairEditFromOffer(o.share, o.fromDevice, o.name) }, onDecline = { vm.decline(o) })
                    }
                }
            }
            item(key = "buttons") {
                Row(Modifier.fillMaxWidth().padding(horizontal = Space.s16, vertical = Space.s12), horizontalArrangement = Arrangement.spacedBy(Space.s12)) {
                    Button(onClick = { nav.devicePairing("add") }, enabled = ui.open, modifier = Modifier.weight(1f).testTag("devices-add")) {
                        Icon(Sym.Add, contentDescription = null, modifier = Modifier.size(18.dp))
                        Text(stringResource(R.string.devices_add), modifier = Modifier.padding(start = Space.s8))
                    }
                    OutlinedButton(onClick = { nav.devicePairing("join") }, enabled = ui.open, modifier = Modifier.weight(1f).testTag("devices-join")) {
                        Icon(Sym.Link, contentDescription = null, modifier = Modifier.size(18.dp))
                        Text(stringResource(R.string.devices_join), modifier = Modifier.padding(start = Space.s8))
                    }
                }
            }
            // Which button goes on which device was left to guess (UX audit 2026-10).
            item(key = "add-or-join") {
                Text(
                    stringResource(R.string.devices_add_or_join_hint), style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(horizontal = Space.s16),
                )
            }

            item(key = "h-when") { SectionHeader(stringResource(R.string.devices_section_when)) }
            item(key = "session") {
                GroupItem(GroupPos.First) {
                    SettingRow(
                        title = stringResource(R.string.devices_session), icon = Sym.SyncAlt,
                        summary = stringResource(if (ui.session) R.string.devices_session_on else R.string.devices_session_off),
                        control = SettingControl.Toggle(ui.session) { vm.setSession(it) },
                    )
                }
            }
            item(key = "charging") {
                GroupItem(GroupPos.Last) {
                    SettingRow(
                        title = stringResource(R.string.devices_charging), icon = Sym.Bolt,
                        summary = stringResource(if (ui.charging) R.string.devices_charging_on else R.string.devices_charging_off),
                        control = SettingControl.Toggle(ui.charging) { vm.setCharging(it) },
                    )
                }
            }
        }
    }

    removing?.let { d ->
        DsDialog(
            title = stringResource(R.string.devices_remove_title, d.name),
            body = stringResource(R.string.devices_remove_body, d.name),
            confirmLabel = stringResource(R.string.devices_remove),
            onConfirm = { removing = null; vm.remove(d) },
            onDismiss = { removing = null },
            icon = Sym.Delete,
            danger = true,
        )
    }
}

/** Connected (and how, FR-B7) or not. */
@Composable
fun connectionText(d: DeviceInfo): String = stringResource(
    when {
        !d.connected -> R.string.devices_not_connected
        d.path == LinkPath.LAN -> R.string.devices_connected_lan
        d.path == LinkPath.DIRECT -> R.string.devices_connected_direct
        d.path == LinkPath.PUNCHED -> R.string.devices_connected_punched
        else -> R.string.devices_connected
    },
)

@Composable
private fun DeviceRow(
    name: String,
    id: String?,
    status: String,
    connected: Boolean,
    phone: Boolean,
    onRemove: (() -> Unit)? = null,
    onCopyId: (() -> Unit)? = null,
) {
    val cs = MaterialTheme.colorScheme
    Row(
        Modifier.fillMaxWidth().padding(horizontal = Space.s16, vertical = Space.s12),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.s16),
    ) {
        Box(Modifier.size(40.dp), contentAlignment = Alignment.Center) {
            Icon(if (phone) Sym.Mobile else Sym.Dns, contentDescription = null, tint = cs.onSurfaceVariant)
        }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(name, style = MaterialTheme.typography.bodyLarge, color = cs.onSurface, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s4)) {
                Icon(
                    if (connected) Sym.CheckCircleFill else Sym.RadioButtonUnchecked, contentDescription = null,
                    tint = if (connected) Ds.status.synced.content else cs.outline, modifier = Modifier.size(12.dp),
                )
                Text(status, style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant)
            }
            // Device IDs are Latin and read left to right in every language. They wrap: on one line
            // the end was cut off on a phone (UX audit 2026-10).
            if (id != null) Text(ltr(id), style = MonoSmall, color = cs.onSurfaceVariant)
        }
        if (onCopyId != null) {
            IconButton(onClick = onCopyId) { Icon(Sym.ContentCopy, contentDescription = stringResource(R.string.devices_copy_id), tint = cs.onSurfaceVariant) }
        }
        if (onRemove != null) {
            IconButton(onClick = onRemove) { Icon(Sym.Delete, contentDescription = stringResource(R.string.devices_remove_for, name), tint = cs.onSurfaceVariant) }
        }
    }
}
