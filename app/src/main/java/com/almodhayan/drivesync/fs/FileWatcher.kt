package com.almodhayan.drivesync.fs

import android.os.FileObserver
import java.io.File
import java.util.concurrent.ConcurrentHashMap

/**
 * Best-effort recursive change monitoring. inotify misses events on FUSE-backed storage, so
 * callers also keep periodic scans; this only makes the common case feel instant.
 */
class FileWatcher(private val root: File, private val onChange: (String) -> Unit) {
    private val observers = ConcurrentHashMap<String, FileObserver>()

    fun start() {
        watchTree(root)
    }

    fun stop() {
        observers.values.forEach { runCatching { it.stopWatching() } }
        observers.clear()
    }

    private fun watchTree(dir: File) {
        if (!dir.isDirectory) return
        if (observers.size > MAX_DIRS) return
        watch(dir)
        dir.listFiles()?.forEach { if (it.isDirectory && !it.name.startsWith(".")) watchTree(it) }
    }

    private fun watch(dir: File) {
        val path = dir.absolutePath
        if (observers.containsKey(path)) return
        val obs = object : FileObserver(dir, MASK) {
            override fun onEvent(event: Int, name: String?) {
                val kind = event and ALL_EVENTS
                if (name == null || name.endsWith(LocalFs.PART_SUFFIX)) return
                val changed = File(dir, name)
                if ((kind == CREATE || kind == MOVED_TO) && changed.isDirectory) watchTree(changed)
                if (kind == DELETE_SELF || kind == MOVE_SELF) observers.remove(path)?.stopWatching()
                onChange(changed.absolutePath)
            }
        }
        observers[path] = obs
        obs.startWatching()
    }

    companion object {
        private const val MASK = FileObserver.CLOSE_WRITE or FileObserver.CREATE or FileObserver.DELETE or
            FileObserver.MOVED_FROM or FileObserver.MOVED_TO or FileObserver.DELETE_SELF or FileObserver.MOVE_SELF or FileObserver.MODIFY
        private const val MAX_DIRS = 4000
    }
}
