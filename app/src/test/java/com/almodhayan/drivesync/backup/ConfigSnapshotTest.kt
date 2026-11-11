package com.almodhayan.drivesync.backup

import com.almodhayan.drivesync.data.db.ConflictPolicy
import com.almodhayan.drivesync.data.db.DocsFormat
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.db.RunStatus
import com.almodhayan.drivesync.data.db.SyncMode
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class ConfigSnapshotTest {

    private fun samplePair(id: String = "p1") = FolderPairEntity(
        id = id,
        accountId = "a@example.com",
        name = "Camera",
        localPath = "/storage/emulated/0/DCIM/Camera",
        remoteFolderId = "rf1",
        remotePath = "/Photos/Camera",
        driveId = null,
        mode = SyncMode.UPLOAD_ONLY,
        wifiOnly = false,
        ssidAllow = "Home, Office",
        includePatterns = "*.jpg\n*.mp4",
        maxFileSizeBytes = 123_456_789L,
        conflictPolicy = ConflictPolicy.NEWER_WINS,
        docsFormat = DocsFormat.PDF,
        encrypted = true,
        vaultId = "v1",
        intervalMinutes = 30,
        instantSync = false,
        createdAt = 1_000L,
        updatedAt = 2_000L,
        // runtime status that must never be exported
        lastRunAt = 999L,
        lastSuccessAt = 998L,
        lastStatus = RunStatus.FAILED,
        lastError = "boom-error",
        pendingApproval = true,
        pendingApprovalSummary = "summary-text",
        approveOnce = true,
        paused = true,
        pauseReason = "pause-text",
        pendingLocalChanges = 3,
    )

    private fun sampleSnapshot() = ConfigSnapshot(
        version = ConfigSnapshot.VERSION,
        exportedAt = 42L,
        deviceName = "Pixel 9",
        settings = SettingsSnapshot(globalIntervalMinutes = 15, theme = "dark", conflictPolicyDefault = "REMOTE_WINS", deviceName = "Pixel 9", autoBackupConfig = false),
        accounts = listOf("a@example.com", "b@example.com"),
        pairs = listOf(PairSnapshot.from(samplePair("p1")), PairSnapshot.from(samplePair("p2").copy(mode = SyncMode.TWO_WAY, driveId = "shared-drive", encrypted = false, vaultId = null))),
        vaults = listOf(VaultSnapshot(id = "v1", name = "Private", saltB64 = "c2FsdA==", iterations = 200_000, keyCheckB64 = "a2V5", createdAt = 5L)),
        pins = listOf(PinSnapshot(accountId = "a@example.com", fileId = "f1", isFolder = true, createdAt = 7L)),
    )

    @Test
    fun jsonRoundTripPreservesEverything() {
        val snap = sampleSnapshot()
        val json = snap.encode()
        val back = ConfigSnapshot.decode(json)
        assertEquals(snap, back)
        assertTrue(json.contains("\"version\": 1"))
        assertTrue(json.contains("\"mode\": \"UPLOAD_ONLY\""))
        assertTrue(json.contains("\"conflictPolicy\": \"NEWER_WINS\""))
        assertTrue(json.contains("\"docsFormat\": \"PDF\""))
    }

    @Test
    fun pairSnapshotCarriesConfigurationOnly() {
        val p = PairSnapshot.from(samplePair())
        assertEquals("p1", p.id)
        assertEquals("a@example.com", p.accountId)
        assertEquals("/storage/emulated/0/DCIM/Camera", p.localPath)
        assertEquals("rf1", p.remoteFolderId)
        assertEquals("UPLOAD_ONLY", p.mode)
        assertEquals(false, p.wifiOnly)
        assertEquals("Home, Office", p.ssidAllow)
        assertEquals(123_456_789L, p.maxFileSizeBytes)
        assertEquals(true, p.encrypted)
        assertEquals("v1", p.vaultId)
        assertEquals(30, p.intervalMinutes)
        assertEquals(false, p.instantSync)
        assertEquals(1_000L, p.createdAt)

        val json = ConfigSnapshot(version = 1, pairs = listOf(p)).encode()
        for (field in listOf("lastRunAt", "lastSuccessAt", "lastStatus", "lastError", "boom-error", "pendingApproval", "summary-text", "approveOnce", "paused", "pause-text", "pendingLocalChanges", "updatedAt")) {
            assertFalse("export must not contain $field", json.contains(field))
        }
    }

    @Test
    fun decodeToleratesUnknownAndMissingFields() {
        val json = """
            {
              "version": 2,
              "somethingFromTheFuture": {"x": 1},
              "pairs": [
                {"id": "p", "accountId": "a", "localPath": "/x", "remoteFolderId": "r", "mode": "two_way", "newOption": true}
              ]
            }
        """.trimIndent()
        val snap = ConfigSnapshot.decode(json)
        assertEquals(2, snap.version)
        assertEquals(1, snap.pairs.size)
        val p = snap.pairs[0]
        assertEquals("two_way", p.mode)
        assertEquals("", p.remotePath)
        assertNull(p.wifiOnly)
        assertNull(p.conflictPolicy)
        assertNull(snap.settings.globalIntervalMinutes)
        assertTrue(snap.accounts.isEmpty())
    }

    @Test
    fun decodeRejectsDocumentsThatAreNotBackups() {
        assertThrows(Exception::class.java) { ConfigSnapshot.decode("""{"pairs": []}""") }   // no version field
        assertThrows(Exception::class.java) { ConfigSnapshot.decode("not json at all") }
        assertThrows(Exception::class.java) { ConfigSnapshot.decode("[1, 2, 3]") }
    }

    @Test
    fun nullsAreOmittedFromTheJson() {
        val json = ConfigSnapshot(version = 1, pairs = listOf(PairSnapshot(id = "p", accountId = "a", localPath = "/x", remoteFolderId = "r", mode = "TWO_WAY"))).encode()
        assertFalse(json.contains("null"))
        assertFalse(json.contains("wifiOnly"))
    }

    @Test
    fun parseEnumIsLenient() {
        assertEquals(SyncMode.TWO_WAY, parseEnum<SyncMode>("two_way"))
        assertEquals(SyncMode.DOWNLOAD_MIRROR, parseEnum<SyncMode>(" DOWNLOAD_MIRROR "))
        assertEquals(ConflictPolicy.SKIP, parseEnum<ConflictPolicy>("skip"))
        assertNull(parseEnum<SyncMode>("bogus"))
        assertNull(parseEnum<SyncMode>(""))
        assertNull(parseEnum<SyncMode>(null))
    }
}
