package com.almodhayan.drivesync.fs

import android.app.Application
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config
import java.io.File

/** Local removals made by a sync land in the recycle bin, never a hard delete. */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35], application = Application::class)
class LocalFsTrashTest {
    @get:Rule val tmp = TemporaryFolder()

    private val fs = LocalFs(RuntimeEnvironment.getApplication())
    private val bytes = "keep me ".repeat(100).toByteArray()

    @Test
    fun moveToTrash_keepsBytesAndRemovesOriginal() = runTest {
        val original = File(tmp.newFolder("pair"), "report.pdf").apply { writeBytes(bytes) }

        val inTrash = fs.moveToTrash("pair-1", original)

        assertFalse("original should be gone from the synced folder", original.exists())
        assertTrue("trash copy should sit under the recycle bin", inTrash.canonicalPath.startsWith(fs.trashRoot().canonicalPath))
        assertArrayEquals(bytes, inTrash.readBytes())
    }

    @Test
    fun restore_putsTheFileBack() = runTest {
        val original = File(tmp.newFolder("pair"), "nested/report.pdf").apply { parentFile!!.mkdirs(); writeBytes(bytes) }
        val inTrash = fs.moveToTrash("pair-1", original)
        original.parentFile!!.deleteRecursively()   // the folder can be gone by the time the user restores

        assertTrue(fs.restoreFromTrash(inTrash, original))

        assertArrayEquals(bytes, original.readBytes())
        assertFalse("restored file should leave the recycle bin", inTrash.exists())
    }
}
