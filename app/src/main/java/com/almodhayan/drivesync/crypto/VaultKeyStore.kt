package com.almodhayan.drivesync.crypto

import android.content.Context
import android.util.Log
import com.almodhayan.drivesync.devices.AndroidKeystoreWrapKey
import com.almodhayan.drivesync.devices.SeedWrapKey
import dagger.hilt.android.qualifiers.ApplicationContext
import java.io.File
import java.io.IOException
import java.security.GeneralSecurityException
import javax.crypto.Cipher
import javax.crypto.spec.GCMParameterSpec
import javax.inject.Inject
import javax.inject.Singleton

/**
 * Persists derived vault keys ("unlocked" state) in app-private storage, wrapped with an
 * Android Keystore AES-256-GCM key (alias [WRAP_ALIAS], no user authentication, so background
 * syncs can use it). File layout: `filesDir/vaults/<vaultId>.key` = iv (12 bytes) || AES-GCM
 * ciphertext of the key material (+ 16-byte tag). The material is what libwasl takes for the
 * vault: keyEnc || keyName (64 bytes) for a v1 vault, the Argon2id output (32 bytes) for a v2 one.
 *
 * The files are useless without this device's Keystore key, so a backup/restore or a copy of the
 * app data cannot unlock the vault; the user just enters the passphrase again. A new wrap key
 * lives in StrongBox where the phone has one; it is usable while the phone is locked, because
 * background sync of vault pairs needs it then (see [AndroidKeystoreWrapKey]).
 *
 * No "lock after N minutes without use" (security review S4-5): "unlocked" here is this file on
 * disk, not keys held in memory, and the users of it are background sync runs and the documents
 * provider, which open it on their own. An idle timer would either lock vaults under a running
 * sync (it reads the keys from here on every run) or count every background run as use and never
 * fire. Locking by hand stays the way to require the passphrase again.
 */
@Singleton
class VaultKeyStore internal constructor(private val dir: File, private val wrap: SeedWrapKey) {
    @Inject constructor(@ApplicationContext context: Context) : this(File(context.filesDir, DIR_NAME), AndroidKeystoreWrapKey(WRAP_ALIAS))

    private val lock = Any()

    /** Stores a v1 vault's keys (keyEnc || keyName). */
    fun save(vaultId: String, keys: VaultKeys) {
        val plain = ByteArray(VaultKeys.MASTER_BYTES)
        System.arraycopy(keys.keyEnc, 0, plain, 0, VaultKeys.KEY_BYTES)
        System.arraycopy(keys.keyName, 0, plain, VaultKeys.KEY_BYTES, VaultKeys.KEY_BYTES)
        try {
            saveMaterial(vaultId, plain)
        } finally {
            plain.fill(0)
        }
    }

    /**
     * Stores a vault's key material: 64 bytes for v1, 32 for v2. [material] stays the caller's,
     * who wipes it.
     */
    fun saveMaterial(vaultId: String, material: ByteArray) {
        require(material.size == VaultKeys.MASTER_BYTES || material.size == VaultKeys.V2_MATERIAL_BYTES) {
            "vault key material is ${VaultKeys.MASTER_BYTES} (v1) or ${VaultKeys.V2_MATERIAL_BYTES} (v2) bytes"
        }
        synchronized(lock) {
            if (!dir.isDirectory && !dir.mkdirs()) throw IOException("Cannot create ${dir.path}")
            val cipher = Cipher.getInstance(TRANSFORM)
            cipher.init(Cipher.ENCRYPT_MODE, wrap.key())
            val iv = cipher.iv
            val ct = cipher.doFinal(material)
            val target = fileFor(vaultId)
            val tmp = File(dir, target.name + ".tmp")
            tmp.outputStream().use { it.write(iv); it.write(ct) }
            if (!tmp.renameTo(target)) {
                target.delete()
                if (!tmp.renameTo(target)) { tmp.delete(); throw IOException("Cannot write ${target.path}") }
            }
        }
    }

    /** A v1 vault's stored keys, or null when it is locked, is a v2 vault, or the file cannot be unwrapped on this device. */
    fun load(vaultId: String): VaultKeys? {
        val material = loadMaterial(vaultId) ?: return null
        return try {
            if (material.size == VaultKeys.MASTER_BYTES) VaultKeys.fromMaster(material) else null
        } finally {
            material.fill(0)
        }
    }

    /**
     * The stored key material (64 bytes for v1, 32 for v2), or null when the vault is locked (no
     * file) or the file cannot be unwrapped on this device. The caller wipes the array.
     */
    fun loadMaterial(vaultId: String): ByteArray? {
        val file = fileFor(vaultId)
        val blob = synchronized(lock) {
            if (!file.isFile) return null
            try { file.readBytes() } catch (e: IOException) { Log.w(TAG, "Cannot read ${file.name}: ${e.message}"); return null }
        }
        if (blob.size < IV_BYTES + VaultKeys.V2_MATERIAL_BYTES + TAG_BYTES) { Log.w(TAG, "Stored key for $vaultId is malformed"); return null }
        val plain = try {
            val cipher = Cipher.getInstance(TRANSFORM)
            cipher.init(Cipher.DECRYPT_MODE, wrap.key(), GCMParameterSpec(TAG_BITS, blob, 0, IV_BYTES))
            cipher.doFinal(blob, IV_BYTES, blob.size - IV_BYTES)
        } catch (e: GeneralSecurityException) {
            Log.w(TAG, "Cannot unwrap stored key for $vaultId (Keystore key changed?): $e"); return null
        } catch (e: Exception) {
            Log.w(TAG, "Keystore failure while unwrapping $vaultId: $e"); return null
        }
        if (plain.size != VaultKeys.MASTER_BYTES && plain.size != VaultKeys.V2_MATERIAL_BYTES) { plain.fill(0); return null }
        return plain
    }

    fun delete(vaultId: String) {
        synchronized(lock) { fileFor(vaultId).delete() }
    }

    fun has(vaultId: String): Boolean = fileFor(vaultId).isFile

    /** Ids of every vault with stored keys (unlocked). */
    fun storedIds(): Set<String> = synchronized(lock) {
        dir.list()?.filter { it.endsWith(FILE_SUFFIX) }?.map { it.removeSuffix(FILE_SUFFIX) }?.toSet() ?: emptySet()
    }

    private fun fileFor(vaultId: String): File = File(dir, safeName(vaultId) + FILE_SUFFIX)

    private fun safeName(id: String): String = id.map { if (it.isLetterOrDigit() || it == '-' || it == '_' || it == '.') it else '_' }.joinToString("")

    companion object {
        const val WRAP_ALIAS = "drivesync-vault-wrap"
        const val DIR_NAME = "vaults"
        const val FILE_SUFFIX = ".key"
        private const val TAG = "VaultKeyStore"
        private const val TRANSFORM = "AES/GCM/NoPadding"
        private const val IV_BYTES = 12
        private const val TAG_BITS = 128
        private const val TAG_BYTES = 16
    }
}
