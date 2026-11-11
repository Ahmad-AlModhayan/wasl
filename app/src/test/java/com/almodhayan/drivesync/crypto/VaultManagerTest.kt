package com.almodhayan.drivesync.crypto

import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.db.VaultDao
import com.almodhayan.drivesync.data.db.VaultEntity
import com.almodhayan.drivesync.devices.SeedWrapKey
import com.kahflabs.wasl.ffi.vaultV2Unlock
import com.kahflabs.wasl.ffi.vaultVersion
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.util.Base64
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey

/**
 * The vault lifecycle on the phone: new vaults are format v2 (Argon2id in libwasl, through the
 * host build), v1 vaults made before keep unlocking as they did (FR-V1), and both kinds keep their
 * key material wrapped like before: 64 bytes for v1, 32 for v2.
 */
class VaultManagerTest {
    @get:Rule val tmp = TemporaryFolder()

    private class SoftKey : SeedWrapKey {
        val k: SecretKey = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
        override fun key(): SecretKey = k
    }

    private class Vaults : VaultDao {
        val rows = MutableStateFlow<List<VaultEntity>>(emptyList())
        override fun observeAll(): Flow<List<VaultEntity>> = rows
        override suspend fun getAll(): List<VaultEntity> = rows.value.sortedBy { it.createdAt }
        override suspend fun get(id: String): VaultEntity? = rows.value.firstOrNull { it.id == id }
        override suspend fun upsert(v: VaultEntity) { rows.value = rows.value.filter { it.id != v.id } + v }
        override suspend fun delete(id: String) { rows.value = rows.value.filter { it.id != id } }
    }

    private val dao = Vaults()
    private val pairs = mutableListOf<FolderPairEntity>()
    private val wrap = SoftKey()
    private val keyDir by lazy { File(tmp.root, "vaults") }
    private val store by lazy { VaultKeyStore(keyDir, wrap) }
    /** Vault ids the on-demand reader was told to close, in order. */
    private val closed = mutableListOf<String>()
    private val manager by lazy { VaultManager({ dao }, { pairs.toList() }, store, { closed += it }) }

    /** A v1 vault as an earlier version of the app made it (few rounds, so the test is quick). */
    private fun v1Vault(id: String, passphrase: String, rounds: Int = 1_000): VaultEntity {
        val salt = VaultKdf.newSalt()
        val keys = VaultKdf.derive(passphrase, salt, rounds)
        val b64 = Base64.getEncoder()
        return VaultEntity(id, "Old", b64.encodeToString(salt), rounds, b64.encodeToString(VaultKdf.keyCheck(keys)), 1_700_000_000_000L)
            .also { keys.zero() }
    }

    @Test fun `a new vault is format 2 and starts unlocked with 32 bytes of key material`() = runBlocking {
        val v = manager.createVault("  Photos ", "correct horse battery")
        assertEquals(2, v.version)
        assertEquals("Photos", v.name)
        assertEquals(0, v.iterations)
        val json = v.manifestJson!!
        assertEquals(2u, vaultVersion(json))
        val m = VaultManifestV2.parse(json)
        assertEquals("argon2id", m.kdf)
        assertEquals(m.salt, v.saltB64)
        assertEquals(m.keyCheck, v.keyCheckB64)
        assertEquals(listOf(v), dao.getAll())

        assertTrue(manager.isUnlocked(v.id))
        assertEquals(setOf(v.id), manager.unlockedIds.value)
        val material = manager.loadKeyMaterial(v.id)!!
        assertEquals(32, material.size)
        // What is stored is exactly what libwasl derives from the passphrase.
        assertArrayEquals(vaultV2Unlock(json, "correct horse battery"), material)
        // The v1 view of the keys does not apply to a v2 vault.
        assertNull(manager.loadKeys(v.id))
        // Wrapped, never in the clear: iv + 32 bytes + tag.
        assertEquals(12 + 32 + 16, File(keyDir, "${v.id}.key").readBytes().size)
        assertEquals(json, manager.exportManifest(v.id))
    }

    @Test fun `a v2 vault unlocks only with its passphrase`() = runBlocking {
        val v = manager.createVault("Docs", "the right one")
        val before = manager.loadKeyMaterial(v.id)!!
        manager.lock(v.id)
        assertFalse(manager.isUnlocked(v.id))
        assertNull(manager.loadKeyMaterial(v.id))

        assertFalse(manager.unlock(v.id, "the wrong one"))
        assertFalse(manager.unlock(v.id, ""))
        assertFalse(manager.isUnlocked(v.id))

        assertTrue(manager.unlock(v.id, "the right one"))
        assertArrayEquals(before, manager.loadKeyMaterial(v.id))
        assertEquals(setOf(v.id), manager.unlockedIds.value)
    }

    @Test fun `a v1 vault made before keeps unlocking unchanged`() = runBlocking {
        val v = v1Vault("old-1", "legacy pass", rounds = VaultManifest.MIN_ITERATIONS)   // its vault.json is parsed below
        dao.upsert(v)
        assertEquals(1, dao.get("old-1")!!.version)

        assertFalse(manager.unlock("old-1", "nope"))
        assertTrue(manager.unlock("old-1", "legacy pass"))
        val keys = manager.loadKeys("old-1")!!
        val expected = VaultKdf.derive("legacy pass", Base64.getDecoder().decode(v.saltB64), v.iterations)
        assertArrayEquals(expected.keyEnc, keys.keyEnc)
        assertArrayEquals(expected.keyName, keys.keyName)
        // The engine gets keyEnc || keyName, 64 bytes, as before.
        assertArrayEquals(expected.keyEnc + expected.keyName, manager.loadKeyMaterial("old-1"))
        assertEquals(12 + 64 + 16, File(keyDir, "old-1.key").readBytes().size)
        // Its vault.json is still the v1 one.
        val m = VaultManifest.parse(manager.exportManifest("old-1"))
        assertEquals(v.saltB64, m.salt)
        assertEquals(v.iterations, m.iterations)
    }

    @Test fun `joining a v2 vault from its vault json unlocks it and finds the same vault again`() = runBlocking {
        val elsewhere = VaultV2.create("Shared", "joined pass", 1_760_000_000_000L)
        elsewhere.material.fill(0)
        assertNull(manager.importFromManifest("", elsewhere.manifestJson, "wrong"))
        assertTrue(dao.getAll().isEmpty())

        val v = manager.importFromManifest("", elsewhere.manifestJson, "joined pass")!!
        assertEquals("Shared", v.name)
        assertEquals(2, v.version)
        assertEquals(1_760_000_000_000L, v.createdAt)
        assertEquals(32, manager.loadKeyMaterial(v.id)!!.size)

        manager.lock(v.id)
        val again = manager.importFromManifest("Other name", elsewhere.manifestJson, "joined pass")!!
        assertEquals(v.id, again.id)
        assertEquals(1, dao.getAll().size)
        assertTrue(manager.isUnlocked(v.id))
    }

    @Test fun `joining a v1 vault from its vault json still works`() = runBlocking {
        val salt = VaultKdf.newSalt()
        val keys = VaultKdf.derive("v1 pass", salt, VaultManifest.MIN_ITERATIONS)
        val json = VaultManifest.create("Legacy", salt, VaultManifest.MIN_ITERATIONS, VaultKdf.keyCheck(keys), 1_700_000_000_000L).toJson()
        keys.zero()
        assertEquals(1u, vaultVersion(json))
        assertNull(manager.importFromManifest("", json, "bad"))
        val v = manager.importFromManifest("", json, "v1 pass")!!
        assertEquals(1, v.version)
        assertNull(v.manifestJson)
        assertEquals(VaultManifest.MIN_ITERATIONS, v.iterations)
        assertNotNull(manager.loadKeys(v.id))
        assertEquals(64, manager.loadKeyMaterial(v.id)!!.size)
    }

    @Test fun `a new vault needs a passphrase of at least eight characters`() = runBlocking {
        val e = runCatching { manager.createVault("Short", "1234567") }.exceptionOrNull()
        assertTrue(e is AppError)
        assertTrue(dao.getAll().isEmpty())
        assertEquals(8, VaultManager.MIN_PASSPHRASE_LENGTH)
        assertEquals(2, manager.createVault("Long enough", "12345678").version)
    }

    @Test fun `a v1 vault json with too few rounds is refused before any key is derived`() = runBlocking {
        val salt = VaultKdf.newSalt()
        val keys = VaultKdf.derive("v1 pass", salt, 1_000)
        val json = VaultManifest.create("Weak", salt, 1_000, VaultKdf.keyCheck(keys), 1_700_000_000_000L).toJson()
        keys.zero()
        TestSupport.assertIntegrity { runBlocking { manager.importFromManifest("", json, "v1 pass") } }
        assertTrue(dao.getAll().isEmpty())
    }

    /** E2E 2026-10: the Vaults screen showed "vault format 2" for a 2.1 vault too. */
    @Test fun `a 2_1 vault json is told apart from 2_0`() {
        val v20 = """{"format":"wasl-vault","version":2,"kdf":"argon2id","m":65536,"t":3,"p":1,"salt":"c2FsdA==","keyCheck":"a2M=","created":"2026-10-26T18:32:11.946Z","name":"x"}"""
        assertTrue(VaultManifestV2.isFormat21(v20.replace("\"version\":2", "\"version\":3")))
        assertFalse(VaultManifestV2.isFormat21(v20))
        assertFalse(VaultManifestV2.isFormat21(null))
        assertFalse(VaultManifestV2.isFormat21("{}"))
    }

    @Test fun `text that is not a vault json is refused`() = runBlocking {
        TestSupport.assertIntegrity { runBlocking { manager.importFromManifest("", "{}", "x") } }
        val broken = """{"format":"wasl-vault","version":2,"kdf":"scrypt","m":1,"t":1,"p":1,"salt":"","keyCheck":"","created":"","name":""}"""
        TestSupport.assertIntegrity { runBlocking { manager.importFromManifest("", broken, "x") } }
        Unit
    }

    @Test fun `a vault in use by a pair is not deleted`() = runBlocking {
        val v = manager.createVault("Busy", "pass phrase")
        pairs += FolderPairEntity(id = "p1", accountId = "a@b.c", name = "Pair", localPath = "/x", remoteFolderId = "F", remotePath = "/F", encrypted = true, vaultId = v.id)
        val e = runCatching { manager.deleteVault(v.id) }.exceptionOrNull()
        assertTrue(e is AppError)
        assertNotNull(dao.get(v.id))
        pairs.clear()
        manager.deleteVault(v.id)
        assertNull(dao.get(v.id))
        assertFalse(manager.isUnlocked(v.id))
    }

    @Test fun `locking or removing a vault closes it in the on-demand reader`() = runBlocking {
        val v = manager.createVault("Private", "pass phrase")
        val old = v1Vault("old-2", "legacy")
        dao.upsert(old)
        assertTrue(manager.unlock(old.id, "legacy"))
        assertTrue(closed.isEmpty())

        manager.lock(v.id)
        assertEquals(listOf(v.id), closed)
        assertFalse(manager.isUnlocked(v.id))
        // Unlocking does not close anything; deleting does.
        assertTrue(manager.unlock(v.id, "pass phrase"))
        assertEquals(listOf(v.id), closed)
        manager.deleteVault(old.id)
        assertEquals(listOf(v.id, old.id), closed)
        // A vault still used by a pair is neither removed nor closed.
        pairs += FolderPairEntity(id = "p2", accountId = "a@b.c", name = "Pair", localPath = "/x", remoteFolderId = "F", remotePath = "/F", encrypted = true, vaultId = v.id)
        assertTrue(runCatching { manager.deleteVault(v.id) }.isFailure)
        assertEquals(listOf(v.id, old.id), closed)
        assertTrue(manager.isUnlocked(v.id))
    }

    @Test fun `the key store keeps both sizes and refuses others`() {
        val v1 = ByteArray(64) { it.toByte() }
        val v2 = ByteArray(32) { (it + 7).toByte() }
        store.saveMaterial("a", v1)
        store.saveMaterial("b", v2)
        assertArrayEquals(v1, store.loadMaterial("a"))
        assertArrayEquals(v2, store.loadMaterial("b"))
        assertNotNull(store.load("a"))
        assertNull(store.load("b"))
        assertTrue(runCatching { store.saveMaterial("c", ByteArray(48)) }.isFailure)
        // Another device's (or a lost) wrap key cannot open them.
        assertNull(VaultKeyStore(keyDir, SoftKey()).loadMaterial("b"))
    }
}
