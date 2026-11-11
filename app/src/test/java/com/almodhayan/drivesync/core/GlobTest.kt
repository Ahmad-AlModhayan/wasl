package com.almodhayan.drivesync.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class GlobTest {

    private fun matches(pattern: String, path: String, isDir: Boolean = false): Boolean =
        Glob.compile(pattern).matches(path, isDir)

    private fun assertMatches(pattern: String, vararg paths: String) {
        for (p in paths) assertTrue("'$pattern' should match '$p'", matches(pattern, p))
    }

    private fun assertNoMatch(pattern: String, vararg paths: String) {
        for (p in paths) assertFalse("'$pattern' should NOT match '$p'", matches(pattern, p))
    }

    // ---------------------------------------------------------------- operators

    @Test
    fun questionMark_matchesExactlyOneCharacterButNeverSlash() {
        assertMatches("/a?c.txt", "abc.txt", "a-c.txt", "aXc.txt")
        assertNoMatch("/a?c.txt", "ac.txt", "abbc.txt", "a/c.txt")
    }

    @Test
    fun star_matchesAnyRunOfCharactersExceptSlash() {
        assertMatches("/*.jpg", "a.jpg", "photo 1.jpg", ".jpg")
        assertNoMatch("/*.jpg", "sub/a.jpg", "a.jpeg", "ajpg")
        assertMatches("/IMG_*", "IMG_", "IMG_0001.jpg")
    }

    @Test
    fun doubleStarWithSlash_matchesZeroOrMoreDirectories() {
        assertMatches("/a/**/b.txt", "a/b.txt", "a/x/b.txt", "a/x/y/z/b.txt")
        assertNoMatch("/a/**/b.txt", "b.txt", "a/b.txt/c", "q/a/b.txt")
    }

    @Test
    fun doubleStarAtEnd_matchesWholeSubtreeButNotTheFolderItself() {
        assertMatches("/cache/**", "cache/x", "cache/x/y.bin")
        assertNoMatch("/cache/**", "cache", "cache2/x")
    }

    @Test
    fun doubleStarWithoutSlash_matchesAcrossDirectories() {
        assertMatches("/foo**", "foo", "foobar", "foo/bar/baz")
        assertNoMatch("/foo**", "xfoo")
    }

    @Test
    fun characterClass_matchesOneListedCharacter() {
        assertMatches("/f[abc].txt", "fa.txt", "fb.txt", "fc.txt")
        assertNoMatch("/f[abc].txt", "fd.txt", "f.txt", "fab.txt")
        assertMatches("/img[0-9].png", "img0.png", "img7.png")
        assertNoMatch("/img[0-9].png", "imga.png", "img10.png")
    }

    @Test
    fun negatedCharacterClass_matchesAnythingButListedCharacters() {
        assertMatches("/f[!abc].txt", "fd.txt", "fz.txt", "f1.txt")
        assertNoMatch("/f[!abc].txt", "fa.txt", "fb.txt", "fc.txt", "f.txt")
        // '^' is accepted as an alias for '!'
        assertMatches("/f[^abc].txt", "fd.txt")
        assertNoMatch("/f[^abc].txt", "fa.txt")
    }

    @Test
    fun unterminatedCharacterClass_isTreatedAsLiteralBracket() {
        assertMatches("/f[a.txt", "f[a.txt")
        assertNoMatch("/f[a.txt", "fa.txt")
    }

    @Test
    fun numericRange_expandsToEveryIntegerInclusive() {
        assertMatches("/img{1..9}.jpg", "img1.jpg", "img5.jpg", "img9.jpg")
        assertNoMatch("/img{1..9}.jpg", "img0.jpg", "img10.jpg", "img.jpg")
        assertMatches("/img{8..12}.jpg", "img8.jpg", "img10.jpg", "img12.jpg")
        assertNoMatch("/img{8..12}.jpg", "img7.jpg", "img13.jpg")
        // reversed bounds are normalised
        assertMatches("/v{3..1}", "v1", "v2", "v3")
        assertNoMatch("/v{3..1}", "v4")
    }

    @Test
    fun numericRange_largerThanTwoThousand_isKeptLiteral() {
        assertMatches("/n{1..5000}", "n{1..5000}")
        assertNoMatch("/n{1..5000}", "n7")
    }

    @Test
    fun alternation_matchesAnyListedOption() {
        assertMatches("/*.{jpg,png}", "a.jpg", "b.png")
        assertNoMatch("/*.{jpg,png}", "a.gif", "a.jpgpng")
        assertMatches("/{Camera,Screenshots}/*", "Camera/x.jpg", "Screenshots/y.png")
        assertNoMatch("/{Camera,Screenshots}/*", "Downloads/x.jpg")
    }

    @Test
    fun braceWithoutRangeOrComma_isLiteral() {
        assertMatches("/a{b}c", "a{b}c")
        assertNoMatch("/a{b}c", "abc")
    }

    // ---------------------------------------------------------------- anchoring

    @Test
    fun patternWithoutLeadingSlash_matchesAtAnyDepth() {
        assertMatches("*.jpg", "a.jpg", "x/a.jpg", "x/y/z/a.jpg")
        assertMatches("thumbs.db", "thumbs.db", "Pictures/thumbs.db")
        assertNoMatch("thumbs.db", "mythumbs.db", "thumbs.db/x")
    }

    @Test
    fun leadingSlash_anchorsToPairRoot() {
        assertMatches("/*.jpg", "a.jpg")
        assertNoMatch("/*.jpg", "x/a.jpg")
        assertMatches("/build", "build")
        assertNoMatch("/build", "x/build", "build/x")
    }

    @Test
    fun explicitDoubleStarPrefix_isNotDoubled() {
        assertMatches("**/node_modules", "node_modules", "a/node_modules", "a/b/node_modules")
        assertNoMatch("**/node_modules", "node_modules/x")
    }

    @Test
    fun trailingSlash_matchesDirectoriesOnly() {
        val g = Glob.compile("build/")
        assertTrue(g.dirOnly)
        assertTrue(g.matches("build", isDir = true))
        assertTrue(g.matches("src/build", isDir = true))
        assertFalse("a file named 'build' is not a directory match", g.matches("build", isDir = false))
        // the glob itself does not match children: pruning is the scanner's job (see PathFilter tests)
        assertFalse(g.matches("build/out.bin", isDir = false))
    }

    @Test
    fun patternWithoutTrailingSlash_matchesFilesAndDirectories() {
        val g = Glob.compile("build")
        assertFalse(g.dirOnly)
        assertTrue(g.matches("build", isDir = true))
        assertTrue(g.matches("build", isDir = false))
    }

    @Test
    fun leadingSlashOnThePath_isIgnored() {
        assertMatches("/a.jpg", "/a.jpg")
        assertMatches("*.jpg", "/x/a.jpg")
    }

    // ---------------------------------------------------------------- case and escaping

    @Test
    fun matching_isCaseInsensitive() {
        assertMatches("*.JPG", "photo.jpg", "PHOTO.Jpg")
        assertMatches("/Photos/*", "photos/a.txt", "PHOTOS/b.txt")
        assertMatches("/[a-c]x", "Bx")
        assertMatches("/{Camera,DCIM}/*", "camera/x", "dcim/y")
    }

    @Test
    fun matching_isCaseInsensitiveForNonAscii() {
        assertMatches("/ärger.txt", "ÄRGER.txt")
    }

    @Test
    fun regexMetaCharacters_areTreatedLiterally() {
        assertMatches("/a+b(c).txt", "a+b(c).txt")
        assertNoMatch("/a+b(c).txt", "aab(c).txt", "abc.txt")
        assertMatches("/price\$.txt", "price\$.txt")
        assertMatches("/x|y.txt", "x|y.txt")
        assertNoMatch("/x|y.txt", "x", "y.txt")
        assertNoMatch("/a.txt", "aXtxt")
        assertMatches("/back\\slash", "back\\slash")
    }

    // ---------------------------------------------------------------- compile

    @Test(expected = IllegalArgumentException::class)
    fun compile_rejectsEmptyPattern() {
        Glob.compile("   ")
    }

    @Test
    fun compile_trimsWhitespaceAndKeepsRawPattern() {
        val g = Glob.compile("  *.jpg  ")
        assertEquals("  *.jpg  ", g.pattern)
        assertTrue(g.matches("a.jpg", false))
    }
}

class PathFilterTest {

    private fun filter(
        includes: List<String> = emptyList(),
        excludes: List<String> = emptyList(),
        excludedSubfolders: List<String> = emptyList(),
        syncHidden: Boolean = false,
    ) = PathFilter(includes, excludes, excludedSubfolders, syncHidden)

    @Test
    fun noPatterns_acceptsEverythingVisible() {
        val f = filter()
        assertTrue(f.accept("a.txt", false))
        assertTrue(f.accept("dir", true))
        assertTrue(f.accept("dir/sub/file.bin", false))
    }

    @Test
    fun rootPath_isAlwaysAccepted() {
        assertTrue(filter(excludes = listOf("**")).accept("", true))
        assertTrue(filter(excludes = listOf("**")).accept("/", true))
    }

    @Test
    fun hiddenEntries_areRejectedUnlessSyncHidden() {
        val f = filter(syncHidden = false)
        assertFalse(f.accept(".git", true))
        assertFalse(f.accept(".nomedia", false))
        assertFalse(f.accept("a/.thumbnails/x.jpg", false))
        assertFalse(f.accept(".hidden/visible.txt", false))
        assertTrue(f.accept("a.b/visible.txt", false))

        val h = filter(syncHidden = true)
        assertTrue(h.accept(".git", true))
        assertTrue(h.accept(".nomedia", false))
        assertTrue(h.accept("a/.thumbnails/x.jpg", false))
    }

    @Test
    fun internalPartAndTmpFiles_areAlwaysRejected() {
        val f = filter(includes = listOf("**"), syncHidden = true)
        assertFalse(f.accept("photo.jpg.drivesync-part", false))
        assertFalse(f.accept("sub/photo.jpg.drivesync-part", false))
        assertFalse(f.accept("x.drivesync-tmp", false))
        assertFalse(f.accept("photo.jpg.drivesync-part", true))
        assertTrue(f.accept("photo.jpg", false))
        // only the suffix is internal; a directory containing the marker in the middle is fine
        assertTrue(f.accept("drivesync-part-notes.txt", false))
    }

    @Test
    fun includes_applyToFilesOnly_soDirectoriesAreStillTraversed() {
        val f = filter(includes = listOf("*.jpg"))
        assertTrue(f.accept("a.jpg", false))
        assertTrue(f.accept("deep/a.JPG", false))
        assertFalse(f.accept("a.png", false))
        assertFalse(f.accept("deep/notes.txt", false))
        // directories are never filtered by includes, otherwise nothing below them could match
        assertTrue(f.accept("deep", true))
        assertTrue(f.accept("deep/deeper", true))
    }

    @Test
    fun excludes_rejectMatchingFiles() {
        val f = filter(excludes = listOf("*.tmp", "~*"))
        assertFalse(f.accept("a.tmp", false))
        assertFalse(f.accept("sub/a.tmp", false))
        assertFalse(f.accept("~lock.docx", false))
        assertTrue(f.accept("a.txt", false))
    }

    @Test
    fun excludeWinsOverInclude() {
        val f = filter(includes = listOf("*.txt"), excludes = listOf("secret*"))
        assertTrue(f.accept("notes.txt", false))
        assertFalse(f.accept("secret.txt", false))
        assertFalse(f.accept("a/secret-plan.txt", false))
    }

    @Test
    fun directoryExclude_prunesTheDirectoryItself() {
        val f = filter(excludes = listOf("node_modules/"))
        assertFalse("directory must be pruned", f.accept("node_modules", true))
        assertFalse(f.accept("a/b/node_modules", true))
        // a file that happens to have the folder's name is not affected by a dir-only pattern
        assertTrue(f.accept("node_modules", false))
        // children are not matched by the pattern: the scanner prunes at the directory, so it never asks
        assertTrue(f.accept("node_modules/x.js", false))
    }

    @Test
    fun excludePatternWithoutSlash_prunesDirectoriesAndFiles() {
        val f = filter(excludes = listOf("build"))
        assertFalse(f.accept("build", true))
        assertFalse(f.accept("build", false))
        assertFalse(f.accept("app/build", true))
        assertTrue(f.accept("builds", true))
    }

    @Test
    fun subtreeExclude_rejectsEverythingUnderThePattern() {
        val f = filter(excludes = listOf("cache/**"))
        assertFalse(f.accept("cache/a.bin", false))
        assertFalse(f.accept("x/cache/a/b.bin", false))
        assertFalse(f.accept("cache/sub", true))
        assertTrue("the folder itself is not matched by 'cache/**'", f.accept("cache", true))
    }

    @Test
    fun excludedSubfolders_matchPrefixCaseInsensitively() {
        val f = filter(excludedSubfolders = listOf("Photos/Raw", "/Videos/"))
        assertFalse(f.accept("Photos/Raw", true))
        assertFalse(f.accept("photos/raw", true))
        assertFalse(f.accept("Photos/Raw/img.dng", false))
        assertFalse(f.accept("PHOTOS/RAW/deep/img.dng", false))
        assertFalse(f.accept("Videos", true))
        assertFalse(f.accept("videos/clip.mp4", false))
        assertTrue("prefix must end at a path separator", f.accept("Photos/Rawr", true))
        assertTrue(f.accept("Photos", true))
        assertTrue(f.accept("Photos/Edited/img.jpg", false))
        assertTrue(f.accept("Other/Photos/Raw/img.dng", false))
    }

    @Test
    fun blankOrInvalidPatterns_areIgnored() {
        val f = filter(includes = listOf("   ", ""), excludes = listOf(" "), excludedSubfolders = listOf("/", " "))
        assertTrue(f.accept("anything.bin", false))
        assertTrue(f.accept("dir", true))
    }

    @Test
    fun patternsAreTrimmed() {
        val f = filter(excludes = listOf("  *.tmp  "))
        assertFalse(f.accept("a.tmp", false))
    }
}
