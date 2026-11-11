package com.almodhayan.drivesync.devices

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.security.keystore.StrongBoxUnavailableException
import com.kahflabs.wasl.ffi.DeviceKeyStore
import com.kahflabs.wasl.ffi.WaslException
import java.io.File
import java.io.IOException
import java.security.KeyStore
import java.security.ProviderException
import java.util.concurrent.ConcurrentHashMap
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Encrypts and decrypts with a key that never leaves where it is kept. */
interface SeedWrapKey {
    /** The AES key for AES/GCM/NoPadding; created on first use. */
    fun key(): SecretKey
}

/**
 * The phone's wrap key: AES-256-GCM held by the Android Keystore, not exportable. Used for the
 * device key seed and (alias [com.almodhayan.drivesync.crypto.VaultKeyStore.WRAP_ALIAS]) for
 * unlocked vaults' keys.
 *
 * New keys go into StrongBox (a separate secure chip) when the phone has one, else the TEE
 * (security review S4-5). Existing keys are used as they are, wherever they live: moving one would
 * mean deleting it, and with it every seed and vault key it wraps.
 *
 * Deliberately not set: user authentication and `setUnlockedDeviceRequired`. Both make the key
 * unusable while the phone is locked, and that is exactly when background sync (devices charging
 * on Wi-Fi, vault pairs in WorkManager runs) opens it. With them a locked phone would stop syncing
 * vaults and stop answering paired devices. The cost: someone running code as this app on an
 * unlocked-once phone can use the key, which the app's own sandbox already allows.
 */
class AndroidKeystoreWrapKey(private val alias: String = ALIAS) : SeedWrapKey {
    override fun key(): SecretKey {
        val ks = KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) }
        return getOrGenerate(
            alias = alias,
            existing = { ks.getKey(alias, null) as? SecretKey },
            generate = { strongBox -> generate(strongBox) },
            discard = { runCatching { ks.deleteEntry(alias) } },
        )
    }

    private fun generate(strongBox: Boolean): SecretKey {
        val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, ANDROID_KEYSTORE)
        gen.init(
            KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                // Background device sync (charging on Wi-Fi) must open it without the person.
                .setUserAuthenticationRequired(false)
                .setIsStrongBoxBacked(strongBox)
                .build(),
        )
        return gen.generateKey()
    }

    companion object {
        const val ALIAS = "wasl-device-key-wrap"
        private const val ANDROID_KEYSTORE = "AndroidKeyStore"

        /** One lock per alias for the whole process: see [getOrGenerate]. */
        private val locks = ConcurrentHashMap<String, Any>()

        /**
         * The key under [alias], made when there is none. Serialized per alias across the process:
         * two first uses at once (the device key and a sync worker, say) would otherwise both find
         * nothing and both generate, and one's fallback [discard] could delete the StrongBox key
         * the other has just wrapped a file with, leaving that file unreadable for good.
         */
        internal fun getOrGenerate(
            alias: String,
            existing: () -> SecretKey?,
            generate: (strongBox: Boolean) -> SecretKey,
            discard: () -> Unit,
        ): SecretKey = synchronized(locks.computeIfAbsent(alias) { Any() }) {
            existing() ?: generateWithFallback(generate, discard, existing)
        }

        /**
         * StrongBox first, then the TEE. Phones without StrongBox throw
         * [StrongBoxUnavailableException]; some with a StrongBox that can't hold this key (no
         * AES-256, chip busy or broken) throw a [ProviderException] instead, so both fall back.
         * [discard] drops anything a failed attempt left under the alias, but never a usable key
         * found there by then ([existing]; another process of the app may have made it): that
         * one is used, since something may already be wrapped with it.
         */
        internal fun generateWithFallback(
            generate: (strongBox: Boolean) -> SecretKey,
            discard: () -> Unit,
            existing: () -> SecretKey? = { null },
        ): SecretKey =
            try {
                generate(true)
            } catch (e: StrongBoxUnavailableException) {
                existing() ?: run { discard(); generate(false) }
            } catch (e: ProviderException) {
                existing() ?: run { discard(); generate(false) }
            }
    }
}

/**
 * The phone's device key seed (SRS FR-N1, docs/DEVICE_SYNC.md §2) for libwasl: 32 bytes kept in
 * [file] as `iv (12) || AES-GCM(seed) (32 + 16-byte tag)` under [wrap]. A copy of the app's data
 * is useless without this phone's Keystore key. The seed never appears in logs or messages.
 */
class DeviceSeedFile(private val file: File, private val wrap: SeedWrapKey) : DeviceKeyStore {
    private val lock = Any()

    override fun load(): ByteArray? {
        val blob = synchronized(lock) {
            if (!file.isFile) return null
            try {
                file.readBytes()
            } catch (e: IOException) {
                throw WaslException.Other("Can't read this phone's device key.")
            }
        }
        if (blob.size != BLOB_BYTES) throw WaslException.Other("This phone's device key is damaged.")
        return try {
            val cipher = Cipher.getInstance(TRANSFORM)
            cipher.init(Cipher.DECRYPT_MODE, wrap.key(), GCMParameterSpec(TAG_BITS, blob, 0, IV_BYTES))
            cipher.doFinal(blob, IV_BYTES, blob.size - IV_BYTES).also {
                if (it.size != SEED_BYTES) { it.fill(0); throw WaslException.Other("This phone's device key is damaged.") }
            }
        } catch (e: WaslException) {
            throw e
        } catch (e: Exception) {
            // The Keystore key changed (restored backup, cleared credentials): the seed is gone.
            // Never pass the cause on, it could carry key material in some providers' messages.
            throw WaslException.Other("This phone's device key can't be opened on this phone.")
        }
    }

    override fun save(seed: ByteArray) {
        if (seed.size != SEED_BYTES) throw WaslException.Other("A device key has 32 bytes.")
        try {
            synchronized(lock) {
                file.parentFile?.let { if (!it.isDirectory && !it.mkdirs()) throw IOException("mkdirs") }
                val cipher = Cipher.getInstance(TRANSFORM)
                cipher.init(Cipher.ENCRYPT_MODE, wrap.key())
                val iv = cipher.iv
                val ct = cipher.doFinal(seed)
                val tmp = File(file.parentFile, file.name + ".tmp")
                tmp.outputStream().use { it.write(iv); it.write(ct) }
                if (!tmp.renameTo(file)) {
                    file.delete()
                    if (!tmp.renameTo(file)) { tmp.delete(); throw IOException("rename") }
                }
            }
        } catch (e: WaslException) {
            throw e
        } catch (e: Exception) {
            throw WaslException.Other("Can't save this phone's device key.")
        } finally {
            seed.fill(0)
        }
    }

    companion object {
        const val SEED_BYTES = 32
        private const val IV_BYTES = 12
        private const val TAG_BITS = 128
        private const val BLOB_BYTES = IV_BYTES + SEED_BYTES + TAG_BITS / 8
        private const val TRANSFORM = "AES/GCM/NoPadding"
    }
}
