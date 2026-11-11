package com.almodhayan.drivesync.ondemand

import android.content.Context
import android.util.Log
import com.almodhayan.drivesync.crypto.VaultKeyStore
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import com.almodhayan.drivesync.engine.WaslRuntime
import com.kahflabs.wasl.ffi.OnDemandFile
import com.kahflabs.wasl.ffi.OnDemandOrigin
import com.kahflabs.wasl.ffi.OnDemandReader
import com.kahflabs.wasl.ffi.VaultLocks
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import java.io.File
import javax.inject.Inject
import javax.inject.Singleton

private const val TAG = "OnDemand"

/**
 * The engine's on-demand reader (docs/ON_DEMAND.md §2.2): byte ranges of Drive and vault files
 * through libwasl's `Hydrator` and one block cache under `cacheDir/wasl-ondemand`, bounded by its
 * share of `AppSettings.onDemandCacheMb` ([CacheBudget]: three quarters of 2 GB unless changed)
 * and following it live. The provider streams
 * through it; Google documents (exports), unsent edits and pinned copies stay in [OnDemandCache].
 *
 * Vaults: before every read of a vault file the engine asks [VaultKeyStore.has] whether the vault
 * is still unlocked here (no key is unwrapped for that), and [closeVault] (called by VaultManager
 * when a vault is locked or removed) drops its keys and its decrypted blocks; pinned files stay.
 */
@Singleton
class EngineReader @Inject constructor(
    @ApplicationContext private val context: Context,
    private val runtime: WaslRuntime,
    private val settings: SettingsRepository,
    private val keyStore: VaultKeyStore,
) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    private val locks = object : VaultLocks {
        override fun isUnlocked(vaultId: String): Boolean = keyStore.has(vaultId)
    }

    val reader: OnDemandReader by lazy {
        val r = runtime.engine.onDemand(File(context.cacheDir, CACHE_DIR).path, CacheBudget.engineBytes(DEFAULT_LIMIT_MB.toInt()).toULong(), locks)
        scope.launch {
            try {
                settings.settings.map { it.onDemandCacheMb }.distinctUntilChanged().collect { mb ->
                    r.setLimit(CacheBudget.engineBytes(mb.coerceAtLeast(MIN_LIMIT_MB)).toULong())
                }
            } catch (t: Throwable) {
                Log.w(TAG, "on-demand cache limit not followed: ${t.message}")
            }
        }
        r
    }

    /**
     * The vault was locked or removed: its keys leave the reader, and the decrypted blocks read
     * through it leave the phone (also those of earlier runs). Pinned vault files stay, like a
     * synced vault pair's plaintext folder. Blocking: call off the main thread.
     */
    fun closeVault(vaultId: String) {
        try {
            reader.closeVault(vaultId)
        } catch (t: Throwable) {
            Log.w(TAG, "on-demand state of a vault not closed: ${t.message}")
        }
    }

    companion object {
        const val CACHE_DIR = "wasl-ondemand"
        /** docs/ON_DEMAND.md §2.1: 2 GB on the phone. */
        const val DEFAULT_LIMIT_MB = 2048uL
        private const val MIN_LIMIT_MB = 16

        fun drive(accountId: String): OnDemandOrigin = OnDemandOrigin.Drive(accountId)

        /** What identifies a plain Drive file's content: its cache tag (MD5, or version). */
        fun file(doc: MappedDoc): OnDemandFile? =
            doc.size?.takeIf { doc.kind == DocKind.FILE && it >= 0 }?.let { OnDemandFile(doc.fileId, doc.cacheTag, it.toULong()) }
    }
}
