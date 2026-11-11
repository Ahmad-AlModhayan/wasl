package com.almodhayan.drivesync.crypto

import com.almodhayan.drivesync.core.AppError
import com.kahflabs.wasl.ffi.WaslException
import com.kahflabs.wasl.ffi.vaultV2Create
import com.kahflabs.wasl.ffi.vaultV2Unlock
import com.kahflabs.wasl.ffi.vaultVersion

/**
 * Vault format v2 (docs/ON_DEMAND.md §1.1) through libwasl: Argon2id, the key check and the
 * vault.json are Rust's, so the app has no implementation of its own. Every call runs Argon2id
 * (64 MiB, seconds on a phone): never call it on the main thread. The passphrase is handed to
 * Rust, which wipes its copy; nothing here logs it.
 */
internal object VaultV2 {
    /** A new vault: its vault.json and its 32-byte key material (the caller wipes [material]). */
    class Created(val manifestJson: String, val material: ByteArray)

    fun create(name: String, passphrase: String, createdAt: Long): Created {
        val v = try {
            vaultV2Create(name, passphrase, createdAt)
        } catch (e: WaslException) {
            throw AppError.Unknown(e.detailText())
        }
        if (v.keys.size != VaultKeys.V2_MATERIAL_BYTES) {
            v.keys.fill(0)
            throw AppError.Integrity("libwasl returned ${v.keys.size} bytes of v2 key material")
        }
        return Created(v.manifestJson, v.keys)
    }

    /** The key material (32 bytes, the caller wipes it), or null when the passphrase does not match. */
    fun unlock(manifestJson: String, passphrase: String): ByteArray? {
        val material = try {
            vaultV2Unlock(manifestJson, passphrase)
        } catch (e: WaslException) {
            throw AppError.Integrity(e.detailText())
        } ?: return null
        if (material.size != VaultKeys.V2_MATERIAL_BYTES) {
            material.fill(0)
            throw AppError.Integrity("libwasl returned ${material.size} bytes of v2 key material")
        }
        return material
    }

    /** 1 or 2 for a Wasl (or Drive Sync) vault.json, null for anything else. */
    fun version(manifestJson: String): Int? = vaultVersion(manifestJson)?.toInt()
}

/** The sentence a libwasl error carries (its generated message is "detail=<sentence>"). */
internal fun WaslException.detailText(): String = message?.removePrefix("detail=")?.takeIf { it.isNotBlank() } ?: javaClass.simpleName
