package com.almodhayan.drivesync.ui.pair

import com.almodhayan.drivesync.crypto.VaultManifest
import android.content.Context
import com.almodhayan.drivesync.engine.SafBridgeImpl
import com.almodhayan.drivesync.engine.WaslRuntime
import com.kahflabs.wasl.ffi.core.Route
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.actions.PairActions
import com.almodhayan.drivesync.backup.ConfigBackup
import com.almodhayan.drivesync.backup.scheduleSoon
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Glob
import com.almodhayan.drivesync.core.Ids
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.db.AccountEntity
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.db.SyncMode
import com.almodhayan.drivesync.data.db.VaultEntity
import com.almodhayan.drivesync.data.prefs.AppSettings
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import com.almodhayan.drivesync.devices.DeviceSyncManager
import com.almodhayan.drivesync.drive.DriveApi
import com.almodhayan.drivesync.drive.DriveNodeCache
import com.almodhayan.drivesync.fs.LocalFs
import com.almodhayan.drivesync.scheduler.SyncScheduler
import com.almodhayan.drivesync.ui.components.FileKind
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.PairDirection
import dagger.hilt.android.lifecycle.HiltViewModel
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.onStart
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import javax.inject.Inject

@HiltViewModel
class PairEditorViewModel @Inject constructor(
    handle: SavedStateHandle,
    @ApplicationContext private val context: Context,
    private val db: Db,
    private val settings: SettingsRepository,
    private val scheduler: SyncScheduler,
    private val localFs: LocalFs,
    private val cache: DriveNodeCache,
    private val api: DriveApi,
    private val backup: ConfigBackup,
    private val pairActions: PairActions,
    private val removal: com.almodhayan.drivesync.ui.home.PairRemoval,
    private val saf: SafBridgeImpl,
    private val devices: DeviceSyncManager,
    private val runtime: WaslRuntime,
) : ViewModel() {

    /** Null when creating a new pair (route argument "new"). */
    val pairId: String? = handle.get<String>("pairId")?.takeIf { it.isNotBlank() && it != "new" }
    private val presetAccountId: String? = handle.get<String>("accountId")?.takeIf { it.isNotBlank() }
    val isNew: Boolean get() = pairId == null

    /** A folder another device offers (docs/INTRODUCTIONS.md §3.3): the new pair syncs it with that device once saved. */
    data class Offer(val share: String, val from: String, val name: String)
    val offer: Offer? = if (pairId != null) null else handle.get<String>("offerShare")?.takeIf { it.isNotBlank() }?.let { share ->
        handle.get<String>("offerFrom")?.takeIf { it.isNotBlank() }?.let { from -> Offer(share, from, handle.get<String>("offerName").orEmpty()) }
    }

    /** What the phone folder holds: everything, and without hidden files and folders. */
    data class PhoneStats(val all: FolderCount, val visible: FolderCount)

    /** Another pair already syncs [path] (the deeper of the two folders). */
    data class Overlap(val pairName: String, val path: String)

    enum class After { Close, Preview }
    data class Saved(val id: String, val after: After)

    data class State(
        val loaded: Boolean = false,
        val missing: Boolean = false,
        val draft: FolderPairEntity? = null,
        /** The stored pair when editing; null for a new pair. */
        val original: FolderPairEntity? = null,
        val nameEdited: Boolean = false,
        val skipText: String = "",
        val includeText: String = "",
        val sizeMbText: String = "",
        val cfg: AppSettings = AppSettings(),
        val accounts: List<AccountEntity> = emptyList(),
        val vaults: List<VaultEntity> = emptyList(),
        val otherPairs: List<FolderPairEntity> = emptyList(),
        val hasAllFilesAccess: Boolean = true,
        val localExists: Boolean = true,
        val phone: PhoneStats? = null,
        val phoneCounting: Boolean = false,
        val drive: FolderCount? = null,
        val driveCounting: Boolean = false,
        val driveHasFiles: Boolean = false,
        /**
         * The chosen Drive folder holds a vault.json: it is an encrypted vault, perhaps made on
         * another device. Null when it holds none.
         */
        val driveVault: DriveVault? = null,
        /** Ancestors of the chosen Drive folder in the metadata cache, nearest first. */
        val driveChain: List<String> = emptyList(),
        /** Other pair id → ancestors of its Drive folder. */
        val otherDriveChains: Map<String, List<String>> = emptyMap(),
        val showErrors: Boolean = false,
        val failedSaves: Int = 0,
        val saving: Boolean = false,
        val saveError: String? = null,
        val saved: Saved? = null,
        val removed: Boolean = false,
        /** The stored pair is syncing now: the engine refuses a change of its folders until it stops (#104). */
        val syncingNow: Boolean = false,
        /** The stored pair syncs with devices: a change of its folders stops that until it is shared again. */
        val sharedWithDevices: Boolean = false,
    ) {
        val direction: PairDirection get() = draft?.let { PairDirection.of(it.mode) } ?: PairDirection.TwoWay
        val mirror: Boolean get() = draft?.mode == SyncMode.UPLOAD_MIRROR || draft?.mode == SyncMode.DOWNLOAD_MIRROR
        val moveAfter: Boolean get() = draft?.mode == SyncMode.UPLOAD_THEN_DELETE || draft?.mode == SyncMode.DOWNLOAD_THEN_DELETE

        /** Whether this mode can delete anything at all, so the large-delete question applies. */
        val deletionsPossible: Boolean get() = mirror || (direction == PairDirection.TwoWay && draft?.propagateDeletions == true)

        val hasSynced: Boolean get() = original?.lastSuccessAt != null
        val foldersChanged: Boolean get() { val o = original ?: return false; val d = draft ?: return false; return PairEditorViewModel.foldersDiffer(o, d) }
        val encryptionChanged: Boolean get() { val o = original ?: return false; val d = draft ?: return false; return PairEditorViewModel.encryptionDiffers(o, d) }

        /** The next sync is a first sync: new, never synced, or starting over after a folder or vault change. */
        val firstSync: Boolean get() = !hasSynced || foldersChanged || encryptionChanged

        val phoneCount: FolderCount? get() = phone?.let { p -> if (draft?.syncHidden == true) p.all else p.visible }

        /** Roughly what the first sync transfers, for the mobile-data warning. */
        val firstSyncBytes: Long?
            get() {
                val phoneBytes = phoneCount?.bytes
                val driveBytes = drive?.bytes
                return when (direction) {
                    PairDirection.PhoneToDrive -> phoneBytes
                    PairDirection.DriveToPhone -> driveBytes
                    PairDirection.TwoWay -> if (phoneBytes == null && driveBytes == null) null else maxOf(phoneBytes ?: 0L, driveBytes ?: 0L)
                }
            }

        val videosSkipped: Boolean
            get() {
                val set = PatternText.parse(skipText).map { it.lowercase() }.toSet()
                return PairEditorViewModel.VIDEO_GLOBS.all { it in set }
            }

        /** Same phone folder, a parent of another pair's folder, or a folder inside one. */
        val phoneOverlap: Overlap?
            get() {
                val d = draft ?: return null
                val mine = comparablePath(d.localPath)
                if (mine.isEmpty()) return null
                for (p in otherPairs) {
                    val theirs = comparablePath(p.localPath)
                    if (theirs.isEmpty()) continue
                    val theirsInside = theirs.startsWith("$mine/", ignoreCase = true)
                    if (theirs.equals(mine, ignoreCase = true) || theirsInside || mine.startsWith("$theirs/", ignoreCase = true)) {
                        return Overlap(p.name, Fmt.humanLocalPath(if (theirsInside) theirs else mine))
                    }
                }
                return null
            }

        /** The same check on the Drive side, within one Google account. */
        val driveOverlap: Overlap?
            get() {
                val d = draft ?: return null
                val mine = d.remoteFolderId
                if (mine.isBlank()) return null
                for (p in otherPairs) {
                    if (p.accountId != d.accountId || p.remoteFolderId.isBlank()) continue
                    val theirsInside = otherDriveChains[p.id]?.contains(mine) == true
                    if (p.remoteFolderId == mine || theirsInside || p.remoteFolderId in driveChain) {
                        return Overlap(p.name, Fmt.humanRemotePath(if (theirsInside) p.remotePath else d.remotePath))
                    }
                }
                return null
            }

        /** An overlap blocks Save and the preview until another folder is chosen. */
        val blocked: Boolean get() = phoneOverlap != null || driveOverlap != null

        // ---- problems, each worded as the fix

        val nameProblem: String? get() = if (draft != null && draft.name.isBlank()) Strings.get(R.string.pair_problem_name) else null

        /** Shown at once, because they describe the folder as it is now. */
        val phoneLiveProblem: String?
            get() = when {
                draft == null || draft.localPath.isBlank() -> null
                !isSaf(draft.localPath) && !hasAllFilesAccess -> Strings.get(R.string.pair_problem_no_access)
                !localExists -> Strings.get(R.string.pair_problem_folder_gone)
                else -> null
            }

        val phoneProblem: String? get() = phoneLiveProblem ?: if (draft != null && draft.localPath.isBlank()) Strings.get(R.string.pair_problem_choose_phone_folder) else null

        val driveProblem: String?
            get() = when {
                draft == null -> null
                draft.accountId.isBlank() -> Strings.get(R.string.pair_problem_choose_account)
                draft.remoteFolderId.isBlank() -> Strings.get(R.string.pair_problem_choose_drive_folder)
                else -> null
            }

        val skipProblem: String? get() = badPattern(skipText)
        val includeProblem: String? get() = badPattern(includeText)
        val vaultProblem: String?
            get() = when {
                draft == null -> null
                draft.encrypted && draft.vaultId.isNullOrBlank() -> Strings.get(R.string.pair_problem_choose_vault)
                // A plain pair on a vault's folder would download its scrambled files and upload
                // plain ones beside them (E2E 2026-10): only that vault may sync there.
                driveVault != null && driveVault.vaultId == null -> Strings.get(R.string.pair_problem_drive_is_unknown_vault)
                driveVault != null && (!draft.encrypted || draft.vaultId != driveVault.vaultId) ->
                    Strings.get(R.string.pair_problem_drive_is_vault, vaults.firstOrNull { it.id == driveVault.vaultId }?.name ?: "")
                else -> null
            }

        val problems: List<String> get() = listOfNotNull(nameProblem, phoneProblem, driveProblem, skipProblem, includeProblem, vaultProblem)

        private fun badPattern(text: String): String? =
            PatternText.parse(text).firstOrNull { p -> runCatching { Glob.compile(p) }.isFailure }?.let { Strings.get(R.string.pair_problem_bad_pattern, it) }
    }

    /** A vault.json in the chosen Drive folder, and the vault on this phone it belongs to, if any. */
    data class DriveVault(val vaultId: String?)

    private val _state = MutableStateFlow(State())
    val state: StateFlow<State> = _state

    private var phoneJob: Job? = null
    private var driveJob: Job? = null
    private var snapshot: DriveNodeCache.Snapshot? = null
    /** The pair removed here, while its Undo is offered. */
    private var removedPair: com.almodhayan.drivesync.ui.home.PairRemoval.Removed? = null

    init {
        viewModelScope.launch { load() }
        viewModelScope.launch {
            db.accounts().observeAll().collect { list -> _state.update { s -> s.copy(accounts = list, draft = s.draft?.let { withDefaultAccount(it, list) }) } }
        }
        viewModelScope.launch { db.vaults().observeAll().collect { list -> _state.update { it.copy(vaults = list) } } }
        viewModelScope.launch {
            db.pairs().observeAll().collect { list ->
                _state.update { s -> s.copy(otherPairs = list.filter { p -> p.id != (s.draft?.id ?: pairId) }) }
                updateOtherChains()
            }
        }
        viewModelScope.launch { settings.settings.collect { c -> _state.update { it.copy(cfg = c) } } }
        pairId?.let { id ->
            viewModelScope.launch {
                runtime.changes.onStart { emit(Unit) }.collect {
                    val (running, shared) = withContext(Dispatchers.IO) {
                        runCatching { runtime.engine.isRunning(id) to (runtime.engine.pair(id)?.route?.let { it != Route.DRIVE } ?: false) }
                            .getOrDefault(false to false)
                    }
                    _state.update { it.copy(syncingNow = running, sharedWithDevices = shared) }
                }
            }
        }
    }

    private suspend fun load() {
        val cfg = settings.current()
        val existing = pairId?.let { db.pairs().get(it) }
        if (pairId != null && existing == null) {
            _state.update { it.copy(loaded = true, missing = true, cfg = cfg) }
            return
        }
        val draft = existing ?: newDraft(cfg)
        _state.update { s ->
            s.copy(
                loaded = true,
                cfg = cfg,
                original = existing,
                draft = withDefaultAccount(draft, s.accounts),
                nameEdited = existing != null || !offer?.name.isNullOrBlank(),
                skipText = PatternText.toUi(draft.excludeList()),
                includeText = PatternText.toUi(draft.includeList()),
                sizeMbText = (draft.maxFileSizeBytes / MB).toString(),
                hasAllFilesAccess = localFs.hasAllFilesAccess(),
                otherPairs = s.otherPairs.filter { it.id != draft.id },
            )
        }
        refreshPhone()
        refreshDrive()
    }

    /** New pairs start from the safety defaults in Settings. */
    private fun newDraft(cfg: AppSettings) = FolderPairEntity(
        id = Ids.newId(),
        accountId = presetAccountId ?: "",
        name = offer?.name.orEmpty(),
        localPath = "",
        remoteFolderId = "",
        remotePath = "",
        wifiOnly = cfg.wifiOnlyDefault,
        allowMetered = !cfg.wifiOnlyDefault,
        requireCharging = cfg.requireChargingDefault,
        minBatteryPercent = cfg.minBatteryDefault,
        conflictPolicy = cfg.conflictPolicyDefault,
        docsFormat = cfg.docsFormatDefault,
        maxFileSizeBytes = cfg.maxFileSizeDefaultMb.coerceAtLeast(0).toLong() * MB,
        massDeleteGuard = cfg.massDeleteGuardDefault,
        syncHidden = false,
        holdForPreview = true,
    )

    private fun withDefaultAccount(d: FolderPairEntity, accounts: List<AccountEntity>): FolderPairEntity =
        if (d.accountId.isBlank() && accounts.size == 1) d.copy(accountId = accounts[0].id) else d

    // ------------------------------------------------------------------ edits

    fun update(f: (FolderPairEntity) -> FolderPairEntity) {
        _state.update { s -> s.copy(draft = s.draft?.let(f), saveError = null) }
    }

    fun setName(name: String) {
        _state.update { s -> s.copy(draft = s.draft?.copy(name = name), nameEdited = name.isNotBlank()) }
    }

    fun setAccount(accountId: String) {
        val d = _state.value.draft ?: return
        if (d.accountId == accountId) return
        _state.update { s -> s.copy(draft = s.draft?.copy(accountId = accountId, remoteFolderId = "", remotePath = "", driveId = null)) }
        refreshDrive()
    }

    /**
     * A folder chosen in the system picker (SAF mode, SRS FR-R3): keeps read and write access across
     * restarts, then uses the tree URI as the pair's phone folder.
     */
    fun onSafPicked(uri: android.net.Uri) {
        val flags = android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION or android.content.Intent.FLAG_GRANT_WRITE_URI_PERMISSION
        try {
            context.contentResolver.takePersistableUriPermission(uri, flags)
        } catch (e: SecurityException) {
            // Kept silently, the pair worked until the next restart, then failed with "Permission
            // denied" (#150 M5). Say so now and keep the folder unchosen.
            _state.update { it.copy(saveError = Strings.get(R.string.pair_editor_grant_failed)) }
            return
        }
        _state.update { it.copy(saveError = null) }
        onLocalPicked(uri.toString())
    }

    /** Whether the folder is there and, for a SAF folder, still granted to the app. */
    private fun phoneFolderExists(path: String): Boolean = if (isSaf(path)) {
        context.contentResolver.persistedUriPermissions.any { it.uri.toString() == path && it.isReadPermission && it.isWritePermission } &&
            runCatching { saf.stat(path, "") != null }.getOrDefault(false)
    } else File(path).isDirectory

    fun onLocalPicked(path: String) {
        _state.update { s ->
            val d = s.draft ?: return@update s
            val next = d.copy(localPath = if (isSaf(path)) path else path.trimEnd('/').ifEmpty { path })
            s.copy(draft = if (s.nameEdited) next else next.copy(name = suggestName(next)))
        }
        refreshPhone()
    }

    fun onRemotePicked(folderId: String, remotePath: String, driveId: String) {
        _state.update { s ->
            val d = s.draft ?: return@update s
            val next = d.copy(remoteFolderId = folderId, remotePath = remotePath, driveId = driveId.takeIf { it.isNotEmpty() })
            s.copy(draft = if (s.nameEdited) next else next.copy(name = suggestName(next)))
        }
        refreshDrive()
    }

    /** Picks one of the three directions; one-way modes start without mirror or move, the safe variant. */
    fun setDirection(direction: PairDirection) {
        update { d -> if (PairDirection.of(d.mode) == direction) d else d.copy(mode = modeFor(direction, mirror = false, move = false)) }
    }

    /** Mirror deletions and Remove after transfer are mutually exclusive variants of a one-way mode. */
    fun setMirror(on: Boolean) = update { d -> d.copy(mode = modeFor(PairDirection.of(d.mode), mirror = on, move = false)) }

    fun setMoveAfter(on: Boolean) = update { d -> d.copy(mode = modeFor(PairDirection.of(d.mode), mirror = false, move = on)) }

    /** Turning Wi‑Fi only off means mobile data is allowed; the advanced metered switch can narrow it again. */
    fun setWifiOnly(on: Boolean) = update { it.copy(wifiOnly = on, allowMetered = !on) }

    fun toggleHidden() = update { it.copy(syncHidden = !it.syncHidden) }

    fun setSkipText(text: String) { _state.update { it.copy(skipText = text, saveError = null) } }

    fun setIncludeText(text: String) { _state.update { it.copy(includeText = text, saveError = null) } }

    /** Adds or removes "*.mp4", "*.mov" and the other video patterns. */
    fun toggleVideos() {
        _state.update { s ->
            val list = PatternText.parse(s.skipText).toMutableList()
            if (s.videosSkipped) {
                list.removeAll { it.lowercase() in VIDEO_SET }
            } else {
                VIDEO_GLOBS.forEach { g -> if (list.none { it.equals(g, ignoreCase = true) }) list += g }
            }
            s.copy(skipText = PatternText.toUi(list))
        }
    }

    /** The "Over 2 GB" chip: on sets the 2 GB limit, off removes any limit. */
    fun toggleSizeLimit() {
        _state.update { s ->
            val d = s.draft ?: return@update s
            val bytes = if (d.maxFileSizeBytes > 0) 0L else DEFAULT_SIZE_LIMIT
            s.copy(draft = d.copy(maxFileSizeBytes = bytes), sizeMbText = (bytes / MB).toString())
        }
    }

    fun setSizeMb(text: String) {
        val clean = text.filter { it.isDigit() }.take(7)
        _state.update { s -> s.copy(sizeMbText = clean, draft = s.draft?.copy(maxFileSizeBytes = (clean.toLongOrNull() ?: 0L) * MB)) }
    }

    fun showErrors() { _state.update { it.copy(showErrors = true) } }

    fun dismissSaved() { _state.update { it.copy(saved = null) } }

    /** On resume: access may have been granted, the folder moved, or Choose subfolders saved. */
    fun recheck() {
        val access = localFs.hasAllFilesAccess()
        val accessChanged = access != _state.value.hasAllFilesAccess
        _state.update { it.copy(hasAllFilesAccess = access) }
        viewModelScope.launch {
            val path = _state.value.draft?.localPath.orEmpty()
            if (path.isNotBlank()) {
                val exists = withContext(Dispatchers.IO) { phoneFolderExists(path) }
                if (accessChanged || exists != _state.value.localExists) refreshPhone()
            }
            val id = pairId ?: return@launch
            val fresh = db.pairs().get(id) ?: return@launch
            _state.update { s -> s.copy(draft = s.draft?.copy(excludedSubfolders = fresh.excludedSubfolders)) }
        }
    }

    private fun suggestName(d: FolderPairEntity): String {
        val local = (if (isSaf(d.localPath)) Fmt.humanLocalPath(d.localPath).substringAfterLast(" / ") else d.localPath.trimEnd('/').substringAfterLast('/'))
        val remote = d.remotePath.trimEnd('/').substringAfterLast('/')
        return when {
            local.isNotEmpty() && remote.isNotEmpty() && !local.equals(remote, ignoreCase = true) -> "$local ↔ $remote"
            local.isNotEmpty() -> local
            else -> remote
        }
    }

    // ------------------------------------------------------------------ folder facts

    private fun refreshPhone() {
        phoneJob?.cancel()
        val path = _state.value.draft?.localPath.orEmpty()
        if (path.isBlank()) {
            _state.update { it.copy(phone = null, phoneCounting = false, localExists = true) }
            return
        }
        _state.update { it.copy(phoneCounting = true, phone = null) }
        phoneJob = viewModelScope.launch {
            val stats = withContext(Dispatchers.IO) {
                when {
                    isSaf(path) -> if (phoneFolderExists(path)) countSaf(path) else null
                    File(path).isDirectory -> countPhone(File(path))
                    else -> null
                }
            }
            _state.update { s -> if (s.draft?.localPath != path) s else s.copy(phone = stats, localExists = stats != null, phoneCounting = false) }
        }
    }

    private suspend fun countSaf(tree: String): PhoneStats {
        var files = 0
        var bytes = 0L
        var visibleFiles = 0
        var visibleBytes = 0L
        val stack = ArrayDeque<Pair<String, Boolean>>()
        stack.addLast("" to false)
        while (stack.isNotEmpty()) {
            currentCoroutineContext().ensureActive()
            val (dir, hidden) = stack.removeLast()
            val kids = runCatching { saf.list(tree, dir) }.getOrNull() ?: continue
            for (e in kids) {
                val h = hidden || e.name.startsWith(".")
                if (e.isDir) {
                    stack.addLast((if (dir.isEmpty()) e.name else "$dir/${e.name}") to h)
                } else {
                    val len = e.size.toLong()
                    files++; bytes += len
                    if (!h) { visibleFiles++; visibleBytes += len }
                }
            }
        }
        return PhoneStats(FolderCount(files, bytes), FolderCount(visibleFiles, visibleBytes))
    }

    private suspend fun countPhone(root: File): PhoneStats {
        var files = 0
        var bytes = 0L
        var visibleFiles = 0
        var visibleBytes = 0L
        val stack = ArrayDeque<Pair<File, Boolean>>()
        stack.addLast(root to false)
        while (stack.isNotEmpty()) {
            currentCoroutineContext().ensureActive()
            val (dir, hidden) = stack.removeLast()
            val kids = dir.listFiles() ?: continue
            for (f in kids) {
                val h = hidden || f.name.startsWith(".")
                if (f.isDirectory) {
                    stack.addLast(f to h)
                } else if (f.isFile) {
                    val len = f.length()
                    files++; bytes += len
                    if (!h) { visibleFiles++; visibleBytes += len }
                }
            }
        }
        return PhoneStats(FolderCount(files, bytes), FolderCount(visibleFiles, visibleBytes))
    }

    private fun refreshDrive() {
        driveJob?.cancel()
        val d = _state.value.draft ?: return
        val folderId = d.remoteFolderId
        if (folderId.isBlank() || d.accountId.isBlank()) {
            _state.update { it.copy(drive = null, driveCounting = false, driveHasFiles = false, driveChain = emptyList()) }
            return
        }
        _state.update { it.copy(driveCounting = true, drive = null, driveHasFiles = false, driveVault = null) }
        driveJob = viewModelScope.launch {
            try {
                checkDriveVault(d.accountId, folderId, d.driveId)
                val account = db.accounts().get(d.accountId)
                var counted = false
                if (account?.lastCrawlAt != null) {
                    try { cache.refresh(d.accountId, d.driveId) } catch (e: CancellationException) { throw e } catch (_: Throwable) { }
                    val snap = withContext(Dispatchers.IO) { cache.snapshot(d.accountId) }
                    snapshot = snap
                    val totals = DriveTotals(snap)
                    if (totals.knows(folderId)) {
                        val count = withContext(Dispatchers.Default) { totals.of(folderId) }
                        val chain = DriveTotals.ancestors(snap, folderId)
                        _state.update { s ->
                            if (s.draft?.remoteFolderId != folderId) s
                            else s.copy(drive = count, driveHasFiles = count.files > 0, driveChain = chain, driveCounting = false)
                        }
                        counted = true
                    }
                    updateOtherChains()
                }
                if (!counted) {
                    // Not indexed yet: ask Drive directly whether the folder has anything in it.
                    val kids = api.listChildren(d.accountId, folderId, d.driveId)
                    _state.update { s ->
                        if (s.draft?.remoteFolderId != folderId) s
                        else s.copy(drive = null, driveHasFiles = kids.isNotEmpty(), driveCounting = false)
                    }
                }
            } catch (e: CancellationException) {
                throw e
            } catch (_: Throwable) {
                _state.update { it.copy(driveCounting = false) }
            }
        }
    }

    /**
     * Looks for a vault.json in the chosen Drive folder (asking Drive, not the cache, which can lag
     * behind a vault just made on another device). One that belongs to a vault on this phone turns
     * encryption on with that vault; any other blocks saving until the vault is joined.
     */
    private suspend fun checkDriveVault(accountId: String, folderId: String, driveId: String?) {
        val manifest = api.listChildren(accountId, folderId, driveId)
            .firstOrNull { !it.isFolder && it.name == VaultManifest.FILE_NAME }
        if (manifest == null) return
        val text = runCatching {
            val buf = ByteArray(MANIFEST_MAX_BYTES)
            val n = api.readRange(accountId, manifest.id, 0, buf.size, buf)
            String(buf, 0, n, Charsets.UTF_8)
        }.getOrNull()
        val keyCheck = text?.let { KEY_CHECK.find(it)?.groupValues?.get(1) }
        val match = keyCheck?.let { k -> _state.value.vaults.firstOrNull { v -> v.keyCheckB64 == k || v.manifestJson?.contains(k) == true } }
        _state.update { s ->
            val draft = s.draft
            if (draft?.remoteFolderId != folderId) s
            else s.copy(
                driveVault = DriveVault(match?.id),
                draft = if (match != null && draft.vaultId.isNullOrBlank()) draft.copy(encrypted = true, vaultId = match.id) else draft,
            )
        }
    }

    private fun updateOtherChains() {
        val snap = snapshot ?: return
        val s = _state.value
        val account = s.draft?.accountId ?: return
        if (snap.accountId != account) return
        val chains = s.otherPairs
            .filter { it.accountId == account && it.remoteFolderId.isNotBlank() }
            .associate { it.id to DriveTotals.ancestors(snap, it.remoteFolderId) }
        _state.update { it.copy(otherDriveChains = chains) }
    }

    // ------------------------------------------------------------------ save

    fun save(after: After) {
        val s = _state.value
        val d = s.draft ?: return
        if (s.saving || s.blocked || s.removed) return
        if (s.problems.isNotEmpty()) {
            _state.update { it.copy(showErrors = true, failedSaves = it.failedSaves + 1) }
            return
        }
        _state.update { it.copy(saving = true, saveError = null) }
        viewModelScope.launch {
            try {
                val edited = d.copy(
                    name = d.name.trim(),
                    localPath = d.localPath.trimEnd('/').ifEmpty { d.localPath },
                    ssidAllow = d.ssidAllowList().joinToString(", "),
                    ssidDeny = d.ssidDenyList().joinToString(", "),
                    includePatterns = PatternText.parse(s.includeText).joinToString("\n"),
                    excludePatterns = PatternText.parse(s.skipText).joinToString("\n"),
                    vaultId = if (d.encrypted) d.vaultId else null,
                    updatedAt = System.currentTimeMillis(),
                )
                val entity = withContext(Dispatchers.IO) {
                    val current = if (s.original == null) null else db.pairs().get(d.id)
                    val merged = if (current == null) edited else mergeEditable(current, edited)
                    db.pairs().upsert(merged)
                    if (current != null && startsOver(current, merged)) {
                        // The old sync records describe other files: the next run is a fresh first sync.
                        db.fileStates().clearPair(merged.id)
                        db.conflicts().clearPair(merged.id)
                        db.blockedPlans().delete(merged.id)
                        db.pairs().setPendingApproval(merged.id, false, null)
                        db.pairs().setHoldForPreview(merged.id, true)
                    }
                    db.pairs().get(merged.id) ?: merged
                }
                // Accepting an offer: the pair syncs the offered folder with the offering device only,
                // and with Drive as every pair on the phone does (docs/INTRODUCTIONS.md §3.3).
                offer?.let { o ->
                    withContext(Dispatchers.IO) {
                        try {
                            devices.withDevices { it.acceptOffer(entity.id, o.share, o.from) }
                                ?: throw AppError.Unknown(Strings.get(R.string.pair_editor_accept_devices_closed))
                        } catch (t: Throwable) {
                            // Not accepted: a pair made only for this offer goes again, so nothing
                            // is left half done (review of #28).
                            if (s.original == null) db.pairs().delete(entity.id)
                            throw t
                        }
                    }
                }
                scheduler.ensurePeriodic()
                if (s.cfg.autoBackupConfig) backup.scheduleSoon(context)
                _state.update { it.copy(saving = false, draft = entity, saved = Saved(entity.id, after)) }
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _state.update { it.copy(saving = false, saveError = AppError.wrap(t).userHint) }
            }
        }
    }

    // ------------------------------------------------------------------ remove, with undo

    /** Removes the pair and its sync records. Files on the phone and on Drive are not touched. */
    fun removePair() {
        val id = pairId ?: return
        if (_state.value.removed) return
        viewModelScope.launch {
            try {
                // Hidden now, deleted when the Undo window closes: deleting at once and putting the
                // pair back on Undo lost its folder grant (#150 M5 review).
                removedPair = withContext(Dispatchers.IO) { removal.remove(id) }
                _state.update { it.copy(removed = true) }
                if (_state.value.cfg.autoBackupConfig) backup.scheduleSoon(context)
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _state.update { it.copy(saveError = AppError.wrap(t).userHint) }
            }
        }
    }

    fun undoRemove() {
        val kept = removedPair ?: return
        viewModelScope.launch {
            try {
                withContext(Dispatchers.IO) { removal.restore(kept) }
                removedPair = null
                _state.update { it.copy(removed = false) }
                if (_state.value.cfg.autoBackupConfig) backup.scheduleSoon(context)
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _state.update { it.copy(saveError = AppError.wrap(t).userHint) }
            }
        }
    }

    companion object {
        const val MB = 1024L * 1024L
        /** A vault.json is a few hundred bytes; read at most this much of one. */
        private const val MANIFEST_MAX_BYTES = 64 * 1024
        private val KEY_CHECK = Regex("\"keyCheck\"\\s*:\\s*\"([^\"]+)\"")
        /** The "Over 2 GB" chip. */
        const val DEFAULT_SIZE_LIMIT = 2048L * MB
        /** Above this, a first sync on mobile data gets a warning. */
        const val LARGE_FIRST_SYNC = 100L * MB
        val INTERVAL_CHOICES = listOf(0, 15, 30, 60, 120, 180, 360, 720, 1440)
        val VIDEO_GLOBS: List<String> = FileKind.videoExtensions.map { "*.$it" }
        private val VIDEO_SET: Set<String> = VIDEO_GLOBS.toSet()

        fun modeFor(direction: PairDirection, mirror: Boolean, move: Boolean): SyncMode = when (direction) {
            PairDirection.TwoWay -> SyncMode.TWO_WAY
            PairDirection.PhoneToDrive -> when {
                mirror -> SyncMode.UPLOAD_MIRROR
                move -> SyncMode.UPLOAD_THEN_DELETE
                else -> SyncMode.UPLOAD_ONLY
            }
            PairDirection.DriveToPhone -> when {
                mirror -> SyncMode.DOWNLOAD_MIRROR
                move -> SyncMode.DOWNLOAD_THEN_DELETE
                else -> SyncMode.DOWNLOAD_ONLY
            }
        }

        fun foldersDiffer(old: FolderPairEntity, new: FolderPairEntity): Boolean =
            !old.localPath.trimEnd('/').equals(new.localPath.trimEnd('/'), ignoreCase = true) ||
                old.remoteFolderId != new.remoteFolderId || old.accountId != new.accountId || old.driveId != new.driveId

        fun encryptionDiffers(old: FolderPairEntity, new: FolderPairEntity): Boolean =
            old.encrypted != new.encrypted || (new.encrypted && old.vaultId != new.vaultId)

        /** A different folder, account or vault means the stored sync records describe other files. */
        fun startsOver(old: FolderPairEntity, new: FolderPairEntity): Boolean = foldersDiffer(old, new) || encryptionDiffers(old, new)

        /** The editor's settings on top of the stored row, so run results written meanwhile are kept. */
        fun mergeEditable(current: FolderPairEntity, e: FolderPairEntity): FolderPairEntity {
            val base = if (!startsOver(current, e)) current else current.copy(
                lastRunAt = null, lastSuccessAt = null, lastStatus = null, lastError = null, lastErrorKind = null, consecutiveFailures = 0,
                pendingApproval = false, pendingApprovalSummary = null, approveOnce = false, pendingLocalChanges = 0,
                excludedSubfolders = if (foldersDiffer(current, e)) "" else current.excludedSubfolders,
            )
            return base.copy(
                accountId = e.accountId, name = e.name, localPath = e.localPath,
                remoteFolderId = e.remoteFolderId, remotePath = e.remotePath, driveId = e.driveId, mode = e.mode,
                wifiOnly = e.wifiOnly, allowMetered = e.allowMetered, allowRoaming = e.allowRoaming, requireCharging = e.requireCharging,
                minBatteryPercent = e.minBatteryPercent, ssidAllow = e.ssidAllow, ssidDeny = e.ssidDeny, requireVpn = e.requireVpn,
                includePatterns = e.includePatterns, excludePatterns = e.excludePatterns, syncHidden = e.syncHidden,
                maxFileSizeBytes = e.maxFileSizeBytes, conflictPolicy = e.conflictPolicy, propagateDeletions = e.propagateDeletions,
                massDeleteGuard = e.massDeleteGuard, docsExport = e.docsExport, docsFormat = e.docsFormat, preserveMtime = e.preserveMtime,
                encrypted = e.encrypted, vaultId = e.vaultId, intervalMinutes = e.intervalMinutes, instantSync = e.instantSync,
                updatedAt = e.updatedAt,
            )
        }
    }
}

private fun isSaf(path: String) = path.startsWith("content://")

/** A phone folder as a file path, so a SAF folder and a path pair can be checked for overlap. */
private fun comparablePath(path: String): String = (if (isSaf(path)) Fmt.safFilePath(path) ?: path else path).trimEnd('/')
