package com.almodhayan.drivesync.ui.pair

import androidx.activity.compose.BackHandler
import androidx.annotation.StringRes
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.DriveNodeEntity
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.drive.DriveApi
import com.almodhayan.drivesync.drive.DriveFile
import com.almodhayan.drivesync.drive.DriveNodeCache
import com.almodhayan.drivesync.drive.SharedDrive
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.common.AppScreen
import com.almodhayan.drivesync.ui.common.EmptyState
import com.almodhayan.drivesync.ui.common.EmptyStateAction
import com.almodhayan.drivesync.ui.common.LoadingState
import com.almodhayan.drivesync.ui.components.BottomActionBar
import com.almodhayan.drivesync.ui.components.Callout
import com.almodhayan.drivesync.ui.components.CalloutTone
import com.almodhayan.drivesync.ui.components.DsDivider
import com.almodhayan.drivesync.ui.components.DsListItem
import com.almodhayan.drivesync.ui.components.DsSegmentedButton
import com.almodhayan.drivesync.ui.components.DsTextField
import com.almodhayan.drivesync.ui.components.FileKind
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.ItemTone
import com.almodhayan.drivesync.ui.components.NavIcon
import com.almodhayan.drivesync.ui.components.PathBreadcrumb
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import javax.inject.Inject

@HiltViewModel
class RemoteFolderPickerViewModel @Inject constructor(
    handle: SavedStateHandle,
    private val db: Db,
    private val api: DriveApi,
    private val cache: DriveNodeCache,
) : ViewModel() {

    val accountId: String = handle.get<String>("accountId") ?: ""

    /**
     * The places the segmented button switches between. [pathRoot] starts the stored Drive path and
     * stays English, so a pair's path doesn't depend on the language it was made in.
     */
    enum class Scope(@StringRes val label: Int, val pathRoot: String) {
        MyDrive(R.string.pair_remote_my_drive, "My Drive"),
        Shared(R.string.pair_remote_shared_with_me, "Shared with me"),
        Drives(R.string.pair_remote_shared_drives, "Shared drives"),
    }

    data class Crumb(val id: String, val name: String)

    /** One row. Files are listed too, disabled, because only folders can be paired. */
    data class Item(
        val id: String,
        val name: String,
        val isFolder: Boolean,
        val isShortcut: Boolean,
        val shared: Boolean,
        val permissions: Int?,
        val driveId: String?,
    )

    data class Hit(val item: Item, val path: String)

    data class State(
        val rootId: String? = null,
        val drives: List<SharedDrive> = emptyList(),
        val scope: Scope = Scope.MyDrive,
        /** The shared drive being browsed, or the drive of a shared folder; null in My Drive. */
        val driveId: String? = null,
        /** Folders below the scope's root. In Shared drives the first crumb is the drive itself. */
        val crumbs: List<Crumb> = emptyList(),
        val items: List<Item> = emptyList(),
        val loading: Boolean = true,
        val indexing: Boolean = false,
        val error: String? = null,
        val fatal: String? = null,
        val creating: Boolean = false,
        /** Folder id → files and size from the metadata cache. */
        val counts: Map<String, FolderCount> = emptyMap(),
        val pairedHere: Map<String, String> = emptyMap(),
        val pairedInside: Map<String, Int> = emptyMap(),
        val searching: Boolean = false,
        val query: String = "",
        val searchBusy: Boolean = false,
        val hits: List<Hit> = emptyList(),
    ) {
        /** The folder the list shows; null at the lists of shared roots and shared drives. */
        val currentId: String? get() = crumbs.lastOrNull()?.id ?: if (scope == Scope.MyDrive) rootId else null
        val currentName: String get() = crumbs.lastOrNull()?.name ?: Strings.get(scope.label)

        /** Storage roots can never be paired: My Drive itself, a shared drive's root, and the lists of roots. */
        val canUse: Boolean
            get() = when (scope) {
                Scope.MyDrive, Scope.Shared -> crumbs.isNotEmpty()
                Scope.Drives -> crumbs.size >= 2
            }

        val canCreate: Boolean get() = currentId != null

        /** Stored as the pair's Drive path: "My Drive/Photos/2025". */
        val path: String
            get() = when (scope) {
                Scope.MyDrive -> (listOf(Scope.MyDrive.pathRoot) + crumbs.map { it.name }).joinToString("/")
                Scope.Shared -> (listOf(Scope.Shared.pathRoot) + crumbs.map { it.name }).joinToString("/")
                Scope.Drives -> crumbs.joinToString("/") { it.name }
            }

        /** The pair using exactly this folder, or "2 paired" when pairs use folders inside it. */
        fun pairMeta(id: String): String? = pairedHere[id]?.let { shortName(it) } ?: pairedInside[id]?.let { Strings.get(R.string.pair_local_n_paired, it) }
    }

    private class Target(val scope: Scope, val crumbs: List<Crumb>, val driveId: String?)

    private val _state = MutableStateFlow(State())
    val state: StateFlow<State> = _state

    private var loadJob: Job? = null
    private var countJob: Job? = null
    private var searchJob: Job? = null
    private var snapshot: DriveNodeCache.Snapshot? = null
    private var totals: DriveTotals? = null
    private val totalsLock = Mutex()
    private var pairs: List<FolderPairEntity> = emptyList()
    private val warmedDrives = HashSet<String>()

    init {
        start()
        // Warm the metadata cache; browsing uses live Drive listings until it is ready.
        viewModelScope.launch {
            try { cache.refresh(accountId) } catch (e: CancellationException) { throw e } catch (_: Throwable) { }
            loadSnapshot()
            _state.update { it.copy(indexing = false) }
            if (_state.value.rootId != null) load()
        }
        viewModelScope.launch {
            db.pairs().observeAll().collect { list ->
                pairs = list.filter { it.accountId == accountId }
                recomputePaired()
            }
        }
    }

    private fun start() {
        viewModelScope.launch {
            _state.update { it.copy(loading = true, fatal = null) }
            try {
                val account = db.accounts().get(accountId)
                if (account == null) {
                    _state.update { it.copy(loading = false, fatal = Strings.get(R.string.pair_remote_account_missing)) }
                    return@launch
                }
                if (snapshot == null) _state.update { it.copy(indexing = account.lastCrawlAt == null) }
                val rootId = account.rootFolderId ?: api.rootId(accountId)
                _state.update { it.copy(rootId = rootId) }
                load()
                val drives = try { api.listSharedDrives(accountId) } catch (e: CancellationException) { throw e } catch (_: Throwable) { emptyList() }
                _state.update { it.copy(drives = drives) }
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _state.update { it.copy(loading = false, fatal = AppError.wrap(t).userHint) }
            }
        }
    }

    // ------------------------------------------------------------------ navigation

    fun setScope(scope: Scope) {
        val s = _state.value
        if (s.scope == scope && s.crumbs.isEmpty()) return
        _state.update { it.copy(scope = scope, crumbs = emptyList(), driveId = null, items = emptyList(), error = null) }
        load()
    }

    fun open(item: Item) {
        if (!item.isFolder) return
        val s = _state.value
        val enteringDrive = s.scope == Scope.Drives && s.crumbs.isEmpty()
        val driveId = if (enteringDrive) item.id else (s.driveId ?: item.driveId)
        _state.update { it.copy(crumbs = it.crumbs + Crumb(item.id, item.name), driveId = driveId, items = emptyList(), error = null) }
        if (enteringDrive) warmDrive(item.id)
        load()
    }

    /** Jumps to crumb [index]; -1 is the root of the current scope. */
    fun goTo(index: Int) {
        _state.update { s ->
            val crumbs = if (index < 0) emptyList() else s.crumbs.take(index + 1)
            s.copy(crumbs = crumbs, driveId = if (crumbs.isEmpty()) null else s.driveId, items = emptyList(), error = null)
        }
        load()
    }

    fun up() {
        val n = _state.value.crumbs.size
        if (n > 0) goTo(n - 2)
    }

    fun retry() {
        val s = _state.value
        if (s.fatal != null || s.rootId == null) start() else load()
    }

    private fun load() {
        loadJob?.cancel()
        val s = _state.value
        _state.update { it.copy(loading = true, error = null) }
        loadJob = viewModelScope.launch {
            try {
                val parent = s.currentId
                val items = when {
                    s.scope == Scope.Drives && s.crumbs.isEmpty() ->
                        s.drives.map { Item(it.id, it.name.ifBlank { Strings.get(R.string.common_shared_drive) }, isFolder = true, isShortcut = false, shared = false, permissions = null, driveId = it.id) }
                    s.scope == Scope.Shared && s.crumbs.isEmpty() -> sharedRoots(s.rootId, s.indexing)
                    parent != null -> children(parent, s.driveId, s.indexing)
                    else -> emptyList()
                }
                val sorted = items.sortedWith(compareByDescending<Item> { it.isFolder }.thenBy(String.CASE_INSENSITIVE_ORDER) { it.name })
                _state.update { cur -> if (cur.scope != s.scope || cur.crumbs != s.crumbs) cur else cur.copy(items = sorted, loading = false) }
                updateCounts(sorted.filter { it.isFolder }.map { it.id } + listOfNotNull(parent.takeIf { s.canUse }))
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _state.update { cur -> if (cur.scope != s.scope || cur.crumbs != s.crumbs) cur else cur.copy(loading = false, error = AppError.wrap(t).userHint) }
            }
        }
    }

    private suspend fun children(parentId: String, driveId: String?, indexing: Boolean): List<Item> {
        if (!indexing) {
            val cached = db.nodes().children(accountId, parentId)
            if (cached.isNotEmpty()) return cached.map { it.toItem() }
        }
        return api.listChildren(accountId, parentId, driveId).filter { !it.trashed }.map { it.toItem() }
    }

    /** Top-level items others shared with this account. */
    private suspend fun sharedRoots(rootId: String?, indexing: Boolean): List<Item> {
        if (!indexing && rootId != null) {
            val cached = db.nodes().sharedWithMeRoots(accountId, rootId)
            if (cached.isNotEmpty()) return cached.map { it.toItem() }
        }
        val out = ArrayList<DriveFile>()
        var token: String? = null
        do {
            val page = api.listPage(accountId, "sharedWithMe = true and trashed = false", token)
            out += page.files
            token = page.nextPageToken
        } while (token != null && out.size < 2000)
        return out.map { it.toItem() }
    }

    // ------------------------------------------------------------------ sizes and pairs, from the cache

    private suspend fun loadSnapshot() {
        try {
            val snap = withContext(Dispatchers.IO) { cache.snapshot(accountId) }
            totalsLock.withLock {
                snapshot = snap
                totals = DriveTotals(snap)
            }
            recomputePaired()
        } catch (e: CancellationException) {
            throw e
        } catch (_: Throwable) {
        }
    }

    private fun updateCounts(ids: List<String>) {
        if (totals == null || ids.isEmpty()) return
        countJob?.cancel()
        countJob = viewModelScope.launch(Dispatchers.Default) {
            val found = HashMap<String, FolderCount>()
            totalsLock.withLock {
                val t = totals ?: return@withLock
                for (id in ids) if (t.knows(id)) found[id] = t.of(id)
            }
            if (found.isNotEmpty()) _state.update { it.copy(counts = it.counts + found) }
        }
    }

    private fun recomputePaired() {
        val snap = snapshot
        val here = HashMap<String, String>()
        val inside = HashMap<String, Int>()
        for (p in pairs) {
            if (p.remoteFolderId.isBlank()) continue
            here.putIfAbsent(p.remoteFolderId, p.name)
            if (snap != null) for (a in DriveTotals.ancestors(snap, p.remoteFolderId)) inside[a] = (inside[a] ?: 0) + 1
        }
        _state.update { it.copy(pairedHere = here, pairedInside = inside) }
    }

    /** Opening a shared drive indexes it once, so its folders get sizes. */
    private fun warmDrive(driveId: String) {
        if (!warmedDrives.add(driveId)) return
        viewModelScope.launch {
            try { cache.refresh(accountId, driveId) } catch (e: CancellationException) { throw e } catch (_: Throwable) { return@launch }
            loadSnapshot()
            val s = _state.value
            updateCounts(s.items.filter { it.isFolder }.map { it.id } + listOfNotNull(s.currentId.takeIf { s.canUse }))
        }
    }

    // ------------------------------------------------------------------ search

    fun openSearch() { _state.update { it.copy(searching = true, query = "", hits = emptyList(), searchBusy = false) } }

    fun closeSearch() {
        searchJob?.cancel()
        _state.update { it.copy(searching = false, query = "", hits = emptyList(), searchBusy = false) }
    }

    fun setQuery(q: String) {
        searchJob?.cancel()
        if (q.isBlank()) {
            _state.update { it.copy(query = q, hits = emptyList(), searchBusy = false) }
            return
        }
        _state.update { it.copy(query = q, searchBusy = true) }
        searchJob = viewModelScope.launch {
            delay(250)
            try {
                val nodes = db.nodes().search(accountId, q.trim()).filter { it.isFolder && !it.isShortcut }
                val snap = snapshot
                val names = _state.value.drives.associate { it.id to it.name }
                val hits = withContext(Dispatchers.Default) {
                    nodes.map { n ->
                        val where = snap?.pathOf(n.id, names)?.takeIf { it.isNotBlank() }?.let { Fmt.humanRemotePath(it) }
                        Hit(n.toItem(), where ?: n.name)
                    }
                }
                _state.update { if (it.query == q) it.copy(hits = hits, searchBusy = false) else it }
            } catch (e: CancellationException) {
                throw e
            } catch (_: Throwable) {
                _state.update { if (it.query == q) it.copy(hits = emptyList(), searchBusy = false) else it }
            }
        }
    }

    /** Opens a search result where it lives: My Drive, a shared drive, or Shared with me. */
    fun openHit(hit: Hit) {
        viewModelScope.launch {
            if (snapshot == null) loadSnapshot()
            val snap = snapshot ?: return@launch
            val node = snap.node(hit.item.id) ?: return@launch
            val chain = ArrayList<DriveNodeEntity>()
            var cur: DriveNodeEntity? = node
            var guard = 0
            while (cur != null && guard++ < 256) {
                chain += cur
                cur = cur.parentId?.let { snap.node(it) }
            }
            chain.reverse()
            val top = chain.first()
            val s = _state.value
            val nodeDrive = node.driveId
            val target = when {
                nodeDrive != null -> {
                    val driveName = s.drives.firstOrNull { it.id == nodeDrive }?.name?.ifBlank { null } ?: Strings.get(R.string.common_shared_drive)
                    Target(Scope.Drives, listOf(Crumb(nodeDrive, driveName)) + chain.filter { it.id != nodeDrive }.map { Crumb(it.id, it.name) }, nodeDrive)
                }
                top.parentId == snap.rootId -> Target(Scope.MyDrive, chain.map { Crumb(it.id, it.name) }, null)
                else -> Target(Scope.Shared, chain.map { Crumb(it.id, it.name) }, null)
            }
            searchJob?.cancel()
            _state.update {
                it.copy(
                    scope = target.scope, crumbs = target.crumbs, driveId = target.driveId,
                    searching = false, query = "", hits = emptyList(), searchBusy = false, items = emptyList(), error = null,
                )
            }
            val targetDrive = target.driveId
            if (target.scope == Scope.Drives && targetDrive != null) warmDrive(targetDrive)
            load()
        }
    }

    // ------------------------------------------------------------------ new folder

    /** Creates a folder inside the current one and opens it. [onDone] receives an error or null. */
    fun createFolder(name: String, onDone: (String?) -> Unit) {
        val parent = _state.value.currentId ?: return onDone(Strings.get(R.string.pair_new_folder_open_first))
        val clean = name.trim()
        if (clean.isEmpty()) return onDone(Strings.get(R.string.pair_new_folder_enter_name))
        if (clean.contains('/')) return onDone(Strings.get(R.string.pair_remote_bad_name))
        _state.update { it.copy(creating = true) }
        viewModelScope.launch {
            try {
                val f = api.createFolder(accountId, parent, clean)
                try { cache.remember(accountId, f) } catch (e: CancellationException) { throw e } catch (_: Throwable) { }
                _state.update { it.copy(creating = false) }
                onDone(null)
                open(f.toItem())
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _state.update { it.copy(creating = false) }
                onDone(AppError.wrap(t).userHint)
            }
        }
    }
}

private fun DriveNodeEntity.toItem() = RemoteFolderPickerViewModel.Item(id, name, isFolder, isShortcut, shared, permissionCount, driveId)

private fun DriveFile.toItem() = RemoteFolderPickerViewModel.Item(id, name, isFolder, isShortcut, shared, permissionIds?.size, driveId)

/**
 * Drive folder picker (design screen 4): My Drive / Shared with me (and shared drives), folders in the
 * Drive colour, files listed but disabled. Shared folders say how many people will see changes.
 */
@Composable
fun RemoteFolderPickerScreen(nav: AppNav, accountId: String) {
    val vm: RemoteFolderPickerViewModel = hiltViewModel()
    val state by vm.state.collectAsStateWithLifecycle()
    var showNewFolder by remember { mutableStateOf(false) }
    var newFolderError by remember { mutableStateOf<String?>(null) }

    BackHandler(enabled = state.searching || state.crumbs.isNotEmpty()) {
        if (state.searching) vm.closeSearch() else vm.up()
    }

    fun choose() {
        val folder = state.crumbs.lastOrNull() ?: return
        val h = nav.controller.previousBackStackEntry?.savedStateHandle
        h?.set(PickerKeys.REMOTE_PATH, state.path)
        h?.set(PickerKeys.DRIVE_ID, state.driveId ?: "")
        h?.set(PickerKeys.REMOTE_FOLDER_ID, folder.id)   // written last: the editor reacts to this key
        nav.back()
    }

    AppScreen(
        title = stringResource(R.string.pair_picker_choose_folder),
        subtitle = stringResource(R.string.pair_remote_in_drive),
        navIcon = NavIcon.Close,
        onBack = { nav.back() },
        actions = {
            if (state.fatal == null && state.rootId != null) {
                IconButton(onClick = { vm.openSearch() }, enabled = !state.searching) {
                    Icon(Sym.Search, contentDescription = stringResource(R.string.pair_remote_search_drive))
                }
                IconButton(onClick = { newFolderError = null; showNewFolder = true }, enabled = state.canCreate && !state.searching && !state.creating, modifier = Modifier.testTag("remote_new_folder")) {
                    Icon(Sym.CreateNewFolder, contentDescription = stringResource(R.string.common_new_folder))
                }
            }
        },
        bottomBar = {
            if (state.fatal == null && state.rootId != null && !state.searching) {
                BottomActionBar(hint = remoteHint(state)) {
                    Button(onClick = { choose() }, enabled = state.canUse, modifier = Modifier.testTag("picker_use")) {
                        Text(if (state.canUse) stringResource(R.string.pair_remote_use_name, shortName(state.currentName, 18)) else stringResource(R.string.pair_picker_use_this_folder), maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                }
            }
        },
    ) { padding ->
        Column(Modifier.padding(padding).fillMaxSize()) {
            when {
                state.fatal != null -> EmptyState(
                    Sym.CloudOff,
                    stringResource(R.string.pair_remote_cant_reach),
                    state.fatal,
                    action = { EmptyStateAction(stringResource(R.string.common_try_again), onClick = { vm.retry() }) },
                )
                state.rootId == null -> LoadingState(stringResource(R.string.pair_remote_connecting))
                state.searching -> SearchPane(state, vm)
                else -> BrowsePane(state, vm)
            }
        }
    }

    if (showNewFolder) {
        NewFolderDialog(
            onDismiss = { showNewFolder = false },
            busy = state.creating,
            error = newFolderError,
            onCreate = { name ->
                newFolderError = null
                vm.createFolder(name) { err -> if (err == null) showNewFolder = false else newFolderError = err }
                false
            },
        )
    }
}

/** "Photos · 2,410 files"; roots say what to do instead. */
private fun remoteHint(state: RemoteFolderPickerViewModel.State): String {
    if (!state.canUse) {
        return if (state.scope == RemoteFolderPickerViewModel.Scope.Drives && state.crumbs.isEmpty()) Strings.get(R.string.pair_remote_hint_open_drive)
        else Strings.get(R.string.pair_local_hint_open_folder)
    }
    val count = state.currentId?.let { state.counts[it] } ?: return state.currentName
    return "${state.currentName} · " + if (count.files == 0) Strings.get(R.string.pair_folder_empty) else Fmt.files(count.files)
}

@Composable
private fun BrowsePane(state: RemoteFolderPickerViewModel.State, vm: RemoteFolderPickerViewModel) {
    val scopes = buildList {
        add(RemoteFolderPickerViewModel.Scope.MyDrive)
        add(RemoteFolderPickerViewModel.Scope.Shared)
        if (state.drives.isNotEmpty() || state.scope == RemoteFolderPickerViewModel.Scope.Drives) add(RemoteFolderPickerViewModel.Scope.Drives)
    }
    DsSegmentedButton(
        items = scopes.map { stringResource(it.label) },
        selectedIndex = scopes.indexOf(state.scope).coerceAtLeast(0),
        onSelect = { vm.setScope(scopes[it]) },
        modifier = Modifier.padding(start = Space.s16, end = Space.s16, bottom = Space.s8),
    )
    if (state.indexing) {
        Callout(
            stringResource(R.string.pair_remote_indexing),
            Modifier.padding(start = Space.s16, end = Space.s16, bottom = Space.s8),
            tone = CalloutTone.Info,
        )
    }
    PathBreadcrumb(rootLabel = stringResource(state.scope.label), rootIcon = Sym.Cloud, parts = state.crumbs.map { it.name }, onCrumb = { vm.goTo(it) })
    DsDivider()
    when {
        state.loading && state.items.isEmpty() -> LoadingState(stringResource(R.string.pair_remote_listing))
        state.error != null -> EmptyState(
            Sym.CloudOff,
            stringResource(R.string.pair_local_cant_open, state.currentName),
            state.error,
            action = { EmptyStateAction(stringResource(R.string.common_try_again), onClick = { vm.retry() }) },
        )
        state.items.isEmpty() -> Text(
            emptyText(state),
            Modifier.padding(Space.s16),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        else -> LazyColumn(Modifier.fillMaxSize()) {
            items(state.items, key = { it.id }) { item -> DriveRow(item, state, onOpen = { vm.open(item) }) }
        }
    }
}

private fun emptyText(state: RemoteFolderPickerViewModel.State): String = when {
    state.scope == RemoteFolderPickerViewModel.Scope.Drives && state.crumbs.isEmpty() -> Strings.get(R.string.pair_remote_no_shared_drives)
    state.scope == RemoteFolderPickerViewModel.Scope.Shared && state.crumbs.isEmpty() -> Strings.get(R.string.pair_remote_nothing_shared)
    else -> Strings.get(R.string.pair_remote_folder_empty)
}

@Composable
private fun DriveRow(item: RemoteFolderPickerViewModel.Item, state: RemoteFolderPickerViewModel.State, onOpen: () -> Unit) {
    if (!item.isFolder) {
        DsListItem(
            headline = item.name,
            icon = FileKind.of(item.name).icon,
            enabled = false,
            disabledReason = stringResource(if (item.isShortcut) R.string.pair_remote_shortcut_reason else R.string.pair_remote_file_reason),
        )
        return
    }
    val driveRoot = state.scope == RemoteFolderPickerViewModel.Scope.Drives && state.crumbs.isEmpty()
    DsListItem(
        headline = item.name,
        icon = if (item.shared || driveRoot) Sym.FolderSharedFill else Sym.FolderFill,
        tone = ItemTone.Drive,
        supporting = when {
            driveRoot -> stringResource(R.string.common_shared_drive)
            item.shared -> sharedText(item.permissions)
            else -> state.counts[item.id]?.text
        },
        meta = state.pairMeta(item.id),
        chevron = true,
        onClick = onOpen,
    )
}

/** A sync in a shared folder edits other people's files, so say how many will see it. */
private fun sharedText(permissions: Int?): String {
    val people = ((permissions ?: 2) - 1).coerceAtLeast(1)
    return Strings.plural(R.plurals.pair_remote_shared_people, people, Fmt.count(people))
}

@Composable
private fun SearchPane(state: RemoteFolderPickerViewModel.State, vm: RemoteFolderPickerViewModel) {
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
    DsTextField(
        value = state.query,
        onValueChange = { vm.setQuery(it) },
        label = stringResource(R.string.pair_remote_search_folders),
        modifier = Modifier.padding(horizontal = Space.s16).focusRequester(focus),
        leading = Sym.Search,
        trailing = { IconButton(onClick = { vm.closeSearch() }) { Icon(Sym.Close, contentDescription = stringResource(R.string.pair_remote_close_search)) } },
    )
    val note = when {
        state.indexing -> stringResource(R.string.pair_remote_search_indexing)
        state.query.isBlank() -> stringResource(R.string.pair_remote_search_type)
        state.searchBusy && state.hits.isEmpty() -> null
        state.hits.isEmpty() -> stringResource(R.string.pair_remote_search_no_match, state.query.trim())
        else -> null
    }
    if (note != null) {
        Text(note, Modifier.padding(Space.s16), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
    if (state.hits.isNotEmpty()) {
        LazyColumn(Modifier.fillMaxSize()) {
            items(state.hits, key = { it.item.id }) { hit ->
                DsListItem(
                    headline = hit.item.name,
                    supporting = hit.path,
                    icon = if (hit.item.shared) Sym.FolderSharedFill else Sym.FolderFill,
                    tone = ItemTone.Drive,
                    meta = state.pairMeta(hit.item.id),
                    chevron = true,
                    onClick = { vm.openHit(hit) },
                )
            }
        }
    }
}
