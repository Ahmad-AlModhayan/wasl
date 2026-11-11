package com.almodhayan.drivesync.ui.about

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.unit.dp
import com.almodhayan.drivesync.BuildConfig
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.common.AppScreen
import com.almodhayan.drivesync.ui.components.DriveSyncLockup
import com.almodhayan.drivesync.ui.components.SectionHeader
import com.almodhayan.drivesync.ui.components.SettingRow
import com.almodhayan.drivesync.ui.components.TopDestination
import com.almodhayan.drivesync.ui.settings.AutomationDialog
import com.almodhayan.drivesync.ui.settings.LicencesDialog
import com.almodhayan.drivesync.ui.settings.PrivacyDialog
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import com.almodhayan.drivesync.ui.theme.tabular

private enum class AboutDialog { Automation, Privacy, Licences }

/** The lockup, what Wasl is and promises, how a sync works, and where to look next. */
@Composable
fun AboutScreen(nav: AppNav) {
    var dialog by remember { mutableStateOf<AboutDialog?>(null) }
    val scrollBehavior = TopAppBarDefaults.pinnedScrollBehavior()
    val cs = MaterialTheme.colorScheme

    AppScreen(title = stringResource(R.string.about_title), onBack = { nav.back() }, scrollBehavior = scrollBehavior) { padding ->
        Column(Modifier.fillMaxSize().padding(padding).verticalScroll(rememberScrollState()).padding(bottom = Space.s24)) {
            Column(
                Modifier.fillMaxWidth().padding(start = Space.s16, end = Space.s16, top = Space.s16, bottom = Space.s8),
                verticalArrangement = Arrangement.spacedBy(Space.s12),
            ) {
                DriveSyncLockup(height = 32.dp)
                Text(stringResource(R.string.about_version, BuildConfig.VERSION_NAME), style = MaterialTheme.typography.bodyMedium.tabular, color = cs.onSurfaceVariant)
                Text(
                    stringResource(R.string.about_intro),
                    style = MaterialTheme.typography.bodyLarge, color = cs.onSurface,
                )
            }

            SectionHeader(stringResource(R.string.about_promises))
            Column(Modifier.padding(horizontal = Space.s16, vertical = Space.s4), verticalArrangement = Arrangement.spacedBy(Space.s16)) {
                Promise(Sym.Visibility, stringResource(R.string.about_promise_preview_title), stringResource(R.string.about_promise_preview_body))
                Promise(Sym.RestoreFromTrash, stringResource(R.string.about_promise_bin_title), stringResource(R.string.about_promise_bin_body))
                Promise(Sym.Block, stringResource(R.string.about_promise_private_title), stringResource(R.string.about_promise_private_body))
            }

            SectionHeader(stringResource(R.string.about_how_sync_works))
            Column(Modifier.padding(horizontal = Space.s16, vertical = Space.s4), verticalArrangement = Arrangement.spacedBy(Space.s12)) {
                Step(1, stringResource(R.string.about_step_compare))
                Step(2, stringResource(R.string.about_step_changes))
                Step(3, stringResource(R.string.about_step_conflicts))
                Step(4, stringResource(R.string.about_step_large_deletes))
                Step(5, stringResource(R.string.about_step_schedule))
            }

            SectionHeader(stringResource(R.string.about_more))
            SettingRow(title = stringResource(R.string.common_diagnostics), icon = Sym.MonitorHeart, summary = stringResource(R.string.settings_diagnostics_summary), onClick = { nav.diagnostics() })
            SettingRow(title = stringResource(R.string.common_settings), icon = Sym.Settings, summary = stringResource(R.string.about_settings_summary), onClick = { nav.top(TopDestination.Settings) })
            SettingRow(title = stringResource(R.string.common_automation), icon = Sym.Terminal, summary = stringResource(R.string.about_automation_summary), onClick = { dialog = AboutDialog.Automation })
            SettingRow(title = stringResource(R.string.settings_privacy), icon = Sym.Shield, summary = stringResource(R.string.settings_privacy_summary), onClick = { dialog = AboutDialog.Privacy })
            SettingRow(title = stringResource(R.string.settings_licences), icon = Sym.Article, summary = stringResource(R.string.settings_licences_summary), onClick = { dialog = AboutDialog.Licences })

            Text(
                stringResource(R.string.about_credits),
                style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = Space.s16, vertical = Space.s16),
            )
        }
    }

    when (dialog) {
        AboutDialog.Automation -> AutomationDialog(onDismiss = { dialog = null })
        AboutDialog.Privacy -> PrivacyDialog(onDismiss = { dialog = null })
        AboutDialog.Licences -> LicencesDialog(onDismiss = { dialog = null })
        null -> Unit
    }
}

@Composable
private fun Promise(icon: ImageVector, title: String, text: String) {
    val cs = MaterialTheme.colorScheme
    Row(horizontalArrangement = Arrangement.spacedBy(Space.s12), verticalAlignment = Alignment.Top) {
        Box(Modifier.size(36.dp).clip(CircleShape).background(cs.primaryContainer), contentAlignment = Alignment.Center) {
            Icon(icon, contentDescription = null, tint = cs.onPrimaryContainer, modifier = Modifier.size(20.dp))
        }
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.titleSmall, color = cs.onSurface)
            Text(text, style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant)
        }
    }
}

@Composable
private fun Step(n: Int, text: String) {
    val cs = MaterialTheme.colorScheme
    Row(horizontalArrangement = Arrangement.spacedBy(Space.s12), verticalAlignment = Alignment.Top) {
        Box(
            Modifier.size(24.dp).clip(CircleShape).background(cs.surfaceContainerHighest).clearAndSetSemantics { },
            contentAlignment = Alignment.Center,
        ) { Text("$n", style = MaterialTheme.typography.labelMedium.tabular, color = cs.onSurfaceVariant) }
        Text(text, style = MaterialTheme.typography.bodyMedium, color = cs.onSurface, modifier = Modifier.weight(1f))
    }
}
