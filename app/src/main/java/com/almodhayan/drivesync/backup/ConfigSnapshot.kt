package com.almodhayan.drivesync.backup

import com.almodhayan.drivesync.data.db.ConflictPolicy
import com.almodhayan.drivesync.data.db.DocsFormat
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.db.PinEntity
import com.almodhayan.drivesync.data.db.VaultEntity
import com.almodhayan.drivesync.data.prefs.AppSettings
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

/**
 * Portable, versioned description of the app configuration.
 *
 * Contains no secrets and no runtime state: no OAuth tokens, no vault keys or passphrases, no sync
 * baselines, history, conflicts or trash. Enum values are stored as strings so a backup written by a
 * newer app version still parses; unknown values are reported and skipped by [ImportPlanner].
 * Everything in this file is plain Kotlin so it can be unit tested on the JVM.
 */
@Serializable
data class ConfigSnapshot(
    /** Required on purpose: a JSON document without it is not a Drive Sync backup. */
    val version: Int,
    val exportedAt: Long = 0L,
    val deviceName: String = "",
    val settings: SettingsSnapshot = SettingsSnapshot(),
    /** Emails of the Google accounts that were signed in when the backup was made. */
    val accounts: List<String> = emptyList(),
    val pairs: List<PairSnapshot> = emptyList(),
    val vaults: List<VaultSnapshot> = emptyList(),
    val pins: List<PinSnapshot> = emptyList(),
) {
    fun encode(): String = json.encodeToString(serializer(), this)

    companion object {
        const val VERSION = 1

        /** Name of the backup file inside each account's hidden Drive app-data folder. */
        const val DRIVE_FILE_NAME = "drivesync-config.json"

        val json: Json = Json {
            prettyPrint = true
            ignoreUnknownKeys = true
            isLenient = true
            encodeDefaults = true
            explicitNulls = false
        }

        /** Throws [kotlinx.serialization.SerializationException] / [IllegalArgumentException] when [text] is not a snapshot. */
        fun decode(text: String): ConfigSnapshot = json.decodeFromString(serializer(), text)
    }
}

/**
 * All [AppSettings] fields except `onboardingDone` and `lastBackupAt`, which belong to the device.
 * Every field is nullable: a field missing from an older backup keeps the current value on import.
 */
@Serializable
data class SettingsSnapshot(
    val globalIntervalMinutes: Int? = null,
    val concurrency: Int? = null,
    val bandwidthLimitKBps: Int? = null,
    val trashRetentionDays: Int? = null,
    val massDeleteMinFiles: Int? = null,
    val massDeleteThresholdPercent: Int? = null,
    val conflictPolicyDefault: String? = null,
    val docsFormatDefault: String? = null,
    val instantMonitoring: Boolean? = null,
    val foregroundPollSeconds: Int? = null,
    val remotePollMinutes: Int? = null,
    val notifyOnSuccess: Boolean? = null,
    val notifyOnError: Boolean? = null,
    val notifyProgress: Boolean? = null,
    val theme: String? = null,
    val deviceName: String? = null,
    val historyRetentionDays: Int? = null,
    val autoBackupConfig: Boolean? = null,
    val onDemandCacheMb: Int? = null,
    val batteryHintDismissed: Boolean? = null,
    val wifiOnlyDefault: Boolean? = null,
    val requireChargingDefault: Boolean? = null,
    val minBatteryDefault: Int? = null,
) {
    /**
     * Applies the snapshot onto [current]. `onboardingDone`, `lastBackupAt` and `deviceName` are never
     * changed: the first two describe this installation, and the device name identifies this device
     * in conflict copies, so a backup made on another phone must not rename it. The automation switch
     * and secret are never backed up either: a backup on Drive must not carry a key to this phone.
     */
    fun applyTo(current: AppSettings): AppSettings = current.copy(
        globalIntervalMinutes = globalIntervalMinutes ?: current.globalIntervalMinutes,
        concurrency = concurrency ?: current.concurrency,
        bandwidthLimitKBps = bandwidthLimitKBps ?: current.bandwidthLimitKBps,
        trashRetentionDays = trashRetentionDays ?: current.trashRetentionDays,
        massDeleteMinFiles = massDeleteMinFiles ?: current.massDeleteMinFiles,
        massDeleteThresholdPercent = massDeleteThresholdPercent ?: current.massDeleteThresholdPercent,
        conflictPolicyDefault = parseEnum<ConflictPolicy>(conflictPolicyDefault) ?: current.conflictPolicyDefault,
        docsFormatDefault = parseEnum<DocsFormat>(docsFormatDefault) ?: current.docsFormatDefault,
        instantMonitoring = instantMonitoring ?: current.instantMonitoring,
        foregroundPollSeconds = foregroundPollSeconds ?: current.foregroundPollSeconds,
        remotePollMinutes = remotePollMinutes ?: current.remotePollMinutes,
        notifyOnSuccess = notifyOnSuccess ?: current.notifyOnSuccess,
        notifyOnError = notifyOnError ?: current.notifyOnError,
        notifyProgress = notifyProgress ?: current.notifyProgress,
        theme = theme?.takeIf { it in THEMES } ?: current.theme,
        historyRetentionDays = historyRetentionDays ?: current.historyRetentionDays,
        autoBackupConfig = autoBackupConfig ?: current.autoBackupConfig,
        onDemandCacheMb = onDemandCacheMb ?: current.onDemandCacheMb,
        batteryHintDismissed = batteryHintDismissed ?: current.batteryHintDismissed,
        wifiOnlyDefault = wifiOnlyDefault ?: current.wifiOnlyDefault,
        requireChargingDefault = requireChargingDefault ?: current.requireChargingDefault,
        minBatteryDefault = minBatteryDefault ?: current.minBatteryDefault,
    )

    companion object {
        private val THEMES = setOf("system", "light", "dark")

        fun from(s: AppSettings): SettingsSnapshot = SettingsSnapshot(
            globalIntervalMinutes = s.globalIntervalMinutes,
            concurrency = s.concurrency,
            bandwidthLimitKBps = s.bandwidthLimitKBps,
            trashRetentionDays = s.trashRetentionDays,
            massDeleteMinFiles = s.massDeleteMinFiles,
            massDeleteThresholdPercent = s.massDeleteThresholdPercent,
            conflictPolicyDefault = s.conflictPolicyDefault.name,
            docsFormatDefault = s.docsFormatDefault.name,
            instantMonitoring = s.instantMonitoring,
            foregroundPollSeconds = s.foregroundPollSeconds,
            remotePollMinutes = s.remotePollMinutes,
            notifyOnSuccess = s.notifyOnSuccess,
            notifyOnError = s.notifyOnError,
            notifyProgress = s.notifyProgress,
            theme = s.theme,
            deviceName = s.deviceName,
            historyRetentionDays = s.historyRetentionDays,
            autoBackupConfig = s.autoBackupConfig,
            onDemandCacheMb = s.onDemandCacheMb,
            batteryHintDismissed = s.batteryHintDismissed,
            wifiOnlyDefault = s.wifiOnlyDefault,
            requireChargingDefault = s.requireChargingDefault,
            minBatteryDefault = s.minBatteryDefault,
        )
    }
}

/**
 * Configuration fields of a [FolderPairEntity]. Runtime status (last run, errors, pending approval,
 * pending local changes) is intentionally absent. Option fields are nullable so a missing field falls
 * back to the existing pair's value (merge) or the entity default (new pair).
 */
@Serializable
data class PairSnapshot(
    val id: String = "",
    val accountId: String = "",
    val name: String = "",
    val localPath: String = "",
    val remoteFolderId: String = "",
    val remotePath: String = "",
    val driveId: String? = null,
    val mode: String = "",
    val enabled: Boolean = true,
    // conditions
    val wifiOnly: Boolean? = null,
    val allowMetered: Boolean? = null,
    val allowRoaming: Boolean? = null,
    val requireCharging: Boolean? = null,
    val minBatteryPercent: Int? = null,
    val ssidAllow: String? = null,
    val ssidDeny: String? = null,
    val requireVpn: Boolean? = null,
    // options
    val includePatterns: String? = null,
    val excludePatterns: String? = null,
    val excludedSubfolders: String? = null,
    val syncHidden: Boolean? = null,
    val maxFileSizeBytes: Long? = null,
    val conflictPolicy: String? = null,
    val propagateDeletions: Boolean? = null,
    val massDeleteGuard: Boolean? = null,
    val docsExport: Boolean? = null,
    val docsFormat: String? = null,
    val preserveMtime: Boolean? = null,
    val encrypted: Boolean = false,
    val vaultId: String? = null,
    val intervalMinutes: Int? = null,
    val instantSync: Boolean? = null,
    val createdAt: Long? = null,
) {
    companion object {
        fun from(p: FolderPairEntity): PairSnapshot = PairSnapshot(
            id = p.id,
            accountId = p.accountId,
            name = p.name,
            localPath = p.localPath,
            remoteFolderId = p.remoteFolderId,
            remotePath = p.remotePath,
            driveId = p.driveId,
            mode = p.mode.name,
            enabled = p.enabled,
            wifiOnly = p.wifiOnly,
            allowMetered = p.allowMetered,
            allowRoaming = p.allowRoaming,
            requireCharging = p.requireCharging,
            minBatteryPercent = p.minBatteryPercent,
            ssidAllow = p.ssidAllow,
            ssidDeny = p.ssidDeny,
            requireVpn = p.requireVpn,
            includePatterns = p.includePatterns,
            excludePatterns = p.excludePatterns,
            excludedSubfolders = p.excludedSubfolders,
            syncHidden = p.syncHidden,
            maxFileSizeBytes = p.maxFileSizeBytes,
            conflictPolicy = p.conflictPolicy.name,
            propagateDeletions = p.propagateDeletions,
            massDeleteGuard = p.massDeleteGuard,
            docsExport = p.docsExport,
            docsFormat = p.docsFormat.name,
            preserveMtime = p.preserveMtime,
            encrypted = p.encrypted,
            vaultId = p.vaultId,
            intervalMinutes = p.intervalMinutes,
            instantSync = p.instantSync,
            createdAt = p.createdAt,
        )
    }
}

/** Vault metadata only. The derived key never leaves the device; the passphrase must be entered again after a restore. */
@Serializable
data class VaultSnapshot(
    val id: String = "",
    val name: String = "",
    val saltB64: String = "",
    val iterations: Int = 0,
    val keyCheckB64: String = "",
    val createdAt: Long? = null,
    /** Vault format (1 or 2); older backups have none and hold v1 vaults only. */
    val version: Int = 1,
    /** v2: the vault.json text (salt, Argon2id cost, key check; no secrets). */
    val manifestJson: String? = null,
) {
    companion object {
        fun from(v: VaultEntity): VaultSnapshot = VaultSnapshot(v.id, v.name, v.saltB64, v.iterations, v.keyCheckB64, v.createdAt, v.version, v.manifestJson)
    }
}

@Serializable
data class PinSnapshot(
    val accountId: String = "",
    val fileId: String = "",
    val isFolder: Boolean = false,
    val createdAt: Long? = null,
) {
    companion object {
        fun from(p: PinEntity): PinSnapshot = PinSnapshot(p.accountId, p.fileId, p.isFolder, p.createdAt)
    }
}

/** Case-insensitive enum lookup that returns null instead of throwing on unknown names. */
inline fun <reified E : Enum<E>> parseEnum(name: String?): E? {
    val n = name?.trim()?.takeIf { it.isNotEmpty() } ?: return null
    return enumValues<E>().firstOrNull { it.name.equals(n, ignoreCase = true) }
}
