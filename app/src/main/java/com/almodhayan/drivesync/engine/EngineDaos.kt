package com.almodhayan.drivesync.engine

import android.content.Context
import com.almodhayan.drivesync.core.ErrorKind
import com.almodhayan.drivesync.data.db.BlockedPlanDao
import com.almodhayan.drivesync.data.db.BlockedPlanEntity
import com.almodhayan.drivesync.data.db.ConflictDao
import com.almodhayan.drivesync.data.db.ConflictEntity
import com.almodhayan.drivesync.data.db.FileStateDao
import com.almodhayan.drivesync.data.db.FileStateEntity
import com.almodhayan.drivesync.data.db.FolderPairDao
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.db.PairCount
import com.almodhayan.drivesync.data.db.PairTotals
import com.almodhayan.drivesync.data.db.RunStatus
import com.almodhayan.drivesync.data.db.SyncEventDao
import com.almodhayan.drivesync.data.db.SyncEventEntity
import com.almodhayan.drivesync.data.db.SyncRunDao
import com.almodhayan.drivesync.data.db.SyncRunEntity
import com.almodhayan.drivesync.data.db.TrashDao
import com.almodhayan.drivesync.data.db.TrashEntity
import com.kahflabs.wasl.ffi.WaslEngine
import com.kahflabs.wasl.ffi.engine.ConflictChoice
import com.kahflabs.wasl.ffi.store.Pair as WPair
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import java.util.concurrent.ConcurrentHashMap
import javax.inject.Inject
import javax.inject.Singleton

/**
 * The app's DAOs over libwasl: the engine's database is the only source of truth for pairs,
 * baselines, runs, conflicts and the bin. Writes that would be sync decisions (inserting runs,
 * editing baselines, adding bin entries) are the engine's alone and are refused here.
 */
@Singleton
class EngineData internal constructor(
    private val source: () -> WaslEngine,
    private val changes: Flow<Unit>,
    private val onChanged: () -> Unit,
    private val grants: FolderGrants,
) {
    @Inject constructor(runtime: WaslRuntime, @ApplicationContext context: Context) :
        this({ runtime.engine }, runtime.changes, runtime::changed, ResolverGrants(context))

    private val json = Json { ignoreUnknownKeys = true; encodeDefaults = true }
    internal val engine: WaslEngine get() = source()

    /** The app's own "files changed since the last run" counter per pair (watchers feed it). */
    private val pendingLocal = ConcurrentHashMap<String, Int>()

    /** Pairs removed with Undo still pending: hidden everywhere until the engine deletes them. */
    private val hidden = ConcurrentHashMap.newKeySet<String>()

    fun hide(pairId: String) { hidden += pairId; changed() }
    fun unhide(pairId: String) { hidden -= pairId; changed() }

    private fun WaslEngine.visiblePairs(): List<WPair> = pairs().filter { it.id !in hidden }

    internal suspend fun <T> io(block: WaslEngine.() -> T): T = withContext(Dispatchers.IO) { engine.block() }

    internal fun <T> watch(block: WaslEngine.() -> T): Flow<T> =
        changes.map { engine.block() }.flowOn(Dispatchers.IO).distinctUntilChanged()

    private fun changed() = onChanged()

    private fun refused(what: String): Nothing = throw UnsupportedOperationException("$what is the engine's to do")

    internal fun WaslEngine.entity(p: WPair): FolderPairEntity {
        val summary = if (p.status.pendingApproval) blockedPlan(p.id)?.summary?.let { json.encodeToString(com.almodhayan.drivesync.sync.PlanSummary.serializer(), it.toKotlin()) } else null
        val e = p.toEntity(pendingLocal[p.id] ?: 0, summary)
        val kind = ErrorKind.parse(e.lastErrorKind)
        val shown = ErrorKind.shown(kind, e.localPath, ::holdsGrant)
        return if (shown == kind) e else e.copy(lastErrorKind = shown?.name)
    }

    /** Whether the app still holds read and write access to the folder chosen in the system picker. */
    private fun holdsGrant(treeUri: String): Boolean = grants.held(treeUri)

    internal fun WaslEngine.pairMap(): Map<String, WPair> = pairs().associateBy { it.id }

    // ------------------------------------------------------------------ pairs

    val pairs: FolderPairDao = object : FolderPairDao {
        override fun observeAll() = watch { visiblePairs().map { entity(it) } }
        override suspend fun getAll() = io { visiblePairs().map { entity(it) } }
        override suspend fun getActive() = io { visiblePairs().filter { it.enabled && !it.paused }.map { entity(it) } }
        override suspend fun get(id: String) = io { pair(id)?.takeIf { id !in hidden }?.let { entity(it) } }
        override fun observe(id: String) = watch { pair(id)?.takeIf { id !in hidden }?.let { entity(it) } }
        override suspend fun byAccount(accountId: String) = io { visiblePairs().filter { it.accountId == accountId }.map { entity(it) } }

        override suspend fun upsert(p: FolderPairEntity) {
            io {
                val existing = pair(p.id)
                if (existing == null) createPair(p.toEngine(null)) else updatePair(p.toEngine(existing))
            }
            changed()
        }

        override suspend fun delete(id: String) {
            io {
                val path = pair(id)?.localRoot
                deletePair(id)
                // The pair is gone for good (a removal's Undo window has closed): its folder grant
                // goes when no other pair, hidden ones included, uses it. Grants are capped (#150 M5).
                if (path != null && FolderGrants.releasable(path, pairs().map { it.localRoot })) grants.release(path)
            }
            pendingLocal.remove(id)
            changed()
        }

        /** The engine records its own runs; the app only reports why a pair did not start. */
        override suspend fun setRunResult(id: String, at: Long, status: RunStatus, error: String?) {
            if (status == RunStatus.SKIPPED) { io { recordSkip(id, error ?: "") }; changed() }
        }

        override suspend fun setPendingApproval(id: String, pending: Boolean, summary: String?) {
            io { setPairFlags(id, null, pending, null) }; changed()
        }

        override suspend fun setApproveOnce(id: String, approve: Boolean) {
            io { setPairFlags(id, approve, null, null) }; changed()
        }

        override suspend fun setPaused(id: String, paused: Boolean, reason: String?) {
            io { setPaused(id, paused, reason) }; changed()
        }

        override suspend fun setEnabled(id: String, enabled: Boolean) {
            io { pair(id)?.let { updatePair(it.copy(enabled = enabled)) } }; changed()
        }

        override suspend fun setPendingLocalChanges(id: String, n: Int) { pendingLocal[id] = n; changed() }

        /** Failure counts are the engine's. */
        override suspend fun setFailure(id: String, kind: String?, n: Int) = Unit

        override suspend fun setHoldForPreview(id: String, hold: Boolean) {
            io { setPairFlags(id, null, null, hold) }; changed()
        }
    }

    // ------------------------------------------------------------------ baseline

    val fileStates: FileStateDao = object : FileStateDao {
        override suspend fun forPair(pairId: String) = io { baseline(pairId).map { it.toEntity(pairId) } }
        override fun observeFileCount(pairId: String) = watch { baselineFileCount(pairId).toInt() }
        override suspend fun get(pairId: String, key: String) = io { baselineEntry(pairId, key)?.toEntity(pairId) }
        override suspend fun byLocalRel(pairId: String, localRel: String) = io { baselineByLocalRel(pairId, localRel)?.toEntity(pairId) }
        override suspend fun upsert(s: FileStateEntity) = refused("Editing the baseline")
        override suspend fun upsertAll(s: List<FileStateEntity>) = refused("Editing the baseline")
        override suspend fun delete(pairId: String, key: String) = refused("Editing the baseline")
        override suspend fun deleteSubtree(pairId: String, prefix: String) = refused("Editing the baseline")
        /** Removing a pair removes its baseline. */
        override suspend fun clearPair(pairId: String) = Unit
        override fun observePairTotals() = watch { pairTotals().map { PairTotals(it.pairId, it.files.toInt(), it.localBytes.toLong(), it.remoteBytes.toLong()) } }
        override suspend fun pairTotals() = io { pairTotals().map { PairTotals(it.pairId, it.files.toInt(), it.localBytes.toLong(), it.remoteBytes.toLong()) } }
    }

    // ------------------------------------------------------------------ runs and events

    val runs: SyncRunDao = object : SyncRunDao {
        override suspend fun insert(r: SyncRunEntity) = refused("Recording a run")
        override suspend fun update(r: SyncRunEntity) = refused("Recording a run")
        override suspend fun get(id: String) = io { run(id)?.toEntity() }
        override fun observeRecent(limit: Int) = watch { runs(null, limit.toUInt()).map { it.toEntity() } }
        override fun observeForPair(pairId: String, limit: Int) = watch { runs(pairId, limit.toUInt()).map { it.toEntity() } }
        override suspend fun lastForPair(pairId: String) = io { runs(pairId, 1u).firstOrNull()?.toEntity() }
        override suspend fun deleteOlderThan(before: Long) { io { pruneRuns(before) }; changed() }
        override fun observeSince(since: Long) = watch { runsSince(since).map { it.toEntity() } }
        /** The engine closes runs a killed process left open when it starts. */
        override suspend fun stalePairs(before: Long): List<String> = emptyList()
        override suspend fun closeStale(before: Long, status: RunStatus, at: Long, message: String) = Unit
    }

    val events: SyncEventDao = object : SyncEventDao {
        override suspend fun insert(e: SyncEventEntity) = refused("Writing the run log")
        override suspend fun insertAll(e: List<SyncEventEntity>) = refused("Writing the run log")
        override fun observeForRun(runId: String) = watch {
            val pairId = run(runId)?.pairId ?: ""
            runEvents(runId).mapIndexed { i, e ->
                com.kahflabs.wasl.ffi.store.PairEvent(runId, pairId, (i + 1).toULong(), e).toEntity()
            }
        }
        override fun observeForPair(pairId: String, limit: Int) = watch { events(pairId, false, null, limit.toUInt()).map { it.toEntity() } }
        override fun observeProblems(limit: Int) = watch { events(null, true, null, limit.toUInt()).map { it.toEntity() } }
        override fun observeRecent(limit: Int) = watch { events(null, false, null, limit.toUInt()).map { it.toEntity() } }
        override suspend fun deleteOlderThan(before: Long) { io { pruneRuns(before) }; changed() }
        override fun observeSince(since: Long) = watch { events(null, false, since, 100_000u).map { it.toEntity() } }
        override suspend fun since(since: Long) = io { events(null, false, since, 100_000u).map { it.toEntity() }.reversed() }
        override suspend fun uploadsFor(pairId: String, path: String) = io { uploadsOf(pairId, path, 200u).map { it.toEntity() } }
    }

    // ------------------------------------------------------------------ conflicts

    val conflicts: ConflictDao = object : ConflictDao {
        override suspend fun upsert(c: ConflictEntity) = refused("Recording a conflict")
        override fun observeOpen() = watch { openConflicts(null).map { it.toEntity() } }
        override fun observeOpenCount() = watch { openConflicts(null).size }
        override suspend fun get(id: String) = io { conflict(id)?.toEntity() }
        override suspend fun resolve(id: String, at: Long) { engine.resolveConflict(id, ConflictChoice.KEEP_BOTH); changed() }
        /** Removing a pair removes its conflicts. */
        override suspend fun clearPair(pairId: String) = Unit
        override fun observeOpenForPair(pairId: String) = watch { openConflicts(pairId).map { it.toEntity() } }
        override suspend fun openForPair(pairId: String) = io { openConflicts(pairId).map { it.toEntity() } }
        override fun observeOpenCountsByPair() = watch { openConflicts(null).groupingBy { it.pairId }.eachCount().map { (k, v) -> PairCount(k, v) } }
        override suspend fun unresolve(id: String) { io { unresolveConflict(id) }; changed() }
    }

    // ------------------------------------------------------------------ recycle bin

    val trash: TrashDao = object : TrashDao {
        override suspend fun insert(t: TrashEntity) = refused("Filing into the bin")
        override suspend fun insertAll(t: List<TrashEntity>) = refused("Filing into the bin")
        override fun observeAll() = watch { val pairs = pairMap(); binEntries(null).map { it.toEntity(pairs) } }
        override suspend fun olderThan(before: Long) = io { val pairs = pairMap(); binEntriesBefore(before).map { it.toEntity(pairs) } }
        override suspend fun get(id: String) = io { binEntry(id)?.toEntity(pairMap()) }
        override fun observeTotalSize() = watch { binTotalSize().toLong() }
        override suspend fun delete(t: TrashEntity) = refused("Removing a bin entry")
        override suspend fun deleteById(id: String) = refused("Removing a bin entry")
        override suspend fun getMany(ids: List<String>) = io { val pairs = pairMap(); ids.mapNotNull { binEntry(it)?.toEntity(pairs) } }
        override suspend fun all() = io { val pairs = pairMap(); binEntries(null).map { it.toEntity(pairs) } }
        override suspend fun forKey(pairId: String, key: String) = io { val pairs = pairMap(); binVersions(pairId, key).map { it.toEntity(pairs) } }
    }

    // ------------------------------------------------------------------ stopped plans

    val blockedPlans: BlockedPlanDao = object : BlockedPlanDao {
        private fun WaslEngine.entity(pairId: String): BlockedPlanEntity? = blockedPlan(pairId)?.let { b ->
            val summary = b.summary?.toKotlin() ?: com.almodhayan.drivesync.sync.PlanSummary(blockedReason = b.reason)
            blockedEntity(pairId, b.createdAt, json.encodeToString(com.almodhayan.drivesync.sync.PlanSummary.serializer(), summary))
        }
        override suspend fun upsert(b: BlockedPlanEntity) = refused("Stopping a plan")
        override suspend fun get(pairId: String) = io { entity(pairId) }
        override fun observe(pairId: String) = watch { entity(pairId) }
        override fun observeAll() = watch { pairs().mapNotNull { entity(it.id) } }
        override suspend fun delete(pairId: String) { io { clearBlockedPlan(pairId) }; changed() }
    }
}
