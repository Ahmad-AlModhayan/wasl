package com.almodhayan.drivesync.engine

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.FileNotFoundException

/** #154: a document moved or deleted since the scan is recognised as gone, not as a fault. */
class SafGoneTest {
    @Test fun theExternalStorageProvidersChildCheckReadsAsGone() {
        // As Android's ExternalStorageProvider words it (the owner's LifeOS pair, 2026-11-03).
        val e = IllegalArgumentException(
            "Failed to determine if primary:LifeOS/x/combined_statements.xlsx is child of primary:LifeOS: " +
                "java.io.FileNotFoundException: Missing file for primary:LifeOS/x/combined_statements.xlsx at /storage/emulated/0/LifeOS/x/combined_statements.xlsx",
        )
        assertTrue(SafBridgeImpl.isGone(e))
        assertTrue(SafBridgeImpl.isGone(IllegalArgumentException("bad", FileNotFoundException("x"))))
        assertTrue(SafBridgeImpl.isGone(FileNotFoundException("x")))
    }

    @Test fun otherFailuresAreNotGone() {
        assertFalse(SafBridgeImpl.isGone(IllegalArgumentException("Invalid URI")))
        assertFalse(SafBridgeImpl.isGone(SecurityException("Permission Denial")))
        assertFalse(SafBridgeImpl.isGone(java.io.IOException("EIO")))
    }
}
