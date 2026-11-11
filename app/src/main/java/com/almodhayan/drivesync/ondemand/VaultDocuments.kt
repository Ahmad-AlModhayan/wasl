package com.almodhayan.drivesync.ondemand

import com.almodhayan.drivesync.crypto.VaultManager
import com.almodhayan.drivesync.data.Db
import com.kahflabs.wasl.ffi.OnDemandOrigin
import com.kahflabs.wasl.ffi.WaslException
import java.util.concurrent.ConcurrentHashMap
import javax.inject.Inject
import javax.inject.Singleton

/** How long a vault folder's listing is reused before the engine lists it again. */
private const val LISTING_TTL_MS = 60_000L

/**
 * The encrypted pairs' vault folders as the provider shows them: their files and folders with
 * names decrypted by the engine (vault v1 or v2), read on demand through [EngineReader], and
 * read-only. While a vault is locked its folder shows as it is on Drive.
 */
@Singleton
class VaultDocuments @Inject constructor(
    private val db: Db,
    private val vaults: VaultManager,
    private val engine: EngineReader,
) {
    /** The Drive folder of an encrypted pair, and its vault. */
    data class VaultFolder(val accountId: String, val folderId: String, val driveId: String?, val vaultId: String, val name: String) {
        val origin: OnDemandOrigin get() = OnDemandOrigin.Vault(accountId, vaultId, folderId, driveId)
    }

    private class Cached(val tree: VaultTree, val at: Long)

    private val listings = ConcurrentHashMap<String, Cached>()

    /** Every vault folder of the account (a pair's Drive folder whose pair is encrypted). */
    suspend fun folders(accountId: String): Map<String, VaultFolder> {
        val out = HashMap<String, VaultFolder>()
        for (p in db.pairs().getAll()) {
            val vaultId = p.vaultId ?: continue
            if (p.accountId != accountId || !p.encrypted || p.remoteFolderId.isBlank()) continue
            out.putIfAbsent(p.remoteFolderId, VaultFolder(accountId, p.remoteFolderId, p.driveId, vaultId, p.name))
        }
        return out
    }

    suspend fun folder(accountId: String, folderId: String): VaultFolder? = folders(accountId)[folderId]

    /** Whether the vault's keys are on this phone. A locked vault's opened state is dropped from the reader. */
    suspend fun unlocked(folder: VaultFolder): Boolean {
        if (vaults.isUnlocked(folder.vaultId)) return true
        listings.remove(folder.folderId)
        engine.closeVault(folder.vaultId)
        return false
    }

    /**
     * The folder's decrypted listing; reused for a minute unless [fresh]. When the engine cannot
     * list it (offline) an older listing of this run is used. Throws while the vault is locked.
     */
    suspend fun tree(folder: VaultFolder, fresh: Boolean = false): VaultTree {
        val now = System.currentTimeMillis()
        val cached = listings[folder.folderId]
        if (!fresh) cached?.takeIf { now - it.at < LISTING_TTL_MS }?.let { return it.tree }
        val items = try {
            engine.reader.vaultItems(folder.origin)
        } catch (e: WaslException.Network) {
            cached?.let { return it.tree }
            throw e
        }
        val tree = VaultTree(items)
        listings[folder.folderId] = Cached(tree, now)
        return tree
    }

    /** The listing as last read, without asking the engine (null when none or stale). */
    fun cachedTree(folderId: String): VaultTree? =
        listings[folderId]?.takeIf { System.currentTimeMillis() - it.at < LISTING_TTL_MS }?.tree
}
