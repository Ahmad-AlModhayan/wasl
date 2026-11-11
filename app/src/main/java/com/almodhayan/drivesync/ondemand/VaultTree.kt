package com.almodhayan.drivesync.ondemand

import com.kahflabs.wasl.ffi.VaultItem

/**
 * Document ids of items inside an encrypted pair's vault folder. Their file-id part is
 * `w~<vault folder Drive id>~<item Drive id>`: Drive ids are `[A-Za-z0-9_-]+`, so neither '~' nor
 * the ':' [DocumentIds.parse] splits at can occur in one, and a vault id is never taken for a plain
 * Drive file (which would serve its ciphertext). Pure, unit-tested.
 */
object VaultDocIds {
    private const val PREFIX = "w~"
    private const val SEP = '~'

    data class Ref(val folderId: String, val itemId: String)

    fun encode(folderId: String, itemId: String): String = "$PREFIX$folderId$SEP$itemId"

    fun parse(fileId: String): Ref? {
        if (!fileId.startsWith(PREFIX)) return null
        val rest = fileId.substring(PREFIX.length)
        val i = rest.indexOf(SEP)
        if (i <= 0 || i == rest.length - 1 || rest.indexOf(SEP, i + 1) >= 0) return null
        return Ref(rest.substring(0, i), rest.substring(i + 1))
    }

    fun isVaultId(fileId: String): Boolean = parse(fileId) != null
}

/**
 * One listing of a vault folder (names decrypted by the engine), indexed for the provider: items
 * by Drive id and by parent path. Pure, unit-tested.
 */
class VaultTree(items: List<VaultItem>) {
    private val byId: Map<String, VaultItem> = items.associateBy { it.id }
    private val byParent: Map<String, List<VaultItem>> = items.groupBy { parentPath(it.path) }

    val size: Int get() = byId.size

    fun item(id: String): VaultItem? = byId[id]

    /** The items directly in the folder at [path] ("" for the vault folder itself). */
    fun children(path: String): List<VaultItem> = byParent[path].orEmpty()

    /** Whether [child] is somewhere below the folder [ancestor]. */
    fun isBelow(child: VaultItem, ancestor: VaultItem): Boolean =
        ancestor.isDir && child.path.length > ancestor.path.length && child.path.startsWith(ancestor.path + "/")

    companion object {
        fun parentPath(path: String): String = path.substringBeforeLast('/', "")
    }
}
