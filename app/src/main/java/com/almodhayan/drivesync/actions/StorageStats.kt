package com.almodhayan.drivesync.actions

import android.os.Environment
import android.os.StatFs
import javax.inject.Inject
import javax.inject.Singleton

/** Phone storage numbers for the storage bars. */
@Singleton
class StorageStats @Inject constructor() {
    data class Volume(val total: Long, val free: Long) { val used: Long get() = (total - free).coerceAtLeast(0) }

    /** The volume that holds [path] (internal storage by default). */
    fun phoneVolume(path: String = Environment.getExternalStorageDirectory().path): Volume? = runCatching {
        val s = StatFs(path)
        Volume(s.totalBytes, s.availableBytes)
    }.getOrNull()
}
