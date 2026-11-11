package com.almodhayan.drivesync.sync

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.wifi.WifiManager
import android.os.BatteryManager
import android.telephony.TelephonyManager
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.db.FolderPairEntity
import dagger.hilt.android.qualifiers.ApplicationContext
import javax.inject.Inject
import javax.inject.Singleton

/** Evaluates a pair's run conditions against the device state. Returns null when the pair may run. */
@Singleton
class Conditions @Inject constructor(@ApplicationContext private val context: Context) {

    data class Device(val online: Boolean, val wifi: Boolean, val metered: Boolean, val vpn: Boolean, val roaming: Boolean, val ssid: String?, val charging: Boolean, val battery: Int)

    fun device(): Device {
        val cm = context.getSystemService(ConnectivityManager::class.java)
        val caps = cm.activeNetwork?.let { cm.getNetworkCapabilities(it) }
        val online = caps?.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) == true
        val wifi = caps?.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) == true || caps?.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) == true
        val metered = caps?.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED) == false
        val vpn = caps?.hasTransport(NetworkCapabilities.TRANSPORT_VPN) == true
        val roaming = caps?.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_ROAMING) == false &&
            (context.getSystemService(TelephonyManager::class.java)?.isNetworkRoaming == true)
        val ssid = if (wifi) currentSsid() else null
        val bm = context.getSystemService(BatteryManager::class.java)
        val battery = bm?.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY) ?: 100
        val charging = bm?.isCharging == true
        return Device(online, wifi, metered, vpn, roaming, ssid, charging, battery)
    }

    @Suppress("DEPRECATION")
    private fun currentSsid(): String? = runCatching {
        val wm = context.applicationContext.getSystemService(WifiManager::class.java)
        wm?.connectionInfo?.ssid?.trim('"')?.takeIf { it.isNotEmpty() && it != "<unknown ssid>" }
    }.getOrNull()

    fun blockReason(pair: FolderPairEntity, manual: Boolean = false): String? {
        val d = device()
        if (!d.online) return Blockers.text(Blockers.Kind.OFFLINE)
        if (manual) return null   // manual sync ignores the pair's power/network preferences
        if (pair.wifiOnly && !d.wifi) return Blockers.text(Blockers.Kind.WIFI)
        if (!pair.allowMetered && d.metered && !d.wifi) return Blockers.text(Blockers.Kind.METERED)
        if (!pair.allowRoaming && d.roaming) return Blockers.text(Blockers.Kind.ROAMING)
        if (pair.requireVpn && !d.vpn) return Blockers.text(Blockers.Kind.VPN)
        if (pair.requireCharging && !d.charging) return Blockers.text(Blockers.Kind.CHARGER)
        if (!d.charging && d.battery < pair.minBatteryPercent) return Blockers.text(Blockers.Kind.BATTERY, pair.minBatteryPercent)
        val allow = pair.ssidAllowList(); val deny = pair.ssidDenyList()
        if (d.wifi && (allow.isNotEmpty() || deny.isNotEmpty())) {
            val ssid = d.ssid
            if (allow.isNotEmpty() && (ssid == null || allow.none { it.equals(ssid, true) })) return Blockers.text(Blockers.Kind.SSID_NOT_ALLOWED)
            if (ssid != null && deny.any { it.equals(ssid, true) }) return Blockers.text(Blockers.Kind.SSID_DENIED)
        }
        return null
    }
}

/**
 * The words for a condition that holds a pair back. The text is stored as the pair's last message,
 * so [kindOf] reads it back: in the app's language, or in English from older versions and the engine.
 */
object Blockers {
    enum class Kind { OFFLINE, WIFI, METERED, ROAMING, VPN, CHARGER, BATTERY, SSID_NOT_ALLOWED, SSID_DENIED }

    fun text(kind: Kind, percent: Int = 0): String = when (kind) {
        Kind.OFFLINE -> Strings.get(R.string.sync_blocker_offline)
        Kind.WIFI -> Strings.get(R.string.sync_blocker_wifi)
        Kind.METERED -> Strings.get(R.string.sync_blocker_metered)
        Kind.ROAMING -> Strings.get(R.string.sync_blocker_roaming)
        Kind.VPN -> Strings.get(R.string.sync_blocker_vpn)
        Kind.CHARGER -> Strings.get(R.string.sync_blocker_charger)
        Kind.BATTERY -> Strings.get(R.string.sync_blocker_battery_below, percent)
        Kind.SSID_NOT_ALLOWED -> Strings.get(R.string.sync_blocker_ssid_not_allowed)
        Kind.SSID_DENIED -> Strings.get(R.string.sync_blocker_ssid_denied)
    }

    fun kindOf(reason: String): Kind? {
        if (reason.isBlank()) return null
        val percent = Regex("\\p{Nd}+").find(reason)?.value?.toIntOrNull() ?: 0
        Kind.entries.firstOrNull { text(it, percent) == reason }?.let { return it }
        return when {
            reason.contains("charger", true) -> Kind.CHARGER
            reason.contains("battery", true) -> Kind.BATTERY
            reason.contains("VPN", true) -> Kind.VPN
            reason.contains("roaming", true) -> Kind.ROAMING
            reason.contains("No internet", true) -> Kind.OFFLINE
            reason.contains("allow list", true) -> Kind.SSID_NOT_ALLOWED
            reason.contains("deny list", true) -> Kind.SSID_DENIED
            reason.contains("Metered", true) -> Kind.METERED
            else -> null
        }
    }
}
