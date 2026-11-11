package com.almodhayan.drivesync.ui.settings

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import androidx.annotation.StringRes
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.scheduler.AutomationGate
import com.almodhayan.drivesync.scheduler.SyncIntentReceiver
import com.almodhayan.drivesync.ui.components.SecureContent
import com.almodhayan.drivesync.ui.components.copySensitiveText
import com.almodhayan.drivesync.ui.theme.MonoSmall
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym

/** The broadcast intents handled by [SyncIntentReceiver], with adb examples. Shared by Settings and About. */
object Automation {
    data class Command(@StringRes val nameRes: Int, val action: String, @StringRes val descriptionRes: Int, val extras: String = "") {
        val name: String get() = Strings.get(nameRes)
        val description: String get() = Strings.get(descriptionRes)
    }

    const val PACKAGE = "com.kahflabs.wasl"
    const val RECEIVER_CLASS = "com.almodhayan.drivesync.scheduler.SyncIntentReceiver"
    const val COMPONENT = "$PACKAGE/.scheduler.SyncIntentReceiver"

    val commands: List<Command> = listOf(
        Command(R.string.settings_automation_sync_all, SyncIntentReceiver.ACTION_SYNC_ALL, R.string.settings_automation_sync_all_description),
        Command(R.string.settings_automation_sync_pair, SyncIntentReceiver.ACTION_SYNC_PAIR, R.string.settings_automation_sync_pair_description, extras = "--es pairId <id>"),
        Command(R.string.settings_automation_pause_all, SyncIntentReceiver.ACTION_PAUSE_ALL, R.string.settings_automation_pause_all_description),
        Command(R.string.settings_automation_resume_all, SyncIntentReceiver.ACTION_RESUME_ALL, R.string.settings_automation_resume_all_description),
        Command(R.string.settings_automation_cancel, SyncIntentReceiver.ACTION_CANCEL, R.string.settings_automation_cancel_description),
        Command(R.string.settings_automation_reschedule, SyncIntentReceiver.ACTION_RESCHEDULE, R.string.settings_automation_reschedule_description),
    )

    fun adb(c: Command, secret: String): String =
        "adb shell am broadcast -a ${c.action}" + (if (c.extras.isNotEmpty()) " ${c.extras}" else "") +
            " --es ${AutomationGate.EXTRA_SECRET} $secret -n $COMPONENT"

    fun cheatSheet(secret: String): String = buildString {
        appendLine(Strings.get(R.string.settings_automation_sheet_title))
        appendLine(Strings.get(R.string.settings_automation_sheet_target, PACKAGE, RECEIVER_CLASS))
        appendLine(Strings.get(R.string.settings_automation_sheet_extra, AutomationGate.EXTRA_SECRET, secret))
        appendLine(Strings.get(R.string.settings_automation_sheet_ignored))
        appendLine()
        for (c in commands) {
            appendLine(
                if (c.extras.isNotEmpty()) Strings.get(R.string.settings_automation_sheet_command_extra, c.name, c.action, c.extras.removePrefix("--es "))
                else "${c.name}: ${c.action}",
            )
            appendLine("  ${c.description}")
            appendLine("  ${adb(c, secret)}")
            appendLine()
        }
        appendLine(Strings.get(R.string.settings_automation_tile))
    }

    fun copyToClipboard(context: Context, label: String, text: String) {
        context.getSystemService(ClipboardManager::class.java)?.setPrimaryClip(ClipData.newPlainText(label, text))
    }
}

/** The secret, then the automation commands: name, what it does, and the exact adb line in mono on surface-container-highest. */
@Composable
fun AutomationCheatSheet(secret: String, onNewSecret: () -> Unit, modifier: Modifier = Modifier) {
    SecureContent()   // whoever has the secret can start, pause or cancel syncs (security review 2026-10)
    val cs = MaterialTheme.colorScheme
    Column(modifier, verticalArrangement = Arrangement.spacedBy(Space.s16)) {
        Column(verticalArrangement = Arrangement.spacedBy(Space.s4)) {
            Text(stringResource(R.string.settings_automation_secret), style = MaterialTheme.typography.titleSmall, color = cs.onSurface)
            Text(
                stringResource(R.string.settings_automation_secret_body, AutomationGate.EXTRA_SECRET),
                style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant,
            )
            SelectionContainer {
                Text(
                    secret, style = MonoSmall, color = cs.onSurface,
                    modifier = Modifier.fillMaxWidth().clip(MaterialTheme.shapes.small).background(cs.surfaceContainerHighest)
                        .padding(horizontal = Space.s12, vertical = Space.s8).testTag("automation_secret"),
                )
            }
            TextButton(onClick = onNewSecret, modifier = Modifier.testTag("automation_new_secret")) { Text(stringResource(R.string.settings_automation_new_secret)) }
        }
        Automation.commands.forEach { c ->
            Column(verticalArrangement = Arrangement.spacedBy(Space.s4)) {
                Text(stringResource(c.nameRes), style = MaterialTheme.typography.titleSmall, color = cs.onSurface)
                Text(stringResource(c.descriptionRes), style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant)
                SelectionContainer {
                    Text(
                        Automation.adb(c, secret),
                        style = MonoSmall,
                        color = cs.onSurface,
                        softWrap = false,
                        modifier = Modifier.fillMaxWidth().clip(MaterialTheme.shapes.small).background(cs.surfaceContainerHighest)
                            .horizontalScroll(rememberScrollState()).padding(horizontal = Space.s12, vertical = Space.s8),
                    )
                }
            }
        }
        Text(stringResource(R.string.settings_automation_tile), style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant)
    }
}

@Composable
fun AutomationDialog(onDismiss: () -> Unit, vm: AutomationViewModel = hiltViewModel()) {
    val context = LocalContext.current
    val cs = MaterialTheme.colorScheme
    val s by vm.settings.collectAsStateWithLifecycle()
    var copied by remember { mutableStateOf(false) }
    val enabled = s?.automationEnabled == true
    val secret = s?.automationSecret?.takeIf { enabled && it.isNotBlank() }
    val copiedLabel = stringResource(R.string.settings_automation_copied)
    val copyAllLabel = stringResource(R.string.settings_automation_copy_all)
    ContentDialog(
        title = stringResource(R.string.common_automation),
        icon = Sym.Terminal,
        onDismiss = onDismiss,
        confirmLabel = when { secret == null -> null; copied -> copiedLabel; else -> copyAllLabel },
        onConfirm = {
            secret?.let { copySensitiveText(context, "Drive Sync automation", Automation.cheatSheet(it)); copied = true }
        },
        dismissLabel = stringResource(R.string.common_close),
    ) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s12)) {
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(Space.s4)) {
                Text(stringResource(R.string.settings_automation_allow), style = MaterialTheme.typography.titleSmall, color = cs.onSurface)
                Text(
                    if (enabled) stringResource(R.string.settings_automation_allow_on)
                    else stringResource(R.string.settings_automation_allow_off),
                    style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant,
                )
            }
            Switch(
                checked = enabled, enabled = s != null,
                onCheckedChange = { vm.setEnabled(it); copied = false },
                modifier = Modifier.testTag("automation_toggle"),
            )
        }
        if (secret != null) AutomationCheatSheet(secret, onNewSecret = { vm.newSecret(); copied = false }, Modifier.padding(top = Space.s8))
    }
}
