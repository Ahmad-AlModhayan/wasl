package com.almodhayan.drivesync.core

import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.UUID

object Ids {
    fun newId(): String = UUID.randomUUID().toString()
}

object TimeFmt {
    private val stamp: DateTimeFormatter = DateTimeFormatter.ofPattern("yyyy-MM-dd HH-mm").withZone(ZoneId.systemDefault())
    private val human: DateTimeFormatter = DateTimeFormatter.ofPattern("yyyy-MM-dd HH:mm").withZone(ZoneId.systemDefault())
    fun stamp(epochMs: Long = System.currentTimeMillis()): String = stamp.format(Instant.ofEpochMilli(epochMs))
    fun human(epochMs: Long): String = human.format(Instant.ofEpochMilli(epochMs))
    fun parseRfc3339(s: String?): Long? = s?.let { runCatching { Instant.parse(it).toEpochMilli() }.getOrNull() }
    fun toRfc3339(epochMs: Long): String = DateTimeFormatter.ISO_INSTANT.format(Instant.ofEpochMilli(epochMs))
}

fun Long.humanBytes(): String {
    if (this < 1024) return "$this B"
    val units = arrayOf("KB", "MB", "GB", "TB")
    var v = this.toDouble() / 1024
    var i = 0
    while (v >= 1024 && i < units.size - 1) { v /= 1024; i++ }
    if (v >= 100) return "%.0f %s".format(java.util.Locale.US, v, units[i])
    val one = "%.1f".format(java.util.Locale.US, v)
    return (if (one.endsWith(".0")) one.dropLast(2) else one) + " " + units[i]
}
