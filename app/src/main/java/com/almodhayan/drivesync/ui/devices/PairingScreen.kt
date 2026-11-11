package com.almodhayan.drivesync.ui.devices

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.common.AppScreen
import com.almodhayan.drivesync.ui.components.DsTextField
import com.almodhayan.drivesync.ui.components.NavIcon
import com.almodhayan.drivesync.ui.components.PermissionState
import com.almodhayan.drivesync.ui.components.PermissionStep
import com.almodhayan.drivesync.ui.components.showMessage
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.SecureContent
import com.almodhayan.drivesync.ui.components.copySensitiveText
import com.almodhayan.drivesync.ui.theme.Ds
import com.almodhayan.drivesync.ui.theme.MonoMedium
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import kotlinx.coroutines.launch
import uniffi.wasl_devices.PairingStage
import uniffi.wasl_devices.PairingStatus

/** Add a device (show the code as a QR and text) or join one (scan or paste), then the four words. */
@Composable
fun PairingScreen(nav: AppNav, vm: PairingViewModel = hiltViewModel()) {
    val open by vm.open.collectAsStateWithLifecycle()
    val status by vm.status.collectAsStateWithLifecycle()
    val started by vm.started.collectAsStateWithLifecycle()
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    LaunchedEffect(vm) { vm.messages.collect { scope.launch { snackbar.showMessage(it) } } }
    // The code and the four words let another device join the group (security review 2026-10).
    SecureContent()
    // Adding a device opens a code as soon as the phone takes part.
    LaunchedEffect(open, started) { if (open && !started && !vm.joining) vm.invite() }

    AppScreen(
        title = stringResource(if (vm.joining) R.string.devices_pairing_join_title else R.string.devices_pairing_add_title),
        onBack = { nav.back() },
        navIcon = NavIcon.Close,
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        Column(
            Modifier.fillMaxSize().padding(padding).verticalScroll(rememberScrollState()).padding(Space.s16),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(Space.s16),
        ) {
            val st = status?.takeIf { started }
            when {
                !open -> Waiting(stringResource(R.string.devices_joining))
                st == null || st.stage == PairingStage.IDLE ->
                    if (vm.joining) JoinInput(onCode = vm::join) else Waiting(stringResource(R.string.devices_pairing_waiting_scan))
                st.stage == PairingStage.WAITING && st.inviting && st.code != null -> ShowCode(st)
                st.stage == PairingStage.WAITING -> Waiting(stringResource(R.string.devices_pairing_reaching))
                st.stage == PairingStage.CONFIRM -> Confirm(st, onAnswer = vm::confirm)
                st.stage == PairingStage.PAIRED -> {
                    val peer = st.peerId
                    Result(
                        ok = true,
                        title = st.peerName?.let { stringResource(R.string.devices_pairing_paired, it) } ?: stringResource(R.string.devices_pairing_paired_unknown),
                        body = stringResource(if (st.inviting) R.string.intro_paired_next else R.string.devices_pairing_paired_body),
                        action = stringResource(R.string.devices_pairing_done), onAction = { nav.back() },
                    )
                    // The phone that showed the code may introduce the new device to its folders: none ticked (§3.4).
                    if (st.inviting && peer != null) {
                        OutlinedButton(onClick = { vm.loadOfferable() }) { Text(stringResource(R.string.intro_offer_button, st.peerName ?: "")) }
                        val offerable by vm.offerable.collectAsStateWithLifecycle()
                        offerable?.let { pairs ->
                            ChoiceDialog(
                                title = stringResource(R.string.intro_offer_title, st.peerName ?: ""),
                                body = stringResource(if (pairs.isEmpty()) R.string.intro_offer_none else R.string.intro_offer_body, st.peerName ?: ""),
                                choices = pairs.map { Choice(it.first, it.second) },
                                confirmLabel = stringResource(R.string.intro_offer_confirm),
                                onConfirm = { picked -> vm.offerTo(peer, st.peerName ?: "", picked) },
                                onDismiss = { vm.closeOfferable() },
                            )
                        }
                    }
                }
                else -> Result(
                    ok = false,
                    title = stringResource(R.string.devices_pairing_failed),
                    body = st.message ?: stringResource(R.string.devices_pairing_failed_body),
                    action = stringResource(R.string.devices_pairing_try_again), onAction = vm::again,
                )
            }
        }
    }
}

@Composable
private fun Waiting(text: String) {
    Column(Modifier.fillMaxWidth().padding(vertical = Space.s48), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(Space.s16)) {
        CircularProgressIndicator(Modifier.size(32.dp), strokeWidth = 3.dp)
        Text(text, style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = TextAlign.Center)
    }
}

/** The inviter's code: a QR to scan and the same code as text, left to right in every language. */
@Composable
private fun ShowCode(st: PairingStatus) {
    val code = st.code ?: return
    val context = LocalContext.current
    val cs = MaterialTheme.colorScheme
    Text(stringResource(R.string.devices_pairing_show_code), style = MaterialTheme.typography.bodyLarge, color = cs.onSurface, textAlign = TextAlign.Center)
    val matrix = remember(code) { runCatching { Qr.encode(code) }.getOrNull() }
    if (matrix != null) {
        // Always dark on white: scanners need the contrast in dark mode too.
        Canvas(
            Modifier.widthIn(max = 280.dp).fillMaxWidth().aspectRatio(1f).clip(MaterialTheme.shapes.medium).background(Color.White).testTag("pairing-qr")
                .semantics { contentDescription = code },
        ) {
            val cell = size.width / matrix.width
            for (y in 0 until matrix.height) for (x in 0 until matrix.width) {
                if (matrix.get(x, y)) drawRect(Color.Black, Offset(x * cell, y * cell), Size(cell + 0.5f, cell + 0.5f))
            }
        }
    }
    Text(stringResource(R.string.devices_pairing_code_label), style = MaterialTheme.typography.labelMedium, color = cs.onSurfaceVariant)
    CompositionLocalProvider(LocalLayoutDirection provides LayoutDirection.Ltr) {
        SelectionContainer {
            Text(
                code, style = MonoMedium, color = cs.onSurface, textAlign = TextAlign.Center,
                modifier = Modifier.fillMaxWidth().clip(MaterialTheme.shapes.small).background(Ds.lab.fill).padding(Space.s12).testTag("pairing-code"),
            )
        }
    }
    TextButton(onClick = { copySensitiveText(context, "Wasl pairing code", code) }) {
        Icon(Sym.ContentCopy, contentDescription = null, modifier = Modifier.size(18.dp))
        Text(stringResource(R.string.devices_pairing_copy_code), modifier = Modifier.padding(start = Space.s8))
    }
    st.expiresAt?.let { Text(stringResource(R.string.devices_pairing_expires, Fmt.clock(it.toLong() * 1000)), style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant) }
    Waiting(stringResource(R.string.devices_pairing_waiting_scan))
}

/** The joiner: scan with the camera (asked for with a reason) or type/paste the code. */
@Composable
private fun JoinInput(onCode: (String) -> Unit) {
    val context = LocalContext.current
    val cs = MaterialTheme.colorScheme
    var granted by remember { mutableStateOf(ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) }
    var denied by rememberSaveable { mutableStateOf(false) }
    var text by rememberSaveable { mutableStateOf("") }
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok -> granted = ok; denied = !ok }

    if (granted) {
        Text(stringResource(R.string.devices_pairing_scan_hint), style = MaterialTheme.typography.bodyLarge, color = cs.onSurface, textAlign = TextAlign.Center)
        Box(Modifier.widthIn(max = 320.dp).fillMaxWidth().aspectRatio(1f).clip(MaterialTheme.shapes.large).background(Color.Black)) {
            QrScanner(onCode = onCode, modifier = Modifier.fillMaxSize())
        }
    } else {
        PermissionStep(
            icon = Sym.Visibility,
            title = stringResource(R.string.devices_pairing_camera_title),
            why = stringResource(if (denied) R.string.devices_pairing_camera_denied else R.string.devices_pairing_camera_why),
            state = PermissionState.Next,
            onAllow = { launcher.launch(Manifest.permission.CAMERA) },
            cta = stringResource(R.string.devices_pairing_camera_allow),
        )
    }
    Spacer(Modifier.height(Space.s8))
    CompositionLocalProvider(LocalLayoutDirection provides LayoutDirection.Ltr) {
        DsTextField(value = text, onValueChange = { text = it }, label = stringResource(R.string.devices_pairing_paste_label), mono = true, singleLine = false, modifier = Modifier.testTag("pairing-paste"))
    }
    Button(onClick = { onCode(text) }, enabled = text.isNotBlank(), modifier = Modifier.fillMaxWidth()) { Text(stringResource(R.string.devices_pairing_join_button)) }
}

/** The four words, large, always left to right in their order, and "Same on the other device?". */
@Composable
private fun Confirm(st: PairingStatus, onAnswer: (Boolean) -> Unit) {
    val cs = MaterialTheme.colorScheme
    val peer = st.peerName
    Text(stringResource(R.string.devices_pairing_words_title), style = MaterialTheme.typography.headlineSmall, color = cs.onSurface, textAlign = TextAlign.Center, modifier = Modifier.semantics { heading() })
    Text(
        peer?.let { stringResource(R.string.devices_pairing_words_body, it) } ?: stringResource(R.string.devices_pairing_words_body_unknown),
        style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant, textAlign = TextAlign.Center,
    )
    Words(st.words)
    if (st.confirmedHere) {
        Waiting(peer?.let { stringResource(R.string.devices_pairing_waiting_confirm, it) } ?: stringResource(R.string.devices_pairing_waiting_confirm_unknown))
    } else {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Space.s12)) {
            OutlinedButton(onClick = { onAnswer(false) }, modifier = Modifier.weight(1f).testTag("pairing-no")) { Text(stringResource(R.string.devices_pairing_no)) }
            Button(onClick = { onAnswer(true) }, modifier = Modifier.weight(1f).testTag("pairing-yes")) { Text(stringResource(R.string.devices_pairing_yes)) }
        }
    }
}

@Composable
fun Words(words: List<String>) {
    val cs = MaterialTheme.colorScheme
    val label = stringResource(R.string.devices_pairing_words_label, words.joinToString(" "))
    // The words are English on both devices: the same order on both screens, whatever the language.
    CompositionLocalProvider(LocalLayoutDirection provides LayoutDirection.Ltr) {
        Column(
            Modifier.fillMaxWidth().clearAndSetSemantics { contentDescription = label }.testTag("pairing-words"),
            verticalArrangement = Arrangement.spacedBy(Space.s8),
        ) {
            words.chunked(2).forEachIndexed { row, pair ->
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Space.s8)) {
                    pair.forEachIndexed { i, w ->
                        Row(
                            Modifier.weight(1f).clip(MaterialTheme.shapes.medium).background(Ds.lab.card).padding(horizontal = Space.s12, vertical = Space.s16),
                            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s8),
                        ) {
                            Text("${row * 2 + i + 1}", style = MaterialTheme.typography.labelMedium, color = cs.onSurfaceVariant)
                            Text(w, style = MaterialTheme.typography.headlineSmall, color = cs.onSurface, maxLines = 1)
                        }
                    }
                    if (pair.size == 1) Spacer(Modifier.weight(1f))
                }
            }
        }
    }
}

@Composable
private fun Result(ok: Boolean, title: String, body: String, action: String, onAction: () -> Unit) {
    val cs = MaterialTheme.colorScheme
    Column(Modifier.fillMaxWidth().padding(vertical = Space.s32), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(Space.s12)) {
        Icon(
            if (ok) Sym.CheckCircleFill else Sym.ErrorFill, contentDescription = null, modifier = Modifier.size(56.dp),
            tint = if (ok) Ds.status.synced.content else cs.error,
        )
        Text(title, style = MaterialTheme.typography.headlineSmall, color = cs.onSurface, textAlign = TextAlign.Center, modifier = Modifier.semantics { heading() })
        Text(body, style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant, textAlign = TextAlign.Center)
        Button(onClick = onAction) { Text(action) }
    }
}
