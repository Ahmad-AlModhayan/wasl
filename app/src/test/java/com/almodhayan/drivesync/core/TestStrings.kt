package com.almodhayan.drivesync.core

import com.almodhayan.drivesync.R
import org.w3c.dom.Element
import java.io.File
import java.util.Locale
import javax.xml.parsers.DocumentBuilderFactory

/**
 * [Strings] for plain JVM tests: the English text from src/main/res/values, formatted as Android
 * would (positional %1$s args, plurals by English rules). Found by name from [Strings].
 */
object TestStrings : Strings.Fallback {
    private val names: Map<Int, String> by lazy {
        (listOf("string", "plurals").mapNotNull { runCatching { Class.forName("${R::class.java.name}$$it") }.getOrNull() }.flatMap { it.fields.toList() }).associate { it.getInt(null) to it.name }
    }

    private val values: Pair<Map<String, String>, Map<String, Map<String, String>>> by lazy {
        val strings = HashMap<String, String>()
        val plurals = HashMap<String, Map<String, String>>()
        val dir = listOf(File("src/main/res/values"), File("app/src/main/res/values")).first { it.isDirectory }
        dir.listFiles { f -> f.extension == "xml" }!!.forEach { f ->
            val doc = DocumentBuilderFactory.newInstance().newDocumentBuilder().parse(f)
            val s = doc.getElementsByTagName("string")
            for (i in 0 until s.length) (s.item(i) as Element).let { strings[it.getAttribute("name")] = unescape(it.textContent) }
            val p = doc.getElementsByTagName("plurals")
            for (i in 0 until p.length) {
                val e = p.item(i) as Element
                val items = e.getElementsByTagName("item")
                plurals[e.getAttribute("name")] = (0 until items.length).associate { j ->
                    (items.item(j) as Element).let { it.getAttribute("quantity") to unescape(it.textContent) }
                }
            }
        }
        strings to plurals
    }

    private fun unescape(s: String): String {
        val t = s.trim().removeSurrounding("\"")
        val out = StringBuilder()
        var i = 0
        while (i < t.length) {
            val c = t[i]
            if (c == '\\' && i + 1 < t.length) {
                out.append(if (t[i + 1] == 'n') '\n' else t[i + 1])
                i += 2
            } else {
                out.append(c); i++
            }
        }
        return out.toString()
    }

    override fun string(id: Int, args: Array<out Any>): String {
        val raw = values.first[names[id]] ?: return "#$id"
        return if (args.isEmpty()) raw else String.format(Locale.US, raw, *args)
    }

    override fun plural(id: Int, count: Int, args: Array<out Any>): String {
        val forms = values.second[names[id]] ?: return "#$id:$count"
        val raw = (if (count == 1) forms["one"] else forms["other"]) ?: forms["other"] ?: return "#$id:$count"
        return String.format(Locale.US, raw, *args)
    }
}
