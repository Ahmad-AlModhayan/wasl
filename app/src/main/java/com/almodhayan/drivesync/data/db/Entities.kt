package com.almodhayan.drivesync.data.db

import androidx.room.ColumnInfo
import androidx.room.Entity
import androidx.room.Index
import androidx.room.PrimaryKey

@Entity(tableName = "accounts")
data class AccountEntity(
    @PrimaryKey val id: String,              // email address
    val displayName: String? = null,
    val photoUrl: String? = null,
    val rootFolderId: String? = null,
    val changesToken: String? = null,
    val lastCrawlAt: Long? = null,
    val lastDeltaAt: Long? = null,
    val storageUsed: Long? = null,
    val storageLimit: Long? = null,
    val addedAt: Long = System.currentTimeMillis(),
    val authError: String? = null,
)

@Entity(tableName = "folder_pairs", indices = [Index("accountId")])
data class FolderPairEntity(
    @PrimaryKey val id: String,
    val accountId: String,
    val name: String,
    val localPath: String,
    val remoteFolderId: String,
    val remotePath: String,
    val driveId: String? = null,
    val mode: SyncMode = SyncMode.TWO_WAY,
    val enabled: Boolean = true,

    // conditions
    val wifiOnly: Boolean = true,
    val allowMetered: Boolean = false,
    val allowRoaming: Boolean = false,
    val requireCharging: Boolean = false,
    val minBatteryPercent: Int = 15,
    val ssidAllow: String = "",              // comma separated
    val ssidDeny: String = "",
    val requireVpn: Boolean = false,

    // options
    val includePatterns: String = "",        // newline separated
    val excludePatterns: String = "",
    val excludedSubfolders: String = "",     // newline separated relative paths
    val syncHidden: Boolean = false,
    val maxFileSizeBytes: Long = 0,          // 0 = unlimited
    val conflictPolicy: ConflictPolicy = ConflictPolicy.KEEP_BOTH,
    val propagateDeletions: Boolean = true,
    val massDeleteGuard: Boolean = true,
    val docsExport: Boolean = true,
    val docsFormat: DocsFormat = DocsFormat.OFFICE,
    val preserveMtime: Boolean = true,
    val encrypted: Boolean = false,
    val vaultId: String? = null,
    val intervalMinutes: Int = 0,            // 0 = follow global setting
    val instantSync: Boolean = true,

    val createdAt: Long = System.currentTimeMillis(),
    val updatedAt: Long = System.currentTimeMillis(),

    // status
    val lastRunAt: Long? = null,
    val lastSuccessAt: Long? = null,
    val lastStatus: RunStatus? = null,
    val lastError: String? = null,
    val pendingApproval: Boolean = false,
    val pendingApprovalSummary: String? = null,
    val approveOnce: Boolean = false,
    val paused: Boolean = false,
    val pauseReason: String? = null,
    val pendingLocalChanges: Int = 0,
    // v2: failure tracking (transient failures retry quietly until they keep failing)
    val lastErrorKind: String? = null,
    @ColumnInfo(defaultValue = "0") val consecutiveFailures: Int = 0,
    /** New (or re-pointed) pairs wait for "Start sync" on the preview: nothing syncs before the preview. */
    @ColumnInfo(defaultValue = "0") val holdForPreview: Boolean = false,
) {
    fun includeList(): List<String> = includePatterns.lines().map { it.trim() }.filter { it.isNotEmpty() }
    fun excludeList(): List<String> = excludePatterns.lines().map { it.trim() }.filter { it.isNotEmpty() }
    fun excludedSubfolderList(): List<String> = excludedSubfolders.lines().map { it.trim() }.filter { it.isNotEmpty() }
    fun ssidAllowList(): List<String> = ssidAllow.split(',').map { it.trim() }.filter { it.isNotEmpty() }
    fun ssidDenyList(): List<String> = ssidDeny.split(',').map { it.trim() }.filter { it.isNotEmpty() }
}

/** Last synced state of one path in a pair (the "baseline" for three-way merges). */
@Entity(tableName = "file_states", primaryKeys = ["pairId", "key"], indices = [Index("pairId"), Index("remoteId")])
data class FileStateEntity(
    val pairId: String,
    val key: String,                         // canonical relative path using remote (Drive) names
    val localRel: String,                    // local relative path (differs only when names were mapped)
    val isDir: Boolean,
    val localSize: Long = 0,
    val localMtime: Long = 0,
    val localHash: String? = null,
    val remoteId: String? = null,
    val remoteMd5: String? = null,
    val remoteSize: Long? = null,
    val remoteMtime: Long? = null,
    val remoteVersion: Long? = null,
    val exportMime: String? = null,          // set for Google Docs export mirrors (never uploaded back)
    val syncedAt: Long = System.currentTimeMillis(),
)

@Entity(tableName = "sync_runs", indices = [Index("pairId"), Index("startedAt")])
data class SyncRunEntity(
    @PrimaryKey val id: String,
    val pairId: String,
    val startedAt: Long,
    val finishedAt: Long? = null,
    val status: RunStatus = RunStatus.RUNNING,
    val trigger: String = "manual",
    val uploaded: Int = 0,
    val downloaded: Int = 0,
    val deletedLocal: Int = 0,
    val deletedRemote: Int = 0,
    val conflicts: Int = 0,
    val skipped: Int = 0,
    val errors: Int = 0,
    val bytesUp: Long = 0,
    val bytesDown: Long = 0,
    val message: String? = null,
)

@Entity(tableName = "sync_events", indices = [Index("runId"), Index("pairId"), Index("at")])
data class SyncEventEntity(
    @PrimaryKey(autoGenerate = true) val id: Long = 0,
    val runId: String,
    val pairId: String,
    val at: Long = System.currentTimeMillis(),
    val level: EventLevel = EventLevel.INFO,
    val action: String,                      // UPLOAD, DOWNLOAD, DELETE_LOCAL, DELETE_REMOTE, CONFLICT, SKIP, ERROR, INFO, EXPORT, MKDIR
    val path: String? = null,
    val message: String,
    val detail: String? = null,
    @ColumnInfo(defaultValue = "0") val bytes: Long = 0,   // v2: size of the file the event is about
)

@Entity(tableName = "conflicts", indices = [Index("pairId")])
data class ConflictEntity(
    @PrimaryKey val id: String,
    val pairId: String,
    val key: String,
    val detectedAt: Long = System.currentTimeMillis(),
    val policy: ConflictPolicy,
    val localCopyRel: String? = null,        // conflict copy created locally (and uploaded)
    val remoteFileId: String? = null,
    val message: String,
    val resolved: Boolean = false,
    val resolvedAt: Long? = null,
)

@Entity(tableName = "trash_entries", indices = [Index("pairId"), Index("createdAt")])
data class TrashEntity(
    @PrimaryKey val id: String,
    val pairId: String,
    val originalPath: String,                // phone: absolute local path · drive: "<remote pair path>/<key>"
    val trashPath: String,                   // phone: absolute path inside the app recycle bin · drive: ""
    val size: Long,
    val reason: TrashReason,
    val createdAt: Long = System.currentTimeMillis(),
    // v2
    @ColumnInfo(defaultValue = "phone") val side: String = SIDE_PHONE,   // where the file was removed: "phone" | "drive"
    val accountId: String? = null,           // drive side: account that owns the file
    val remoteFileId: String? = null,        // drive side: id in Drive's own bin
    val displayName: String? = null,
    val pairName: String? = null,            // kept so rows stay readable after a pair is removed
    val syncKey: String? = null,             // pair-relative key, links overwritten versions to the file
) {
    companion object {
        const val SIDE_PHONE = "phone"
        const val SIDE_DRIVE = "drive"
    }
}

/** Cached Drive metadata for an account, kept fresh with the Changes API. */
@Entity(tableName = "drive_nodes", primaryKeys = ["accountId", "id"], indices = [Index("accountId", "parentId"), Index("accountId", "driveId")])
data class DriveNodeEntity(
    val accountId: String,
    val id: String,
    val name: String,
    val mimeType: String,
    val parentId: String? = null,
    val driveId: String? = null,
    val size: Long? = null,
    val md5: String? = null,
    val modifiedTime: Long = 0,
    val version: Long? = null,
    val trashed: Boolean = false,
    val isFolder: Boolean = false,
    val isShortcut: Boolean = false,
    val shortcutTargetId: String? = null,
    val shortcutTargetMime: String? = null,
    // v2
    @ColumnInfo(defaultValue = "0") val shared: Boolean = false,
    val permissionCount: Int? = null,
)

@Entity(tableName = "upload_sessions")
data class UploadSessionEntity(
    @PrimaryKey val key: String,             // pairId|key|localHash
    val sessionUri: String,
    val createdAt: Long = System.currentTimeMillis(),
    val bytesSent: Long = 0,
)

/** Files/folders pinned for offline use in the on-demand Drive provider. */
@Entity(tableName = "pins", primaryKeys = ["accountId", "fileId"])
data class PinEntity(
    val accountId: String,
    val fileId: String,
    val isFolder: Boolean,
    val createdAt: Long = System.currentTimeMillis(),
    val lastSyncedAt: Long? = null,
)

@Entity(tableName = "vaults")
data class VaultEntity(
    @PrimaryKey val id: String,
    val name: String,
    val saltB64: String,
    val iterations: Int,                     // PBKDF2 rounds (v1); 0 for v2, whose Argon2id cost is in manifestJson
    val keyCheckB64: String,                 // HMAC of a constant with the derived key, for passphrase validation
    val createdAt: Long = System.currentTimeMillis(),
    /** The vault's format: 1 (PBKDF2, one stream per file) or 2 (Argon2id, chunks; docs/ON_DEMAND.md §1). */
    @ColumnInfo(defaultValue = "1") val version: Int = 1,
    /** v2 only: the vault.json text exactly as created or joined (salt, Argon2id cost, key check). */
    val manifestJson: String? = null,
)

/** Result of the last planning pass that was blocked by the mass-delete guard. */
@Entity(tableName = "blocked_plans")
data class BlockedPlanEntity(
    @PrimaryKey val pairId: String,
    val createdAt: Long,
    @ColumnInfo(defaultValue = "") val summaryJson: String,
)
