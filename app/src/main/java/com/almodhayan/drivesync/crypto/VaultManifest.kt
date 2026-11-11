package com.almodhayan.drivesync.crypto

import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.TimeFmt
import com.almodhayan.drivesync.data.db.VaultEntity
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import java.util.Base64

/**
 * `vault.json`, the only plaintext file in an encrypted Drive folder. It carries everything a
 * second device (or the desktop tool) needs to derive the keys from the passphrase, plus a key
 * check so a wrong passphrase is detected before anything is decrypted. No key material.
 */
@Serializable
data class VaultManifest(
    val format: String,
    val version: Int,
    val kdf: String,
    val iterations: Int,
    val salt: String,       // standard base64, 16 bytes
    val keyCheck: String,   // standard base64, HMAC-SHA256(keyName, "drivesync-vault-check-v1")
    val created: String,    // ISO-8601 instant
    val name: String,
) {
    fun saltBytes(): ByteArray = Base64.getDecoder().decode(salt)
    fun keyCheckBytes(): ByteArray = Base64.getDecoder().decode(keyCheck)

    fun toJson(json: Json = DefaultJson): String = json.encodeToString(serializer(), this)

    fun toEntity(id: String, name: String = this.name): VaultEntity = VaultEntity(
        id = id, name = name, saltB64 = salt, iterations = iterations, keyCheckB64 = keyCheck,
        createdAt = TimeFmt.parseRfc3339(created) ?: System.currentTimeMillis(),
    )

    /** True when both manifests describe the same vault (same salt, iterations and key check). */
    fun sameVaultAs(other: VaultManifest): Boolean =
        salt == other.salt && iterations == other.iterations && java.security.MessageDigest.isEqual(keyCheckBytes(), other.keyCheckBytes())

    companion object {
        const val FILE_NAME = "vault.json"
        const val FORMAT = "drivesync-vault"
        const val VERSION = 1
        const val KDF = "PBKDF2-HMAC-SHA256"

        /**
         * PBKDF2 iteration bounds, the same as libwasl's (wasl-crypto manifest.rs MIN_ITERATIONS /
         * MAX_ITERATIONS). vault.json sits on Drive where anyone with write access can edit it: a tiny
         * count weakens the passphrase check, a huge one stalls every device that checks a
         * passphrase (security review 2026-10).
         */
        const val MIN_ITERATIONS = 100_000
        const val MAX_ITERATIONS = 10_000_000

        fun iterationsInBounds(iterations: Int): Boolean = iterations in MIN_ITERATIONS..MAX_ITERATIONS

        /** Pretty-printed so the text is pleasant to paste on another device; parsing is lenient. */
        val DefaultJson: Json = Json { ignoreUnknownKeys = true; isLenient = true; encodeDefaults = true; prettyPrint = true }

        fun create(name: String, salt: ByteArray, iterations: Int, keyCheck: ByteArray, createdAt: Long = System.currentTimeMillis()): VaultManifest = VaultManifest(
            format = FORMAT, version = VERSION, kdf = KDF, iterations = iterations,
            salt = Base64.getEncoder().encodeToString(salt), keyCheck = Base64.getEncoder().encodeToString(keyCheck),
            created = TimeFmt.toRfc3339(createdAt), name = name,
        )

        fun fromEntity(v: VaultEntity): VaultManifest = VaultManifest(
            format = FORMAT, version = VERSION, kdf = KDF, iterations = v.iterations,
            salt = v.saltB64, keyCheck = v.keyCheckB64, created = TimeFmt.toRfc3339(v.createdAt), name = v.name,
        )

        /** Parses and validates. Throws [AppError.Integrity] with a specific reason for anything this app cannot use. */
        fun parse(text: String, json: Json = DefaultJson): VaultManifest {
            val m = try {
                json.decodeFromString(serializer(), text.trim())
            } catch (e: Exception) {
                throw AppError.Integrity("vault.json is not a valid Drive Sync vault manifest (${e.message?.lineSequence()?.firstOrNull() ?: "parse error"})")
            }
            if (m.format != FORMAT) throw AppError.Integrity("vault.json has format \"${m.format}\", expected \"$FORMAT\"")
            if (m.version != VERSION) throw AppError.Integrity("vault format version ${m.version} is not supported by this app (supports $VERSION)")
            if (m.kdf != KDF) throw AppError.Integrity("vault KDF \"${m.kdf}\" is not supported by this app (supports $KDF)")
            if (!iterationsInBounds(m.iterations)) {
                throw AppError.Integrity("vault.json has an invalid iteration count (${m.iterations}; allowed $MIN_ITERATIONS to $MAX_ITERATIONS)")
            }
            val salt = try { m.saltBytes() } catch (e: IllegalArgumentException) { throw AppError.Integrity("vault.json salt is not valid base64") }
            if (salt.size < 8) throw AppError.Integrity("vault.json salt is too short (${salt.size} bytes)")
            val check = try { m.keyCheckBytes() } catch (e: IllegalArgumentException) { throw AppError.Integrity("vault.json keyCheck is not valid base64") }
            if (check.size != VaultKdf.KEY_CHECK_BYTES) throw AppError.Integrity("vault.json keyCheck has the wrong length (${check.size} bytes)")
            return m
        }
    }
}

/**
 * A v2 `vault.json` (docs/ON_DEMAND.md §1.1), as far as the app reads it: what a [VaultEntity]
 * shows and what tells two copies of one vault apart. libwasl creates and fully validates it
 * ([VaultV2]); the app keeps the text as it came ([VaultEntity.manifestJson]).
 */
@Serializable
data class VaultManifestV2(
    val format: String,
    val version: Int,
    val kdf: String,
    val m: Long,
    val t: Long,
    val p: Long,
    val salt: String,
    val keyCheck: String,
    val created: String,
    val name: String,
) {
    /** Same salt, Argon2id cost, format and key check: the same vault (a 2.1 vault.json edited
     *  back to 2.0 on Drive is not the same vault). */
    fun sameVaultAs(other: VaultManifestV2): Boolean =
        version == other.version && salt == other.salt && m == other.m && t == other.t && p == other.p &&
            java.security.MessageDigest.isEqual(keyCheck.toByteArray(), other.keyCheck.toByteArray())

    fun toEntity(id: String, name: String, manifestJson: String, now: Long = System.currentTimeMillis()): VaultEntity = VaultEntity(
        id = id, name = name, saltB64 = salt, iterations = 0, keyCheckB64 = keyCheck,
        createdAt = TimeFmt.parseRfc3339(created) ?: now, version = VERSION, manifestJson = manifestJson,
    )

    companion object {
        const val FORMAT = "wasl-vault"
        const val VERSION = 2
        /** The newest vault.json version the engine writes (format 2.1). */
        const val LATEST_VERSION = 3

        /** Reads the fields the app uses. Throws [AppError.Integrity] for text that is not a v2 vault.json. */
        fun parse(text: String): VaultManifestV2 {
            val m = try {
                VaultManifest.DefaultJson.decodeFromString(serializer(), text.trim())
            } catch (e: Exception) {
                throw AppError.Integrity("vault.json is not a valid Wasl vault manifest (${e.message?.lineSequence()?.firstOrNull() ?: "parse error"})")
            }
            // version 2 is format 2.0, version 3 is format 2.1 (keyed chunking, padded chunks): the
            // engine reads and writes both; the entity keeps VERSION as the family.
            if (m.format != FORMAT || m.version !in VERSION..LATEST_VERSION) throw AppError.Integrity("vault.json is not a Wasl vault of format 2")
            return m
        }

        /** Whether a kept vault.json is format 2.1 (what the Vaults screen shows); false for 2.0 or unreadable text. */
        fun isFormat21(manifestJson: String?): Boolean =
            manifestJson != null && runCatching { parse(manifestJson).version >= LATEST_VERSION }.getOrDefault(false)

        fun parseOrNull(text: String?): VaultManifestV2? = text?.let { runCatching { parse(it) }.getOrNull() }
    }
}
