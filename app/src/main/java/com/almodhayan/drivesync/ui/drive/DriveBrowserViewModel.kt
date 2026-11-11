package com.almodhayan.drivesync.ui.drive

import android.content.Context
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import androidx.annotation.StringRes
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.DriveNodeEntity
import com.almodhayan.drivesync.data.db.PinEntity
import com.almodhayan.drivesync.drive.DocsExport
import com.almodhayan.drivesync.drive.DriveApi
import com.almodhayan.drivesync.drive.DriveNodeCache
import com.almodhayan.drivesync.drive.DriveNodeCache.Companion.toNode
import com.almodhayan.drivesync.drive.FOLDER_MIME
import com.almodhayan.drivesync.drive.GOOGLE_APPS_PREFIX
import com.almodhayan.drivesync.drive.SharedDrive
import com.almodhayan.drivesync.fs.LocalFs
import com.almodhayan.drivesync.ondemand.PinRequests
import dagger.hilt.android.lifecycle.HiltViewModel
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import javax.inject.Inject

@HiltViewModel
class DriveBrowserViewModel @Inject constructor(
    handle: SavedStateHandle,
    @ApplicationContext private val context: Context,
    private val db: Db,
    private val api: DriveApi,
    private val cache: DriveNodeCache,
    private val pinRequests: PinRequests,
) : ViewModel() {

    val accountId: String = handle.get<String>("accountId") ?: ""
    private val startFolderId: String? = handle.get<String>("folderId")?.takeIf { it.isNotBlank() }

    /** The places the segmented button switches between. Shared drives only shows when the account has any. */
    enum class Place(@StringRes val labelRes: Int) {
        MyDrive(R.string.drive_place_my_drive), SharedWithMe(R.string.drive_place_shared_with_me), SharedDrives(R.string.drive_place_shared_drives);

        val label: String get() = Strings.get(labelRes)
    }

    /** One breadcrumb level. [driveId] is the shared drive the folder lives in, for listing it from the API. */
    data class Crumb(val id: String, val name: String, val driveId: String? = null)

    data class Ui(
        val rootId: String? = null,
        val drives: List<SharedDrive> = emptyList(),
        val place: Place = Place.MyDrive,
        val crumbs: List<Crumb> = emptyList(),
        val items: List<DriveNodeEntity> = emptyList(),
        val loading: Boolean = true,
        val indexing: Boolean = false,
        val error: String? = null,
        val query: String = "",
        val results: List<DriveNodeEntity>? = null,     // non-null while a search is active
        val pinned: Set<String> = emptySet(),
        val busy: String? = null,                         // progress text for downloads
    ) {
        val places: List<Place> get() = if (drives.isEmpty()) listOf(Place.MyDrive, Place.SharedWithMe) else Place.entries
        /** At the list of shared drives (not inside one). */
        val atDriveList: Boolean get() = crumbs.lastOrNull()?.id == DRIVES_ROOT
    }

    /** A short confirmation for the snackbar; [undo] for reversible actions. */
    class Note(val text: String, val undo: (() -> Unit)? = null)

    /** What failed, why, and the one fix. */
    class Failure(val title: String, val hint: String, val retry: (() -> Unit)? = null)

    private val _ui = MutableStateFlow(Ui())
    val ui: StateFlow<Ui> = _ui

    private val _notes = MutableSharedFlow<Note>(extraBufferCapacity = 8)
    val notes: SharedFlow<Note> = _notes

    private val _failure = MutableStateFlow<Failure?>(null)
    val failure: StateFlow<Failure?> = _failure
    fun dismissFailure() { _failure.value = null }

    private var loadJob: Job? = null
    private var searchJob: Job? = null

    init {
        viewModelScope.launch {
            try {
                val acct = db.accounts().get(accountId) ?: throw AppError.NotFound(Strings.get(R.string.drive_account_not_connected, accountId))
                _ui.update { it.copy(indexing = acct.lastCrawlAt == null) }
                val rootId = acct.rootFolderId ?: api.rootId(accountId)
                val drives = runCatching { api.listSharedDrives(accountId) }.getOrDefault(emptyList())
                _ui.update { it.copy(rootId = rootId, drives = drives) }
                val start = startFolderId
                if (start == null || start == rootId) openMyDrive() else openById(start, rootId, drives)
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _ui.update { it.copy(loading = false, error = AppError.wrap(t).userHint) }
            }
        }
        viewModelScope.launch {
            runCatching { cache.refresh(accountId) }
            val wasIndexing = _ui.value.indexing
            _ui.update { it.copy(indexing = false) }
            if (wasIndexing && _ui.value.crumbs.isNotEmpty()) load()
        }
        viewModelScope.launch {
            db.pins().observeAll().collect { pins -> _ui.update { u -> u.copy(pinned = pins.filter { it.accountId == accountId }.map { it.fileId }.toSet()) } }
        }
    }

    /** Opens a folder by id, rebuilding the breadcrumb from the cache (or a single crumb from the API). */
    private suspend fun openById(folderId: String, rootId: String, drives: List<SharedDrive>) {
        val chain = ArrayList<Crumb>()
        var cur = db.nodes().get(accountId, folderId)
        var driveId: String? = cur?.driveId
        var reachedRoot = false
        var drive: SharedDrive? = null
        var guard = 0
        while (cur != null && guard++ < 64) {
            chain += Crumb(cur.id, cur.name, cur.driveId)
            val p = cur.parentId ?: break
            if (p == rootId) { reachedRoot = true; break }
            val parent = db.nodes().get(accountId, p)
            if (parent == null) { drive = drives.firstOrNull { it.id == p }; break }
            cur = parent
        }
        if (chain.isEmpty()) {
            val f = runCatching { api.getFile(accountId, folderId) }.getOrNull()
            driveId = f?.driveId
            if (f?.parents?.firstOrNull() == rootId) reachedRoot = true
            chain += Crumb(folderId, f?.name ?: Strings.get(R.string.drive_folder), driveId)
        }
        chain.reverse()
        val sharedDrive = drive ?: driveId?.let { id -> drives.firstOrNull { it.id == id } }
        val (place, prefix) = when {
            reachedRoot -> Place.MyDrive to listOf(Crumb(rootId, Place.MyDrive.label))
            driveId != null || sharedDrive != null -> Place.SharedDrives to listOfNotNull(
                Crumb(DRIVES_ROOT, Place.SharedDrives.label),
                sharedDrive?.let { Crumb(it.id, it.name.ifBlank { Strings.get(R.string.common_shared_drive) }, it.id) },
            )
            else -> Place.SharedWithMe to listOf(Crumb(SHARED_ROOT, Place.SharedWithMe.label))
        }
        _ui.update { it.copy(place = place, crumbs = prefix + chain, results = null, query = "", error = null) }
        load()
    }

    fun openPlace(place: Place) {
        when (place) {
            Place.MyDrive -> openMyDrive()
            Place.SharedWithMe -> setRoot(Place.SharedWithMe, Crumb(SHARED_ROOT, Place.SharedWithMe.label))
            Place.SharedDrives -> setRoot(Place.SharedDrives, Crumb(DRIVES_ROOT, Place.SharedDrives.label))
        }
    }

    fun openMyDrive() {
        val root = _ui.value.rootId ?: return
        setRoot(Place.MyDrive, Crumb(root, Place.MyDrive.label))
    }

    private fun setRoot(place: Place, crumb: Crumb) {
        _ui.update { it.copy(place = place, crumbs = listOf(crumb), items = emptyList(), results = null, query = "", error = null) }
        load()
    }

    /** A folder to pair, in the folder picker's terms: Drive id, stored path ("My Drive/Photos"), shared drive id. */
    data class PairTarget(val folderId: String, val path: String, val driveId: String?)

    /**
     * [node] as a pair's Drive folder, or null when the path can't be told from here (search results,
     * a shared drive's own root, which the picker doesn't offer either). Paths use the picker's English roots.
     */
    fun pairTarget(node: DriveNodeEntity): PairTarget? {
        val s = _ui.value
        if (s.results != null || s.atDriveList || s.crumbs.isEmpty()) return null
        val names = s.crumbs.drop(1).map { it.name } + node.name
        val path = when (s.place) {
            Place.MyDrive -> (listOf("My Drive") + names).joinToString("/")
            Place.SharedWithMe -> (listOf("Shared with me") + names).joinToString("/")
            Place.SharedDrives -> names.joinToString("/")
        }
        // A shortcut's drive id is the shortcut's, not its target's (which may sit in a shared drive): start a plain new pair.
        if (node.isShortcut) return null
        return PairTarget(node.id, path, node.driveId ?: s.crumbs.lastOrNull()?.driveId)
    }

    fun open(node: DriveNodeEntity) {
        val s = _ui.value
        val crumb = when {
            s.atDriveList -> Crumb(node.id, node.name, node.id)
            node.isShortcut && node.shortcutTargetId != null -> Crumb(node.shortcutTargetId, node.name, node.driveId ?: s.crumbs.lastOrNull()?.driveId)
            else -> Crumb(node.id, node.name, node.driveId ?: s.crumbs.lastOrNull()?.driveId)
        }
        _ui.update { it.copy(crumbs = it.crumbs + crumb, results = null, query = "", error = null) }
        load()
    }

    fun goTo(index: Int) {
        _ui.update { it.copy(crumbs = it.crumbs.take(index + 1), results = null, query = "", error = null) }
        load()
    }

    fun up(): Boolean {
        val n = _ui.value.crumbs.size
        if (n <= 1) return false
        goTo(n - 2)
        return true
    }

    fun refresh() { load() }

    private fun load() {
        loadJob?.cancel()
        loadJob = viewModelScope.launch {
            val s = _ui.value
            val top = s.crumbs.lastOrNull() ?: return@launch
            _ui.update { it.copy(loading = true, error = null) }
            try {
                val list: List<DriveNodeEntity> = when (top.id) {
                    SHARED_ROOT -> db.nodes().sharedWithMeRoots(accountId, s.rootId ?: "root")
                    DRIVES_ROOT -> s.drives.map { d ->
                        DriveNodeEntity(accountId = accountId, id = d.id, name = d.name.ifBlank { Strings.get(R.string.common_shared_drive) }, mimeType = FOLDER_MIME, driveId = d.id, isFolder = true)
                    }
                    else -> {
                        val cached = if (!s.indexing) db.nodes().children(accountId, top.id) else emptyList()
                        cached.ifEmpty {
                            api.listChildren(accountId, top.id, top.driveId).map { it.toNode(accountId) }
                                .sortedWith(compareByDescending<DriveNodeEntity> { it.isFolder }.thenBy(String.CASE_INSENSITIVE_ORDER) { it.name })
                        }
                    }
                }
                _ui.update { cur -> if (cur.crumbs.lastOrNull()?.id == top.id) cur.copy(items = list, loading = false) else cur }
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _ui.update { it.copy(loading = false, error = AppError.wrap(t).userHint) }
            }
        }
    }

    fun setQuery(q: String) {
        _ui.update { it.copy(query = q) }
        searchJob?.cancel()
        if (q.isBlank()) { _ui.update { it.copy(results = null) }; return }
        searchJob = viewModelScope.launch {
            delay(300)
            val results = runCatching { db.nodes().search(accountId, q.trim()) }.getOrDefault(emptyList())
            _ui.update { if (it.query == q) it.copy(results = results) else it }
        }
    }

    fun clearSearch() { searchJob?.cancel(); _ui.update { it.copy(query = "", results = null) } }

    /** Keep offline / stop keeping offline, with Undo. */
    fun togglePin(node: DriveNodeEntity) = setPinned(node, pin = node.id !in _ui.value.pinned, announce = true)

    private fun setPinned(node: DriveNodeEntity, pin: Boolean, announce: Boolean) {
        viewModelScope.launch {
            try {
                if (pin) db.pins().upsert(PinEntity(accountId, node.id, node.isFolder)) else db.pins().delete(accountId, node.id)
                pinRequests.notifyChanged()
                if (announce) {
                    _notes.emit(Note(Strings.get(if (pin) R.string.drive_note_kept_offline else R.string.drive_note_no_longer_offline, node.name)) { setPinned(node, !pin, announce = false) })
                }
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _failure.value = Failure(Strings.get(if (pin) R.string.drive_failure_keep_offline else R.string.drive_failure_change, node.name), AppError.wrap(t).userHint) { setPinned(node, pin, announce) }
            }
        }
    }

    /** Saves a copy of [node] into Downloads; Google-native documents are exported as Office files. */
    fun download(node: DriveNodeEntity) {
        if (_ui.value.busy != null) return
        _failure.value = null
        viewModelScope.launch {
            _ui.update { it.copy(busy = Strings.get(R.string.drive_busy_saving, node.name)) }
            try {
                val isGoogle = node.mimeType.startsWith(GOOGLE_APPS_PREFIX)
                val target = if (isGoogle) DocsExport.target(node.mimeType, preferPdf = false) else null
                if (isGoogle && target == null) throw AppError.Unknown(Strings.get(R.string.drive_cannot_export))
                val fileName = if (target != null) DocsExport.localName(node.name, target) else node.name
                val dest = withContext(Dispatchers.IO) { DownloadsDir.uniqueFile(fileName) }
                val temp = File(dest.parentFile, dest.name + LocalFs.PART_SUFFIX)
                withContext(Dispatchers.IO) { if (temp.exists()) temp.delete() }
                if (target != null) {
                    api.export(accountId, node.id, target.mime, temp) { done, _ -> _ui.update { it.copy(busy = Strings.get(R.string.drive_busy_exporting_kb, node.name, done / 1024)) } }
                } else {
                    api.download(accountId, node.id, temp, node.size) { done, total ->
                        _ui.update { it.copy(busy = if (total != null && total > 0) Strings.get(R.string.drive_busy_saving_percent, node.name, done * 100 / total) else Strings.get(R.string.drive_busy_saving_kb, node.name, done / 1024)) }
                    }
                }
                withContext(Dispatchers.IO) { if (!temp.renameTo(dest)) { temp.copyTo(dest, overwrite = true); temp.delete() } }
                DownloadsDir.scan(context, dest)
                _notes.emit(Note(Strings.get(R.string.drive_saved_to_downloads, dest.name)))
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _failure.value = Failure(Strings.get(R.string.drive_failure_save, node.name), AppError.wrap(t).userHint) { download(node) }
            } finally {
                _ui.update { it.copy(busy = null) }
            }
        }
    }

    fun note(text: String) { _notes.tryEmit(Note(text)) }

    companion object {
        /** Ids of the virtual roots; never real Drive ids. */
        const val SHARED_ROOT = "shared-with-me:root"
        const val DRIVES_ROOT = "shared-drives:root"
    }
}
