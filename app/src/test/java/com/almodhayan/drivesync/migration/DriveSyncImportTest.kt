package com.almodhayan.drivesync.migration

import com.almodhayan.drivesync.backup.ImportMode
import com.almodhayan.drivesync.backup.ImportReport
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.engine.toEngine
import com.kahflabs.wasl.ffi.EngineInit
import com.kahflabs.wasl.ffi.Platform
import com.kahflabs.wasl.ffi.SafBridge
import com.kahflabs.wasl.ffi.TokenSource
import com.kahflabs.wasl.ffi.VaultAccess
import com.kahflabs.wasl.ffi.WaslEngine
import com.kahflabs.wasl.ffi.core.DeviceState
import com.kahflabs.wasl.ffi.core.RunStatus
import com.kahflabs.wasl.ffi.store.BinSide
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.ByteArrayOutputStream
import java.io.File
import java.util.zip.ZipEntry
import java.util.zip.ZipOutputStream

/** Wasl's side of the Drive Sync 1.1.5 export (SRS FR-K2, docs/MIGRATION.md), on the real engine. */
class DriveSyncImportTest {
    @get:Rule val tmp = TemporaryFolder()
    private lateinit var engine: WaslEngine
    private lateinit var root: File

    private val now = 1_760_000_000_000L

    @Before fun setUp() {
        root = tmp.newFolder("Notes")
        engine = WaslEngine(
            EngineInit(
                dbPath = File(tmp.root, "wasl.db").path,
                binDir = File(tmp.root, "bin").path,
                stagingDir = File(tmp.root, "staging").path,
                deviceName = "Pixel",
                driveBaseUrl = "http://127.0.0.1:9/drive/v3",
                driveUploadUrl = "http://127.0.0.1:9/upload/drive/v3",
            ),
            object : TokenSource {
                override fun accessToken(account: String) = "tok"
                override fun invalidate(account: String) {}
            },
            object : Platform {
                override fun deviceState() = DeviceState(true, true, false, false, false, null, true, 100u)
                override fun saf(): SafBridge? = null
                override fun vaultAccess(vaultId: String): VaultAccess? = null
            },
            null,
        )
    }

    @After fun tearDown() = engine.close()

    private fun migrationJson() = """
        {
          "format": "drive-sync-to-wasl", "version": 1, "exportedAt": $now, "appVersion": "1.1.5",
          "config": { "version": 1, "pairs": [ { "id": "p1" } ] },
          "pairs": [ {
            "pairId": "p1", "lastStatus": "SUCCESS",
            "baseline": [
              { "key": "a.txt", "localRel": "a.txt", "isDir": false, "localSize": 5, "localMtime": ${now - 1000},
                "localHash": null, "remoteId": "id-a", "remoteMd5": "m", "remoteSize": 5, "remoteMtime": ${now - 1000}, "remoteVersion": 3, "exportMime": null },
              { "key": "ملف.txt", "localRel": "ملف.txt", "isDir": false, "localSize": 2, "localMtime": 1 }
            ],
            "runs": [ { "id": "r1", "pairId": "p1", "startedAt": ${now - 5000}, "finishedAt": ${now - 4000}, "status": "SUCCESS",
                        "trigger": "schedule", "uploaded": 1, "bytesUp": 5, "message": null } ],
            "events": [ { "runId": "r1", "pairId": "p1", "at": ${now - 4500}, "level": "INFO", "action": "UPLOAD", "path": "a.txt",
                          "message": "Uploaded", "detail": null, "bytes": 5 } ],
            "conflicts": [ { "id": "c1", "pairId": "p1", "key": "b.txt", "detectedAt": $now, "policy": "KEEP_BOTH",
                             "localCopyRel": null, "remoteFileId": null, "message": "both", "resolved": false, "resolvedAt": null } ]
          }, { "pairId": "not-imported", "baseline": [] },
             { "pairId": "mine", "baseline": [ { "key": "x.txt", "localRel": "x.txt", "isDir": false } ] } ],
          "bin": [
            { "id": "t1", "pairId": "p1", "originalPath": "/storage/emulated/0/Notes/dir/old.txt", "size": 5, "reason": "OVERWRITTEN",
              "createdAt": ${now - 1000}, "side": "phone", "file": "bin/t1" },
            { "id": "t2", "pairId": "p1", "originalPath": "/Notes/gone.txt", "size": 9, "reason": "DELETED", "createdAt": ${now - 1000},
              "side": "drive", "accountId": "me@example.com", "remoteFileId": "rid", "syncKey": "gone.txt" },
            { "id": "t3", "pairId": "p1", "originalPath": "x/lost.txt", "size": 1, "reason": "DELETED", "createdAt": 1, "side": "phone" }
          ]
        }
    """.trimIndent()

    private fun zip(entries: Map<String, ByteArray>): ByteArray = ByteArrayOutputStream().also { b ->
        ZipOutputStream(b).use { z -> entries.forEach { (name, bytes) -> z.putNextEntry(ZipEntry(name)); z.write(bytes); z.closeEntry() } }
    }.toByteArray()

    /** The config step as Settings › Backup does it, reduced to what matters here: the pair with its id. */
    private val importConfig: suspend (String) -> ImportReport = {
        if (engine.pair("p1") == null) {
            engine.createPair(
                FolderPairEntity(id = "p1", accountId = "me@example.com", name = "Notes", localPath = "/storage/emulated/0/Notes", remoteFolderId = "F", remotePath = "/Notes")
                    .toEngine(null),
            )
        }
        ImportReport(ImportMode.MERGE, 1, 0, 0, 0, 0, 0, 0, 0, true, emptyList(), false, emptyList(), emptyList())
    }

    private fun importOnce(): DriveSyncImport.Report = runBlocking {
        val work = tmp.newFolder()
        val bytes = zip(
            mapOf(
                "migration.json" to migrationJson().toByteArray(),
                "bin/t1" to "older".toByteArray(),
                "bin/../../evil" to "x".toByteArray(),
                "other.txt" to "ignored".toByteArray(),
            ),
        )
        val (json, bin) = unpackDriveSyncExport(bytes.inputStream(), work)
        assertEquals(setOf("t1"), bin.keys)
        applyDriveSyncExport(engine, json, bin, importConfig)
    }

    @Test fun `a Drive Sync export arrives whole, held for a preview, and only once`() {
        // A pair of Wasl's own, not in the file's configuration: the file cannot give it a baseline.
        engine.createPair(
            FolderPairEntity(id = "mine", accountId = "me@example.com", name = "Mine", localPath = "/storage/emulated/0/Mine", remoteFolderId = "M", remotePath = "/Mine")
                .toEngine(null),
        )
        // Previewed already: the import must leave a pair it doesn't bring as it is.
        engine.setPairFlags("mine", null, null, false)
        val r = importOnce()
        assertTrue(engine.baseline("mine").isEmpty())
        assertTrue(!engine.pair("mine")!!.status.holdForPreview)
        assertEquals(listOf(1, 2, 1, 1, 2, 1), listOf(r.pairs, r.baselineRows, r.runs, r.conflicts, r.binImported, r.binFailed))

        assertEquals(setOf("a.txt", "ملف.txt"), engine.baseline("p1").map { it.key }.toSet())
        assertEquals(listOf(RunStatus.SUCCESS), engine.runs("p1", 10u).map { it.status })
        assertEquals("UPLOAD", engine.runEvents("r1").single().action)
        assertEquals(listOf("c1"), engine.openConflicts("p1").map { it.id })
        assertTrue("the first run is previewed", engine.pair("p1")!!.status.holdForPreview)

        val t1 = engine.binEntry("t1")!!.entry
        assertEquals(BinSide.LOCAL, t1.side)
        assertEquals("dir/old.txt", t1.key)
        assertEquals("older", File(tmp.root, "bin/p1/${t1.location}").readText())
        val t2 = engine.binEntry("t2")!!.entry
        assertEquals(BinSide.DRIVE, t2.side)
        assertEquals("gone.txt", t2.key)

        val again = importOnce()
        assertEquals(listOf(0, 0, 0, 0), listOf(again.baselineRows, again.runs, again.conflicts, again.binImported))
        assertEquals(2, engine.binEntries("p1").size)
    }

    /** A file written by Drive Sync 1.1.5's own exporter (its WaslExportTest), so the two sides meet. */
    @Test fun `a file from the real Drive Sync 1_1_5 exporter imports`() = runBlocking {
        val bytes = javaClass.getResourceAsStream("/drive-sync-1.1.5-export.zip")!!.readBytes()
        val (json, bin) = unpackDriveSyncExport(bytes.inputStream(), tmp.newFolder())
        val r = applyDriveSyncExport(engine, json, bin, importConfig)
        assertEquals(listOf(1, 2, 1, 1, 2, 0), listOf(r.pairs, r.baselineRows, r.runs, r.conflicts, r.binImported, r.binFailed))
        assertEquals(setOf("a.txt", "ملف.txt"), engine.baseline("p1").map { it.key }.toSet())
        assertEquals("UPLOAD", engine.runEvents("r1").single().action)
        assertEquals("a.txt", engine.binEntry("t1")!!.entry.key)
        assertEquals("older", File(tmp.root, "bin/p1/${engine.binEntry("t1")!!.entry.location}").readText())
        assertEquals(BinSide.DRIVE, engine.binEntry("t2")!!.entry.side)
    }

    @Test fun `files that are not a Drive Sync export are refused`() {
        val work = tmp.newFolder()
        assertThrows(Exception::class.java) { unpackDriveSyncExport(zip(mapOf("x.txt" to ByteArray(1))).inputStream(), work) }
        assertThrows(Exception::class.java) {
            unpackDriveSyncExport(zip(mapOf("migration.json" to """{"format":"other","version":1}""".toByteArray())).inputStream(), work)
        }
        assertThrows(Exception::class.java) {
            unpackDriveSyncExport(zip(mapOf("migration.json" to """{"format":"drive-sync-to-wasl","version":99}""".toByteArray())).inputStream(), work)
        }
    }
}
