package com.almodhayan.drivesync.ui

import android.app.Application
import android.graphics.Bitmap
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.activity.ComponentActivity
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.unit.dp
import com.almodhayan.drivesync.ui.components.*
import com.almodhayan.drivesync.ui.theme.DriveSyncTheme
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import com.almodhayan.drivesync.ui.theme.SyncStatus
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import java.io.File
import java.io.FileOutputStream

/**
 * Renders the design system's Compose components to PNGs (app/build/design-screens/) so the UI can be
 * checked without a device. Not an assertion test: it only fails if rendering crashes.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], application = Application::class, qualifiers = "w360dp-h900dp-xxhdpi")
class DesignGalleryTest {
    @get:Rule val compose = createAndroidComposeRule<ComponentActivity>()

    private fun shoot(name: String, dark: Boolean = false, content: @Composable () -> Unit) {
        compose.mainClock.autoAdvance = false
        compose.setContent {
            DriveSyncTheme(theme = if (dark) "dark" else "light", dynamicColor = false) {
                Box(Modifier.background(MaterialTheme.colorScheme.surface)) { content() }
            }
        }
        compose.mainClock.advanceTimeBy(700)
        compose.activityRule.scenario.onActivity { activity ->
            // Draw the window straight onto a bitmap (Robolectric native graphics); avoids waiting for a
            // frame callback that never comes while the paused clock holds the infinite sync animation.
            val root = activity.window.decorView
            val bmp = Bitmap.createBitmap(root.width, root.height, Bitmap.Config.ARGB_8888)
            root.draw(android.graphics.Canvas(bmp))
            val dir = File(System.getProperty("user.dir"), "build/design-screens").apply { mkdirs() }
            FileOutputStream(File(dir, "$name.png")).use { bmp.compress(Bitmap.CompressFormat.PNG, 100, it) }
        }
    }

    private val GB = 1024L * 1024 * 1024

    @Test fun brand() = shoot("01-brand") {
        Column(Modifier.padding(Space.s16), verticalArrangement = Arrangement.spacedBy(Space.s24)) {
            DriveSyncLockup(height = 40.dp)
            Row(horizontalArrangement = Arrangement.spacedBy(Space.s24)) {
                AppIconTile(size = 72.dp); DriveSyncMark(size = 48.dp); DriveSyncMark(size = 28.dp); DriveSyncMark(size = 18.dp)
            }
            Row(horizontalArrangement = Arrangement.spacedBy(Space.s16)) {
                listOf(Sym.CloudDoneFill, Sym.Sync, Sym.Wifi, Sym.FrontHandFill, Sym.CallSplit, Sym.ErrorFill, Sym.Mobile, Sym.Cloud, Sym.FolderFill).forEach { Icon(it, null) }
            }
            Text("Your phone and Drive, kept the same", style = MaterialTheme.typography.headlineMedium)
            Text("IBM Plex Sans for everything people read. 1,204 files · 2.4 GB", style = MaterialTheme.typography.bodyLarge)
            Text("Documents/Work/Invoice 0941.pdf", style = com.almodhayan.drivesync.ui.theme.MonoMedium)
            DsNavigationBar(current = TopDestination.Pairs, problems = 3, onSelect = {})
        }
    }

    @Test fun status() = shoot("02-status") {
        Column(Modifier.padding(Space.s16), verticalArrangement = Arrangement.spacedBy(Space.s12)) {
            SyncStatus.entries.forEach { s -> Row(horizontalArrangement = Arrangement.spacedBy(Space.s12)) { StatusChip(s, progress = if (s == SyncStatus.Syncing) 0.42f else null, count = if (s == SyncStatus.Conflict) 2 else 0); StatusChip(s, variant = ChipVariant.Tonal, size = ChipSize.Sm); StatusChip(s, variant = ChipVariant.Icon) } }
            StatusBanner(SyncStatus.Approval, "Waiting for your OK", body = "41 files were deleted from Work on your phone. Drive still has them.", primary = UiAction("Review 41 deletions") {})
            StatusBanner(SyncStatus.Failed, "Can't reach the Scans folder", body = "Android removed access after an update.", primary = UiAction("Grant access again") {})
            Callout("The Drive folder already has files. The first sync merges both folders and deletes nothing.")
            Callout("On mobile data this may use your plan's allowance.", tone = CalloutTone.Warning, title = "First sync is about 2.1 GB", action = UiAction("Turn on Only on Wi‑Fi") {})
            Callout("Deleting photos here to free space will delete them on Drive too.", tone = CalloutTone.Danger, title = "Freeing space will empty your backup")
        }
    }

    @Composable private fun HomeMock() {
        Scaffold(
            topBar = { DsTopAppBar("Drive Sync", navIcon = NavIcon.None, logo = true, actions = { IconButton({}) { Icon(Sym.Sync, "Sync all now") }; IconButton({}) { Icon(Sym.AccountCircle, "Account") } }) },
            floatingActionButton = { DsFab("New pair", Sym.Add, expanded = true, onClick = {}) },
            bottomBar = { DsNavigationBar(TopDestination.Pairs, problems = 3, onSelect = {}) },
        ) { p ->
            Column(Modifier.padding(p).padding(horizontal = Space.s16), verticalArrangement = Arrangement.spacedBy(Space.s12)) {
                StatusBanner(SyncStatus.Approval, "Waiting for your OK", body = "41 files were deleted from Work on your phone. Drive still has them.", primary = UiAction("Review 41 deletions") {})
                FolderPairCard(
                    PairCardModel("Camera backup", PairDirection.PhoneToDrive, SyncStatus.Syncing, conditions = "Wi‑Fi only", phonePath = "DCIM / Camera", drivePath = "My Drive / Photos / Pixel",
                        progress = SyncProgressModel(false, 0.37f, 142, 380, (1.2 * GB).toLong(), (3.4 * GB).toLong(), TransferDirection.Up, "IMG_2041.jpg", "About 6 min left"),
                        storage = listOf(StorageModel(Side.Drive, 15 * GB, (3.4 * GB).toLong(), (6.1 * GB).toLong()))),
                    menu = listOf(MenuAction("Sync now") {}), onPause = {},
                )
                FolderPairCard(PairCardModel("Music", PairDirection.DriveToPhone, SyncStatus.Waiting, lastSync = "Synced yesterday", conditions = "Wi‑Fi only", phonePath = "Music", drivePath = "My Drive / Music"), menu = listOf(MenuAction("Sync now") {}))
            }
        }
    }

    @Test fun homeLight() = shoot("03-home-light") { HomeMock() }
    @Test fun homeDark() = shoot("04-home-dark", dark = true) { HomeMock() }

    @Test fun editor() = shoot("05-pair-editor") {
        Scaffold(
            topBar = { DsTopAppBar("New pair", navIcon = NavIcon.Close, actions = { TextButton({}) { Text("Save") } }) },
            bottomBar = { BottomActionBar(hint = "Nothing syncs before the preview.") { Button({}) { Icon(Sym.Visibility, null); Text("  Preview first sync") } } },
        ) { p ->
            Column(Modifier.padding(p).padding(horizontal = Space.s16), verticalArrangement = Arrangement.spacedBy(Space.s8)) {
                DsTextField("Work documents", {}, "Name")
                SectionHeader("Folders", Modifier.padding(start = 0.dp))
                PathRow(Side.Phone, "Documents / Work", meta = "388 files · 2.1 GB", onPick = {})
                PathRow(Side.Drive, "My Drive / Work", meta = "1,204 files · 2 GB", onPick = {})
                Callout("The Drive folder already has files. The first sync merges both folders and deletes nothing.")
                SectionHeader("Sync mode")
                SyncModeOption(PairDirection.TwoWay, true, "Edits go both ways. Deletions wait in the bin.", {}, recommended = true)
                SyncModeOption(PairDirection.PhoneToDrive, false, "Copies to Drive. Never changes your phone.", {})
                SyncModeOption(PairDirection.DriveToPhone, false, "Copies to your phone. Never changes Drive.", {})
            }
        }
    }

    @Test fun dryRun() = shoot("06-dry-run") {
        Column {
            DsTopAppBar("Preview first sync", subtitle = "Work documents")
            ChangeSummary(1204, listOf(SummaryItem(ChangeAction.Upload, 34, "48 MB"), SummaryItem(ChangeAction.Download, 12, "9.1 MB"), SummaryItem(ChangeAction.Bin, 0), SummaryItem(ChangeAction.Conflict, 2, label = "Conflicts")), Modifier.padding(Space.s16))
            FilterChipRow(listOf(ChipModel("All", true, 51) {}, ChipModel("To Drive", false, 34) {}, ChipModel("To phone", false, 12) {}))
            ChangeRow(ChangeAction.Conflict, "Budget 2027.xlsx", "Work", size = "88 KB", reason = "Changed on both")
            ChangeRow(ChangeAction.Bin, "Draft v1.docx", "Work/Notes", size = "44 KB", reason = "Deleted on phone")
            ChangeRow(ChangeAction.Upload, "Invoice 0941.pdf", "Work/Invoices", size = "210 KB", reason = "New")
            ChangeRow(ChangeAction.Download, "Lease 2026.pdf", "Work/Contracts", size = "380 KB", reason = "Changed on Drive")
        }
    }

    @Test fun conflictAndVersions() = shoot("07-conflict-versions") {
        Column(Modifier.padding(vertical = Space.s16), verticalArrangement = Arrangement.spacedBy(Space.s12)) {
            ConflictCard("Budget 2027.xlsx", "Work", ConflictSideInfo("Today 09:12", "88 KB · this phone"), ConflictSideInfo("Today 09:40", "91 KB · edited on Drive"), Modifier.padding(horizontal = Space.s16), chosen = null, onChoose = {})
            SectionHeader("Versions")
            VersionItem("Today 09:40", VersionSource.Drive, "91 KB", current = true, first = true)
            VersionItem("Today 09:12", VersionSource.Phone, "88 KB", onRestore = {}, onSaveCopy = {})
            VersionItem("Sep 28, 14:03", VersionSource.Conflict, "86 KB", note = "replaced on this phone", last = true, onRestore = {}, onSaveCopy = {})
        }
    }

    @Test fun diagnostics() = shoot("08-diagnostics") {
        Column(Modifier.padding(vertical = Space.s8)) {
            HealthCheckRow(HealthState.Ok, "Folder access", "All files access is on")
            HealthCheckRow(HealthState.Warn, "Battery optimisation", "Android may stop syncs in the background", actionLabel = "Fix", onAction = {})
            HealthCheckRow(HealthState.Fail, "Drive storage", "Drive has no space left for new files", actionLabel = "Free up space", onAction = {})
            HealthCheckRow(HealthState.Wait, "Network", "Waiting for Wi‑Fi")
            SectionHeader("Last 24 hours")
            LogView(listOf(LogLine("10:31:04", LogLevel.OK, "Work · Invoice 0941.pdf: Uploaded"), LogLine("10:31:09", LogLevel.WARN, "Work · Budget 2027.xlsx: Both sides changed"), LogLine("10:32:40", LogLevel.ERROR, "Scans: Can't reach the folder"), LogLine("10:33:00", LogLevel.INFO, "Camera backup: Comparing folders")))
            SectionHeader("Sync defaults")
            SettingRow("Only on Wi‑Fi", icon = Sym.Wifi, summary = "Off: mobile data allowed", control = SettingControl.Toggle(false) {})
            SettingRow("Ask before large deletes", icon = Sym.FrontHand, summary = "More than 20 files or 5% of a folder", control = SettingControl.Toggle(true) {})
            SettingRow("Disconnect Google account", icon = Sym.Logout, danger = true, control = SettingControl.None)
            PermissionStep(Sym.AccountCircle, "Google Drive", "Read and write the Drive folders you pair.", PermissionState.Granted, {}, Modifier.padding(Space.s16))
            PermissionStep(Sym.FolderOpen, "Files on this phone", "Copy the phone folders you choose.", PermissionState.Next, {}, Modifier.padding(horizontal = Space.s16))
        }
    }
}
