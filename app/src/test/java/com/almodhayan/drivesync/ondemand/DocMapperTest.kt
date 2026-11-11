package com.almodhayan.drivesync.ondemand

import com.almodhayan.drivesync.core.NameMapper
import com.almodhayan.drivesync.data.db.DriveNodeEntity
import com.almodhayan.drivesync.drive.DocsExport
import com.almodhayan.drivesync.drive.FOLDER_MIME
import com.almodhayan.drivesync.drive.SHORTCUT_MIME
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class DocMapperTest {
    private val account = "me@gmail.com"
    private val xlsxMime = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
    private val docxMime = "application/vnd.openxmlformats-officedocument.wordprocessingml.document"

    private fun node(
        id: String = "f1",
        name: String = "file.bin",
        mime: String = "application/octet-stream",
        parentId: String? = "root1",
        size: Long? = null,
        md5: String? = null,
        modified: Long = 1_700_000_000_000L,
        version: Long? = 7,
        trashed: Boolean = false,
        isFolder: Boolean = mime == FOLDER_MIME,
        isShortcut: Boolean = mime == SHORTCUT_MIME,
        targetId: String? = null,
        targetMime: String? = null,
    ) = DriveNodeEntity(
        accountId = account, id = id, name = name, mimeType = mime, parentId = parentId, driveId = null, size = size, md5 = md5,
        modifiedTime = modified, version = version, trashed = trashed, isFolder = isFolder, isShortcut = isShortcut,
        shortcutTargetId = targetId, shortcutTargetMime = targetMime,
    )

    @Test
    fun googleSheetIsExportedAsXlsx() {
        val doc = DocMapper.map(node(name = "Budget", mime = "application/vnd.google-apps.spreadsheet", version = 7))!!
        assertEquals(DocKind.GOOGLE_DOC, doc.kind)
        assertEquals("Budget.xlsx", doc.displayName)
        assertEquals("Budget", doc.driveName)
        assertEquals(xlsxMime, doc.mimeType)
        assertEquals("xlsx", doc.exportTarget!!.extension)
        assertEquals("v7.xlsx", doc.cacheTag)
        assertNull(doc.size)
        assertTrue(doc.readOnly)
        assertEquals(1_700_000_000_000L, doc.lastModified)
    }

    @Test
    fun existingExportExtensionIsNotDuplicated() {
        val doc = DocMapper.map(node(name = "Report.docx", mime = "application/vnd.google-apps.document"))!!
        assertEquals("Report.docx", doc.displayName)
        assertEquals(docxMime, doc.mimeType)
    }

    @Test
    fun googleTypesWithoutExportAreHidden() {
        assertNull(DocMapper.map(node(name = "Survey", mime = "application/vnd.google-apps.form")))
        assertNull(DocMapper.map(node(name = "Site", mime = "application/vnd.google-apps.site")))
    }

    @Test
    fun folderMapsToDirectoryMime() {
        val doc = DocMapper.map(node(id = "d1", name = "Photos", mime = FOLDER_MIME))!!
        assertEquals(DocKind.FOLDER, doc.kind)
        assertTrue(doc.isFolder)
        assertEquals(DocMapper.DIR_MIME, doc.mimeType)
        assertEquals("vnd.android.document/directory", doc.mimeType)
        assertEquals("", doc.cacheTag)
        assertFalse(doc.readOnly)
    }

    @Test
    fun regularFileKeepsMimeSizeAndUsesMd5AsTag() {
        val doc = DocMapper.map(node(name = "book.epub", mime = "application/epub+zip", size = 1234, md5 = "ABCDEF0123"))!!
        assertEquals(DocKind.FILE, doc.kind)
        assertEquals("book.epub", doc.displayName)
        assertEquals("application/epub+zip", doc.mimeType)
        assertEquals(1234L, doc.size)
        assertEquals("abcdef0123", doc.cacheTag)
        assertEquals("ABCDEF0123", doc.md5)
        assertNull(doc.exportTarget)
        assertFalse(doc.readOnly)
    }

    @Test
    fun fileWithoutMd5FallsBackToVersionThenModifiedTime() {
        assertEquals("v3", DocMapper.map(node(md5 = null, version = 3))!!.cacheTag)
        assertEquals("v1700000000000", DocMapper.map(node(md5 = null, version = null))!!.cacheTag)
        assertEquals("v9.pdf", DocMapper.exportCacheTag(9, 1L, DocsExport.Target("application/pdf", "pdf")))
    }

    @Test
    fun blankMimeBecomesOctetStream() {
        assertEquals(DocMapper.OCTET_STREAM, DocMapper.map(node(mime = ""))!!.mimeType)
    }

    @Test
    fun trashedNodesAreHidden() {
        assertNull(DocMapper.map(node(trashed = true)))
    }

    @Test
    fun zeroModifiedTimeMapsToNull() {
        assertNull(DocMapper.map(node(modified = 0))!!.lastModified)
    }

    @Test
    fun shortcutResolvesToItsTarget() {
        val shortcut = node(id = "s1", name = "Link to manual", mime = SHORTCUT_MIME, parentId = "folderA", targetId = "t1", targetMime = "application/pdf")
        val target = node(id = "t1", name = "manual.pdf", mime = "application/pdf", parentId = "folderB", size = 10, md5 = "aa")
        val doc = DocMapper.map(shortcut) { id -> if (id == "t1") target else null }!!
        assertEquals("t1", doc.fileId)                 // document id points at the real file
        assertEquals("Link to manual", doc.displayName) // but the user sees the shortcut's name
        assertEquals("application/pdf", doc.mimeType)
        assertEquals(10L, doc.size)
        assertEquals("aa", doc.cacheTag)
        assertEquals("folderA", doc.parentId)          // listed where the shortcut lives
    }

    @Test
    fun shortcutToGoogleDocGetsExportExtension() {
        val shortcut = node(id = "s1", name = "Notes", mime = SHORTCUT_MIME, targetId = "t1", targetMime = "application/vnd.google-apps.document")
        val target = node(id = "t1", name = "Notes", mime = "application/vnd.google-apps.document", version = 2)
        val doc = DocMapper.map(shortcut) { target }!!
        assertEquals("Notes.docx", doc.displayName)
        assertEquals(DocKind.GOOGLE_DOC, doc.kind)
        assertEquals("v2.docx", doc.cacheTag)
    }

    @Test
    fun unresolvedShortcutUsesTargetMetadata() {
        val folderLink = DocMapper.map(node(id = "s1", name = "Shared album", mime = SHORTCUT_MIME, targetId = "t9", targetMime = FOLDER_MIME))!!
        assertEquals(DocKind.FOLDER, folderLink.kind)
        assertEquals("t9", folderLink.fileId)

        val fileLink = DocMapper.map(node(id = "s2", name = "song.mp3", mime = SHORTCUT_MIME, targetId = "t8", targetMime = "audio/mpeg"))!!
        assertEquals(DocKind.FILE, fileLink.kind)
        assertEquals("audio/mpeg", fileLink.mimeType)
        assertNull(fileLink.size)
        assertEquals("v0", fileLink.cacheTag)

        val docLink = DocMapper.map(node(id = "s3", name = "Deck", mime = SHORTCUT_MIME, targetId = "t7", targetMime = "application/vnd.google-apps.presentation"))!!
        assertEquals("Deck.pptx", docLink.displayName)
        assertEquals("v0.pptx", docLink.cacheTag)
    }

    @Test
    fun danglingShortcutIsHidden() {
        assertNull(DocMapper.map(node(id = "s1", name = "broken", mime = SHORTCUT_MIME, targetId = null)))
        assertNull(DocMapper.map(node(id = "s2", name = "form link", mime = SHORTCUT_MIME, targetId = "t1", targetMime = "application/vnd.google-apps.form")))
    }

    @Test
    fun unsafeCharactersAreMappedForDisplayOnly() {
        val doc = DocMapper.map(node(name = "a:b?c*.txt", mime = "text/plain"))!!
        assertEquals(NameMapper.toLocalName("a:b?c*.txt"), doc.displayName)
        assertFalse(doc.displayName.contains(':'))
        assertEquals("a:b?c*.txt", doc.driveName)
    }

    @Test
    fun renameStripsTheExportExtensionForGoogleDocsOnly() {
        val sheet = DocMapper.map(node(name = "Budget", mime = "application/vnd.google-apps.spreadsheet"))!!
        assertEquals("Budget 2027", DocMapper.driveNameForRename(sheet, "Budget 2027.xlsx"))
        assertEquals("Budget 2027", DocMapper.driveNameForRename(sheet, "Budget 2027.XLSX"))
        assertEquals("Budget 2027", DocMapper.driveNameForRename(sheet, "Budget 2027"))
        assertEquals(".xlsx", DocMapper.driveNameForRename(sheet, ".xlsx"))   // nothing left to strip

        val pdf = DocMapper.map(node(name = "manual.pdf", mime = "application/pdf"))!!
        assertEquals("manual v2.pdf", DocMapper.driveNameForRename(pdf, "manual v2.pdf"))
    }

    @Test
    fun isGoogleDocExcludesFoldersAndShortcuts() {
        assertTrue(DocMapper.isGoogleDoc("application/vnd.google-apps.document"))
        assertFalse(DocMapper.isGoogleDoc(FOLDER_MIME))
        assertFalse(DocMapper.isGoogleDoc(SHORTCUT_MIME))
        assertFalse(DocMapper.isGoogleDoc("application/pdf"))
        assertNotNull(DocsExport.target("application/vnd.google-apps.drawing", preferPdf = false))
    }
}
