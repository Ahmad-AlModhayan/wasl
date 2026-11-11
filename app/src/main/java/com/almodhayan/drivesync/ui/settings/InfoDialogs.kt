package com.almodhayan.drivesync.ui.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym

/** What leaves the phone and where it goes. Shared by Settings and About. */
@Composable
fun PrivacyDialog(onDismiss: () -> Unit) {
    ContentDialog(title = stringResource(R.string.settings_privacy), icon = Sym.Shield, onDismiss = onDismiss, dismissLabel = stringResource(R.string.common_close)) {
        Fact(Sym.Block, stringResource(R.string.settings_privacy_no_ads_title), stringResource(R.string.settings_privacy_no_ads_body))
        Fact(Sym.SyncAlt, stringResource(R.string.settings_privacy_direct_title), stringResource(R.string.settings_privacy_direct_body))
        Fact(Sym.CloudUpload, stringResource(R.string.settings_privacy_backup_title), stringResource(R.string.settings_privacy_backup_body))
        Fact(Sym.Key, stringResource(R.string.settings_privacy_sign_in_title), stringResource(R.string.settings_privacy_sign_in_body))
        Fact(Sym.Wifi, stringResource(R.string.settings_privacy_location_title), stringResource(R.string.settings_privacy_location_body))
    }
}

/** Open-source licences of what ships in the APK. Shared by Settings and About. */
@Composable
fun LicencesDialog(onDismiss: () -> Unit) {
    ContentDialog(title = stringResource(R.string.settings_licences), icon = Sym.Article, onDismiss = onDismiss, dismissLabel = stringResource(R.string.common_close)) {
        Text(stringResource(R.string.settings_licences_intro), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Licence("Readex Pro", "SIL Open Font License 1.1")
        Licence("IBM Plex Mono", "SIL Open Font License 1.1")
        Licence("Material Symbols", "Apache License 2.0")
        Licence(stringResource(R.string.settings_licence_androidx), "Apache License 2.0")
        Licence(stringResource(R.string.settings_licence_kotlin), "Apache License 2.0")
        Licence(stringResource(R.string.settings_licence_dagger), "Apache License 2.0")
        Licence("OkHttp", "Apache License 2.0")
    }
}

@Composable
private fun Fact(icon: ImageVector, title: String, body: String) {
    val cs = MaterialTheme.colorScheme
    Row(horizontalArrangement = Arrangement.spacedBy(Space.s12)) {
        Icon(icon, contentDescription = null, tint = cs.onSurfaceVariant, modifier = Modifier.padding(top = 2.dp).size(20.dp))
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.titleSmall, color = cs.onSurface)
            Text(body, style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant)
        }
    }
}

@Composable
private fun Licence(name: String, licence: String) {
    val cs = MaterialTheme.colorScheme
    Row(verticalAlignment = Alignment.Top) {
        Column(Modifier.weight(1f)) {
            Text(name, style = MaterialTheme.typography.bodyLarge, color = cs.onSurface)
            Text(licence, style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant)
        }
    }
}
