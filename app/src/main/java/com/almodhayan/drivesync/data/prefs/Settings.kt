package com.almodhayan.drivesync.data.prefs

import android.content.Context
import android.os.Build
import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.booleanPreferencesKey
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.intPreferencesKey
import androidx.datastore.preferences.core.longPreferencesKey
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import androidx.datastore.core.handlers.ReplaceFileCorruptionHandler
import androidx.datastore.preferences.core.emptyPreferences
import com.almodhayan.drivesync.data.db.ConflictPolicy
import com.almodhayan.drivesync.data.db.DocsFormat
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import javax.inject.Inject
import javax.inject.Singleton

// A damaged settings file falls back to defaults instead of crashing every start and every sync.
private val Context.dataStore: DataStore<Preferences> by preferencesDataStore(
    name = "settings",
    corruptionHandler = ReplaceFileCorruptionHandler { emptyPreferences() },
)

data class AppSettings(
    val onboardingDone: Boolean = false,
    val globalIntervalMinutes: Int = 60,
    val concurrency: Int = 3,
    val bandwidthLimitKBps: Int = 0,
    val trashRetentionDays: Int = 30,
    val massDeleteMinFiles: Int = 20,
    val massDeleteThresholdPercent: Int = 5,
    val conflictPolicyDefault: ConflictPolicy = ConflictPolicy.KEEP_BOTH,
    val docsFormatDefault: DocsFormat = DocsFormat.OFFICE,
    val instantMonitoring: Boolean = true,
    val foregroundPollSeconds: Int = 60,
    val remotePollMinutes: Int = 5,
    val notifyOnSuccess: Boolean = false,
    val notifyOnError: Boolean = true,
    val notifyProgress: Boolean = true,
    val theme: String = "system",          // system | light | dark
    val deviceName: String = Build.MODEL ?: "Android",
    val historyRetentionDays: Int = 90,
    val lastBackupAt: Long = 0,
    val autoBackupConfig: Boolean = true,
    val onDemandCacheMb: Int = 2048,         // files on demand: 2 GB on the phone (docs/ON_DEMAND.md §2.1)
    val batteryHintDismissed: Boolean = false,
    val wifiOnlyDefault: Boolean = true,
    val requireChargingDefault: Boolean = false,
    val minBatteryDefault: Int = 15,
    val dynamicColor: Boolean = false,         // Appearance › Device colours (family default: off)
    val localVersionsKept: Int = 5,            // overwritten phone versions kept per file
    val maxFileSizeDefaultMb: Int = 2048,      // new pairs skip files over 2 GB
    val massDeleteGuardDefault: Boolean = true,
    val automationEnabled: Boolean = false,    // Settings › Allow automation; never restored from a backup
    val automationSecret: String = "",        // per install, created when automation is first turned on
    val devicesWhileCharging: Boolean = false, // Settings › Sync with devices while charging on Wi-Fi (FR-N12)
)

@Singleton
class SettingsRepository @Inject constructor(@ApplicationContext private val context: Context) {
    private object K {
        val onboardingDone = booleanPreferencesKey("onboardingDone")
        val globalIntervalMinutes = intPreferencesKey("globalIntervalMinutes")
        val concurrency = intPreferencesKey("concurrency")
        val bandwidthLimitKBps = intPreferencesKey("bandwidthLimitKBps")
        val trashRetentionDays = intPreferencesKey("trashRetentionDays")
        val massDeleteMinFiles = intPreferencesKey("massDeleteMinFiles")
        val massDeleteThresholdPercent = intPreferencesKey("massDeleteThresholdPercent")
        val conflictPolicyDefault = stringPreferencesKey("conflictPolicyDefault")
        val docsFormatDefault = stringPreferencesKey("docsFormatDefault")
        val instantMonitoring = booleanPreferencesKey("instantMonitoring")
        val foregroundPollSeconds = intPreferencesKey("foregroundPollSeconds")
        val remotePollMinutes = intPreferencesKey("remotePollMinutes")
        val notifyOnSuccess = booleanPreferencesKey("notifyOnSuccess")
        val notifyOnError = booleanPreferencesKey("notifyOnError")
        val notifyProgress = booleanPreferencesKey("notifyProgress")
        val theme = stringPreferencesKey("theme")
        val deviceName = stringPreferencesKey("deviceName")
        val historyRetentionDays = intPreferencesKey("historyRetentionDays")
        val lastBackupAt = longPreferencesKey("lastBackupAt")
        val autoBackupConfig = booleanPreferencesKey("autoBackupConfig")
        val onDemandCacheMb = intPreferencesKey("onDemandCacheMb")
        val batteryHintDismissed = booleanPreferencesKey("batteryHintDismissed")
        val wifiOnlyDefault = booleanPreferencesKey("wifiOnlyDefault")
        val requireChargingDefault = booleanPreferencesKey("requireChargingDefault")
        val minBatteryDefault = intPreferencesKey("minBatteryDefault")
        val dynamicColor = booleanPreferencesKey("dynamicColor")
        val localVersionsKept = intPreferencesKey("localVersionsKept")
        val maxFileSizeDefaultMb = intPreferencesKey("maxFileSizeDefaultMb")
        val massDeleteGuardDefault = booleanPreferencesKey("massDeleteGuardDefault")
        val automationEnabled = booleanPreferencesKey("automationEnabled")
        val automationSecret = stringPreferencesKey("automationSecret")
        val devicesWhileCharging = booleanPreferencesKey("devicesWhileCharging")
    }

    val settings: Flow<AppSettings> = context.dataStore.data.map { p ->
        val d = AppSettings()
        AppSettings(
            onboardingDone = p[K.onboardingDone] ?: d.onboardingDone,
            globalIntervalMinutes = p[K.globalIntervalMinutes] ?: d.globalIntervalMinutes,
            concurrency = p[K.concurrency] ?: d.concurrency,
            bandwidthLimitKBps = p[K.bandwidthLimitKBps] ?: d.bandwidthLimitKBps,
            trashRetentionDays = p[K.trashRetentionDays] ?: d.trashRetentionDays,
            massDeleteMinFiles = p[K.massDeleteMinFiles] ?: d.massDeleteMinFiles,
            massDeleteThresholdPercent = p[K.massDeleteThresholdPercent] ?: d.massDeleteThresholdPercent,
            conflictPolicyDefault = p[K.conflictPolicyDefault]?.let { runCatching { ConflictPolicy.valueOf(it) }.getOrNull() } ?: d.conflictPolicyDefault,
            docsFormatDefault = p[K.docsFormatDefault]?.let { runCatching { DocsFormat.valueOf(it) }.getOrNull() } ?: d.docsFormatDefault,
            instantMonitoring = p[K.instantMonitoring] ?: d.instantMonitoring,
            foregroundPollSeconds = p[K.foregroundPollSeconds] ?: d.foregroundPollSeconds,
            remotePollMinutes = p[K.remotePollMinutes] ?: d.remotePollMinutes,
            notifyOnSuccess = p[K.notifyOnSuccess] ?: d.notifyOnSuccess,
            notifyOnError = p[K.notifyOnError] ?: d.notifyOnError,
            notifyProgress = p[K.notifyProgress] ?: d.notifyProgress,
            theme = p[K.theme] ?: d.theme,
            deviceName = p[K.deviceName] ?: phoneName() ?: d.deviceName,
            historyRetentionDays = p[K.historyRetentionDays] ?: d.historyRetentionDays,
            lastBackupAt = p[K.lastBackupAt] ?: d.lastBackupAt,
            autoBackupConfig = p[K.autoBackupConfig] ?: d.autoBackupConfig,
            onDemandCacheMb = p[K.onDemandCacheMb] ?: d.onDemandCacheMb,
            batteryHintDismissed = p[K.batteryHintDismissed] ?: d.batteryHintDismissed,
            wifiOnlyDefault = p[K.wifiOnlyDefault] ?: d.wifiOnlyDefault,
            requireChargingDefault = p[K.requireChargingDefault] ?: d.requireChargingDefault,
            minBatteryDefault = p[K.minBatteryDefault] ?: d.minBatteryDefault,
            dynamicColor = p[K.dynamicColor] ?: d.dynamicColor,
            localVersionsKept = p[K.localVersionsKept] ?: d.localVersionsKept,
            maxFileSizeDefaultMb = p[K.maxFileSizeDefaultMb] ?: d.maxFileSizeDefaultMb,
            massDeleteGuardDefault = p[K.massDeleteGuardDefault] ?: d.massDeleteGuardDefault,
            automationEnabled = p[K.automationEnabled] ?: d.automationEnabled,
            automationSecret = p[K.automationSecret] ?: d.automationSecret,
            devicesWhileCharging = p[K.devicesWhileCharging] ?: d.devicesWhileCharging,
        )
    }

    suspend fun current(): AppSettings = settings.first()

    /** The name the person gave the phone in Android's settings ("Galaxy A54"), not the model code (UX audit 2026-10). */
    private fun phoneName(): String? =
        runCatching { android.provider.Settings.Global.getString(context.contentResolver, android.provider.Settings.Global.DEVICE_NAME) }
            .getOrNull()?.trim()?.takeIf { it.isNotEmpty() }

    suspend fun update(block: (AppSettings) -> AppSettings) {
        val next = block(current())
        context.dataStore.edit { p ->
            p[K.onboardingDone] = next.onboardingDone
            p[K.globalIntervalMinutes] = next.globalIntervalMinutes
            p[K.concurrency] = next.concurrency
            p[K.bandwidthLimitKBps] = next.bandwidthLimitKBps
            p[K.trashRetentionDays] = next.trashRetentionDays
            p[K.massDeleteMinFiles] = next.massDeleteMinFiles
            p[K.massDeleteThresholdPercent] = next.massDeleteThresholdPercent
            p[K.conflictPolicyDefault] = next.conflictPolicyDefault.name
            p[K.docsFormatDefault] = next.docsFormatDefault.name
            p[K.instantMonitoring] = next.instantMonitoring
            p[K.foregroundPollSeconds] = next.foregroundPollSeconds
            p[K.remotePollMinutes] = next.remotePollMinutes
            p[K.notifyOnSuccess] = next.notifyOnSuccess
            p[K.notifyOnError] = next.notifyOnError
            p[K.notifyProgress] = next.notifyProgress
            p[K.theme] = next.theme
            p[K.deviceName] = next.deviceName
            p[K.historyRetentionDays] = next.historyRetentionDays
            p[K.lastBackupAt] = next.lastBackupAt
            p[K.autoBackupConfig] = next.autoBackupConfig
            p[K.onDemandCacheMb] = next.onDemandCacheMb
            p[K.batteryHintDismissed] = next.batteryHintDismissed
            p[K.wifiOnlyDefault] = next.wifiOnlyDefault
            p[K.requireChargingDefault] = next.requireChargingDefault
            p[K.minBatteryDefault] = next.minBatteryDefault
            p[K.dynamicColor] = next.dynamicColor
            p[K.localVersionsKept] = next.localVersionsKept
            p[K.maxFileSizeDefaultMb] = next.maxFileSizeDefaultMb
            p[K.massDeleteGuardDefault] = next.massDeleteGuardDefault
            p[K.automationEnabled] = next.automationEnabled
            p[K.automationSecret] = next.automationSecret
            p[K.devicesWhileCharging] = next.devicesWhileCharging
        }
    }
}
