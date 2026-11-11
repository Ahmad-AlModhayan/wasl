package com.almodhayan.drivesync.crypto

import com.almodhayan.drivesync.crypto.TestSupport.hex
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.Base64

class VaultKdfTest {

    // RFC 7914 section 11: PBKDF2-HMAC-SHA-256 test vectors.
    @Test
    fun pbkdf2MatchesRfc7914Vector1() {
        val dk = VaultKdf.pbkdf2HmacSha256("passwd".toByteArray(), "salt".toByteArray(), 1, 64)
        assertEquals(
            "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc49ca9cccf179b645991664b39d77ef317c71b845b1e30bd509112041d3a19783",
            hex(dk),
        )
    }

    @Test
    fun pbkdf2MatchesRfc7914Vector2() {
        val dk = VaultKdf.pbkdf2HmacSha256("Password".toByteArray(), "NaCl".toByteArray(), 80000, 64)
        assertEquals(
            "4ddcd8f60b98be21830cee5ef22701f9641a4418d04c0414aeff08876b34ab56a1d425a1225833549adb841b51c9b3176a272bdebba1d078478f62b397f33c8d",
            hex(dk),
        )
    }

    @Test
    fun pbkdf2ShortOutputIsPrefixOfLongOutput() {
        val long = VaultKdf.pbkdf2HmacSha256("passwd".toByteArray(), "salt".toByteArray(), 3, 64)
        val short = VaultKdf.pbkdf2HmacSha256("passwd".toByteArray(), "salt".toByteArray(), 3, 20)
        assertArrayEquals(long.copyOf(20), short)
    }

    // Known answers generated with Python: hashlib.pbkdf2_hmac + hmac (see tools/decrypt_vault.py).
    @Test
    fun deriveKnownAnswer() {
        val salt = ByteArray(16) { it.toByte() }
        val keys = VaultKdf.derive("correct horse battery staple", salt, 1000)
        assertEquals("a69b179e3add3c1e0aaf227a0eb3aa2aa8645ab86fecf6ca00c17512697c719e", hex(keys.keyEnc))
        assertEquals("d35f6dc6fba420111c1d0abb5dabbfc8539681e885523a94f3ea280704b6791b", hex(keys.keyName))
        assertEquals("F4rq5G5A9kz3h90RlZsFN5YBkI/8NO9O5L7TqXA7ioU=", Base64.getEncoder().encodeToString(VaultKdf.keyCheck(keys)))
    }

    @Test
    fun deriveUsesUtf8ForNonAsciiPassphrases() {
        val salt = ByteArray(16) { it.toByte() }
        val keys = VaultKdf.derive("pässwörd ✓", salt, 1000)
        assertEquals("f82c708c2298e13beeff065189bf03ae7cf0beaa66d302e3cd6a7ff049e8bdce", hex(keys.keyEnc))
    }

    @Test
    fun wrongPassphraseFailsKeyCheck() {
        val salt = VaultKdf.newSalt()
        val right = VaultKdf.derive("open sesame please", salt, 500)
        val check = VaultKdf.keyCheck(right)
        assertTrue(VaultKdf.keyCheckMatches(VaultKdf.derive("open sesame please", salt, 500), check))
        assertFalse(VaultKdf.keyCheckMatches(VaultKdf.derive("open sesame pleas", salt, 500), check))
        assertFalse(VaultKdf.keyCheckMatches(VaultKdf.derive("Open sesame please", salt, 500), check))
        assertFalse("different salt must not match", VaultKdf.keyCheckMatches(VaultKdf.derive("open sesame please", VaultKdf.newSalt(), 500), check))
        assertFalse("different iterations must not match", VaultKdf.keyCheckMatches(VaultKdf.derive("open sesame please", salt, 501), check))
    }

    @Test
    fun keyCheckDependsOnlyOnKeyName() {
        val a = VaultKeys(ByteArray(32) { 1 }, ByteArray(32) { 2 })
        val b = VaultKeys(ByteArray(32) { 9 }, ByteArray(32) { 2 })
        assertArrayEquals(VaultKdf.keyCheck(a), VaultKdf.keyCheck(b))
        assertEquals(VaultKdf.KEY_CHECK_BYTES, VaultKdf.keyCheck(a).size)
    }

    @Test
    fun newSaltIsSixteenRandomBytes() {
        val a = VaultKdf.newSalt(); val b = VaultKdf.newSalt()
        assertEquals(VaultKdf.SALT_BYTES, a.size)
        assertFalse(a.contentEquals(b))
    }

    @Test(expected = IllegalArgumentException::class)
    fun deriveRejectsEmptyPassphrase() {
        VaultKdf.derive("", VaultKdf.newSalt(), 10)
    }

    @Test
    fun zeroWipesKeys() {
        val keys = VaultKdf.derive("wipe me", VaultKdf.newSalt(), 10)
        keys.zero()
        assertTrue(keys.keyEnc.all { it == 0.toByte() } && keys.keyName.all { it == 0.toByte() })
    }
}
