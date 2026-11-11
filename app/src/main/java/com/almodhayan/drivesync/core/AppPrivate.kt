package com.almodhayan.drivesync.core

import android.content.Context
import com.almodhayan.drivesync.ui.components.Fmt
import java.io.File
import java.io.IOException

/**
 * What counts as the app's own private storage and its own content URIs (security review 2026-10).
 * Input from outside the app (an imported backup, a share intent) must never point Wasl at its own
 * files: they hold the Google tokens, vault keys and the sync database. The checks are pure string
 * work, so they are unit tested on the JVM; only [roots] touches Android.
 */
object AppPrivate {
    /** True for an authority of this app ("com.kahflabs.wasl", "com.kahflabs.wasl.documents"). */
    fun isOwnAuthority(authority: String?, packageName: String): Boolean {
        if (authority.isNullOrBlank() || packageName.isBlank()) return false
        // Android resolves "0@com.kahflabs.wasl" (a user id) as this app's own provider, so compare
        // the host only, without any "user@" prefix or ":port".
        val host = authority.substringAfterLast('@').substringBefore(':')
        return host.equals(packageName, ignoreCase = true) || host.startsWith("$packageName.", ignoreCase = true)
    }

    /**
     * True when [path] (a file path or a folder chosen in the system picker) is inside one of
     * [roots] or one of the well-known places of [packageName]'s private data. Case is ignored:
     * shared storage is case-insensitive, so "/storage/emulated/0/android/DATA/<pkg>" is the same folder.
     * [canonical] resolves links for a path that exists (`/proc/self/root/...`, symlinked folders);
     * both the path as written and as resolved must pass. Anything under /proc is refused outright.
     */
    fun isPrivateLocalPath(
        path: String,
        roots: Collection<String>,
        packageName: String,
        canonical: (String) -> String? = { null },
    ): Boolean {
        val file = if (path.startsWith("content://")) {
            val authority = path.removePrefix("content://").substringBefore('/')
            if (isOwnAuthority(authority, packageName)) return true
            Fmt.safFilePath(path) ?: return false
        } else {
            path
        }
        if (isPrivateSegments(normalize(file), roots, packageName)) return true
        // The same folder through a link: /proc/self/root/data/user/0/<pkg>, a symlinked folder.
        val resolved = canonical(file) ?: return false
        return isPrivateSegments(normalize(resolved), roots, packageName)
    }

    private fun isPrivateSegments(segments: List<String>, roots: Collection<String>, packageName: String): Boolean {
        // /proc reaches every folder of this process (/proc/self/root, /proc/self/cwd): never a pair.
        if (segments.firstOrNull() == "proc") return true
        if (roots.any { root -> normalize(root).let { r -> r.isNotEmpty() && isPrefix(r, segments) } }) return true
        if (packageName.isBlank()) return false
        // Aliases a root list can miss, at any depth: /sdcard, /storage/self/primary, other users'
        // /data/user/<n>, and the same places seen through a mount or link prefix.
        for (i in segments.indices) {
            if (i + 2 < segments.size && segments[i].equals("Android", true) && segments[i + 1].equals("data", true) && segments[i + 2].equals(packageName, true)) return true
            if (i + 2 < segments.size && segments[i] == "data" && segments[i + 1] == "data" && segments[i + 2].equals(packageName, true)) return true
            if (i + 3 < segments.size && segments[i] == "data" && (segments[i + 1] == "user" || segments[i + 1] == "user_de") && segments[i + 3].equals(packageName, true)) return true
        }
        return false
    }

    /**
     * The app's private folders on this device: data, files, cache, no-backup and the app's folder
     * in shared storage (Android/data/<pkg>), each also as its canonical path.
     */
    fun roots(context: Context): List<String> {
        val dirs = listOfNotNull(
            context.dataDir, context.filesDir, context.cacheDir, context.noBackupFilesDir,
            context.getExternalFilesDir(null)?.parentFile, context.externalCacheDir?.parentFile,
        )
        return dirs.flatMap { d -> listOfNotNull(d.path, canonical(d)) }.distinct()
    }

    private fun canonical(f: File): String? = try { f.canonicalPath } catch (e: IOException) { null }

    /** [path] with links resolved, when it exists; null otherwise (nothing to resolve). */
    fun canonicalPath(path: String): String? = File(path).takeIf { it.exists() }?.let(::canonical)

    /** "/a/./b//c/../d/" → [a, b, d]. Pure, so a path with ".." cannot step out of the check. */
    internal fun normalize(path: String): List<String> {
        val out = ArrayList<String>()
        for (part in path.replace('\\', '/').split('/')) {
            when (part) {
                "", "." -> Unit
                ".." -> if (out.isNotEmpty()) out.removeAt(out.size - 1)
                else -> out += part
            }
        }
        return out
    }

    private fun isPrefix(root: List<String>, path: List<String>): Boolean =
        path.size >= root.size && root.indices.all { root[it].equals(path[it], ignoreCase = true) }
}
