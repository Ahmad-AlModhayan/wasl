package com.almodhayan.drivesync.ondemand

/**
 * Where a Drive item is relative to the encrypted pairs' vault folders. Everything below a vault
 * folder is part of the vault (encrypted names and content on Drive, v2 chunks and markers), so
 * the provider never writes there: a file created, moved, written or renamed inside would sit in
 * the vault unencrypted, or break it. When the ancestry cannot be told (a parent neither in the
 * index nor reachable), the item counts as inside: fail closed. Pure, unit-tested.
 */
object VaultAncestry {
    enum class Place {
        /** Not in any vault: the provider writes as it always did. */
        OUTSIDE,
        /** An encrypted pair's Drive folder itself (it may be renamed or binned, nothing made in it). */
        VAULT_FOLDER,
        /** Below a vault folder: read-only. */
        INSIDE,
        /** The ancestry could not be determined: treated as inside. */
        UNKNOWN;

        /** Whether the provider may create, write, move, rename or delete here. */
        val writable: Boolean get() = this == OUTSIDE

        /** A vault's own object (or maybe one): not to be renamed, moved, written or deleted from here. */
        val insideVault: Boolean get() = this == INSIDE || this == UNKNOWN
    }

    /** What the index (or Drive) says of an item's parent. */
    sealed interface Parent {
        /** [id] is null for an item at the top (no parent: shared with me, a shared drive's root). */
        data class Known(val id: String?) : Parent
        data object Unknown : Parent
    }

    private const val MAX_DEPTH = 256

    /**
     * The place of [fileId]: walks its parents ([parentOf]) up to the top ([isTop]: My Drive's root
     * and the virtual folders) looking for one of [vaultFolders]. [memo] keeps the place of every
     * item walked through, so a listing walks its folder's ancestry once.
     */
    suspend fun place(
        fileId: String,
        vaultFolders: Set<String>,
        isTop: (String) -> Boolean,
        memo: MutableMap<String, Place> = HashMap(),
        parentOf: suspend (String) -> Parent,
    ): Place {
        if (vaultFolders.isEmpty()) return Place.OUTSIDE
        if (fileId in vaultFolders) return Place.VAULT_FOLDER
        memo[fileId]?.let { return it }
        val walked = ArrayList<String>()
        var cur = fileId
        val result: Place = run {
            repeat(MAX_DEPTH) {
                if (cur != fileId) {
                    if (cur in vaultFolders) return@run Place.INSIDE
                    memo[cur]?.let { return@run if (it == Place.VAULT_FOLDER) Place.INSIDE else it }
                }
                if (isTop(cur)) return@run Place.OUTSIDE
                walked += cur
                when (val p = parentOf(cur)) {
                    Parent.Unknown -> return@run Place.UNKNOWN
                    is Parent.Known -> cur = p.id ?: return@run Place.OUTSIDE
                }
            }
            Place.UNKNOWN   // deeper than any real tree: a loop in the index
        }
        // Everything walked through is where the item is: inside the same vault, or outside all.
        for (id in walked) memo[id] = result
        return result
    }
}
