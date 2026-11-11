package com.almodhayan.drivesync.migration

import android.content.Context
import android.net.Uri
import com.almodhayan.drivesync.backup.ConfigBackup
import com.almodhayan.drivesync.backup.ImportMode
import com.almodhayan.drivesync.backup.ImportReport
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.engine.WaslRuntime
import com.kahflabs.wasl.ffi.ImportedRun
import com.kahflabs.wasl.ffi.WaslEngine
import com.kahflabs.wasl.ffi.WaslException
import com.kahflabs.wasl.ffi.core.BaselineEntry
import com.kahflabs.wasl.ffi.core.ConflictPolicy
import com.kahflabs.wasl.ffi.core.RunStatus
import com.kahflabs.wasl.ffi.store.BinSide
import com.kahflabs.wasl.ffi.store.ConflictRecord
import com.kahflabs.wasl.ffi.store.EventLevel
import com.kahflabs.wasl.ffi.store.NewBinEntry
import com.kahflabs.wasl.ffi.store.RunEvent
import com.kahflabs.wasl.ffi.store.RunRecord
import com.kahflabs.wasl.ffi.store.RunStats
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.longOrNull
import java.io.File
import java.io.InputStream
import java.util.zip.ZipInputStream
import javax.inject.Inject
import javax.inject.Singleton

/**
 * Imports the file Drive Sync 1.1.5 writes with "Export for Wasl" (SRS FR-K2, docs/MIGRATION.md).
 * The configuration goes through Settings › Backup's merge (pairs keep their ids, vaults arrive
 * locked); each pair's baseline, history and open conflicts, and the recycle bin with its files, go
 * into the engine. Every imported pair is held for one preview. Importing the same file twice
 * changes nothing the second time.
 *
 * The file comes from outside the app: entry names are not trusted, sizes are capped, and the engine
 * checks the ids that name folders.
 */
@Singleton
class DriveSyncImport @Inject constructor(
    @ApplicationContext private val context: Context,
    private val runtime: WaslRuntime,
    private val backup: ConfigBackup,
) {
    data class Report(
        val config: ImportReport,
        val pairs: Int,
        val baselineRows: Int,
        val runs: Int,
        val conflicts: Int,
        val binImported: Int,
        val binFailed: Int,
    )

    suspend fun import(uri: Uri): Report {
        val input = context.contentResolver.openInputStream(uri) ?: throw AppError.Io(Strings.get(R.string.migration_error_open))
        return input.use { import(it) }
    }

    suspend fun import(input: InputStream): Report {
        val work = File(context.cacheDir, "drive-sync-import").apply { deleteRecursively(); mkdirs() }
        try {
            val (json, binFiles) = withContext(Dispatchers.IO) { unpackDriveSyncExport(input, work) }
            val report = applyDriveSyncExport(runtime.engine, json, binFiles) { config -> backup.import(backup.parse(config), ImportMode.MERGE) }
            runtime.changed()
            return report
        } finally {
            withContext(Dispatchers.IO) { work.deleteRecursively() }
        }
    }

    companion object {
        const val FORMAT = "drive-sync-to-wasl"
        const val VERSION = 1
        internal const val BIN_DAYS = 30
        internal const val MAX_JSON_BYTES = 256L shl 20
        internal const val MAX_TOTAL_BYTES = 64L shl 30
    }
}

/** Reads the zip: migration.json into memory, each bin/<id> into [work] under a name of our own. */
internal fun unpackDriveSyncExport(input: InputStream, work: File): Pair<JsonObject, Map<String, File>> {
    var json: JsonObject? = null
    val bin = HashMap<String, File>()
    var total = 0L
    ZipInputStream(input.buffered()).use { zip ->
        while (true) {
            val e = zip.nextEntry ?: break
            if (e.isDirectory) continue
            when {
                e.name == "migration.json" -> {
                    val bytes = zip.readCapped(DriveSyncImport.MAX_JSON_BYTES)
                    json = Json.parseToJsonElement(bytes.toString(Charsets.UTF_8)) as? JsonObject
                }
                e.name.startsWith("bin/") -> {
                    val id = e.name.removePrefix("bin/")
                    if (id.isEmpty() || id.any { it == '/' || it == '\\' }) continue
                    val f = File(work, "bin-${bin.size}")
                    f.outputStream().use { out -> total += zip.copyCapped(out, DriveSyncImport.MAX_TOTAL_BYTES - total) }
                    bin[id] = f
                }
            }
        }
    }
    val root = json ?: throw AppError.Unknown(Strings.get(R.string.migration_error_not_export))
    if (root.str("format") != DriveSyncImport.FORMAT) throw AppError.Unknown(Strings.get(R.string.migration_error_not_export))
    val version = root.int("version") ?: 0
    if (version < 1 || version > DriveSyncImport.VERSION) throw AppError.Unknown(Strings.get(R.string.migration_error_newer))
    return root to bin
}

/** Writes [root] into [engine]; [importConfig] takes the configuration backup's JSON. */
internal suspend fun applyDriveSyncExport(
    engine: WaslEngine,
    root: JsonObject,
    binFiles: Map<String, File>,
    importConfig: suspend (String) -> ImportReport,
): DriveSyncImport.Report {
    val configJson = root["config"]?.let { Json.encodeToString(JsonElement.serializer(), it) }
        ?: throw AppError.Unknown(Strings.get(R.string.migration_error_empty))
    val config = importConfig(configJson)
    // Only pairs this file's own configuration brought in take its state, and every one of them is
    // held for a preview before any state is written: a baseline from a file decides what the first
    // run deletes, so the user sees that run before anything moves.
    val fromFile = (root["config"] as? JsonObject)?.arr("pairs").orEmpty().mapNotNull { (it as? JsonObject)?.str("id") }.toSet()
    val known = engine.pairs().filter { it.id in fromFile }.associateBy { it.id }
    known.keys.forEach { engine.setPairFlags(it, null, null, true) }

    var pairs = 0
    var baselineRows = 0
    var runs = 0
    var conflicts = 0
    for (p in root.arr("pairs").mapNotNull { it as? JsonObject }) {
        val id = p.str("pairId") ?: continue
        if (id !in known) continue
        val events = p.arr("events").mapNotNull { it as? JsonObject }.groupBy { it.str("runId") }
        val counts = withContext(Dispatchers.IO) {
            engine.importPairState(
                id,
                p.arr("baseline").mapNotNull { (it as? JsonObject)?.baseline() },
                p.arr("runs").mapNotNull { (it as? JsonObject)?.importedRun(id, events) },
                p.arr("conflicts").mapNotNull { (it as? JsonObject)?.conflict(id) },
            )
        }
        pairs++
        baselineRows += counts.baseline.toInt()
        runs += counts.runs.toInt()
        conflicts += counts.conflicts.toInt()
    }

    var binImported = 0
    var binFailed = 0
    for (t in root.arr("bin").mapNotNull { it as? JsonObject }) {
        val entry = t.binEntry(known) ?: continue
        val source = binFiles[entry.id]?.path
        if (entry.side == BinSide.LOCAL && source == null) {
            binFailed++
            continue
        }
        try {
            if (withContext(Dispatchers.IO) { engine.importBinEntry(entry, source) }) binImported++
        } catch (e: WaslException) {
            binFailed++
        }
    }
    return DriveSyncImport.Report(config, pairs, baselineRows, runs, conflicts, binImported, binFailed)
}

private fun JsonObject.baseline(): BaselineEntry? {
    val key = str("key") ?: return null
    return BaselineEntry(
        key = key,
        localRel = str("localRel") ?: key,
        isDir = bool("isDir") ?: false,
        localSize = (long("localSize") ?: 0).coerceAtLeast(0).toULong(),
        localMtime = long("localMtime") ?: 0,
        localHash = str("localHash"),
        remoteId = str("remoteId"),
        remoteMd5 = str("remoteMd5"),
        remoteSize = long("remoteSize")?.coerceAtLeast(0)?.toULong(),
        remoteMtime = long("remoteMtime"),
        remoteVersion = long("remoteVersion"),
        exportMime = str("exportMime"),
    )
}

private fun JsonObject.importedRun(pairId: String, events: Map<String?, List<JsonObject>>): ImportedRun? {
    val id = str("id") ?: return null
    val started = long("startedAt") ?: return null
    fun u(name: String) = (int(name) ?: 0).coerceAtLeast(0).toUInt()
    fun ul(name: String) = (long(name) ?: 0).coerceAtLeast(0).toULong()
    return ImportedRun(
        run = RunRecord(
            id = id,
            pairId = pairId,
            trigger = str("trigger") ?: "manual",
            status = enumOr(str("status"), RunStatus.FAILED),
            stats = RunStats(
                uploaded = u("uploaded"), downloaded = u("downloaded"), exported = 0u,
                deletedLocal = u("deletedLocal"), deletedRemote = u("deletedRemote"), conflicts = u("conflicts"),
                skipped = u("skipped"), errors = u("errors"), bytesUp = ul("bytesUp"), bytesDown = ul("bytesDown"),
            ),
            message = str("message"),
            startedAt = started,
            endedAt = long("finishedAt"),
        ),
        events = events[id].orEmpty().map { e ->
            RunEvent(
                at = e.long("at") ?: started,
                level = enumOr(e.str("level"), EventLevel.INFO),
                action = e.str("action") ?: "INFO",
                key = e.str("path"),
                message = e.str("message") ?: "",
                detail = e.str("detail"),
                bytes = (e.long("bytes") ?: 0).coerceAtLeast(0).toULong(),
            )
        },
    )
}

private fun JsonObject.conflict(pairId: String): ConflictRecord? {
    val id = str("id") ?: return null
    val key = str("key") ?: return null
    return ConflictRecord(
    id = id,
    pairId = pairId,
    key = key,
    detectedAt = long("detectedAt") ?: 0,
    policy = enumOr(str("policy"), ConflictPolicy.KEEP_BOTH),
    localCopyRel = str("localCopyRel"),
    remoteFileId = str("remoteFileId"),
    message = str("message") ?: "",
    resolved = bool("resolved") ?: false,
    resolvedAt = long("resolvedAt"),
    )
}

/** A 1.1 bin row as the engine's entry; the key comes from syncKey or the path inside the pair. */
private fun JsonObject.binEntry(pairs: Map<String, com.kahflabs.wasl.ffi.store.Pair>): NewBinEntry? {
    val id = str("id") ?: return null
    val pairId = str("pairId") ?: return null
    val pair = pairs[pairId] ?: return null
    val drive = str("side") == "drive"
    val original = str("originalPath") ?: ""
    val root = (if (drive) pair.remotePath else pair.localRoot).trimEnd('/')
    val key = str("syncKey")?.takeIf { it.isNotBlank() }
        ?: original.takeIf { it.startsWith("$root/") }?.removePrefix("$root/")
        ?: original.substringAfterLast('/').takeIf { it.isNotBlank() }
        ?: return null
    val created = long("createdAt") ?: System.currentTimeMillis()
    return NewBinEntry(
        id = id,
        pairId = pairId,
        side = if (drive) BinSide.DRIVE else BinSide.LOCAL,
        key = key,
        reason = str("reason") ?: "DELETED",
        location = "",
        deletedAt = created,
        expiresAt = created + DriveSyncImport.BIN_DAYS * 86_400_000L,
        size = (long("size") ?: 0).coerceAtLeast(0).toULong(),
        accountId = str("accountId") ?: pair.accountId,
        remoteFileId = str("remoteFileId"),
        displayName = str("displayName"),
        pairName = str("pairName") ?: pair.name,
    )
}

private fun JsonObject.prim(name: String): JsonPrimitive? = (this[name] as? JsonPrimitive)?.takeIf { it !is JsonNull }
private fun JsonObject.str(name: String): String? = prim(name)?.takeIf { it.isString }?.content
private fun JsonObject.long(name: String): Long? = prim(name)?.longOrNull
private fun JsonObject.int(name: String): Int? = prim(name)?.intOrNull
private fun JsonObject.bool(name: String): Boolean? = prim(name)?.booleanOrNull
private fun JsonObject.arr(name: String): JsonArray = (this[name] as? JsonArray) ?: JsonArray(emptyList())

private inline fun <reified E : Enum<E>> enumOr(name: String?, fallback: E): E =
    name?.let { n -> enumValues<E>().firstOrNull { it.name == n } } ?: fallback

private fun InputStream.readCapped(max: Long): ByteArray {
    val out = java.io.ByteArrayOutputStream()
    copyCapped(out, max)
    return out.toByteArray()
}

/** Copies at most [max] bytes; a longer entry is refused rather than cut. */
private fun InputStream.copyCapped(out: java.io.OutputStream, max: Long): Long {
    val buf = ByteArray(1 shl 16)
    var total = 0L
    while (true) {
        val n = read(buf)
        if (n < 0) return total
        total += n
        if (total > max) throw AppError.Io(Strings.get(R.string.migration_error_too_large))
        out.write(buf, 0, n)
    }
}
