package com.almodhayan.drivesync.core

/**
 * Glob matcher with the same surface as the incumbent app so users can keep their patterns:
 *  ?        one character except '/'
 *  *        any characters except '/'
 *  **       any characters including '/'
 *  [abc]    character class, [!abc] negated class, ranges allowed
 *  {1..9}   numeric range (max 2000 values)
 * Patterns are case-insensitive. A pattern without a leading '/' is implicitly prefixed with "**\/"
 * so it matches at any depth; a leading '/' anchors it to the pair root. A trailing '/' matches
 * directories only.
 */
class Glob private constructor(val pattern: String, private val regex: Regex, val dirOnly: Boolean) {

    /** @param relPath path relative to the pair root, '/'-separated, no leading slash. */
    fun matches(relPath: String, isDir: Boolean): Boolean {
        if (dirOnly && !isDir) return false
        return regex.matches(relPath.trimStart('/'))
    }

    companion object {
        fun compile(raw: String): Glob {
            var p = raw.trim()
            require(p.isNotEmpty()) { "empty pattern" }
            val dirOnly = p.endsWith("/")
            if (dirOnly) p = p.trimEnd('/')
            val anchored = p.startsWith("/")
            p = p.trimStart('/')
            if (!anchored && !p.startsWith("**/")) p = "**/$p"
            return Glob(raw, Regex(toRegex(p), RegexOption.IGNORE_CASE), dirOnly)
        }

        private fun toRegex(p: String): String {
            val sb = StringBuilder("^")
            var i = 0
            while (i < p.length) {
                val c = p[i]
                when (c) {
                    '*' -> {
                        if (i + 1 < p.length && p[i + 1] == '*') {
                            // "**/" should also match zero directories
                            if (i + 2 < p.length && p[i + 2] == '/') { sb.append("(?:.*/)?"); i += 3; continue }
                            sb.append(".*"); i += 2; continue
                        }
                        sb.append("[^/]*")
                    }
                    '?' -> sb.append("[^/]")
                    '[' -> {
                        val end = p.indexOf(']', i + 1)
                        if (end < 0) { sb.append("\\["); i++; continue }
                        var cls = p.substring(i + 1, end)
                        val neg = cls.startsWith("!") || cls.startsWith("^")
                        if (neg) cls = cls.substring(1)
                        sb.append('[').append(if (neg) "^" else "")
                        sb.append(cls.replace("\\", "\\\\"))
                        sb.append(']')
                        i = end + 1; continue
                    }
                    '{' -> {
                        val end = p.indexOf('}', i + 1)
                        val body = if (end > 0) p.substring(i + 1, end) else null
                        val range = body?.let { RANGE.matchEntire(it) }
                        if (range != null) {
                            val a = range.groupValues[1].toLong(); val b = range.groupValues[2].toLong()
                            val lo = minOf(a, b); val hi = maxOf(a, b)
                            if (hi - lo <= 2000) {
                                sb.append("(?:")
                                sb.append((lo..hi).joinToString("|") { Regex.escape(it.toString()) })
                                sb.append(')')
                                i = end + 1; continue
                            }
                        }
                        if (body != null && body.contains(',')) {
                            sb.append("(?:").append(body.split(',').joinToString("|") { Regex.escape(it) }).append(')')
                            i = end + 1; continue
                        }
                        sb.append("\\{")
                    }
                    '.', '(', ')', '+', '|', '^', '$', '\\' -> sb.append('\\').append(c)
                    else -> sb.append(c)
                }
                i++
            }
            sb.append('$')
            return sb.toString()
        }

        private val RANGE = Regex("""\s*(\d+)\s*\.\.\s*(\d+)\s*""")
    }
}

/** Include/exclude evaluation with directory pruning semantics. */
class PathFilter(
    includes: List<String>,
    excludes: List<String>,
    excludedSubfolders: List<String>,
    private val syncHidden: Boolean,
) {
    private val inc = includes.mapNotNull { runCatching { Glob.compile(it) }.getOrNull() }
    private val exc = excludes.mapNotNull { runCatching { Glob.compile(it) }.getOrNull() }
    private val excDirs = excludedSubfolders.map { it.trim('/').lowercase() }.filter { it.isNotEmpty() }

    /** true = keep. For directories, false means prune the whole subtree. */
    fun accept(relPath: String, isDir: Boolean): Boolean {
        val rel = relPath.trim('/')
        if (rel.isEmpty()) return true
        val name = rel.substringAfterLast('/')
        if (INTERNAL_NAMES.any { name.endsWith(it) }) return false
        if (!syncHidden && rel.split('/').any { it.startsWith(".") }) return false
        val lower = rel.lowercase()
        if (excDirs.any { lower == it || lower.startsWith("$it/") }) return false
        if (exc.any { it.matches(rel, isDir) }) return false
        if (inc.isNotEmpty() && !isDir) {
            return inc.any { it.matches(rel, false) }
        }
        return true
    }

    companion object {
        /** Suffixes of files we create ourselves and must never sync. */
        val INTERNAL_NAMES = listOf(".drivesync-part", ".drivesync-tmp")
    }
}
