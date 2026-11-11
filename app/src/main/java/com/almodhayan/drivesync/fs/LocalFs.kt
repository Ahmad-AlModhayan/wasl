package com.almodhayan.drivesync.fs

import android.content.Context
import android.os.Environment
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Hashing
import com.almodhayan.drivesync.core.PathFilter
import com.almodhayan.drivesync.core.Strings
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import java.io.File
import javax.inject.Inject
import javax.inject.Singleton

data class LocalEntry(
    val rel: String,             // relative path with local names
    val isDir: Boolean,
    val size: Long,
    val mtime: Long,
    val file: File,
)

@Singleton
open class LocalFs @Inject constructor(@ApplicationContext private val context: Context) {

    open fun hasAllFilesAccess(): Boolean = Environment.isExternalStorageManager()   // open for tests

    /** Recycle bin lives in app-private external storage: same volume as the user's files, invisible to media scanners. */
    fun trashRoot(): File = File(context.getExternalFilesDir(null) ?: context.filesDir, "trash").apply { mkdirs() }

    fun tempFor(target: File): File = File(target.parentFile, target.name + PART_SUFFIX)

    /**
     * Walks [root] honouring [filter]; directories that the filter rejects are pruned. A folder that
     * cannot be listed is not empty: its path goes to [unreadable] so the sync leaves it alone this
     * time, and the pair folder itself being unreadable fails the scan.
     */
    suspend fun scan(root: File, filter: PathFilter, unreadable: MutableList<String>? = null): List<LocalEntry> = withContext(Dispatchers.IO) {
        if (!root.exists()) throw AppError.Io(Strings.get(R.string.fs_folder_missing, root.path))
        if (!root.isDirectory) throw AppError.Io(Strings.get(R.string.fs_not_a_folder, root.path))
        if (!root.canRead()) throw AppError.Permission(Strings.get(R.string.fs_cannot_read, root.path))
        val out = ArrayList<LocalEntry>()
        val stack = ArrayDeque<Pair<File, String>>()
        stack.add(root to "")
        while (stack.isNotEmpty()) {
            currentCoroutineContext().ensureActive()
            val (dir, prefix) = stack.removeLast()
            val kids = listDir(dir) ?: run {
                if (prefix.isEmpty()) throw AppError.Io(Strings.get(R.string.fs_cannot_list, root.path))
                unreadable?.add(prefix)
                null
            } ?: continue
            for (f in kids) {
                val name = f.name
                val rel = if (prefix.isEmpty()) name else "$prefix/$name"
                val isDir = f.isDirectory
                if (!filter.accept(rel, isDir)) continue
                if (isDir) {
                    out += LocalEntry(rel, true, 0, f.lastModified(), f)
                    stack.add(f to rel)
                } else if (f.isFile) {
                    out += LocalEntry(rel, false, f.length(), f.lastModified(), f)
                }
            }
        }
        out
    }

    /** Directory listing; null when the folder cannot be read right now. Open for tests. */
    protected open fun listDir(dir: File): Array<File>? = dir.listFiles()

    suspend fun md5(file: File): String = withContext(Dispatchers.IO) { Hashing.md5(file) }

    /** Atomically replace [target] with [temp]; the previous content is moved to the recycle bin by the caller first. */
    suspend fun commit(temp: File, target: File, mtime: Long?) = withContext(Dispatchers.IO) {
        target.parentFile?.mkdirs()
        if (target.exists() && !target.delete()) throw AppError.Io(Strings.get(R.string.fs_cannot_replace, target.path))
        if (!temp.renameTo(target)) {
            // Cross-volume fallback
            temp.copyTo(target, overwrite = true); temp.delete()
        }
        if (mtime != null && mtime > 0) target.setLastModified(mtime)
        Unit
    }

    /** Moves [file] into the recycle bin and returns its new location. */
    suspend fun moveToTrash(pairId: String, file: File): File = withContext(Dispatchers.IO) {
        val dir = File(trashRoot(), pairId).apply { mkdirs() }
        var dest = File(dir, "${System.currentTimeMillis()}_${file.name}")
        var i = 1
        while (dest.exists()) { dest = File(dir, "${System.currentTimeMillis()}_${i++}_${file.name}") }
        if (!file.renameTo(dest)) {
            file.copyTo(dest, overwrite = true)
            if (!file.delete()) throw AppError.Io(Strings.get(R.string.fs_cannot_remove_after_trash, file.path))
        }
        dest
    }

    suspend fun restoreFromTrash(trashFile: File, original: File): Boolean = withContext(Dispatchers.IO) {
        original.parentFile?.mkdirs()
        if (trashFile.renameTo(original)) return@withContext true
        runCatching { trashFile.copyTo(original, overwrite = true); trashFile.delete() }.isSuccess
    }

    suspend fun deleteEmptyDirs(root: File, rel: String) = withContext(Dispatchers.IO) {
        val f = File(root, rel)
        if (f.isDirectory && (f.list()?.isEmpty() == true)) f.delete()
        Unit
    }

    fun volumes(): List<File> {
        val out = ArrayList<File>()
        out += Environment.getExternalStorageDirectory()
        context.getExternalFilesDirs(null).filterNotNull().forEach { d ->
            // /storage/XXXX-XXXX/Android/data/<pkg>/files → /storage/XXXX-XXXX
            val root = generateSequence(d) { it.parentFile }.firstOrNull { it.name == "Android" }?.parentFile
            if (root != null && out.none { it.path == root.path }) out += root
        }
        return out
    }

    companion object {
        const val PART_SUFFIX = ".drivesync-part"
    }
}
