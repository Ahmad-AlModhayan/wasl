package com.almodhayan.drivesync.crypto

import com.almodhayan.drivesync.core.Hashing
import com.almodhayan.drivesync.crypto.TestSupport.assertIntegrity
import com.almodhayan.drivesync.crypto.TestSupport.assertMessageContains
import com.almodhayan.drivesync.crypto.TestSupport.hex
import com.almodhayan.drivesync.crypto.TestSupport.randomBytes
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.File

class VaultCipherTest {
    private val keys = TestSupport.fixedKeys()
    private val cipher = VaultCipher(keys)
    private val mib = VaultCipher.CHUNK_BYTES

    private fun encrypt(plain: ByteArray, c: VaultCipher = cipher): ByteArray =
        ByteArrayOutputStream().also { c.encrypt(ByteArrayInputStream(plain), it) }.toByteArray()

    private fun decrypt(ct: ByteArray, c: VaultCipher = cipher): ByteArray =
        ByteArrayOutputStream().also { c.decrypt(ByteArrayInputStream(ct), it) }.toByteArray()

    private fun roundTrip(n: Int) {
        val plain = randomBytes(n, seed = n.toLong())
        val ct = encrypt(plain)
        assertEquals("ciphertext size for $n bytes", VaultCipher.encryptedSize(n.toLong()), ct.size.toLong())
        assertArrayEquals(plain, decrypt(ct))
    }

    @Test fun roundTripEmpty() = roundTrip(0)
    @Test fun roundTripOneByte() = roundTrip(1)
    @Test fun roundTripExactlyOneMiB() = roundTrip(mib)
    @Test fun roundTripOneMiBPlusOne() = roundTrip(mib + 1)
    @Test fun roundTripThreeAndAHalfMiB() = roundTrip(3 * mib + mib / 2)
    @Test fun roundTripExactlyTwoMiB() = roundTrip(2 * mib)

    @Test
    fun chunkCountAndSize() {
        assertEquals(1, VaultCipher.chunkCount(0))
        assertEquals(1, VaultCipher.chunkCount(1))
        assertEquals(1, VaultCipher.chunkCount(mib.toLong()))
        assertEquals(2, VaultCipher.chunkCount(mib + 1L))
        assertEquals(4, VaultCipher.chunkCount(3L * mib + mib / 2))
        assertEquals(12L + 16, VaultCipher.encryptedSize(0))
        assertEquals(12L + mib + 16, VaultCipher.encryptedSize(mib.toLong()))
        assertEquals(12L + mib + 1 + 32, VaultCipher.encryptedSize(mib + 1L))
    }

    @Test
    fun fileRoundTripAndNonce() {
        val plain = randomBytes(2 * mib + 12345, seed = 7)
        val src = File.createTempFile("vault-src", ".bin"); val enc = File.createTempFile("vault-enc", ".dsv"); val out = File.createTempFile("vault-out", ".bin")
        try {
            src.writeBytes(plain)
            val nonce = cipher.encryptFile(src, enc)
            assertEquals(VaultCipher.FILE_NONCE_BYTES, nonce.size)
            assertArrayEquals(nonce, VaultCipher.readFileNonce(enc))
            assertEquals(VaultCipher.encryptedSize(plain.size.toLong()), enc.length())
            cipher.decryptFile(enc, out)
            assertArrayEquals(plain, out.readBytes())
            assertNull(VaultCipher.readFileNonce(src))
        } finally {
            src.delete(); enc.delete(); out.delete()
        }
    }

    // Known answers generated with Python `cryptography` (AESGCM) for fileNonce 01..08.
    private fun fixedNonceCipher() = VaultCipher(keys) { byteArrayOf(1, 2, 3, 4, 5, 6, 7, 8) }

    @Test
    fun knownAnswerHelloWorld() {
        val ct = encrypt("hello world".toByteArray(), fixedNonceCipher())
        assertEquals("4453563101020304050607089a8aaa4e57b08e47d70191df91e92ea93ac8b2541b937ff1e1988d", hex(ct))
        assertEquals("hello world", String(decrypt(ct)))
    }

    @Test
    fun knownAnswerEmpty() {
        val ct = encrypt(ByteArray(0), fixedNonceCipher())
        assertEquals("445356310102030405060708c30f0aec6d312875fabd66c8a89a7797", hex(ct))
        assertEquals(0, decrypt(ct).size)
    }

    @Test
    fun knownAnswerOneMiBPlusOneOfZeros() {
        val ct = encrypt(ByteArray(mib + 1), fixedNonceCipher())
        assertEquals(1048621, ct.size)
        assertEquals("7d793d13c57d20f67559b0b98a5dd0e70ca2d49d94d023c18aa4e8621682693a", Hashing.sha256(ct))
        assertEquals("3b46260f6698a0366d97aeea64f486ebda", hex(ct.copyOfRange(ct.size - 17, ct.size)))
    }

    @Test
    fun sameContentEncryptsDifferentlyEachTime() {
        val plain = "same bytes".toByteArray()
        assertFalse(encrypt(plain).contentEquals(encrypt(plain)))
    }

    @Test
    fun flippedByteInEveryRegionIsDetected() {
        val plain = randomBytes(mib + 100, seed = 3)
        val ct = encrypt(plain)
        val positions = listOf(5, 11, 12, 12 + 1000, 12 + mib, 12 + mib + 15, 12 + mib + 16, ct.size - 1)
        for (pos in positions) {
            val bad = ct.copyOf(); bad[pos] = (bad[pos].toInt() xor 0x01).toByte()
            val e = assertIntegrity { decrypt(bad) }
            assertMessageContains(e, "authentication failed")
        }
    }

    @Test
    fun badMagicIsRejected() {
        val ct = encrypt("x".toByteArray())
        val bad = ct.copyOf(); bad[3] = '2'.code.toByte()   // "DSV2"
        val e = assertIntegrity { decrypt(bad) }
        assertMessageContains(e, "unknown format")
    }

    @Test
    fun droppingTheLastChunkIsDetected() {
        val plain = randomBytes(2 * mib + mib / 2, seed = 5)   // 3 chunks
        val ct = encrypt(plain)
        val cut = ct.copyOf(12 + 2 * (mib + 16))                // header + two full chunks
        val e = assertIntegrity { decrypt(cut) }
        assertMessageContains(e, "truncated")
    }

    @Test
    fun droppingTheLastChunkOfAlignedFileIsDetected() {
        val plain = randomBytes(2 * mib, seed = 6)             // 2 full chunks, last has lastFlag = 1
        val ct = encrypt(plain)
        val cut = ct.copyOf(12 + (mib + 16))
        assertIntegrity { decrypt(cut) }
    }

    @Test
    fun truncationInsideAChunkIsDetected() {
        val ct = encrypt(randomBytes(mib + 5000, seed = 8))
        assertIntegrity { decrypt(ct.copyOf(ct.size - 100)) }
        assertIntegrity { decrypt(ct.copyOf(ct.size - 1)) }
    }

    @Test
    fun headerOnlyAndTooShortAreRejected() {
        val ct = encrypt("abc".toByteArray())
        assertMessageContains(assertIntegrity { decrypt(ct.copyOf(12)) }, "no data chunk")
        assertMessageContains(assertIntegrity { decrypt(ct.copyOf(20)) }, "no data chunk")
        assertMessageContains(assertIntegrity { decrypt(ct.copyOf(5)) }, "shorter than the header")
        assertIntegrity { decrypt(ByteArray(0)) }
    }

    @Test
    fun appendedBytesAreRejected() {
        val ct = encrypt(randomBytes(1000, seed = 9))
        assertIntegrity { decrypt(ct + byteArrayOf(1, 2, 3)) }
        assertIntegrity { decrypt(ct + ByteArray(40)) }
        val full = encrypt(randomBytes(mib, seed = 10))
        assertIntegrity { decrypt(full + byteArrayOf(0)) }
    }

    @Test
    fun wrongKeyIsRejected() {
        val ct = encrypt("secret".toByteArray())
        assertIntegrity { decrypt(ct, VaultCipher(TestSupport.otherKeys())) }
    }

    @Test
    fun chunkReorderingIsRejected() {
        val ct = encrypt(randomBytes(3 * mib, seed = 11))      // 3 full chunks
        val c = mib + 16
        val swapped = ct.copyOf()
        System.arraycopy(ct, 12, swapped, 12 + c, c)
        System.arraycopy(ct, 12 + c, swapped, 12, c)
        assertIntegrity { decrypt(swapped) }
    }

    @Test
    fun decryptFileDeletesPartialOutputOnFailure() {
        val enc = File.createTempFile("vault-bad", ".dsv"); val out = File.createTempFile("vault-out", ".bin")
        try {
            val ct = encrypt(randomBytes(mib + 10, seed = 12))
            enc.writeBytes(ct.copyOf(ct.size - 3))
            assertIntegrity { cipher.decryptFile(enc, out) }
            assertFalse("partial plaintext must not be left behind", out.exists())
        } finally {
            enc.delete(); out.delete()
        }
    }

    @Test
    fun ivAndAadLayout() {
        val nonce = byteArrayOf(1, 2, 3, 4, 5, 6, 7, 8)
        assertEquals("01020304050607080000002a", hex(VaultCipher.chunkIv(nonce, 42)))
        assertEquals("010203040506070800000000", hex(VaultCipher.chunkIv(nonce, 0)))
        assertEquals("64726976657379"+"6e632d6368756e6b2d7631" + "00000001" + "01", hex(VaultCipher.chunkAad(1, true)))
        assertEquals("647269766573796e632d6368756e6b2d7631" + "00000000" + "00", hex(VaultCipher.chunkAad(0, false)))
        assertNotNull(VaultCipher.MAGIC)
        assertEquals("44535631", hex(VaultCipher.MAGIC))
        assertTrue(VaultCipher.randomFileNonce().size == 8)
    }
}
