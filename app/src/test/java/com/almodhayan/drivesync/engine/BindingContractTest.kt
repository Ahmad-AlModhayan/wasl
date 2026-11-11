package com.almodhayan.drivesync.engine

import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.db.SyncMode
import com.kahflabs.wasl.ffi.EngineInit
import com.kahflabs.wasl.ffi.Platform
import com.kahflabs.wasl.ffi.ProgressSink
import com.kahflabs.wasl.ffi.SafBridge
import com.kahflabs.wasl.ffi.TokenSource
import com.kahflabs.wasl.ffi.VaultAccess
import com.kahflabs.wasl.ffi.WaslEngine
import com.kahflabs.wasl.ffi.WaslException
import com.kahflabs.wasl.ffi.core.DeviceState
import com.kahflabs.wasl.ffi.core.ErrorKind
import com.kahflabs.wasl.ffi.core.RunStatus
import com.kahflabs.wasl.ffi.engine.Progress
import com.kahflabs.wasl.ffi.engine.Trigger
import kotlinx.coroutines.runBlocking
import okhttp3.mockwebserver.Dispatcher
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import okhttp3.mockwebserver.RecordedRequest
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.security.MessageDigest
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicInteger

/**
 * Contract tests of the Kotlin binding (SRS FR-E4): the app's own calls, through JNA into the host
 * build of libwasl, against a mocked Drive. They pin what the Rust contract tests cannot: the
 * generated Kotlin types, callbacks from Rust into Kotlin, suspend functions, errors as
 * [WaslException], and the app's pair mapping.
 */
class BindingContractTest {
    @get:Rule val tmp = TemporaryFolder()

    private lateinit var server: MockWebServer
    private val uploads = CopyOnWriteArrayList<String>()

    private fun md5(b: ByteArray) = MessageDigest.getInstance("MD5").digest(b).joinToString("") { "%02x".format(it) }

    private fun json(body: String) = MockResponse().setHeader("Content-Type", "application/json; charset=utf-8").setBody(body)

    /** A Drive with folder `A` holding `remote.txt`; uploads answer with the MD5 of their content part. */
    private fun drive(remote: String) = object : Dispatcher() {
        override fun dispatch(request: RecordedRequest): MockResponse {
            val path = request.requestUrl!!.encodedPath
            val alt = request.requestUrl!!.queryParameter("alt")
            return when {
                request.method == "GET" && path == "/drive/v3/files/root" ->
                    json("""{"id":"ROOT","name":"My Drive","mimeType":"application/vnd.google-apps.folder"}""")
                // The pair's folder, which a first (live) listing checks is still there.
                request.method == "GET" && path == "/drive/v3/files/A" ->
                    json("""{"id":"A","name":"Sync","mimeType":"application/vnd.google-apps.folder","parents":["ROOT"]}""")
                request.method == "GET" && path == "/drive/v3/changes/startPageToken" -> json("""{"startPageToken":"7"}""")
                request.method == "GET" && path == "/drive/v3/changes" -> json("""{"newStartPageToken":"7","changes":[]}""")
                request.method == "GET" && path == "/drive/v3/files/B" && alt == "media" -> MockResponse().setBody(remote)
                request.method == "GET" && path == "/drive/v3/files" -> json(
                    """{"files":[
                        {"id":"A","name":"Sync","mimeType":"application/vnd.google-apps.folder","parents":["ROOT"]},
                        {"id":"B","name":"remote.txt","mimeType":"text/plain","parents":["A"],"size":"${remote.toByteArray().size}","md5Checksum":"${md5(remote.toByteArray())}","modifiedTime":"2026-01-02T03:04:05.000Z","version":"2"}
                    ]}""",
                )
                request.method == "POST" && path == "/upload/drive/v3/files" -> {
                    val body = request.body.readUtf8()
                    val content = body.split("\r\n\r\n").getOrNull(2)?.substringBefore("\r\n--") ?: ""
                    val name = Regex("\"name\"\\s*:\\s*\"([^\"]+)\"").find(body)?.groupValues?.get(1) ?: "file"
                    uploads += name
                    val bytes = content.toByteArray()
                    json("""{"id":"U${uploads.size}","name":"$name","mimeType":"text/plain","parents":["A"],"size":"${bytes.size}","md5Checksum":"${md5(bytes)}","version":"1"}""")
                }
                else -> MockResponse().setResponseCode(404).setBody("""{"error":{"code":404,"message":"$path"}}""")
            }
        }
    }

    private class Tokens : TokenSource {
        val asked = AtomicInteger()
        override fun accessToken(account: String): String { asked.incrementAndGet(); return "tok" }
        override fun invalidate(account: String) {}
    }

    private class Device : Platform {
        override fun deviceState() = DeviceState(
            online = true, wifi = true, metered = false, vpn = false, roaming = false, ssid = null, charging = true, batteryPercent = 100u,
        )
        override fun saf(): SafBridge? = null
        override fun vaultAccess(vaultId: String): VaultAccess? = null
    }

    private class Sink : ProgressSink {
        val seen = CopyOnWriteArrayList<Progress>()
        override fun onProgress(progress: Progress) { seen += progress }
    }

    private val engines = ArrayList<WaslEngine>()

    private fun engine(tokens: TokenSource = Tokens(), sink: ProgressSink? = null): WaslEngine {
        val dir = tmp.root
        return WaslEngine(
            EngineInit(
                dbPath = File(dir, "wasl.db").path,
                binDir = File(dir, "bin").path,
                stagingDir = File(dir, "staging").path,
                deviceName = "Pixel",
                driveBaseUrl = server.url("/drive/v3").toString().trimEnd('/'),
                driveUploadUrl = server.url("/upload/drive/v3").toString().trimEnd('/'),
            ),
            tokens,
            Device(),
            sink,
        ).also { engines += it }
    }

    private fun pairEntity(root: File, vaultId: String? = null) = FolderPairEntity(
        id = "p",
        accountId = "me@example.com",
        name = "Sync",
        localPath = root.path,
        remoteFolderId = "A",
        remotePath = "My Drive/Sync",
        encrypted = vaultId != null,
        vaultId = vaultId,
    )

    @Before fun start() {
        server = MockWebServer()
        server.dispatcher = drive("from drive")
        server.start()
    }

    @After fun stop() {
        engines.forEach { it.close() }
        server.shutdown()
    }

    @Test fun `a pair runs end to end through the Kotlin binding`() = runBlocking {
        val root = tmp.newFolder("pair")
        File(root, "ملف.txt").writeText("من الجهاز")
        val tokens = Tokens()
        val sink = Sink()
        val engine = engine(tokens, sink)
        assertTrue(engine.version().isNotEmpty())

        engine.createPair(pairEntity(root).toEngine(null))
        assertEquals(1, engine.pairs().size)

        // Preview, then the first-merge stop, then the start from the preview.
        val preview = engine.preview("p")
        assertEquals(1u, preview.plan.summary.uploads)
        assertEquals(1u, preview.plan.summary.downloads)
        // A new pair waits for its preview (ADR-0001): a manual run doesn't start it.
        assertEquals(RunStatus.SKIPPED, engine.sync("p", Trigger.MANUAL).status)
        // As if the preview had been seen: the first-merge stop is next.
        engine.setPairFlags("p", null, null, false)
        val stopped = engine.sync("p", Trigger.MANUAL)
        assertEquals(RunStatus.BLOCKED, stopped.status)
        assertNotNull("saved for the Needs-you screen", engine.blockedPlan("p"))

        val run = engine.sync("p", Trigger.PREVIEW)
        assertEquals(run.message, RunStatus.SUCCESS, run.status)
        assertEquals("from drive", File(root, "remote.txt").readText())
        assertEquals(listOf("ملف.txt"), uploads.toList())
        assertTrue("tokens come from Kotlin", tokens.asked.get() > 0)
        assertTrue("progress reaches Kotlin", sink.seen.isNotEmpty())
        assertNull(engine.blockedPlan("p"))

        // History, baseline and the app's view of the pair.
        val runs = engine.runs("p", 10u)
        assertEquals(listOf(RunStatus.SUCCESS, RunStatus.BLOCKED), runs.map { it.status })
        assertTrue(engine.runEvents(runs[0].id).any { it.action == "UPLOAD" })
        assertEquals(setOf("remote.txt", "ملف.txt"), engine.baseline("p").map { it.key }.toSet())
        val entity = engine.pair("p")!!.toEntity()
        assertEquals(com.almodhayan.drivesync.data.db.RunStatus.SUCCESS, entity.lastStatus)

        // An edit from the screens keeps the engine's status.
        engine.updatePair(entity.copy(mode = SyncMode.UPLOAD_ONLY, lastStatus = null).toEngine(engine.pair("p")))
        val edited = engine.pair("p")!!
        assertEquals(com.kahflabs.wasl.ffi.core.SyncMode.UPLOAD_ONLY, edited.method)
        assertEquals(RunStatus.SUCCESS, edited.status.lastStatus)

        engine.setSetting("device_name", "جهازي")
        assertEquals("جهازي", engine.setting("device_name"))
        engine.deletePair("p")
        assertTrue(engine.pairs().isEmpty())
    }

    @Test fun `a locked vault pair fails with the Locked kind`() = runBlocking {
        val engine = engine()
        engine.createPair(pairEntity(tmp.newFolder("pair"), vaultId = "v1").toEngine(null))
        // Started from its preview, as every new pair is.
        val r = engine.sync("p", Trigger.PREVIEW)
        assertEquals(RunStatus.FAILED, r.status)
        assertEquals(ErrorKind.LOCKED, r.errorKind)
        assertTrue(r.message!!.contains("Unlock"))
    }

    @Test fun `a token failure in Kotlin reaches the engine as an auth error`() = runBlocking {
        val failing = object : TokenSource {
            override fun accessToken(account: String): String = throw WaslException.Auth("Sign in again")
            override fun invalidate(account: String) {}
        }
        val engine = engine(failing)
        engine.createPair(pairEntity(tmp.newFolder("pair")).toEngine(null))
        val r = engine.sync("p", Trigger.PREVIEW)
        assertEquals(RunStatus.FAILED, r.status)
        assertEquals(ErrorKind.AUTH, r.errorKind)
        assertNotNull(engine.accountAuthError("me@example.com"))
        assertFalse(File(tmp.root, "pair/remote.txt").exists())
    }

    @Test fun `engine errors arrive as WaslException`() = runBlocking {
        val engine = engine()
        val e = assertThrows(WaslException::class.java) { runBlocking { engine.preview("missing") } }
        assertTrue(e.message!!.startsWith("detail="))
    }
}
