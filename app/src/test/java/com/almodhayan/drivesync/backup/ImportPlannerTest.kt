package com.almodhayan.drivesync.backup

import com.almodhayan.drivesync.data.db.ConflictPolicy
import com.almodhayan.drivesync.data.db.DocsFormat
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.db.RunStatus
import com.almodhayan.drivesync.data.db.SyncMode
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ImportPlannerTest {
    private val account = "me@example.com"
    private val now = 1_700_000_000_000L

    private fun pair(
        id: String,
        accountId: String = account,
        localPath: String = "/storage/emulated/0/Documents",
        remoteFolderId: String = "rf-$id",
        mode: String = "TWO_WAY",
        name: String = "Pair $id",
        encrypted: Boolean = false,
        vaultId: String? = null,
        conflictPolicy: String? = null,
        wifiOnly: Boolean? = null,
    ) = PairSnapshot(
        id = id, accountId = accountId, name = name, localPath = localPath, remoteFolderId = remoteFolderId, remotePath = "/Remote/$id",
        mode = mode, encrypted = encrypted, vaultId = vaultId, conflictPolicy = conflictPolicy, wifiOnly = wifiOnly,
    )

    private fun snapshot(vararg pairs: PairSnapshot, accounts: List<String> = listOf(account), vaults: List<VaultSnapshot> = emptyList(), pins: List<PinSnapshot> = emptyList(), version: Int = 1) =
        ConfigSnapshot(version = version, exportedAt = now, deviceName = "Old phone", accounts = accounts, pairs = pairs.toList(), vaults = vaults, pins = pins)

    private fun existing(id: String) = FolderPairEntity(
        id = id, accountId = account, name = "Existing $id", localPath = "/old/path/$id", remoteFolderId = "old-rf-$id", remotePath = "/Old/$id",
        mode = SyncMode.UPLOAD_ONLY, wifiOnly = false, conflictPolicy = ConflictPolicy.LOCAL_WINS, createdAt = 10L,
        lastRunAt = 500L, lastSuccessAt = 400L, lastStatus = RunStatus.SUCCESS, pendingLocalChanges = 4,
    )

    private val pkg = "com.kahflabs.wasl"
    private val roots = listOf("/data/user/0/$pkg", "/storage/emulated/0/Android/data/$pkg")

    private fun plan(snap: ConfigSnapshot, mode: ImportMode = ImportMode.MERGE, existingPairs: List<FolderPairEntity> = emptyList(), accounts: Set<String> = setOf(account), vaults: Set<String> = emptySet()) =
        ImportPlanner.plan(snap, mode, existingPairs, accounts, vaults, now, roots, pkg)

    @Test
    fun skipsInvalidPairsWithReasons() {
        val p = plan(
            snapshot(
                pair("ok"),
                pair("noPath", localPath = "  "),
                pair("badMode", mode = "SIDEWAYS"),
                pair("", name = "no id"),
                pair("ok", name = "duplicate"),
                pair("noRemote", remoteFolderId = ""),
                pair("noAccount", accountId = ""),
            ),
        )
        assertEquals(listOf("ok"), p.pairsToUpsert.map { it.id })
        assertEquals(1, p.pairsAdded)
        assertEquals(6, p.pairsSkipped)
        assertEquals(6, p.skipped.size)
        assertTrue(p.skipped.any { it.contains("device folder path is empty") })
        assertTrue(p.skipped.any { it.contains("unknown sync mode \"SIDEWAYS\"") })
        assertTrue(p.skipped.any { it.contains("missing id") })
        assertTrue(p.skipped.any { it.contains("duplicate id ok") })
        assertTrue(p.skipped.any { it.contains("Drive folder id is empty") })
        assertTrue(p.skipped.any { it.contains("no Google account") })
    }

    @Test
    fun pairsOfMissingAccountsAreImportedDisabledAndPaused() {
        val other = "other@example.com"
        val p = plan(snapshot(pair("a"), pair("b", accountId = other), accounts = listOf(account, other)))
        val b = p.pairsToUpsert.first { it.id == "b" }
        assertFalse(b.enabled)
        assertTrue(b.paused)
        assertEquals("Sign in to $other, then resume", b.pauseReason)
        val a = p.pairsToUpsert.first { it.id == "a" }
        assertTrue(a.enabled)
        assertFalse(a.paused)
        assertEquals(1, p.pairsPaused)
        assertEquals(listOf(other), p.accountsToSignIn)
    }

    @Test
    fun mergeUpdatesConfigurationButKeepsRuntimeStateAndUnsetOptions() {
        val ex = existing("p1")
        val p = plan(snapshot(pair("p1", name = "Renamed", mode = "DOWNLOAD_ONLY", wifiOnly = null, conflictPolicy = null)), existingPairs = listOf(ex))
        assertEquals(0, p.pairsAdded)
        assertEquals(1, p.pairsUpdated)
        assertTrue(p.pairIdsToRemove.isEmpty())
        val e = p.pairsToUpsert.single()
        assertEquals("Renamed", e.name)
        assertEquals(SyncMode.DOWNLOAD_ONLY, e.mode)
        assertEquals("/storage/emulated/0/Documents", e.localPath)
        assertEquals("rf-p1", e.remoteFolderId)
        // unset options keep the existing pair's values
        assertEquals(false, e.wifiOnly)
        assertEquals(ConflictPolicy.LOCAL_WINS, e.conflictPolicy)
        // runtime state survives
        assertEquals(500L, e.lastRunAt)
        assertEquals(RunStatus.SUCCESS, e.lastStatus)
        assertEquals(4, e.pendingLocalChanges)
        assertEquals(10L, e.createdAt)
        assertEquals(now, e.updatedAt)
    }

    @Test
    fun replaceRemovesAllExistingPairsAndAddsFreshOnes() {
        val p = plan(snapshot(pair("p1"), pair("p9")), mode = ImportMode.REPLACE, existingPairs = listOf(existing("p1"), existing("p2")))
        assertEquals(listOf("p1", "p2"), p.pairIdsToRemove)
        assertEquals(2, p.pairsAdded)
        assertEquals(0, p.pairsUpdated)
        val p1 = p.pairsToUpsert.first { it.id == "p1" }
        assertNull(p1.lastRunAt)          // fresh entity, not the old one
        assertEquals(true, p1.wifiOnly)   // entity default, not the old pair's false
    }

    @Test
    fun newEncryptedPairsArePausedUntilThePassphraseIsEntered() {
        val vault = VaultSnapshot(id = "v1", name = "Private", saltB64 = "c2FsdA==", iterations = 100_000, keyCheckB64 = "a2V5")
        val p = plan(snapshot(pair("enc", encrypted = true, vaultId = "v1"), vaults = listOf(vault)))
        val e = p.pairsToUpsert.single()
        assertTrue(e.paused)
        assertEquals(ImportPlanner.PAUSE_REASON_VAULT, e.pauseReason)
        assertTrue(e.enabled)
        assertEquals(1, p.pairsPaused)
        assertTrue(p.needsVaultPassphrase)
        assertTrue(p.warnings.none { it.contains("vault is not") })

        // Existing encrypted pair on the same device (merge): its state is left alone.
        val ex = existing("enc").copy(encrypted = true, vaultId = "v1", paused = false)
        val merged = plan(snapshot(pair("enc", encrypted = true, vaultId = "v1")), existingPairs = listOf(ex), vaults = setOf("v1"))
        assertFalse(merged.pairsToUpsert.single().paused)
        assertEquals(0, merged.pairsPaused)
    }

    @Test
    fun encryptedPairWithUnknownVaultIsImportedWithWarning() {
        val p = plan(snapshot(pair("enc", encrypted = true, vaultId = "gone")))
        assertEquals(1, p.pairsToUpsert.size)
        assertTrue(p.warnings.any { it.contains("vault is not in the backup or on this device") })
    }

    @Test
    fun unknownEnumOptionsFallBackWithWarning() {
        val p = plan(snapshot(pair("p1", conflictPolicy = "COIN_TOSS").copy(docsFormat = "PAPYRUS")))
        val e = p.pairsToUpsert.single()
        assertEquals(ConflictPolicy.KEEP_BOTH, e.conflictPolicy)
        assertEquals(DocsFormat.OFFICE, e.docsFormat)
        assertEquals(2, p.warnings.size)
        assertTrue(p.warnings.any { it.contains("COIN_TOSS") })
        assertTrue(p.warnings.any { it.contains("PAPYRUS") })
    }

    @Test
    fun blankNameFallsBackToFolderName() {
        val p = plan(snapshot(pair("p1", name = "", localPath = "/storage/emulated/0/DCIM/Camera/")))
        assertEquals("Camera", p.pairsToUpsert.single().name)
    }

    @Test
    fun newerFormatVersionOnlyWarns() {
        val p = plan(snapshot(pair("p1"), version = 7))
        assertEquals(1, p.pairsAdded)
        assertTrue(p.warnings.any { it.contains("newer Wasl") })
    }

    @Test
    fun vaultsAndPinsAreValidated() {
        val good = VaultSnapshot(id = "v1", name = "Private", saltB64 = "c2FsdA==", iterations = 100_000, keyCheckB64 = "a2V5", createdAt = 3L)
        val incomplete = VaultSnapshot(id = "v2", name = "Broken", saltB64 = "", iterations = 0, keyCheckB64 = "")
        val duplicate = good.copy(name = "Again")
        val p = plan(
            snapshot(
                vaults = listOf(good, incomplete, duplicate),
                pins = listOf(
                    PinSnapshot(accountId = account, fileId = "f1", isFolder = false, createdAt = 1L),
                    PinSnapshot(accountId = account, fileId = "f1", isFolder = false),  // duplicate, collapsed silently
                    PinSnapshot(accountId = account, fileId = "", isFolder = true),     // invalid
                ),
            ),
        )
        assertEquals(listOf("v1"), p.vaults.map { it.id })
        assertEquals("Private", p.vaults.single().name)
        assertEquals(100_000, p.vaults.single().iterations)
        assertEquals(3L, p.vaults.single().createdAt)
        assertEquals(1, p.pins.size)
        assertEquals("f1", p.pins.single().fileId)
        assertEquals(1L, p.pins.single().createdAt)
        assertTrue(p.skipped.any { it.contains("Vault \"Broken\": incomplete metadata") })
        assertTrue(p.skipped.any { it.contains("duplicate id v1") })
        assertTrue(p.skipped.any { it.contains("Pin: missing account or file id") })
    }

    @Test
    fun accountsToSignInCombinesAccountListAndPairAccounts() {
        val p = plan(
            snapshot(pair("p1", accountId = "x@example.com"), accounts = listOf(account, "y@example.com")),
            accounts = setOf(account),
        )
        assertEquals(listOf("y@example.com", "x@example.com"), p.accountsToSignIn)
    }

    @Test
    fun settingsArePassedThroughUnchanged() {
        val settings = SettingsSnapshot(globalIntervalMinutes = 30, theme = "light")
        val p = plan(snapshot().copy(settings = settings))
        assertEquals(settings, p.settings)
    }

    // security review 2026-10: an imported file must not start syncing anything unseen.

    @Test
    fun newAndRepointedPairsAreHeldForPreview() {
        val fresh = plan(snapshot(pair("new")))
        assertTrue(fresh.pairsToUpsert.single().holdForPreview)

        val replaced = plan(snapshot(pair("p1")), mode = ImportMode.REPLACE, existingPairs = listOf(existing("p1")))
        assertTrue(replaced.pairsToUpsert.single().holdForPreview)

        // Merge: same folder, Drive folder and mode keeps running as it did.
        val ex = existing("p1")
        val same = pair("p1", localPath = ex.localPath, remoteFolderId = ex.remoteFolderId, mode = ex.mode.name, name = "Renamed")
        assertFalse(plan(snapshot(same), existingPairs = listOf(ex)).pairsToUpsert.single().holdForPreview)
        // Any of the three changed: held.
        assertTrue(plan(snapshot(same.copy(localPath = "/storage/emulated/0/Other")), existingPairs = listOf(ex)).pairsToUpsert.single().holdForPreview)
        assertTrue(plan(snapshot(same.copy(remoteFolderId = "elsewhere")), existingPairs = listOf(ex)).pairsToUpsert.single().holdForPreview)
        assertTrue(plan(snapshot(same.copy(mode = "DOWNLOAD_MIRROR")), existingPairs = listOf(ex)).pairsToUpsert.single().holdForPreview)
        // A pair already waiting for its preview keeps waiting.
        assertTrue(plan(snapshot(same), existingPairs = listOf(ex.copy(holdForPreview = true))).pairsToUpsert.single().holdForPreview)
    }

    @Test
    fun newPairsKeepTheDeletionSafeguards() {
        val p = plan(snapshot(pair("new").copy(massDeleteGuard = false, propagateDeletions = false)))
        val e = p.pairsToUpsert.single()
        assertTrue(e.massDeleteGuard)
        assertTrue(e.propagateDeletions)
        assertTrue(p.warnings.any { it.contains("deletion safeguards stay on") })

        // Values that match the defaults say nothing.
        assertTrue(plan(snapshot(pair("new").copy(massDeleteGuard = true, propagateDeletions = true))).warnings.isEmpty())

        // An existing pair on this phone takes the file's choice (the user made it there).
        val merged = plan(snapshot(pair("p1").copy(massDeleteGuard = false, propagateDeletions = false)), existingPairs = listOf(existing("p1")))
        assertFalse(merged.pairsToUpsert.single().massDeleteGuard)
        assertFalse(merged.pairsToUpsert.single().propagateDeletions)
    }

    @Test
    fun pairsInsideTheAppsPrivateStorageAreRefused() {
        val p = plan(
            snapshot(
                pair("ok"),
                pair("data", localPath = "/data/user/0/$pkg/files"),
                pair("alias", localPath = "/data/data/$pkg/databases"),
                pair("ext", localPath = "/sdcard/Android/data/$pkg/files"),
                pair("case", localPath = "/storage/emulated/0/android/DATA/$pkg"),
                pair("dots", localPath = "/storage/emulated/0/Documents/../Android/data/$pkg/cache"),
                pair("own", localPath = "content://$pkg.documents/tree/root"),
                pair("saf", localPath = "content://com.android.externalstorage.documents/tree/primary%3AAndroid%2Fdata%2F$pkg"),
            ),
        )
        assertEquals(listOf("ok"), p.pairsToUpsert.map { it.id })
        assertEquals(7, p.pairsSkipped)
        assertEquals(7, p.skipped.count { it.contains("private storage") })
    }

    @Test
    fun v1VaultsOutsideTheIterationBoundsAreLeftOut() {
        val weak = VaultSnapshot(id = "v1", name = "Weak", saltB64 = "c2FsdA==", iterations = 1_000, keyCheckB64 = "a2V5")
        val huge = VaultSnapshot(id = "v2", name = "Huge", saltB64 = "c2FsdA==", iterations = Int.MAX_VALUE, keyCheckB64 = "a2V5")
        val p = plan(snapshot(vaults = listOf(weak, huge)))
        assertTrue(p.vaults.isEmpty())
        assertEquals(2, p.skipped.count { it.contains("incomplete metadata") })
    }
}
