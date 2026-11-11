package com.almodhayan.drivesync.core

import android.content.Context
import androidx.annotation.PluralsRes
import androidx.annotation.StringRes

/**
 * String resources for code that has no Compose or Activity context: view models, notifications,
 * formatters and the engine glue. It reads the application's resources, which follow the app's
 * language (Android 13+ per-app language included). Compose code uses stringResource instead.
 *
 * Before [init] (plain JVM unit tests) it asks `TestStrings` from the test sources, which reads the
 * English res/values files, so tests see the real English text. Without it, it returns "#id".
 */
object Strings {
    @Volatile private var context: Context? = null

    fun init(app: Context) { context = app.applicationContext }

    fun get(@StringRes id: Int, vararg args: Any): String {
        val c = context ?: return fallback?.string(id, args) ?: "#$id"
        return if (args.isEmpty()) c.getString(id) else c.getString(id, *args)
    }

    fun plural(@PluralsRes id: Int, count: Int, vararg args: Any): String {
        val a: Array<out Any> = if (args.isEmpty()) arrayOf<Any>(count) else args
        val c = context ?: return fallback?.plural(id, count, a) ?: "#$id:$count"
        return c.resources.getQuantityString(id, count, *a)
    }

    /** Resolves strings without Android; only the unit tests provide one. */
    interface Fallback {
        fun string(id: Int, args: Array<out Any>): String
        fun plural(id: Int, count: Int, args: Array<out Any>): String
    }

    private val fallback: Fallback? by lazy {
        runCatching { Class.forName("com.almodhayan.drivesync.core.TestStrings").getField("INSTANCE").get(null) as Fallback }.getOrNull()
    }
}
