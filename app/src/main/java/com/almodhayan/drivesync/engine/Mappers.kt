package com.almodhayan.drivesync.engine

import com.almodhayan.drivesync.data.db.BlockedPlanEntity
import com.almodhayan.drivesync.data.db.ConflictEntity
import com.almodhayan.drivesync.data.db.ConflictPolicy
import com.almodhayan.drivesync.data.db.DocsFormat
import com.almodhayan.drivesync.data.db.EventLevel
import com.almodhayan.drivesync.data.db.FileStateEntity
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.db.RunStatus
import com.almodhayan.drivesync.data.db.SyncEventEntity
import com.almodhayan.drivesync.data.db.SyncMode
import com.almodhayan.drivesync.data.db.SyncRunEntity
import com.almodhayan.drivesync.data.db.TrashEntity
import com.almodhayan.drivesync.data.db.TrashReason
import com.almodhayan.drivesync.sync.Phase
import com.almodhayan.drivesync.sync.PlanSummary
import com.kahflabs.wasl.ffi.core.BaselineEntry
import com.kahflabs.wasl.ffi.core.BlockKind
import com.kahflabs.wasl.ffi.core.BlockSide
import com.kahflabs.wasl.ffi.core.Conditions
import com.kahflabs.wasl.ffi.core.ErrorKind
import com.kahflabs.wasl.ffi.core.Reach
import com.kahflabs.wasl.ffi.core.Route
import com.kahflabs.wasl.ffi.store.BinEntry
import com.kahflabs.wasl.ffi.store.BinSide
import com.kahflabs.wasl.ffi.store.ConflictRecord
import com.kahflabs.wasl.ffi.store.PairEvent
import com.kahflabs.wasl.ffi.store.PairStatus
import com.kahflabs.wasl.ffi.store.RunRecord
import com.kahflabs.wasl.ffi.store.StorageMode
import com.kahflabs.wasl.ffi.core.ConflictPolicy as WConflictPolicy
import com.kahflabs.wasl.ffi.core.DocsFormat as WDocsFormat
import com.kahflabs.wasl.ffi.core.PlanSummary as WPlanSummary
import com.kahflabs.wasl.ffi.core.RunStatus as WRunStatus
import com.kahflabs.wasl.ffi.core.SyncMode as WSyncMode
import com.kahflabs.wasl.ffi.engine.Phase as WPhase
import com.kahflabs.wasl.ffi.store.EventLevel as WEventLevel
import com.kahflabs.wasl.ffi.store.Pair as WPair

// Enums: both sides use 1.1.4's names, so they map by name.
internal fun WSyncMode.toKotlin(): SyncMode = SyncMode.valueOf(name)
internal fun SyncMode.toEngine(): WSyncMode = WSyncMode.valueOf(name)
internal fun WConflictPolicy.toKotlin(): ConflictPolicy = ConflictPolicy.valueOf(name)
internal fun ConflictPolicy.toEngine(): WConflictPolicy = WConflictPolicy.valueOf(name)
internal fun WRunStatus.toKotlin(): RunStatus = RunStatus.valueOf(name)
internal fun RunStatus.toEngine(): WRunStatus = WRunStatus.valueOf(name)
internal fun WDocsFormat.toKotlin(): DocsFormat = if (this == WDocsFormat.PDF) DocsFormat.PDF else DocsFormat.OFFICE
internal fun DocsFormat.toEngine(): WDocsFormat = if (this == DocsFormat.PDF) WDocsFormat.PDF else WDocsFormat.OFFICE
internal fun WEventLevel.toKotlin(): EventLevel = EventLevel.valueOf(name)

internal fun WPhase.toKotlin(): Phase = when (this) {
    WPhase.STARTING -> Phase.PREPARING
    WPhase.LISTING_REMOTE -> Phase.LISTING_REMOTE
    WPhase.SCANNING_LOCAL -> Phase.SCANNING_LOCAL
    WPhase.PLANNING -> Phase.PLANNING
    WPhase.TRANSFERRING -> Phase.TRANSFERRING
    WPhase.DELETING -> Phase.DELETING
    WPhase.FINISHING -> Phase.FINISHING
}

private fun lines(s: String): List<String> = s.lines().map { it.trim() }.filter { it.isNotEmpty() }
private fun commas(s: String): List<String> = s.split(',').map { it.trim() }.filter { it.isNotEmpty() }

/** The engine's pair as the screens know it. [pendingLocalChanges] is the app's own counter. */
internal fun WPair.toEntity(pendingLocalChanges: Int = 0, blockedSummaryJson: String? = null): FolderPairEntity = FolderPairEntity(
    id = id,
    accountId = accountId ?: "",
    name = name,
    localPath = localRoot,
    remoteFolderId = driveFolderId ?: "",
    remotePath = remotePath,
    driveId = driveId,
    mode = method.toKotlin(),
    enabled = enabled,
    wifiOnly = conditions.wifiOnly,
    allowMetered = conditions.allowMetered,
    allowRoaming = conditions.allowRoaming,
    requireCharging = conditions.requireCharging,
    minBatteryPercent = conditions.minBatteryPercent.toInt(),
    ssidAllow = conditions.ssidAllow.joinToString(","),
    ssidDeny = conditions.ssidDeny.joinToString(","),
    requireVpn = conditions.requireVpn,
    includePatterns = includePatterns.joinToString("\n"),
    excludePatterns = excludePatterns.joinToString("\n"),
    excludedSubfolders = excludedSubfolders.joinToString("\n"),
    syncHidden = syncHidden,
    maxFileSizeBytes = sizeLimit.toLong(),
    conflictPolicy = conflictPolicy.toKotlin(),
    propagateDeletions = propagateDeletions,
    massDeleteGuard = massDeleteGuard,
    docsExport = docsExport,
    docsFormat = docsFormat.toKotlin(),
    preserveMtime = preserveMtime,
    encrypted = vaultId != null,
    vaultId = vaultId,
    intervalMinutes = intervalMinutes.toInt(),
    instantSync = instantSync,
    createdAt = createdAt,
    updatedAt = updatedAt,
    lastRunAt = status.lastRunAt,
    lastSuccessAt = status.lastSuccessAt,
    lastStatus = status.lastStatus?.toKotlin(),
    lastError = engineMessageText(status.lastError),
    pendingApproval = status.pendingApproval,
    pendingApprovalSummary = blockedSummaryJson,
    approveOnce = status.approveOnce,
    paused = paused,
    pauseReason = pauseReason,
    pendingLocalChanges = pendingLocalChanges,
    lastErrorKind = status.lastErrorKind?.name,
    consecutiveFailures = status.consecutiveFailures.toInt(),
    holdForPreview = status.holdForPreview,
)

/**
 * The screens' pair for the engine. What the screens do not know (route, reach, storage mode,
 * presets, the .gitignore option) is kept from [existing]; a new pair gets Wasl's defaults.
 */
internal fun FolderPairEntity.toEngine(existing: WPair?): WPair = WPair(
    id = id,
    name = name,
    accountId = accountId.ifEmpty { null },
    localRoot = localPath,
    storageMode = existing?.storageMode ?: (if (localPath.startsWith("content://")) StorageMode.SAF else StorageMode.FILES),
    driveFolderId = remoteFolderId.ifEmpty { null },
    remotePath = remotePath,
    driveId = driveId,
    route = existing?.route ?: Route.DRIVE,
    reach = existing?.reach ?: Reach.LAN,
    method = mode.toEngine(),
    conflictPolicy = conflictPolicy.toEngine(),
    enabled = enabled,
    paused = paused,
    pauseReason = pauseReason,
    conditions = Conditions(
        wifiOnly = wifiOnly,
        allowMetered = allowMetered,
        allowRoaming = allowRoaming,
        requireCharging = requireCharging,
        minBatteryPercent = minBatteryPercent.coerceIn(0, 100).toUByte(),
        ssidAllow = commas(ssidAllow),
        ssidDeny = commas(ssidDeny),
        requireVpn = requireVpn,
    ),
    includePatterns = lines(includePatterns),
    excludePatterns = lines(excludePatterns),
    excludedSubfolders = lines(excludedSubfolders),
    syncHidden = syncHidden,
    sizeLimit = maxFileSizeBytes.coerceAtLeast(0).toULong(),
    propagateDeletions = propagateDeletions,
    massDeleteGuard = massDeleteGuard,
    docsExport = docsExport,
    docsFormat = docsFormat.toEngine(),
    preserveMtime = preserveMtime,
    vaultId = vaultId.takeIf { encrypted },
    honorGitignore = existing?.honorGitignore ?: false,
    presets = existing?.presets ?: emptyList(),
    intervalMinutes = intervalMinutes.coerceAtLeast(0).toUInt(),
    instantSync = instantSync,
    createdAt = createdAt,
    updatedAt = updatedAt,
    status = PairStatus(
        lastRunAt = lastRunAt,
        lastSuccessAt = lastSuccessAt,
        lastStatus = lastStatus?.toEngine(),
        lastError = lastError,
        lastErrorKind = lastErrorKind?.let { k -> runCatching { ErrorKind.valueOf(k) }.getOrNull() },
        consecutiveFailures = consecutiveFailures.coerceAtLeast(0).toUInt(),
        pendingApproval = pendingApproval,
        approveOnce = approveOnce,
        holdForPreview = holdForPreview,
    ),
)

internal fun WPlanSummary.toKotlin(): PlanSummary = PlanSummary(
    uploads = uploads.toInt(),
    uploadBytes = uploadBytes.toLong(),
    downloads = downloads.toInt(),
    downloadBytes = downloadBytes.toLong(),
    exports = exports.toInt(),
    deleteLocal = deleteLocal.toInt(),
    deleteRemote = deleteRemote.toInt(),
    conflicts = conflicts.toInt(),
    mkdirs = mkdirs.toInt(),
    skips = skips.toInt(),
    localFiles = localFiles.toInt(),
    remoteFiles = remoteFiles.toInt(),
    warnings = warnings,
    blockedReason = blockedReason,
    sampleDeletes = sampleDeletes,
    unchanged = unchanged.toInt(),
    blockedKind = when (blockedKind) {
        BlockKind.DELETIONS -> "deletions"
        BlockKind.WIPE -> "wipe"
        BlockKind.FIRST_MERGE -> "first-merge"
        null -> null
    },
    blockedSide = when (blockedSide) {
        BlockSide.DRIVE -> "drive"
        BlockSide.DEVICE -> "phone"
        null -> null
    },
    blockedPercent = blockedPercent.toInt(),
    blockedCount = blockedCount.toInt(),
    movedAfterTransfer = movedAfterTransfer.toInt(),
)

internal fun RunRecord.toEntity(): SyncRunEntity = SyncRunEntity(
    id = id,
    pairId = pairId,
    startedAt = startedAt,
    finishedAt = endedAt,
    status = status.toKotlin(),
    trigger = trigger,
    uploaded = stats.uploaded.toInt(),
    downloaded = (stats.downloaded + stats.exported).toInt(),
    deletedLocal = stats.deletedLocal.toInt(),
    deletedRemote = stats.deletedRemote.toInt(),
    conflicts = stats.conflicts.toInt(),
    skipped = stats.skipped.toInt(),
    errors = stats.errors.toInt(),
    bytesUp = stats.bytesUp.toLong(),
    bytesDown = stats.bytesDown.toLong(),
    message = engineMessageText(message),
)

/** Events get a stable numeric id from their run and position, as Room's autogenerated ids were. */
internal fun PairEvent.toEntity(): SyncEventEntity = SyncEventEntity(
    id = (runId.hashCode().toLong() shl 32) or (seq.toLong() and 0xffffffffL),
    runId = runId,
    pairId = pairId,
    at = event.at,
    level = event.level.toKotlin(),
    action = event.action,
    path = event.key,
    message = event.message,
    detail = event.detail,
    bytes = event.bytes.toLong(),
)

internal fun ConflictRecord.toEntity(): ConflictEntity = ConflictEntity(
    id = id,
    pairId = pairId,
    key = key,
    detectedAt = detectedAt,
    policy = policy.toKotlin(),
    localCopyRel = localCopyRel,
    remoteFileId = remoteFileId,
    message = message,
    resolved = resolved,
    resolvedAt = resolvedAt,
)

/**
 * A bin entry as the recycle bin screen knows it. Paths are for display: the device side shows
 * the file's place in the pair's folder, the Drive side its Drive path.
 */
internal fun BinEntry.toEntity(pairs: Map<String, WPair>): TrashEntity {
    val e = entry
    val pair = pairs[e.pairId]
    val original = when (e.side) {
        BinSide.LOCAL -> (pair?.localRoot?.trimEnd('/') ?: "") + "/" + e.key
        BinSide.DRIVE -> (pair?.remotePath?.trimEnd('/') ?: "") + "/" + e.key
    }
    return TrashEntity(
        id = e.id,
        pairId = e.pairId,
        originalPath = original,
        trashPath = e.location,
        size = e.size.toLong(),
        reason = runCatching { TrashReason.valueOf(e.reason) }.getOrDefault(TrashReason.DELETED),
        createdAt = e.deletedAt,
        side = if (e.side == BinSide.DRIVE) TrashEntity.SIDE_DRIVE else TrashEntity.SIDE_PHONE,
        accountId = e.accountId,
        remoteFileId = e.remoteFileId,
        displayName = e.displayName,
        pairName = e.pairName,
        syncKey = e.key,
    )
}

internal fun BaselineEntry.toEntity(pairId: String): FileStateEntity = FileStateEntity(
    pairId = pairId,
    key = key,
    localRel = localRel,
    isDir = isDir,
    localSize = localSize.toLong(),
    localMtime = localMtime,
    localHash = localHash,
    remoteId = remoteId,
    remoteMd5 = remoteMd5,
    remoteSize = remoteSize?.toLong(),
    remoteMtime = remoteMtime,
    remoteVersion = remoteVersion,
    exportMime = exportMime,
    syncedAt = 0,
)

internal fun blockedEntity(pairId: String, createdAt: Long, summaryJson: String) = BlockedPlanEntity(pairId, createdAt, summaryJson)
