package com.almodhayan.drivesync.ui.components

import android.app.Activity
import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.content.ContextWrapper
import android.os.Build
import android.os.PersistableBundle
import android.view.Window
import android.view.WindowManager
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.window.DialogWindowProvider
import java.util.WeakHashMap

/**
 * Keeps screenshots, screen recording and the recents thumbnail away from secrets (passphrases,
 * vault.json, the automation secret, pairing codes) while this is in the composition (security
 * review 2026-10). Covers the activity window and, inside a dialog, the dialog's own window too:
 * a dialog is a separate window and FLAG_SECURE does not carry over to it.
 */
@Composable
fun SecureContent() {
    val view = LocalView.current
    DisposableEffect(view) {
        val windows = listOfNotNull(view.context.findActivity()?.window, (view.parent as? DialogWindowProvider)?.window).distinct()
        windows.forEach { SecureWindows.acquire(it) }
        onDispose { windows.forEach { SecureWindows.release(it) } }
    }
}

/**
 * Counts the [SecureContent] uses per window, so a dialog closing over a secure screen does not
 * clear the flag the screen still needs. Main thread only, like all composition effects.
 */
private object SecureWindows {
    private val counts = WeakHashMap<Window, Int>()

    fun acquire(w: Window) {
        val n = counts[w] ?: 0
        if (n == 0) w.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
        counts[w] = n + 1
    }

    fun release(w: Window) {
        val n = (counts[w] ?: return) - 1
        if (n <= 0) {
            counts.remove(w)
            w.clearFlags(WindowManager.LayoutParams.FLAG_SECURE)
        } else {
            counts[w] = n
        }
    }
}

private tailrec fun Context.findActivity(): Activity? = when (this) {
    is Activity -> this
    is ContextWrapper -> baseContext.findActivity()
    else -> null
}

/**
 * Copies a secret (automation secret, vault.json, share or pairing code) marked sensitive: from
 * Android 13 the system copy preview and keyboard suggestions hide it (security review 2026-10).
 */
fun copySensitiveText(context: Context, label: String, text: String) {
    val clip = ClipData.newPlainText(label, text)
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
        clip.description.extras = PersistableBundle().apply { putBoolean(ClipDescription.EXTRA_IS_SENSITIVE, true) }
    }
    context.getSystemService(ClipboardManager::class.java)?.setPrimaryClip(clip)
}
