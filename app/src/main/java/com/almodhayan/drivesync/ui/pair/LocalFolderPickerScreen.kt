package com.almodhayan.drivesync.ui.pair

import android.content.Context
import android.os.storage.StorageManager
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Box
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.fs.LocalFs
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
import com.almodhayan.drivesync.ui.components.ItemTone
import com.almodhayan.drivesync.ui.components.NavIcon
import com.almodhayan.drivesync.ui.components.PathBreadcrumb
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import dagger.hilt.android.lifecycle.HiltViewModel
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.job
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.io.IOException
import java.util.concurrent.ConcurrentHashMap
import javax.inject.Inject

@HiltViewModel
class LocalFolderPickerViewModel @Inject constructor(
    handle: SavedStateHandle,
    @ApplicationContext private val context: Context,
    private val localFs: LocalFs,
    db: Db,
) : ViewModel() {

    /** One storage volume; [internal] is the phone's own storage. */
    data class Volume(val label: String, val root: File, val internal: Boolean)

    data class State(
        val volumes: List<Volume> = emptyList(),
        val volume: Volume? = null,
        val current: File? = null,
        val entries: List<File> = emptyList(),
        val showHidden: Boolean = false,
        val loading: Boolean = true,
        val error: String? = null,
        val hasAccess: Boolean = true,
        /** Folder path → files and size inside it, counted lazily with a cap. */
        val counts: Map<String, FolderCount> = emptyMap(),
        val pairs: List<FolderPairEntity> = emptyList(),
    ) {
        /** Folders below the volume root, down to the current one. */
        val crumbs: List<File>
            get() {
                val cur = current ?: return emptyList()
                val root = volume?.root ?: return emptyList()
                val out = ArrayList<File>()
                var f: File? = cur
                while (f != null && f.path != root.path) {
                    out += f
                    f = f.parentFile
                }
                return out.asReversed()
            }

        /** Storage roots can never be paired. */
        val atRoot: Boolean get() = current != null && volume != null && current.path == volume.root.path

        /** Inside Android/, where only media can be paired. */
        val inAndroid: Boolean get() = current?.let { rel(it).equals("Android", ignoreCase = true) } == true

        val canUse: Boolean get() = current != null && !atRoot && !inAndroid && !isProtected(current)

        /** Path relative to the current volume: "DCIM/Camera". */
        fun rel(f: File): String {
            val root = volume?.root?.path ?: return f.path
            return if (f.path == root) "" else f.path.removePrefix("$root/")
        }

        /** Android/data and Android/obb are private to each app since Android 11. */
        fun isProtected(f: File): Boolean {
            val r = rel(f)
            return LocalFolderPickerViewModel.PROTECTED.any { p -> r.equals(p, ignoreCase = true) || r.startsWith("$p/", ignoreCase = true) }
        }

        /** The pair using exactly this folder, or "2 paired" when pairs use folders inside it. */
        fun pairMeta(f: File): String? {
            val path = f.path.trimEnd('/')
            pairs.firstOrNull { it.localPath.trimEnd('/').equals(path, ignoreCase = true) }?.let { return shortName(it.name) }
            val inside = pairs.count { it.localPath.startsWith("$path/", ignoreCase = true) }
            return if (inside > 0) Strings.get(R.string.pair_local_n_paired, inside) else null
        }
    }

    private val _state = MutableStateFlow(State())
    val state: StateFlow<State> = _state

    private var listJob: Job? = null
    private val countJobs = ConcurrentHashMap<String, Job>()
    private val countDispatcher = Dispatchers.IO.limitedParallelism(2)

    init {
        val volumes = localFs.volumes().take(3).mapIndexed { i, f -> Volume(volumeLabel(f, i), f, internal = i == 0) }
        _state.update { it.copy(volumes = volumes, hasAccess = localFs.hasAllFilesAccess()) }
        val start = handle.get<String>("start")?.takeIf { it.isNotBlank() }?.let { File(it) }?.takeIf { it.isDirectory }
        (start ?: volumes.firstOrNull()?.root)?.let { open(it) }
        viewModelScope.launch { db.pairs().observeAll().collect { list -> _state.update { it.copy(pairs = list) } } }
    }

    fun recheckAccess() {
        val access = localFs.hasAllFilesAccess()
        if (access == _state.value.hasAccess) return
        _state.update { it.copy(hasAccess = access) }
        if (access) reload()
    }

    fun open(dir: File) {
        val volume = _state.value.volumes.firstOrNull { dir.path == it.root.path || dir.path.startsWith(it.root.path + "/") }
        _state.update { it.copy(current = dir, volume = volume ?: it.volume, entries = emptyList(), loading = true, error = null) }
        cancelCountsOutside(dir)
        reload()
        val s = _state.value
        if (s.canUse) requestCount(dir)
    }

    fun openVolume(index: Int) {
        _state.value.volumes.getOrNull(index)?.let { open(it.root) }
    }

    /** One level up; at a storage root back closes the picker instead. */
    fun up() {
        val s = _state.value
        val cur = s.current ?: return
        if (s.atRoot) return
        open(cur.parentFile ?: return)
    }

    fun toggleHidden() {
        countJobs.values.forEach { it.cancel() }
        countJobs.clear()
        _state.update { it.copy(showHidden = !it.showHidden, counts = emptyMap()) }
        reload()
        val s = _state.value
        val cur = s.current
        if (cur != null && s.canUse) requestCount(cur)
    }

    private fun reload() {
        val s = _state.value
        val dir = s.current ?: return
        val showHidden = s.showHidden
        val androidDir = s.inAndroid
        listJob?.cancel()
        _state.update { it.copy(loading = true, error = null) }
        listJob = viewModelScope.launch {
            val result = withContext(Dispatchers.IO) {
                runCatching {
                    val kids = dir.listFiles() ?: throw IOException(Strings.get(R.string.pair_local_read_denied))
                    val dirs = kids.filter { it.isDirectory && (showHidden || !it.name.startsWith(".")) }.toMutableList()
                    // Android/data and Android/obb stay visible, disabled, with the reason.
                    if (androidDir) {
                        for (name in listOf("data", "obb")) if (dirs.none { it.name.equals(name, ignoreCase = true) }) dirs += File(dir, name)
                    }
                    dirs.sortedWith(compareBy(String.CASE_INSENSITIVE_ORDER) { it.name })
                }
            }
            _state.update { cur ->
                if (cur.current?.path != dir.path) cur
                else result.fold(
                    onSuccess = { list -> cur.copy(entries = list, loading = false, error = null) },
                    onFailure = { e -> cur.copy(entries = emptyList(), loading = false, error = e.message ?: Strings.get(R.string.pair_local_read_denied)) },
                )
            }
        }
    }

    /** Counts a folder's files once, on IO, two folders at a time, stopping at [COUNT_CAP]. */
    fun requestCount(dir: File) {
        val key = dir.path
        val s = _state.value
        if (s.counts.containsKey(key) || countJobs.containsKey(key)) return
        val includeHidden = s.showHidden
        val job = viewModelScope.launch(countDispatcher, start = CoroutineStart.LAZY) {
            try {
                val count = countPhoneFolder(dir, includeHidden, COUNT_CAP)
                _state.update { st -> if (st.showHidden != includeHidden) st else st.copy(counts = st.counts + (key to count)) }
            } finally {
                countJobs.remove(key, coroutineContext.job)
            }
        }
        countJobs[key] = job
        job.start()
    }

    /** Leaving a folder stops counting its rows; finished counts stay cached. */
    private fun cancelCountsOutside(dir: File) {
        for ((key, job) in countJobs) {
            if (key != dir.path && File(key).parent != dir.path) job.cancel()
        }
    }

    /** Creates a folder inside the current one and opens it. [onDone] receives an error or null. */
    fun createFolder(name: String, onDone: (String?) -> Unit) {
        val dir = _state.value.current ?: return onDone(Strings.get(R.string.pair_new_folder_open_first))
        val clean = name.trim()
        if (clean.isEmpty()) return onDone(Strings.get(R.string.pair_new_folder_enter_name))
        if (clean.contains('/') || clean == "." || clean == "..") return onDone(Strings.get(R.string.pair_new_folder_bad_name))
        viewModelScope.launch {
            val folder = File(dir, clean)
            val error = withContext(Dispatchers.IO) {
                when {
                    folder.exists() -> Strings.get(R.string.pair_new_folder_exists, clean)
                    !folder.mkdir() -> Strings.get(R.string.pair_local_mkdir_denied)
                    else -> null
                }
            }
            onDone(error)
            if (error == null) open(folder)
        }
    }

    private fun volumeLabel(root: File, index: Int): String {
        if (index == 0) return Strings.get(R.string.pair_local_internal_storage)
        val description = runCatching {
            context.getSystemService(StorageManager::class.java)?.getStorageVolume(root)?.getDescription(context)
        }.getOrNull()
        return description?.takeIf { it.isNotBlank() } ?: Strings.get(R.string.pair_local_sd_card)
    }

    companion object {
        const val COUNT_CAP = 10_000
        val PROTECTED = listOf("Android/data", "Android/obb")
    }
}

/**
 * Phone folder picker (design screen 4): tap a row to open it, then the bottom bar chooses the folder
 * you are in. Protected folders stay visible, disabled, with the reason. Storage roots can't be paired.
 */
@Composable
fun LocalFolderPickerScreen(nav: AppNav, start: String?) {
    val vm: LocalFolderPickerViewModel = hiltViewModel()
    val state by vm.state.collectAsStateWithLifecycle()
    val context = LocalContext.current
    var showNewFolder by remember { mutableStateOf(false) }
    var newFolderError by remember { mutableStateOf<String?>(null) }
    var creating by remember { mutableStateOf(false) }

    LifecycleResumeEffect(Unit) { vm.recheckAccess(); onPauseOrDispose { } }
    BackHandler(enabled = state.current != null && !state.atRoot) { vm.up() }

    fun choose(dir: File) {
        nav.controller.previousBackStackEntry?.savedStateHandle?.set(PickerKeys.LOCAL_PATH, dir.absolutePath)
        nav.back()
    }

    val current = state.current
    AppScreen(
        title = stringResource(R.string.pair_picker_choose_folder),
        subtitle = stringResource(R.string.pair_local_on_this_phone),
        navIcon = NavIcon.Close,
        onBack = { nav.back() },
        actions = {
            if (state.hasAccess && current != null) {
                IconButton(onClick = { vm.toggleHidden() }) {
                    Icon(
                        if (state.showHidden) Sym.VisibilityOff else Sym.Visibility,
                        contentDescription = stringResource(if (state.showHidden) R.string.pair_local_hide_hidden else R.string.pair_local_show_hidden),
                    )
                }
                IconButton(onClick = { newFolderError = null; showNewFolder = true }) {
                    Icon(Sym.CreateNewFolder, contentDescription = stringResource(R.string.common_new_folder))
                }
            }
        },
        bottomBar = {
            if (state.hasAccess && current != null) {
                BottomActionBar(hint = localHint(state, current)) {
                    Button(onClick = { choose(current) }, enabled = state.canUse, modifier = Modifier.testTag("picker_use")) { Text(stringResource(R.string.pair_picker_use_this_folder)) }
                }
            }
        },
    ) { padding ->
        when {
            !state.hasAccess -> EmptyState(
                Sym.Lock,
                stringResource(R.string.pair_local_no_access_title),
                stringResource(R.string.pair_local_no_access_body),
                Modifier.padding(padding),
            ) { Button(onClick = { openAllFilesAccess(context) }) { Text(stringResource(R.string.pair_allow_all_files_access)) } }
            current == null -> Box(Modifier.padding(padding)) { LoadingState(stringResource(R.string.pair_local_opening_storage)) }
            else -> Column(Modifier.padding(padding).fillMaxSize()) {
                if (state.volumes.size > 1) {
                    DsSegmentedButton(
                        items = state.volumes.map { it.label },
                        selectedIndex = state.volumes.indexOfFirst { it == state.volume }.coerceAtLeast(0),
                        onSelect = { vm.openVolume(it) },
                        modifier = Modifier.padding(start = Space.s16, end = Space.s16, bottom = Space.s8),
                    )
                }
                val volume = state.volume
                val crumbs = state.crumbs
                if (volume != null) {
                    PathBreadcrumb(
                        rootLabel = volume.label,
                        rootIcon = if (volume.internal) Sym.Mobile else Sym.SdCard,
                        parts = crumbs.map { it.name },
                        onCrumb = { i -> if (i < 0) vm.open(volume.root) else crumbs.getOrNull(i)?.let { vm.open(it) } },
                    )
                }
                DsDivider()
                when {
                    state.loading -> LoadingState(stringResource(R.string.pair_picker_reading_folder))
                    state.error != null -> EmptyState(
                        Sym.Block,
                        stringResource(R.string.pair_local_cant_open, current.name),
                        state.error,
                        action = { if (!state.atRoot) EmptyStateAction(stringResource(R.string.pair_local_go_up), onClick = { vm.up() }) },
                    )
                    else -> PhoneFolderList(state, vm)
                }
            }
        }
    }

    if (showNewFolder) {
        NewFolderDialog(
            onDismiss = { showNewFolder = false },
            error = newFolderError,
            busy = creating,
            onCreate = { name ->
                newFolderError = null
                creating = true
                vm.createFolder(name) { err ->
                    creating = false
                    if (err == null) showNewFolder = false else newFolderError = err
                }
                false
            },
        )
    }
}

/** "DCIM · 2,612 files · 23.4 GB"; roots and Android/ explain why they can't be chosen. */
private fun localHint(state: LocalFolderPickerViewModel.State, current: File): String = when {
    state.atRoot -> Strings.get(R.string.pair_local_hint_open_folder)
    state.inAndroid -> {
        val n = state.entries.count { !state.isProtected(it) }
        Strings.plural(R.plurals.pair_local_hint_android_pairable, n)
    }
    else -> state.counts[current.path]?.let { "${current.name} · ${it.text}" } ?: current.name
}

@Composable
private fun PhoneFolderList(state: LocalFolderPickerViewModel.State, vm: LocalFolderPickerViewModel) {
    LazyColumn(Modifier.fillMaxSize()) {
        if (state.entries.isEmpty()) {
            item {
                Text(
                    stringResource(R.string.pair_local_no_folders),
                    Modifier.padding(Space.s16),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        items(state.entries, key = { it.path }) { dir ->
            PhoneFolderRow(dir, state, onOpen = { vm.open(dir) }, onShown = { vm.requestCount(dir) })
        }
        if (state.inAndroid) {
            item {
                Callout(
                    stringResource(R.string.pair_local_android_private),
                    Modifier.padding(Space.s16),
                    tone = CalloutTone.Info,
                )
            }
        }
    }
}

@Composable
private fun PhoneFolderRow(dir: File, state: LocalFolderPickerViewModel.State, onOpen: () -> Unit, onShown: () -> Unit) {
    val rel = state.rel(dir)
    when {
        state.isProtected(dir) -> DsListItem(
            headline = dir.name,
            icon = Sym.FolderFill,
            enabled = false,
            disabledReason = stringResource(R.string.pair_local_protected_reason),
        )
        rel.equals("Android", ignoreCase = true) -> DsListItem(
            headline = dir.name,
            icon = Sym.FolderFill,
            supporting = stringResource(R.string.pair_local_only_media),
            chevron = true,
            onClick = onOpen,
        )
        rel.equals("Android/media", ignoreCase = true) -> DsListItem(
            headline = dir.name,
            icon = Sym.FolderFill,
            tone = ItemTone.Folder,
            supporting = stringResource(R.string.pair_local_media_supporting),
            meta = state.pairMeta(dir),
            chevron = true,
            onClick = onOpen,
        )
        else -> {
            LaunchedEffect(dir.path, state.showHidden) { onShown() }
            DsListItem(
                headline = dir.name,
                icon = Sym.FolderFill,
                tone = ItemTone.Folder,
                supporting = state.counts[dir.path]?.text ?: stringResource(R.string.pair_picker_counting),
                meta = state.pairMeta(dir),
                chevron = true,
                onClick = onOpen,
            )
        }
    }
}
