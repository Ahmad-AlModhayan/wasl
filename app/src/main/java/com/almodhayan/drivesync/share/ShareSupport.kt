package com.almodhayan.drivesync.share

import android.content.ContentResolver
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import androidx.core.content.IntentCompat
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.AppPrivate
import com.almodhayan.drivesync.core.Strings
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.io.OutputStream
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter

/**
 * What another app handed to the share target. Streams win over text: when an app sends both, the
 * text is usually a caption for the files. Plain text alone is saved as a .txt file.
 */
class SharedPayload(
    val uris: List<Uri>,
    val text: String?,
    val subject: String?,
    /** Streams left out by [ShareUriPolicy]: file:// paths and this app's own content. */
    val refused: Int = 0,
) {
    val hasText: Boolean get() = uris.isEmpty() && !text.isNullOrBlank()
    val isEmpty: Boolean get() = uris.isEmpty() && !hasText
    val count: Int get() = if (hasText) 1 else uris.size

    fun describe(): String = when {
        hasText -> subject?.takeIf { it.isNotBlank() }?.let { Strings.get(R.string.share_describe_text_subject, it) } ?: Strings.get(R.string.share_describe_text)
        else -> Strings.plural(R.plurals.share_describe_files, count, count.toString())
    }

    companion object {
        fun from(intent: Intent?, packageName: String): SharedPayload {
            if (intent == null) return SharedPayload(emptyList(), null, null)
            val all: List<Uri> = when (intent.action) {
                Intent.ACTION_SEND -> listOfNotNull(IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java))
                Intent.ACTION_SEND_MULTIPLE -> IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java)?.filterNotNull() ?: emptyList()
                else -> emptyList()
            }
            val uris = all.filter { ShareUriPolicy.accepts(it.scheme, it.authority, packageName) }
            return SharedPayload(uris, intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString(), intent.getStringExtra(Intent.EXTRA_SUBJECT), all.size - uris.size)
        }
    }
}

/**
 * Which shared streams are read (security review 2026-10, "Dirty Stream"): only content:// from
 * another app. This app's own authorities would make Wasl copy its private files (tokens, vault
 * keys, the database) into a synced folder; file:// paths are read with Wasl's own storage access,
 * not the sender's, so another app could name any file Wasl can read. Pure, for JVM tests.
 */
object ShareUriPolicy {
    fun accepts(scheme: String?, authority: String?, packageName: String): Boolean =
        scheme.equals(ContentResolver.SCHEME_CONTENT, ignoreCase = true) && !authority.isNullOrBlank() &&
            !AppPrivate.isOwnAuthority(authority, packageName)
}

/** File-name helpers. Pure functions. */
object ShareNaming {
    private val forbidden = Regex("[\\\\/:*?\"<>|\\p{Cntrl}]")
    private val extension = Regex("\\.[A-Za-z0-9]{1,8}$")
    private const val MAX_LEN = 180

    /** Strips path separators and characters that are unsafe on FAT/Drive; null when nothing usable is left. */
    fun sanitize(raw: String?): String? {
        val s = raw?.replace(forbidden, "_")?.trim()?.trim('.', ' ') ?: return null
        if (s.isEmpty() || s.all { it == '_' }) return null
        return if (s.length > MAX_LEN) s.take(MAX_LEN) else s
    }

    fun hasExtension(name: String): Boolean = extension.containsMatchIn(name)

    /** Appends [ext] when [name] has no extension of its own. */
    fun withExtension(name: String, ext: String?): String = if (ext.isNullOrBlank() || hasExtension(name)) name else "$name.$ext"

    /** "photo.jpg" → "photo (2).jpg" → "photo (3).jpg" until [exists] says no. */
    fun unique(name: String, exists: (String) -> Boolean): String {
        if (!exists(name)) return name
        val dot = name.lastIndexOf('.')
        val base = if (dot > 0) name.substring(0, dot) else name
        val ext = if (dot > 0) name.substring(dot) else ""
        var i = 2
        while (true) {
            val candidate = "$base ($i)$ext"
            if (!exists(candidate)) return candidate
            i++
        }
    }

    fun timestamp(now: Long): String = DateTimeFormatter.ofPattern("yyyyMMdd-HHmmss").withZone(ZoneId.systemDefault()).format(Instant.ofEpochMilli(now))

    /** Name for shared plain text: the subject when there is one, else "shared-<timestamp>.txt". */
    fun textFileName(subject: String?, now: Long): String = sanitize(subject)?.let { withExtension(it, "txt") } ?: "shared-${timestamp(now)}.txt"

    fun fallbackName(now: Long, ext: String?): String = withExtension("shared-${timestamp(now)}", ext)
}

/** Copies shared content into a folder. Reads content URIs only through the resolver; no permissions are persisted. */
class ShareCopier(private val resolver: ContentResolver, private val packageName: String) {
    class Result(val added: Int, val failures: List<String>)

    suspend fun copy(payload: SharedPayload, dir: File): Result = withContext(Dispatchers.IO) {
        if (!dir.isDirectory && !dir.mkdirs()) throw IOException("Cannot create folder ${dir.path}")
        var added = 0
        val failures = ArrayList<String>()
        val now = System.currentTimeMillis()

        if (payload.hasText) {
            val name = ShareNaming.unique(ShareNaming.textFileName(payload.subject, now)) { File(dir, it).exists() }
            try {
                writeAtomic(File(dir, name)) { it.write(payload.text.orEmpty().toByteArray(Charsets.UTF_8)) }
                added++
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                failures += "$name: ${e.message ?: Strings.get(R.string.share_write_failed)}"
            }
        }

        for (uri in payload.uris) {
            if (!ShareUriPolicy.accepts(uri.scheme, uri.authority, packageName)) continue   // from() filtered these already
            val name = ShareNaming.unique(nameFor(uri, now)) { File(dir, it).exists() }
            try {
                val input = resolver.openInputStream(uri) ?: throw IOException(Strings.get(R.string.share_read_not_allowed))
                input.use { src -> writeAtomic(File(dir, name)) { out -> src.copyTo(out, 256 * 1024) } }
                added++
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                failures += "$name: ${e.message ?: Strings.get(R.string.share_copy_failed)}"
            }
        }
        Result(added, failures)
    }

    private fun writeAtomic(target: File, write: (OutputStream) -> Unit) {
        val part = File(target.parentFile, target.name + ".part")
        try {
            FileOutputStream(part).use { out -> write(out); out.fd.sync() }
            if (!part.renameTo(target)) throw IOException("cannot rename ${part.name}")
        } catch (e: Exception) {
            part.delete()
            throw e
        }
    }

    private fun nameFor(uri: Uri, now: Long): String {
        val mime = runCatching { resolver.getType(uri) }.getOrNull()
        val ext = mime?.let { MimeTypeMap.getSingleton().getExtensionFromMimeType(it) }
        val display = runCatching {
            resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
                val i = c.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                if (i >= 0 && c.moveToFirst()) c.getString(i) else null
            }
        }.getOrNull()
        val base = ShareNaming.sanitize(display) ?: ShareNaming.sanitize(uri.lastPathSegment)
        return if (base != null) ShareNaming.withExtension(base, ext) else ShareNaming.fallbackName(now, ext)
    }
}
