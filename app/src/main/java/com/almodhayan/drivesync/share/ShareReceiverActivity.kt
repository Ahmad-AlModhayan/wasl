package com.almodhayan.drivesync.share

import android.content.Intent
import android.os.Bundle
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.lifecycleScope
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import com.almodhayan.drivesync.ui.MainActivity
import com.almodhayan.drivesync.ui.common.AppScreen
import com.almodhayan.drivesync.ui.common.EmptyState
import com.almodhayan.drivesync.ui.common.LoadingState
import com.almodhayan.drivesync.ui.components.BottomActionBar
import com.almodhayan.drivesync.ui.components.Callout
import com.almodhayan.drivesync.ui.components.CalloutTone
import com.almodhayan.drivesync.ui.components.DsListItem
import com.almodhayan.drivesync.ui.components.DsTextField
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.ItemTone
import com.almodhayan.drivesync.ui.components.NavIcon
import com.almodhayan.drivesync.ui.components.PairDirection
import com.almodhayan.drivesync.ui.components.SectionHeader
import com.almodhayan.drivesync.ui.settings.ContentDialog
import com.almodhayan.drivesync.ui.theme.DriveSyncTheme
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import dagger.hilt.android.AndroidEntryPoint
import kotlinx.coroutines.launch
import javax.inject.Inject

/**
 * Share-sheet target ("Add to Wasl"): copies shared files or text into a pair's phone folder
 * and starts a sync of that pair. Content URIs are read only while this activity lives; no URI
 * permission is persisted.
 */
@AndroidEntryPoint
class ShareReceiverActivity : ComponentActivity() {
    @Inject lateinit var settings: SettingsRepository
    private val vm: ShareViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val payload = SharedPayload.from(intent, packageName)
        if (payload.isEmpty) {
            val why = if (payload.refused > 0) R.string.share_refused_all else R.string.share_nothing_to_add
            Toast.makeText(this, getString(why), Toast.LENGTH_LONG).show()
            finish()
            return
        }
        if (payload.refused > 0) Toast.makeText(this, getString(R.string.share_refused_some), Toast.LENGTH_LONG).show()
        lifecycleScope.launch {
            vm.done.collect { text ->
                Toast.makeText(applicationContext, text, Toast.LENGTH_LONG).show()
                finish()
            }
        }
        setContent {
            val cfg by settings.settings.collectAsState(initial = null)
            DriveSyncTheme(theme = cfg?.theme ?: "system", dynamicColor = cfg?.dynamicColor ?: false) {
                ShareScreen(
                    payload = payload,
                    vm = vm,
                    onClose = { finish() },
                    onOpenApp = {
                        startActivity(Intent(this@ShareReceiverActivity, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                        finish()
                    },
                )
            }
        }
    }
}

@Composable
private fun ShareScreen(payload: SharedPayload, vm: ShareViewModel, onClose: () -> Unit, onOpenApp: () -> Unit) {
    val s by vm.state.collectAsStateWithLifecycle()
    var newFolderText by remember { mutableStateOf<String?>(null) }
    var newFolderError by remember { mutableStateOf<String?>(null) }
    val openAppLabel = stringResource(R.string.share_open_app)

    AppScreen(
        title = stringResource(R.string.share_target_label),
        subtitle = payload.describe().replaceFirstChar { it.uppercase() },
        onBack = onClose,
        navIcon = NavIcon.Close,
        bottomBar = {
            if (s.loaded && s.hasStorageAccess && s.pairs.isNotEmpty()) {
                val target = s.selectedPair?.let { p -> Fmt.humanLocalPath(p.localPath) + (if (s.subfolder.isNotEmpty()) " / ${s.subfolder}" else "") }
                BottomActionBar(hint = target?.let { stringResource(R.string.share_into, it) } ?: stringResource(R.string.share_choose_pair_first)) {
                    Button(onClick = { vm.add(payload) }, enabled = s.selectedPair != null && !s.busy) {
                        if (s.busy) {
                            CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp, color = LocalContentColor.current)
                            Spacer(Modifier.size(Space.s8))
                            Text(stringResource(R.string.share_adding))
                        } else {
                            Text(addLabel(payload))
                        }
                    }
                }
            }
        },
    ) { padding ->
        when {
            !s.loaded -> Box(Modifier.fillMaxSize().padding(padding)) { LoadingState() }
            !s.hasStorageAccess -> EmptyState(
                icon = Sym.Lock,
                title = stringResource(R.string.share_no_access_title),
                body = stringResource(R.string.share_no_access_body),
                modifier = Modifier.padding(padding),
            ) { Button(onClick = onOpenApp) { Text(openAppLabel) } }
            s.pairs.isEmpty() -> EmptyState(
                icon = Sym.CloudOff,
                title = stringResource(R.string.share_no_pairs_title),
                body = stringResource(R.string.share_no_pairs_body, PairDirection.TwoWay.title, PairDirection.PhoneToDrive.title),
                modifier = Modifier.padding(padding),
            ) { Button(onClick = onOpenApp) { Text(openAppLabel) } }
            else -> Column(Modifier.fillMaxSize().padding(padding).verticalScroll(rememberScrollState()).padding(bottom = Space.s16)) {
                SectionHeader(stringResource(R.string.share_section_pair))
                Column(Modifier.selectableGroup()) {
                    s.pairs.forEach { p ->
                        PairOption(p, selected = p.id == s.selectedPairId) { vm.selectPair(p.id) }
                    }
                }
                SectionHeader(stringResource(R.string.share_section_folder))
                Column(Modifier.selectableGroup()) {
                    FolderOption(stringResource(R.string.share_top_folder), selected = s.subfolder.isEmpty()) { vm.selectSubfolder("") }
                    s.subfolders.forEach { sub -> FolderOption(sub, selected = s.subfolder == sub) { vm.selectSubfolder(sub) } }
                }
                DsListItem(
                    headline = stringResource(R.string.common_new_folder),
                    icon = Sym.CreateNewFolder,
                    tone = ItemTone.Folder,
                    onClick = { newFolderText = ""; newFolderError = null },
                )
                s.error?.let {
                    Callout(text = it, title = stringResource(R.string.share_add_failed_title), tone = CalloutTone.Danger, modifier = Modifier.padding(horizontal = Space.s16, vertical = Space.s8))
                }
            }
        }
    }

    newFolderText?.let { text ->
        ContentDialog(
            title = stringResource(R.string.common_new_folder),
            icon = Sym.CreateNewFolder,
            onDismiss = { newFolderText = null },
            confirmLabel = stringResource(R.string.share_create_folder),
            onConfirm = {
                val err = vm.newSubfolder(text)
                if (err == null) newFolderText = null else newFolderError = err
            },
        ) {
            DsTextField(
                value = text,
                onValueChange = { newFolderText = it; newFolderError = null },
                label = stringResource(R.string.share_folder_name),
                error = newFolderError,
                helper = stringResource(R.string.share_folder_helper),
            )
        }
    }
}

/** One pair to add into: radio, name, phone folder, and why it might not sync right away. */
@Composable
private fun PairOption(p: FolderPairEntity, selected: Boolean, onClick: () -> Unit) {
    val cs = MaterialTheme.colorScheme
    DsListItem(
        headline = p.name,
        supporting = Fmt.humanLocalPath(p.localPath),
        leading = {
            Icon(if (selected) Sym.RadioButtonChecked else Sym.RadioButtonUnchecked, contentDescription = null, tint = if (selected) cs.primary else cs.onSurfaceVariant)
        },
        meta = when {
            !p.enabled -> stringResource(R.string.share_turned_off)
            p.paused -> stringResource(R.string.common_paused)
            else -> PairDirection.of(p.mode).title
        },
        selected = selected,
        onClick = onClick,
        modifier = Modifier.semantics { this.selected = selected },
    )
}

@Composable
private fun FolderOption(name: String, selected: Boolean, onClick: () -> Unit) {
    val cs = MaterialTheme.colorScheme
    DsListItem(
        headline = name,
        leading = {
            Row(horizontalArrangement = Arrangement.spacedBy(Space.s16)) {
                Icon(if (selected) Sym.RadioButtonChecked else Sym.RadioButtonUnchecked, contentDescription = null, tint = if (selected) cs.primary else cs.onSurfaceVariant)
                Icon(Sym.FolderFill, contentDescription = null, tint = cs.primary)
            }
        },
        selected = selected,
        onClick = onClick,
        modifier = Modifier.semantics { this.selected = selected },
    )
}

@Composable
private fun addLabel(payload: SharedPayload): String =
    if (payload.hasText) stringResource(R.string.share_add_text) else pluralStringResource(R.plurals.share_add_files, payload.count, Fmt.count(payload.count))
