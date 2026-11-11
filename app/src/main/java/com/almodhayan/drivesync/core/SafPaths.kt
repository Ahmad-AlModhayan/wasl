package com.almodhayan.drivesync.core

/** Folders chosen with Android's folder picker (SRS FR-R3): `content://…/tree/…` URIs. */
object SafPaths {
    fun isSaf(path: String): Boolean = path.startsWith("content://")

    /**
     * The file path behind a shared-storage tree (primary:DCIM/Camera →
     * /storage/emulated/0/DCIM/Camera), or null when another app provides the folder.
     */
    fun filePath(treeUri: String): String? {
        val prefix = "content://com.android.externalstorage.documents/tree/"
        if (!treeUri.startsWith(prefix)) return null
        val id = java.net.URLDecoder.decode(treeUri.removePrefix(prefix).substringBefore('/').replace("+", "%2B"), "UTF-8")
        val volume = id.substringBefore(':')
        val rel = id.substringAfter(':', "").trim('/')
        val base = if (volume == "primary") "/storage/emulated/0" else "/storage/$volume"
        return if (rel.isEmpty()) base else "$base/$rel"
    }

    /** Whether Wasl can watch the folder for changes: a path, or a tree on shared storage. */
    fun watchable(path: String): Boolean = !isSaf(path) || filePath(path) != null
}
