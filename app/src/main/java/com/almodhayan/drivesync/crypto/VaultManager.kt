package com.almodhayan.drivesync.crypto

import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.Ids
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.core.TimeFmt
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.db.VaultDao
import com.almodhayan.drivesync.data.db.VaultEntity
import com.almodhayan.drivesync.ondemand.EngineReader
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.withContext
import java.util.Base64
import javax.inject.Inject
import javax.inject.Provider
import javax.inject.Singleton

/**
 * Vault lifecycle: create / unlock / lock / delete / import / export. Metadata lives in Room as
 * [VaultEntity]; the key material lives in [VaultKeyStore] while unlocked.
 *
 * New vaults are format v2 (docs/ON_DEMAND.md §1): libwasl runs Argon2id and writes the vault.json
 * ([VaultV2]), and the entity keeps that text. Vaults of format v1 (PBKDF2, docs/ENCRYPTION.md)
 * made before stay as they are and unlock as they always did (FR-V1). Key derivation runs on
 * [Dispatchers.Default], storage on [Dispatchers.IO]; all functions are safe to call from the main
 * thread. Passphrases are never logged, and derived bytes are wiped once stored.
 */
@Singleton
class VaultManager internal constructor(
    private val vaults: () -> VaultDao,
    private val pairs: suspend () -> List<FolderPairEntity>,
    private val keyStore: VaultKeyStore,
    /** Runs after a vault is locked or removed: the on-demand reader drops its keys and decrypted blocks. */
    private val onLocked: suspend (vaultId: String) -> Unit = {},
) {
    @Inject constructor(db: Db, keyStore: VaultKeyStore, reader: Provider<EngineReader>) : this(
        { db.vaults() }, { db.pairs().getAll() }, keyStore,
        { vaultId -> withContext(Dispatchers.IO) { reader.get().closeVault(vaultId) } },
    )

    private val unlocked = MutableStateFlow<Set<String>>(emptySet())

    /** Ids of unlocked vaults (keys stored). Call [refreshUnlocked] once to populate it. */
    val unlockedIds: StateFlow<Set<String>> get() = unlocked

    fun observeVaults(): Flow<List<VaultEntity>> = vaults().observeAll()

    suspend fun getVault(vaultId: String): VaultEntity? = vaults().get(vaultId)

    suspend fun refreshUnlocked() {
        unlocked.value = withContext(Dispatchers.IO) { keyStore.storedIds() }
    }

    /**
     * Creates a new vault (format v2) and stores its key material: the vault starts unlocked.
     * Argon2id makes this take a few seconds on a phone.
     */
    suspend fun createVault(name: String, passphrase: String): VaultEntity {
        val cleanName = name.trim()
        if (cleanName.isEmpty()) throw AppError.Unknown(Strings.get(R.string.crypto_vault_name_required))
        if (passphrase.isEmpty()) throw AppError.Unknown(Strings.get(R.string.crypto_passphrase_required))
        // Enforced here, not only in the create dialog, so no caller can make a vault with a short passphrase (security review 2026-10).
        if (passphrase.length < MIN_PASSPHRASE_LENGTH) throw AppError.Unknown(Strings.get(R.string.vaults_hint_too_short, MIN_PASSPHRASE_LENGTH))
        val now = System.currentTimeMillis()
        val created = withContext(Dispatchers.Default) { VaultV2.create(cleanName, passphrase, now) }
        try {
            val entity = VaultManifestV2.parse(created.manifestJson).toEntity(Ids.newId(), cleanName, created.manifestJson, now)
            vaults().upsert(entity)
            withContext(Dispatchers.IO) { keyStore.saveMaterial(entity.id, created.material) }
            refreshUnlocked()
            return entity
        } finally {
            created.material.fill(0)
        }
    }

    /** Derives the keys and stores them when the passphrase matches the vault's key check. */
    suspend fun unlock(vaultId: String, passphrase: String): Boolean {
        val vault = vaults().get(vaultId) ?: throw AppError.Unknown(Strings.get(R.string.crypto_vault_gone))
        if (passphrase.isEmpty()) return false
        if (vault.version >= 2) {
            val manifestJson = vault.manifestJson ?: throw AppError.Integrity("The vault ${vault.name} has no vault.json on this phone")
            val material = withContext(Dispatchers.Default) { VaultV2.unlock(manifestJson, passphrase) } ?: return false
            try {
                withContext(Dispatchers.IO) { keyStore.saveMaterial(vaultId, material) }
            } finally {
                material.fill(0)
            }
            refreshUnlocked()
            return true
        }
        val keys = withContext(Dispatchers.Default) { VaultKdf.derive(passphrase, b64d(vault.saltB64), vault.iterations) }
        try {
            if (!VaultKdf.keyCheckMatches(keys, b64d(vault.keyCheckB64))) return false
            withContext(Dispatchers.IO) { keyStore.save(vaultId, keys) }
            refreshUnlocked()
            return true
        } finally {
            keys.zero()
        }
    }

    suspend fun isUnlocked(vaultId: String): Boolean = withContext(Dispatchers.IO) { keyStore.has(vaultId) }

    /**
     * Forgets the stored keys; metadata stays so the vault can be unlocked again with the
     * passphrase. Files on demand drop the vault's keys and its decrypted cache too.
     */
    suspend fun lock(vaultId: String) {
        withContext(Dispatchers.IO) { keyStore.delete(vaultId) }
        refreshUnlocked()
        onLocked(vaultId)
    }

    /** Pairs configured to use this vault. */
    suspend fun pairsUsing(vaultId: String): List<FolderPairEntity> = pairs().filter { it.vaultId == vaultId }

    /**
     * Removes the vault metadata and keys from this device. Refuses while a folder pair uses the
     * vault. Nothing on Drive is touched: the encrypted files stay, and the vault can be joined again
     * from its vault.json as long as the passphrase is known.
     */
    suspend fun deleteVault(vaultId: String) {
        val users = pairsUsing(vaultId)
        if (users.isNotEmpty()) {
            throw AppError.Unknown(Strings.plural(R.plurals.crypto_vault_in_use, users.size, users.size, users.joinToString(Strings.get(R.string.crypto_list_separator)) { Strings.get(R.string.crypto_quoted_name, it.name) }))
        }
        vaults().delete(vaultId)
        withContext(Dispatchers.IO) { keyStore.delete(vaultId) }
        refreshUnlocked()
        onLocked(vaultId)
    }

    /**
     * Joins a vault created elsewhere from the text of its vault.json (format v1 or v2). Returns
     * null when the passphrase does not match the manifest's key check; throws
     * [AppError.Integrity] when the text is not a usable manifest. When this device already knows
     * the same vault, that entry is unlocked and returned instead of creating a duplicate.
     */
    suspend fun importFromManifest(name: String, manifestJson: String, passphrase: String): VaultEntity? {
        if (VaultV2.version(manifestJson) == VaultManifestV2.VERSION) return importV2(name, manifestJson.trim(), passphrase)
        val manifest = VaultManifest.parse(manifestJson)
        if (passphrase.isEmpty()) return null
        val keys = withContext(Dispatchers.Default) { VaultKdf.derive(passphrase, manifest.saltBytes(), manifest.iterations) }
        try {
            if (!VaultKdf.keyCheckMatches(keys, manifest.keyCheckBytes())) return null
            val existing = vaults().getAll().firstOrNull { it.version < 2 && VaultManifest.fromEntity(it).sameVaultAs(manifest) }
            val entity = existing ?: VaultEntity(
                id = Ids.newId(), name = name.trim().ifEmpty { manifest.name.trim().ifEmpty { Strings.get(R.string.backup_default_vault_name) } },
                saltB64 = manifest.salt, iterations = manifest.iterations, keyCheckB64 = manifest.keyCheck,
                createdAt = TimeFmt.parseRfc3339(manifest.created) ?: System.currentTimeMillis(),
            )
            if (existing == null) vaults().upsert(entity)
            withContext(Dispatchers.IO) { keyStore.save(entity.id, keys) }
            refreshUnlocked()
            return entity
        } finally {
            keys.zero()
        }
    }

    private suspend fun importV2(name: String, manifestJson: String, passphrase: String): VaultEntity? {
        val manifest = VaultManifestV2.parse(manifestJson)
        if (passphrase.isEmpty()) return null
        val material = withContext(Dispatchers.Default) { VaultV2.unlock(manifestJson, passphrase) } ?: return null
        try {
            val existing = vaults().getAll().firstOrNull { it.version >= 2 && VaultManifestV2.parseOrNull(it.manifestJson)?.sameVaultAs(manifest) == true }
            val entity = existing ?: manifest.toEntity(
                Ids.newId(), name.trim().ifEmpty { manifest.name.trim().ifEmpty { Strings.get(R.string.backup_default_vault_name) } }, manifestJson,
            )
            if (existing == null) vaults().upsert(entity)
            withContext(Dispatchers.IO) { keyStore.saveMaterial(entity.id, material) }
            refreshUnlocked()
            return entity
        } finally {
            material.fill(0)
        }
    }

    /** The vault.json text for this vault (no secrets; safe to paste on another device together with the passphrase). */
    suspend fun exportManifest(vaultId: String): String {
        val vault = vaults().get(vaultId) ?: throw AppError.Unknown(Strings.get(R.string.crypto_vault_gone))
        if (vault.version >= 2) return vault.manifestJson ?: throw AppError.Integrity("The vault ${vault.name} has no vault.json on this phone")
        return VaultManifest.fromEntity(vault).toJson()
    }

    /** Stored keys of a v1 vault, or null when locked (or a v2 vault). Blocking: call on [Dispatchers.IO]. */
    fun loadKeys(vaultId: String): VaultKeys? = keyStore.load(vaultId)

    /**
     * The stored key material as libwasl takes it (64 bytes for v1, 32 for v2), or null when the
     * vault is locked. Handed to the engine for encrypted pairs (WaslRuntime). The caller owns the
     * array. Blocking: call on [Dispatchers.IO].
     */
    fun loadKeyMaterial(vaultId: String): ByteArray? = keyStore.loadMaterial(vaultId)

    private fun b64d(text: String): ByteArray = Base64.getDecoder().decode(text)

    companion object {
        /** Shortest passphrase a new vault takes; the create dialog's strength meter uses the same value. */
        const val MIN_PASSPHRASE_LENGTH = 8
    }
}
