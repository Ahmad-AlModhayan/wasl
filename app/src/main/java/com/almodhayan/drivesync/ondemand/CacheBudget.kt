package com.almodhayan.drivesync.ondemand

/**
 * One on-demand budget (`AppSettings.onDemandCacheMb`) shared by the two caches, so together they
 * never hold more than the setting: the engine's block cache ([EngineReader], where streamed Drive
 * and vault blocks go) gets three quarters, [OnDemandCache] (Google document exports, unsent edits,
 * whole pinned copies) the rest. Each has a floor so a tiny setting still works.
 */
object CacheBudget {
    private const val MB = 1024L * 1024L
    const val MIN_ENGINE_MB = 12L
    const val MIN_FILES_MB = 4L

    /** The engine block cache's share, in bytes. */
    fun engineBytes(settingMb: Int): Long = maxOf(MIN_ENGINE_MB, settingMb.toLong() * 3 / 4) * MB

    /** [OnDemandCache]'s share, in bytes. */
    fun filesBytes(settingMb: Int): Long = maxOf(MIN_FILES_MB, settingMb.toLong() - settingMb.toLong() * 3 / 4) * MB
}
