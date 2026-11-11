package com.almodhayan.drivesync.ui.components

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class SafPathTest {
    private val tree = "content://com.android.externalstorage.documents/tree/"

    @Test fun `primary storage maps to the emulated path`() {
        assertEquals("/storage/emulated/0/DCIM/Camera", Fmt.safFilePath(tree + "primary%3ADCIM%2FCamera"))
        assertEquals("/storage/emulated/0", Fmt.safFilePath(tree + "primary%3A"))
        assertEquals("DCIM / Camera", Fmt.humanLocalPath(tree + "primary%3ADCIM%2FCamera"))
    }

    @Test fun `SD cards, spaces, plus signs and Arabic decode`() {
        assertEquals("/storage/1A2B-3C4D/My Files/C++", Fmt.safFilePath(tree + "1A2B-3C4D%3AMy%20Files%2FC%2B%2B"))
        assertEquals("/storage/emulated/0/مستندات", Fmt.safFilePath(tree + "primary%3A%D9%85%D8%B3%D8%AA%D9%86%D8%AF%D8%A7%D8%AA"))
        assertEquals("SD card / My Files", Fmt.humanLocalPath(tree + "1A2B-3C4D%3AMy%20Files"))
    }

    @Test fun `other providers have no file path`() {
        assertNull(Fmt.safFilePath("content://com.example.cloud/tree/abc"))
        assertNull(Fmt.safFilePath("/storage/emulated/0/DCIM"))
    }
}
