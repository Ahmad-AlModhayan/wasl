package com.almodhayan.drivesync.engine

import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.kahflabs.wasl.ffi.EngineInit
import com.kahflabs.wasl.ffi.Platform
import com.kahflabs.wasl.ffi.SafBridge
import com.kahflabs.wasl.ffi.TokenSource
import com.kahflabs.wasl.ffi.VaultAccess
import com.kahflabs.wasl.ffi.WaslEngine
import com.kahflabs.wasl.ffi.core.DeviceState
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

/**
 * #150 M5, through the pair DAO the app uses (the real engine over JNA): a folder grant is released
 * only when the pair is finally deleted, also when it was hidden for Undo, and never while another
 * pair, hidden ones included, still uses the same folder.
 */
class FolderGrantsTest {
    @get:Rule val tmp = TemporaryFolder()

    private val tree = "content://com.android.externalstorage.documents/tree/primary%3ANotes"
    private val released = ArrayList<String>()
    private val grants = object : FolderGrants {
        override fun held(tree: String) = tree !in released
        override fun release(tree: String) { released += tree }
    }
    private lateinit var engine: WaslEngine

    private fun data(): EngineData {
        val dir = tmp.root
        engine = WaslEngine(
            EngineInit(
                dbPath = File(dir, "wasl.db").path, binDir = File(dir, "bin").path, stagingDir = File(dir, "staging").path,
                deviceName = "Pixel", driveBaseUrl = "http://127.0.0.1:9/drive/v3", driveUploadUrl = "http://127.0.0.1:9/upload/drive/v3",
            ),
            object : TokenSource {
                override fun accessToken(account: String) = "tok"
                override fun invalidate(account: String) {}
            },
            object : Platform {
                override fun deviceState() = DeviceState(online = true, wifi = true, metered = false, vpn = false, roaming = false, ssid = null, charging = true, batteryPercent = 100u)
                override fun saf(): SafBridge? = null
                override fun vaultAccess(vaultId: String): VaultAccess? = null
            },
            null,
        )
        return EngineData({ engine }, flowOf(Unit), {}, grants)
    }

    private fun pair(id: String, path: String, remote: String) = FolderPairEntity(
        id = id, accountId = "me@example.com", name = id, localPath = path, remoteFolderId = remote, remotePath = "My Drive/$id",
    )

    @After fun close() { if (::engine.isInitialized) engine.destroy() }

    @Test fun aPairRemovedWithUndoReleasesItsGrantWhenFinallyDeleted() = runBlocking {
        val data = data()
        data.pairs.upsert(pair("p", tree, "A"))
        data.hide("p")  // Remove, with Undo pending
        assertEquals(emptyList<String>(), released)
        data.pairs.delete("p")  // the Undo window closed
        assertEquals(listOf(tree), released)
    }

    // The engine refuses two pairs on the same folder, so a grant is shared by at most one pair;
    // the DAO still counts hidden pairs (FolderGrants.releasable gets every pair's folder).
    @Test fun onlyTheDeletedPairsGrantGoes() = runBlocking {
        val data = data()
        val other = "content://com.android.externalstorage.documents/tree/primary%3ADocs"
        data.pairs.upsert(pair("p", tree, "A"))
        data.pairs.upsert(pair("q", other, "B"))
        data.hide("q")
        data.pairs.delete("p")
        assertEquals(listOf(tree), released)
    }

    @Test fun aPathPairHasNoGrantToRelease() = runBlocking {
        val data = data()
        data.pairs.upsert(pair("p", tmp.newFolder("Docs").path, "A"))
        data.pairs.delete("p")
        assertEquals(emptyList<String>(), released)
    }
}
