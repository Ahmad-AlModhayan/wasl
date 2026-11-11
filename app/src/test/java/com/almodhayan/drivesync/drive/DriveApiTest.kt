package com.almodhayan.drivesync.drive

import com.almodhayan.drivesync.core.AppError
import kotlinx.coroutines.test.currentTime
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.OkHttpClient
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import okhttp3.mockwebserver.RecordedRequest
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.io.RandomAccessFile
import java.time.Instant
import java.util.concurrent.TimeUnit

/** Token provider double: hands out "tok" until invalidated, then "tok2". */
internal class FakeTokens : TokenProvider {
    var token = "tok"
    val requested = ArrayList<String>()
    var invalidations = 0
    /** Errors the next token requests throw, in order (a flaky network, an unanswered Play services call). */
    val failNext = ArrayDeque<Throwable>()
    override suspend fun accessToken(email: String, forceRefresh: Boolean): String {
        requested += email
        failNext.removeFirstOrNull()?.let { throw it }
        return token
    }
    override fun invalidate(email: String) { invalidations++; token = "tok2" }
}

/** Same configuration as AppModule.json so wire-format assertions reflect production. */
internal val appJson: Json = Json { ignoreUnknownKeys = true; isLenient = true; encodeDefaults = true; explicitNulls = false }

internal fun testClient(): OkHttpClient = OkHttpClient.Builder()
    .connectTimeout(2, TimeUnit.SECONDS)
    .readTimeout(5, TimeUnit.SECONDS)
    .writeTimeout(5, TimeUnit.SECONDS)
    .build()

internal fun MockWebServer.endpoints() = DriveEndpoints(
    base = url("/drive/v3").toString().trimEnd('/'),
    upload = url("/upload/drive/v3").toString().trimEnd('/'),
)

internal fun jsonResponse(body: String, code: Int = 200): MockResponse =
    MockResponse().setResponseCode(code).setHeader("Content-Type", "application/json; charset=utf-8").setBody(body)

internal fun apiError(code: Int, reason: String, message: String = "error"): MockResponse =
    jsonResponse("""{"error":{"code":$code,"message":"$message","errors":[{"domain":"global","reason":"$reason","message":"$message"}]}}""", code)

internal fun fileJson(id: String, name: String = "n", mimeType: String = "text/plain", parent: String? = null, extra: String = ""): String {
    val parents = if (parent == null) "" else ""","parents":["$parent"]"""
    return """{"id":"$id","name":"$name","mimeType":"$mimeType"$parents$extra}"""
}

internal fun MockWebServer.take(): RecordedRequest = takeRequest(2, TimeUnit.SECONDS) ?: throw AssertionError("no request was recorded")

internal inline fun <reified T : Throwable> assertFails(block: () -> Unit): T {
    try {
        block()
    } catch (t: Throwable) {
        if (t is T) return t
        throw AssertionError("expected ${T::class.java.simpleName} but got ${t::class.java.simpleName}: ${t.message}", t)
    }
    throw AssertionError("expected ${T::class.java.simpleName} but nothing was thrown")
}

class DriveApiTest {

    @get:Rule
    val tmp = TemporaryFolder()

    private val server = MockWebServer()
    private val tokens = FakeTokens()
    private lateinit var api: DriveApi
    private val account = "me@example.com"

    private class FakeSessionStore(initial: Map<String, String> = emptyMap()) : DriveApi.UploadSessionStore {
        val map = HashMap(initial)
        val ops = ArrayList<String>()
        override suspend fun get(key: String): String? { ops += "get:$key"; return map[key] }
        override suspend fun put(key: String, uri: String) { ops += "put:$key"; map[key] = uri }
        override suspend fun remove(key: String) { ops += "remove:$key"; map.remove(key) }
    }

    @Before
    fun setUp() {
        server.start()
        api = DriveApi(testClient(), tokens, appJson, server.endpoints())
    }

    @After
    fun tearDown() {
        server.shutdown()
    }

    /** Sparse file of [size] bytes with recognisable marker bytes at the given offsets. */
    private fun bigFile(name: String, size: Long, vararg markers: Pair<Long, Char>): File {
        val f = tmp.newFile(name)
        RandomAccessFile(f, "rw").use { raf ->
            raf.setLength(size)
            markers.forEach { (offset, ch) -> raf.seek(offset); raf.write(ch.code) }
        }
        return f
    }

    private fun sessionResponse(path: String): MockResponse =
        MockResponse().setResponseCode(200).setHeader("Location", server.url(path).toString())

    // ================================================================ plumbing

    @Test
    fun getFile_sendsBearerTokenAndFieldSelection() = runTest {
        server.enqueue(jsonResponse(fileJson("abc", "hello.txt", parent = "root", extra = ""","size":"12","md5Checksum":"aa","modifiedTime":"2026-01-02T03:04:05.000Z","version":"7","trashed":false""")))
        val f = api.getFile(account, "abc")
        assertEquals("abc", f.id)
        assertEquals("hello.txt", f.name)
        assertEquals(12L, f.sizeLong)
        assertEquals(7L, f.versionLong)
        assertEquals("aa", f.md5Checksum)
        assertEquals(listOf("root"), f.parents)
        assertFalse(f.isFolder)
        assertFalse(f.isShortcut)
        assertFalse(f.isGoogleDoc)

        val req = server.take()
        assertEquals("GET", req.method)
        assertEquals("Bearer tok", req.getHeader("Authorization"))
        val u = req.requestUrl!!
        assertEquals("/drive/v3/files/abc", u.encodedPath)
        assertEquals(FILE_FIELDS, u.queryParameter("fields"))
        assertEquals("true", u.queryParameter("supportsAllDrives"))
        assertEquals(listOf(account), tokens.requested)
    }

    @Test
    fun rootId_resolvesTheRootAlias() = runTest {
        server.enqueue(jsonResponse(fileJson("0AB", "My Drive", FOLDER_MIME)))
        assertEquals("0AB", api.rootId(account))
        assertEquals("/drive/v3/files/root", server.take().requestUrl!!.encodedPath)
    }

    @Test
    fun about_parsesUserAndQuota() = runTest {
        server.enqueue(jsonResponse("""{"user":{"emailAddress":"a@b.c","displayName":"A","photoLink":"http://p"},"storageQuota":{"limit":"100","usage":"5","usageInDrive":"4","usageInDriveTrash":"1"}}"""))
        val about = api.about(account)
        assertEquals("a@b.c", about.user!!.emailAddress)
        assertEquals("A", about.user!!.displayName)
        assertEquals("100", about.storageQuota!!.limit)
        assertEquals("5", about.storageQuota!!.usage)
        val u = server.take().requestUrl!!
        assertEquals("/drive/v3/about", u.encodedPath)
        assertTrue(u.queryParameter("fields")!!.contains("storageQuota"))
    }

    // ================================================================ listing

    @Test
    fun listChildren_paginatesAndSendsQueryParameters() = runTest {
        server.enqueue(jsonResponse("""{"nextPageToken":"t2","files":[${fileJson("1", "a")}]}"""))
        server.enqueue(jsonResponse("""{"files":[${fileJson("2", "b")}]}"""))

        val files = api.listChildren(account, "parent1")
        assertEquals(listOf("1", "2"), files.map { it.id })

        val r1 = server.take().requestUrl!!
        assertEquals("/drive/v3/files", r1.encodedPath)
        assertEquals("'parent1' in parents and trashed = false", r1.queryParameter("q"))
        assertEquals("1000", r1.queryParameter("pageSize"))
        assertEquals("nextPageToken,incompleteSearch,files($FILE_FIELDS)", r1.queryParameter("fields"))
        assertEquals("drive", r1.queryParameter("spaces"))
        assertEquals("true", r1.queryParameter("supportsAllDrives"))
        assertEquals("false", r1.queryParameter("includeItemsFromAllDrives"))
        assertEquals("user", r1.queryParameter("corpora"))
        assertNull(r1.queryParameter("driveId"))
        assertNull(r1.queryParameter("pageToken"))
        assertNull(r1.queryParameter("orderBy"))

        val r2 = server.take().requestUrl!!
        assertEquals("t2", r2.queryParameter("pageToken"))
        assertEquals("'parent1' in parents and trashed = false", r2.queryParameter("q"))
    }

    @Test
    fun listPage_sharedDrive_setsCorporaAndDriveId() = runTest {
        server.enqueue(jsonResponse("""{"files":[],"incompleteSearch":true}"""))
        val page = api.listPage(account, q = null, pageToken = "p9", driveId = "D1", pageSize = 50, orderBy = "name")
        assertTrue(page.files.isEmpty())
        assertNull(page.nextPageToken)
        assertTrue(page.incompleteSearch)
        val u = server.take().requestUrl!!
        assertEquals("drive", u.queryParameter("corpora"))
        assertEquals("D1", u.queryParameter("driveId"))
        assertEquals("true", u.queryParameter("includeItemsFromAllDrives"))
        assertEquals("true", u.queryParameter("supportsAllDrives"))
        assertEquals("50", u.queryParameter("pageSize"))
        assertEquals("name", u.queryParameter("orderBy"))
        assertEquals("p9", u.queryParameter("pageToken"))
        assertNull(u.queryParameter("q"))
    }

    @Test
    fun listAppData_usesTheAppDataFolderSpace() = runTest {
        server.enqueue(jsonResponse("""{"files":[${fileJson("cfg", "config.json")}]}"""))
        val files = api.listAppData(account)
        assertEquals(listOf("cfg"), files.map { it.id })
        val u = server.take().requestUrl!!
        assertEquals("appDataFolder", u.queryParameter("spaces"))
        assertNull(u.queryParameter("q"))
        assertEquals("user", u.queryParameter("corpora"))
    }

    @Test
    fun listSharedDrives_paginates() = runTest {
        server.enqueue(jsonResponse("""{"nextPageToken":"x","drives":[{"id":"D1","name":"Team"}]}"""))
        server.enqueue(jsonResponse("""{"drives":[{"id":"D2","name":"Other"}]}"""))
        val drives = api.listSharedDrives(account)
        assertEquals(listOf("D1" to "Team", "D2" to "Other"), drives.map { it.id to it.name })
        val r1 = server.take().requestUrl!!
        assertEquals("/drive/v3/drives", r1.encodedPath)
        assertEquals("100", r1.queryParameter("pageSize"))
        assertNull(r1.queryParameter("pageToken"))
        assertEquals("x", server.take().requestUrl!!.queryParameter("pageToken"))
    }

    // ================================================================ changes

    @Test
    fun startPageToken_parsesAndSupportsSharedDrives() = runTest {
        server.enqueue(jsonResponse("""{"startPageToken":"123"}"""))
        assertEquals("123", api.startPageToken(account))
        val u = server.take().requestUrl!!
        assertEquals("/drive/v3/changes/startPageToken", u.encodedPath)
        assertEquals("true", u.queryParameter("supportsAllDrives"))
        assertNull(u.queryParameter("driveId"))

        server.enqueue(jsonResponse("""{"startPageToken":"7"}"""))
        assertEquals("7", api.startPageToken(account, driveId = "D1"))
        assertEquals("D1", server.take().requestUrl!!.queryParameter("driveId"))
    }

    @Test
    fun changesPage_parsesRemovedItemsAndNewStartPageToken() = runTest {
        server.enqueue(jsonResponse("""{"newStartPageToken":"99","changes":[
            {"changeType":"file","fileId":"f1","removed":true,"time":"2026-01-01T00:00:00.000Z"},
            {"changeType":"file","fileId":"f2","removed":false,"file":${fileJson("f2", "n", parent = "root", extra = ""","size":"12","md5Checksum":"abc","modifiedTime":"2026-01-02T03:04:05.000Z","version":"7","trashed":false""")}},
            {"changeType":"drive","driveId":"D","time":"2026-01-01T00:00:00.000Z"}
        ]}"""))
        val page = api.changesPage(account, "5")
        assertEquals("99", page.newStartPageToken)
        assertNull(page.nextPageToken)
        assertEquals(3, page.changes.size)
        val (c1, c2, c3) = page.changes
        assertTrue(c1.removed)
        assertEquals("f1", c1.fileId)
        assertNull(c1.file)
        assertFalse(c2.removed)
        assertEquals("f2", c2.file!!.id)
        assertEquals("abc", c2.file!!.md5Checksum)
        assertEquals(12L, c2.file!!.sizeLong)
        assertEquals("drive", c3.changeType)
        assertEquals("D", c3.driveId)
        assertNull(c3.fileId)

        val u = server.take().requestUrl!!
        assertEquals("/drive/v3/changes", u.encodedPath)
        assertEquals("5", u.queryParameter("pageToken"))
        assertEquals("true", u.queryParameter("includeRemoved"))
        assertEquals("1000", u.queryParameter("pageSize"))
        assertEquals("true", u.queryParameter("supportsAllDrives"))
        assertEquals("true", u.queryParameter("includeItemsFromAllDrives"))
        assertNull(u.queryParameter("driveId"))
        assertTrue(u.queryParameter("fields")!!.contains("newStartPageToken"))
        assertTrue(u.queryParameter("fields")!!.contains("file($FILE_FIELDS)"))
    }

    @Test
    fun changesPage_withNextPageToken_isReturnedForTheCallerToContinue() = runTest {
        server.enqueue(jsonResponse("""{"nextPageToken":"6","changes":[]}"""))
        val page = api.changesPage(account, "5", driveId = "D1")
        assertEquals("6", page.nextPageToken)
        assertNull(page.newStartPageToken)
        assertEquals("D1", server.take().requestUrl!!.queryParameter("driveId"))
    }

    // ================================================================ error mapping and retries

    @Test
    fun unauthorized_invalidatesTheTokenAndRetriesOnceWithoutBackoff() = runTest {
        server.enqueue(apiError(401, "authError", "Invalid Credentials"))
        server.enqueue(jsonResponse(fileJson("abc")))
        assertEquals("abc", api.getFile(account, "abc").id)
        assertEquals(1, tokens.invalidations)
        assertEquals(2, server.requestCount)
        assertEquals("Bearer tok", server.take().getHeader("Authorization"))
        assertEquals("Bearer tok2", server.take().getHeader("Authorization"))
        assertEquals("the auth retry is immediate", 0L, currentTime)
    }

    @Test
    fun unauthorizedTwice_failsWithAuthAfterASingleRefresh() = runTest {
        server.enqueue(apiError(401, "authError", "Invalid Credentials"))
        server.enqueue(apiError(401, "authError", "Invalid Credentials"))
        val e = assertFails<AppError.Auth> { api.getFile(account, "abc") }
        assertTrue(e.message!!.contains("Invalid Credentials"))
        assertEquals(1, tokens.invalidations)
        assertEquals(2, server.requestCount)
    }

    @Test
    fun aNetworkErrorWhileGettingTheToken_isRetriedLikeAnyNetworkError() = runTest {
        tokens.failNext += AppError.Network("Network error while contacting Google.")
        server.enqueue(jsonResponse(fileJson("abc")))

        assertEquals("abc", api.getFile(account, "abc").id)
        assertEquals(1, server.requestCount)
    }

    @Test
    fun rateLimited403_isRetriedAfterExponentialBackoff() = runTest {
        server.enqueue(apiError(403, "userRateLimitExceeded", "Rate Limit Exceeded"))
        server.enqueue(jsonResponse(fileJson("abc")))
        assertEquals("abc", api.getFile(account, "abc").id)
        assertEquals(2, server.requestCount)
        assertTrue("first backoff is 1s plus up to 1s jitter, got $currentTime", currentTime in 1000L..1999L)
        assertEquals(0, tokens.invalidations)
    }

    @Test
    fun tooManyRequests429_waitsExactlyRetryAfter() = runTest {
        server.enqueue(apiError(429, "rateLimitExceeded", "Too many").setHeader("Retry-After", "5"))
        server.enqueue(jsonResponse(fileJson("abc")))
        assertEquals("abc", api.getFile(account, "abc").id)
        assertEquals(2, server.requestCount)
        assertEquals(5000L, currentTime)
    }

    @Test
    fun storageQuota403_isNotRetried() = runTest {
        server.enqueue(apiError(403, "storageQuotaExceeded", "The user's Drive storage quota has been exceeded."))
        val e = assertFails<AppError.StorageQuota> { api.getFile(account, "abc") }
        assertTrue(e.message!!.contains("storage quota"))
        assertEquals(1, server.requestCount)
        assertEquals(0L, currentTime)
    }

    @Test
    fun notFound404_isNotRetried() = runTest {
        server.enqueue(apiError(404, "notFound", "File not found: abc."))
        val e = assertFails<AppError.NotFound> { api.getFile(account, "abc") }
        assertEquals("File not found: abc.", e.message)
        assertEquals(1, server.requestCount)
    }

    @Test
    fun serverError500_isRetriedThenSucceeds() = runTest {
        server.enqueue(apiError(500, "backendError", "Backend Error"))
        server.enqueue(jsonResponse(fileJson("abc")))
        assertEquals("abc", api.getFile(account, "abc").id)
        assertEquals(2, server.requestCount)
        assertTrue(currentTime in 1000L..1999L)
    }

    @Test
    fun nonJsonServerError_isStillTransient() = runTest {
        server.enqueue(MockResponse().setResponseCode(502).setBody("<html>Bad Gateway</html>"))
        server.enqueue(jsonResponse(fileJson("abc")))
        assertEquals("abc", api.getFile(account, "abc").id)
        assertEquals(2, server.requestCount)
    }

    @Test
    fun transientErrors_giveUpAfterSixAttempts() = runTest {
        repeat(6) { server.enqueue(apiError(503, "backendError", "Backend Error")) }
        val e = assertFails<AppError.Http> { api.getFile(account, "abc") }
        assertEquals(503, e.code)
        assertEquals(6, server.requestCount)
        // 1+2+4+8+16 seconds of backoff (each with < 1s jitter), all in virtual time
        assertTrue("backoff total was $currentTime", currentTime in 31_000L..35_999L)
    }

    @Test
    fun forbidden_withUnknownReason_isNotRetried() = runTest {
        server.enqueue(apiError(403, "domainPolicy", "Blocked"))
        val e = assertFails<AppError.Http> { api.getFile(account, "abc") }
        assertEquals(403, e.code)
        assertEquals("domainPolicy", e.reason)
        assertEquals(1, server.requestCount)
    }

    // ================================================================ downloads

    @Test
    fun download_writesTheFileAndReportsProgress() = runTest {
        server.enqueue(MockResponse().setBody("hello world"))
        val dest = File(tmp.root, "a.part")
        val progress = ArrayList<Pair<Long, Long?>>()
        api.download(account, "f1", dest, 11) { done, total -> progress += done to total }
        assertEquals("hello world", dest.readText())
        val req = server.take()
        val u = req.requestUrl!!
        assertEquals("/drive/v3/files/f1", u.encodedPath)
        assertEquals("media", u.queryParameter("alt"))
        assertEquals("true", u.queryParameter("supportsAllDrives"))
        assertNull(req.getHeader("Range"))
        assertEquals("Bearer tok", req.getHeader("Authorization"))
        assertEquals(11L to 11L, progress.last())
    }

    @Test
    fun download_withoutExpectedSize_usesContentLengthForProgress() = runTest {
        server.enqueue(MockResponse().setBody("hello world"))
        val dest = File(tmp.root, "a.part")
        val progress = ArrayList<Pair<Long, Long?>>()
        api.download(account, "f1", dest, null) { done, total -> progress += done to total }
        assertEquals(11L to 11L, progress.last())
    }

    @Test
    fun download_sizeMismatch_throwsIntegrityWithoutRetrying() = runTest {
        server.enqueue(MockResponse().setBody("hello world"))
        val dest = File(tmp.root, "a.part")
        val e = assertFails<AppError.Integrity> { api.download(account, "f1", dest, 20) }
        assertTrue(e.message!!.contains("11 != 20"))
        assertEquals(1, server.requestCount)
        assertEquals("partial data is kept for a later resume", 11L, dest.length())
    }

    @Test
    fun download_resumesFromAnExistingPartFile() = runTest {
        val dest = File(tmp.root, "a.part").apply { writeText("hello ") }
        server.enqueue(MockResponse().setResponseCode(206).setHeader("Content-Range", "bytes 6-10/11").setBody("world"))
        val progress = ArrayList<Pair<Long, Long?>>()
        api.download(account, "f1", dest, 11) { done, total -> progress += done to total }
        assertEquals("bytes=6-", server.take().getHeader("Range"))
        assertEquals("hello world", dest.readText())
        assertEquals(11L to 11L, progress.last())
    }

    @Test
    fun download_resumeWithoutExpectedSize_addsExistingBytesToTheTotal() = runTest {
        val dest = File(tmp.root, "a.part").apply { writeText("hello ") }
        server.enqueue(MockResponse().setResponseCode(206).setBody("world"))
        val progress = ArrayList<Pair<Long, Long?>>()
        api.download(account, "f1", dest, null) { done, total -> progress += done to total }
        assertEquals("hello world", dest.readText())
        assertEquals(11L to 11L, progress.last())
    }

    @Test
    fun download_whenThePartFileIsAlreadyComplete_makesNoRequest() = runTest {
        val dest = File(tmp.root, "a.part").apply { writeText("hello world") }
        api.download(account, "f1", dest, 11)
        assertEquals(0, server.requestCount)
        assertEquals("hello world", dest.readText())
    }

    @Test
    fun download_whenTheServerIgnoresTheRange_restartsFromScratch() = runTest {
        val dest = File(tmp.root, "a.part").apply { writeText("hello ") }
        server.enqueue(MockResponse().setResponseCode(200).setBody("hello world"))
        api.download(account, "f1", dest, 11)
        assertEquals("bytes=6-", server.take().getHeader("Range"))
        assertEquals("hello world", dest.readText())
    }

    @Test
    fun download_rangeNotSatisfiable_isTreatedAsComplete() = runTest {
        val dest = File(tmp.root, "a.part").apply { writeText("hello world") }
        server.enqueue(MockResponse().setResponseCode(416))
        api.download(account, "f1", dest, null)
        assertEquals("bytes=11-", server.take().getHeader("Range"))
        assertEquals("hello world", dest.readText())
    }

    @Test
    fun download_aPartFileLargerThanTheFile_isDroppedAndTheDownloadStartsOver() = runTest {
        // Left by an interrupted download of an older, bigger version of the file.
        val dest = File(tmp.root, "a.part").apply { writeText("an older, longer version") }
        server.enqueue(MockResponse().setBody("hello world"))

        api.download(account, "f1", dest, 11)

        assertNull("a stale part file must not be resumed", server.take().getHeader("Range"))
        assertEquals("hello world", dest.readText())
    }

    @Test
    fun download_rangeNotSatisfiableButTheSizeIsWrong_dropsThePartFile() = runTest {
        // Otherwise every later run resumes the same stale bytes and fails the same way.
        val dest = File(tmp.root, "a.part").apply { writeText("hello") }
        server.enqueue(MockResponse().setResponseCode(416))

        assertFails<AppError.Integrity> { api.download(account, "f1", dest, 11) }

        assertFalse("the stale part file is kept and fails every run", dest.exists())
    }

    @Test
    fun download_serverError_isRetriedAndResumesFromWhatWasWritten() = runTest {
        val dest = File(tmp.root, "a.part")
        server.enqueue(apiError(503, "backendError"))
        server.enqueue(MockResponse().setBody("hello world"))
        api.download(account, "f1", dest, 11)
        assertEquals("hello world", dest.readText())
        assertEquals(2, server.requestCount)
    }

    @Test
    fun downloadRevision_targetsTheRevisionMedia() = runTest {
        server.enqueue(MockResponse().setBody("v1"))
        val dest = File(tmp.root, "rev.part")
        api.downloadRevision(account, "f1", "r1", dest)
        assertEquals("v1", dest.readText())
        val u = server.take().requestUrl!!
        assertEquals("/drive/v3/files/f1/revisions/r1", u.encodedPath)
        assertEquals("media", u.queryParameter("alt"))
    }

    @Test
    fun export_deletesTheStaleDestinationAndNeverResumes() = runTest {
        val dest = File(tmp.root, "doc.part").apply { writeText("old") }
        server.enqueue(MockResponse().setBody("new-doc"))
        api.export(account, "f1", "application/pdf", dest)
        val req = server.take()
        assertEquals("/drive/v3/files/f1/export", req.requestUrl!!.encodedPath)
        assertEquals("application/pdf", req.requestUrl!!.queryParameter("mimeType"))
        assertNull(req.getHeader("Range"))
        assertEquals("new-doc", dest.readText())
    }

    @Test
    fun readRange_sendsTheRangeHeaderAndReturnsTheBytesRead() = runTest {
        server.enqueue(MockResponse().setResponseCode(206).setBody("cdefg"))
        val buf = ByteArray(10)
        val n = api.readRange(account, "f1", 2, 5, buf)
        assertEquals(5, n)
        assertEquals("cdefg", String(buf, 0, n))
        val req = server.take()
        assertEquals("bytes=2-6", req.getHeader("Range"))
        assertEquals("media", req.requestUrl!!.queryParameter("alt"))
    }

    @Test
    fun readRange_beyondTheEnd_returnsZero() = runTest {
        server.enqueue(MockResponse().setResponseCode(416))
        assertEquals(0, api.readRange(account, "f1", 100, 5, ByteArray(5)))
    }

    // ================================================================ uploads: multipart

    @Test
    fun uploadSmallFile_usesASingleMultipartRelatedRequest() = runTest {
        val src = tmp.newFile("photo.jpg").apply { writeText("0123456789") }
        server.enqueue(jsonResponse(fileJson("new1", "photo.jpg", "image/jpeg", extra = ""","md5Checksum":"x"""")))
        val mtime = Instant.parse("2025-01-02T03:04:05Z").toEpochMilli()
        val progress = ArrayList<Pair<Long, Long>>()

        val result = api.upload(account, src, "photo.jpg", "parent1", null, mtime, mimeType = "image/jpeg") { done, total -> progress += done to total }
        assertEquals("new1", result.file.id)

        val req = server.take()
        assertEquals("POST", req.method)
        val u = req.requestUrl!!
        assertEquals("/upload/drive/v3/files", u.encodedPath)
        assertEquals("multipart", u.queryParameter("uploadType"))
        assertEquals("true", u.queryParameter("supportsAllDrives"))
        assertEquals(FILE_FIELDS, u.queryParameter("fields"))
        assertEquals("Bearer tok", req.getHeader("Authorization"))
        assertTrue(req.getHeader("Content-Type")!!.startsWith("multipart/related; boundary="))

        val body = req.body.readUtf8()
        assertTrue(body.contains("Content-Type: application/json; charset=utf-8"))
        assertTrue(body.contains("Content-Type: image/jpeg"))
        assertTrue("metadata part comes first", body.indexOf("application/json") < body.indexOf("image/jpeg"))
        assertTrue(body.contains("\r\n\r\n0123456789\r\n"))
        val meta = appJson.parseToJsonElement(body.lines().first { it.startsWith("{") }) as JsonObject
        assertEquals("photo.jpg", meta["name"]!!.jsonPrimitive.content)
        assertEquals("parent1", meta["parents"]!!.jsonArray.single().jsonPrimitive.content)
        assertEquals("2025-01-02T03:04:05Z", meta["modifiedTime"]!!.jsonPrimitive.content)
        assertNull("the MIME type travels in the media part, not the metadata", meta["mimeType"])
        assertEquals(10L to 10L, progress.last())
    }

    @Test
    fun uploadSmallFile_toAnExistingFile_patchesWithoutParents() = runTest {
        val src = tmp.newFile("photo.jpg").apply { writeText("0123456789") }
        server.enqueue(jsonResponse(fileJson("e1", "photo.jpg")))
        api.upload(account, src, "photo.jpg", null, "e1", null)
        val req = server.take()
        assertEquals("PATCH", req.method)
        assertEquals("/upload/drive/v3/files/e1", req.requestUrl!!.encodedPath)
        assertEquals("multipart", req.requestUrl!!.queryParameter("uploadType"))
        val meta = appJson.parseToJsonElement(req.body.readUtf8().lines().first { it.startsWith("{") }) as JsonObject
        assertEquals("photo.jpg", meta["name"]!!.jsonPrimitive.content)
        assertNull(meta["parents"])
        assertNull(meta["modifiedTime"])
    }

    @Test
    fun uploadSmallFile_serverError_isRetriedWithTheFullBody() = runTest {
        val src = tmp.newFile("photo.jpg").apply { writeText("0123456789") }
        server.enqueue(apiError(500, "backendError"))
        server.enqueue(jsonResponse(fileJson("new1")))
        assertEquals("new1", api.upload(account, src, "photo.jpg", "parent1", null, null).file.id)
        assertTrue(server.take().body.readUtf8().contains("0123456789"))
        assertTrue(server.take().body.readUtf8().contains("0123456789"))
    }

    // ================================================================ uploads: resumable

    @Test
    fun uploadLargeFile_startsASessionAndSendsChunksWithContentRange() = runTest {
        val size = DriveApi.CHUNK.toLong() + 1000
        val src = bigFile("big.bin", size, 0L to 'A', DriveApi.CHUNK.toLong() to 'B', (size - 1) to 'Z')
        val store = FakeSessionStore()
        val key = "pair|big.bin|md5"
        server.enqueue(sessionResponse("/upload/session/s1"))
        server.enqueue(MockResponse().setResponseCode(308).setHeader("Range", "bytes=0-${DriveApi.CHUNK - 1}"))
        server.enqueue(jsonResponse(fileJson("big1", "big.bin")))
        val progress = ArrayList<Pair<Long, Long>>()

        val result = api.upload(account, src, "big.bin", "parent1", null, null, sessionStore = store, sessionKey = key) { done, total -> progress += done to total }
        assertEquals("big1", result.file.id)

        val start = server.take()
        assertEquals("POST", start.method)
        assertEquals("/upload/drive/v3/files", start.requestUrl!!.encodedPath)
        assertEquals("resumable", start.requestUrl!!.queryParameter("uploadType"))
        assertEquals("true", start.requestUrl!!.queryParameter("supportsAllDrives"))
        assertEquals(size.toString(), start.getHeader("X-Upload-Content-Length"))
        assertEquals("application/octet-stream", start.getHeader("X-Upload-Content-Type"))
        assertEquals("Bearer tok", start.getHeader("Authorization"))
        assertTrue(start.getHeader("Content-Type")!!.startsWith("application/json"))
        val meta = appJson.parseToJsonElement(start.body.readUtf8()) as JsonObject
        assertEquals("big.bin", meta["name"]!!.jsonPrimitive.content)
        assertEquals("parent1", meta["parents"]!!.jsonArray.single().jsonPrimitive.content)
        assertNull(meta["modifiedTime"])

        val c1 = server.take()
        assertEquals("PUT", c1.method)
        assertEquals("/upload/session/s1", c1.requestUrl!!.encodedPath)
        assertEquals("bytes 0-${DriveApi.CHUNK - 1}/$size", c1.getHeader("Content-Range"))
        assertEquals("Bearer tok", c1.getHeader("Authorization"))
        assertEquals(DriveApi.CHUNK.toLong(), c1.bodySize)
        assertEquals('A'.code.toByte(), c1.body.readByte())

        val c2 = server.take()
        assertEquals("PUT", c2.method)
        assertEquals("bytes ${DriveApi.CHUNK}-${size - 1}/$size", c2.getHeader("Content-Range"))
        assertEquals(1000L, c2.bodySize)
        val tail = c2.body.readByteArray()
        assertEquals('B'.code.toByte(), tail[0])
        assertEquals('Z'.code.toByte(), tail[999])

        assertEquals(listOf("get:$key", "put:$key", "remove:$key"), store.ops)
        assertTrue(store.map.isEmpty())
        assertEquals(listOf(DriveApi.CHUNK.toLong() to size, size to size), progress)
    }

    @Test
    fun uploadLargeFile_persistsTheSessionUriBeforeSendingData() = runTest {
        val size = DriveApi.MULTIPART_LIMIT + 1
        val src = bigFile("big.bin", size)
        val store = FakeSessionStore()
        server.enqueue(sessionResponse("/upload/session/s1"))
        server.enqueue(apiError(404, "notFound", "Not Found"))   // chunk PUT: session vanished
        val e = assertFails<AppError.NotFound> { api.upload(account, src, "big.bin", "parent1", null, null, sessionStore = store, sessionKey = "k") }
        assertEquals("upload session expired", e.message)
        assertEquals(server.url("/upload/session/s1").toString(), store.map["k"])
        assertEquals(listOf("get:k", "put:k"), store.ops)
    }

    @Test
    fun uploadLargeFile_whenTheServerAcknowledgesLess_continuesFromTheAcknowledgedOffset() = runTest {
        val size = DriveApi.MULTIPART_LIMIT + 1     // a single chunk normally
        val ack = 1_048_576L
        val src = bigFile("big.bin", size, ack to 'M', (size - 1) to 'Z')
        server.enqueue(sessionResponse("/upload/session/s1"))
        server.enqueue(MockResponse().setResponseCode(308).setHeader("Range", "bytes=0-${ack - 1}"))
        server.enqueue(jsonResponse(fileJson("big1")))

        assertEquals("big1", api.upload(account, src, "big.bin", "parent1", null, null).file.id)

        server.take()   // session start
        val c1 = server.take()
        assertEquals("bytes 0-${size - 1}/$size", c1.getHeader("Content-Range"))
        assertEquals(size, c1.bodySize)
        val c2 = server.take()
        assertEquals("bytes $ack-${size - 1}/$size", c2.getHeader("Content-Range"))
        assertEquals(size - ack, c2.bodySize)
        assertEquals('M'.code.toByte(), c2.body.readByte())
    }

    @Test
    fun uploadLargeFile_resumesAnExistingSessionFromTheServerReportedOffset() = runTest {
        val size = DriveApi.MULTIPART_LIMIT + 1
        val offset = 2_097_152L
        val src = bigFile("big.bin", size, offset to 'M')
        val store = FakeSessionStore(mapOf("k" to server.url("/upload/session/old").toString()))
        server.enqueue(MockResponse().setResponseCode(308).setHeader("Range", "bytes=0-${offset - 1}"))
        server.enqueue(jsonResponse(fileJson("big1")))

        assertEquals("big1", api.upload(account, src, "big.bin", "parent1", null, null, sessionStore = store, sessionKey = "k").file.id)

        val query = server.take()
        assertEquals("PUT", query.method)
        assertEquals("/upload/session/old", query.requestUrl!!.encodedPath)
        assertEquals("bytes */$size", query.getHeader("Content-Range"))
        assertEquals(0L, query.bodySize)
        assertEquals("Bearer tok", query.getHeader("Authorization"))

        val chunk = server.take()
        assertEquals("/upload/session/old", chunk.requestUrl!!.encodedPath)
        assertEquals("bytes $offset-${size - 1}/$size", chunk.getHeader("Content-Range"))
        assertEquals(size - offset, chunk.bodySize)
        assertEquals('M'.code.toByte(), chunk.body.readByte())

        assertEquals("no new session was started", 2, server.requestCount)
        assertEquals(listOf("get:k", "remove:k"), store.ops)
        assertTrue(store.map.isEmpty())
    }

    @Test
    fun uploadLargeFile_expiredSession_isForgottenAndANewOneStarted() = runTest {
        val size = DriveApi.MULTIPART_LIMIT + 1
        val src = bigFile("big.bin", size)
        val store = FakeSessionStore(mapOf("k" to server.url("/upload/session/old").toString()))
        server.enqueue(MockResponse().setResponseCode(404))               // status query: gone
        server.enqueue(sessionResponse("/upload/session/new"))
        server.enqueue(jsonResponse(fileJson("big1")))

        assertEquals("big1", api.upload(account, src, "big.bin", "parent1", null, null, sessionStore = store, sessionKey = "k").file.id)

        assertEquals("/upload/session/old", server.take().requestUrl!!.encodedPath)
        val start = server.take()
        assertEquals("POST", start.method)
        assertEquals("/upload/drive/v3/files", start.requestUrl!!.encodedPath)
        val chunk = server.take()
        assertEquals("/upload/session/new", chunk.requestUrl!!.encodedPath)
        assertEquals("bytes 0-${size - 1}/$size", chunk.getHeader("Content-Range"))
        assertEquals(listOf("get:k", "remove:k", "put:k", "remove:k"), store.ops)
    }

    @Test
    fun uploadLargeFile_existingSessionAlreadyComplete_returnsWithoutSendingData() = runTest {
        val size = DriveApi.MULTIPART_LIMIT + 1
        val src = bigFile("big.bin", size)
        val store = FakeSessionStore(mapOf("k" to server.url("/upload/session/old").toString()))
        server.enqueue(jsonResponse(fileJson("done1")))
        assertEquals("done1", api.upload(account, src, "big.bin", "parent1", null, null, sessionStore = store, sessionKey = "k").file.id)
        assertEquals(1, server.requestCount)
        assertEquals("bytes */$size", server.take().getHeader("Content-Range"))
        assertEquals(listOf("get:k", "remove:k"), store.ops)
    }

    @Test
    fun uploadLargeFile_toAnExistingFile_startsThePatchSession() = runTest {
        val size = DriveApi.MULTIPART_LIMIT + 1
        val src = bigFile("big.bin", size)
        val mtime = Instant.parse("2025-01-02T03:04:05Z").toEpochMilli()
        server.enqueue(sessionResponse("/upload/session/s1"))
        server.enqueue(jsonResponse(fileJson("e1")))
        api.upload(account, src, "big.bin", null, "e1", mtime)
        val start = server.take()
        assertEquals("PATCH", start.method)
        assertEquals("/upload/drive/v3/files/e1", start.requestUrl!!.encodedPath)
        assertEquals("resumable", start.requestUrl!!.queryParameter("uploadType"))
        val meta = appJson.parseToJsonElement(start.body.readUtf8()) as JsonObject
        assertEquals("big.bin", meta["name"]!!.jsonPrimitive.content)
        assertEquals("2025-01-02T03:04:05Z", meta["modifiedTime"]!!.jsonPrimitive.content)
        assertNull(meta["parents"])
        assertEquals("bytes 0-${size - 1}/$size", server.take().getHeader("Content-Range"))
    }

    @Test
    fun uploadLargeFile_sessionStartWithoutLocation_fails() = runTest {
        val src = bigFile("big.bin", DriveApi.MULTIPART_LIMIT + 1)
        server.enqueue(MockResponse().setResponseCode(200))
        val e = assertFails<AppError.Http> { api.upload(account, src, "big.bin", "parent1", null, null) }
        assertTrue(e.message!!.contains("Location"))
    }

    @Test
    fun uploadLargeFile_transientChunkFailure_isRetriedOnTheSameSession() = runTest {
        val size = DriveApi.MULTIPART_LIMIT + 1
        val src = bigFile("big.bin", size)
        server.enqueue(sessionResponse("/upload/session/s1"))
        server.enqueue(apiError(503, "backendError"))
        server.enqueue(jsonResponse(fileJson("big1")))
        assertEquals("big1", api.upload(account, src, "big.bin", "parent1", null, null).file.id)
        server.take()
        assertEquals("/upload/session/s1", server.take().requestUrl!!.encodedPath)
        val retry = server.take()
        assertEquals("/upload/session/s1", retry.requestUrl!!.encodedPath)
        assertEquals("bytes 0-${size - 1}/$size", retry.getHeader("Content-Range"))
        assertTrue(currentTime >= 1000L)
    }

    // ================================================================ revisions

    @Test
    fun listRevisions_paginatesAndParses() = runTest {
        server.enqueue(jsonResponse("""{"nextPageToken":"n","revisions":[{"id":"1","modifiedTime":"2026-01-02T03:04:05.000Z","size":"5","md5Checksum":"x","keepForever":true,"originalFilename":"a.txt","mimeType":"text/plain","lastModifyingUser":{"displayName":"A","emailAddress":"a@b.c"}}]}"""))
        server.enqueue(jsonResponse("""{"revisions":[{"id":"2"}]}"""))
        val revs = api.listRevisions(account, "f1")
        assertEquals(listOf("1", "2"), revs.map { it.id })
        assertTrue(revs[0].keepForever)
        assertEquals("5", revs[0].size)
        assertEquals("x", revs[0].md5Checksum)
        assertEquals("a.txt", revs[0].originalFilename)
        assertEquals("A", revs[0].lastModifyingUser!!.displayName)
        assertFalse(revs[1].keepForever)
        assertNull(revs[1].lastModifyingUser)

        val r1 = server.take().requestUrl!!
        assertEquals("/drive/v3/files/f1/revisions", r1.encodedPath)
        assertEquals("200", r1.queryParameter("pageSize"))
        assertNull(r1.queryParameter("pageToken"))
        assertTrue(r1.queryParameter("fields")!!.contains("revisions(id,modifiedTime,size,md5Checksum,keepForever"))
        assertEquals("n", server.take().requestUrl!!.queryParameter("pageToken"))
    }

    @Test
    fun keepRevision_patchesKeepForever() = runTest {
        server.enqueue(jsonResponse("{}"))
        api.keepRevision(account, "f1", "r9", true)
        val req = server.take()
        assertEquals("PATCH", req.method)
        assertEquals("/drive/v3/files/f1/revisions/r9", req.requestUrl!!.encodedPath)
        assertTrue(req.getHeader("Content-Type")!!.startsWith("application/json"))
        assertEquals("""{"keepForever":true}""", req.body.readUtf8())

        server.enqueue(jsonResponse("{}"))
        api.keepRevision(account, "f1", "r9", false)
        assertEquals("""{"keepForever":false}""", server.take().body.readUtf8())
    }

    // ================================================================ folders and metadata patches

    @Test
    fun createFolder_postsFolderMetadata() = runTest {
        server.enqueue(jsonResponse(fileJson("d1", "New", FOLDER_MIME, parent = "p1")))
        val f = api.createFolder(account, "p1", "New")
        assertEquals("d1", f.id)
        assertTrue(f.isFolder)
        val req = server.take()
        assertEquals("POST", req.method)
        assertEquals("/drive/v3/files", req.requestUrl!!.encodedPath)
        assertEquals(FILE_FIELDS, req.requestUrl!!.queryParameter("fields"))
        assertEquals("true", req.requestUrl!!.queryParameter("supportsAllDrives"))
        assertTrue(req.getHeader("Content-Type")!!.startsWith("application/json"))
        val meta = appJson.parseToJsonElement(req.body.readUtf8()) as JsonObject
        assertEquals("New", meta["name"]!!.jsonPrimitive.content)
        assertEquals(FOLDER_MIME, meta["mimeType"]!!.jsonPrimitive.content)
        assertEquals("p1", meta["parents"]!!.jsonArray.single().jsonPrimitive.content)
        assertNull(meta["modifiedTime"])
    }

    @Test
    fun trashUntrashRename_sendMinimalPatchBodies() = runTest {
        repeat(3) { server.enqueue(jsonResponse(fileJson("f1"))) }

        api.trash(account, "f1")
        val t = server.take()
        assertEquals("PATCH", t.method)
        assertEquals("/drive/v3/files/f1", t.requestUrl!!.encodedPath)
        assertEquals(FILE_FIELDS, t.requestUrl!!.queryParameter("fields"))
        assertEquals("true", t.requestUrl!!.queryParameter("supportsAllDrives"))
        assertNull(t.requestUrl!!.queryParameter("addParents"))
        assertNull(t.requestUrl!!.queryParameter("removeParents"))
        assertEquals("""{"trashed":true}""", t.body.readUtf8())

        api.untrash(account, "f1")
        assertEquals("""{"trashed":false}""", server.take().body.readUtf8())

        api.rename(account, "f1", "renamed.txt")
        assertEquals("""{"name":"renamed.txt"}""", server.take().body.readUtf8())
    }

    @Test
    fun move_usesAddParentsAndRemoveParents() = runTest {
        server.enqueue(jsonResponse(fileJson("f1")))
        api.move(account, "f1", fromParent = "from", toParent = "to")
        val req = server.take()
        assertEquals("PATCH", req.method)
        assertEquals("to", req.requestUrl!!.queryParameter("addParents"))
        assertEquals("from", req.requestUrl!!.queryParameter("removeParents"))
        assertEquals("{}", req.body.readUtf8())
    }

    @Test
    fun patch_withModifiedTime_sendsRfc3339() = runTest {
        server.enqueue(jsonResponse(fileJson("f1")))
        val mtime = Instant.parse("2025-01-02T03:04:05Z").toEpochMilli()
        api.patch(account, "f1", PatchFileMetadata(modifiedTime = com.almodhayan.drivesync.core.TimeFmt.toRfc3339(mtime)))
        assertEquals("""{"modifiedTime":"2025-01-02T03:04:05Z"}""", server.take().body.readUtf8())
    }

    @Test
    fun deletePermanently_sendsDelete() = runTest {
        server.enqueue(MockResponse().setResponseCode(204))
        api.deletePermanently(account, "f1")
        val req = server.take()
        assertEquals("DELETE", req.method)
        assertEquals("/drive/v3/files/f1", req.requestUrl!!.encodedPath)
        assertEquals("true", req.requestUrl!!.queryParameter("supportsAllDrives"))
    }
}
