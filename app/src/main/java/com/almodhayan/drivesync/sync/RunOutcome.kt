package com.almodhayan.drivesync.sync

import com.almodhayan.drivesync.core.ErrorKind
import com.almodhayan.drivesync.data.db.RunStatus

/** How a run ended, as notifications and screens use it (mapped from the engine's outcome). */
data class RunOutcome(val runId: String, val status: RunStatus, val message: String?, val summary: PlanSummary?, val errorKind: ErrorKind? = null)
