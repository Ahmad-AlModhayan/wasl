package com.almodhayan.drivesync.ondemand

import android.os.ProxyFileDescriptorCallback
import android.system.ErrnoException
import android.system.OsConstants
import android.util.Log
import com.kahflabs.wasl.ffi.OnDemandFile
import com.kahflabs.wasl.ffi.OnDemandOrigin
import com.kahflabs.wasl.ffi.OnDemandReaderInterface
import com.kahflabs.wasl.ffi.WaslException
import kotlinx.coroutines.runBlocking

private const val TAG = "OnDemand"

/**
 * Serves one file to a read-only proxy file descriptor (`StorageManager.openProxyFileDescriptor`)
 * through the engine's on-demand reader: every read asks [OnDemandReader.read] for the 1 MiB
 * block(s) it falls in ([BlockWindow]), and the engine answers from its block cache or fetches only
 * those blocks (Drive range requests; a vault v1 chunk; the vault v2 chunks covering them), with
 * read-ahead once a reader goes through the file in order. A seek costs one block, never the file.
 *
 * A vault file keeps no window: every read goes to the engine, so each one passes the vault's lock
 * check and no decrypted bytes stay in the app after a lock (the engine drops its own on lock).
 *
 * Writes are refused (EBADF): the descriptor is opened read-only, and edits of plain Drive files go
 * through the provider's write sessions instead. Callbacks run on the provider's own HandlerThread
 * for this file, so blocking on the engine here blocks no one else.
 */
class EngineReadCallback(
    private val reader: OnDemandReaderInterface,
    private val origin: OnDemandOrigin,
    private val file: OnDemandFile,
    private val onReleased: () -> Unit,
) : ProxyFileDescriptorCallback() {
    private val size = file.size.toLong()
    private val window: BlockWindow? = if (origin is OnDemandOrigin.Vault) {
        null
    } else {
        BlockWindow(size) { idx ->
            runBlocking {
                reader.read(origin, file, BlockMath.blockOffset(idx).toULong(), BlockMath.blockLength(idx, size).toULong())
            }
        }
    }

    @Volatile private var released = false

    override fun onGetSize(): Long = size

    override fun onRead(offset: Long, size: Int, data: ByteArray): Int {
        if (released) throw ErrnoException("onRead", OsConstants.EBADF)
        if (offset < 0) throw ErrnoException("onRead", OsConstants.EINVAL)
        return try {
            window?.read(offset, data, size) ?: readThrough(offset, data, size)
        } catch (t: Throwable) {
            throw errno(t)
        }
    }

    /** Exactly the bytes asked for, straight from the engine (vault files). */
    private fun readThrough(offset: Long, data: ByteArray, length: Int): Int {
        if (offset >= size || length <= 0) return 0
        val want = minOf(length.toLong(), size - offset)
        val bytes = runBlocking { reader.read(origin, file, offset.toULong(), want.toULong()) }
        bytes.copyInto(data, 0, 0, minOf(bytes.size, length))
        return minOf(bytes.size, length)
    }

    override fun onWrite(offset: Long, size: Int, data: ByteArray): Int = throw ErrnoException("onWrite", OsConstants.EBADF)

    override fun onFsync() = Unit

    override fun onRelease() {
        released = true
        window?.clear()
        onReleased()
    }

    private fun errno(t: Throwable): ErrnoException {
        if (t is ErrnoException) return t
        val code = when (t) {
            is WaslException.NotFound -> OsConstants.ENOENT
            is WaslException.Auth, is WaslException.Permission, is WaslException.Locked -> OsConstants.EACCES
            is InterruptedException -> OsConstants.EINTR
            else -> OsConstants.EIO
        }
        Log.w(TAG, failureLine(file, t))
        return ErrnoException("onRead", code)
    }

    companion object {
        /**
         * The log line for a failed read. Logcat is readable through adb and bug reports, so it names
         * the file only by an opaque tail of its Drive id and the error only by its class: the
         * engine's detail text can carry the file's name (S1-17, security review L4).
         */
        internal fun failureLine(file: OnDemandFile, t: Throwable): String =
            "file …${file.id.takeLast(6)}: read failed: ${t.javaClass.simpleName}"
    }
}
