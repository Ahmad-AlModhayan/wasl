package com.almodhayan.drivesync.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test

/** "Android removed access" only when the folder grant is really gone (#127). */
class ErrorKindTest {
    private val tree = "content://com.android.externalstorage.documents/tree/primary%3AWaslTest%2FNotes"

    @Test fun aPickedFolderStillGrantedShowsTheRealCauseNotLostAccess() {
        assertEquals(ErrorKind.IO, ErrorKind.shown(ErrorKind.PERMISSION, tree) { it == tree })
    }

    @Test fun aPickedFolderWhoseGrantIsGoneAsksToGrantAccessAgain() {
        assertEquals(ErrorKind.PERMISSION, ErrorKind.shown(ErrorKind.PERMISSION, tree) { false })
    }

    @Test fun aFolderPathKeepsThePermissionFix() {
        var asked = false
        assertEquals(ErrorKind.PERMISSION, ErrorKind.shown(ErrorKind.PERMISSION, "/storage/emulated/0/Notes") { asked = true; true })
        assertFalse("no grant to look up for a path", asked)
    }

    @Test fun otherKindsAreUnchangedAndDontLookUpTheGrant() {
        var asked = false
        val granted: (String) -> Boolean = { asked = true; true }
        assertEquals(ErrorKind.NETWORK, ErrorKind.shown(ErrorKind.NETWORK, tree, granted))
        assertEquals(null, ErrorKind.shown(null, tree, granted))
        assertFalse(asked)
    }
}
