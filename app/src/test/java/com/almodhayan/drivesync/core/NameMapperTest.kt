package com.almodhayan.drivesync.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class NameMapperTest {

    // ---------------------------------------------------------------- needsMapping

    @Test
    fun ordinaryNames_doNotNeedMapping() {
        listOf(
            "Report 2026.docx", "résumé.pdf", "日本語ファイル.txt", "file-name_v2 (1).tar.gz",
            "IMG_20260930-140301.jpg", ".hidden", "no-extension", "a b c", "emoji 😀.png", "trailing.",
        ).forEach { assertFalse("'$it' should not need mapping", NameMapper.needsMapping(it)) }
    }

    @Test
    fun namesWithFatInvalidCharacters_needMapping() {
        listOf("a:b", "a\"b", "a*b", "a<b", "a>b", "a?b", "a\\b", "a|b").forEach {
            assertTrue("'$it' should need mapping", NameMapper.needsMapping(it))
        }
    }

    @Test
    fun specialNames_needMapping() {
        assertTrue(NameMapper.needsMapping("."))
        assertTrue(NameMapper.needsMapping(".."))
        assertTrue(NameMapper.needsMapping(""))
        assertTrue(NameMapper.needsMapping("tab\tname"))
        assertTrue(NameMapper.needsMapping("new\nline"))
        assertTrue(NameMapper.needsMapping("a".repeat(256)))
        assertFalse(NameMapper.needsMapping("a".repeat(255)))
        // byte length counts, not character length: 128 two-byte characters = 256 bytes
        assertTrue(NameMapper.needsMapping("é".repeat(128)))
        assertFalse(NameMapper.needsMapping("é".repeat(127)))
    }

    // ---------------------------------------------------------------- toLocalName

    @Test
    fun ordinaryName_isReturnedUnchanged() {
        assertEquals("Report 2026.docx", NameMapper.toLocalName("Report 2026.docx"))
    }

    @Test
    fun fatInvalidCharacters_areMappedToFullWidthEquivalents() {
        assertEquals("a＂b", NameMapper.toLocalName("a\"b"))
        assertEquals("a＊b", NameMapper.toLocalName("a*b"))
        assertEquals("a：b", NameMapper.toLocalName("a:b"))
        assertEquals("a＜b", NameMapper.toLocalName("a<b"))
        assertEquals("a＞b", NameMapper.toLocalName("a>b"))
        assertEquals("a？b", NameMapper.toLocalName("a?b"))
        assertEquals("a＼b", NameMapper.toLocalName("a\\b"))
        assertEquals("a｜b", NameMapper.toLocalName("a|b"))
        // '&' is legal on FAT and must survive untouched
        assertEquals("Q&A： what？ ＜draft＞ ＊final＊ ｜ v2＂.txt", NameMapper.toLocalName("Q&A: what? <draft> *final* | v2\".txt"))
    }

    @Test
    fun controlCharacters_areReplacedBySymbolForNull() {
        assertEquals("a␀b␀c", NameMapper.toLocalName("a\u0001b\nc"))
        assertEquals("tab␀sep", NameMapper.toLocalName("tab\tsep"))
    }

    @Test
    fun dotAndDotDot_getAPrefix() {
        assertEquals("＿.", NameMapper.toLocalName("."))
        assertEquals("＿..", NameMapper.toLocalName(".."))
        assertEquals("...", NameMapper.toLocalName("..."))
    }

    @Test
    fun emptyName_becomesUnnamed() {
        assertEquals("＿unnamed", NameMapper.toLocalName(""))
    }

    @Test
    fun overlongName_isTruncatedButKeepsExtension() {
        val out = NameMapper.toLocalName("a".repeat(300) + ".txt")
        assertEquals("a".repeat(251) + ".txt", out)
        assertEquals(255, out.toByteArray(Charsets.UTF_8).size)
    }

    @Test
    fun overlongMultiByteName_isTruncatedByBytesNotCharacters() {
        val out = NameMapper.toLocalName("é".repeat(200) + ".jpg")
        assertTrue(out.endsWith(".jpg"))
        assertTrue(out.startsWith("é"))
        assertTrue(out.toByteArray(Charsets.UTF_8).size <= 255)
        // 125 * 2 bytes + ".jpg" (4 bytes) = 254: the largest fit, never cutting a character in half
        assertEquals("é".repeat(125) + ".jpg", out)
    }

    @Test
    fun overlongNameWithoutExtension_isSimplyTruncated() {
        val out = NameMapper.toLocalName("b".repeat(400))
        assertEquals("b".repeat(255), out)
    }

    @Test
    fun overlongNameWithLongExtension_doesNotPreserveTheExtension() {
        val ext = "x".repeat(20)   // 20 chars: not treated as an extension (limit is < 16)
        val out = NameMapper.toLocalName("a".repeat(300) + "." + ext)
        assertEquals("a".repeat(255), out)
    }

    @Test
    fun overlongNameWithInvalidCharacters_isMappedThenTruncated() {
        val out = NameMapper.toLocalName("a:b".repeat(120) + ".mp4")
        assertTrue(out.endsWith(".mp4"))
        assertFalse(out.contains(':'))
        assertTrue(out.contains('：'))
        assertTrue(out.toByteArray(Charsets.UTF_8).size <= 255)
    }

    // ---------------------------------------------------------------- conflictName

    @Test
    fun conflictName_keepsExtension() {
        assertEquals(
            "report (conflicted copy from Pixel, 2026-09-30 14-03).docx",
            NameMapper.conflictName("report.docx", "Pixel", "2026-09-30 14-03"),
        )
    }

    @Test
    fun conflictName_withoutExtension() {
        assertEquals(
            "README (conflicted copy from Pixel, 2026-09-30 14-03)",
            NameMapper.conflictName("README", "Pixel", "2026-09-30 14-03"),
        )
    }

    @Test
    fun conflictName_dotFileHasNoExtension() {
        assertEquals(
            ".bashrc (conflicted copy from Pixel, s)",
            NameMapper.conflictName(".bashrc", "Pixel", "s"),
        )
    }

    @Test
    fun conflictName_usesOnlyTheLastExtension() {
        assertEquals(
            "archive.tar (conflicted copy from Pixel, s).gz",
            NameMapper.conflictName("archive.tar.gz", "Pixel", "s"),
        )
    }

    @Test
    fun conflictName_longSuffixIsNotAnExtension() {
        assertEquals(
            "notes.averyveryverylongext (conflicted copy from Pixel, s)",
            NameMapper.conflictName("notes.averyveryverylongext", "Pixel", "s"),
        )
        // 11 characters after the dot is still an extension
        assertEquals(
            "n (conflicted copy from Pixel, s).abcdefghijk",
            NameMapper.conflictName("n.abcdefghijk", "Pixel", "s"),
        )
    }

    @Test
    fun conflictName_trailingDotIsNotAnExtension() {
        assertEquals("name. (conflicted copy from Pixel, s)", NameMapper.conflictName("name.", "Pixel", "s"))
    }
}
