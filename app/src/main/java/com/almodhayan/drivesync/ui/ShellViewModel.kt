package com.almodhayan.drivesync.ui

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.ui.status.PairStatusResolver
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import javax.inject.Inject

/** App-wide state for the shell: the Problems badge (approvals + conflicts + failures). */
@HiltViewModel
class ShellViewModel @Inject constructor(db: Db) : ViewModel() {
    val problemCount: StateFlow<Int> = combine(db.pairs().observeAll(), db.conflicts().observeOpenCountsByPair()) { pairs, counts ->
        PairStatusResolver.problemCount(pairs, counts.associate { it.pairId to it.n })
    }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), 0)
}
