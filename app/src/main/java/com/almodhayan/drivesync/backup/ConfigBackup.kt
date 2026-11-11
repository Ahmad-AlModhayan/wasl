package com.almodhayan.drivesync.backup

import android.content.Context
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.AppPrivate
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.core.TimeFmt
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import com.almodhayan.drivesync.drive.DriveApi
import com.almodhayan.drivesync.drive.DriveFile
import com.almodhayan.drivesync.ondemand.PinRequests
import com.almodhayan.drivesync.scheduler.MonitorService
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.withContext
import java.io.File
import javax.inject.Inject
import javax.inject.Singleton

/** The backup file stored in one account's Drive app-data folder. */
data class DriveBackupInfo(val fileId: String, val modifiedTime: Long?, val size: Long?)

/** Outcome of [ConfigBackup.backupAll]: per-account errors are collected, never thrown. */
data class BackupAllResult(val succeeded: List<String>, val failed: Map<String, AppError>) {
    val allFailed: Boolean get() = succeeded.isEmpty() && failed.isNotEmpty()
}

/** What an import did. [lines] is ready to show to the user. */
data class ImportReport(
    val mode: ImportMode,
    val pairsAdded: Int,
    val pairsUpdated: Int,
    val pairsRemoved: Int,
    val pairsSkipped: Int,
    val pairsPaused: Int,
    val vaultsImported: Int,
    val pinsImported: Int,
    val pinsRemoved: Int,
    val settingsApplied: Boolean,
    val accountsToSignIn: List<String>,
    val needsVaultPassphrase: Boolean,
    val skipped: List<String>,
    val warnings: List<String>,
) {
    fun lines(): List<String> = buildList {
        val sep = Strings.get(R.string.backup_list_separator)
        val parts = ArrayList<String>()
        parts += Strings.get(R.string.backup_report_added, pairsAdded)
        if (pairsUpdated > 0) parts += Strings.get(R.string.backup_report_updated, pairsUpdated)
        if (pairsRemoved > 0) parts += Strings.get(R.string.backup_report_removed, pairsRemoved)
        if (pairsSkipped > 0) parts += Strings.get(R.string.backup_report_skipped, pairsSkipped)
        add(Strings.get(R.string.backup_report_pairs, parts.joinToString(sep)))
        if (pairsPaused > 0) add(Strings.plural(R.plurals.backup_report_paused, pairsPaused))
        if (vaultsImported > 0) add(Strings.get(R.string.backup_report_vaults, vaultsImported))
        if (pinsImported > 0 || pinsRemoved > 0) add(
            if (pinsRemoved > 0) Strings.get(R.string.backup_report_pins_removed, pinsImported, pinsRemoved)
            else Strings.get(R.string.backup_report_pins, pinsImported)
        )
        if (settingsApplied) add(Strings.get(R.string.backup_report_settings_applied))
        if (accountsToSignIn.isNotEmpty()) add(Strings.get(R.string.backup_report_sign_in, accountsToSignIn.joinToString(sep)))
        addAll(skipped)
        addAll(warnings)
    }
}

/**
 * Exports and imports the app configuration, locally (JSON text the UI writes to a file) and to the
 * hidden app-data area of each Google account's Drive. Vault passphrases and Google sign-in are never
 * part of a backup.
 */
@Singleton
class ConfigBackup @Inject constructor(
    @ApplicationContext private val context: Context,
    private val db: Db,
    private val settings: SettingsRepository,
    private val api: DriveApi,
    private val pinRequests: PinRequests,
) {
    // ---------------------------------------------------------------- export

    suspend fun snapshot(): ConfigSnapshot {
        val s = settings.current()
        return ConfigSnapshot(
            version = ConfigSnapshot.VERSION,
            exportedAt = System.currentTimeMillis(),
            deviceName = s.deviceName,
            settings = SettingsSnapshot.from(s),
            accounts = db.accounts().getAll().map { it.id },
            pairs = db.pairs().getAll().map { PairSnapshot.from(it) },
            vaults = db.vaults().getAll().map { VaultSnapshot.from(it) },
            pins = db.pins().observeAll().first().map { PinSnapshot.from(it) },
        )
    }

    /** Pretty-printed JSON of the current configuration. */
    suspend fun export(): String = snapshot().encode()

    // ---------------------------------------------------------------- import

    /** Parses backup text. Throws an [AppError] with a user hint when the text is not a Wasl backup. */
    fun parse(json: String): ConfigSnapshot {
        val snap = try {
            ConfigSnapshot.decode(json)
        } catch (e: Exception) {
            throw AppError.Unknown(Strings.get(R.string.backup_error_not_a_backup), e)
        }
        if (snap.version < 1) throw AppError.Unknown(Strings.get(R.string.backup_error_unsupported_version, snap.version))
        return snap
    }

    /** Computes what an import would do, without changing anything. Used for the confirmation dialog. */
    suspend fun preview(snapshot: ConfigSnapshot, mode: ImportMode): ImportPlan = ImportPlanner.plan(
        snapshot = snapshot,
        mode = mode,
        existingPairs = db.pairs().getAll(),
        existingAccountIds = db.accounts().getAll().map { it.id }.toSet(),
        existingVaultIds = db.vaults().getAll().map { it.id }.toSet(),
        privateRoots = AppPrivate.roots(context),
        packageName = context.packageName,
        canonicalPath = AppPrivate::canonicalPath,
    )

    suspend fun import(json: String, mode: ImportMode): ImportReport = import(parse(json), mode)

    suspend fun import(snapshot: ConfigSnapshot, mode: ImportMode): ImportReport {
        val plan = preview(snapshot, mode)
        var pinsRemoved = 0
        run {
            // Removing a pair in the engine removes its baseline, conflicts and stopped plan too.
            for (id in plan.pairIdsToRemove) db.pairs().delete(id)
            if (mode == ImportMode.REPLACE) {
                for (account in db.accounts().getAll()) {
                    for (pin in db.pins().forAccount(account.id)) {
                        db.pins().delete(pin.accountId, pin.fileId)
                        pinsRemoved++
                    }
                }
            }
            plan.pairsToUpsert.forEach { db.pairs().upsert(it) }
            plan.vaults.forEach { db.vaults().upsert(it) }
            plan.pins.forEach { db.pins().upsert(it) }
        }

        val before = settings.current()
        settings.update { plan.settings.applyTo(it) }
        val warnings = plan.warnings.toMutableList()
        val backupDevice = plan.settings.deviceName ?: snapshot.deviceName
        if (backupDevice.isNotBlank() && backupDevice != before.deviceName) {
            warnings += Strings.get(R.string.backup_warning_device_name, before.deviceName, backupDevice)
        }

        if (plan.pins.isNotEmpty() || pinsRemoved > 0) pinRequests.notifyChanged()
        runCatching { MonitorService.startIfEnabled(context) }

        return ImportReport(
            mode = mode,
            pairsAdded = plan.pairsAdded,
            pairsUpdated = plan.pairsUpdated,
            pairsRemoved = plan.pairIdsToRemove.size,
            pairsSkipped = plan.pairsSkipped,
            pairsPaused = plan.pairsPaused,
            vaultsImported = plan.vaults.size,
            pinsImported = plan.pins.size,
            pinsRemoved = pinsRemoved,
            settingsApplied = true,
            accountsToSignIn = plan.accountsToSignIn,
            needsVaultPassphrase = plan.needsVaultPassphrase,
            skipped = plan.skipped,
            warnings = warnings,
        )
    }

    // ---------------------------------------------------------------- Google Drive app data

    /** Uploads the current configuration to [accountId]'s app-data folder, replacing the previous file. */
    suspend fun backupToDrive(accountId: String): DriveBackupInfo = uploadSnapshot(accountId, export())

    /** The backup file in [accountId]'s app-data folder, or null when none was written yet. */
    suspend fun listDriveBackups(accountId: String): DriveBackupInfo? = findBackupFile(accountId)?.toInfo()

    /** Downloads the backup text from [accountId]'s app-data folder without importing it. */
    suspend fun downloadFromDrive(accountId: String): String {
        val file = findBackupFile(accountId)
            ?: throw AppError.Unknown(Strings.get(R.string.backup_error_no_drive_backup))
        val tmp = withContext(Dispatchers.IO) { File.createTempFile("drivesync-restore-", ".json", context.cacheDir) }
        try {
            withContext(Dispatchers.IO) { tmp.delete() }   // download() would otherwise resume into the empty temp file
            api.download(accountId, file.id, tmp, file.sizeLong)
            return withContext(Dispatchers.IO) { tmp.readText() }
        } finally {
            tmp.delete()   // plain call: a suspend call here would throw first when the work was stopped
        }
    }

    suspend fun restoreFromDrive(accountId: String, mode: ImportMode): ImportReport = import(downloadFromDrive(accountId), mode)

    /** Backs the configuration up to every signed-in account. Errors are collected per account, not thrown. */
    suspend fun backupAll(): BackupAllResult {
        val accounts = db.accounts().getAll()
        if (accounts.isEmpty()) return BackupAllResult(emptyList(), emptyMap())
        val json = export()
        val ok = ArrayList<String>()
        val failed = LinkedHashMap<String, AppError>()
        for (account in accounts) {
            try {
                uploadSnapshot(account.id, json)
                ok += account.id
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                failed[account.id] = AppError.wrap(t)
            }
        }
        return BackupAllResult(ok, failed)
    }

    private suspend fun uploadSnapshot(accountId: String, json: String): DriveBackupInfo {
        val tmp = withContext(Dispatchers.IO) {
            File.createTempFile("drivesync-backup-", ".json", context.cacheDir).apply { writeText(json) }
        }
        try {
            val existing = findBackupFile(accountId)
            val result = api.upload(
                account = accountId,
                src = tmp,
                name = ConfigSnapshot.DRIVE_FILE_NAME,
                parentId = if (existing == null) APP_DATA_FOLDER else null,
                existingFileId = existing?.id,
                modifiedTime = null,
                mimeType = "application/json",
            )
            val now = System.currentTimeMillis()
            settings.update { it.copy(lastBackupAt = now) }
            return result.file.toInfo()
        } finally {
            tmp.delete()   // plain call: a suspend call here would throw first when the work was stopped
        }
    }

    private suspend fun findBackupFile(accountId: String): DriveFile? =
        api.listAppData(accountId)
            .filter { it.name == ConfigSnapshot.DRIVE_FILE_NAME && !it.trashed }
            .maxByOrNull { TimeFmt.parseRfc3339(it.modifiedTime) ?: 0L }

    private fun DriveFile.toInfo() = DriveBackupInfo(id, TimeFmt.parseRfc3339(modifiedTime), sizeLong)

    companion object {
        const val APP_DATA_FOLDER = "appDataFolder"
    }
}
