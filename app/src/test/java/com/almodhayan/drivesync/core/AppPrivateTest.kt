package com.almodhayan.drivesync.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** security review 2026-10: outside input must never point Wasl at its own private files. */
class AppPrivateTest {
    private val pkg = "com.kahflabs.wasl"
    private val roots = listOf("/data/user/0/$pkg", "/storage/emulated/0/Android/data/$pkg")

    private fun private(path: String) = AppPrivate.isPrivateLocalPath(path, roots, pkg)

    @Test
    fun ownAuthoritiesAreThePackageAndItsDottedChildren() {
        assertTrue(AppPrivate.isOwnAuthority(pkg, pkg))
        assertTrue(AppPrivate.isOwnAuthority("$pkg.documents", pkg))
        assertTrue(AppPrivate.isOwnAuthority("$pkg.androidx-startup", pkg))
        assertTrue(AppPrivate.isOwnAuthority("COM.KAHFLABS.WASL.documents", pkg))
        assertFalse(AppPrivate.isOwnAuthority("$pkg-evil", pkg))
        assertFalse(AppPrivate.isOwnAuthority("com.kahflabs.waslx", pkg))
        assertFalse(AppPrivate.isOwnAuthority("com.android.externalstorage.documents", pkg))
        assertFalse(AppPrivate.isOwnAuthority(null, pkg))
        assertFalse(AppPrivate.isOwnAuthority("", pkg))
        // A user id or port in front of the host still names this app's provider (review 2026-10).
        assertTrue(AppPrivate.isOwnAuthority("0@$pkg", pkg))
        assertTrue(AppPrivate.isOwnAuthority("10@$pkg.documents", pkg))
        assertTrue(AppPrivate.isOwnAuthority("$pkg:80", pkg))
        assertTrue(private("content://0@$pkg.documents/tree/x"))
    }

    @Test
    fun privateFoldersAndTheirAliasesAreRecognised() {
        assertTrue(private("/data/user/0/$pkg"))
        assertTrue(private("/data/user/0/$pkg/shared_prefs/"))
        assertTrue(private("/data/data/$pkg/databases"))
        assertTrue(private("/data/user/10/$pkg/files"))
        assertTrue(private("/data/user_de/0/$pkg"))
        assertTrue(private("/sdcard/Android/data/$pkg/files"))
        assertTrue(private("/storage/self/primary/Android/data/$pkg"))
        assertTrue(private("/storage/emulated/0/ANDROID/Data/$pkg/cache"))
        assertTrue(private("/storage/emulated/0/DCIM/../Android/data/$pkg"))
        assertTrue(private("/storage/emulated/0/./Android//data/$pkg/"))
        assertTrue(private("content://$pkg.documents/tree/root"))
        assertTrue(private("content://com.android.externalstorage.documents/tree/primary%3AAndroid%2Fdata%2F$pkg"))
    }

    @Test
    fun ordinaryFoldersAreNotPrivate() {
        assertFalse(private("/storage/emulated/0/DCIM/Camera"))
        assertFalse(private("/storage/emulated/0/Android/data/com.other.app"))
        assertFalse(private("/storage/emulated/0/Documents/$pkg"))          // a folder merely named like the app
        assertFalse(private("/storage/emulated/0/Android/data/$pkg-backup"))
        assertFalse(private("/data/user/0/com.kahflabs.waslx"))
        assertFalse(private("/data/user/0/$pkg/../com.other.app"))
        assertFalse(private("content://com.android.externalstorage.documents/tree/primary%3ADCIM"))
        assertFalse(private("content://com.google.android.apps.docs.storage/tree/abc"))
    }

    @Test
    fun normalizeDropsDotsAndEmptySegments() {
        assertEquals(listOf("a", "b", "d"), AppPrivate.normalize("/a/./b//c/../d/"))
        assertEquals(listOf("x"), AppPrivate.normalize("/../../x"))
        assertEquals(emptyList<String>(), AppPrivate.normalize("/"))
    }

    /** Automated review 2026-10: the same private folder through /proc, a mount prefix or a link. */
    @Test
    fun privateFoldersReachedThroughProcPrefixesOrLinksAreRefused() {
        assertTrue(private("/proc/self/root/data/user/0/$pkg/files"))
        assertTrue(private("/proc/self/cwd"))
        assertTrue(private("/mnt/expand/1234/data/data/$pkg"))
        assertTrue(private("/mnt/x/data/user/10/$pkg/databases"))
        val linked = AppPrivate.isPrivateLocalPath("/storage/emulated/0/Notes", roots, pkg) { "/data/user/0/$pkg/files" }
        assertTrue("a folder that resolves into private data", linked)
        val plain = AppPrivate.isPrivateLocalPath("/storage/emulated/0/Notes", roots, pkg) { "/storage/emulated/0/Notes" }
        assertFalse(plain)
    }
}
