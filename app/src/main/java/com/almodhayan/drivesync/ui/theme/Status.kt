package com.almodhayan.drivesync.ui.theme

import androidx.annotation.StringRes
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings

/** One status tone: [content] for icons and labels on any surface, [container] as a chip/banner ground, [onContainer] for text on it. */
@Immutable
data class StatusColor(val content: Color, val container: Color, val onContainer: Color)

@Immutable
data class StatusColors(
    val synced: StatusColor,
    val syncing: StatusColor,
    val waiting: StatusColor,
    val approval: StatusColor,
    val conflict: StatusColor,
    val failed: StatusColor,
) {
    fun of(status: SyncStatus): StatusColor = when (status) {
        SyncStatus.Synced -> synced
        SyncStatus.Syncing -> syncing
        SyncStatus.Waiting -> waiting
        SyncStatus.Approval -> approval
        SyncStatus.Conflict -> conflict
        SyncStatus.Failed -> failed
    }
}

// The family's status tones (Modhayan Labs › Status system, Sync group): synced = positive, syncing = info,
// waiting = neutral, needs approval and conflict = attention (told apart by icon and label), failed = critical.
private val PositiveLight = StatusColor(Color(0xFF0B7550), Color(0xFFE2F1E9), Color(0xFF05432D))
private val InfoLight = StatusColor(Color(0xFF0A6B8F), Color(0xFFD4ECF8), Color(0xFF0B3D52))
private val NeutralLight = StatusColor(Color(0xFF6B675F), Color(0xFFEEECE6), Color(0xFF6B675F))
private val AttentionLight = StatusColor(Color(0xFF9A5E00), Color(0xFFFFF1DA), Color(0xFF5A3600))
private val CriticalLight = StatusColor(Color(0xFFD0342C), Color(0xFFFCE8E6), Color(0xFF8C1D18))

private val PositiveDark = StatusColor(Color(0xFF4CC897), Color(0xFF123A2B), Color(0xFFA6EBCB))
private val InfoDark = StatusColor(Color(0xFF7CC8EC), Color(0xFF173A4A), Color(0xFFCFEAF8))
private val NeutralDark = StatusColor(Color(0xFFA29D94), Color(0xFF2A2824), Color(0xFFA29D94))
private val AttentionDark = StatusColor(Color(0xFFFFB547), Color(0xFF3A2A10), Color(0xFFFFDDA8))
private val CriticalDark = StatusColor(Color(0xFFFF8A80), Color(0xFF3D1A17), Color(0xFFFFD3CE))

val StatusLight = StatusColors(
    synced = PositiveLight, syncing = InfoLight, waiting = NeutralLight,
    approval = AttentionLight, conflict = AttentionLight, failed = CriticalLight,
)

val StatusDark = StatusColors(
    synced = PositiveDark, syncing = InfoDark, waiting = NeutralDark,
    approval = AttentionDark, conflict = AttentionDark, failed = CriticalDark,
)

val LocalStatusColors = staticCompositionLocalOf { StatusLight }

/**
 * The six states. Use exactly these; don't invent a seventh.
 * Loud states (approval, conflict, failed) get a card action, the Problems badge and a notification.
 * Every state is tone + icon + label: approval and conflict share the attention tone and differ by icon and word.
 */
enum class SyncStatus(@StringRes private val labelRes: Int, val loud: Boolean, val urgency: Int) {
    Synced(R.string.common_synced, loud = false, urgency = 5),
    Syncing(R.string.common_syncing, loud = false, urgency = 3),
    Waiting(R.string.home_status_waiting, loud = false, urgency = 4),
    Approval(R.string.home_status_approval, loud = true, urgency = 0),
    Conflict(R.string.common_conflict, loud = true, urgency = 2),
    Failed(R.string.home_status_failed, loud = true, urgency = 1);

    /** The state's word, in the app's language. */
    val label: String get() = Strings.get(labelRes)

    /** Default icon. Waiting may override it (bolt for charging, schedule for a scheduled run, pause when paused). */
    val icon: ImageVector
        get() = when (this) {
            Synced -> Sym.CloudDoneFill
            Syncing -> Sym.Sync
            Waiting -> Sym.Wifi
            Approval -> Sym.FrontHandFill
            Conflict -> Sym.CallSplit
            Failed -> Sym.ErrorFill
        }

    companion object {
        /** Needs approval → Failed → Conflict → Syncing → Waiting → Synced. */
        fun mostUrgent(all: Iterable<SyncStatus>): SyncStatus? = all.minByOrNull { it.urgency }
    }
}
