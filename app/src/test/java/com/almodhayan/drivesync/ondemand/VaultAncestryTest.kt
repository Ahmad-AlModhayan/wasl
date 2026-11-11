package com.almodhayan.drivesync.ondemand

import com.almodhayan.drivesync.ondemand.VaultAncestry.Parent
import com.almodhayan.drivesync.ondemand.VaultAncestry.Place
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Everything below an encrypted pair's vault folder is part of the vault, however deep, and the
 * provider writes nowhere it cannot tell is outside one.
 *
 * My Drive: ROOT/Photos/a.jpg, ROOT/Vault (an encrypted pair's folder)/sub/deep/file.enc,
 * ROOT/Vault/x.enc, a top-level shared item, and Lost/orphan.txt whose folder is unknown.
 */
class VaultAncestryTest {
    private val parents = mapOf(
        "photos" to "ROOT", "a.jpg" to "photos",
        "vault" to "ROOT", "sub" to "vault", "deep" to "sub", "file.enc" to "deep", "x.enc" to "vault",
        "shared" to null,
        "orphan.txt" to "lost",
        "loop1" to "loop2", "loop2" to "loop1",
    )
    private val vaults = setOf("vault")
    private val asked = mutableListOf<String>()

    private fun place(id: String, folders: Set<String> = vaults, memo: MutableMap<String, Place> = HashMap()): Place = runBlocking {
        VaultAncestry.place(id, folders, { it == "ROOT" || it == DocumentIds.SHARED_WITH_ME }, memo) { cur ->
            asked += cur
            if (parents.containsKey(cur)) Parent.Known(parents[cur]) else Parent.Unknown
        }
    }

    @Test fun `a nested folder and file inside a vault are inside it`() {
        assertEquals(Place.VAULT_FOLDER, place("vault"))
        assertEquals(Place.INSIDE, place("sub"))
        assertEquals(Place.INSIDE, place("deep"))
        assertEquals(Place.INSIDE, place("file.enc"))
        assertEquals(Place.INSIDE, place("x.enc"))
    }

    @Test fun `items elsewhere are outside`() {
        assertEquals(Place.OUTSIDE, place("photos"))
        assertEquals(Place.OUTSIDE, place("a.jpg"))
        assertEquals(Place.OUTSIDE, place("ROOT"))
        assertEquals(Place.OUTSIDE, place("shared"))
        assertEquals(Place.OUTSIDE, place(DocumentIds.SHARED_WITH_ME))
    }

    @Test fun `an ancestry that cannot be followed counts as inside`() {
        assertEquals(Place.UNKNOWN, place("orphan.txt"))
        assertEquals(Place.UNKNOWN, place("not-indexed"))
        assertEquals(Place.UNKNOWN, place("loop1"))
    }

    @Test fun `only outside is writable`() {
        assertTrue(Place.OUTSIDE.writable)
        for (p in listOf(Place.VAULT_FOLDER, Place.INSIDE, Place.UNKNOWN)) assertFalse(p.name, p.writable)
        assertTrue(Place.INSIDE.insideVault)
        assertTrue(Place.UNKNOWN.insideVault)
        assertFalse(Place.VAULT_FOLDER.insideVault)
        assertFalse(Place.OUTSIDE.insideVault)
    }

    @Test fun `without vaults nothing is looked up`() {
        assertEquals(Place.OUTSIDE, place("file.enc", folders = emptySet()))
        assertEquals(Place.OUTSIDE, place("orphan.txt", folders = emptySet()))
        assertTrue(asked.isEmpty())
    }

    @Test fun `a listing walks its folder's ancestry once`() {
        val memo = HashMap<String, Place>()
        assertEquals(Place.INSIDE, place("file.enc", memo = memo))
        asked.clear()
        // A sibling in the same folder: one lookup (itself), then the folder is known.
        assertEquals(Place.INSIDE, place("deep", memo = memo))
        assertTrue(asked.isEmpty())
        assertEquals(Place.OUTSIDE, place("a.jpg", memo = memo))
        asked.clear()
        assertEquals(Place.OUTSIDE, place("photos", memo = memo))
        assertTrue(asked.isEmpty())
    }
}
