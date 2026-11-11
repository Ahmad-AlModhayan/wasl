package com.almodhayan.drivesync.ui.accounts

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.IntentSenderRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.material3.SnackbarHostState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.ui.components.StatusBanner
import com.almodhayan.drivesync.ui.components.UiAction
import com.almodhayan.drivesync.ui.components.showMessage
import com.almodhayan.drivesync.ui.theme.SyncStatus

/**
 * Wires Google sign-in for [vm]: launches Play services' consent screen (the AuthorizationClient
 * resolution) with StartIntentSenderForResult, hands the result back, and shows the view model's
 * short messages in [snackbar]. Used by Accounts and by onboarding's Google Drive step.
 */
@Composable
fun GoogleSignInHost(vm: AccountsViewModel, snackbar: SnackbarHostState) {
    val resolution by vm.resolution.collectAsStateWithLifecycle()
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.StartIntentSenderForResult()) { result ->
        vm.onAuthResult(result.resultCode, result.data)
    }
    LaunchedEffect(resolution) {
        val r = resolution ?: return@LaunchedEffect
        vm.resolutionLaunched()
        try {
            launcher.launch(IntentSenderRequest.Builder(r.intent).build())
        } catch (t: Throwable) {
            vm.onResolutionLaunchFailed(t)
        }
    }
    LaunchedEffect(vm) { vm.messages.collect { snackbar.showMessage(it) } }
}

/** This build's OAuth client isn't registered with Google (DEVELOPER_ERROR): what failed, why, the fix. */
@Composable
fun SignInSetupBanner(onTryAgain: () -> Unit, onHide: () -> Unit, modifier: Modifier = Modifier) {
    StatusBanner(
        status = SyncStatus.Failed,
        title = stringResource(R.string.accounts_setup_title),
        modifier = modifier,
        body = stringResource(R.string.accounts_setup_body),
        primary = UiAction(stringResource(R.string.common_try_again), onClick = onTryAgain),
        secondary = UiAction(stringResource(R.string.accounts_hide), onClick = onHide),
    )
}
