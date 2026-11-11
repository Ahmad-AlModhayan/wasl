package com.almodhayan.drivesync.sync

import kotlinx.serialization.Serializable

/** Counts and samples of a plan, as the screens show them (mapped from the engine's summary). */
@Serializable
data class PlanSummary(
    val uploads: Int = 0, val uploadBytes: Long = 0,
    val downloads: Int = 0, val downloadBytes: Long = 0,
    val exports: Int = 0,
    val deleteLocal: Int = 0, val deleteRemote: Int = 0,
    val conflicts: Int = 0, val mkdirs: Int = 0, val skips: Int = 0,
    val localFiles: Int = 0, val remoteFiles: Int = 0,
    val warnings: List<String> = emptyList(),
    val blockedReason: String? = null,
    val sampleDeletes: List<String> = emptyList(),
    val unchanged: Int = 0,                 // files that already match and will not be touched
    val blockedKind: String? = null,        // "deletions" | "wipe" | "first-merge"
    val blockedSide: String? = null,        // where files would be deleted: "drive" | "phone"
    val blockedPercent: Int = 0,
    val blockedCount: Int = 0,
    /** "Then delete" methods: files that go to a bin once they arrived on the other side. */
    val movedAfterTransfer: Int = 0,
) {
    val isEmpty: Boolean get() = uploads + downloads + exports + deleteLocal + deleteRemote + conflicts + mkdirs == 0
}
