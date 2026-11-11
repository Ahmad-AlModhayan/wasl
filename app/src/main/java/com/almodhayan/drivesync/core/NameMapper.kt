package com.almodhayan.drivesync.core

/**
 * Google Drive allows any name except '/'. Android external storage (FUSE/MediaProvider) enforces
 * FAT-compatible names, rejecting  " * / : < > ? \ |  and control characters. The incumbent app
 * refuses such files; we map them to full-width look-alikes when materialising locally and keep
 * the original remote name in the file state so nothing is ever renamed on Drive.
 */
object NameMapper {
    private val MAP = mapOf(
        '"' to '＂', '*' to '＊', ':' to '：', '<' to '＜', '>' to '＞', '?' to '？', '\\' to '＼', '|' to '｜',
    )

    fun needsMapping(name: String): Boolean =
        name.any { it in MAP || it.code < 0x20 } || name == "." || name == ".." || name.isEmpty() ||
            name.toByteArray(Charsets.UTF_8).size > 255

    fun toLocalName(remoteName: String): String {
        if (!needsMapping(remoteName)) return remoteName
        val sb = StringBuilder(remoteName.length)
        for (ch in remoteName) {
            when {
                ch in MAP -> sb.append(MAP.getValue(ch))
                ch.code < 0x20 -> sb.append('␀')
                else -> sb.append(ch)
            }
        }
        var out = sb.toString()
        if (out == "." || out == "..") out = "＿$out"
        if (out.isEmpty()) out = "＿unnamed"
        // Keep under 255 bytes while preserving the extension when possible.
        val bytes = out.toByteArray(Charsets.UTF_8)
        if (bytes.size > 255) {
            val ext = out.substringAfterLast('.', "")
            val extPart = if (ext.isNotEmpty() && ext.length < 16) ".$ext" else ""
            var base = out.removeSuffix(extPart)
            while ((base + extPart).toByteArray(Charsets.UTF_8).size > 255 && base.isNotEmpty()) base = base.dropLast(1)
            out = base + extPart
        }
        return out
    }

    /** Conflict copy naming: "report (conflicted copy from Pixel, 2026-09-30 14-03).docx" */
    fun conflictName(name: String, device: String, stamp: String): String {
        val dot = name.lastIndexOf('.')
        val hasExt = dot > 0 && dot < name.length - 1 && name.length - dot <= 12
        val base = if (hasExt) name.substring(0, dot) else name
        val ext = if (hasExt) name.substring(dot) else ""
        return "$base (conflicted copy from $device, $stamp)$ext"
    }
}
