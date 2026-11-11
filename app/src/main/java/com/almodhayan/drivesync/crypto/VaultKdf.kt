package com.almodhayan.drivesync.crypto

import java.security.MessageDigest
import java.security.SecureRandom
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/**
 * Passphrase → keys. PBKDF2-HMAC-SHA256 over the UTF-8 passphrase, 64 bytes of output, split by
 * [VaultKeys.fromMaster]. PBKDF2 is implemented directly on top of HmacSHA256 so the result is the
 * same on every platform (some Android providers historically mangled non-ASCII passphrases).
 *
 * keyCheck = HMAC-SHA256(keyName, "drivesync-vault-check-v1") lets the app validate a passphrase
 * without storing anything derived from keyEnc.
 */
object VaultKdf {
    const val DEFAULT_ITERATIONS = 600_000
    const val SALT_BYTES = 16
    const val KEY_CHECK_BYTES = 32
    const val KEY_CHECK_MESSAGE = "drivesync-vault-check-v1"

    private const val HMAC = "HmacSHA256"
    private val random = SecureRandom()

    fun newSalt(): ByteArray = ByteArray(SALT_BYTES).also { random.nextBytes(it) }

    /** Derives the vault keys. Costs [iterations] PBKDF2 rounds (about a second or two on a phone at the default); run it off the main thread. */
    fun derive(passphrase: String, salt: ByteArray, iterations: Int): VaultKeys {
        require(passphrase.isNotEmpty()) { "passphrase must not be empty" }
        require(salt.isNotEmpty()) { "salt must not be empty" }
        require(iterations >= 1) { "iterations must be >= 1" }
        val password = passphrase.toByteArray(Charsets.UTF_8)
        val master = try {
            pbkdf2HmacSha256(password, salt, iterations, VaultKeys.MASTER_BYTES)
        } finally {
            password.fill(0)
        }
        try {
            return VaultKeys.fromMaster(master)
        } finally {
            master.fill(0)
        }
    }

    fun keyCheck(keys: VaultKeys): ByteArray = hmacSha256(keys.keyName, KEY_CHECK_MESSAGE.toByteArray(Charsets.UTF_8))

    /** Constant-time comparison of the computed key check against a stored one. */
    fun keyCheckMatches(keys: VaultKeys, expected: ByteArray): Boolean = MessageDigest.isEqual(keyCheck(keys), expected)

    fun hmacSha256(key: ByteArray, data: ByteArray): ByteArray {
        val mac = Mac.getInstance(HMAC)
        mac.init(SecretKeySpec(key, HMAC))
        return mac.doFinal(data)
    }

    /** RFC 8018 section 5.2, PRF = HMAC-SHA256. */
    fun pbkdf2HmacSha256(password: ByteArray, salt: ByteArray, iterations: Int, dkLen: Int): ByteArray {
        require(password.isNotEmpty()) { "password must not be empty" }
        require(iterations >= 1) { "iterations must be >= 1" }
        require(dkLen >= 1) { "dkLen must be >= 1" }
        val mac = Mac.getInstance(HMAC)
        mac.init(SecretKeySpec(password, HMAC))
        val hLen = mac.macLength
        val blocks = (dkLen + hLen - 1) / hLen
        val out = ByteArray(blocks * hLen)
        val u = ByteArray(hLen)
        val t = ByteArray(hLen)
        val counter = ByteArray(4)
        for (block in 1..blocks) {
            counter[0] = (block ushr 24).toByte(); counter[1] = (block ushr 16).toByte(); counter[2] = (block ushr 8).toByte(); counter[3] = block.toByte()
            mac.update(salt)
            mac.update(counter)
            mac.doFinal(u, 0)
            System.arraycopy(u, 0, t, 0, hLen)
            for (i in 1 until iterations) {
                mac.update(u)
                mac.doFinal(u, 0)
                for (j in 0 until hLen) t[j] = (t[j].toInt() xor u[j].toInt()).toByte()
            }
            System.arraycopy(t, 0, out, (block - 1) * hLen, hLen)
        }
        u.fill(0); t.fill(0)
        if (out.size == dkLen) return out
        val result = out.copyOf(dkLen)
        out.fill(0)
        return result
    }
}
