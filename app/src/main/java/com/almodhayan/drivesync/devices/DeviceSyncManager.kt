package com.almodhayan.drivesync.devices

import android.content.Context
import android.net.ConnectivityManager
import android.net.wifi.WifiManager
import android.os.BatteryManager
import android.util.Log
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import com.almodhayan.drivesync.engine.WaslRuntime
import com.kahflabs.wasl.ffi.DeviceEvents
import com.kahflabs.wasl.ffi.DevicesInit
import com.kahflabs.wasl.ffi.WaslDevices
import com.kahflabs.wasl.ffi.outcomeHasNoPeer
import com.kahflabs.wasl.ffi.core.Route
import com.kahflabs.wasl.ffi.core.RunStatus
import com.kahflabs.wasl.ffi.engine.RunOutcome
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineExceptionHandler
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import uniffi.wasl_devices.DevicesInfo
import uniffi.wasl_devices.PairingStatus
import java.io.File
import javax.inject.Inject
import javax.inject.Singleton

/** The last time a pair ran with devices, for its screen (the engine keeps Drive runs only). */
/** The last device run of a pair. [skipped]: nothing synced, and [message] says why (#148). */
data class DeviceRunNote(
    val at: Long,
    val ok: Boolean,
    val message: String?,
    val skipped: Boolean = false,
    /** No device that has the pair was connected ([outcomeHasNoPeer]): worded by the app. */
    val noPeer: Boolean = false,
)

/**
 * This phone in the device group (docs/DEVICE_SYNC.md, SRS FR-N12). It owns the one [WaslDevices]
 * while the phone takes part: while the app is open, during a sync session the person started, and
 * while charging on Wi-Fi when allowed. Outside those windows it is closed: no listening, no
 * discovery, and pairs synced only with devices wait.
 */
@Singleton
class DeviceSyncManager @Inject constructor(
    @ApplicationContext private val context: Context,
    private val runtime: WaslRuntime,
    private val settings: SettingsRepository,
) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO + CoroutineExceptionHandler { _, t -> Log.w(TAG, "Device sync task failed", t) })
    private val mutex = Mutex()
    private val prefs by lazy { context.getSharedPreferences(PREFS, Context.MODE_PRIVATE) }

    @Volatile private var devices: WaslDevices? = null
    private var multicast: WifiManager.MulticastLock? = null
    private var periodic: Job? = null

    private val _windows = MutableStateFlow<Set<Window>>(emptySet())
    val windows: StateFlow<Set<Window>> = _windows.asStateFlow()

    private val _open = MutableStateFlow(false)
    /** The phone takes part right now. */
    val open: StateFlow<Boolean> = _open.asStateFlow()

    private val _info = MutableStateFlow<DevicesInfo?>(null)
    /** This phone and its group, while open. */
    val info: StateFlow<DevicesInfo?> = _info.asStateFlow()

    private val _pairing = MutableStateFlow<PairingStatus?>(null)
    val pairing: StateFlow<PairingStatus?> = _pairing.asStateFlow()

    private val _problem = MutableStateFlow<String?>(null)
    /** Why the phone could not take part, in the person's words. */
    val problem: StateFlow<String?> = _problem.asStateFlow()

    private val _runs = MutableStateFlow<Map<String, DeviceRunNote>>(emptyMap())
    val runs: StateFlow<Map<String, DeviceRunNote>> = _runs.asStateFlow()

    private val _names = MutableStateFlow<List<String>>(emptyList())
    /** The other devices' names, remembered from the last time the phone took part. */
    val knownDevices: StateFlow<List<String>> = _names.asStateFlow()

    private var started = false

    /** Follows the windows (call once, from the Application). */
    fun start() {
        if (started) return
        started = true
        _names.value = prefs.getString(KEY_NAMES, null)?.split('\n')?.filter { it.isNotBlank() } ?: emptyList()
        scope.launch {
            _windows.map { Participation.shouldOpen(it) }.distinctUntilChanged().collectLatest { want ->
                if (want) openNow() else closeNow()
            }
        }
        ProcessLifecycleOwner.get().lifecycle.addObserver(object : DefaultLifecycleObserver {
            override fun onStart(owner: LifecycleOwner) = enter(Window.APP_OPEN)
            override fun onStop(owner: LifecycleOwner) = leave(Window.APP_OPEN)
        })
    }

    fun enter(w: Window) = _windows.update { it + w }
    fun leave(w: Window) = _windows.update { it - w }

    /** Runs [block] with the open [WaslDevices]; null when the phone is not taking part. */
    fun <T> withDevices(block: (WaslDevices) -> T): T? = devices?.let(block)

    // ------------------------------------------------------------------------ open and close

    private suspend fun openNow() = mutex.withLock {
        if (devices != null) return@withLock
        try {
            val dir = File(context.filesDir, "devices").apply { mkdirs() }
            val name = settings.current().deviceName
            acquireMulticast()
            // The binding's async constructor runs on the engine's tokio runtime.
            val d = WaslDevices.open(
                runtime.engine,
                DevicesInit(dataDir = dir.path, deviceName = name, port = 0u, bindAddress = null, discover = true),
                DeviceSeedFile(File(dir, KEY_FILE), AndroidKeystoreWrapKey()),
                Events(),
            )
            devices = d
            _problem.value = null
            _open.value = true
            refresh()
            // Catch up on what changed while the phone was away, then keep checking while open.
            periodic = scope.launch {
                while (isActive) {
                    applyBudget(d)
                    syncDevicePairs()
                    delay(PERIODIC_MS)
                }
            }
        } catch (e: CancellationException) {
            releaseMulticast()
            throw e
        } catch (t: Throwable) {
            Log.w(TAG, "Couldn't start device sync: ${t.javaClass.simpleName}")
            _problem.value = t.message ?: Strings.get(R.string.devices_couldnt_start)
            releaseMulticast()
        }
    }

    private suspend fun closeNow() = mutex.withLock {
        periodic?.cancel()
        periodic = null
        val d = devices ?: return@withLock
        devices = null
        _open.value = false
        withContext(Dispatchers.IO) {
            runCatching { d.shutdown() }
            runCatching { d.destroy() }
        }
        releaseMulticast()
        _info.value = null
        _pairing.value = null
    }

    /**
     * Tells device sync what it may spend on compression (docs/COMPRESSION.md §2.3): on mobile
     * data it asks peers to compress harder; on battery it compresses what it sends only lightly.
     * Checked before every round, so a charger or a network change is followed within one round.
     */
    private fun applyBudget(d: WaslDevices) {
        runCatching {
            val metered = context.getSystemService(ConnectivityManager::class.java)?.isActiveNetworkMetered ?: false
            val charging = context.getSystemService(BatteryManager::class.java)?.isCharging ?: true
            d.setBudget(metered, !charging)
        }
    }

    private fun acquireMulticast() {
        if (multicast?.isHeld == true) return
        runCatching {
            val wm = context.applicationContext.getSystemService(WifiManager::class.java) ?: return
            multicast = wm.createMulticastLock("wasl-devices").apply { setReferenceCounted(false); acquire() }
        }
    }

    private fun releaseMulticast() {
        runCatching { multicast?.takeIf { it.isHeld }?.release() }
        multicast = null
    }

    // ------------------------------------------------------------------------ state

    /** Reads devices and pairing again (the binding said they changed). */
    fun refresh() {
        val d = devices ?: return
        val info = runCatching { d.info() }.getOrNull()
        _info.value = info
        _pairing.value = runCatching { d.pairingStatus() }.getOrNull()
        val names = info?.devices?.filter { !it.thisDevice }?.map { it.name } ?: return
        if (names != _names.value) {
            _names.value = names
            prefs.edit().putString(KEY_NAMES, names.joinToString("\n")).apply()
        }
    }

    private inner class Events : DeviceEvents {
        // Called on the engine's threads, possibly under its locks: read back later, never in here.
        override fun changed() {
            scope.launch {
                refresh()
                runtime.changed()
            }
        }

        override fun pairChanged(pairId: String) {
            scope.launch { syncPair(pairId) }
        }
    }

    // ------------------------------------------------------------------------ running pairs

    /** Syncs one pair with the connected devices; null when the phone is not taking part. */
    suspend fun syncPair(pairId: String): RunOutcome? {
        val d = devices ?: return null
        val outcome = try {
            d.syncPair(pairId)
        } catch (e: CancellationException) {
            throw e
        } catch (t: Throwable) {
            Log.w(TAG, "Device sync of a pair failed: ${t.javaClass.simpleName}")
            _runs.update { it + (pairId to DeviceRunNote(System.currentTimeMillis(), false, t.message)) }
            return null
        }
        val ok = outcome.status == RunStatus.SUCCESS
        if (outcome.status != RunStatus.SKIPPED || outcome.message != null) {
            val skipped = outcome.status == RunStatus.SKIPPED
            val note = DeviceRunNote(System.currentTimeMillis(), ok, outcome.message, skipped, outcomeHasNoPeer(outcome))
            _runs.update { it + (pairId to note) }
        }
        if (ok) clearWaiting(pairId)
        runtime.changed()
        return outcome
    }

    /** Every turned-on pair that syncs with devices. */
    suspend fun syncDevicePairs() {
        val pairs = withContext(Dispatchers.IO) { runCatching { runtime.engine.pairs() }.getOrDefault(emptyList()) }
        for (p in pairs) {
            if (devices == null) return
            if (p.route != Route.DRIVE && p.enabled && !p.paused) syncPair(p.id)
        }
    }

    /** "Waiting for <device>" for a pair synced only with devices while the phone is not taking part. */
    fun waitingText(): String {
        val who = Participation.waitingFor(_names.value, Strings.get(R.string.notif_list_separator))
        return if (who == null) Strings.get(R.string.devices_waiting_for_devices) else Strings.get(R.string.devices_waiting_for, who)
    }

    /** Records the wait on the pair's card (once), so it says why it hasn't synced. */
    fun recordWaiting(pairId: String) {
        val text = waitingText()
        runCatching { runtime.engine.recordSkip(pairId, text) }
        prefs.edit().putBoolean(KEY_WAITING + pairId, true).apply()
        runtime.changed()
    }

    /** A pair that synced with devices again no longer shows the wait. */
    private fun clearWaiting(pairId: String) {
        if (!prefs.getBoolean(KEY_WAITING + pairId, false)) return
        // The engine records Drive runs only; an empty note clears the wait from the card.
        runCatching { runtime.engine.recordSkip(pairId, "") }
        prefs.edit().remove(KEY_WAITING + pairId).apply()
    }

    companion object {
        private const val TAG = "DeviceSync"
        private const val PREFS = "devices"
        private const val KEY_NAMES = "peerNames"
        private const val KEY_WAITING = "waiting-"
        private const val KEY_FILE = "device.key"
        private const val PERIODIC_MS = 5 * 60_000L
    }
}
