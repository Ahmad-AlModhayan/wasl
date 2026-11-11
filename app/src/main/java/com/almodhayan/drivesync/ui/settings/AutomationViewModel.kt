package com.almodhayan.drivesync.ui.settings

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.almodhayan.drivesync.data.prefs.AppSettings
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import com.almodhayan.drivesync.scheduler.AutomationGate
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import javax.inject.Inject

/** Settings › Automation: the "Allow automation" switch and the secret Tasker must send. */
@HiltViewModel
class AutomationViewModel @Inject constructor(private val repo: SettingsRepository) : ViewModel() {
    val settings: StateFlow<AppSettings?> = repo.settings.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    private val writeLock = Mutex()

    fun setEnabled(enabled: Boolean) {
        viewModelScope.launch { writeLock.withLock { repo.update { it.withAutomation(enabled) } } }
    }

    /** Anything still sending the old secret stops working, which is the point. */
    fun newSecret() {
        viewModelScope.launch { writeLock.withLock { repo.update { it.copy(automationSecret = AutomationGate.newSecret()) } } }
    }
}

/** Turning automation on for the first time creates the secret; turning it off keeps it for next time. */
fun AppSettings.withAutomation(enabled: Boolean): AppSettings = copy(
    automationEnabled = enabled,
    automationSecret = if (enabled && automationSecret.isBlank()) AutomationGate.newSecret() else automationSecret,
)
