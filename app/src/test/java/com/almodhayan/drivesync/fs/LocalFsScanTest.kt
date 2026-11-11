package com.almodhayan.drivesync.fs

import android.app.Application
import com.almodhayan.drivesync.core.AppError
import com.almodhayan.drivesync.core.PathFilter
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config
import java.io.File

/**
 * A folder the phone cannot list right now (storage hiccup, a restricted folder) is not an empty
 * folder: reading it as empty would delete its files on Drive.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35], application = Application::class)
class LocalFsScanTest {
    @get:Rule val tmp = TemporaryFolder()

    private val all = PathFilter(emptyList(), emptyList(), emptyList(), true)

    /** LocalFs whose listing of [failing] fails, as File.listFiles does on an I/O error. */
    private fun fsFailingOn(vararg failing: File) = object : LocalFs(RuntimeEnvironment.getApplication()) {
        override fun listDir(dir: File): Array<File>? = if (failing.any { it.path == dir.path }) null else super.listDir(dir)
    }

    @Test
    fun anUnreadableSubfolder_isReported_andTheRestIsScanned() = runTest {
        val root = tmp.newFolder("pair")
        val sub = File(root, "sub").apply { mkdirs(); File(this, "a.txt").writeText("a") }
        File(root, "b.txt").writeText("b")
        val unreadable = ArrayList<String>()

        val entries = fsFailingOn(sub).scan(root, all, unreadable)

        assertEquals(listOf("sub"), unreadable)
        assertEquals(setOf("sub", "b.txt"), entries.map { it.rel }.toSet())
    }

    @Test
    fun anUnreadablePairFolder_failsTheScan() = runTest {
        val root = tmp.newFolder("pair").apply { File(this, "b.txt").writeText("b") }

        try {
            fsFailingOn(root).scan(root, all, ArrayList())
            fail("an unreadable pair folder read as empty")
        } catch (e: AppError.Io) {
            assertTrue(e.message!!.contains(root.path))
        }
    }
}
