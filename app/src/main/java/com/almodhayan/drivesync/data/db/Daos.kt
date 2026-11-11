package com.almodhayan.drivesync.data.db

import androidx.room.Dao
import androidx.room.Delete
import androidx.room.Insert
import androidx.room.OnConflictStrategy
import androidx.room.Query
import androidx.room.Transaction
import androidx.room.Update
import androidx.room.Upsert
import kotlinx.coroutines.flow.Flow

@Dao
interface AccountDao {
    @Query("SELECT * FROM accounts ORDER BY addedAt") fun observeAll(): Flow<List<AccountEntity>>
    @Query("SELECT * FROM accounts ORDER BY addedAt") suspend fun getAll(): List<AccountEntity>
    @Query("SELECT * FROM accounts WHERE id = :id") suspend fun get(id: String): AccountEntity?
    @Query("SELECT * FROM accounts WHERE id = :id") fun observe(id: String): Flow<AccountEntity?>
    @Upsert suspend fun upsert(a: AccountEntity)
    @Query("DELETE FROM accounts WHERE id = :id") suspend fun delete(id: String)
    @Query("UPDATE accounts SET changesToken = :token, lastDeltaAt = :at WHERE id = :id") suspend fun setChangesToken(id: String, token: String?, at: Long)
    @Query("UPDATE accounts SET lastCrawlAt = :at WHERE id = :id") suspend fun setCrawled(id: String, at: Long)
    @Query("UPDATE accounts SET storageUsed = :used, storageLimit = :limit WHERE id = :id") suspend fun setQuota(id: String, used: Long?, limit: Long?)
    @Query("UPDATE accounts SET authError = :err WHERE id = :id") suspend fun setAuthError(id: String, err: String?)
}

@Dao
interface FolderPairDao {
    @Query("SELECT * FROM folder_pairs ORDER BY createdAt") fun observeAll(): Flow<List<FolderPairEntity>>
    @Query("SELECT * FROM folder_pairs ORDER BY createdAt") suspend fun getAll(): List<FolderPairEntity>
    @Query("SELECT * FROM folder_pairs WHERE enabled = 1 AND paused = 0 ORDER BY createdAt") suspend fun getActive(): List<FolderPairEntity>
    @Query("SELECT * FROM folder_pairs WHERE id = :id") suspend fun get(id: String): FolderPairEntity?
    @Query("SELECT * FROM folder_pairs WHERE id = :id") fun observe(id: String): Flow<FolderPairEntity?>
    @Query("SELECT * FROM folder_pairs WHERE accountId = :accountId") suspend fun byAccount(accountId: String): List<FolderPairEntity>
    @Upsert suspend fun upsert(p: FolderPairEntity)
    @Query("DELETE FROM folder_pairs WHERE id = :id") suspend fun delete(id: String)
    @Query("UPDATE folder_pairs SET lastRunAt = :at, lastStatus = :status, lastError = :error, lastSuccessAt = CASE WHEN :status = 'SUCCESS' THEN :at ELSE lastSuccessAt END WHERE id = :id")
    suspend fun setRunResult(id: String, at: Long, status: RunStatus, error: String?)
    @Query("UPDATE folder_pairs SET pendingApproval = :pending, pendingApprovalSummary = :summary WHERE id = :id")
    suspend fun setPendingApproval(id: String, pending: Boolean, summary: String?)
    @Query("UPDATE folder_pairs SET approveOnce = :approve WHERE id = :id") suspend fun setApproveOnce(id: String, approve: Boolean)
    @Query("UPDATE folder_pairs SET paused = :paused, pauseReason = :reason WHERE id = :id") suspend fun setPaused(id: String, paused: Boolean, reason: String?)
    @Query("UPDATE folder_pairs SET enabled = :enabled WHERE id = :id") suspend fun setEnabled(id: String, enabled: Boolean)
    @Query("UPDATE folder_pairs SET pendingLocalChanges = :n WHERE id = :id") suspend fun setPendingLocalChanges(id: String, n: Int)
    @Query("UPDATE folder_pairs SET lastErrorKind = :kind, consecutiveFailures = :n WHERE id = :id") suspend fun setFailure(id: String, kind: String?, n: Int)
    @Query("UPDATE folder_pairs SET holdForPreview = :hold WHERE id = :id") suspend fun setHoldForPreview(id: String, hold: Boolean)
}

/** Per-pair totals from the sync baseline (files only). */
data class PairTotals(val pairId: String, val files: Int, val localBytes: Long, val remoteBytes: Long)

/** Generic per-pair count. */
data class PairCount(val pairId: String, val n: Int)

@Dao
interface FileStateDao {
    @Query("SELECT * FROM file_states WHERE pairId = :pairId") suspend fun forPair(pairId: String): List<FileStateEntity>
    @Query("SELECT COUNT(*) FROM file_states WHERE pairId = :pairId AND isDir = 0") fun observeFileCount(pairId: String): Flow<Int>
    @Query("SELECT * FROM file_states WHERE pairId = :pairId AND `key` = :key") suspend fun get(pairId: String, key: String): FileStateEntity?
    @Query("SELECT * FROM file_states WHERE pairId = :pairId AND localRel = :localRel") suspend fun byLocalRel(pairId: String, localRel: String): FileStateEntity?
    @Upsert suspend fun upsert(s: FileStateEntity)
    @Upsert suspend fun upsertAll(s: List<FileStateEntity>)
    @Query("DELETE FROM file_states WHERE pairId = :pairId AND `key` = :key") suspend fun delete(pairId: String, key: String)
    @Query("DELETE FROM file_states WHERE pairId = :pairId AND (`key` = :prefix OR `key` LIKE :prefix || '/%')") suspend fun deleteSubtree(pairId: String, prefix: String)
    @Query("DELETE FROM file_states WHERE pairId = :pairId") suspend fun clearPair(pairId: String)
    @Query("SELECT pairId, COUNT(*) AS files, COALESCE(SUM(localSize), 0) AS localBytes, COALESCE(SUM(remoteSize), 0) AS remoteBytes FROM file_states WHERE isDir = 0 GROUP BY pairId")
    fun observePairTotals(): Flow<List<PairTotals>>
    @Query("SELECT pairId, COUNT(*) AS files, COALESCE(SUM(localSize), 0) AS localBytes, COALESCE(SUM(remoteSize), 0) AS remoteBytes FROM file_states WHERE isDir = 0 GROUP BY pairId")
    suspend fun pairTotals(): List<PairTotals>
}

@Dao
interface SyncRunDao {
    @Insert suspend fun insert(r: SyncRunEntity)
    @Update suspend fun update(r: SyncRunEntity)
    @Query("SELECT * FROM sync_runs WHERE id = :id") suspend fun get(id: String): SyncRunEntity?
    @Query("SELECT * FROM sync_runs ORDER BY startedAt DESC LIMIT :limit") fun observeRecent(limit: Int): Flow<List<SyncRunEntity>>
    @Query("SELECT * FROM sync_runs WHERE pairId = :pairId ORDER BY startedAt DESC LIMIT :limit") fun observeForPair(pairId: String, limit: Int): Flow<List<SyncRunEntity>>
    @Query("SELECT * FROM sync_runs WHERE pairId = :pairId ORDER BY startedAt DESC LIMIT 1") suspend fun lastForPair(pairId: String): SyncRunEntity?
    @Query("DELETE FROM sync_runs WHERE startedAt < :before") suspend fun deleteOlderThan(before: Long)
    @Query("SELECT * FROM sync_runs WHERE startedAt >= :since ORDER BY startedAt DESC") fun observeSince(since: Long): Flow<List<SyncRunEntity>>
    /** Pairs with a run still open from before [before] (an earlier app process). */
    @Query("SELECT DISTINCT pairId FROM sync_runs WHERE status = 'RUNNING' AND finishedAt IS NULL AND startedAt < :before")
    suspend fun stalePairs(before: Long): List<String>

    /** Runs still open but started before [before] (an earlier app process) can no longer be running. */
    @Query("UPDATE sync_runs SET status = :status, finishedAt = :at, message = :message WHERE status = 'RUNNING' AND finishedAt IS NULL AND startedAt < :before")
    suspend fun closeStale(before: Long, status: RunStatus, at: Long, message: String)
}

@Dao
interface SyncEventDao {
    @Insert suspend fun insert(e: SyncEventEntity)
    @Insert suspend fun insertAll(e: List<SyncEventEntity>)
    @Query("SELECT * FROM sync_events WHERE runId = :runId ORDER BY at, id") fun observeForRun(runId: String): Flow<List<SyncEventEntity>>
    @Query("SELECT * FROM sync_events WHERE pairId = :pairId ORDER BY at DESC, id DESC LIMIT :limit") fun observeForPair(pairId: String, limit: Int): Flow<List<SyncEventEntity>>
    @Query("SELECT * FROM sync_events WHERE level != 'INFO' ORDER BY at DESC, id DESC LIMIT :limit") fun observeProblems(limit: Int): Flow<List<SyncEventEntity>>
    @Query("SELECT * FROM sync_events ORDER BY at DESC, id DESC LIMIT :limit") fun observeRecent(limit: Int): Flow<List<SyncEventEntity>>
    @Query("DELETE FROM sync_events WHERE at < :before") suspend fun deleteOlderThan(before: Long)
    @Query("SELECT * FROM sync_events WHERE at >= :since ORDER BY at DESC, id DESC") fun observeSince(since: Long): Flow<List<SyncEventEntity>>
    @Query("SELECT * FROM sync_events WHERE at >= :since ORDER BY at, id") suspend fun since(since: Long): List<SyncEventEntity>
    @Query("SELECT * FROM sync_events WHERE pairId = :pairId AND path = :path AND action = 'UPLOAD' ORDER BY at DESC LIMIT 200") suspend fun uploadsFor(pairId: String, path: String): List<SyncEventEntity>
}

@Dao
interface ConflictDao {
    @Upsert suspend fun upsert(c: ConflictEntity)
    @Query("SELECT * FROM conflicts WHERE resolved = 0 ORDER BY detectedAt DESC") fun observeOpen(): Flow<List<ConflictEntity>>
    @Query("SELECT COUNT(*) FROM conflicts WHERE resolved = 0") fun observeOpenCount(): Flow<Int>
    @Query("SELECT * FROM conflicts WHERE id = :id") suspend fun get(id: String): ConflictEntity?
    @Query("UPDATE conflicts SET resolved = 1, resolvedAt = :at WHERE id = :id") suspend fun resolve(id: String, at: Long)
    @Query("DELETE FROM conflicts WHERE pairId = :pairId") suspend fun clearPair(pairId: String)
    @Query("SELECT * FROM conflicts WHERE pairId = :pairId AND resolved = 0 ORDER BY detectedAt DESC") fun observeOpenForPair(pairId: String): Flow<List<ConflictEntity>>
    @Query("SELECT * FROM conflicts WHERE pairId = :pairId AND resolved = 0 ORDER BY detectedAt DESC") suspend fun openForPair(pairId: String): List<ConflictEntity>
    @Query("SELECT pairId, COUNT(*) AS n FROM conflicts WHERE resolved = 0 GROUP BY pairId") fun observeOpenCountsByPair(): Flow<List<PairCount>>
    @Query("UPDATE conflicts SET resolved = 0, resolvedAt = NULL WHERE id = :id") suspend fun unresolve(id: String)
}

@Dao
interface TrashDao {
    @Insert suspend fun insert(t: TrashEntity)
    @Query("SELECT * FROM trash_entries ORDER BY createdAt DESC") fun observeAll(): Flow<List<TrashEntity>>
    @Query("SELECT * FROM trash_entries WHERE createdAt < :before") suspend fun olderThan(before: Long): List<TrashEntity>
    @Query("SELECT * FROM trash_entries WHERE id = :id") suspend fun get(id: String): TrashEntity?
    @Query("SELECT COALESCE(SUM(size),0) FROM trash_entries") fun observeTotalSize(): Flow<Long>
    @Delete suspend fun delete(t: TrashEntity)
    @Query("DELETE FROM trash_entries WHERE id = :id") suspend fun deleteById(id: String)
    @Query("SELECT * FROM trash_entries WHERE id IN (:ids)") suspend fun getMany(ids: List<String>): List<TrashEntity>
    @Query("SELECT * FROM trash_entries") suspend fun all(): List<TrashEntity>
    @Query("SELECT * FROM trash_entries WHERE pairId = :pairId AND syncKey = :key ORDER BY createdAt DESC") suspend fun forKey(pairId: String, key: String): List<TrashEntity>
    @Insert suspend fun insertAll(t: List<TrashEntity>)
}

@Dao
interface DriveNodeDao {
    @Query("SELECT * FROM drive_nodes WHERE accountId = :accountId AND trashed = 0") suspend fun allForAccount(accountId: String): List<DriveNodeEntity>
    @Query("SELECT * FROM drive_nodes WHERE accountId = :accountId AND id = :id") suspend fun get(accountId: String, id: String): DriveNodeEntity?
    @Query("SELECT * FROM drive_nodes WHERE accountId = :accountId AND parentId = :parentId AND trashed = 0 ORDER BY isFolder DESC, name COLLATE NOCASE") suspend fun children(accountId: String, parentId: String): List<DriveNodeEntity>
    @Query("SELECT * FROM drive_nodes WHERE accountId = :accountId AND parentId = :parentId AND trashed = 0 ORDER BY isFolder DESC, name COLLATE NOCASE") fun observeChildren(accountId: String, parentId: String): Flow<List<DriveNodeEntity>>
    @Query("SELECT COUNT(*) FROM drive_nodes WHERE accountId = :accountId") suspend fun count(accountId: String): Int
    @Upsert suspend fun upsertAll(nodes: List<DriveNodeEntity>)
    @Upsert suspend fun upsert(node: DriveNodeEntity)
    @Query("DELETE FROM drive_nodes WHERE accountId = :accountId AND id IN (:ids)") suspend fun deleteIds(accountId: String, ids: List<String>)
    @Query("DELETE FROM drive_nodes WHERE accountId = :accountId") suspend fun clearAccount(accountId: String)
    @Query("DELETE FROM drive_nodes WHERE accountId = :accountId AND driveId = :driveId") suspend fun clearDrive(accountId: String, driveId: String)
    @Query("SELECT * FROM drive_nodes WHERE accountId = :accountId AND name LIKE '%' || :q || '%' AND trashed = 0 ORDER BY isFolder DESC, name COLLATE NOCASE LIMIT 200") suspend fun search(accountId: String, q: String): List<DriveNodeEntity>
    /** Top-level items shared with the user (their parent isn't visible, and they aren't in My Drive or a shared drive). */
    @Query("SELECT * FROM drive_nodes WHERE accountId = :accountId AND trashed = 0 AND driveId IS NULL AND id != :rootId AND (parentId IS NULL OR (parentId != :rootId AND parentId NOT IN (SELECT id FROM drive_nodes WHERE accountId = :accountId))) ORDER BY isFolder DESC, name COLLATE NOCASE")
    suspend fun sharedWithMeRoots(accountId: String, rootId: String): List<DriveNodeEntity>
}

@Dao
interface UploadSessionDao {
    @Query("SELECT * FROM upload_sessions WHERE `key` = :key") suspend fun get(key: String): UploadSessionEntity?
    @Upsert suspend fun upsert(s: UploadSessionEntity)
    @Query("DELETE FROM upload_sessions WHERE `key` = :key") suspend fun delete(key: String)
    @Query("DELETE FROM upload_sessions WHERE createdAt < :before") suspend fun deleteOlderThan(before: Long)
}

@Dao
interface PinDao {
    @Query("SELECT * FROM pins WHERE accountId = :accountId") suspend fun forAccount(accountId: String): List<PinEntity>
    @Query("SELECT * FROM pins") fun observeAll(): Flow<List<PinEntity>>
    @Query("SELECT * FROM pins WHERE accountId = :accountId AND fileId = :fileId") suspend fun get(accountId: String, fileId: String): PinEntity?
    @Upsert suspend fun upsert(p: PinEntity)
    @Query("DELETE FROM pins WHERE accountId = :accountId AND fileId = :fileId") suspend fun delete(accountId: String, fileId: String)
    @Query("UPDATE pins SET lastSyncedAt = :at WHERE accountId = :accountId AND fileId = :fileId") suspend fun touch(accountId: String, fileId: String, at: Long)
}

@Dao
interface VaultDao {
    @Query("SELECT * FROM vaults ORDER BY createdAt") fun observeAll(): Flow<List<VaultEntity>>
    @Query("SELECT * FROM vaults ORDER BY createdAt") suspend fun getAll(): List<VaultEntity>
    @Query("SELECT * FROM vaults WHERE id = :id") suspend fun get(id: String): VaultEntity?
    @Upsert suspend fun upsert(v: VaultEntity)
    @Query("DELETE FROM vaults WHERE id = :id") suspend fun delete(id: String)
}

@Dao
interface BlockedPlanDao {
    @Upsert suspend fun upsert(b: BlockedPlanEntity)
    @Query("SELECT * FROM blocked_plans WHERE pairId = :pairId") suspend fun get(pairId: String): BlockedPlanEntity?
    @Query("SELECT * FROM blocked_plans WHERE pairId = :pairId") fun observe(pairId: String): Flow<BlockedPlanEntity?>
    @Query("SELECT * FROM blocked_plans") fun observeAll(): Flow<List<BlockedPlanEntity>>
    @Query("DELETE FROM blocked_plans WHERE pairId = :pairId") suspend fun delete(pairId: String)
}
