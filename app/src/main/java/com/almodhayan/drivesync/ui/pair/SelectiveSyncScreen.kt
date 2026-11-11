package com.almodhayan.drivesync.ui.pair

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.selection.toggleable
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.drive.DriveNodeCache
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.common.AppScreen
import com.almodhayan.drivesync.ui.common.EmptyState
import com.almodhayan.drivesync.ui.common.EmptyStateAction
import com.almodhayan.drivesync.ui.common.LoadingState
import com.almodhayan.drivesync.ui.components.BottomActionBar
import com.almodhayan.drivesync.ui.components.Callout
import com.almodhayan.drivesync.ui.components.CalloutTone
import com.almodhayan.drivesync.ui.components.DsListItem
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.NavIcon
import com.almodhayan.drivesync.ui.components.SectionHeader
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import javax.inject.Inject

@HiltViewModel
class SelectiveSyncViewModel @Inject constructor(
    handle: SavedStateHandle,
    private val db: Db,
    private val cache: DriveNodeCache,
) : ViewModel() {

    val pairId: String = handle.get<String>("pairId") ?: ""

    /** One folder that exists on the phone, on Drive, or both. [rel] keeps the original spelling. */
    class Node(val rel: String, val name: String, val children: List<Node>, val onDevice: Boolean, val onDrive: Boolean)
    data class Counts(val files: Int, val bytes: Long)

    data class State(
        val pair: FolderPairEntity? = null,
        val root: Node? = null,
        val loading: Boolean = true,
        val error: String? = null,
        val remoteWarning: String? = null,
        val excluded: Set<String> = emptySet(),
        val expanded: Set<String> = emptySet(),
        val counts: Map<String, Counts> = emptyMap(),
        val saving: Boolean = false,
        val saved: Boolean = false,
    ) {
        fun isExcluded(rel: String): Boolean = excluded.any { it.equals(rel, ignoreCase = true) }
        fun isUnderExcluded(rel: String): Boolean = excluded.any { rel.startsWith("$it/", ignoreCase = true) }
    }

    private val _state = MutableStateFlow(State())
    val state: StateFlow<State> = _state
    private val counting = HashSet<String>()

    init { load() }

    fun load() {
        viewModelScope.launch {
            _state.update { it.copy(loading = true, error = null) }
            try {
                val pair = db.pairs().get(pairId)
                if (pair == null) {
                    _state.update { it.copy(loading = false, error = Strings.get(R.string.pair_selective_missing)) }
                    return@launch
                }
                val excluded = pair.excludedSubfolderList().map { it.trim('/') }.filter { it.isNotEmpty() }.toSet()
                val entries = HashMap<String, Entry>()   // lowercase rel -> entry
                withContext(Dispatchers.IO) {
                    val root = File(pair.localPath)
                    if (root.isDirectory) walkLocal(root, "", pair.syncHidden, entries)
                }
                var warning: String? = null
                try {
                    try {
                        cache.refresh(pair.accountId, pair.driveId)
                    } catch (e: CancellationException) {
                        throw e
                    } catch (t: Throwable) {
                        warning = Strings.get(R.string.pair_selective_drive_stale, AppError.wrap(t).userHint)
                    }
                    val snap = withContext(Dispatchers.IO) { cache.snapshot(pair.accountId) }
                    walkRemote(snap, pair.remoteFolderId, "", entries, 0)
                } catch (e: CancellationException) {
                    throw e
                } catch (t: Throwable) {
                    warning = Strings.get(R.string.pair_selective_drive_unavailable, AppError.wrap(t).userHint)
                }
                val root = withContext(Dispatchers.Default) { buildTree(entries) }
                _state.update {
                    it.copy(pair = pair, root = root, loading = false, remoteWarning = warning, excluded = excluded, expanded = root.children.map { c -> c.rel }.toSet())
                }
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _state.update { it.copy(loading = false, error = AppError.wrap(t).userHint) }
            }
        }
    }

    private class Entry(val rel: String, var onDevice: Boolean = false, var onDrive: Boolean = false)

    private fun walkLocal(dir: File, prefix: String, syncHidden: Boolean, out: HashMap<String, Entry>) {
        val kids = dir.listFiles() ?: return
        for (f in kids) {
            if (!f.isDirectory) continue
            if (!syncHidden && f.name.startsWith(".")) continue
            val rel = if (prefix.isEmpty()) f.name else "$prefix/${f.name}"
            out.getOrPut(rel.lowercase()) { Entry(rel) }.onDevice = true
            walkLocal(f, rel, syncHidden, out)
        }
    }

    private fun walkRemote(snap: DriveNodeCache.Snapshot, folderId: String, prefix: String, out: HashMap<String, Entry>, depth: Int) {
        if (depth > 64) return
        for (n in snap.childrenOf(folderId)) {
            if (!n.isFolder || n.isShortcut) continue
            val rel = if (prefix.isEmpty()) n.name else "$prefix/${n.name}"
            out.getOrPut(rel.lowercase()) { Entry(rel) }.onDrive = true
            walkRemote(snap, n.id, rel, out, depth + 1)
        }
    }

    private fun buildTree(entries: HashMap<String, Entry>): Node {
        val byParent = HashMap<String, MutableList<Entry>>()
        for ((lower, e) in entries) byParent.getOrPut(lower.substringBeforeLast('/', "")) { ArrayList() }.add(e)
        fun build(lowerRel: String, rel: String, name: String, onDevice: Boolean, onDrive: Boolean): Node {
            val kids = (byParent[lowerRel] ?: emptyList()).sortedWith(compareBy(String.CASE_INSENSITIVE_ORDER) { it.rel.substringAfterLast('/') })
                .map { build(it.rel.lowercase(), it.rel, it.rel.substringAfterLast('/'), it.onDevice, it.onDrive) }
            return Node(rel, name, kids, onDevice, onDrive)
        }
        return build("", "", "", true, true)
    }

    fun toggleExpanded(rel: String) { _state.update { s -> s.copy(expanded = if (rel in s.expanded) s.expanded - rel else s.expanded + rel) } }

    fun toggle(rel: String) {
        _state.update { s ->
            if (s.isUnderExcluded(rel)) return@update s
            if (s.isExcluded(rel)) s.copy(excluded = s.excluded.filterNot { it.equals(rel, ignoreCase = true) }.toSet())
            else s.copy(excluded = s.excluded.filterNot { it.startsWith("$rel/", ignoreCase = true) }.toSet() + rel)
        }
    }

    /** Counts the phone files and size of a subtree once, lazily, when its row is shown. */
    fun requestCounts(rel: String) {
        val s = _state.value
        if (s.counts.containsKey(rel) || !counting.add(rel)) return
        val pair = s.pair ?: return
        viewModelScope.launch {
            val c = withContext(Dispatchers.IO) {
                var files = 0
                var bytes = 0L
                val stack = ArrayDeque<File>()
                stack.add(File(pair.localPath, rel))
                while (stack.isNotEmpty()) {
                    val d = stack.removeLast()
                    val kids = d.listFiles() ?: continue
                    for (f in kids) {
                        if (!pair.syncHidden && f.name.startsWith(".")) continue
                        if (f.isDirectory) stack.add(f) else if (f.isFile) { files++; bytes += f.length() }
                    }
                }
                Counts(files, bytes)
            }
            _state.update { it.copy(counts = it.counts + (rel to c)) }
        }
    }

    fun save() {
        val s = _state.value
        val pair = s.pair ?: return
        _state.update { it.copy(saving = true) }
        viewModelScope.launch {
            try {
                db.pairs().upsert(pair.copy(excludedSubfolders = s.excluded.sorted().joinToString("\n"), updatedAt = System.currentTimeMillis()))
                _state.update { it.copy(saving = false, saved = true) }
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _state.update { it.copy(saving = false, error = AppError.wrap(t).userHint) }
            }
        }
    }
}

/**
 * Choose subfolders: untick a folder to leave it, and everything inside it, out of the sync.
 * Opened from the pair editor's Skip section.
 */
@Composable
fun SelectiveSyncScreen(nav: AppNav, pairId: String) {
    val vm: SelectiveSyncViewModel = hiltViewModel()
    val state by vm.state.collectAsStateWithLifecycle()
    LaunchedEffect(state.saved) { if (state.saved) nav.back() }

    val visible = remember(state.root, state.expanded) {
        val out = ArrayList<Pair<SelectiveSyncViewModel.Node, Int>>()
        fun walk(n: SelectiveSyncViewModel.Node, depth: Int) {
            for (c in n.children) {
                out += c to depth
                if (c.rel in state.expanded) walk(c, depth + 1)
            }
        }
        state.root?.let { walk(it, 0) }
        out
    }

    AppScreen(
        title = stringResource(R.string.pair_skip_choose_subfolders),
        subtitle = state.pair?.name,
        navIcon = NavIcon.Close,
        onBack = { nav.back() },
        bottomBar = {
            if (state.root != null) {
                val n = state.excluded.size
                BottomActionBar(
                    hint = when (n) {
                        0 -> stringResource(R.string.pair_selective_all_sync)
                        else -> pluralStringResource(R.plurals.pair_selective_n_skipped, n, Fmt.count(n))
                    },
                ) {
                    Button(onClick = { vm.save() }, enabled = !state.saving) { Text(stringResource(R.string.common_save)) }
                }
            }
        },
    ) { padding ->
        when {
            state.loading -> Box(Modifier.padding(padding)) { LoadingState(stringResource(R.string.pair_selective_loading)) }
            state.error != null && state.root == null -> EmptyState(
                Sym.SyncProblem,
                stringResource(R.string.pair_selective_load_failed),
                state.error,
                Modifier.padding(padding),
                action = { EmptyStateAction(stringResource(R.string.common_try_again), onClick = { vm.load() }) },
            )
            visible.isEmpty() -> EmptyState(
                Sym.Folder,
                stringResource(R.string.pair_selective_none_title),
                stringResource(R.string.pair_selective_none_body),
                Modifier.padding(padding),
            )
            else -> LazyColumn(Modifier.padding(padding).fillMaxSize()) {
                item {
                    Text(
                        stringResource(R.string.pair_selective_intro),
                        Modifier.padding(start = Space.s16, end = Space.s16, top = Space.s8),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                state.remoteWarning?.let { w ->
                    item { Callout(w, Modifier.padding(start = Space.s16, end = Space.s16, top = Space.s12), tone = CalloutTone.Warning) }
                }
                state.error?.let { e ->
                    item { Callout(e, Modifier.padding(start = Space.s16, end = Space.s16, top = Space.s12), tone = CalloutTone.Danger, title = stringResource(R.string.pair_selective_save_failed)) }
                }
                item { SectionHeader(stringResource(R.string.pair_selective_subfolders)) }
                items(visible, key = { it.first.rel }) { (node, depth) ->
                    SubfolderRow(
                        node = node,
                        depth = depth,
                        state = state,
                        onToggle = { vm.toggle(node.rel) },
                        onExpand = { vm.toggleExpanded(node.rel) },
                        onShown = { vm.requestCounts(node.rel) },
                    )
                }
            }
        }
    }
}

@Composable
private fun SubfolderRow(
    node: SelectiveSyncViewModel.Node,
    depth: Int,
    state: SelectiveSyncViewModel.State,
    onToggle: () -> Unit,
    onExpand: () -> Unit,
    onShown: () -> Unit,
) {
    LaunchedEffect(node.rel) { onShown() }
    val excluded = state.isExcluded(node.rel)
    val inherited = state.isUnderExcluded(node.rel)
    val checked = !excluded && !inherited
    val expanded = node.rel in state.expanded
    val counts = state.counts[node.rel]
    val where = when {
        node.onDevice && node.onDrive -> stringResource(R.string.pair_selective_on_both)
        node.onDevice -> stringResource(R.string.pair_selective_only_phone)
        else -> stringResource(R.string.pair_selective_only_drive)
    }
    val expandLabel = stringResource(if (expanded) R.string.pair_selective_collapse else R.string.pair_selective_expand, node.name)
    val size = when {
        !node.onDevice -> null
        counts == null -> stringResource(R.string.pair_selective_counting)
        else -> Fmt.filesAndSize(counts.files, counts.bytes)
    }
    DsListItem(
        headline = node.name,
        modifier = Modifier
            .padding(start = (depth * 24).dp)
            .toggleable(value = checked, enabled = !inherited, role = Role.Checkbox, onValueChange = { onToggle() }),
        supporting = listOfNotNull(where, size).joinToString(" · "),
        leading = { Checkbox(checked = checked, onCheckedChange = null, enabled = !inherited) },
        trailing = if (node.children.isEmpty()) null else {
            {
                IconButton(onClick = onExpand) {
                    Icon(
                        Sym.KeyboardArrowDown,
                        contentDescription = expandLabel,
                        modifier = Modifier.rotate(if (expanded) 180f else 0f),
                    )
                }
            }
        },
        enabled = !inherited,
        disabledReason = if (inherited) stringResource(R.string.pair_selective_inherited) else null,
    )
}
