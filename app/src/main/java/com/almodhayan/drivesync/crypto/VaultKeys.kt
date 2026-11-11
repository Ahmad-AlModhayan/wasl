package com.almodhayan.drivesync.crypto

/**
 * The two 256-bit keys derived from a vault passphrase.
 *
 * - [keyEnc] encrypts file contents and names (AES-256-GCM).
 * - [keyName] derives deterministic name nonces and the manifest key check (HMAC-SHA256).
 *
 * Call [zero] (or use as [AutoCloseable]) when the keys are no longer needed; the arrays are
 * overwritten in place so the instance must not be used afterwards.
 */
class VaultKeys(val keyEnc: ByteArray, val keyName: ByteArray) : AutoCloseable {
    init {
        require(keyEnc.size == KEY_BYTES) { "keyEnc must be $KEY_BYTES bytes" }
        require(keyName.size == KEY_BYTES) { "keyName must be $KEY_BYTES bytes" }
    }

    /** Independent copy (the original can be zeroed without affecting the copy). */
    fun copy(): VaultKeys = VaultKeys(keyEnc.copyOf(), keyName.copyOf())

    /** Overwrites both keys with zeros. */
    fun zero() {
        keyEnc.fill(0)
        keyName.fill(0)
    }

    override fun close() = zero()

    companion object {
        const val KEY_BYTES = 32
        const val MASTER_BYTES = 64
        /** A v2 vault's key material: the 32-byte Argon2id output its keys are derived from in libwasl. */
        const val V2_MATERIAL_BYTES = 32

        /** Splits the 64-byte PBKDF2 output: keyEnc = master[0..32), keyName = master[32..64). */
        fun fromMaster(master: ByteArray): VaultKeys {
            require(master.size == MASTER_BYTES) { "master key must be $MASTER_BYTES bytes" }
            return VaultKeys(master.copyOfRange(0, KEY_BYTES), master.copyOfRange(KEY_BYTES, MASTER_BYTES))
        }

        fun zero(vararg arrays: ByteArray) {
            for (a in arrays) a.fill(0)
        }
    }
}
