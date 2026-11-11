package com.almodhayan.drivesync.crypto

import com.almodhayan.drivesync.core.AppError
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.security.GeneralSecurityException
import java.security.SecureRandom
import javax.crypto.AEADBadTagException
import javax.crypto.Cipher
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec

/**
 * Streaming file content encryption (vault format v1).
 *
 * output = "DSV1" || fileNonce (8 random bytes) || chunk_0 || chunk_1 || ...
 * chunk_i = AES-256-GCM(keyEnc, iv = fileNonce || i, plain_i, aad = "drivesync-chunk-v1" || i || lastFlag)
 *   - i is a uint32 big-endian chunk index
 *   - plain_i is 1 MiB for every chunk but the last; the last may be shorter or empty
 *   - lastFlag is one byte: 1 for the final chunk, 0 otherwise
 *   - every chunk is written as raw ciphertext followed by the 16-byte tag
 *
 * There is always at least one chunk (an empty file is header + one empty chunk). Decryption
 * verifies every tag, so modified, truncated or extended streams fail with [AppError.Integrity].
 * Nothing is loaded whole into memory: the working set is two chunk buffers.
 */
class VaultCipher(private val keys: VaultKeys, private val nonceSource: () -> ByteArray = { randomFileNonce() }) {

    /** Encrypts [src] into [dst] (overwritten). On failure [dst] is deleted. Returns the file nonce. */
    fun encryptFile(src: File, dst: File): ByteArray {
        try {
            return src.inputStream().buffered(BUFFER_BYTES).use { input ->
                dst.outputStream().buffered(BUFFER_BYTES).use { output -> encrypt(input, output) }
            }
        } catch (t: Throwable) {
            dst.delete()
            throw t
        }
    }

    /** Decrypts [src] into [dst] (overwritten). On failure [dst] is deleted. */
    fun decryptFile(src: File, dst: File) {
        try {
            src.inputStream().buffered(BUFFER_BYTES).use { input ->
                dst.outputStream().buffered(BUFFER_BYTES).use { output -> decrypt(input, output) }
            }
        } catch (t: Throwable) {
            dst.delete()
            throw t
        }
    }

    /** Encrypts [input] to [output]; neither stream is closed. Returns the file nonce. */
    fun encrypt(input: InputStream, output: OutputStream): ByteArray {
        val fileNonce = nonceSource()
        require(fileNonce.size == FILE_NONCE_BYTES) { "file nonce must be $FILE_NONCE_BYTES bytes" }
        output.write(MAGIC)
        output.write(fileNonce)
        val cipher = Cipher.getInstance(TRANSFORM)
        val key = SecretKeySpec(keys.keyEnc, "AES")
        val plain = ByteArray(CHUNK_BYTES)
        val out = ByteArray(CHUNK_BYTES + TAG_BYTES)
        var index = 0L
        var n = readFully(input, plain, 0, CHUNK_BYTES)
        while (true) {
            checkInterrupted()
            val next = if (n == CHUNK_BYTES) input.read() else -1
            val last = next < 0
            if (index > MAX_CHUNK_INDEX) throw AppError.Io("file too large for the vault format")
            cipher.init(Cipher.ENCRYPT_MODE, key, GCMParameterSpec(TAG_BITS, chunkIv(fileNonce, index)))
            cipher.updateAAD(chunkAad(index, last))
            val len = cipher.doFinal(plain, 0, n, out, 0)
            output.write(out, 0, len)
            if (last) break
            plain[0] = next.toByte()
            n = 1 + readFully(input, plain, 1, CHUNK_BYTES - 1)
            index++
        }
        output.flush()
        return fileNonce
    }

    /** Decrypts [input] to [output]; neither stream is closed. Throws [AppError.Integrity] for anything that does not verify. */
    fun decrypt(input: InputStream, output: OutputStream) {
        val header = ByteArray(HEADER_BYTES)
        if (readFully(input, header, 0, HEADER_BYTES) != HEADER_BYTES) throw AppError.Integrity("not an encrypted vault file (shorter than the header)")
        for (i in MAGIC.indices) if (header[i] != MAGIC[i]) throw AppError.Integrity("not an encrypted vault file (unknown format or version)")
        val fileNonce = header.copyOfRange(MAGIC.size, HEADER_BYTES)
        val cipher = Cipher.getInstance(TRANSFORM)
        val key = SecretKeySpec(keys.keyEnc, "AES")
        val buf = ByteArray(CHUNK_BYTES + TAG_BYTES)
        val plain = ByteArray(CHUNK_BYTES)
        var index = 0L
        var n = readFully(input, buf, 0, buf.size)
        while (true) {
            checkInterrupted()
            val next = if (n == buf.size) input.read() else -1
            val last = next < 0
            if (n < TAG_BYTES) throw AppError.Integrity(if (index == 0L) "encrypted file has no data chunk (truncated)" else "encrypted file is truncated after chunk ${index - 1}")
            cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(TAG_BITS, chunkIv(fileNonce, index)))
            cipher.updateAAD(chunkAad(index, last))
            val len = try {
                cipher.doFinal(buf, 0, n, plain, 0)
            } catch (e: AEADBadTagException) {
                throw AppError.Integrity(
                    if (last) "authentication failed on the final chunk ($index): the file was modified or truncated"
                    else "authentication failed on chunk $index: the file was modified",
                )
            } catch (e: GeneralSecurityException) {
                throw AppError.Integrity("decryption failed on chunk $index: ${e.message}")
            }
            output.write(plain, 0, len)
            if (last) break
            buf[0] = next.toByte()
            n = 1 + readFully(input, buf, 1, buf.size - 1)
            index++
        }
        output.flush()
    }

    private fun checkInterrupted() {
        if (Thread.currentThread().isInterrupted) throw InterruptedException("vault cipher interrupted")
    }

    companion object {
        const val TRANSFORM = "AES/GCM/NoPadding"
        const val CHUNK_BYTES = 1 shl 20
        const val TAG_BITS = 128
        const val TAG_BYTES = TAG_BITS / 8
        const val FILE_NONCE_BYTES = 8
        const val HEADER_BYTES = 4 + FILE_NONCE_BYTES
        const val CHUNK_AAD = "drivesync-chunk-v1"
        const val MAX_CHUNK_INDEX = 0xFFFF_FFFFL
        private const val BUFFER_BYTES = 256 * 1024
        val MAGIC: ByteArray = "DSV1".toByteArray(Charsets.US_ASCII)
        private val chunkAadBytes = CHUNK_AAD.toByteArray(Charsets.UTF_8)
        private val random = SecureRandom()

        fun randomFileNonce(): ByteArray = ByteArray(FILE_NONCE_BYTES).also { random.nextBytes(it) }

        fun chunkCount(plainSize: Long): Long = if (plainSize <= 0) 1 else (plainSize + CHUNK_BYTES - 1) / CHUNK_BYTES

        /** Exact size of the ciphertext for a plaintext of [plainSize] bytes. */
        fun encryptedSize(plainSize: Long): Long = HEADER_BYTES + plainSize + TAG_BYTES * chunkCount(plainSize)

        /** Reads the file nonce from an encrypted file's header, or null when the file does not start with a valid header. */
        fun readFileNonce(file: File): ByteArray? {
            if (!file.isFile || file.length() < HEADER_BYTES) return null
            val header = ByteArray(HEADER_BYTES)
            try {
                file.inputStream().use { if (readFully(it, header, 0, HEADER_BYTES) != HEADER_BYTES) return null }
            } catch (e: IOException) {
                return null
            }
            for (i in MAGIC.indices) if (header[i] != MAGIC[i]) return null
            return header.copyOfRange(MAGIC.size, HEADER_BYTES)
        }

        fun chunkIv(fileNonce: ByteArray, index: Long): ByteArray {
            val iv = ByteArray(FILE_NONCE_BYTES + 4)
            System.arraycopy(fileNonce, 0, iv, 0, FILE_NONCE_BYTES)
            putUInt32(iv, FILE_NONCE_BYTES, index)
            return iv
        }

        fun chunkAad(index: Long, last: Boolean): ByteArray {
            val aad = ByteArray(chunkAadBytes.size + 4 + 1)
            System.arraycopy(chunkAadBytes, 0, aad, 0, chunkAadBytes.size)
            putUInt32(aad, chunkAadBytes.size, index)
            aad[aad.size - 1] = if (last) 1 else 0
            return aad
        }

        private fun putUInt32(dst: ByteArray, offset: Int, value: Long) {
            dst[offset] = (value ushr 24).toByte()
            dst[offset + 1] = (value ushr 16).toByte()
            dst[offset + 2] = (value ushr 8).toByte()
            dst[offset + 3] = value.toByte()
        }

        /** Reads up to [len] bytes; returns fewer only at end of stream. */
        private fun readFully(input: InputStream, buf: ByteArray, off: Int, len: Int): Int {
            var total = 0
            while (total < len) {
                val r = input.read(buf, off + total, len - total)
                if (r < 0) break
                total += r
            }
            return total
        }
    }
}
