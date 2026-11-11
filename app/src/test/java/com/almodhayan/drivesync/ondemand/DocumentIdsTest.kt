package com.almodhayan.drivesync.ondemand

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import java.io.FileNotFoundException

class DocumentIdsTest {

    @Test
    fun parsesAccountAndFileId() {
        val id = DocumentIds.parse("me@gmail.com:1AbC-def_9")
        assertEquals("me@gmail.com", id!!.accountId)
        assertEquals("1AbC-def_9", id.fileId)
    }

    @Test
    fun splitsAtTheLastColon() {
        // Drive ids never contain ':', so anything before the last ':' belongs to the account.
        val id = DocumentIds.parse("odd:local@example.com:root")
        assertEquals("odd:local@example.com", id!!.accountId)
        assertEquals("root", id.fileId)
    }

    @Test
    fun roundTripsThroughTheSharedConvention() {
        val text = OnDemandConstants.documentId("a@b.com", "xyz")
        assertEquals("a@b.com:xyz", text)
        assertEquals(DocId("a@b.com", "xyz"), DocumentIds.parse(text))
        assertEquals(text, DocId("a@b.com", "xyz").toString())
    }

    @Test
    fun rootAndVirtualIdsParseLikeAnyOther() {
        assertEquals(DocId("a@b.com", DocumentIds.ROOT_ALIAS), DocumentIds.parse("a@b.com:root"))
        assertEquals(DocId("a@b.com", DocumentIds.SHARED_WITH_ME), DocumentIds.parse("a@b.com:shared-with-me"))
    }

    @Test
    fun rejectsMalformedIds() {
        assertNull(DocumentIds.parse(null))
        assertNull(DocumentIds.parse(""))
        assertNull(DocumentIds.parse("nocolon"))
        assertNull(DocumentIds.parse(":abc"))
        assertNull(DocumentIds.parse("a@b.com:"))
    }

    @Test(expected = FileNotFoundException::class)
    fun parseOrThrowSignalsNotFound() {
        DocumentIds.parseOrThrow("garbage")
    }
}
