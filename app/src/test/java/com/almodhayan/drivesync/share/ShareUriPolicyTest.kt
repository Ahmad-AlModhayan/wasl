package com.almodhayan.drivesync.share

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** "Dirty Stream" (security review 2026-10): only another app's content:// streams are read. */
class ShareUriPolicyTest {
    private val pkg = "com.kahflabs.wasl"

    @Test
    fun anotherAppsContentIsAccepted() {
        assertTrue(ShareUriPolicy.accepts("content", "com.google.android.apps.photos.contentprovider", pkg))
        assertTrue(ShareUriPolicy.accepts("content", "media", pkg))
        assertTrue(ShareUriPolicy.accepts("CONTENT", "com.android.providers.downloads.documents", pkg))
    }

    @Test
    fun ownContentFilePathsAndOtherSchemesAreRefused() {
        assertFalse(ShareUriPolicy.accepts("content", pkg, pkg))
        assertFalse(ShareUriPolicy.accepts("content", "$pkg.documents", pkg))
        assertFalse(ShareUriPolicy.accepts("content", "$pkg.androidx-startup", pkg))
        assertFalse(ShareUriPolicy.accepts("content", null, pkg))
        assertFalse(ShareUriPolicy.accepts("content", "", pkg))
        assertFalse(ShareUriPolicy.accepts("file", null, pkg))
        assertFalse(ShareUriPolicy.accepts("file", "", pkg))
        assertFalse(ShareUriPolicy.accepts("http", "example.com", pkg))
        assertFalse(ShareUriPolicy.accepts(null, "media", pkg))
    }
}
