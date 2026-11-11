package com.almodhayan.drivesync.ondemand

import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import javax.inject.Inject
import javax.inject.Singleton

/** UI writes PinEntity rows through PinDao and then calls [notifyChanged]; the on-demand module reacts. */
@Singleton
class PinRequests @Inject constructor() {
    private val _changes = MutableSharedFlow<Unit>(replay = 0, extraBufferCapacity = 1)
    val changes: SharedFlow<Unit> = _changes
    fun notifyChanged() { _changes.tryEmit(Unit) }
}
