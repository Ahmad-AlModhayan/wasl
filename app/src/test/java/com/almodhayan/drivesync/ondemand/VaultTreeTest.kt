package com.almodhayan.drivesync.ondemand

import com.kahflabs.wasl.ffi.VaultItem
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Vault documents in the provider: their ids, and the listing indexed by folder. */
class VaultTreeTest {
    private fun item(path: String, id: String, dir: Boolean = false) =
        VaultItem(path = path, name = path.substringAfterLast('/'), id = id, isDir = dir, size = if (dir) 0u else 10u, mtime = 1L, version = "1")

    @Test fun `vault ids round-trip and are never taken for Drive ids`() {
        val id = VaultDocIds.encode("FOLDER_1-a", "item-B_2")
        assertEquals(VaultDocIds.Ref("FOLDER_1-a", "item-B_2"), VaultDocIds.parse(id))
        // The provider's document id splits at the last ':'; a vault id has none.
        val doc = DocumentIds.parse(OnDemandConstants.documentId("me@example.com", id))!!
        assertEquals(id, doc.fileId)
        assertTrue(VaultDocIds.isVaultId(id))
        for (plain in listOf("1AbC-def_GHI", "root", DocumentIds.SHARED_WITH_ME, "w~", "w~a", "w~~b", "w~a~", "w~a~b~c")) {
            assertNull(plain, VaultDocIds.parse(plain))
        }
    }

    @Test fun `children are found by folder and the root is the vault folder`() {
        val tree = VaultTree(
            listOf(
                item("Docs", "d1", dir = true),
                item("Docs/a.pdf", "f1"),
                item("Docs/Deep", "d2", dir = true),
                item("Docs/Deep/b.txt", "f2"),
                item("top.jpg", "f3"),
            ),
        )
        assertEquals(5, tree.size)
        assertEquals(setOf("d1", "f3"), tree.children("").map { it.id }.toSet())
        assertEquals(setOf("f1", "d2"), tree.children("Docs").map { it.id }.toSet())
        assertEquals(listOf("f2"), tree.children("Docs/Deep").map { it.id })
        assertTrue(tree.children("nothing").isEmpty())

        val docs = tree.item("d1")!!
        val deep = tree.item("f2")!!
        assertTrue(tree.isBelow(deep, docs))
        assertTrue(tree.isBelow(deep, tree.item("d2")!!))
        assertFalse(tree.isBelow(docs, deep))
        assertFalse(tree.isBelow(tree.item("f3")!!, docs))
        // "Docs2/x" is not below "Docs".
        val other = VaultTree(listOf(item("Docs", "d1", dir = true), item("Docs2", "d3", dir = true), item("Docs2/x", "f4")))
        assertFalse(other.isBelow(other.item("f4")!!, other.item("d1")!!))
    }
}
