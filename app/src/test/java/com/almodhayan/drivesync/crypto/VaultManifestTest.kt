package com.almodhayan.drivesync.crypto

import com.almodhayan.drivesync.crypto.TestSupport.assertIntegrity
import com.almodhayan.drivesync.crypto.TestSupport.assertMessageContains
import com.almodhayan.drivesync.data.db.VaultEntity
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.Base64

class VaultManifestTest {
    private val salt = ByteArray(16) { it.toByte() }
    /** The fewest rounds parse accepts, so the tests stay quick. */
    private val rounds = VaultManifest.MIN_ITERATIONS
    private val keys = VaultKdf.derive("correct horse battery staple", salt, rounds)

    @Test
    fun serializeAndParseRoundTrip() {
        val m = VaultManifest.create("Photos vault", salt, rounds, VaultKdf.keyCheck(keys), createdAt = 1_700_000_000_000L)
        val json = m.toJson()
        assertTrue(json.contains("\"format\": \"drivesync-vault\""))
        assertTrue(json.contains("\"version\": 1"))
        assertTrue(json.contains("\"kdf\": \"PBKDF2-HMAC-SHA256\""))
        assertTrue(json.contains("\"iterations\": $rounds"))
        assertTrue(json.contains("\"created\": \"2023-11-14T22:13:20Z\""))
        val parsed = VaultManifest.parse(json)
        assertEquals(m, parsed)
        assertArrayEquals(salt, parsed.saltBytes())
        assertArrayEquals(VaultKdf.keyCheck(keys), parsed.keyCheckBytes())
        assertTrue(parsed.sameVaultAs(m))
    }

    @Test
    fun parseAcceptsCompactJsonWithUnknownKeys() {
        val text = """{"format":"drivesync-vault","version":1,"kdf":"PBKDF2-HMAC-SHA256","iterations":600000,"salt":"AAECAwQFBgcICQoLDA0ODw==","keyCheck":"F4rq5G5A9kz3h90RlZsFN5YBkI/8NO9O5L7TqXA7ioU=","created":"2026-09-30T12:00:00Z","name":"x","extra":true}"""
        val m = VaultManifest.parse(text)
        assertEquals(600000, m.iterations)
        assertEquals("x", m.name)
        assertArrayEquals(salt, m.saltBytes())
    }

    @Test
    fun entityRoundTrip() {
        val m = VaultManifest.create("Docs", salt, rounds, VaultKdf.keyCheck(keys), createdAt = 1_700_000_000_000L)
        val entity = m.toEntity("id-1")
        assertEquals(VaultEntity("id-1", "Docs", m.salt, rounds, m.keyCheck, 1_700_000_000_000L), entity)
        assertEquals(m, VaultManifest.fromEntity(entity))
    }

    @Test
    fun keysDerivedFromManifestMatchKeyCheck() {
        val m = VaultManifest.parse(VaultManifest.create("v", salt, rounds, VaultKdf.keyCheck(keys)).toJson())
        val again = VaultKdf.derive("correct horse battery staple", m.saltBytes(), m.iterations)
        assertTrue(VaultKdf.keyCheckMatches(again, m.keyCheckBytes()))
        assertArrayEquals(keys.keyEnc, again.keyEnc)
        val wrong = VaultKdf.derive("correct horse battery stapler", m.saltBytes(), m.iterations)
        assertFalse(VaultKdf.keyCheckMatches(wrong, m.keyCheckBytes()))
    }

    @Test
    fun sameVaultAsComparesSaltIterationsAndCheck() {
        val a = VaultManifest.create("a", salt, rounds, VaultKdf.keyCheck(keys))
        assertTrue(a.sameVaultAs(a.copy(name = "renamed", created = "2000-01-01T00:00:00Z")))
        assertFalse(a.sameVaultAs(a.copy(iterations = rounds + 1)))
        assertFalse(a.sameVaultAs(a.copy(salt = Base64.getEncoder().encodeToString(ByteArray(16) { 9 }))))
        assertFalse(a.sameVaultAs(a.copy(keyCheck = Base64.getEncoder().encodeToString(ByteArray(32)))))
    }

    @Test
    fun parseRejectsInvalidManifests() {
        val good = VaultManifest.create("v", salt, rounds, VaultKdf.keyCheck(keys))
        assertMessageContains(assertIntegrity { VaultManifest.parse("not json at all") }, "not a valid")
        assertMessageContains(assertIntegrity { VaultManifest.parse("{}") }, "not a valid")
        assertMessageContains(assertIntegrity { VaultManifest.parse(good.copy(format = "other").toJson()) }, "format")
        assertMessageContains(assertIntegrity { VaultManifest.parse(good.copy(version = 2).toJson()) }, "version 2")
        assertMessageContains(assertIntegrity { VaultManifest.parse(good.copy(kdf = "scrypt").toJson()) }, "KDF")
        assertMessageContains(assertIntegrity { VaultManifest.parse(good.copy(iterations = 0).toJson()) }, "iteration")
        assertMessageContains(assertIntegrity { VaultManifest.parse(good.copy(iterations = 1000).toJson()) }, "iteration")
        assertMessageContains(assertIntegrity { VaultManifest.parse(good.copy(salt = "***").toJson()) }, "salt")
        assertMessageContains(assertIntegrity { VaultManifest.parse(good.copy(salt = "AAAA").toJson()) }, "salt is too short")
        assertMessageContains(assertIntegrity { VaultManifest.parse(good.copy(keyCheck = "AAAA").toJson()) }, "keyCheck")
    }

    /** Same bounds as libwasl (manifest.rs): a vault.json edited on Drive cannot weaken or stall the passphrase check. */
    @Test
    fun parseEnforcesTheIterationBoundsOfTheEngine() {
        val good = VaultManifest.create("v", salt, rounds, VaultKdf.keyCheck(keys))
        assertEquals(VaultManifest.MIN_ITERATIONS, VaultManifest.parse(good.toJson()).iterations)
        assertEquals(VaultManifest.MAX_ITERATIONS, VaultManifest.parse(good.copy(iterations = VaultManifest.MAX_ITERATIONS).toJson()).iterations)
        assertMessageContains(assertIntegrity { VaultManifest.parse(good.copy(iterations = VaultManifest.MIN_ITERATIONS - 1).toJson()) }, "iteration")
        assertMessageContains(assertIntegrity { VaultManifest.parse(good.copy(iterations = VaultManifest.MAX_ITERATIONS + 1).toJson()) }, "iteration")
        assertMessageContains(assertIntegrity { VaultManifest.parse(good.copy(iterations = Int.MAX_VALUE).toJson()) }, "iteration")
        assertMessageContains(assertIntegrity { VaultManifest.parse(good.copy(iterations = -5).toJson()) }, "iteration")
    }
}
