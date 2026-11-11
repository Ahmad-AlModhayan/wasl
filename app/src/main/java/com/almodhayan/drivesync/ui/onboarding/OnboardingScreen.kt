@file:OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)

package com.almodhayan.drivesync.ui.onboarding

import android.Manifest
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.accounts.AccountsViewModel
import com.almodhayan.drivesync.ui.accounts.GoogleSignInHost
import com.almodhayan.drivesync.ui.accounts.SignInSetupBanner
import com.almodhayan.drivesync.ui.components.AppIconTile
import com.almodhayan.drivesync.ui.components.Callout
import com.almodhayan.drivesync.ui.components.CalloutTone
import com.almodhayan.drivesync.ui.components.DsTopAppBar
import com.almodhayan.drivesync.ui.components.NavIcon
import com.almodhayan.drivesync.ui.components.PermissionState
import com.almodhayan.drivesync.ui.components.PermissionStep
import com.almodhayan.drivesync.ui.components.UiAction
import com.almodhayan.drivesync.ui.components.showMessage
import com.almodhayan.drivesync.ui.theme.Opacity
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Size
import com.almodhayan.drivesync.ui.theme.Sym
import kotlinx.coroutines.launch
import com.almodhayan.drivesync.ui.theme.Ds

private enum class OnboardingStep { Welcome, Permissions, Storage }

/**
 * Three short steps: the promises, then what Wasl needs (Google Drive is required; All files access,
 * Notifications and Background are optional, since each pair can use Android's folder picker, SRS
 * FR-R3), with the storage-access choice as its own screen. Google sign-in happens right here;
 * Continue unlocks once the account is added. "Set up later" never traps anyone who can't finish now.
 */
@Composable
fun OnboardingScreen(nav: AppNav, vm: OnboardingViewModel = hiltViewModel()) {
    val accountsVm: AccountsViewModel = hiltViewModel()
    val accounts by accountsVm.state.collectAsStateWithLifecycle()
    val setupError by accountsVm.setupError.collectAsStateWithLifecycle()
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }
    var step by rememberSaveable { mutableStateOf(OnboardingStep.Welcome) }
    var backgroundAsked by rememberSaveable { mutableStateOf(false) }
    var notificationAsked by rememberSaveable { mutableStateOf(false) }
    // Storage step: one folder per pair (the default, needs nothing now) or All files access.
    var wantsAllFiles by rememberSaveable { mutableStateOf(false) }
    val vendor = remember { VendorGuide.current() }

    // Permissions are system state: re-check them every time the app comes back to the front.
    val tick = rememberResumeTick()
    var device by remember { mutableStateOf(DeviceChecks.status(context)) }
    LaunchedEffect(tick) { device = DeviceChecks.status(context) }
    // Set when Continue asked for notifications first: onboarding ends once Android answers.
    var finishAfterAsking by remember { mutableStateOf(false) }
    val notificationLauncher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
        device = DeviceChecks.status(context)
        if (finishAfterAsking) vm.finish { nav.home() }
    }

    GoogleSignInHost(accountsVm, snackbar)

    // Back from Android settings with All files access on: the checklist now shows Allowed.
    LaunchedEffect(step, device.allFilesAccess) {
        if (step == OnboardingStep.Storage && device.allFilesAccess) step = OnboardingStep.Permissions
    }
    val back: () -> Unit = { step = if (step == OnboardingStep.Storage) OnboardingStep.Permissions else OnboardingStep.Welcome }
    BackHandler(enabled = step != OnboardingStep.Welcome, onBack = back)

    val hasAccount = accounts.rows.isNotEmpty()
    val requiredDone = hasAccount
    // The status notification is how Wasl says it is syncing, waiting or needs you: ask once before
    // leaving onboarding, even when the row was skipped (E2E 2026-10: it was never asked).
    val finish: () -> Unit = {
        if (DeviceChecks.needsNotificationRuntimePermission() && !device.notifications && !notificationAsked) {
            notificationAsked = true
            finishAfterAsking = true
            notificationLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
        } else {
            vm.finish { nav.home() }
        }
    }
    val autostartMissing = stringResource(R.string.onboarding_autostart_missing)

    Scaffold(
        topBar = {
            if (step != OnboardingStep.Welcome) {
                DsTopAppBar(
                    title = "",
                    navIcon = NavIcon.Back,
                    onNav = back,
                    actions = {
                        // "Later" means later: no permission question on the way out (UX audit 2026-10).
                        // The pairs screen asks once there is a pair to report on.
                        if (step == OnboardingStep.Permissions) TextButton(onClick = { vm.finish { nav.home() } }) { Text(stringResource(R.string.onboarding_set_up_later)) } },
                )
            }
        },
        bottomBar = {
            OnboardingBottomBar(current = step.ordinal + 1, total = OnboardingStep.entries.size) {
                when (step) {
                    OnboardingStep.Welcome -> LargeButton(stringResource(R.string.onboarding_get_started), onClick = { step = OnboardingStep.Permissions })
                    OnboardingStep.Permissions -> LargeButton(stringResource(R.string.onboarding_continue), enabled = requiredDone, onClick = finish)
                    OnboardingStep.Storage ->
                        if (wantsAllFiles) LargeButton(stringResource(R.string.onboarding_open_android_settings), icon = Sym.OpenInNew, onClick = { DeviceChecks.openAllFilesAccess(context) })
                        else LargeButton(stringResource(R.string.onboarding_continue), onClick = { step = OnboardingStep.Permissions })
                }
            }
        },
        snackbarHost = { SnackbarHost(snackbar) },
        containerColor = MaterialTheme.colorScheme.surface,
    ) { padding ->
        val modifier = Modifier.fillMaxSize().padding(padding)
        when (step) {
            OnboardingStep.Welcome -> WelcomeStep(modifier)
            OnboardingStep.Permissions -> PermissionsStep(
                modifier = modifier,
                hasAccount = hasAccount,
                signingIn = accounts.signingIn,
                setupError = setupError != null,
                device = device,
                vendor = vendor,
                backgroundAsked = backgroundAsked,
                onSignIn = accountsVm::addAccount,
                onSetupRetry = {
                    accountsVm.dismissSetupError()
                    accountsVm.addAccount()
                },
                onSetupHide = accountsVm::dismissSetupError,
                onFiles = { step = OnboardingStep.Storage },
                onNotifications = {
                    if (DeviceChecks.needsNotificationRuntimePermission() && !notificationAsked) {
                        notificationAsked = true
                        notificationLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
                    } else {
                        // Android stops showing the dialog after a refusal; its own page still works.
                        DeviceChecks.openNotificationSettings(context)
                    }
                },
                onBackground = {
                    backgroundAsked = true
                    DeviceChecks.requestIgnoreBatteryOptimizations(context)
                },
                onAutostart = {
                    if (!VendorGuide.openAutostartSettings(context)) {
                        scope.launch { snackbar.showMessage(autostartMissing) }
                    }
                },
            )
            OnboardingStep.Storage -> StorageStep(modifier, allFiles = wantsAllFiles, onChoose = { wantsAllFiles = it })
        }
    }
}

// ------------------------------------------------------------------ 1 · welcome

@Composable
private fun WelcomeStep(modifier: Modifier) {
    val cs = MaterialTheme.colorScheme
    Column(
        modifier.verticalScroll(rememberScrollState()).padding(start = Space.s24, end = Space.s24, top = Space.s48, bottom = Space.s16),
        verticalArrangement = Arrangement.spacedBy(Space.s24),
    ) {
        AppIconTile(size = 72.dp)
        Column {
            Text(
                stringResource(R.string.onboarding_welcome_title),
                style = MaterialTheme.typography.headlineMedium,
                color = cs.onSurface,
                modifier = Modifier.padding(bottom = Space.s8).semantics { heading() },
            )
            Text(
                stringResource(R.string.onboarding_welcome_body),
                style = MaterialTheme.typography.bodyLarge,
                color = cs.onSurfaceVariant,
            )
        }
        Column(verticalArrangement = Arrangement.spacedBy(Space.s16)) {
            PromiseRow(Sym.Visibility, stringResource(R.string.onboarding_promise_preview_title), stringResource(R.string.onboarding_promise_preview_body))
            PromiseRow(Sym.RestoreFromTrash, stringResource(R.string.onboarding_promise_bin_title), stringResource(R.string.onboarding_promise_bin_body))
            PromiseRow(Sym.Block, stringResource(R.string.onboarding_promise_privacy_title), stringResource(R.string.onboarding_promise_privacy_body))
        }
    }
}

@Composable
private fun PromiseRow(icon: ImageVector, title: String, text: String) {
    val cs = MaterialTheme.colorScheme
    Row(
        Modifier.fillMaxWidth().semantics(mergeDescendants = true) {},
        horizontalArrangement = Arrangement.spacedBy(Space.s12),
        verticalAlignment = Alignment.Top,
    ) {
        Box(Modifier.size(36.dp).clip(CircleShape).background(cs.primaryContainer), contentAlignment = Alignment.Center) {
            Icon(icon, contentDescription = null, tint = cs.onPrimaryContainer, modifier = Modifier.size(20.dp))
        }
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.titleSmall, color = cs.onSurface)
            Text(text, style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant)
        }
    }
}

// ------------------------------------------------------------------ 2 · what Drive Sync needs

@Composable
private fun PermissionsStep(
    modifier: Modifier,
    hasAccount: Boolean,
    signingIn: Boolean,
    setupError: Boolean,
    device: DeviceStatus,
    vendor: VendorInfo?,
    backgroundAsked: Boolean,
    onSignIn: () -> Unit,
    onSetupRetry: () -> Unit,
    onSetupHide: () -> Unit,
    onFiles: () -> Unit,
    onNotifications: () -> Unit,
    onBackground: () -> Unit,
    onAutostart: () -> Unit,
) {
    Column(
        modifier.verticalScroll(rememberScrollState()).padding(start = Space.s16, end = Space.s16, bottom = Space.s16),
        verticalArrangement = Arrangement.spacedBy(Space.s12),
    ) {
        StepTitle(stringResource(R.string.onboarding_needs_title), stringResource(R.string.onboarding_needs_body))
        if (setupError) SignInSetupBanner(onTryAgain = onSetupRetry, onHide = onSetupHide)
        // The first required step that isn't allowed yet is the one to do now.
        PermissionStep(
            icon = Sym.AccountCircle,
            title = stringResource(R.string.onboarding_perm_drive_title),
            why = stringResource(R.string.onboarding_perm_drive_why),
            state = if (hasAccount) PermissionState.Granted else PermissionState.Next,
            onAllow = onSignIn,
            cta = stringResource(if (signingIn) R.string.onboarding_signing_in else R.string.onboarding_sign_in),
            enabled = !signingIn,
        )
        PermissionStep(
            icon = Sym.FolderOpen,
            title = stringResource(R.string.onboarding_perm_files_title),
            why = stringResource(R.string.onboarding_perm_files_why),
            state = when {
                device.allFilesAccess -> PermissionState.Granted
                else -> PermissionState.Needed
            },
            onAllow = onFiles,
            optional = true,
        )
        PermissionStep(
            icon = Sym.Notifications,
            title = stringResource(R.string.onboarding_perm_notifications_title),
            why = stringResource(R.string.onboarding_perm_notifications_why),
            state = if (device.notifications) PermissionState.Granted else PermissionState.Needed,
            onAllow = onNotifications,
            optional = true,
        )
        PermissionStep(
            icon = Sym.BatteryAlert,
            title = stringResource(R.string.onboarding_perm_background_title),
            why = stringResource(R.string.onboarding_perm_background_why),
            state = if (device.batteryUnrestricted) PermissionState.Granted else PermissionState.Needed,
            onAllow = onBackground,
            optional = true,
        )
        if (vendor != null && (device.batteryUnrestricted || backgroundAsked)) {
            Callout(
                text = stringResource(vendor.guidanceRes),
                tone = CalloutTone.Info,
                title = stringResource(R.string.onboarding_vendor_title, vendor.label),
                action = UiAction(stringResource(R.string.onboarding_open_autostart), onClick = onAutostart),
            )
        }
    }
}

// ------------------------------------------------------------------ 2b · how to reach your files

@Composable
private fun StorageStep(modifier: Modifier, allFiles: Boolean, onChoose: (Boolean) -> Unit) {
    Column(
        modifier.verticalScroll(rememberScrollState()).padding(start = Space.s16, end = Space.s16, bottom = Space.s16),
        verticalArrangement = Arrangement.spacedBy(Space.s12),
    ) {
        StepTitle(stringResource(R.string.onboarding_storage_title), stringResource(R.string.onboarding_storage_body))
        Column(Modifier.selectableGroup(), verticalArrangement = Arrangement.spacedBy(Space.s12)) {
            AccessOption(
                title = stringResource(R.string.onboarding_storage_one_folder_title),
                description = stringResource(R.string.onboarding_storage_one_folder_body),
                selected = !allFiles,
                enabled = true,
                recommended = true,
                onClick = { onChoose(false) },
            )
            AccessOption(
                title = stringResource(R.string.common_all_files_access),
                description = stringResource(R.string.onboarding_storage_all_files_body),
                selected = allFiles,
                enabled = true,
                onClick = { onChoose(true) },
            )
        }
        Callout(stringResource(R.string.onboarding_storage_android_data), tone = CalloutTone.Info)
    }
}

/** A radio card in the sync-mode style. Disabled cards stay visible, dimmed, with the reason fully readable. */
@Composable
private fun AccessOption(
    title: String,
    description: String,
    selected: Boolean,
    enabled: Boolean,
    recommended: Boolean = false,
    disabledReason: String? = null,
    onClick: () -> Unit,
) {
    val cs = MaterialTheme.colorScheme
    val shape = MaterialTheme.shapes.large
    val dim = if (enabled) 1f else Opacity.disabledContent
    Row(
        Modifier.fillMaxWidth().clip(shape)
            .background(if (selected) cs.primaryContainer else Ds.lab.card)
            .then(if (selected) Modifier.border(2.dp, cs.primary, shape) else Modifier)
            .selectable(selected = selected, enabled = enabled, role = Role.RadioButton, onClick = onClick)
            .padding(Space.s16),
        horizontalArrangement = Arrangement.spacedBy(Space.s12),
        verticalAlignment = Alignment.Top,
    ) {
        Icon(
            if (selected) Sym.RadioButtonChecked else Sym.RadioButtonUnchecked,
            contentDescription = null,
            tint = if (selected) cs.primary else cs.onSurfaceVariant,
            modifier = Modifier.alpha(dim),
        )
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            FlowRow(horizontalArrangement = Arrangement.spacedBy(Space.s8), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(title, style = MaterialTheme.typography.titleMedium, color = cs.onSurface, modifier = Modifier.alpha(dim).align(Alignment.CenterVertically))
                if (recommended) {
                    Surface(shape = RoundedCornerShape(50), color = cs.primary, modifier = Modifier.align(Alignment.CenterVertically)) {
                        Text(stringResource(R.string.common_recommended), style = MaterialTheme.typography.labelSmall, color = cs.onPrimary, modifier = Modifier.padding(horizontal = Space.s8, vertical = 2.dp))
                    }
                }
            }
            Text(description, style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant, modifier = Modifier.alpha(dim))
            if (!enabled && disabledReason != null) {
                Row(Modifier.padding(top = Space.s4), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s4)) {
                    Icon(Sym.Block, contentDescription = null, tint = cs.onSurfaceVariant, modifier = Modifier.size(14.dp))
                    Text(disabledReason, style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant)
                }
            }
        }
    }
}

// ------------------------------------------------------------------ pieces

@Composable
private fun StepTitle(title: String, body: String) {
    val cs = MaterialTheme.colorScheme
    Column(Modifier.padding(start = Space.s8, end = Space.s8, bottom = Space.s8)) {
        Text(title, style = MaterialTheme.typography.headlineMedium, color = cs.onSurface, modifier = Modifier.padding(bottom = Space.s8).semantics { heading() })
        Text(body, style = MaterialTheme.typography.bodyLarge, color = cs.onSurfaceVariant)
    }
}

/** Plain bottom area: step dots over one large button. */
@Composable
private fun OnboardingBottomBar(current: Int, total: Int, button: @Composable () -> Unit) {
    Column(
        Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surface).navigationBarsPadding()
            .padding(start = Space.s16, end = Space.s16, top = Space.s12, bottom = Space.s16),
        verticalArrangement = Arrangement.spacedBy(Space.s16),
    ) {
        StepDots(current, total)
        button()
    }
}

@Composable
private fun StepDots(current: Int, total: Int) {
    val cs = MaterialTheme.colorScheme
    val stepText = stringResource(R.string.onboarding_step_of, current, total)
    Row(
        Modifier.fillMaxWidth().semantics(mergeDescendants = true) { contentDescription = stepText },
        horizontalArrangement = Arrangement.spacedBy(6.dp, Alignment.CenterHorizontally),
    ) {
        repeat(total) { i ->
            val active = i + 1 == current
            Box(Modifier.width(if (active) 20.dp else 6.dp).height(6.dp).clip(RoundedCornerShape(3.dp)).background(if (active) cs.primary else cs.outlineVariant))
        }
    }
}

/** The design's large full-width button ([Size.button]). */
@Composable
private fun LargeButton(text: String, onClick: () -> Unit, enabled: Boolean = true, icon: ImageVector? = null) {
    Button(onClick = onClick, enabled = enabled, modifier = Modifier.fillMaxWidth().heightIn(min = Size.button), contentPadding = PaddingValues(horizontal = Space.s24)) {
        if (icon != null) {
            Icon(icon, contentDescription = null, modifier = Modifier.size(20.dp))
            Spacer(Modifier.width(Space.s8))
        }
        Text(text, style = MaterialTheme.typography.labelLarge)
    }
}
