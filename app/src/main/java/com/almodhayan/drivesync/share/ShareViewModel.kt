package com.almodhayan.drivesync.share

import android.content.Context
import android.os.Environment
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.scheduler.SyncScheduler
import dagger.hilt.android.lifecycle.HiltViewModel
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import javax.inject.Inject

@HiltViewModel
class ShareViewModel @Inject constructor(
    @ApplicationContext private val context: Context,
    private val db: Db,
    private val scheduler: SyncScheduler,
) : ViewModel() {

    data class UiState(
        val loaded: Boolean = false,
        val hasStorageAccess: Boolean = true,
        /** Pairs whose mode uploads to Drive. */
        val pairs: List<FolderPairEntity> = emptyList(),
        val selectedPairId: String? = null,
        /** Existing first-level subfolders of the selected pair plus ones created here (relative names). */
        val subfolders: List<String> = emptyList(),
        /** "" means the top level of the pair folder. */
        val subfolder: String = "",
        val busy: Boolean = false,
        val error: String? = null,
    ) {
        val selectedPair: FolderPairEntity? get() = pairs.firstOrNull { it.id == selectedPairId }
    }

    private val _state = MutableStateFlow(UiState())
    val state: StateFlow<UiState> = _state.asStateFlow()

    /** Emits the toast text once the files were added; the activity shows it and finishes. */
    private val _done = MutableSharedFlow<String>(replay = 1)
    val done: SharedFlow<String> = _done

    init {
        viewModelScope.launch {
            val pairs = db.pairs().getAll().filter { it.mode.uploads }
            val preferred = pairs.firstOrNull { it.enabled && !it.paused } ?: pairs.firstOrNull()
            _state.update { it.copy(loaded = true, pairs = pairs, hasStorageAccess = Environment.isExternalStorageManager()) }
            preferred?.let { selectPair(it.id) }
        }
    }

    fun selectPair(id: String) {
        val pair = _state.value.pairs.firstOrNull { it.id == id } ?: return
        _state.update { it.copy(selectedPairId = id, subfolder = "", subfolders = emptyList(), error = null) }
        viewModelScope.launch {
            val subs = withContext(Dispatchers.IO) {
                File(pair.localPath).listFiles()
                    ?.filter { it.isDirectory && !it.name.startsWith(".") }
                    ?.map { it.name }
                    ?.sortedBy { it.lowercase() }
                    ?: emptyList()
            }
            _state.update { s -> if (s.selectedPairId == id) s.copy(subfolders = subs) else s }
        }
    }

    fun selectSubfolder(rel: String) {
        _state.update { it.copy(subfolder = rel) }
    }

    /** Adds a folder name to the list and selects it. The folder is created on disk when the files are added. Returns an error text or null. */
    fun newSubfolder(raw: String): String? {
        val name = ShareNaming.sanitize(raw) ?: return Strings.get(R.string.share_folder_name_needed)
        _state.update { s -> s.copy(subfolders = (s.subfolders + name).distinct().sortedBy { it.lowercase() }, subfolder = name) }
        return null
    }

    fun add(payload: SharedPayload) {
        val s = _state.value
        val pair = s.selectedPair ?: return
        if (s.busy) return
        _state.update { it.copy(busy = true, error = null) }
        viewModelScope.launch {
            try {
                val dir = if (s.subfolder.isEmpty()) File(pair.localPath) else File(pair.localPath, s.subfolder)
                val result = ShareCopier(context.contentResolver, context.packageName).copy(payload, dir)
                if (result.added > 0) scheduler.syncNow(pair.id, "share", manual = true)
                val n = result.added
                when {
                    n == 0 -> _state.update { it.copy(busy = false, error = Strings.get(R.string.share_nothing_added, result.failures.joinToString("; "))) }
                    result.failures.isNotEmpty() -> _done.tryEmit(Strings.plural(R.plurals.share_added_partial, n, n.toString(), pair.name, result.failures.size.toString()))
                    !pair.enabled -> _done.tryEmit(Strings.plural(R.plurals.share_added_turn_on, n, n.toString(), pair.name))
                    else -> _done.tryEmit(Strings.plural(R.plurals.share_added_syncing, n, n.toString(), pair.name))
                }
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _state.update { it.copy(busy = false, error = AppError.wrap(t).userHint) }
            }
        }
    }
}
