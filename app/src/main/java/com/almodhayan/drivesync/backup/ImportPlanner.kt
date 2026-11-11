package com.almodhayan.drivesync.backup

import androidx.annotation.StringRes
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.AppPrivate
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.crypto.VaultManifest
import com.almodhayan.drivesync.data.db.ConflictPolicy
import com.almodhayan.drivesync.data.db.DocsFormat
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.db.PinEntity
import com.almodhayan.drivesync.data.db.SyncMode
import com.almodhayan.drivesync.data.db.VaultEntity

enum class ImportMode(@StringRes private val labelRes: Int) {
    /** Adds pairs from the backup and updates pairs with the same id. Nothing is removed. */
    MERGE(R.string.backup_import_mode_merge),

    /** Removes all current folder pairs (and their sync state) and offline pins, then adds the ones from the backup. */
    REPLACE(R.string.backup_import_mode_replace);

    val label: String get() = Strings.get(labelRes)
}

/** Everything [ConfigBackup.import] will write, computed without touching the database. */
data class ImportPlan(
    val mode: ImportMode,
    val pairsToUpsert: List<FolderPairEntity>,
    val pairIdsToRemove: List<String>,
    val vaults: List<VaultEntity>,
    val pins: List<PinEntity>,
    val settings: SettingsSnapshot,
    val pairsAdded: Int,
    val pairsUpdated: Int,
    val pairsSkipped: Int,
    /** Pairs that were imported paused because the user has to act first (sign in, enter a vault passphrase). */
    val pairsPaused: Int,
    val accountsToSignIn: List<String>,
    /** One line per skipped item, with the reason. */
    val skipped: List<String>,
    val warnings: List<String>,
) {
    val needsVaultPassphrase: Boolean get() = pairsToUpsert.any { it.pauseReason == ImportPlanner.PAUSE_REASON_VAULT } || vaults.isNotEmpty()
}

/**
 * Pure import logic: validates a [ConfigSnapshot] against the current database contents and decides
 * what to write. No Android or Room runtime dependency, so it is unit tested on the JVM.
 */
object ImportPlanner {
    val PAUSE_REASON_VAULT: String get() = Strings.get(R.string.backup_pause_reason_vault)

    /** Exact text used for pairs whose Google account is not signed in on this device. */
    fun pauseReasonForMissingAccount(email: String): String = Strings.get(R.string.backup_pause_reason_sign_in, email)

    fun plan(
        snapshot: ConfigSnapshot,
        mode: ImportMode,
        existingPairs: List<FolderPairEntity>,
        existingAccountIds: Set<String>,
        existingVaultIds: Set<String>,
        now: Long = System.currentTimeMillis(),
        /** The app's private folders ([AppPrivate.roots]); a pair pointing inside one is refused. */
        privateRoots: Collection<String> = emptyList(),
        packageName: String = "",
        /** Resolves links for a path that exists on this device; see [AppPrivate.isPrivateLocalPath]. */
        canonicalPath: (String) -> String? = { null },
    ): ImportPlan {
        require(snapshot.version >= 1) { "Unsupported backup format version ${snapshot.version}" }
        val warnings = ArrayList<String>()
        val skipped = ArrayList<String>()
        if (snapshot.version > ConfigSnapshot.VERSION) {
            warnings += Strings.get(R.string.backup_warning_newer_format, snapshot.version)
        }

        val existingById = existingPairs.associateBy { it.id }
        val removeIds = if (mode == ImportMode.REPLACE) existingPairs.map { it.id } else emptyList()
        val snapshotVaultIds = snapshot.vaults.map { it.id }.toSet()

        val toUpsert = ArrayList<FolderPairEntity>()
        val seenIds = HashSet<String>()
        var added = 0
        var updated = 0
        var paused = 0

        for (p in snapshot.pairs) {
            val label = p.name.ifBlank { p.localPath.ifBlank { p.id.ifBlank { Strings.get(R.string.backup_unnamed) } } }
            if (p.id.isBlank()) { skipped += Strings.get(R.string.backup_skipped_pair_missing_id, label); continue }
            if (!seenIds.add(p.id)) { skipped += Strings.get(R.string.backup_skipped_pair_duplicate_id, label, p.id); continue }
            if (p.localPath.isBlank()) { skipped += Strings.get(R.string.backup_skipped_pair_no_device_folder, label); continue }
            if (p.remoteFolderId.isBlank()) { skipped += Strings.get(R.string.backup_skipped_pair_no_drive_folder, label); continue }
            // A backup file is outside input: a pair inside the app's own storage would upload tokens,
            // vault keys and the database to a Drive folder of the file's choosing (security review 2026-10).
            if (AppPrivate.isPrivateLocalPath(p.localPath, privateRoots, packageName, canonicalPath)) {
                skipped += Strings.get(R.string.backup_skipped_pair_private_storage, label); continue
            }
            if (p.accountId.isBlank()) { skipped += Strings.get(R.string.backup_skipped_pair_no_account, label); continue }
            val syncMode = parseEnum<SyncMode>(p.mode)
            if (syncMode == null) { skipped += Strings.get(R.string.backup_skipped_pair_unknown_mode, label, p.mode); continue }

            val existing = if (mode == ImportMode.MERGE) existingById[p.id] else null
            val base = existing ?: FolderPairEntity(
                id = p.id,
                accountId = p.accountId,
                name = p.name,
                localPath = p.localPath,
                remoteFolderId = p.remoteFolderId,
                remotePath = p.remotePath,
                driveId = p.driveId,
                mode = syncMode,
                createdAt = p.createdAt ?: now,
                updatedAt = now,
            )

            val conflictPolicy = parseEnum<ConflictPolicy>(p.conflictPolicy)
            if (p.conflictPolicy != null && conflictPolicy == null) {
                warnings += Strings.get(R.string.backup_warning_unknown_conflict_policy, label, p.conflictPolicy.orEmpty(), base.conflictPolicy.label)
            }
            val docsFormat = parseEnum<DocsFormat>(p.docsFormat)
            if (p.docsFormat != null && docsFormat == null) {
                warnings += Strings.get(R.string.backup_warning_unknown_docs_format, label, p.docsFormat.orEmpty(), base.docsFormat.label)
            }

            var entity = base.copy(
                accountId = p.accountId,
                name = p.name.ifBlank { p.localPath.trimEnd('/').substringAfterLast('/').ifBlank { Strings.get(R.string.backup_default_pair_name) } },
                localPath = p.localPath,
                remoteFolderId = p.remoteFolderId,
                remotePath = p.remotePath,
                driveId = p.driveId,
                mode = syncMode,
                enabled = p.enabled,
                wifiOnly = p.wifiOnly ?: base.wifiOnly,
                allowMetered = p.allowMetered ?: base.allowMetered,
                allowRoaming = p.allowRoaming ?: base.allowRoaming,
                requireCharging = p.requireCharging ?: base.requireCharging,
                minBatteryPercent = p.minBatteryPercent ?: base.minBatteryPercent,
                ssidAllow = p.ssidAllow ?: base.ssidAllow,
                ssidDeny = p.ssidDeny ?: base.ssidDeny,
                requireVpn = p.requireVpn ?: base.requireVpn,
                includePatterns = p.includePatterns ?: base.includePatterns,
                excludePatterns = p.excludePatterns ?: base.excludePatterns,
                excludedSubfolders = p.excludedSubfolders ?: base.excludedSubfolders,
                syncHidden = p.syncHidden ?: base.syncHidden,
                maxFileSizeBytes = p.maxFileSizeBytes ?: base.maxFileSizeBytes,
                conflictPolicy = conflictPolicy ?: base.conflictPolicy,
                // A new pair keeps the app's deletion safeguards whatever the file says; the user can
                // turn them off in the pair's settings after the preview (security review 2026-10).
                propagateDeletions = if (existing == null) base.propagateDeletions else p.propagateDeletions ?: base.propagateDeletions,
                massDeleteGuard = if (existing == null) base.massDeleteGuard else p.massDeleteGuard ?: base.massDeleteGuard,
                docsExport = p.docsExport ?: base.docsExport,
                docsFormat = docsFormat ?: base.docsFormat,
                preserveMtime = p.preserveMtime ?: base.preserveMtime,
                encrypted = p.encrypted,
                vaultId = p.vaultId,
                intervalMinutes = p.intervalMinutes ?: base.intervalMinutes,
                instantSync = p.instantSync ?: base.instantSync,
                updatedAt = now,
            )

            if (existing == null &&
                ((p.massDeleteGuard != null && p.massDeleteGuard != base.massDeleteGuard) || (p.propagateDeletions != null && p.propagateDeletions != base.propagateDeletions))
            ) {
                warnings += Strings.get(R.string.backup_warning_safeguards_kept, label)
            }
            // Nothing syncs before the user previews a pair that is new or now points somewhere
            // else, as for a pair made in the editor (security review 2026-10).
            val repointed = existing != null &&
                (existing.localPath != entity.localPath || existing.remoteFolderId != entity.remoteFolderId || existing.mode != entity.mode)
            if (existing == null || repointed) entity = entity.copy(holdForPreview = true)

            val accountMissing = p.accountId !in existingAccountIds
            when {
                accountMissing -> {
                    entity = entity.copy(enabled = false, paused = true, pauseReason = pauseReasonForMissingAccount(p.accountId))
                    paused++
                }
                existing == null && entity.encrypted -> {
                    entity = entity.copy(paused = true, pauseReason = PAUSE_REASON_VAULT)
                    paused++
                }
            }
            if (entity.encrypted) {
                val v = entity.vaultId
                if (v.isNullOrBlank() || (v !in snapshotVaultIds && v !in existingVaultIds)) {
                    warnings += Strings.get(R.string.backup_warning_vault_missing, label)
                }
            }
            toUpsert += entity
            if (existing == null) added++ else updated++
        }

        val accountsToSignIn = (snapshot.accounts + snapshot.pairs.map { it.accountId })
            .filter { it.isNotBlank() && it !in existingAccountIds }
            .distinct()

        val vaults = ArrayList<VaultEntity>()
        val seenVaults = HashSet<String>()
        for (v in snapshot.vaults) {
            val label = v.name.ifBlank { v.id.ifBlank { Strings.get(R.string.backup_unnamed) } }
            // v1 derives its keys from salt and rounds; v2 from its vault.json (Argon2id).
            val complete = if (v.version >= 2) !v.manifestJson.isNullOrBlank() && v.saltB64.isNotBlank() && v.keyCheckB64.isNotBlank()
            else v.saltB64.isNotBlank() && v.keyCheckB64.isNotBlank() && VaultManifest.iterationsInBounds(v.iterations)
            if (v.id.isBlank() || !complete) {
                skipped += Strings.get(R.string.backup_skipped_vault_incomplete, label); continue
            }
            if (!seenVaults.add(v.id)) { skipped += Strings.get(R.string.backup_skipped_vault_duplicate_id, label, v.id); continue }
            vaults += VaultEntity(
                id = v.id, name = v.name.ifBlank { Strings.get(R.string.backup_default_vault_name) }, saltB64 = v.saltB64, iterations = v.iterations,
                keyCheckB64 = v.keyCheckB64, createdAt = v.createdAt ?: now,
                version = if (v.version >= 2) 2 else 1, manifestJson = if (v.version >= 2) v.manifestJson else null,
            )
        }

        val pins = ArrayList<PinEntity>()
        val seenPins = HashSet<Pair<String, String>>()
        for (pin in snapshot.pins) {
            if (pin.accountId.isBlank() || pin.fileId.isBlank()) { skipped += Strings.get(R.string.backup_skipped_pin_missing_id); continue }
            if (!seenPins.add(pin.accountId to pin.fileId)) continue
            pins += PinEntity(accountId = pin.accountId, fileId = pin.fileId, isFolder = pin.isFolder, createdAt = pin.createdAt ?: now)
        }

        return ImportPlan(
            mode = mode,
            pairsToUpsert = toUpsert,
            pairIdsToRemove = removeIds,
            vaults = vaults,
            pins = pins,
            settings = snapshot.settings,
            pairsAdded = added,
            pairsUpdated = updated,
            pairsSkipped = snapshot.pairs.size - toUpsert.size,
            pairsPaused = paused,
            accountsToSignIn = accountsToSignIn,
            skipped = skipped,
            warnings = warnings,
        )
    }
}
