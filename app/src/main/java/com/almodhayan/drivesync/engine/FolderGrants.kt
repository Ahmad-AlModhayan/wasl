package com.almodhayan.drivesync.engine

import android.content.Context
import android.content.Intent
import android.net.Uri

/** The folder grants Android keeps for folders chosen in its picker (SRS FR-R3). */
interface FolderGrants {
    /** Whether the app still holds read and write access to the folder at [tree]. */
    fun held(tree: String): Boolean

    /** Lets go of the grant for [tree]; Android caps how many an app may keep. */
    fun release(tree: String)

    companion object {
        /**
         * A deleted pair's folder grant can go when it is a folder-picker grant that no remaining
         * pair, hidden ones (removed, Undo pending) included, still uses (#150 M5).
         */
        fun releasable(deletedPath: String, remaining: List<String>): Boolean =
            deletedPath.startsWith("content://") && deletedPath !in remaining
    }
}

/** [FolderGrants] through the app's content resolver. */
class ResolverGrants(private val context: Context) : FolderGrants {
    override fun held(tree: String): Boolean = runCatching {
        context.contentResolver.persistedUriPermissions.any { it.uri.toString() == tree && it.isReadPermission && it.isWritePermission }
    }.getOrDefault(false)

    override fun release(tree: String) {
        runCatching {
            context.contentResolver.releasePersistableUriPermission(Uri.parse(tree), Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
        }
    }
}
