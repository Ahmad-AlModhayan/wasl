package com.almodhayan.drivesync.ui.devices

import android.content.Context
import androidx.compose.runtime.Immutable
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import com.almodhayan.drivesync.devices.DeviceRunNote
import com.almodhayan.drivesync.devices.DeviceSessionService
import com.almodhayan.drivesync.devices.DeviceSyncManager
import com.almodhayan.drivesync.devices.Window
import com.almodhayan.drivesync.engine.WaslRuntime
import com.kahflabs.wasl.ffi.core.Route
import dagger.hilt.android.lifecycle.HiltViewModel
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.wasl_devices.DeviceInfo
import uniffi.wasl_devices.OfferInfo
import uniffi.wasl_devices.PairSharing
import uniffi.wasl_devices.PairingStatus
import javax.inject.Inject

@Immutable
data class DevicesUi(
    val open: Boolean = false,
    val problem: String? = null,
    val me: DeviceInfo? = null,
    val others: List<DeviceInfo> = emptyList(),
    val session: Boolean = false,
    val charging: Boolean = false,
    /** Folders other devices offer this phone, still waiting for an answer (docs/INTRODUCTIONS.md §3.3). */
    val offers: List<OfferInfo> = emptyList(),
)

/** The Devices screen: this phone, its group with who is connected and how, and when it takes part. */
@HiltViewModel
class DevicesViewModel @Inject constructor(
    @ApplicationContext private val context: Context,
    private val manager: DeviceSyncManager,
    private val settings: SettingsRepository,
) : ViewModel() {

    val ui: StateFlow<DevicesUi> = combine(manager.open, manager.info, manager.problem, manager.windows, settings.settings) { open, info, problem, windows, s ->
        val list = info?.devices.orEmpty()
        DevicesUi(
            open = open,
            problem = problem,
            me = list.firstOrNull { it.thisDevice },
            others = list.filter { !it.thisDevice }.sortedWith(compareByDescending<DeviceInfo> { it.connected }.thenBy { it.name.lowercase() }),
            session = Window.SESSION in windows,
            charging = s.devicesWhileCharging,
            offers = if (open) waiting(runCatching { manager.withDevices { it.offers() } }.getOrNull().orEmpty()) else emptyList(),
        )
    }.flowOn(Dispatchers.IO).stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), DevicesUi())

    private val _messages = MutableSharedFlow<String>(extraBufferCapacity = 8)
    val messages: SharedFlow<String> = _messages

    init {
        // Connections come and go without a pairing event; keep the list current while it is shown.
        viewModelScope.launch {
            while (isActive) {
                manager.refresh()
                delay(3_000)
            }
        }
    }

    fun remove(d: DeviceInfo) = io {
        manager.withDevices { it.removeDevice(d.id) }
        manager.refresh()
        _messages.tryEmit(Strings.get(R.string.devices_removed, d.name))
    }

    /** Keeps a device another one added: it may then sync what is shared with all devices (§3.1). */
    fun keep(d: DeviceInfo) = io {
        manager.withDevices { it.keepDevice(d.id) }
        manager.refresh()
        _messages.tryEmit(Strings.get(R.string.intro_kept, d.name))
    }

    fun decline(o: OfferInfo) = io {
        manager.withDevices { it.declineOffer(o.share, o.fromDevice) }
        manager.refresh()
        _messages.tryEmit(Strings.get(R.string.intro_declined, o.name, o.fromName))
    }

    fun setSession(on: Boolean) {
        if (on) DeviceSessionService.start(context) else DeviceSessionService.stop(context)
    }

    fun setCharging(on: Boolean) = io { settings.update { it.copy(devicesWhileCharging = on) } }

    fun say(text: String) { _messages.tryEmit(text) }

    private fun io(block: suspend () -> Unit) {
        viewModelScope.launch(Dispatchers.IO) {
            try {
                block()
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _messages.tryEmit(AppError.wrap(t).userHint)
            }
        }
    }
}

/** Add a device (show a code) or join one (scan or paste a code), then the four words. */
@HiltViewModel
class PairingViewModel @Inject constructor(
    handle: SavedStateHandle,
    private val manager: DeviceSyncManager,
    private val runtime: WaslRuntime,
) : ViewModel() {
    val joining: Boolean = handle.get<String>("mode") == MODE_JOIN

    val open: StateFlow<Boolean> = manager.open
    val status: StateFlow<PairingStatus?> = manager.pairing

    private val _started = MutableStateFlow(false)
    /** A code was made (add) or used (join) on this screen. */
    val started: StateFlow<Boolean> = _started

    private val _messages = MutableSharedFlow<String>(extraBufferCapacity = 8)
    val messages: SharedFlow<String> = _messages

    init {
        viewModelScope.launch {
            while (isActive) {
                manager.refresh()
                delay(1_000)
            }
        }
    }

    /** Opens a code to show (once the phone takes part). */
    fun invite() = io {
        if (manager.withDevices { it.invite() } != null) _started.value = true
        manager.refresh()
    }

    fun join(code: String) {
        val c = Qr.clean(code)
        if (c.isEmpty()) { _messages.tryEmit(Strings.get(R.string.devices_pairing_bad_code)); return }
        io {
            manager.withDevices { it.join(c) } ?: run { _messages.tryEmit(Strings.get(R.string.devices_pairing_not_open)); return@io }
            _started.value = true
            manager.refresh()
        }
    }

    fun confirm(accept: Boolean) = io {
        manager.withDevices { it.confirmPairing(accept) }
        manager.refresh()
    }

    /** Starts over: a new code (add) or the scanner again (join). */
    fun again() = io {
        manager.withDevices { it.cancelPairing() }
        _started.value = false
        manager.refresh()
        if (!joining) invite()
    }

    private val _offerable = MutableStateFlow<List<Pair<String, String>>?>(null)
    /** While the "offer folders" dialog is open: the pairs that sync with devices, as (id, name). */
    val offerable: StateFlow<List<Pair<String, String>>?> = _offerable

    fun loadOfferable() = io {
        _offerable.value = runtime.engine.pairs().filter { it.route != Route.DRIVE }.map { it.id to it.name }
    }

    fun closeOfferable() { _offerable.value = null }

    /** Offers the ticked pairs to the device just paired, keeping whoever they were shared with (§3.4). */
    fun offerTo(peer: String, peerName: String, pairIds: Set<String>) = io {
        _offerable.value = null
        if (pairIds.isEmpty()) return@io
        for (id in pairIds) {
            manager.withDevices { d ->
                val now = d.sharing(id)
                d.shareWith(id, now?.allDevices ?: false, (now?.devices.orEmpty() + peer).distinct())
            }
        }
        runtime.changed()
        _messages.tryEmit(Strings.plural(R.plurals.intro_offered, pairIds.size, pairIds.size, peerName))
    }

    override fun onCleared() {
        // Leaving the screen ends a pairing that hasn't finished; one that has stays.
        if (_started.value) runCatching { manager.withDevices { it.cancelPairing() } }
        manager.refresh()
    }

    private fun io(block: suspend () -> Unit) {
        viewModelScope.launch(Dispatchers.IO) {
            try {
                block()
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _messages.tryEmit(AppError.wrap(t).userHint)
            }
        }
    }

    companion object {
        const val MODE_ADD = "add"
        const val MODE_JOIN = "join"
    }
}

@Immutable
data class PairDevicesUi(
    val open: Boolean = false,
    val route: Route = Route.DRIVE,
    val hasDrive: Boolean = true,
    val code: String? = null,
    val waiting: String? = null,
    val last: DeviceRunNote? = null,
    /** Who the pair is shared with (docs/INTRODUCTIONS.md §3.2), and the group to choose from. */
    val sharing: PairSharing? = null,
    val others: List<DeviceInfo> = emptyList(),
    /** The devices this shared pair syncs with, when none of them is connected now ("Waiting for …"). */
    val waitingFor: List<String> = emptyList(),
)

/** A pair's Devices section: share it with the group (its code), or join another device's folder. */
@HiltViewModel
class PairDevicesViewModel @Inject constructor(
    handle: SavedStateHandle,
    private val manager: DeviceSyncManager,
    private val runtime: WaslRuntime,
) : ViewModel() {
    private val pairId: String = handle.get<String>("pairId") ?: ""

    val ui: StateFlow<PairDevicesUi> = combine(manager.open, runtime.changes, manager.runs, manager.knownDevices, manager.info) { open, _, runs, _, _ -> open to runs }
        .map { (open, runs) ->
            withContext(Dispatchers.IO) {
                val pair = runCatching { runtime.engine.pair(pairId) }.getOrNull()
                val route = pair?.route ?: Route.DRIVE
                val sharing = if (route != Route.DRIVE) runCatching { manager.withDevices { it.sharing(pairId) } }.getOrNull() else null
                val others = manager.info.value?.devices.orEmpty().filter { !it.thisDevice }
                PairDevicesUi(
                    open = open,
                    route = route,
                    hasDrive = pair?.driveFolderId != null,
                    code = if (route != Route.DRIVE) runCatching { manager.withDevices { it.shareCode(pairId) } }.getOrNull() else null,
                    waiting = if (route == Route.DEVICES && !open) manager.waitingText() else null,
                    last = runs[pairId],
                    sharing = sharing,
                    others = others,
                    waitingFor = if (open) waitingFor(sharing, others) else emptyList(),
                )
            }
        }
        .flowOn(Dispatchers.IO)
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), PairDevicesUi())

    private val _messages = MutableSharedFlow<String>(extraBufferCapacity = 8)
    val messages: SharedFlow<String> = _messages

    fun setShared(on: Boolean) = io {
        manager.withDevices { if (on) it.sharePair(pairId, null) else it.unsharePair(pairId) }
        runtime.changed()
        if (on) manager.syncPair(pairId)
    }

    /** Shares the pair with all devices known here and/or the devices ticked; each ticked one is offered it. */
    fun shareWith(all: Boolean, devices: List<String>) = io {
        manager.withDevices { it.shareWith(pairId, all, devices) }
        runtime.changed()
        _messages.tryEmit(Strings.get(R.string.intro_choose_done))
    }

    fun useCode(code: String) = io {
        manager.withDevices { it.sharePair(pairId, Qr.clean(code)) }
        runtime.changed()
        manager.syncPair(pairId)
    }

    fun say(text: String) { _messages.tryEmit(text) }

    private fun io(block: suspend () -> Unit) {
        viewModelScope.launch(Dispatchers.IO) {
            try {
                block()
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                _messages.tryEmit(AppError.wrap(t).userHint)
            }
        }
    }
}

/**
 * The devices a shared pair syncs with when none of them is connected: the pair waits for them, as
 * the desktop's "Waiting for <device>" says (#103). Empty when one is connected or none is chosen.
 */
internal fun waitingFor(sharing: PairSharing?, others: List<DeviceInfo>): List<String> {
    if (sharing == null) return emptyList()
    val peers = if (sharing.allDevices) others else others.filter { it.id in sharing.devices }
    return if (peers.isEmpty() || peers.any { it.connected }) emptyList() else peers.map { it.name }
}
