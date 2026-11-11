package com.almodhayan.drivesync.ui.onboarding

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import javax.inject.Inject

/** Onboarding's own state is the step it shows; sign-in lives in AccountsViewModel, permissions in DeviceChecks. */
@HiltViewModel
class OnboardingViewModel @Inject constructor(
    private val settings: SettingsRepository,
) : ViewModel() {

    private var finishing = false

    /** Marks onboarding as done, then runs [onDone] (navigation) on the main thread. */
    fun finish(onDone: () -> Unit) {
        if (finishing) return
        finishing = true
        viewModelScope.launch {
            withContext(Dispatchers.IO) { settings.update { it.copy(onboardingDone = true) } }
            onDone()
        }
    }
}
