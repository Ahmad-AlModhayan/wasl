package com.almodhayan.drivesync.engine

import android.content.Context
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract
import android.provider.DocumentsContract.Document
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.kahflabs.wasl.ffi.SafBridge
import com.kahflabs.wasl.ffi.SafEntry
import com.kahflabs.wasl.ffi.WaslException
import dagger.hilt.android.qualifiers.ApplicationContext
import java.io.File
import java.io.FileNotFoundException
import java.security.MessageDigest
import java.util.concurrent.atomic.AtomicLong
import javax.inject.Inject
import javax.inject.Singleton

/**
 * The Storage Access Framework side of a SAF pair (SRS FR-R3). The engine sends pair-relative
 * paths it has already checked; this class only turns them into documents. File contents never
 * cross into Kotlin: reads hand the engine a file descriptor, downloads arrive as a finished file in
 * the app's cache.
 *
 * SAF cannot move a document out of the folder the user granted, so the recycle bin keeps copies in
 * the app's own storage; restoring writes them back. SAF also cannot set a file's modification
 * time; the engine records the time the document reports.
 */
@Singleton
class SafBridgeImpl @Inject constructor(@ApplicationContext private val context: Context) : SafBridge {
    private val resolver get() = context.contentResolver
    private val binRoot: File get() = File(context.getExternalFilesDir(null) ?: context.filesDir, "wasl-saf-bin")
    private val counter = AtomicLong(0)

    private class Doc(val id: String, val name: String, val isDir: Boolean, val size: Long, val mtime: Long)

    private fun io(e: Throwable): WaslException = when (e) {
        is WaslException -> e
        is SecurityException -> WaslException.Permission(Strings.get(R.string.engine_saf_no_access))
        is FileNotFoundException -> WaslException.NotFound(e.message ?: Strings.get(R.string.engine_saf_not_found))
        is IllegalArgumentException ->
            if (isGone(e)) WaslException.NotFound(Strings.get(R.string.engine_saf_not_found)) else WaslException.Io(e.message ?: e.javaClass.simpleName)
        else -> WaslException.Io(e.message ?: e.javaClass.simpleName)
    }

    private inline fun <T> guard(block: () -> T): T = try { block() } catch (e: Throwable) { throw io(e) }

    /**
     * Opens the document at [rel] with [open], looking the path up again when the remembered
     * document is gone. Moved, renamed or deleted since the scan saw it (an app saving through a
     * temp file and a rename), the provider throws FileNotFoundException or, under a folder grant,
     * IllegalArgumentException ("Failed to determine if … is child of …"). Still missing after the
     * second lookup, it is a plain NotFound naming the file, which the engine skips (#154).
     */
    private fun <T : Any> openGone(tree: Uri, rel: String, open: (Uri) -> T?): T {
        val first = try {
            val d = findKnown(tree, rel) ?: throw FileNotFoundException(rel)
            open(docUri(tree, d.id))
        } catch (e: Exception) {
            if (!isGone(e)) throw e
            known.remove("$tree|$rel")
            null
        }
        if (first != null) return first
        val again = try {
            find(tree, rel)?.let { open(docUri(tree, it.id)) }
        } catch (e: Exception) {
            if (!isGone(e)) throw e
            null
        }
        return again ?: throw WaslException.NotFound(Strings.get(R.string.engine_saf_vanished, rel))
    }

    /** [guard] for a call that changes the tree: the listings of it are dropped before and after. */
    private inline fun <T> changing(tree: String, block: () -> T): T {
        val t = treeUri(tree)
        forget(t)
        try { return guard(block) } finally { forget(t) }
    }

    private fun treeUri(tree: String): Uri = Uri.parse(tree)

    private fun rootId(tree: Uri): String = DocumentsContract.getTreeDocumentId(tree)

    private fun docUri(tree: Uri, id: String): Uri = DocumentsContract.buildDocumentUriUsingTree(tree, id)

    /**
     * Recent folder listings by tree and folder id. Every file the engine stats or opens used to
     * walk its path from the root, listing each folder on the way: a folder of N files cost N
     * listings of N rows each (perf audit 2026-10-29). A listing is reused for [LISTING_TTL_MS] and
     * dropped whenever Wasl changes anything in that tree.
     */
    private val listings = java.util.concurrent.ConcurrentHashMap<String, Pair<Long, List<Doc>>>()

    /**
     * Documents by tree and pair-relative path, as the last scan listed them. A document id stays
     * valid until the document is moved or deleted, so opening a scanned file needs no lookup: a
     * preview of 12,000 files opened each one by walking its path (the owner's phone, 2026-10-29).
     */
    private val known = java.util.concurrent.ConcurrentHashMap<String, Doc>()

    private fun forget(tree: Uri) {
        val prefix = "$tree|"
        listings.keys.removeIf { it.startsWith(prefix) }
        known.keys.removeIf { it.startsWith(prefix) }
    }

    /** [find], from the last scan's listing when it saw [rel]. */
    private fun findKnown(tree: Uri, rel: String): Doc? = known["$tree|$rel"] ?: find(tree, rel)

    private fun children(tree: Uri, parentId: String): List<Doc> {
        val key = "$tree|$parentId"
        val now = android.os.SystemClock.elapsedRealtime()
        listings[key]?.let { (at, docs) -> if (now - at < LISTING_TTL_MS) return docs }
        return query(tree, parentId).also { listings[key] = now to it }
    }

    private fun query(tree: Uri, parentId: String): List<Doc> {
        val uri = DocumentsContract.buildChildDocumentsUriUsingTree(tree, parentId)
        val cols = arrayOf(Document.COLUMN_DOCUMENT_ID, Document.COLUMN_DISPLAY_NAME, Document.COLUMN_MIME_TYPE, Document.COLUMN_SIZE, Document.COLUMN_LAST_MODIFIED)
        val out = ArrayList<Doc>()
        resolver.query(uri, cols, null, null, null)?.use { c ->
            while (c.moveToNext()) {
                val name = c.getString(1) ?: continue
                out += Doc(
                    id = c.getString(0),
                    name = name,
                    isDir = c.getString(2) == Document.MIME_TYPE_DIR,
                    size = if (c.isNull(3)) 0 else c.getLong(3),
                    mtime = if (c.isNull(4)) 0 else c.getLong(4),
                )
            }
        } ?: throw FileNotFoundException(Strings.get(R.string.engine_saf_cannot_list))
        return out
    }

    /** The document at [rel] ("" is the pair's root), or null. */
    private fun find(tree: Uri, rel: String): Doc? {
        var cur = Doc(rootId(tree), "", true, 0, 0)
        if (rel.isEmpty()) return cur
        for (part in rel.split('/')) {
            if (!cur.isDir) return null
            cur = children(tree, cur.id).firstOrNull { it.name == part } ?: return null
        }
        return cur
    }

    /** The folder at [rel], created with any missing parents. */
    private fun ensureDir(tree: Uri, rel: String): Doc {
        var cur = Doc(rootId(tree), "", true, 0, 0)
        if (rel.isEmpty()) return cur
        for (part in rel.split('/')) {
            val existing = children(tree, cur.id).firstOrNull { it.name == part }
            cur = when {
                existing == null -> {
                    val created = DocumentsContract.createDocument(resolver, docUri(tree, cur.id), Document.MIME_TYPE_DIR, part)
                        ?: throw FileNotFoundException(Strings.get(R.string.engine_saf_cannot_create_folder, part))
                    forget(tree)
                    Doc(DocumentsContract.getDocumentId(created), part, true, 0, 0)
                }
                existing.isDir -> existing
                else -> throw WaslException.Io(Strings.get(R.string.engine_saf_file_not_folder, part))
            }
        }
        return cur
    }

    internal companion object {
        const val LISTING_TTL_MS = 3_000L

        /**
         * A provider's way of saying the document is no longer there: FileNotFoundException, or the
         * IllegalArgumentException Android's external storage provider throws when it can't
         * check a missing document against the folder grant.
         */
        fun isGone(e: Throwable): Boolean = when (e) {
            is FileNotFoundException -> true
            is IllegalArgumentException -> e.cause is FileNotFoundException ||
                e.message.orEmpty().let { "Failed to determine if" in it || "FileNotFoundException" in it || "Missing file" in it }
            else -> false
        }
    }

    private fun entry(d: Doc) = SafEntry(name = d.name, isDir = d.isDir, size = d.size.toULong(), mtimeMs = d.mtime)

    private fun parentOf(rel: String) = rel.substringBeforeLast('/', "")
    private fun nameOf(rel: String) = rel.substringAfterLast('/')

    override fun list(tree: String, dir: String): List<SafEntry> = guard {
        val t = treeUri(tree)
        val d = findKnown(t, dir) ?: throw FileNotFoundException(dir)
        // A scan reads every folder fresh; the files in it are then looked up from this listing.
        val docs = try {
            query(t, d.id)
        } catch (e: Exception) {
            // Moved, renamed or made again outside Wasl since the last scan saw it: the remembered
            // document is gone, so look the path up again (until the app restarted, the folder read
            // as unreadable on every sync).
            if (known.remove("$t|$dir") == null) throw e
            query(t, find(t, dir)?.id ?: throw e)
        }
        listings["$t|${d.id}"] = android.os.SystemClock.elapsedRealtime() to docs
        val prefix = if (dir.isEmpty()) "" else "$dir/"
        docs.forEach { known["$t|$prefix${it.name}"] = it }
        docs.map { entry(it) }
    }

    override fun stat(tree: String, rel: String): SafEntry? = guard { find(treeUri(tree), rel)?.let { entry(it) } }

    override fun openReadFd(tree: String, rel: String): Int = guard {
        openGone(treeUri(tree), rel) { resolver.openFileDescriptor(it, "r") }.detachFd()
    }

    override fun closeFd(fd: Int) {
        runCatching { ParcelFileDescriptor.adoptFd(fd).close() }
    }

    /**
     * Copies a document out through the folder grant, for a file Android won't let the app open by
     * its path: without All files access, a file another app made can only be read through the
     * stream the provider opens (#127).
     */
    override fun copyOut(tree: String, rel: String, dest: String) = guard {
        val input = openGone(treeUri(tree), rel) { resolver.openInputStream(it) }
        input.use { i -> File(dest).outputStream().use { i.copyTo(it, 1 shl 20) } }
        Unit
    }

    override fun install(tree: String, staged: String, rel: String, mtimeMs: Long?): SafEntry = changing(tree) {
        val t = treeUri(tree)
        val parent = ensureDir(t, parentOf(rel))
        val name = nameOf(rel)
        // Write under a part name, then rename: a half-written file never carries the real name.
        val partName = "$name.wasl-part"
        children(t, parent.id).firstOrNull { it.name == partName }?.let { DocumentsContract.deleteDocument(resolver, docUri(t, it.id)) }
        val part = DocumentsContract.createDocument(resolver, docUri(t, parent.id), "application/octet-stream", partName)
            ?: throw FileNotFoundException(Strings.get(R.string.engine_saf_cannot_create, partName))
        resolver.openOutputStream(part, "wt")?.use { out -> File(staged).inputStream().use { it.copyTo(out, 1 shl 20) } }
            ?: throw FileNotFoundException(Strings.get(R.string.engine_saf_cannot_write, partName))
        val renamed = DocumentsContract.renameDocument(resolver, part, name) ?: part
        val size = File(staged).length()
        File(staged).delete()
        forget(t)
        val d = find(t, rel) ?: Doc(DocumentsContract.getDocumentId(renamed), name, false, size, System.currentTimeMillis())
        entry(d)
    }

    override fun mkdir(tree: String, rel: String) = changing(tree) { ensureDir(treeUri(tree), rel); Unit }

    override fun removeFile(tree: String, rel: String) = changing(tree) {
        val t = treeUri(tree)
        val d = find(t, rel) ?: return@changing
        if (d.isDir) throw WaslException.Io(Strings.get(R.string.engine_saf_is_folder, rel))
        DocumentsContract.deleteDocument(resolver, docUri(t, d.id))
        Unit
    }

    override fun removeDirIfEmpty(tree: String, rel: String): Boolean = changing(tree) {
        if (rel.isEmpty()) return@changing false
        val t = treeUri(tree)
        val d = find(t, rel) ?: return@changing false
        val kids = query(t, d.id)
        if (kids.any { !it.name.endsWith(".wasl-part") }) return@changing false
        kids.forEach { DocumentsContract.deleteDocument(resolver, docUri(t, it.id)) }
        DocumentsContract.deleteDocument(resolver, docUri(t, d.id))
    }

    override fun rename(tree: String, from: String, to: String) = changing(tree) {
        val t = treeUri(tree)
        if (parentOf(from) != parentOf(to)) throw WaslException.Io("Renames stay in one folder")
        if (find(t, to) != null) throw WaslException.Io("${nameOf(to)} exists")
        val d = find(t, from) ?: throw FileNotFoundException(from)
        DocumentsContract.renameDocument(resolver, docUri(t, d.id), nameOf(to)) ?: throw FileNotFoundException(Strings.get(R.string.engine_saf_cannot_rename, from))
        Unit
    }

    /** The bin folder of one tree (named by a hash of its URI). */
    private fun binDir(tree: String): File {
        val hash = MessageDigest.getInstance("SHA-256").digest(tree.toByteArray()).joinToString("") { "%02x".format(it) }.take(24)
        return File(binRoot, hash).apply { mkdirs() }
    }

    override fun moveToBin(tree: String, rel: String): String = changing(tree) {
        val t = treeUri(tree)
        val d = find(t, rel) ?: throw FileNotFoundException(rel)
        if (d.isDir) throw WaslException.Io("Folders are not binned")
        val location = "${System.currentTimeMillis()}_${counter.incrementAndGet()}_${d.name}"
        val target = File(binDir(tree), location)
        resolver.openInputStream(docUri(t, d.id))?.use { input -> target.outputStream().use { input.copyTo(it, 1 shl 20) } }
            ?: throw FileNotFoundException(rel)
        if (d.mtime > 0) target.setLastModified(d.mtime)
        DocumentsContract.deleteDocument(resolver, docUri(t, d.id))
        location
    }

    override fun restoreFromBin(tree: String, location: String, rel: String) = changing(tree) {
        val source = binFile(tree, location)
        if (find(treeUri(tree), rel) != null) throw WaslException.Io("${nameOf(rel)} exists")
        val staged = File(context.cacheDir, "wasl-restore-${counter.incrementAndGet()}")
        source.copyTo(staged, overwrite = true)
        install(tree, staged.path, rel, source.lastModified())
        source.delete()
        Unit
    }

    override fun purgeFromBin(tree: String, location: String) = guard { binFile(tree, location).delete(); Unit }

    /** A bin location is one file name this class made; anything else is refused. */
    private fun binFile(tree: String, location: String): File {
        if (location.isEmpty() || location.contains('/') || location.contains('\\') || location == "." || location == "..") {
            throw WaslException.Io("Bad bin location")
        }
        return File(binDir(tree), location)
    }
}
