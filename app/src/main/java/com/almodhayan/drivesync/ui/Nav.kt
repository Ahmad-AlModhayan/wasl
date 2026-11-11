package com.almodhayan.drivesync.ui

import android.net.Uri
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import com.almodhayan.drivesync.ui.activity.ConflictScreen
import com.almodhayan.drivesync.ui.components.TopDestination
import com.almodhayan.drivesync.ui.pair.ReviewDeletionsScreen
import com.almodhayan.drivesync.ui.theme.LocalReduceMotion
import com.almodhayan.drivesync.ui.theme.Motion
import androidx.navigation.NavHostController
import androidx.navigation.NavType
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.navArgument
import com.almodhayan.drivesync.ui.about.AboutScreen
import com.almodhayan.drivesync.ui.accounts.AccountsScreen
import com.almodhayan.drivesync.ui.activity.HistoryScreen
import com.almodhayan.drivesync.ui.activity.ProblemsScreen
import com.almodhayan.drivesync.ui.activity.RunDetailScreen
import com.almodhayan.drivesync.ui.activity.StatusScreen
import com.almodhayan.drivesync.ui.backup.BackupScreen
import com.almodhayan.drivesync.ui.diagnostics.DiagnosticsScreen
import com.almodhayan.drivesync.ui.devices.DevicesScreen
import com.almodhayan.drivesync.ui.devices.PairingScreen
import com.almodhayan.drivesync.ui.drive.DriveBrowserScreen
import com.almodhayan.drivesync.ui.home.HomeScreen
import com.almodhayan.drivesync.ui.onboarding.OnboardingScreen
import com.almodhayan.drivesync.ui.pair.DryRunScreen
import com.almodhayan.drivesync.ui.pair.LocalFolderPickerScreen
import com.almodhayan.drivesync.ui.pair.PairDetailScreen
import com.almodhayan.drivesync.ui.pair.PairEditorScreen
import com.almodhayan.drivesync.ui.pair.PickerKeys
import com.almodhayan.drivesync.ui.pair.RemoteFolderPickerScreen
import com.almodhayan.drivesync.ui.pair.SelectiveSyncScreen
import com.almodhayan.drivesync.ui.revisions.RevisionsScreen
import com.almodhayan.drivesync.ui.settings.SettingsScreen
import com.almodhayan.drivesync.ui.trash.TrashScreen
import com.almodhayan.drivesync.ui.vaults.VaultsScreen

/**
 * Route table. Owned by the shell; screens live in their own packages and receive an [AppNav].
 * Picker screens return results through the previous back stack entry's SavedStateHandle:
 *   LocalFolderPicker → "localPath": String
 *   RemoteFolderPicker → "remoteFolderId": String, "remotePath": String, "driveId": String? (empty string for My Drive)
 */
object Routes {
    const val ONBOARDING = "onboarding"
    const val HOME = "home"
    const val ACCOUNTS = "accounts"
    // pairId = "new" for a new pair; optional accountId, and an offer to accept (docs/INTRODUCTIONS.md §3.3).
    const val PAIR_EDIT = "pair/{pairId}/edit?accountId={accountId}&offerShare={offerShare}&offerFrom={offerFrom}&offerName={offerName}"
    const val PAIR_DETAIL = "pair/{pairId}"
    const val SELECTIVE = "pair/{pairId}/selective"
    const val DRY_RUN = "pair/{pairId}/dryrun"
    const val LOCAL_PICKER = "picker/local?start={start}"
    const val REMOTE_PICKER = "picker/remote/{accountId}"
    const val STATUS = "status"
    const val HISTORY = "history"
    const val RUN_DETAIL = "run/{runId}"
    const val PROBLEMS = "problems"
    const val TRASH = "trash"
    const val REVISIONS = "revisions/{accountId}/{fileId}?name={name}&pairId={pairId}&key={key}"
    const val SETTINGS = "settings"
    const val DIAGNOSTICS = "diagnostics"
    const val VAULTS = "vaults"
    const val BACKUP = "backup"
    const val DRIVE_BROWSER = "drive/{accountId}?folderId={folderId}"
    const val ABOUT = "about"
    const val CONFLICT = "conflict/{conflictId}"
    const val REVIEW_DELETIONS = "pair/{pairId}/deletions"
    const val DEVICES = "devices"
    const val DEVICE_PAIRING = "devices/pairing/{mode}"       // mode = add | join

    /** The four destinations of the bottom navigation bar. */
    val topLevel = mapOf(HOME to TopDestination.Pairs, PROBLEMS to TopDestination.Problems, TRASH to TopDestination.Bin, SETTINGS to TopDestination.Settings)
}

class AppNav(val controller: NavHostController) {
    fun back() { controller.popBackStack() }
    fun home() { controller.navigate(Routes.HOME) { popUpTo(0) { inclusive = true } } }
    fun onboarding() { controller.navigate(Routes.ONBOARDING) }
    fun accounts() { controller.navigate(Routes.ACCOUNTS) }
    fun pairEdit(pairId: String?, accountId: String? = null) { controller.navigate("pair/${pairId ?: "new"}/edit" + (accountId?.let { "?accountId=${Uri.encode(it)}" } ?: "")) }
    /** A new pair with its Drive folder already chosen, handed over the way the folder picker does it. */
    fun pairEditWithDriveFolder(accountId: String, folderId: String, path: String, driveId: String?) {
        pairEdit(null, accountId)
        val h = controller.currentBackStackEntry?.savedStateHandle ?: return
        h[PickerKeys.REMOTE_PATH] = path
        h[PickerKeys.DRIVE_ID] = driveId ?: ""
        h[PickerKeys.REMOTE_FOLDER_ID] = folderId   // written last: the editor reacts to this key
    }
    /** A new pair for a folder another device offers: saved, it syncs that folder with the offering device. */
    fun pairEditFromOffer(share: String, from: String, name: String) {
        controller.navigate("pair/new/edit?offerShare=${Uri.encode(share)}&offerFrom=${Uri.encode(from)}&offerName=${Uri.encode(name)}")
    }
    fun pairDetail(pairId: String) { controller.navigate("pair/$pairId") }
    fun selective(pairId: String) { controller.navigate("pair/$pairId/selective") }
    fun dryRun(pairId: String) { controller.navigate("pair/$pairId/dryrun") }
    fun localPicker(start: String? = null) { controller.navigate("picker/local" + (start?.let { "?start=${Uri.encode(it)}" } ?: "")) }
    fun remotePicker(accountId: String) { controller.navigate("picker/remote/${Uri.encode(accountId)}") }
    fun status() { controller.navigate(Routes.STATUS) }
    fun history() { controller.navigate(Routes.HISTORY) }
    fun runDetail(runId: String) { controller.navigate("run/$runId") }
    fun problems() { controller.navigate(Routes.PROBLEMS) }
    fun trash() { controller.navigate(Routes.TRASH) }
    fun revisions(accountId: String, fileId: String, name: String, pairId: String? = null, key: String? = null) {
        controller.navigate("revisions/${Uri.encode(accountId)}/${Uri.encode(fileId)}?name=${Uri.encode(name)}&pairId=${Uri.encode(pairId ?: "")}&key=${Uri.encode(key ?: "")}")
    }
    fun settings() { controller.navigate(Routes.SETTINGS) }
    fun diagnostics() { controller.navigate(Routes.DIAGNOSTICS) }
    fun vaults() { controller.navigate(Routes.VAULTS) }
    fun backup() { controller.navigate(Routes.BACKUP) }
    fun driveBrowser(accountId: String, folderId: String? = null) { controller.navigate("drive/${Uri.encode(accountId)}" + (folderId?.let { "?folderId=${Uri.encode(it)}" } ?: "")) }
    fun about() { controller.navigate(Routes.ABOUT) }
    fun conflict(conflictId: String) { controller.navigate("conflict/$conflictId") }
    /** Back to the pairs list (after removing a pair from its own screen). */
    fun backToPairs() { if (!controller.popBackStack(Routes.HOME, inclusive = false)) home() }
    fun reviewDeletions(pairId: String) { controller.navigate("pair/$pairId/deletions") }
    fun devices() { controller.navigate(Routes.DEVICES) { launchSingleTop = true } }
    fun devicePairing(mode: String) { controller.navigate("devices/pairing/$mode") }

    /** Switch between the four top-level destinations, keeping each one's state. */
    fun top(d: TopDestination) {
        val route = when (d) { TopDestination.Pairs -> Routes.HOME; TopDestination.Problems -> Routes.PROBLEMS; TopDestination.Bin -> Routes.TRASH; TopDestination.Settings -> Routes.SETTINGS }
        controller.navigate(route) {
            popUpTo(Routes.HOME) { saveState = true }
            launchSingleTop = true
            restoreState = true
        }
    }

    /** Deep links from notifications and the widget: "home", "problems", "trash", "settings", "history", "status", "pair:<id>", "review:<pairId>", "conflict:<id>", "devices". */
    fun route(name: String) {
        when {
            name.startsWith("pair:") -> pairDetail(name.removePrefix("pair:"))
            name.startsWith("review:") -> reviewDeletions(name.removePrefix("review:"))
            name.startsWith("conflict:") -> conflict(name.removePrefix("conflict:"))
            name == "home" -> top(TopDestination.Pairs)
            name == "problems" -> top(TopDestination.Problems)
            name == "trash" -> top(TopDestination.Bin)
            name == "settings" -> top(TopDestination.Settings)
            name == "status" -> status()
            name == "history" -> history()
            name == "accounts" -> accounts()
            name == "devices" -> devices()
        }
    }
}

@Composable
fun AppNavHost(nav: AppNav, startDestination: String, modifier: Modifier = Modifier) {
    val still = LocalReduceMotion.current
    val enter: EnterTransition = if (still) EnterTransition.None else fadeIn(tween(Motion.Long, easing = Motion.EmphasizedDecelerate)) + slideInHorizontally(tween(Motion.Long, easing = Motion.EmphasizedDecelerate)) { it / 10 }
    val exit: ExitTransition = if (still) ExitTransition.None else fadeOut(tween(Motion.Medium, easing = Motion.EmphasizedAccelerate))
    val popEnter: EnterTransition = if (still) EnterTransition.None else fadeIn(tween(Motion.Long, easing = Motion.EmphasizedDecelerate))
    val popExit: ExitTransition = if (still) ExitTransition.None else fadeOut(tween(Motion.Medium, easing = Motion.EmphasizedAccelerate)) + slideOutHorizontally(tween(Motion.Medium, easing = Motion.EmphasizedAccelerate)) { it / 10 }
    NavHost(
        navController = nav.controller, startDestination = startDestination, modifier = modifier,
        enterTransition = { if (initialState.destination.route in Routes.topLevel && targetState.destination.route in Routes.topLevel) (if (still) EnterTransition.None else fadeIn(tween(Motion.Medium))) else enter },
        exitTransition = { if (initialState.destination.route in Routes.topLevel && targetState.destination.route in Routes.topLevel) (if (still) ExitTransition.None else fadeOut(tween(Motion.Short))) else exit },
        popEnterTransition = { popEnter },
        popExitTransition = { popExit },
    ) {
        composable(Routes.ONBOARDING) { OnboardingScreen(nav) }
        composable(Routes.HOME) { HomeScreen(nav) }
        composable(Routes.ACCOUNTS) { AccountsScreen(nav) }
        composable(
            Routes.PAIR_EDIT,
            arguments = listOf(navArgument("pairId") { type = NavType.StringType }) +
                listOf("accountId", "offerShare", "offerFrom", "offerName").map { navArgument(it) { type = NavType.StringType; nullable = true; defaultValue = null } },
        ) { e ->
            PairEditorScreen(nav, e.arguments?.getString("pairId")?.takeIf { it != "new" }, e.arguments?.getString("accountId"))
        }
        composable(Routes.PAIR_DETAIL, arguments = listOf(navArgument("pairId") { type = NavType.StringType })) { e -> PairDetailScreen(nav, e.arguments!!.getString("pairId")!!) }
        composable(Routes.SELECTIVE, arguments = listOf(navArgument("pairId") { type = NavType.StringType })) { e -> SelectiveSyncScreen(nav, e.arguments!!.getString("pairId")!!) }
        composable(Routes.DRY_RUN, arguments = listOf(navArgument("pairId") { type = NavType.StringType })) { e -> DryRunScreen(nav, e.arguments!!.getString("pairId")!!) }
        composable(Routes.LOCAL_PICKER, arguments = listOf(navArgument("start") { type = NavType.StringType; nullable = true; defaultValue = null })) { e -> LocalFolderPickerScreen(nav, e.arguments?.getString("start")) }
        composable(Routes.REMOTE_PICKER, arguments = listOf(navArgument("accountId") { type = NavType.StringType })) { e -> RemoteFolderPickerScreen(nav, e.arguments!!.getString("accountId")!!) }
        composable(Routes.STATUS) { StatusScreen(nav) }
        composable(Routes.HISTORY) { HistoryScreen(nav) }
        composable(Routes.RUN_DETAIL, arguments = listOf(navArgument("runId") { type = NavType.StringType })) { e -> RunDetailScreen(nav, e.arguments!!.getString("runId")!!) }
        composable(Routes.PROBLEMS) { ProblemsScreen(nav) }
        composable(Routes.TRASH) { TrashScreen(nav) }
        composable(
            Routes.REVISIONS,
            arguments = listOf(
                navArgument("accountId") { type = NavType.StringType }, navArgument("fileId") { type = NavType.StringType },
                navArgument("name") { type = NavType.StringType; defaultValue = "" }, navArgument("pairId") { type = NavType.StringType; defaultValue = "" },
                navArgument("key") { type = NavType.StringType; defaultValue = "" },
            ),
        ) { e ->
            val a = e.arguments!!
            RevisionsScreen(nav, a.getString("accountId")!!, a.getString("fileId")!!, a.getString("name") ?: "", a.getString("pairId")?.takeIf { it.isNotEmpty() }, a.getString("key")?.takeIf { it.isNotEmpty() })
        }
        composable(Routes.SETTINGS) { SettingsScreen(nav) }
        composable(Routes.DIAGNOSTICS) { DiagnosticsScreen(nav) }
        composable(Routes.VAULTS) { VaultsScreen(nav) }
        composable(Routes.BACKUP) { BackupScreen(nav) }
        composable(Routes.DRIVE_BROWSER, arguments = listOf(navArgument("accountId") { type = NavType.StringType }, navArgument("folderId") { type = NavType.StringType; nullable = true; defaultValue = null })) { e ->
            DriveBrowserScreen(nav, e.arguments!!.getString("accountId")!!, e.arguments?.getString("folderId"))
        }
        composable(Routes.ABOUT) { AboutScreen(nav) }
        composable(Routes.CONFLICT, arguments = listOf(navArgument("conflictId") { type = NavType.StringType })) { e -> ConflictScreen(nav, e.arguments!!.getString("conflictId")!!) }
        composable(Routes.DEVICES) { DevicesScreen(nav) }
        composable(Routes.DEVICE_PAIRING, arguments = listOf(navArgument("mode") { type = NavType.StringType })) { PairingScreen(nav) }
        composable(Routes.REVIEW_DELETIONS, arguments = listOf(navArgument("pairId") { type = NavType.StringType })) { e -> ReviewDeletionsScreen(nav, e.arguments!!.getString("pairId")!!) }
    }
}
