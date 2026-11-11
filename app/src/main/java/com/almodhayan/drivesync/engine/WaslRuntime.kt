package com.almodhayan.drivesync.engine

import android.content.Context
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.crypto.VaultManager
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import com.almodhayan.drivesync.drive.DriveAuth
import com.almodhayan.drivesync.sync.Conditions
import com.almodhayan.drivesync.sync.PairProgress
import com.almodhayan.drivesync.sync.SyncStateHolder
import com.kahflabs.wasl.ffi.EngineInit
import com.kahflabs.wasl.ffi.Platform
import com.kahflabs.wasl.ffi.ProgressSink
import com.kahflabs.wasl.ffi.SafBridge
import com.kahflabs.wasl.ffi.TokenSource
import com.kahflabs.wasl.ffi.VaultAccess
import com.kahflabs.wasl.ffi.WaslEngine
import com.kahflabs.wasl.ffi.WaslException
import com.kahflabs.wasl.ffi.core.DeviceState
import com.kahflabs.wasl.ffi.engine.Progress
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeoutOrNull
import java.io.File
import java.util.concurrent.atomic.AtomicLong
import javax.inject.Inject
import javax.inject.Provider
import javax.inject.Singleton

/**
 * The one libwasl engine of the app. Every sync decision is made inside it (SRS FR-E3); this class
 * only supplies what Android knows: OAuth tokens, network and battery state, vault keys from the
 * Keystore, and SAF access. It also feeds live progress to [SyncStateHolder] and tells screens when
 * the engine's data changed.
 */
@Singleton
class WaslRuntime @Inject constructor(
    @ApplicationContext private val context: Context,
    private val auth: DriveAuth,
    private val conditions: Conditions,
    private val vaults: Provider<VaultManager>,
    private val saf: Provider<SafBridgeImpl>,
    private val state: SyncStateHolder,
    private val settings: Provider<SettingsRepository>,
) {
    private val _changes = MutableSharedFlow<Unit>(replay = 1, extraBufferCapacity = 16, onBufferOverflow = BufferOverflow.DROP_OLDEST)

    /** Ticks after anything in the engine's database may have changed. Screens re-query on it. */
    val changes: SharedFlow<Unit> = _changes

    init { _changes.tryEmit(Unit) }

    fun changed() { _changes.tryEmit(Unit) }

    private val lastProgressTick = AtomicLong(0)

    val engine: WaslEngine by lazy {
        val files = context.filesDir
        val external = context.getExternalFilesDir(null) ?: files
        WaslEngine(
            EngineInit(
                dbPath = File(files, "wasl.db").path,
                // Same volume as most pair folders, so binning is a rename; invisible to media scanners.
                binDir = File(external, "wasl-bin").path,
                stagingDir = File(context.cacheDir, "wasl-staging").path,
                // The name set in Settings › Phone name, which conflict copies carry (it was the model
                // code whatever the setting said; UX audit 2026-10). A change applies from the next start.
                // Bounded: the first use of the engine can come from the main thread, which must not
                // wait on the settings file.
                deviceName = runBlocking { withTimeoutOrNull(500) { settings.get().current().deviceName } }
                    ?: android.os.Build.MODEL ?: "Android",
                driveBaseUrl = null,
                driveUploadUrl = null,
            ),
            Tokens(),
            AndroidPlatform(),
            Sink(),
        )
    }

    private inner class Tokens : TokenSource {
        override fun accessToken(account: String): String = try {
            runBlocking { auth.accessToken(account) }
        } catch (e: com.almodhayan.drivesync.core.AppError.Auth) {
            throw WaslException.Auth(e.message ?: Strings.get(R.string.sync_fix_sign_in))
        } catch (e: com.almodhayan.drivesync.core.AppError.Network) {
            throw WaslException.Network(e.message ?: Strings.get(R.string.engine_no_connection))
        } catch (e: Exception) {
            throw WaslException.Other(e.message ?: e.javaClass.simpleName)
        }

        override fun invalidate(account: String) = auth.invalidate(account)
    }

    private inner class AndroidPlatform : Platform {
        override fun deviceState(): DeviceState {
            val d = conditions.device()
            return DeviceState(
                online = d.online, wifi = d.wifi, metered = d.metered, vpn = d.vpn, roaming = d.roaming,
                ssid = d.ssid, charging = d.charging, batteryPercent = d.battery.coerceIn(0, 100).toUByte(),
            )
        }

        override fun saf(): SafBridge = saf.get()

        override fun vaultAccess(vaultId: String): VaultAccess? {
            val manager = vaults.get()
            // 64 bytes for a v1 vault, 32 for a v2 one: the engine tells them apart by the manifest.
            val material = manager.loadKeyMaterial(vaultId) ?: return null
            return try {
                VaultAccess(manifestJson = runBlocking { manager.exportManifest(vaultId) }, keys = material)
            } catch (e: Exception) {
                material.fill(0)
                throw e
            }
        }
    }

    private inner class Sink : ProgressSink {
        override fun onProgress(progress: Progress) {
            val p = progress.toPairProgress(state.running.value[progress.pairId]?.startedAt)
            if (state.isRunning(p.pairId)) state.update(p.pairId) { p } else state.start(p)
            // Screens re-query at most every second while a run is moving.
            val now = System.currentTimeMillis()
            if (now - lastProgressTick.get() > 1000) { lastProgressTick.set(now); changed() }
        }
    }
}

internal fun Progress.toPairProgress(startedAt: Long?): PairProgress = PairProgress(
    pairId = pairId,
    runId = runId,
    phase = phase.toKotlin(),
    totalActions = totalActions.toInt(),
    doneActions = doneActions.toInt(),
    bytesTotal = bytesTotal.toLong(),
    bytesDone = bytesDone.toLong(),
    currentFile = currentFile,
    startedAt = startedAt ?: System.currentTimeMillis(),
    errors = errors.toInt(),
    checked = checked.toInt(),
)
