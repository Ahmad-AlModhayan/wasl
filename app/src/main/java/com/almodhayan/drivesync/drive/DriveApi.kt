package com.almodhayan.drivesync.drive

import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.TimeFmt
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.MultipartBody
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import okio.BufferedSink
import java.io.File
import java.io.RandomAccessFile
import java.util.concurrent.TimeUnit
import javax.inject.Inject
import javax.inject.Singleton
import kotlin.math.min
import kotlin.random.Random

/** Base URLs, overridable in tests (MockWebServer). */
data class DriveEndpoints(val base: String = DriveApi.BASE, val upload: String = DriveApi.UPLOAD)

/** Thin, resumable-aware Google Drive v3 REST client. One instance serves all accounts. */
@Singleton
class DriveApi @Inject constructor(
    private val client: OkHttpClient,
    private val auth: TokenProvider,
    private val json: Json,
    private val endpoints: DriveEndpoints,
) {
    private val jsonType = "application/json; charset=utf-8".toMediaType()

    // ---------------------------------------------------------------- core request plumbing

    private suspend fun <T> withRetry(account: String, maxAttempts: Int = 6, block: suspend (token: String) -> T): T {
        var attempt = 0
        var refreshed = false
        while (true) {
            attempt++
            try {
                // Inside the try: a network blip while refreshing the token is retried like any other.
                return block(auth.accessToken(account))
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                val err = AppError.wrap(t)
                if (err is AppError.Auth && !refreshed && err.resolution == null) {
                    refreshed = true
                    auth.invalidate(account)
                    continue
                }
                if (!err.isTransient || attempt >= maxAttempts) throw err
                val backoff = when (err) {
                    is AppError.RateLimited -> if (err.retryAfterMs > 0) err.retryAfterMs else expBackoff(attempt)
                    else -> expBackoff(attempt)
                }
                delay(backoff)
            }
        }
    }

    private fun expBackoff(attempt: Int): Long = min(64_000L, (1000L shl (attempt - 1))) + Random.nextLong(0, 1000)

    private fun Request.Builder.bearer(token: String) = header("Authorization", "Bearer $token")

    /** OkHttp reports transport failures as plain IOException; they are network errors, not local storage errors. */
    private inline fun <T> net(block: () -> T): T = try { block() } catch (e: AppError) { throw e } catch (e: java.io.IOException) { throw AppError.Network(e.message ?: "network error", e) }

    /**
     * Runs [req] and hands the response to [use]. Cancelling the coroutine cancels the HTTP call, so a
     * stopped sync never waits on a slow or silent connection. [timeoutMs] bounds the whole call
     * (0 = none: transfers report progress instead, and the sync's stall watchdog covers them).
     */
    private suspend fun <T> exchange(req: Request, timeoutMs: Long = CALL_TIMEOUT_MS, use: suspend (Response) -> T): T = coroutineScope {
        val call = client.newCall(req)
        if (timeoutMs > 0) call.timeout().timeout(timeoutMs, TimeUnit.MILLISECONDS)
        var done = false
        val canceller = launch(start = CoroutineStart.UNDISPATCHED) {
            try { awaitCancellation() } finally { if (!done) call.cancel() }
        }
        try {
            withContext(Dispatchers.IO) { net { call.execute() }.use { use(it) } }
        } catch (e: Throwable) {
            ensureActive()   // the call failed because the sync stopped: that is a cancellation, not a network error
            throw e
        } finally {
            done = true
            canceller.cancel()
        }
    }

    private suspend fun <T> execute(req: Request, timeoutMs: Long = CALL_TIMEOUT_MS, parse: (Response) -> T): T = exchange(req, timeoutMs) { resp ->
        if (!resp.isSuccessful) throw DriveErrors.fromResponse(resp, net { resp.body?.string() })
        net { parse(resp) }
    }

    private inline fun <reified T> Response.decode(): T = json.decodeFromString(body!!.string())

    private fun url(path: String, vararg params: Pair<String, String?>): HttpUrl {
        val b = "${endpoints.base}/$path".toHttpUrl().newBuilder()
        params.forEach { (k, v) -> if (v != null) b.addQueryParameter(k, v) }
        return b.build()
    }

    // ---------------------------------------------------------------- metadata

    suspend fun about(account: String): About = withRetry(account) { t -> aboutWithToken(t) }

    /** Used right after interactive sign-in, before the account email is known. */
    suspend fun aboutWithToken(accessToken: String): About =
        execute(Request.Builder().url(url("about", "fields" to "user(emailAddress,displayName,photoLink),storageQuota(limit,usage,usageInDrive,usageInDriveTrash)")).bearer(accessToken).build()) { it.decode() }

    /** Resolve the id of the My Drive root. */
    suspend fun rootId(account: String): String = getFile(account, "root").id

    suspend fun getFile(account: String, fileId: String, fields: String = FILE_FIELDS): DriveFile = withRetry(account) { t ->
        execute(Request.Builder().url(url("files/$fileId", "fields" to fields, "supportsAllDrives" to "true")).bearer(t).build()) { it.decode() }
    }

    suspend fun listPage(
        account: String,
        q: String?,
        pageToken: String?,
        driveId: String? = null,
        spaces: String = "drive",
        pageSize: Int = 1000,
        orderBy: String? = null,
    ): FileList = withRetry(account) { t ->
        val u = url(
            "files",
            "q" to q,
            "pageToken" to pageToken,
            "pageSize" to pageSize.toString(),
            "fields" to "nextPageToken,incompleteSearch,files($FILE_FIELDS)",
            "spaces" to spaces,
            "supportsAllDrives" to "true",
            "includeItemsFromAllDrives" to (driveId != null).toString(),
            "corpora" to if (driveId != null) "drive" else "user",
            "driveId" to driveId,
            "orderBy" to orderBy,
        )
        execute(Request.Builder().url(u).bearer(t).build()) { it.decode() }
    }

    suspend fun listChildren(account: String, parentId: String, driveId: String? = null): List<DriveFile> {
        val out = ArrayList<DriveFile>()
        var token: String? = null
        do {
            val page = listPage(account, "'$parentId' in parents and trashed = false", token, driveId)
            out += page.files
            token = page.nextPageToken
        } while (token != null)
        return out
    }

    suspend fun listAppData(account: String): List<DriveFile> {
        val out = ArrayList<DriveFile>()
        var token: String? = null
        do {
            val page = listPage(account, null, token, spaces = "appDataFolder")
            out += page.files; token = page.nextPageToken
        } while (token != null)
        return out
    }

    suspend fun listSharedDrives(account: String): List<SharedDrive> {
        val out = ArrayList<SharedDrive>()
        var token: String? = null
        do {
            val page: DriveList = withRetry(account) { t ->
                execute(Request.Builder().url(url("drives", "pageSize" to "100", "pageToken" to token, "fields" to "nextPageToken,drives(id,name)")).bearer(t).build()) { it.decode() }
            }
            out += page.drives; token = page.nextPageToken
        } while (token != null)
        return out
    }

    suspend fun startPageToken(account: String, driveId: String? = null): String = withRetry(account) { t ->
        execute(Request.Builder().url(url("changes/startPageToken", "supportsAllDrives" to "true", "driveId" to driveId)).bearer(t).build()) { it.decode<StartPageToken>().startPageToken }
    }

    suspend fun changesPage(account: String, pageToken: String, driveId: String? = null): ChangeList = withRetry(account) { t ->
        val u = url(
            "changes",
            "pageToken" to pageToken,
            "pageSize" to "1000",
            "includeRemoved" to "true",
            "supportsAllDrives" to "true",
            "includeItemsFromAllDrives" to "true",
            "driveId" to driveId,
            "fields" to "nextPageToken,newStartPageToken,changes(changeType,fileId,removed,driveId,time,file($FILE_FIELDS))",
        )
        execute(Request.Builder().url(u).bearer(t).build()) { it.decode() }
    }

    suspend fun createFolder(account: String, parentId: String, name: String): DriveFile = withRetry(account) { t ->
        val body = json.encodeToString(NewFileMetadata.serializer(), NewFileMetadata(name, FOLDER_MIME, listOf(parentId))).toRequestBody(jsonType)
        execute(Request.Builder().url(url("files", "fields" to FILE_FIELDS, "supportsAllDrives" to "true")).bearer(t).post(body).build()) { it.decode() }
    }

    suspend fun patch(account: String, fileId: String, meta: PatchFileMetadata, addParents: String? = null, removeParents: String? = null): DriveFile = withRetry(account) { t ->
        val body = json.encodeToString(PatchFileMetadata.serializer(), meta).toRequestBody(jsonType)
        val u = url("files/$fileId", "fields" to FILE_FIELDS, "supportsAllDrives" to "true", "addParents" to addParents, "removeParents" to removeParents)
        execute(Request.Builder().url(u).bearer(t).patch(body).build()) { it.decode() }
    }

    suspend fun trash(account: String, fileId: String) { patch(account, fileId, PatchFileMetadata(trashed = true)) }
    suspend fun untrash(account: String, fileId: String) { patch(account, fileId, PatchFileMetadata(trashed = false)) }
    suspend fun rename(account: String, fileId: String, name: String) { patch(account, fileId, PatchFileMetadata(name = name)) }
    suspend fun move(account: String, fileId: String, fromParent: String, toParent: String) { patch(account, fileId, PatchFileMetadata(), addParents = toParent, removeParents = fromParent) }

    suspend fun deletePermanently(account: String, fileId: String) = withRetry(account) { t ->
        execute(Request.Builder().url(url("files/$fileId", "supportsAllDrives" to "true")).bearer(t).delete().build()) { }
    }

    // ---------------------------------------------------------------- revisions

    suspend fun listRevisions(account: String, fileId: String): List<Revision> {
        val out = ArrayList<Revision>()
        var token: String? = null
        do {
            val page: RevisionList = withRetry(account) { t ->
                execute(Request.Builder().url(url("files/$fileId/revisions", "pageSize" to "200", "pageToken" to token,
                    "fields" to "nextPageToken,revisions(id,modifiedTime,size,md5Checksum,keepForever,originalFilename,mimeType,lastModifyingUser(displayName,emailAddress))")).bearer(t).build()) { it.decode() }
            }
            out += page.revisions; token = page.nextPageToken
        } while (token != null)
        return out
    }

    suspend fun keepRevision(account: String, fileId: String, revisionId: String, keep: Boolean) = withRetry(account) { t ->
        val body = json.encodeToString(PatchRevision.serializer(), PatchRevision(keep)).toRequestBody(jsonType)
        execute(Request.Builder().url(url("files/$fileId/revisions/$revisionId")).bearer(t).patch(body).build()) { }
    }

    suspend fun downloadRevision(account: String, fileId: String, revisionId: String, dest: File, onProgress: (Long, Long?) -> Unit = { _, _ -> }) {
        downloadUrl(account, url("files/$fileId/revisions/$revisionId", "alt" to "media"), dest, null, onProgress)
    }

    // ---------------------------------------------------------------- downloads

    /** Downloads with byte-range resumption into [dest] (a .part file the caller renames). */
    suspend fun download(account: String, fileId: String, dest: File, expectedSize: Long?, onProgress: (Long, Long?) -> Unit = { _, _ -> }) {
        downloadUrl(account, url("files/$fileId", "alt" to "media", "supportsAllDrives" to "true"), dest, expectedSize, onProgress)
    }

    suspend fun export(account: String, fileId: String, mime: String, dest: File, onProgress: (Long, Long?) -> Unit = { _, _ -> }) {
        if (dest.exists()) dest.delete()   // exports are not range-resumable
        downloadUrl(account, url("files/$fileId/export", "mimeType" to mime), dest, null, onProgress, allowResume = false)
    }

    /** Reads an arbitrary byte range (used by the on-demand provider). Returns bytes actually read. */
    suspend fun readRange(account: String, fileId: String, offset: Long, length: Int, into: ByteArray): Int = withRetry(account) { t ->
        val req = Request.Builder().url(url("files/$fileId", "alt" to "media", "supportsAllDrives" to "true")).bearer(t)
            .header("Range", "bytes=$offset-${offset + length - 1}").build()
        exchange(req) { resp ->
            if (resp.code == 416) return@exchange 0
            if (!resp.isSuccessful) throw DriveErrors.fromResponse(resp, net { resp.body?.string() })
            val input = resp.body!!.byteStream()
            var read = 0
            while (read < length) {
                val n = net { input.read(into, read, length - read) }
                if (n < 0) break
                read += n
            }
            read
        }
    }

    private suspend fun downloadUrl(account: String, u: HttpUrl, dest: File, expectedSize: Long?, onProgress: (Long, Long?) -> Unit, allowResume: Boolean = true) {
        withRetry(account) { t ->
            var existing = if (allowResume && dest.exists()) dest.length() else 0L
            // Left by an interrupted download of an older, bigger version: resuming it can never work.
            if (expectedSize != null && existing > expectedSize) { dest.delete(); existing = 0L }
            if (expectedSize != null && existing == expectedSize && existing > 0) return@withRetry
            val rb = Request.Builder().url(u).bearer(t)
            if (existing > 0) rb.header("Range", "bytes=$existing-")
            exchange(rb.build(), timeoutMs = 0) { resp ->
                    if (resp.code == 416) {
                        // Nothing left to send. That is "complete" only when the size is right; otherwise
                        // the part file is stale and every later run would fail on it the same way.
                        if (expectedSize != null && existing != expectedSize) {
                            dest.delete()
                            throw AppError.Integrity("stale partial download of ${dest.name}: $existing bytes, expected $expectedSize")
                        }
                        return@exchange
                    }
                    if (!resp.isSuccessful) throw DriveErrors.fromResponse(resp, net { resp.body?.string() })
                    val append = resp.code == 206 && existing > 0
                    val total = expectedSize ?: resp.body?.contentLength()?.takeIf { it >= 0 }?.let { it + (if (append) existing else 0) }
                    val input = resp.body!!.byteStream()
                    RandomAccessFile(dest, "rw").use { raf ->
                        if (append) raf.seek(existing) else raf.setLength(0)
                        val buf = ByteArray(256 * 1024)
                        var done = if (append) existing else 0L
                        var lastReport = 0L
                        while (true) {
                            currentCoroutineContext().ensureActive()
                            val n = net { input.read(buf) }
                            if (n < 0) break
                            BandwidthLimiter.acquire(n)
                            try { raf.write(buf, 0, n) } catch (e: java.io.IOException) { throw AppError.Io("cannot write ${dest.name}: ${e.message}", e) }
                            done += n
                            if (done - lastReport >= 1_000_000) { lastReport = done; onProgress(done, total) }
                        }
                        onProgress(done, total)
                    }
            }
        }
        if (expectedSize != null && dest.length() != expectedSize) {
            val got = dest.length()
            if (got > expectedSize) dest.delete()   // a shorter part is kept and resumed; a longer one never fits
            throw AppError.Integrity("size mismatch after download: $got != $expectedSize")
        }
    }

    // ---------------------------------------------------------------- uploads

    class UploadResult(val file: DriveFile)

    /**
     * Uploads [src] as new file (when [existingFileId] is null) or as new content of an existing
     * file. Files up to [MULTIPART_LIMIT] use a single multipart request; larger ones use the
     * resumable protocol with [sessionStore] persistence so interrupted uploads continue.
     */
    suspend fun upload(
        account: String,
        src: File,
        name: String,
        parentId: String?,
        existingFileId: String?,
        modifiedTime: Long?,
        mimeType: String = "application/octet-stream",
        sessionStore: UploadSessionStore? = null,
        sessionKey: String? = null,
        onProgress: (Long, Long) -> Unit = { _, _ -> },
    ): UploadResult {
        val size = src.length()
        val meta = if (existingFileId == null) {
            json.encodeToString(NewFileMetadata.serializer(), NewFileMetadata(name, null, parentId?.let { listOf(it) }, modifiedTime?.let { TimeFmt.toRfc3339(it) }))
        } else {
            json.encodeToString(PatchFileMetadata.serializer(), PatchFileMetadata(name = name, modifiedTime = modifiedTime?.let { TimeFmt.toRfc3339(it) }))
        }
        return if (size <= MULTIPART_LIMIT) {
            uploadMultipart(account, src, meta, existingFileId, mimeType, onProgress)
        } else {
            uploadResumable(account, src, meta, existingFileId, mimeType, sessionStore, sessionKey, onProgress)
        }
    }

    private suspend fun uploadMultipart(account: String, src: File, meta: String, existingFileId: String?, mimeType: String, onProgress: (Long, Long) -> Unit): UploadResult =
        withRetry(account) { t ->
            val body = MultipartBody.Builder().setType("multipart/related".toMediaType())
                .addPart(meta.toRequestBody(jsonType))
                .addPart(src.asRequestBody(mimeType.toMediaType()) { done -> onProgress(done, src.length()) })
                .build()
            val u = uploadUrl(existingFileId, "multipart")
            val rb = Request.Builder().url(u).bearer(t)
            if (existingFileId == null) rb.post(body) else rb.patch(body)
            execute(rb.build(), timeoutMs = 0) { UploadResult(it.decode()) }
        }

    private suspend fun uploadResumable(
        account: String, src: File, meta: String, existingFileId: String?, mimeType: String,
        sessionStore: UploadSessionStore?, sessionKey: String?, onProgress: (Long, Long) -> Unit,
    ): UploadResult {
        val size = src.length()
        var sessionUri: String? = sessionKey?.let { sessionStore?.get(it) }
        var offset = 0L

        suspend fun startSession(): String = withRetry(account) { t ->
            val req = Request.Builder().url(uploadUrl(existingFileId, "resumable")).bearer(t)
                .header("X-Upload-Content-Type", mimeType)
                .header("X-Upload-Content-Length", size.toString())
                .apply { if (existingFileId == null) post(meta.toRequestBody(jsonType)) else patch(meta.toRequestBody(jsonType)) }
                .build()
            execute(req) { it.header("Location") ?: throw AppError.Http(it.code, null, "resumable session without Location") }
        }

        if (sessionUri != null) {
            // Query status of an existing session.
            val status = queryResumable(account, sessionUri, size)
            when (status) {
                is ResumeStatus.Complete -> { sessionKey?.let { sessionStore?.remove(it) }; return UploadResult(status.file) }
                is ResumeStatus.Partial -> offset = status.nextOffset
                ResumeStatus.Expired -> { sessionUri = null; sessionKey?.let { sessionStore?.remove(it) } }
            }
        }
        if (sessionUri == null) {
            sessionUri = startSession()
            if (sessionKey != null) sessionStore?.put(sessionKey, sessionUri)
            offset = 0
        }

        RandomAccessFile(src, "r").use { raf ->
            val buf = ByteArray(CHUNK)
            while (offset < size) {
                currentCoroutineContext().ensureActive()
                val len = min(CHUNK.toLong(), size - offset).toInt()
                raf.seek(offset); raf.readFully(buf, 0, len)
                val end = offset + len - 1
                val body = buf.copyOf(len).toRequestBody(mimeType.toMediaType())
                val chunkStart = offset
                var throttled = 0
                while (throttled < len) { val slice = min(256 * 1024, len - throttled); BandwidthLimiter.acquire(slice); throttled += slice }
                val result: ChunkResult = withRetry(account, maxAttempts = 8) { t ->
                    val req = Request.Builder().url(sessionUri!!).bearer(t)
                        .header("Content-Range", "bytes $chunkStart-$end/$size").put(body).build()
                    exchange(req, timeoutMs = 0) { resp ->
                        when {
                            resp.code == 308 -> ChunkResult.More(parseRangeEnd(resp.header("Range")) ?: (end + 1))
                            resp.isSuccessful -> ChunkResult.Done(json.decodeFromString(DriveFile.serializer(), net { resp.body!!.string() }))
                            resp.code == 404 || resp.code == 410 -> throw AppError.NotFound("upload session expired")
                            else -> throw DriveErrors.fromResponse(resp, net { resp.body?.string() })
                        }
                    }
                }
                when (result) {
                    is ChunkResult.Done -> { sessionKey?.let { sessionStore?.remove(it) }; onProgress(size, size); return UploadResult(result.file) }
                    is ChunkResult.More -> { offset = result.nextOffset; onProgress(offset, size) }
                }
            }
        }
        // Zero-length or fully acknowledged without a final 200: query status.
        return when (val s = queryResumable(account, sessionUri!!, size)) {
            is ResumeStatus.Complete -> { sessionKey?.let { sessionStore?.remove(it) }; UploadResult(s.file) }
            else -> throw AppError.Http(0, null, "resumable upload did not complete")
        }
    }

    private sealed class ChunkResult { class More(val nextOffset: Long) : ChunkResult(); class Done(val file: DriveFile) : ChunkResult() }
    private sealed class ResumeStatus { class Complete(val file: DriveFile) : ResumeStatus(); class Partial(val nextOffset: Long) : ResumeStatus(); object Expired : ResumeStatus() }

    private suspend fun queryResumable(account: String, sessionUri: String, size: Long): ResumeStatus = withRetry(account) { t ->
        val req = Request.Builder().url(sessionUri).bearer(t).header("Content-Range", "bytes */$size").put(ByteArray(0).toRequestBody()).build()
        exchange(req) { resp ->
            when {
                resp.code == 308 -> ResumeStatus.Partial(parseRangeEnd(resp.header("Range")) ?: 0L)
                resp.isSuccessful -> ResumeStatus.Complete(json.decodeFromString(DriveFile.serializer(), net { resp.body!!.string() }))
                resp.code == 404 || resp.code == 410 || resp.code == 400 -> ResumeStatus.Expired
                else -> throw DriveErrors.fromResponse(resp, net { resp.body?.string() })
            }
        }
    }

    /** "bytes=0-524287" → 524288 (next offset). */
    private fun parseRangeEnd(range: String?): Long? = range?.substringAfter('-', "")?.toLongOrNull()?.plus(1)

    private fun uploadUrl(existingFileId: String?, type: String): HttpUrl {
        val base = if (existingFileId == null) "${endpoints.upload}/files" else "${endpoints.upload}/files/$existingFileId"
        return base.toHttpUrl().newBuilder().addQueryParameter("uploadType", type).addQueryParameter("supportsAllDrives", "true")
            .addQueryParameter("fields", FILE_FIELDS).build()
    }

    private fun File.asRequestBody(mediaType: okhttp3.MediaType, progress: (Long) -> Unit): RequestBody = object : RequestBody() {
        override fun contentType() = mediaType
        override fun contentLength() = length()
        override fun writeTo(sink: BufferedSink) {
            inputStream().use { input ->
                val buf = ByteArray(256 * 1024)
                var done = 0L
                while (true) {
                    val n = input.read(buf); if (n < 0) break
                    BandwidthLimiter.acquireBlocking(n)
                    sink.write(buf, 0, n); done += n; progress(done)
                }
            }
        }
    }

    interface UploadSessionStore {
        suspend fun get(key: String): String?
        suspend fun put(key: String, uri: String)
        suspend fun remove(key: String)
    }

    companion object {
        const val BASE = "https://www.googleapis.com/drive/v3"
        const val UPLOAD = "https://www.googleapis.com/upload/drive/v3"
        const val MULTIPART_LIMIT = 5L * 1024 * 1024
        const val CHUNK = 8 * 1024 * 1024          // multiple of 256 KiB as Drive requires
        /** Ceiling for one metadata request; a connection that trickles bytes would otherwise never end. */
        const val CALL_TIMEOUT_MS = 5 * 60_000L
    }
}
