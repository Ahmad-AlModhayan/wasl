package com.almodhayan.drivesync.ui.drive

import android.content.Context
import android.media.MediaScannerConnection
import android.os.Environment
import java.io.File

/** Helpers for saving copies of Drive content into the public Downloads folder. */
object DownloadsDir {

    @Suppress("DEPRECATION")
    fun dir(): File = Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS).apply { mkdirs() }

    /** "report.pdf" → "report (1).pdf" when a file with that name already exists. */
    fun uniqueFile(name: String): File {
        val clean = name.replace('/', '_').ifBlank { "download" }
        val d = dir()
        var f = File(d, clean)
        if (!f.exists()) return f
        val dot = clean.lastIndexOf('.')
        val base = if (dot > 0) clean.substring(0, dot) else clean
        val ext = if (dot > 0) clean.substring(dot) else ""
        var i = 1
        while (f.exists()) { f = File(d, "$base ($i)$ext"); i++ }
        return f
    }

    /** Makes the file show up in the Files/Downloads apps right away. */
    fun scan(context: Context, file: File) {
        runCatching { MediaScannerConnection.scanFile(context, arrayOf(file.absolutePath), null, null) }
    }
}
