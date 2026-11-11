package com.almodhayan.drivesync.crypto

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class VaultNamesTest {
    private val keys = TestSupport.fixedKeys()
    private val parent = "1AbCdEf-parent"

    private fun roundTrip(name: String) {
        val enc = VaultNames.encryptName(keys, parent, name)
        assertTrue("encrypted name must end with .dsv: $enc", enc.endsWith(VaultNames.SUFFIX))
        assertTrue("encrypted name must be base64url: $enc", enc.removeSuffix(VaultNames.SUFFIX).all { it.isLetterOrDigit() || it == '-' || it == '_' })
        assertEquals(name, VaultNames.decryptName(keys, enc))
    }

    @Test fun roundTripAscii() = roundTrip("hello.txt")
    @Test fun roundTripSpacesAndDots() = roundTrip("My Report (final).v2.docx")
    @Test fun roundTripUnicode() = roundTrip("Report (final) — résumé.pdf")
    @Test fun roundTripArabic() = roundTrip("ملف مهم.txt")
    @Test fun roundTripCjkAndEmoji() = roundTrip("写真 📷 2026.jpg")
    @Test fun roundTripSingleChar() = roundTrip("a")
    @Test fun roundTripLongName() = roundTrip("x".repeat(200) + ".bin")
    @Test fun roundTripFolderLikeName() = roundTrip("Camera Roll")

    @Test
    fun deterministicForSameParentAndName() {
        val a = VaultNames.encryptName(keys, parent, "hello.txt")
        val b = VaultNames.encryptName(keys, parent, "hello.txt")
        assertEquals(a, b)
    }

    @Test
    fun differentParentGivesDifferentCiphertext() {
        val a = VaultNames.encryptName(keys, parent, "hello.txt")
        val b = VaultNames.encryptName(keys, "otherParent", "hello.txt")
        val c = VaultNames.encryptName(keys, "", "hello.txt")
        assertNotEquals(a, b); assertNotEquals(a, c); assertNotEquals(b, c)
        assertEquals("hello.txt", VaultNames.decryptName(keys, b))
        assertEquals("hello.txt", VaultNames.decryptName(keys, c))
    }

    @Test
    fun differentNamesGiveDifferentCiphertext() {
        assertNotEquals(VaultNames.encryptName(keys, parent, "hello.txt"), VaultNames.encryptName(keys, parent, "Hello.txt"))
        assertNotEquals(VaultNames.encryptName(keys, parent, "hello.txt"), VaultNames.encryptName(keys, parent, "hello.tx"))
    }

    // Known answers generated with the Python reference implementation (tools/decrypt_vault.py primitives).
    @Test
    fun knownAnswers() {
        assertEquals("NTuQV7P2RFdk1_IqEnfNkO_CbgbqOzMJV2F5P0hg0NzwmWYJSw.dsv", VaultNames.encryptName(keys, parent, "hello.txt"))
        assertEquals("BjB1rRvvamd0oJytoL1H5otLMB4RRWX0qQmXhfXcVlvWmQTr8Vg9hz7Y00H8UgJL0GfGvQKdl3M6fro.dsv", VaultNames.encryptName(keys, parent, "Report (final) — résumé.pdf"))
        assertEquals("Omq2FLBxK0GM-h4b-agoelYTaqsWccb36bGO7CT7gcVcaKrJ8Q.dsv", VaultNames.encryptName(keys, "", "hello.txt"))
        assertEquals("hello.txt", VaultNames.decryptName(keys, "NTuQV7P2RFdk1_IqEnfNkO_CbgbqOzMJV2F5P0hg0NzwmWYJSw.dsv"))
    }

    @Test
    fun encryptedLengthIsPredictable() {
        // base64url of (12 nonce + 9 plaintext + 16 tag = 37 bytes) = 50 chars, plus ".dsv".
        assertEquals(54, VaultNames.encryptName(keys, parent, "hello.txt").length)
    }

    @Test
    fun decryptReturnsNullForNonVaultNames() {
        assertNull(VaultNames.decryptName(keys, "vault.json"))
        assertNull(VaultNames.decryptName(keys, "hello.txt"))
        assertNull(VaultNames.decryptName(keys, ".dsv"))
        assertNull(VaultNames.decryptName(keys, "notbase64!!.dsv"))
        assertNull(VaultNames.decryptName(keys, "AAAA.dsv"))                       // too short
        assertNull(VaultNames.decryptName(keys, "NTuQV7P2RFdk1_IqEnfNkO_CbgbqOzMJV2F5P0hg0NzwmWYJSw==.dsv")) // padding not allowed
        assertNull(VaultNames.decryptName(keys, "NTuQV7P2RFdk1_IqEnfNkO_CbgbqOzMJV2F5P0hg0NzwmWYJSw.DSV"))  // suffix is case sensitive
    }

    @Test
    fun decryptReturnsNullForTamperedName() {
        val enc = VaultNames.encryptName(keys, parent, "hello.txt")
        val body = enc.removeSuffix(VaultNames.SUFFIX)
        val flipped = body.substring(0, 20) + (if (body[20] == 'A') 'B' else 'A') + body.substring(21)
        assertNull(VaultNames.decryptName(keys, flipped + VaultNames.SUFFIX))
    }

    @Test
    fun decryptReturnsNullWithWrongKeys() {
        val enc = VaultNames.encryptName(keys, parent, "hello.txt")
        assertNull(VaultNames.decryptName(TestSupport.otherKeys(), enc))
    }

    @Test(expected = IllegalArgumentException::class)
    fun encryptRejectsSlash() {
        VaultNames.encryptName(keys, parent, "a/b")
    }

    @Test(expected = IllegalArgumentException::class)
    fun encryptRejectsEmpty() {
        VaultNames.encryptName(keys, parent, "")
    }

    @Test
    fun isVaultName() {
        assertTrue(VaultNames.isVaultName("abc.dsv"))
        assertFalse(VaultNames.isVaultName(".dsv"))
        assertFalse(VaultNames.isVaultName("abc.txt"))
    }
}
