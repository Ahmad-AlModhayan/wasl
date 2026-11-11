@file:OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)

package com.almodhayan.drivesync.ui.vaults

import android.content.ClipboardManager
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.crypto.VaultManifestV2
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.common.AppScreen
import com.almodhayan.drivesync.ui.common.EmptyState
import com.almodhayan.drivesync.ui.common.KeyValueRow
import com.almodhayan.drivesync.ui.common.LoadingState
import com.almodhayan.drivesync.ui.components.Callout
import com.almodhayan.drivesync.ui.components.CalloutTone
import com.almodhayan.drivesync.ui.components.DsDialog
import com.almodhayan.drivesync.ui.components.DsFab
import com.almodhayan.drivesync.ui.components.DsTextField
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.MenuAction
import com.almodhayan.drivesync.ui.components.OverflowMenu
import com.almodhayan.drivesync.ui.components.showMessage
import com.almodhayan.drivesync.ui.components.SecureContent
import com.almodhayan.drivesync.ui.components.copySensitiveText
import com.almodhayan.drivesync.ui.settings.ContentDialog
import com.almodhayan.drivesync.ui.theme.Ds
import com.almodhayan.drivesync.ui.theme.MonoSmall
import com.almodhayan.drivesync.ui.theme.Size
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import kotlinx.coroutines.launch

private sealed class VaultDialog {
    data object Create : VaultDialog()
    data object Join : VaultDialog()
    data class Unlock(val row: VaultRow) : VaultDialog()
    data class Delete(val row: VaultRow) : VaultDialog()
    data class Manifest(val row: VaultRow, val text: String) : VaultDialog()
}

/** Vaults: pairs encrypted on the phone before upload. Create, join, unlock, lock, copy the manifest, delete. */
@Composable
fun VaultsScreen(nav: AppNav) {
    val vm: VaultsViewModel = hiltViewModel()
    val state by vm.state.collectAsStateWithLifecycle()
    var dialog by remember { mutableStateOf<VaultDialog?>(null) }
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    val listState = rememberLazyListState()
    val fabExpanded by remember { derivedStateOf { listState.firstVisibleItemIndex == 0 } }
    val scrollBehavior = TopAppBarDefaults.pinnedScrollBehavior()

    LaunchedEffect(state.message) {
        val m = state.message ?: return@LaunchedEffect
        snackbar.showMessage(m)
        vm.dismissMessage()
    }

    AppScreen(
        title = stringResource(R.string.vaults_title),
        onBack = { nav.back() },
        scrollBehavior = scrollBehavior,
        snackbarHost = { SnackbarHost(snackbar) },
        actions = {
            IconButton(onClick = { dialog = VaultDialog.Join }) { Icon(Sym.Link, contentDescription = stringResource(R.string.vaults_join_from_other_phone)) }
        },
        floatingActionButton = {
            if (!state.loading && state.rows.isNotEmpty()) DsFab(stringResource(R.string.vaults_new_vault), Sym.Add, expanded = fabExpanded, onClick = { dialog = VaultDialog.Create })
        },
    ) { padding ->
        when {
            state.loading -> Box(Modifier.fillMaxSize().padding(padding)) { LoadingState() }
            state.rows.isEmpty() -> EmptyState(
                icon = Sym.Lock,
                title = stringResource(R.string.vaults_empty_title),
                body = stringResource(R.string.vaults_empty_body),
                modifier = Modifier.padding(padding),
            ) {
                // FlowRow: with longer (Arabic) labels the second button wraps instead of overflowing.
                FlowRow(horizontalArrangement = Arrangement.spacedBy(Space.s8, Alignment.CenterHorizontally), verticalArrangement = Arrangement.spacedBy(Space.s8)) {
                    Button(onClick = { dialog = VaultDialog.Create }) { Text(stringResource(R.string.vaults_create_vault)) }
                    OutlinedButton(onClick = { dialog = VaultDialog.Join }) { Text(stringResource(R.string.vaults_join_a_vault)) }
                }
            }
            else -> LazyColumn(
                Modifier.fillMaxSize(),
                state = listState,
                contentPadding = PaddingValues(top = padding.calculateTopPadding() + Space.s8, bottom = padding.calculateBottomPadding() + Size.fab + Space.s32),
                verticalArrangement = Arrangement.spacedBy(Space.s12),
            ) {
                item(key = "intro") {
                    Text(
                        stringResource(R.string.vaults_intro),
                        style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(horizontal = Space.s16),
                    )
                }
                items(state.rows, key = { it.vault.id }) { row ->
                    VaultCard(
                        row = row,
                        busy = state.busy,
                        onUnlock = { dialog = VaultDialog.Unlock(row) },
                        onLock = { vm.lock(row.vault.id) },
                        onManifest = {
                            vm.manifest(row.vault.id) { text, error ->
                                if (text != null) dialog = VaultDialog.Manifest(row, text)
                                else if (error != null) scope.launch { snackbar.showMessage(error) }
                            }
                        },
                        onDelete = { dialog = VaultDialog.Delete(row) },
                        modifier = Modifier.padding(horizontal = Space.s16),
                    )
                }
            }
        }
    }

    val close = { dialog = null }
    when (val d = dialog) {
        null -> Unit
        VaultDialog.Create -> CreateVaultDialog(onDismiss = close, onCreate = vm::create)
        VaultDialog.Join -> JoinVaultDialog(onDismiss = close, onJoin = vm::join)
        is VaultDialog.Unlock -> UnlockDialog(d.row, onDismiss = close, onUnlock = { pass, cb -> vm.unlock(d.row.vault.id, pass, cb) })
        is VaultDialog.Manifest -> ManifestDialog(d.row, d.text, onDismiss = close)
        is VaultDialog.Delete -> if (d.row.pairsUsing > 0) {
            ContentDialog(title = stringResource(R.string.vaults_in_use_title), icon = Sym.Lock, onDismiss = close, dismissLabel = stringResource(R.string.common_close)) {
                Text(
                    pluralStringResource(R.plurals.vaults_in_use_body, d.row.pairsUsing, d.row.pairsUsing, d.row.vault.name),
                    style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        } else {
            DsDialog(
                title = stringResource(R.string.vaults_delete_title, d.row.vault.name),
                body = stringResource(R.string.vaults_delete_body),
                confirmLabel = stringResource(R.string.vaults_delete_vault),
                onConfirm = {
                    dialog = null
                    vm.delete(d.row.vault.id) { err -> if (err != null) scope.launch { snackbar.showMessage(err) } }
                },
                onDismiss = close,
                icon = Sym.Delete,
                danger = true,
            )
        }
    }
}

// ---------------------------------------------------------------------------------------------- list

@Composable
private fun VaultCard(
    row: VaultRow,
    busy: Boolean,
    onUnlock: () -> Unit,
    onLock: () -> Unit,
    onManifest: () -> Unit,
    onDelete: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val cs = MaterialTheme.colorScheme
    val shape = MaterialTheme.shapes.large
    Column(
        modifier.fillMaxWidth().clip(shape).background(Ds.lab.card).padding(Space.s16),
        verticalArrangement = Arrangement.spacedBy(Space.s12),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s12)) {
            Box(
                Modifier.size(40.dp).clip(CircleShape).background(if (row.unlocked) cs.primaryContainer else cs.surfaceContainerHighest),
                contentAlignment = Alignment.Center,
            ) {
                Icon(if (row.unlocked) Sym.Key else Sym.Lock, contentDescription = null, tint = if (row.unlocked) cs.onPrimaryContainer else cs.onSurfaceVariant)
            }
            Column(Modifier.weight(1f)) {
                Text(row.vault.name, style = MaterialTheme.typography.titleMedium, color = cs.onSurface)
                Text(stringResource(if (row.unlocked) R.string.vaults_unlocked_on_phone else R.string.vaults_locked), style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant)
            }
            OverflowMenu(
                listOf(
                    MenuAction(stringResource(R.string.vaults_show_manifest), Sym.Visibility) { onManifest() },
                    MenuAction(stringResource(R.string.vaults_delete_vault), Sym.Delete, destructive = true) { onDelete() },
                ),
                contentDescription = stringResource(R.string.components_more_for, row.vault.name),
            )
        }
        Column(verticalArrangement = Arrangement.spacedBy(Space.s4)) {
            KeyValueRow(stringResource(R.string.vaults_used_by), if (row.pairsUsing == 0) stringResource(R.string.vaults_no_pairs) else pluralStringResource(R.plurals.vaults_pairs, row.pairsUsing, row.pairsUsing))
            KeyValueRow(stringResource(R.string.vaults_created), Fmt.dayAndTime(row.vault.createdAt))
            KeyValueRow(
                stringResource(R.string.vaults_key),
                if (VaultManifestV2.isFormat21(row.vault.manifestJson)) stringResource(R.string.vaults_key_value_v21)
                else if (row.vault.version >= 2) stringResource(R.string.vaults_key_value_v2) else stringResource(R.string.vaults_key_value, Fmt.count(row.vault.iterations)),
            )
        }
        if (!row.unlocked && row.pairsUsing > 0) {
            Callout(text = pluralStringResource(R.plurals.vaults_locked_callout, row.pairsUsing, row.pairsUsing), tone = CalloutTone.Warning)
        }
        Row(horizontalArrangement = Arrangement.spacedBy(Space.s8)) {
            if (row.unlocked) OutlinedButton(onClick = onLock, enabled = !busy) { Text(stringResource(R.string.vaults_lock)) }
            else FilledTonalButton(onClick = onUnlock, enabled = !busy) { Text(stringResource(R.string.vaults_unlock)) }
        }
    }
}

// ---------------------------------------------------------------------------------------------- dialogs

/** Filled field like DsTextField, with the passphrase hidden until you show it. */
@Composable
private fun PassphraseField(
    value: String,
    onChange: (String) -> Unit,
    label: String,
    modifier: Modifier = Modifier,
    error: String? = null,
    helper: String? = null,
    imeAction: ImeAction = ImeAction.Next,
    enabled: Boolean = true,
) {
    val cs = MaterialTheme.colorScheme
    var visible by remember { mutableStateOf(false) }
    TextField(
        value = value,
        onValueChange = onChange,
        modifier = modifier.fillMaxWidth(),
        label = { Text(label) },
        singleLine = true,
        enabled = enabled,
        isError = error != null,
        visualTransformation = if (visible) VisualTransformation.None else PasswordVisualTransformation(),
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = imeAction),
        trailingIcon = {
            IconButton(onClick = { visible = !visible }) {
                Icon(if (visible) Sym.VisibilityOff else Sym.Visibility, contentDescription = stringResource(if (visible) R.string.vaults_hide_passphrase else R.string.vaults_show_passphrase))
            }
        },
        supportingText = (error ?: helper)?.let { msg ->
            {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s4)) {
                    if (error != null) Icon(Sym.ErrorFill, contentDescription = null, modifier = Modifier.size(14.dp))
                    Text(msg)
                }
            }
        },
        textStyle = MaterialTheme.typography.bodyLarge,
        colors = TextFieldDefaults.colors(focusedContainerColor = cs.surfaceContainerHighest, unfocusedContainerColor = cs.surfaceContainerHighest, errorContainerColor = cs.surfaceContainerHighest, disabledContainerColor = cs.surfaceContainerHighest),
    )
}

@Composable
private fun WorkingRow(text: String) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s12)) {
        CircularProgressIndicator(modifier = Modifier.size(20.dp), strokeWidth = 2.dp, color = Ds.status.syncing.content)
        Text(text, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
private fun ErrorLine(text: String) {
    Row(verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(Space.s8)) {
        Icon(Sym.ErrorFill, contentDescription = null, tint = Ds.status.failed.content, modifier = Modifier.padding(top = 2.dp).size(16.dp))
        Text(text, style = MaterialTheme.typography.bodyMedium, color = Ds.status.failed.content)
    }
}

@Composable
private fun CreateVaultDialog(onDismiss: () -> Unit, onCreate: (name: String, passphrase: String, onResult: ActionResult) -> Unit) {
    var name by remember { mutableStateOf("") }
    var pass by remember { mutableStateOf("") }
    var confirm by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    var submitting by remember { mutableStateOf(false) }
    val level = PassphraseStrength.level(pass)
    val mismatch = confirm.isNotEmpty() && confirm != pass
    val canSubmit = name.isNotBlank() && level != PassphraseStrength.Level.TOO_SHORT && confirm == pass && !submitting
    val st = Ds.status
    val strengthColor = when (level) {
        PassphraseStrength.Level.TOO_SHORT, PassphraseStrength.Level.WEAK -> st.failed.content
        PassphraseStrength.Level.FAIR -> st.approval.content
        PassphraseStrength.Level.GOOD, PassphraseStrength.Level.STRONG -> st.synced.content
    }

    ContentDialog(
        title = stringResource(R.string.vaults_new_vault),
        icon = Sym.Lock,
        onDismiss = { if (!submitting) onDismiss() },
        confirmLabel = stringResource(R.string.vaults_create_vault),
        confirmEnabled = canSubmit,
        onConfirm = {
            submitting = true; error = null
            onCreate(name.trim(), pass) { err -> submitting = false; if (err == null) onDismiss() else error = err }
        },
        dismissLabel = if (submitting) null else stringResource(R.string.common_cancel),
        dismissOnOutside = !submitting,
    ) {
        SecureContent()
        DsTextField(
            value = name, onValueChange = { name = it }, label = stringResource(R.string.vaults_vault_name),
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Next),
        )
        PassphraseField(
            value = pass, onChange = { pass = it }, label = stringResource(R.string.vaults_passphrase), enabled = !submitting,
            error = if (pass.isNotEmpty() && level == PassphraseStrength.Level.TOO_SHORT) PassphraseStrength.hint(pass) else null,
            helper = if (pass.isEmpty()) stringResource(R.string.vaults_passphrase_helper, PassphraseStrength.MIN_LENGTH) else PassphraseStrength.hint(pass),
        )
        if (pass.isNotEmpty()) {
            Column(verticalArrangement = Arrangement.spacedBy(Space.s4)) {
                LinearProgressIndicator(
                    progress = { level.fraction },
                    modifier = Modifier.fillMaxWidth().height(4.dp),
                    color = strengthColor,
                    trackColor = MaterialTheme.colorScheme.surfaceContainerHighest,
                    strokeCap = StrokeCap.Round,
                )
                Text(stringResource(R.string.vaults_strength, stringResource(level.labelRes)), style = MaterialTheme.typography.labelMedium, color = strengthColor)
            }
        }
        PassphraseField(
            value = confirm, onChange = { confirm = it }, label = stringResource(R.string.vaults_repeat_passphrase), enabled = !submitting,
            error = if (mismatch) stringResource(R.string.vaults_passphrase_mismatch) else null, imeAction = ImeAction.Done,
        )
        Callout(
            text = stringResource(R.string.vaults_lose_passphrase_warning),
            tone = CalloutTone.Warning,
        )
        if (submitting) WorkingRow(stringResource(R.string.vaults_making_keys))
        error?.let { ErrorLine(it) }
    }
}

@Composable
private fun UnlockDialog(row: VaultRow, onDismiss: () -> Unit, onUnlock: (passphrase: String, onResult: ActionResult) -> Unit) {
    var pass by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    var submitting by remember { mutableStateOf(false) }
    ContentDialog(
        title = stringResource(R.string.vaults_unlock_title, row.vault.name),
        icon = Sym.Key,
        onDismiss = { if (!submitting) onDismiss() },
        confirmLabel = stringResource(R.string.vaults_unlock),
        confirmEnabled = pass.isNotEmpty() && !submitting,
        onConfirm = {
            submitting = true; error = null
            onUnlock(pass) { err -> submitting = false; if (err == null) onDismiss() else error = err }
        },
        dismissLabel = if (submitting) null else stringResource(R.string.common_cancel),
        dismissOnOutside = !submitting,
    ) {
        SecureContent()
        Text(
            stringResource(R.string.vaults_unlock_body),
            style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        PassphraseField(value = pass, onChange = { pass = it; error = null }, label = stringResource(R.string.vaults_passphrase), enabled = !submitting, error = error, imeAction = ImeAction.Done)
        if (submitting) WorkingRow(stringResource(R.string.vaults_checking_passphrase))
    }
}

@Composable
private fun JoinVaultDialog(onDismiss: () -> Unit, onJoin: (name: String, manifest: String, passphrase: String, onResult: ActionResult) -> Unit) {
    val context = LocalContext.current
    var name by remember { mutableStateOf("") }
    var manifest by remember { mutableStateOf("") }
    var pass by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    var submitting by remember { mutableStateOf(false) }
    val clipboardEmpty = stringResource(R.string.vaults_clipboard_empty)
    ContentDialog(
        title = stringResource(R.string.vaults_join_a_vault),
        icon = Sym.Link,
        onDismiss = { if (!submitting) onDismiss() },
        confirmLabel = stringResource(R.string.vaults_join_vault),
        confirmEnabled = manifest.isNotBlank() && pass.isNotEmpty() && !submitting,
        onConfirm = {
            submitting = true; error = null
            onJoin(name.trim(), manifest, pass) { err -> submitting = false; if (err == null) onDismiss() else error = err }
        },
        dismissLabel = if (submitting) null else stringResource(R.string.common_cancel),
        dismissOnOutside = !submitting,
    ) {
        SecureContent()
        Text(
            stringResource(R.string.vaults_join_body),
            style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        DsTextField(
            value = manifest, onValueChange = { manifest = it; error = null }, label = stringResource(R.string.vaults_manifest_label),
            mono = true, singleLine = false, minLines = 4,
        )
        TextButton(
            enabled = !submitting,
            onClick = {
                val clip = context.getSystemService(ClipboardManager::class.java)?.primaryClip
                val text = clip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(context)?.toString()
                if (text.isNullOrBlank()) error = clipboardEmpty else { manifest = text; error = null }
            },
        ) {
            Icon(Sym.ContentCopy, contentDescription = null, modifier = Modifier.size(18.dp))
            Spacer(Modifier.size(Space.s8))
            Text(stringResource(R.string.vaults_paste_clipboard))
        }
        DsTextField(value = name, onValueChange = { name = it }, label = stringResource(R.string.vaults_name_on_phone), helper = stringResource(R.string.vaults_name_on_phone_helper))
        PassphraseField(value = pass, onChange = { pass = it; error = null }, label = stringResource(R.string.vaults_passphrase), enabled = !submitting, imeAction = ImeAction.Done)
        if (submitting) WorkingRow(stringResource(R.string.vaults_checking_passphrase))
        error?.let { ErrorLine(it) }
    }
}

@Composable
private fun ManifestDialog(row: VaultRow, text: String, onDismiss: () -> Unit) {
    val context = LocalContext.current
    var copied by remember { mutableStateOf(false) }
    val cs = MaterialTheme.colorScheme
    ContentDialog(
        title = stringResource(R.string.vaults_manifest_title, row.vault.name),
        icon = Sym.ContentCopy,
        onDismiss = onDismiss,
        confirmLabel = stringResource(if (copied) R.string.vaults_copied else R.string.vaults_copy_manifest),
        onConfirm = {
            copySensitiveText(context, "vault.json", text)
            copied = true
        },
        dismissLabel = stringResource(R.string.common_close),
    ) {
        SecureContent()
        Text(
            stringResource(R.string.vaults_manifest_body),
            style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant,
        )
        SelectionContainer {
            Text(
                text, style = MonoSmall, color = cs.onSurface,
                modifier = Modifier.fillMaxWidth().clip(MaterialTheme.shapes.medium).background(cs.surfaceContainerHighest).padding(Space.s12),
            )
        }
    }
}

