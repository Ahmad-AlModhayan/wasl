package com.almodhayan.drivesync.scheduler

import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.core.SafPaths

/**
 * The status service runs whenever a pair is turned on, so the status notification is always
 * there; it watches folders only for active instant-sync pairs while Instant sync is on.
 */
object MonitorPolicy {
    fun shouldRun(pairs: List<FolderPairEntity>): Boolean = pairs.any { it.enabled }

    fun watches(instantMonitoring: Boolean, pair: FolderPairEntity): Boolean =
        instantMonitoring && pair.enabled && !pair.paused && pair.instantSync

    /**
     * The folder on disk to watch for [pair]: its path, or for a folder chosen with Android's picker
     * on shared storage the path behind the tree (#133). Null for a folder another app provides.
     */
    fun watchPath(pair: FolderPairEntity): String? =
        if (SafPaths.isSaf(pair.localPath)) SafPaths.filePath(pair.localPath) else pair.localPath

    /**
     * The pairs a MediaStore change wakes: path pairs with a watcher, as before, and folder-picker
     * pairs without one (without All files access their path can't always be watched, #133). A
     * folder-picker pair with a watcher already wakes on its own changes.
     */
    fun mediaTargets(pairs: List<FolderPairEntity>, watched: Set<String>): List<String> =
        pairs.filter { if (SafPaths.isSaf(it.localPath)) it.id !in watched else it.id in watched }.map { it.id }
}
