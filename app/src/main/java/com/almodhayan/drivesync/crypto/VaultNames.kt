package com.almodhayan.drivesync.crypto

import java.nio.ByteBuffer
import java.nio.charset.CharacterCodingException
import java.nio.charset.CodingErrorAction
import java.security.GeneralSecurityException
import java.util.Base64
import javax.crypto.Cipher
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec

/**
 * Deterministic file/folder name encryption.
 *
 * nonce = HMAC-SHA256(keyName, "name-nonce" || 0x00 || parentToken || 0x00 || nameUtf8)[0..12)
 * encrypted = base64url-no-padding(nonce || AES-256-GCM(keyEnc, nonce, nameUtf8, aad = "drivesync-name-v1")) + ".dsv"
 *
 * The same plaintext name in the same parent always maps to the same ciphertext, so re-created
 * folders keep their Drive name. Decryption only needs the keys because the nonce is embedded.
 */
object VaultNames {
    const val SUFFIX = ".dsv"
    const val NONCE_BYTES = 12
    const val TAG_BITS = 128
    const val AAD = "drivesync-name-v1"
    const val NONCE_LABEL = "name-nonce"

    private const val TRANSFORM = "AES/GCM/NoPadding"
    private val aadBytes = AAD.toByteArray(Charsets.UTF_8)
    private val nonceLabelBytes = NONCE_LABEL.toByteArray(Charsets.UTF_8)

    /** True when [name] has the shape of a vault name (suffix only; the content is not checked). */
    fun isVaultName(name: String): Boolean = name.length > SUFFIX.length && name.endsWith(SUFFIX)

    /** [parentToken] is the Drive id of the parent folder ("" when unknown). */
    fun encryptName(keys: VaultKeys, parentToken: String, name: String): String {
        require(name.isNotEmpty()) { "name must not be empty" }
        require(!name.contains('/')) { "name must not contain '/'" }
        val plain = name.toByteArray(Charsets.UTF_8)
        val nonce = nameNonce(keys, parentToken, plain)
        val cipher = Cipher.getInstance(TRANSFORM)
        cipher.init(Cipher.ENCRYPT_MODE, SecretKeySpec(keys.keyEnc, "AES"), GCMParameterSpec(TAG_BITS, nonce))
        cipher.updateAAD(aadBytes)
        val ct = cipher.doFinal(plain)
        val out = ByteArray(NONCE_BYTES + ct.size)
        System.arraycopy(nonce, 0, out, 0, NONCE_BYTES)
        System.arraycopy(ct, 0, out, NONCE_BYTES, ct.size)
        return Base64.getUrlEncoder().withoutPadding().encodeToString(out) + SUFFIX
    }

    /** Returns the plaintext name, or null when [encryptedName] is not a valid name for these keys. */
    fun decryptName(keys: VaultKeys, encryptedName: String): String? {
        if (!isVaultName(encryptedName)) return null
        val body = encryptedName.substring(0, encryptedName.length - SUFFIX.length)
        if (body.indexOf('=') >= 0) return null
        val raw = try { Base64.getUrlDecoder().decode(body) } catch (e: IllegalArgumentException) { return null }
        if (raw.size < NONCE_BYTES + TAG_BITS / 8) return null
        val plain = try {
            val cipher = Cipher.getInstance(TRANSFORM)
            cipher.init(Cipher.DECRYPT_MODE, SecretKeySpec(keys.keyEnc, "AES"), GCMParameterSpec(TAG_BITS, raw, 0, NONCE_BYTES))
            cipher.updateAAD(aadBytes)
            cipher.doFinal(raw, NONCE_BYTES, raw.size - NONCE_BYTES)
        } catch (e: GeneralSecurityException) {
            return null
        }
        val name = try { strictUtf8(plain) } catch (e: CharacterCodingException) { return null }
        if (name.isEmpty() || name.indexOf('/') >= 0 || name == "." || name == "..") return null
        return name
    }

    private fun nameNonce(keys: VaultKeys, parentToken: String, nameUtf8: ByteArray): ByteArray {
        val parent = parentToken.toByteArray(Charsets.UTF_8)
        val msg = ByteArray(nonceLabelBytes.size + 1 + parent.size + 1 + nameUtf8.size)
        var o = 0
        System.arraycopy(nonceLabelBytes, 0, msg, o, nonceLabelBytes.size); o += nonceLabelBytes.size
        msg[o++] = 0
        System.arraycopy(parent, 0, msg, o, parent.size); o += parent.size
        msg[o++] = 0
        System.arraycopy(nameUtf8, 0, msg, o, nameUtf8.size)
        return VaultKdf.hmacSha256(keys.keyName, msg).copyOf(NONCE_BYTES)
    }

    private fun strictUtf8(bytes: ByteArray): String =
        Charsets.UTF_8.newDecoder()
            .onMalformedInput(CodingErrorAction.REPORT)
            .onUnmappableCharacter(CodingErrorAction.REPORT)
            .decode(ByteBuffer.wrap(bytes))
            .toString()
}
