package com.almodhayan.drivesync.ondemand

import android.database.Cursor
import android.database.MatrixCursor
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.Bundle
import android.os.CancellationSignal
import android.os.Handler
import android.os.HandlerThread
import android.os.OperationCanceledException
import android.os.ParcelFileDescriptor
import android.os.storage.StorageManager
import android.provider.DocumentsContract
import android.provider.DocumentsContract.Document
import android.provider.DocumentsContract.Root
import android.provider.DocumentsProvider
import android.util.Log
import android.webkit.MimeTypeMap
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.NameMapper
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.crypto.detailText
import com.almodhayan.drivesync.data.db.AccountEntity
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.DriveNodeEntity
import com.almodhayan.drivesync.drive.DriveApi
import com.almodhayan.drivesync.drive.DriveNodeCache
import com.almodhayan.drivesync.drive.DriveNodeCache.Companion.toNode
import com.almodhayan.drivesync.drive.PatchFileMetadata
import com.kahflabs.wasl.ffi.OnDemandFile
import com.kahflabs.wasl.ffi.OnDemandOrigin
import com.kahflabs.wasl.ffi.VaultItem
import com.kahflabs.wasl.ffi.WaslException
import dagger.hilt.EntryPoint
import dagger.hilt.InstallIn
import dagger.hilt.android.EntryPointAccessors
import dagger.hilt.components.SingletonComponent
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import java.io.FileNotFoundException
import java.io.IOException
import java.io.RandomAccessFile
import java.util.concurrent.ConcurrentHashMap

private const val TAG = "OnDemand"
private const val AUTHORITY = OnDemandConstants.AUTHORITY
private const val REFRESH_INTERVAL_MS = 60_000L

/**
 * Storage Access Framework provider that exposes every signed-in Google account as a root, so the
 * system Files app and any SAF-aware app (readers, players, editors) can browse the whole Drive
 * without downloading it. Listings come from the Drive metadata index ([DriveNodeCache] /
 * `drive_nodes`); files are read through the engine's on-demand reader ([EngineReader],
 * [EngineReadCallback]): a seekable read-only descriptor whose reads fetch only the blocks they
 * touch, kept in the engine's block cache. Edits of plain Drive files go through a scratch copy
 * written back on close.
 *
 * An encrypted pair's Drive folder shows its vault (v1 or v2) decrypted while the vault is
 * unlocked on this phone ([VaultDocuments]); its files read through the same reader and are
 * read-only here.
 *
 * Dependencies: a DocumentsProvider is created BEFORE Application.onCreate, so Hilt is not ready in
 * [onCreate]. Everything is resolved lazily on first use through an @EntryPoint.
 *
 * Every entry point is synchronous by contract, so suspend work runs under `runBlocking(Dispatchers.IO)`
 * (see [blocking]); the calling binder thread waits, which is exactly what the framework expects.
 */
class DriveDocumentsProvider : DocumentsProvider() {

    @EntryPoint
    @InstallIn(SingletonComponent::class)
    interface OnDemandEntryPoint {
        fun api(): DriveApi
        fun nodeCache(): DriveNodeCache
        fun db(): Db
        fun cache(): OnDemandCache
        fun fetcher(): ContentFetcher
        fun pins(): PinManager
        fun engineReader(): EngineReader
        fun vaultDocs(): VaultDocuments
    }

    private val deps: OnDemandEntryPoint by lazy {
        EntryPointAccessors.fromApplication(context!!.applicationContext, OnDemandEntryPoint::class.java)
    }

    /** Background work that outlives a single call: delta refreshes and post-close uploads. */
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val lastRefresh = ConcurrentHashMap<String, Long>()
    private val refreshing: MutableSet<String> = ConcurrentHashMap.newKeySet()
    private val pendingNotify = ConcurrentHashMap<String, MutableSet<String>>()

    /** Handler for write-session close callbacks (must not be the main looper: the close does I/O bookkeeping). */
    private val ioThread: HandlerThread by lazy { HandlerThread("OnDemand-io").apply { start() } }
    private val ioHandler: Handler by lazy { Handler(ioThread.looper) }

    override fun onCreate(): Boolean = true   // No Hilt here: the Application has not been created yet.

    // ---------------------------------------------------------------- roots

    override fun queryRoots(projection: Array<String>?): Cursor = guard {
        val cursor = MatrixCursor(projection ?: DEFAULT_ROOT_PROJECTION)
        val accounts = blocking { deps.db().accounts().getAll() }
        for (a in accounts) {
            val row = cursor.newRow()
            row.add(Root.COLUMN_ROOT_ID, OnDemandConstants.rootId(a.id))
            row.add(Root.COLUMN_DOCUMENT_ID, OnDemandConstants.documentId(a.id, a.rootFolderId ?: DocumentIds.ROOT_ALIAS))
            // "Wasl", not "Google Drive": Google's own app and the old Drive Sync app list roots of
            // that name too, and the Files app showed two identical ones (E2E 2026-10).
            row.add(Root.COLUMN_TITLE, context?.getString(R.string.app_name) ?: "Wasl")
            row.add(Root.COLUMN_SUMMARY, a.id)
            row.add(Root.COLUMN_FLAGS, Root.FLAG_SUPPORTS_CREATE or Root.FLAG_SUPPORTS_SEARCH or Root.FLAG_SUPPORTS_IS_CHILD)
            row.add(Root.COLUMN_ICON, R.mipmap.ic_launcher)
            row.add(Root.COLUMN_MIME_TYPES, "*/*")
            val limit = a.storageLimit
            if (limit != null && limit > 0) {
                row.add(Root.COLUMN_CAPACITY_BYTES, limit)
                row.add(Root.COLUMN_AVAILABLE_BYTES, (limit - (a.storageUsed ?: 0L)).coerceAtLeast(0L))
            }
        }
        cursor
    }

    // ---------------------------------------------------------------- documents

    override fun queryDocument(documentId: String, projection: Array<String>?): Cursor = guard {
        val id = DocumentIds.parseOrThrow(documentId)
        val cursor = MatrixCursor(projection ?: DEFAULT_DOC_PROJECTION)
        blocking {
            val acct = account(id.accountId)
            val vaultRef = VaultDocIds.parse(id.fileId)
            when {
                vaultRef != null -> {
                    val (folder, tree) = vaultOrThrow(acct, vaultRef.folderId)
                    addVaultRow(cursor, acct.id, folder, vaultItemOrThrow(folder, tree, vaultRef.itemId), isOnline())
                }
                isRoot(acct, id.fileId) -> addRootRow(cursor, acct)
                id.fileId == DocumentIds.SHARED_WITH_ME -> addSharedRow(cursor, acct)
                else -> {
                    val online = isOnline()
                    val doc = docOrThrow(acct, id.fileId)
                    addRow(cursor, acct.id, doc, online, placeOf(acct, doc.fileId, deps.vaultDocs().folders(acct.id).keys, online))
                }
            }
        }
        cursor
    }

    override fun queryChildDocuments(parentDocumentId: String, projection: Array<String>?, sortOrder: String?): Cursor = guard {
        val id = DocumentIds.parseOrThrow(parentDocumentId)
        val cursor = MatrixCursor(projection ?: DEFAULT_DOC_PROJECTION)
        // Re-queried by the Files app whenever we notifyChange() this uri (after a background refresh, an upload, ...).
        cursor.setNotificationUri(context!!.contentResolver, DocumentsContract.buildChildDocumentsUri(AUTHORITY, parentDocumentId))
        val loading = blocking {
            val acct = account(id.accountId)
            val db = deps.db()
            val online = isOnline()
            // Inside a vault: its decrypted listing, read-only.
            VaultDocIds.parse(id.fileId)?.let { ref ->
                val (folder, tree) = vaultOrThrow(acct, ref.folderId)
                val dir = vaultItemOrThrow(folder, tree, ref.itemId)
                if (!dir.isDir) throw FileNotFoundException(Strings.get(R.string.ondemand_is_a_folder, dir.name))
                for (item in sortVault(tree.children(dir.path))) addVaultRow(cursor, acct.id, folder, item, online)
                return@blocking false
            }
            val vaultFolders = deps.vaultDocs().folders(acct.id)
            // An encrypted pair's folder: its vault, decrypted, while the vault is unlocked here.
            vaultFolders[id.fileId]?.takeIf { deps.vaultDocs().unlocked(it) }?.let { folder ->
                val tree = deps.vaultDocs().tree(folder)
                for (item in sortVault(tree.children(""))) addVaultRow(cursor, acct.id, folder, item, online)
                return@blocking false
            }
            // The listed items' parents, so placing them in or out of a vault needs no lookups.
            val parents = HashMap<String, String?>()
            val docs: List<MappedDoc> = when {
                id.fileId == DocumentIds.SHARED_WITH_ME -> sharedWithMe(acct)
                db.nodes().count(acct.id) == 0 -> {
                    // First use, before the account-wide crawl exists: one cheap folder listing straight
                    // from Drive. The crawl is kicked off below and later listings use the index.
                    if (!online) throw FileNotFoundException(Strings.get(R.string.ondemand_not_indexed))
                    val folderId = if (isRoot(acct, id.fileId)) DocumentIds.ROOT_ALIAS else id.fileId
                    val children = deps.api().listChildren(acct.id, folderId).map { it.toNode(acct.id) }
                    children.forEach { parents[it.id] = it.parentId }
                    children.mapNotNull { DocMapper.map(it) { null } }
                }
                else -> {
                    val folderId = if (isRoot(acct, id.fileId)) (acct.rootFolderId ?: DocumentIds.ROOT_ALIAS) else id.fileId
                    // The DAO query is indexed on (accountId, parentId) and already excludes trashed rows;
                    // DriveNodeCache.snapshot() would read the whole account table for every folder open.
                    val children = db.nodes().children(acct.id, folderId)
                    children.forEach { parents[it.id] = it.parentId }
                    val resolve = db.nodes().shortcutResolver(acct.id, children)
                    children.mapNotNull { DocMapper.map(it, resolve) }
                }
            }
            if (isRoot(acct, id.fileId)) addSharedRow(cursor, acct)   // virtual folder first
            val memo = HashMap<String, VaultAncestry.Place>()
            for (d in sortForListing(docs)) addRow(cursor, acct.id, d, online, placeOf(acct, d.fileId, vaultFolders.keys, online, memo, parents))
            maybeRefresh(acct.id, parentDocumentId, online)
        }
        if (loading) cursor.extras = Bundle().apply { putBoolean(DocumentsContract.EXTRA_LOADING, true) }
        cursor
    }

    override fun querySearchDocuments(rootId: String, query: String, projection: Array<String>?): Cursor = guard {
        val cursor = MatrixCursor(projection ?: DEFAULT_DOC_PROJECTION)
        val q = query.trim()
        if (q.isNotEmpty()) blocking {
            val acct = account(rootId)   // root id == account id by convention
            val db = deps.db()
            val online = isOnline()
            val hits = db.nodes().search(acct.id, q)
            val resolve = db.nodes().shortcutResolver(acct.id, hits)
            val docs = hits.mapNotNull { DocMapper.map(it, resolve) }
            val vaultFolders = deps.vaultDocs().folders(acct.id).keys
            val parents = hits.associate { it.id to it.parentId }
            val memo = HashMap<String, VaultAncestry.Place>()
            for (d in sortForListing(docs)) addRow(cursor, acct.id, d, online, placeOf(acct, d.fileId, vaultFolders, online, memo, parents))
        }
        cursor
    }

    override fun getDocumentType(documentId: String): String = guard {
        val id = DocumentIds.parseOrThrow(documentId)
        blocking {
            val acct = account(id.accountId)
            val vaultRef = VaultDocIds.parse(id.fileId)
            when {
                vaultRef != null -> {
                    val (folder, tree) = vaultOrThrow(acct, vaultRef.folderId)
                    vaultItemOrThrow(folder, tree, vaultRef.itemId).let { if (it.isDir) Document.MIME_TYPE_DIR else mimeFor(it.name) }
                }
                isRoot(acct, id.fileId) || id.fileId == DocumentIds.SHARED_WITH_ME -> Document.MIME_TYPE_DIR
                else -> docOrThrow(acct, id.fileId).let { if (it.isFolder) Document.MIME_TYPE_DIR else it.mimeType }
            }
        }
    }

    override fun isChildDocument(parentDocumentId: String, documentId: String): Boolean = guard {
        val p = DocumentIds.parseOrThrow(parentDocumentId)
        val d = DocumentIds.parseOrThrow(documentId)
        if (p.accountId != d.accountId) return@guard false
        blocking {
            val acct = account(p.accountId)
            val pRef = VaultDocIds.parse(p.fileId)
            val dRef = VaultDocIds.parse(d.fileId) ?: return@blocking pRef == null && isDriveChild(acct, p.fileId, d.fileId)
            if (pRef == null) {
                // A vault item is below its vault folder, and so below everything above that folder.
                return@blocking p.fileId == dRef.folderId || isDriveChild(acct, p.fileId, dRef.folderId)
            }
            if (pRef.folderId != dRef.folderId) return@blocking false
            val (_, tree) = vaultOrThrow(acct, dRef.folderId)
            val child = tree.item(dRef.itemId) ?: return@blocking false
            val parent = tree.item(pRef.itemId) ?: return@blocking false
            tree.isBelow(child, parent)
        }
    }

    /** Whether the Drive item [fileId] is below the Drive folder [parentId] in the index. */
    private suspend fun isDriveChild(acct: AccountEntity, parentId: String, fileId: String): Boolean {
        val nodes = deps.db().nodes()
        if (isRoot(acct, parentId)) return true   // everything of the account lives under its root (shared items via the virtual folder)
        if (fileId == DocumentIds.SHARED_WITH_ME) return false
        var cur = nodes.get(acct.id, fileId) ?: return false
        repeat(256) {
            val pid = cur.parentId
                ?: return parentId == DocumentIds.SHARED_WITH_ME && cur.driveId == null && cur.id != acct.rootFolderId
            if (pid == parentId) return true
            if (isRoot(acct, pid)) return false   // reached My Drive without meeting the parent
            cur = nodes.get(acct.id, pid) ?: return false
        }
        return false
    }

    // ---------------------------------------------------------------- open

    override fun openDocument(documentId: String, mode: String, signal: CancellationSignal?): ParcelFileDescriptor = guard {
        val id = DocumentIds.parseOrThrow(documentId)
        val pfdMode = ParcelFileDescriptor.parseMode(mode)
        val write = (pfdMode and ParcelFileDescriptor.MODE_WRITE_ONLY) != 0
        val truncate = (pfdMode and ParcelFileDescriptor.MODE_TRUNCATE) != 0
        blocking(signal) {
            val acct = account(id.accountId)
            VaultDocIds.parse(id.fileId)?.let { ref -> return@blocking openVaultItem(acct, ref, write) }
            if (isRoot(acct, id.fileId) || id.fileId == DocumentIds.SHARED_WITH_ME) throw FileNotFoundException(Strings.get(R.string.ondemand_folders_not_files))
            val doc = docOrThrow(acct, id.fileId)
            if (doc.isFolder) throw FileNotFoundException(Strings.get(R.string.ondemand_is_a_folder, doc.displayName))
            // An encrypted pair's files, as they are on Drive (the vault locked): read-only.
            if (write && !placeOf(acct, doc.fileId).writable) throw FileNotFoundException(Strings.get(R.string.ondemand_vault_read_only, doc.displayName))
            if (write) openForWrite(acct, doc, pfdMode, truncate) else openForRead(acct, doc)
        }
    }

    private suspend fun openForRead(acct: AccountEntity, doc: MappedDoc): ParcelFileDescriptor {
        val cache = deps.cache()
        // 1. An edit that has not reached Drive yet is the newest content there is.
        cache.pendingWork(acct.id, doc.fileId)?.let { return ParcelFileDescriptor.open(it, ParcelFileDescriptor.MODE_READ_ONLY) }
        // 2. A complete copy the app keeps (pinned, exported, or an edit it sent).
        cache.fullFile(acct.id, doc.fileId, doc.cacheTag)?.let { return ParcelFileDescriptor.open(it, ParcelFileDescriptor.MODE_READ_ONLY) }
        // 3. Google-native docs have no byte range to read: export the whole document once, then serve it.
        // 4. So do the rare files Drive gives no size for.
        val file = EngineReader.file(doc)
        if (file == null) {
            if (!isOnline()) throw FileNotFoundException(Strings.get(R.string.ondemand_not_offline, doc.displayName))
            val f = deps.fetcher().ensureFull(acct.id, doc, pinned = false)
            return ParcelFileDescriptor.open(f, ParcelFileDescriptor.MODE_READ_ONLY)
        }
        // 5. Everything else reads through the engine: a seek fetches one block, never the file, and
        //    what was read stays in its block cache (offline too, once complete).
        val origin = EngineReader.drive(acct.id)
        if (!isOnline() && !deps.engineReader().reader.isComplete(origin, file)) {
            throw FileNotFoundException(Strings.get(R.string.ondemand_not_offline, doc.displayName))
        }
        return openThroughEngine(origin, file)
    }

    /** A vault item: read-only, through the engine's reader with the vault's keys. */
    private suspend fun openVaultItem(acct: AccountEntity, ref: VaultDocIds.Ref, write: Boolean): ParcelFileDescriptor {
        val (folder, tree) = vaultOrThrow(acct, ref.folderId)
        val item = vaultItemOrThrow(folder, tree, ref.itemId)
        if (item.isDir) throw FileNotFoundException(Strings.get(R.string.ondemand_is_a_folder, item.name))
        if (write) throw FileNotFoundException(Strings.get(R.string.ondemand_vault_read_only, item.name))
        val file = OnDemandFile(item.id, item.version, item.size)
        if (!isOnline() && !deps.engineReader().reader.isComplete(folder.origin, file)) {
            throw FileNotFoundException(Strings.get(R.string.ondemand_not_offline, item.name))
        }
        return openThroughEngine(folder.origin, file)
    }

    /** A seekable, read-only descriptor whose reads go to the engine's on-demand reader. */
    private fun openThroughEngine(origin: OnDemandOrigin, file: OnDemandFile): ParcelFileDescriptor {
        val ctx = context!!
        val sm = ctx.getSystemService(StorageManager::class.java) ?: throw IOException("StorageManager unavailable")
        // One HandlerThread per open file: the proxy callbacks block on the network, and two apps
        // reading different files must not queue behind each other.
        val thread = HandlerThread("OnDemand-proxy-${file.id.takeLast(8)}").apply { start() }
        val callback = EngineReadCallback(deps.engineReader().reader, origin, file) { thread.quitSafely() }
        return try {
            sm.openProxyFileDescriptor(ParcelFileDescriptor.MODE_READ_ONLY, callback, Handler(thread.looper))
        } catch (e: IOException) {
            thread.quitSafely()
            throw e
        }
    }

    private suspend fun openForWrite(acct: AccountEntity, doc: MappedDoc, pfdMode: Int, truncate: Boolean): ParcelFileDescriptor {
        if (doc.readOnly) throw FileNotFoundException(Strings.get(R.string.ondemand_google_doc_read_only, doc.displayName))
        val cache = deps.cache()
        val work = cache.workFile(acct.id, doc.fileId)
        // Under the per-file lock so this never races with an upload that is about to move the
        // previous edit's work copy into the cache (the FD we hand out must stay on a scratch file).
        deps.pins().withFileLock(acct.id, doc.fileId) {
            withContext(Dispatchers.IO) {
                when {
                    truncate -> RandomAccessFile(work, "rw").use { it.setLength(0) }
                    cache.pendingWork(acct.id, doc.fileId) != null -> Unit   // base is the unsent edit: newest content
                    else -> {
                        // The cached copy is keyed by content; never hand it out for writing. Work on a copy.
                        val base = cache.fullFile(acct.id, doc.fileId, doc.cacheTag) ?: run {
                            if (!isOnline()) throw FileNotFoundException(Strings.get(R.string.ondemand_not_offline_no_edit, doc.displayName))
                            deps.fetcher().ensureFull(acct.id, doc, pinned = false)
                        }
                        base.copyTo(work, overwrite = true)
                    }
                }
            }
        }
        // A plain file descriptor on the work copy; the framework tells us (on ioHandler) when the
        // client closed it, and only then does the content go back to Drive.
        val listener = ParcelFileDescriptor.OnCloseListener { error -> onWriteSessionClosed(acct, doc, error) }
        return ParcelFileDescriptor.open(work, pfdMode, ioHandler, listener)
    }

    private fun onWriteSessionClosed(acct: AccountEntity, doc: MappedDoc, error: IOException?) {
        val cache = deps.cache()
        if (error != null) {
            // closeWithError() or the client process died: the content is of unknown completeness.
            // No file name or message in logcat (security review 2026-10).
            Log.w(TAG, "write session ended with an error, local changes discarded: ${error.javaClass.simpleName}")
            cache.discardWork(acct.id, doc.fileId)
            return
        }
        cache.markPending(acct.id, doc.fileId)
        scope.launch {
            val ok = deps.pins().uploadPending(acct.id, doc.fileId)
            if (ok) {
                notifyParent(acct, doc.parentId)
                notifyDocument(OnDemandConstants.documentId(acct.id, doc.fileId))
            } else {
                deps.pins().schedule(context!!)   // PinWorker retries the upload once the device is connected
            }
        }
    }

    // ---------------------------------------------------------------- create / delete / rename / move

    override fun createDocument(parentDocumentId: String, mimeType: String, displayName: String): String = guard {
        val p = DocumentIds.parseOrThrow(parentDocumentId)
        blocking {
            val acct = account(p.accountId)
            if (p.fileId == DocumentIds.SHARED_WITH_ME) throw UnsupportedOperationException(Strings.get(R.string.ondemand_no_create_in_shared))
            // Anything made in or below a vault folder would sit in the vault unencrypted: never.
            if (VaultDocIds.isVaultId(p.fileId) || !placeOf(acct, p.fileId).writable) throw UnsupportedOperationException(Strings.get(R.string.ondemand_vault_no_changes))
            val parentId = if (isRoot(acct, p.fileId)) (acct.rootFolderId ?: DocumentIds.ROOT_ALIAS) else p.fileId
            val api = deps.api()
            val file = if (mimeType == Document.MIME_TYPE_DIR) {
                api.createFolder(acct.id, parentId, displayName)
            } else {
                val temp = deps.cache().newTempFile(acct.id)
                try {
                    withContext(Dispatchers.IO) { temp.createNewFile() }
                    api.upload(acct.id, temp, displayName, parentId, null, System.currentTimeMillis(), mimeType.ifBlank { DocMapper.OCTET_STREAM }).file
                } finally {
                    temp.delete()
                }
            }
            deps.nodeCache().remember(acct.id, file)
            notifyParent(acct, parentId)
            OnDemandConstants.documentId(acct.id, file.id)
        }
    }

    override fun deleteDocument(documentId: String) = guard {
        val d = DocumentIds.parseOrThrow(documentId)
        if (VaultDocIds.isVaultId(d.fileId)) throw UnsupportedOperationException(Strings.get(R.string.ondemand_vault_no_changes))
        blocking {
            val acct = account(d.accountId)
            if (isRoot(acct, d.fileId) || d.fileId == DocumentIds.SHARED_WITH_ME) throw UnsupportedOperationException(Strings.get(R.string.ondemand_folder_no_delete))
            // A vault's own objects (encrypted files, folders, chunks) are the engine's to remove.
            if (placeOf(acct, d.fileId).insideVault) throw UnsupportedOperationException(Strings.get(R.string.ondemand_vault_no_changes))
            val node = deps.db().nodes().get(acct.id, d.fileId)
            deps.api().trash(acct.id, d.fileId)   // Drive trash, never a permanent delete
            deps.nodeCache().forget(acct.id, d.fileId)
            deps.cache().dropFile(acct.id, d.fileId)
            deps.db().pins().delete(acct.id, d.fileId)
            notifyParent(acct, node?.parentId)
        }
    }

    override fun renameDocument(documentId: String, displayName: String): String? = guard {
        val d = DocumentIds.parseOrThrow(documentId)
        if (VaultDocIds.isVaultId(d.fileId)) throw UnsupportedOperationException(Strings.get(R.string.ondemand_vault_no_changes))
        blocking {
            val acct = account(d.accountId)
            if (isRoot(acct, d.fileId) || d.fileId == DocumentIds.SHARED_WITH_ME) throw UnsupportedOperationException(Strings.get(R.string.ondemand_folder_no_rename))
            val doc = docOrThrow(acct, d.fileId)
            // A vault's encrypted names are its own; renaming one breaks it.
            if (placeOf(acct, doc.fileId).insideVault) throw UnsupportedOperationException(Strings.get(R.string.ondemand_vault_no_changes))
            val newName = DocMapper.driveNameForRename(doc, displayName)
            if (newName.isBlank()) throw IllegalArgumentException(Strings.get(R.string.ondemand_name_empty))
            if (newName != doc.driveName) {
                // Same request DriveApi.rename() sends, but patch() returns the updated file for the index.
                val f = deps.api().patch(acct.id, doc.fileId, PatchFileMetadata(name = newName))
                deps.nodeCache().remember(acct.id, f)
                notifyParent(acct, doc.parentId)
                notifyDocument(documentId)
            }
            null   // ids never change on rename
        }
    }

    override fun moveDocument(sourceDocumentId: String, sourceParentDocumentId: String, targetParentDocumentId: String): String = guard {
        val s = DocumentIds.parseOrThrow(sourceDocumentId)
        val sp = DocumentIds.parseOrThrow(sourceParentDocumentId)
        val tp = DocumentIds.parseOrThrow(targetParentDocumentId)
        if (s.accountId != tp.accountId) throw UnsupportedOperationException(Strings.get(R.string.ondemand_no_move_between_accounts))
        if (VaultDocIds.isVaultId(s.fileId) || VaultDocIds.isVaultId(tp.fileId)) throw UnsupportedOperationException(Strings.get(R.string.ondemand_vault_no_changes))
        blocking {
            val acct = account(s.accountId)
            if (isRoot(acct, s.fileId) || s.fileId == DocumentIds.SHARED_WITH_ME) throw UnsupportedOperationException(Strings.get(R.string.ondemand_folder_no_move))
            if (tp.fileId == DocumentIds.SHARED_WITH_ME) throw UnsupportedOperationException(Strings.get(R.string.ondemand_no_move_into_shared))
            // A plain file moved into a vault would sit there unencrypted; a vault's object moved out
            // (or within it) breaks the vault. The vault folder itself may move.
            if (!placeOf(acct, tp.fileId).writable) throw UnsupportedOperationException(Strings.get(R.string.ondemand_vault_no_changes))
            val doc = docOrThrow(acct, s.fileId)
            if (placeOf(acct, doc.fileId).insideVault) throw UnsupportedOperationException(Strings.get(R.string.ondemand_vault_no_changes))
            val from = doc.parentId ?: (if (isRoot(acct, sp.fileId)) (acct.rootFolderId ?: DocumentIds.ROOT_ALIAS) else sp.fileId)
            val to = if (isRoot(acct, tp.fileId)) (acct.rootFolderId ?: DocumentIds.ROOT_ALIAS) else tp.fileId
            if (from != to) {
                // Same request DriveApi.move() sends; patch() hands back the updated file for the index.
                val f = deps.api().patch(acct.id, doc.fileId, PatchFileMetadata(), addParents = to, removeParents = from)
                deps.nodeCache().remember(acct.id, f)
                notifyParent(acct, from)
                notifyParent(acct, to)
            }
            OnDemandConstants.documentId(acct.id, doc.fileId)
        }
    }

    // ---------------------------------------------------------------- rows

    /** [place]: where the item is relative to the vault folders; in or maybe in a vault, it is read-only. */
    private fun addRow(cursor: MatrixCursor, accountId: String, doc: MappedDoc, online: Boolean, place: VaultAncestry.Place) {
        val cache = deps.cache()
        val local: Long? = if (doc.isFolder) null else cache.pendingWork(accountId, doc.fileId)?.length() ?: cache.fullFileSize(accountId, doc.fileId, doc.cacheTag)
            ?: engineComplete(accountId, doc)
        var flags = when {
            place.insideVault -> 0
            // An encrypted pair's folder: nothing is made in it from here.
            place == VaultAncestry.Place.VAULT_FOLDER -> Document.FLAG_SUPPORTS_DELETE or Document.FLAG_SUPPORTS_RENAME
            doc.kind == DocKind.FOLDER -> Document.FLAG_DIR_SUPPORTS_CREATE or Document.FLAG_SUPPORTS_DELETE or Document.FLAG_SUPPORTS_RENAME
            doc.kind == DocKind.FILE -> Document.FLAG_SUPPORTS_WRITE or Document.FLAG_SUPPORTS_DELETE or Document.FLAG_SUPPORTS_RENAME or Document.FLAG_SUPPORTS_MOVE
            // Exported snapshots: content is read-only, metadata operations are fine.
            else -> Document.FLAG_SUPPORTS_DELETE or Document.FLAG_SUPPORTS_RENAME or Document.FLAG_SUPPORTS_MOVE
        }
        // Offline and not cached: the Files app greys the item out instead of failing on tap.
        if (!doc.isFolder && !online && local == null) flags = flags or Document.FLAG_PARTIAL
        val row = cursor.newRow()
        row.add(Document.COLUMN_DOCUMENT_ID, OnDemandConstants.documentId(accountId, doc.fileId))
        row.add(Document.COLUMN_DISPLAY_NAME, doc.displayName)
        row.add(Document.COLUMN_MIME_TYPE, if (doc.isFolder) Document.MIME_TYPE_DIR else doc.mimeType)
        row.add(Document.COLUMN_FLAGS, flags)
        row.add(Document.COLUMN_SIZE, local ?: doc.size)
        row.add(Document.COLUMN_LAST_MODIFIED, doc.lastModified)
        row.add(Document.COLUMN_SUMMARY, if (local != null) Strings.get(R.string.ondemand_available_offline) else null)
    }

    private fun addRootRow(cursor: MatrixCursor, acct: AccountEntity) {
        val row = cursor.newRow()
        row.add(Document.COLUMN_DOCUMENT_ID, OnDemandConstants.documentId(acct.id, acct.rootFolderId ?: DocumentIds.ROOT_ALIAS))
        row.add(Document.COLUMN_DISPLAY_NAME, Strings.get(R.string.drive_my_drive))
        row.add(Document.COLUMN_MIME_TYPE, Document.MIME_TYPE_DIR)
        row.add(Document.COLUMN_FLAGS, Document.FLAG_DIR_SUPPORTS_CREATE)
        row.add(Document.COLUMN_SUMMARY, acct.id)
    }

    private fun addSharedRow(cursor: MatrixCursor, acct: AccountEntity) {
        val row = cursor.newRow()
        row.add(Document.COLUMN_DOCUMENT_ID, OnDemandConstants.documentId(acct.id, DocumentIds.SHARED_WITH_ME))
        row.add(Document.COLUMN_DISPLAY_NAME, Strings.get(R.string.ondemand_shared_with_me))
        row.add(Document.COLUMN_MIME_TYPE, Document.MIME_TYPE_DIR)
        row.add(Document.COLUMN_FLAGS, 0)
    }

    /** The file's size when the engine's block cache holds all of it (it then opens offline). */
    private fun engineComplete(accountId: String, doc: MappedDoc): Long? {
        val file = EngineReader.file(doc) ?: return null
        return runCatching { deps.engineReader().reader.isComplete(EngineReader.drive(accountId), file) }.getOrDefault(false)
            .let { if (it) doc.size else null }
    }

    /** A vault's file or folder: read-only (no write, create, rename, move or delete from here). */
    private fun addVaultRow(cursor: MatrixCursor, accountId: String, folder: VaultDocuments.VaultFolder, item: VaultItem, online: Boolean) {
        val file = if (item.isDir) null else OnDemandFile(item.id, item.version, item.size)
        val local = file != null && runCatching { deps.engineReader().reader.isComplete(folder.origin, file) }.getOrDefault(false)
        val name = NameMapper.toLocalName(item.name)
        val row = cursor.newRow()
        row.add(Document.COLUMN_DOCUMENT_ID, OnDemandConstants.documentId(accountId, VaultDocIds.encode(folder.folderId, item.id)))
        row.add(Document.COLUMN_DISPLAY_NAME, name)
        row.add(Document.COLUMN_MIME_TYPE, if (item.isDir) Document.MIME_TYPE_DIR else mimeFor(item.name))
        row.add(Document.COLUMN_FLAGS, if (file != null && !online && !local) Document.FLAG_PARTIAL else 0)
        row.add(Document.COLUMN_SIZE, file?.size?.toLong())
        row.add(Document.COLUMN_LAST_MODIFIED, item.mtime.takeIf { it > 0 })
        row.add(Document.COLUMN_SUMMARY, if (local) Strings.get(R.string.ondemand_available_offline) else null)
    }

    private fun sortVault(items: List<VaultItem>): List<VaultItem> =
        items.sortedWith(compareBy<VaultItem> { !it.isDir }.thenBy(String.CASE_INSENSITIVE_ORDER) { it.name })

    private fun mimeFor(name: String): String {
        val ext = name.substringAfterLast('.', "").lowercase()
        return ext.takeIf { it.isNotEmpty() }?.let { MimeTypeMap.getSingleton().getMimeTypeFromExtension(it) } ?: DocMapper.OCTET_STREAM
    }

    private fun sortForListing(docs: List<MappedDoc>): List<MappedDoc> =
        docs.sortedWith(compareBy<MappedDoc> { !it.isFolder }.thenBy(String.CASE_INSENSITIVE_ORDER) { it.displayName })

    /** Items shared by other people have no parent; this virtual folder is the only way to reach them. */
    private suspend fun sharedWithMe(acct: AccountEntity): List<MappedDoc> {
        val nodes = deps.db().nodes()
        val shared = nodes.allForAccount(acct.id).filter { it.parentId == null && it.driveId == null && it.id != acct.rootFolderId }
        val resolve = nodes.shortcutResolver(acct.id, shared)
        return shared.mapNotNull { DocMapper.map(it, resolve) }
    }

    // ---------------------------------------------------------------- lookups

    private suspend fun account(accountId: String): AccountEntity =
        deps.db().accounts().get(accountId) ?: throw FileNotFoundException(Strings.get(R.string.ondemand_account_not_signed_in, accountId))

    private fun isRoot(acct: AccountEntity, fileId: String): Boolean =
        DocumentIds.isRootAlias(fileId) || (acct.rootFolderId != null && fileId == acct.rootFolderId)

    /** Node from the index, falling back to one metadata request when online (shortcut targets, shared items not crawled yet). */
    private suspend fun nodeOrThrow(acct: AccountEntity, fileId: String): DriveNodeEntity {
        val nodes = deps.db().nodes()
        nodes.get(acct.id, fileId)?.let { if (it.trashed) throw FileNotFoundException(Strings.get(R.string.ondemand_in_drive_trash, it.name)); return it }
        if (!isOnline()) throw FileNotFoundException(Strings.get(R.string.ondemand_not_indexed_offline))
        val f = deps.api().getFile(acct.id, fileId)
        if (f.trashed) throw FileNotFoundException(Strings.get(R.string.ondemand_in_drive_trash, f.name))
        deps.nodeCache().remember(acct.id, f)
        return f.toNode(acct.id)
    }

    /** Resolves shortcuts to their target so callers always deal with the real file. */
    private suspend fun docOrThrow(acct: AccountEntity, fileId: String): MappedDoc {
        val nodes = deps.db().nodes()
        val node = nodeOrThrow(acct, fileId)
        val doc = DocMapper.map(node, nodes.shortcutResolver(acct.id, listOf(node)))
            ?: throw FileNotFoundException(Strings.get(R.string.ondemand_no_export_format, node.name))
        if (doc.fileId == node.id || !node.isShortcut) return doc
        // Shortcut whose target was not in the index: fetch the target itself.
        val target = nodeOrThrow(acct, doc.fileId)
        return DocMapper.map(target, nodes.shortcutResolver(acct.id, listOf(target)))
            ?: throw FileNotFoundException(Strings.get(R.string.ondemand_shortcut_unopenable, node.name))
    }

    /** The encrypted pair's vault folder [folderId] and its decrypted listing; fails while the vault is locked. */
    private suspend fun vaultOrThrow(acct: AccountEntity, folderId: String): Pair<VaultDocuments.VaultFolder, VaultTree> {
        val docs = deps.vaultDocs()
        val folder = docs.folder(acct.id, folderId) ?: throw FileNotFoundException(Strings.get(R.string.ondemand_not_found_on_drive))
        if (!docs.unlocked(folder)) throw FileNotFoundException(Strings.get(R.string.ondemand_vault_locked, folder.name))
        return folder to docs.tree(folder)
    }

    /**
     * Where a Drive item is relative to the account's vault folders ([VaultAncestry]): its parents
     * from [parents] (a listing's own nodes), the index, or one metadata request when online; an
     * ancestry that cannot be followed counts as inside a vault.
     */
    private suspend fun placeOf(
        acct: AccountEntity,
        fileId: String,
        vaultFolders: Set<String>? = null,
        online: Boolean? = null,
        memo: MutableMap<String, VaultAncestry.Place> = HashMap(),
        parents: Map<String, String?> = emptyMap(),
    ): VaultAncestry.Place {
        val folders = vaultFolders ?: deps.vaultDocs().folders(acct.id).keys
        if (folders.isEmpty()) return VaultAncestry.Place.OUTSIDE
        val canAsk = online ?: isOnline()
        val nodes = deps.db().nodes()
        return VaultAncestry.place(fileId, folders, { isRoot(acct, it) || it == DocumentIds.SHARED_WITH_ME }, memo) { id ->
            if (parents.containsKey(id)) return@place VaultAncestry.Parent.Known(parents[id])
            val node = nodes.get(acct.id, id) ?: if (canAsk) fetchNode(acct, id) else null
            node?.let { VaultAncestry.Parent.Known(it.parentId) } ?: VaultAncestry.Parent.Unknown
        }
    }

    private suspend fun fetchNode(acct: AccountEntity, id: String): DriveNodeEntity? = try {
        deps.api().getFile(acct.id, id).also { deps.nodeCache().remember(acct.id, it) }.toNode(acct.id)
    } catch (e: CancellationException) {
        throw e
    } catch (e: Exception) {
        null
    }

    /** An item of the listing; a listing older than the item (added since) is read again once. */
    private suspend fun vaultItemOrThrow(folder: VaultDocuments.VaultFolder, tree: VaultTree, itemId: String): VaultItem =
        tree.item(itemId) ?: deps.vaultDocs().tree(folder, fresh = true).item(itemId)
            ?: throw FileNotFoundException(Strings.get(R.string.ondemand_not_found_on_drive))

    // ---------------------------------------------------------------- refresh + notifications

    /**
     * Applies Drive's change delta in the background at most once a minute per account. Returns
     * true while a refresh is running so the listing carries EXTRA_LOADING; when it finishes every
     * folder queried meanwhile is notified and the Files app re-queries.
     */
    private fun maybeRefresh(accountId: String, notifyDocId: String, online: Boolean): Boolean {
        if (!online) return false
        if (refreshing.contains(accountId)) {
            pendingNotify.getOrPut(accountId) { ConcurrentHashMap.newKeySet() }.add(notifyDocId)
            return true
        }
        if (System.currentTimeMillis() - (lastRefresh[accountId] ?: 0L) < REFRESH_INTERVAL_MS) return false
        if (!refreshing.add(accountId)) return true
        pendingNotify.getOrPut(accountId) { ConcurrentHashMap.newKeySet() }.add(notifyDocId)
        scope.launch {
            try {
                val changed = deps.nodeCache().refresh(accountId)
                if (changed) Log.i(TAG, "Drive index refreshed for $accountId")
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                Log.w(TAG, "Drive index refresh failed for $accountId: ${AppError.wrap(t).message}")
            } finally {
                lastRefresh[accountId] = System.currentTimeMillis()
                refreshing.remove(accountId)
                // Always notify: the Files app shows its progress bar until a re-query without EXTRA_LOADING lands.
                pendingNotify.remove(accountId)?.forEach { notifyChildren(it) }
            }
        }
        return true
    }

    private fun notifyChildren(parentDocumentId: String) {
        runCatching { context!!.contentResolver.notifyChange(DocumentsContract.buildChildDocumentsUri(AUTHORITY, parentDocumentId), null) }
    }

    private fun notifyDocument(documentId: String) {
        runCatching { context!!.contentResolver.notifyChange(DocumentsContract.buildDocumentUri(AUTHORITY, documentId), null) }
    }

    /** Notifies a folder listing; the root may have been queried under its alias or its real id, so both are notified. */
    private fun notifyParent(acct: AccountEntity, parentId: String?) {
        if (parentId == null) { notifyChildren(OnDemandConstants.documentId(acct.id, DocumentIds.SHARED_WITH_ME)); return }
        notifyChildren(OnDemandConstants.documentId(acct.id, parentId))
        if (isRoot(acct, parentId)) {
            notifyChildren(OnDemandConstants.documentId(acct.id, DocumentIds.ROOT_ALIAS))
            acct.rootFolderId?.let { notifyChildren(OnDemandConstants.documentId(acct.id, it)) }
        }
    }

    private fun isOnline(): Boolean {
        val cm = context!!.getSystemService(ConnectivityManager::class.java) ?: return false
        val caps = cm.activeNetwork?.let { cm.getNetworkCapabilities(it) } ?: return false
        return caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
    }

    // ---------------------------------------------------------------- plumbing

    /**
     * Runs suspend work synchronously on the IO dispatcher; a CancellationSignal from the caller
     * cancels the coroutine. DocumentsProvider methods are synchronous by contract and run on
     * binder threads, so blocking the caller is expected here.
     */
    private fun <T> blocking(signal: CancellationSignal? = null, block: suspend () -> T): T = runBlocking(Dispatchers.IO) {
        val job = coroutineContext[Job]
        signal?.setOnCancelListener { job?.cancel() }
        try {
            block()
        } finally {
            signal?.setOnCancelListener(null)
        }
    }

    /**
     * Only a handful of exception types survive the binder boundary (FileNotFoundException,
     * IllegalArgument/IllegalState/UnsupportedOperation/Security/OperationCanceled); anything else
     * kills the provider process. Every entry point goes through here.
     */
    private inline fun <T> guard(block: () -> T): T = try {
        block()
    } catch (e: FileNotFoundException) {
        throw e
    } catch (e: UnsupportedOperationException) {
        throw e
    } catch (e: IllegalArgumentException) {
        throw e
    } catch (e: IllegalStateException) {
        throw e
    } catch (e: SecurityException) {
        throw e
    } catch (e: OperationCanceledException) {
        throw e
    } catch (t: Throwable) {
        throw translate(t)
    }

    private fun translate(t: Throwable): Exception {
        if (t is WaslException) {
            // From the engine's on-demand reader (a vault listing).
            // Logcat gets the error class only: the detail text can carry a file's name (S1-17, L4).
            Log.w(TAG, "provider operation failed: ${t.javaClass.simpleName}")
            return when (t) {
                is WaslException.NotFound, is WaslException.Locked -> FileNotFoundException(t.detailText())
                is WaslException.Auth -> IllegalStateException(Strings.get(R.string.ondemand_auth_needed))
                is WaslException.Network -> IllegalStateException(Strings.get(R.string.ondemand_offline))
                else -> IllegalStateException(t.detailText())
            }
        }
        val err = AppError.wrap(t)
        Log.w(TAG, "provider operation failed: ${err.message}", if (err is AppError.Unknown) t else null)
        return when (err) {
            is AppError.NotFound -> FileNotFoundException(err.message ?: Strings.get(R.string.ondemand_not_found_on_drive))
            is AppError.Cancelled -> OperationCanceledException("Cancelled")
            is AppError.Auth -> IllegalStateException(Strings.get(R.string.ondemand_auth_needed))
            is AppError.Network -> IllegalStateException(Strings.get(R.string.ondemand_offline))
            else -> IllegalStateException(err.userHint)
        }
    }

    private companion object {
        val DEFAULT_ROOT_PROJECTION = arrayOf(
            Root.COLUMN_ROOT_ID, Root.COLUMN_DOCUMENT_ID, Root.COLUMN_TITLE, Root.COLUMN_SUMMARY, Root.COLUMN_FLAGS,
            Root.COLUMN_ICON, Root.COLUMN_MIME_TYPES, Root.COLUMN_AVAILABLE_BYTES, Root.COLUMN_CAPACITY_BYTES,
        )
        val DEFAULT_DOC_PROJECTION = arrayOf(
            Document.COLUMN_DOCUMENT_ID, Document.COLUMN_DISPLAY_NAME, Document.COLUMN_MIME_TYPE, Document.COLUMN_FLAGS,
            Document.COLUMN_SIZE, Document.COLUMN_LAST_MODIFIED, Document.COLUMN_SUMMARY,
        )
    }
}
