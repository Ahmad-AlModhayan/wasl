package com.almodhayan.drivesync.ui.onboarding

import android.Manifest
import android.app.Activity
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.os.PowerManager
import android.provider.Settings
import androidx.annotation.StringRes
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings

/**
 * Snapshot of the device-level prerequisites the app depends on. Cheap to compute, so screens
 * recompute it on every resume (see [rememberResumeTick]).
 */
data class DeviceStatus(
    val allFilesAccess: Boolean,
    val notifications: Boolean,
    val batteryUnrestricted: Boolean,
    val fineLocation: Boolean,
)

/** Permission / system-state checks and the intents that fix them. Shared by onboarding, home and diagnostics. */
object DeviceChecks {

    fun hasAllFilesAccess(): Boolean = Environment.isExternalStorageManager()

    /** POST_NOTIFICATIONS is a runtime permission from Android 13; below that only the app-level toggle matters. */
    fun canPostNotifications(context: Context): Boolean =
        if (needsNotificationRuntimePermission()) {
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
        } else {
            NotificationManagerCompat.from(context).areNotificationsEnabled()
        }

    fun needsNotificationRuntimePermission(): Boolean = Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU

    fun ignoresBatteryOptimizations(context: Context): Boolean =
        context.getSystemService(PowerManager::class.java)?.isIgnoringBatteryOptimizations(context.packageName) == true

    fun hasFineLocation(context: Context): Boolean =
        ContextCompat.checkSelfPermission(context, Manifest.permission.ACCESS_FINE_LOCATION) == PackageManager.PERMISSION_GRANTED

    fun status(context: Context): DeviceStatus = DeviceStatus(
        allFilesAccess = hasAllFilesAccess(),
        notifications = canPostNotifications(context),
        batteryUnrestricted = ignoresBatteryOptimizations(context),
        fineLocation = hasFineLocation(context),
    )

    private fun packageUri(context: Context): Uri = Uri.parse("package:${context.packageName}")

    /** Opens the system page where the user grants "All files access" for this app. */
    fun openAllFilesAccess(context: Context): Boolean =
        context.safeStart(Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, packageUri(context))) ||
            context.safeStart(Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION)) ||
            openAppDetails(context)

    /** Shows the system dialog that exempts the app from battery optimization. */
    fun requestIgnoreBatteryOptimizations(context: Context): Boolean =
        context.safeStart(Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, packageUri(context))) ||
            context.safeStart(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)) ||
            openAppDetails(context)

    fun openNotificationSettings(context: Context): Boolean =
        context.safeStart(Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName)) ||
            openAppDetails(context)

    fun openAppDetails(context: Context): Boolean =
        context.safeStart(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, packageUri(context)))

    fun openUrl(context: Context, url: String): Boolean = context.safeStart(Intent(Intent.ACTION_VIEW, Uri.parse(url)))
}

/** Starts [intent] and returns false instead of crashing when nothing handles it (vendor screens, missing browser, exported=false). */
fun Context.safeStart(intent: Intent): Boolean = try {
    if (this !is Activity) intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    startActivity(intent)
    true
} catch (e: Exception) {
    false
}

/** A manufacturer whose Android skin kills background apps unless the user whitelists them. */
data class VendorInfo(
    /** Also the path on dontkillmyapp.com. */
    val key: String,
    val label: String,
    @StringRes val guidanceRes: Int,
    /** "package/class" of known autostart / background-manager screens, tried in order. */
    val components: List<String>,
) {
    val guidance: String get() = Strings.get(guidanceRes)
}

object VendorGuide {
    private val vendors = listOf(
        VendorInfo(
            "xiaomi", "Xiaomi / Redmi / POCO",
            R.string.onboarding_vendor_xiaomi,
            listOf(
                "com.miui.securitycenter/com.miui.permcenter.autostart.AutoStartManagementActivity",
                "com.miui.securitycenter/com.miui.powercenter.PowerSettings",
            ),
        ),
        VendorInfo(
            "huawei", "Huawei / Honor",
            R.string.onboarding_vendor_huawei,
            listOf(
                "com.huawei.systemmanager/.startupmgr.ui.StartupNormalAppListActivity",
                "com.huawei.systemmanager/.appcontrol.activity.StartupAppControlActivity",
                "com.huawei.systemmanager/.optimize.process.ProtectActivity",
            ),
        ),
        VendorInfo(
            "samsung", "Samsung",
            R.string.onboarding_vendor_samsung,
            listOf(
                "com.samsung.android.lool/com.samsung.android.sm.battery.ui.BatteryActivity",
                "com.samsung.android.lool/com.samsung.android.sm.ui.battery.BatteryActivity",
                "com.samsung.android.sm/.ui.battery.BatteryActivity",
            ),
        ),
        VendorInfo(
            "oppo", "OPPO / Realme",
            R.string.onboarding_vendor_oppo,
            listOf(
                "com.coloros.safecenter/.permission.startup.StartupAppListActivity",
                "com.coloros.safecenter/.startupapp.StartupAppListActivity",
                "com.oppo.safe/.permission.startup.StartupAppListActivity",
                "com.coloros.oppoguardelf/com.coloros.powermanager.fuelgaue.PowerUsageModelActivity",
            ),
        ),
        VendorInfo(
            "vivo", "vivo / iQOO",
            R.string.onboarding_vendor_vivo,
            listOf(
                "com.vivo.permissionmanager/.activity.BgStartUpManagerActivity",
                "com.iqoo.secure/.ui.phoneoptimize.BgStartUpManager",
                "com.iqoo.secure/.ui.phoneoptimize.AddWhiteListActivity",
            ),
        ),
        VendorInfo(
            "oneplus", "OnePlus",
            R.string.onboarding_vendor_oneplus,
            listOf(
                "com.oneplus.security/.chainlaunch.view.ChainLaunchAppListActivity",
                "com.android.settings/.Settings\$BgOptimizeAppListActivity",
            ),
        ),
        VendorInfo(
            "asus", "ASUS",
            R.string.onboarding_vendor_asus,
            listOf(
                "com.asus.mobilemanager/.powersaver.PowerSaverSettings",
                "com.asus.mobilemanager/.autostart.AutoStartActivity",
                "com.asus.mobilemanager/com.asus.mobilemanager.entry.FunctionActivity",
            ),
        ),
        VendorInfo(
            "lenovo", "Lenovo / Motorola",
            R.string.onboarding_vendor_lenovo,
            listOf("com.lenovo.security/.purebackground.view.PureBackgroundActivity"),
        ),
        VendorInfo(
            "nothing", "Nothing",
            R.string.onboarding_vendor_nothing,
            emptyList(),
        ),
    )

    private val aliases = mapOf(
        "redmi" to "xiaomi", "poco" to "xiaomi", "pocophone" to "xiaomi",
        "honor" to "huawei",
        "realme" to "oppo",
        "iqoo" to "vivo",
        "motorola" to "lenovo", "moto" to "lenovo",
    )

    /** Vendor of this device, or null for stock-like Android (Google, Sony, Fairphone, …). */
    fun current(): VendorInfo? {
        val names = listOf(Build.MANUFACTURER ?: "", Build.BRAND ?: "").map { it.lowercase() }
        for (n in names) {
            if (n.isBlank()) continue
            val direct = vendors.firstOrNull { n.contains(it.key) }
            if (direct != null) return direct
            val alias = aliases.entries.firstOrNull { n.contains(it.key) }?.value
            if (alias != null) return vendors.first { it.key == alias }
        }
        return null
    }

    fun manufacturerLabel(): String = listOf(Build.MANUFACTURER ?: "", Build.MODEL ?: "").filter { it.isNotBlank() }.joinToString(" ")

    fun dontKillMyAppUrl(vendor: VendorInfo? = current()): String = "https://dontkillmyapp.com/" + (vendor?.key ?: "general")

    /**
     * Tries the vendor's known autostart / background-manager screens (each wrapped in try/catch) and
     * falls back to the app's system settings page. Returns true when a vendor screen opened.
     */
    fun openAutostartSettings(context: Context): Boolean {
        val vendor = current()
        if (vendor != null) {
            for (flat in vendor.components) {
                val cn = ComponentName.unflattenFromString(flat) ?: continue
                if (context.safeStart(Intent().setComponent(cn))) return true
            }
        }
        DeviceChecks.openAppDetails(context)
        return false
    }
}

/** Increments every time the host lifecycle resumes, so callers can re-run permission checks with `remember(tick)` / `LaunchedEffect(tick)`. */
@Composable
fun rememberResumeTick(): Int {
    val owner = LocalLifecycleOwner.current
    var tick by remember { mutableIntStateOf(0) }
    DisposableEffect(owner) {
        val observer = LifecycleEventObserver { _, event -> if (event == Lifecycle.Event.ON_RESUME) tick++ }
        owner.lifecycle.addObserver(observer)
        onDispose { owner.lifecycle.removeObserver(observer) }
    }
    return tick
}
