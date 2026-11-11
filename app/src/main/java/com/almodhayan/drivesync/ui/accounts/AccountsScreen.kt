package com.almodhayan.drivesync.ui.accounts

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.common.AppScreen
import com.almodhayan.drivesync.ui.common.LoadingState
import com.almodhayan.drivesync.ui.components.DsDialog
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.MenuAction
import com.almodhayan.drivesync.ui.components.OverflowMenu
import com.almodhayan.drivesync.ui.components.Side
import com.almodhayan.drivesync.ui.components.StatusBanner
import com.almodhayan.drivesync.ui.components.StorageBar
import com.almodhayan.drivesync.ui.components.StorageModel
import com.almodhayan.drivesync.ui.components.UiAction
import com.almodhayan.drivesync.ui.home.LoudStates
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import com.almodhayan.drivesync.ui.theme.tabular
import com.almodhayan.drivesync.ui.theme.Ds

/**
 * Google accounts: one card per account with its Drive storage and folder-list state, the sign-in fix
 * when Google asks for it, and Add Google account. Disconnect is the one confirmation dialog here.
 */
@Composable
fun AccountsScreen(nav: AppNav, vm: AccountsViewModel = hiltViewModel()) {
    val state by vm.state.collectAsStateWithLifecycle()
    val setupError by vm.setupError.collectAsStateWithLifecycle()
    val snackbar = remember { SnackbarHostState() }
    val scrollBehavior = TopAppBarDefaults.pinnedScrollBehavior()
    var disconnect by remember { mutableStateOf<AccountRow?>(null) }

    GoogleSignInHost(vm, snackbar)

    AppScreen(
        title = stringResource(R.string.accounts_title),
        onBack = { nav.back() },
        scrollBehavior = scrollBehavior,
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        if (state.loading) {
            Box(Modifier.fillMaxSize().padding(padding)) { LoadingState() }
        } else {
            val now = System.currentTimeMillis()
            LazyColumn(
                Modifier.fillMaxSize().padding(padding),
                contentPadding = PaddingValues(start = Space.s16, end = Space.s16, top = Space.s8, bottom = Space.s24),
                verticalArrangement = Arrangement.spacedBy(Space.s12),
            ) {
                if (setupError != null) {
                    item(key = "setup") {
                        SignInSetupBanner(
                            onTryAgain = {
                                vm.dismissSetupError()
                                vm.addAccount()
                            },
                            onHide = vm::dismissSetupError,
                        )
                    }
                }
                if (state.rows.isEmpty()) item(key = "intro") { AccountsIntro() }
                items(state.rows, key = { it.account.id }) { row ->
                    AccountCard(
                        row = row,
                        now = now,
                        onRefresh = { vm.refresh(row.account.id) },
                        onSignInAgain = { vm.reauthorize(row.account.id) },
                        onIndex = { vm.startIndexing(row.account.id) },
                        onBrowse = { nav.driveBrowser(row.account.id) },
                        onDisconnect = { disconnect = row },
                    )
                }
                item(key = "add") { AddAccount(signingIn = state.signingIn, onAdd = vm::addAccount, onRestore = { nav.backup() }) }
            }
        }
    }

    disconnect?.let { row ->
        val email = row.account.id
        val n = row.pairCount
        DsDialog(
            title = stringResource(R.string.accounts_disconnect_title, email),
            body = if (n == 0) stringResource(R.string.accounts_disconnect_body_none) else pluralStringResource(R.plurals.accounts_disconnect_body, n, Fmt.count(n)),
            confirmLabel = stringResource(R.string.accounts_disconnect),
            onConfirm = {
                disconnect = null
                vm.remove(email)
            },
            onDismiss = { disconnect = null },
            icon = Sym.Logout,
            danger = true,
        )
    }
}

@Composable
private fun AccountCard(
    row: AccountRow,
    now: Long,
    onRefresh: () -> Unit,
    onSignInAgain: () -> Unit,
    onIndex: () -> Unit,
    onBrowse: () -> Unit,
    onDisconnect: () -> Unit,
) {
    val a = row.account
    val cs = MaterialTheme.colorScheme
    val shape = MaterialTheme.shapes.large
    val authError = !a.authError.isNullOrBlank()
    val name = a.displayName?.takeIf { it.isNotBlank() }
    Column(
        Modifier.fillMaxWidth().clip(shape).background(Ds.lab.card).padding(Space.s16),
        verticalArrangement = Arrangement.spacedBy(Space.s12),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s12)) {
            Box(Modifier.size(40.dp).clip(CircleShape).background(cs.tertiaryContainer), contentAlignment = Alignment.Center) {
                Text((name ?: a.id).firstOrNull()?.uppercase() ?: "?", style = MaterialTheme.typography.titleMedium, color = cs.onTertiaryContainer)
            }
            Column(Modifier.weight(1f)) {
                Text(name ?: stringResource(R.string.common_google_account), style = MaterialTheme.typography.titleMedium, color = cs.onSurface, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(a.id, style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            if (row.busy) {
                val working = stringResource(R.string.accounts_working)
                CircularProgressIndicator(Modifier.size(20.dp).semantics { contentDescription = working }, strokeWidth = 2.dp)
            }
            OverflowMenu(
                listOf(
                    MenuAction(stringResource(R.string.accounts_refresh), Sym.Update, onClick = onRefresh),
                    MenuAction(stringResource(R.string.accounts_sign_in_again), Sym.Key, onClick = onSignInAgain),
                    MenuAction(stringResource(R.string.drive_browse_drive), Sym.FolderOpen, onClick = onBrowse),
                    MenuAction(stringResource(R.string.accounts_disconnect), Sym.Logout, destructive = true, onClick = onDisconnect),
                ),
                contentDescription = stringResource(R.string.components_more_for, a.id),
            )
        }

        if (authError) {
            val spec = LoudStates.signInNeeded(a, row.pairCount, onlyAccount = true)
            StatusBanner(
                status = spec.status,
                title = spec.title,
                body = spec.body,
                primary = UiAction(spec.actionLabel, onClick = onSignInAgain),
            )
        }

        val limit = a.storageLimit
        val used = a.storageUsed
        when {
            limit != null && limit > 0 -> StorageBar(
                StorageModel(Side.Drive, total = limit, pair = row.pairBytes, other = ((used ?: 0L) - row.pairBytes).coerceAtLeast(0), label = stringResource(R.string.common_drive_storage), pairLabel = stringResource(R.string.common_your_pairs)),
                modifier = Modifier.testTag("account_storage"),
                compact = true,
            )
            used != null -> Text(stringResource(R.string.accounts_storage_used_no_limit, Fmt.bytes(used)), style = MaterialTheme.typography.bodySmall.tabular, color = cs.onSurfaceVariant)
            else -> Text(stringResource(R.string.accounts_storage_not_checked), style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant)
        }

        FolderListLine(row, now, onIndex, enabled = !authError)
    }
}

/** The Drive folder list (metadata index) behind pickers and previews: reading, failed, never read, or when updated. */
@Composable
private fun FolderListLine(row: AccountRow, now: Long, onIndex: () -> Unit, enabled: Boolean) {
    val cs = MaterialTheme.colorScheme
    val style = MaterialTheme.typography.bodySmall
    val pairs = if (row.pairCount == 0) stringResource(R.string.accounts_no_pairs_yet)
    else pluralStringResource(R.plurals.accounts_pairs, row.pairCount, Fmt.count(row.pairCount))
    val updatedAt = row.account.lastDeltaAt ?: row.account.lastCrawlAt
    val error = row.indexError
    when {
        row.indexing -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s8)) {
            CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
            Text(stringResource(R.string.accounts_index_reading), style = style, color = cs.onSurfaceVariant)
        }
        error != null -> Column {
            Text(stringResource(R.string.accounts_index_failed, error), style = style, color = cs.onSurfaceVariant)
            TextButton(onClick = onIndex, enabled = enabled, modifier = Modifier.offset(x = (-12).dp)) { Text(stringResource(R.string.common_try_again)) }
        }
        updatedAt == null -> Column {
            Text(stringResource(R.string.accounts_index_never), style = style, color = cs.onSurfaceVariant)
            TextButton(onClick = onIndex, enabled = enabled, modifier = Modifier.offset(x = (-12).dp)) { Text(stringResource(R.string.accounts_index_read_now)) }
        }
        else -> Text(stringResource(R.string.accounts_index_updated, pairs, Fmt.relativeLower(updatedAt, now)), style = style.tabular, color = cs.onSurfaceVariant)
    }
}

@Composable
private fun AccountsIntro() {
    val cs = MaterialTheme.colorScheme
    Column(Modifier.fillMaxWidth().padding(horizontal = Space.s8, vertical = Space.s16), verticalArrangement = Arrangement.spacedBy(Space.s8)) {
        Icon(Sym.AccountCircle, contentDescription = null, tint = cs.primary, modifier = Modifier.size(40.dp))
        Text(stringResource(R.string.accounts_intro_title), style = MaterialTheme.typography.titleLarge, color = cs.onSurface, modifier = Modifier.semantics { heading() })
        Text(
            stringResource(R.string.accounts_intro_body),
            style = MaterialTheme.typography.bodyMedium,
            color = cs.onSurfaceVariant,
        )
    }
}

@Composable
private fun AddAccount(signingIn: Boolean, onAdd: () -> Unit, onRestore: () -> Unit) {
    val cs = MaterialTheme.colorScheme
    Column(Modifier.fillMaxWidth().padding(top = Space.s4), verticalArrangement = Arrangement.spacedBy(Space.s8)) {
        FilledTonalButton(onClick = onAdd, enabled = !signingIn, contentPadding = ButtonDefaults.ButtonWithIconContentPadding) {
            if (signingIn) CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
            else Icon(Sym.Add, contentDescription = null, modifier = Modifier.size(18.dp))
            Spacer(Modifier.width(Space.s8))
            Text(stringResource(if (signingIn) R.string.accounts_waiting_for_google else R.string.accounts_add_google_account))
        }
        Text(
            stringResource(R.string.accounts_add_scope_note),
            style = MaterialTheme.typography.bodySmall,
            color = cs.onSurfaceVariant,
        )
        TextButton(onClick = onRestore, modifier = Modifier.offset(x = (-12).dp)) { Text(stringResource(R.string.accounts_restore_from_backup)) }
    }
}

