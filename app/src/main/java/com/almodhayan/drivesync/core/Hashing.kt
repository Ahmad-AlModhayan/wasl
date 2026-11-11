package com.almodhayan.drivesync.core

import java.io.File
import java.io.InputStream
import java.security.MessageDigest

object Hashing {
    fun md5(file: File): String = file.inputStream().use { md5(it) }

    fun md5(input: InputStream): String {
        val md = MessageDigest.getInstance("MD5")
        val buf = ByteArray(256 * 1024)
        while (true) {
            val n = input.read(buf)
            if (n < 0) break
            md.update(buf, 0, n)
        }
        return md.digest().toHex()
    }

    fun sha256(bytes: ByteArray): String = MessageDigest.getInstance("SHA-256").digest(bytes).toHex()

    fun ByteArray.toHex(): String = joinToString("") { "%02x".format(it) }
}
