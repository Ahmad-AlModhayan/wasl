package com.almodhayan.drivesync.ui

/**
 * MainActivity is exported (it is the launcher entry), so any app can start it with a route extra.
 * Only the routes the app's own notifications and widget send are honoured, and ids must look like
 * ids: "pair:x/edit" would otherwise navigate to the pair editor.
 */
object ExternalRoutes {
    private val ID = Regex("[A-Za-z0-9-]{1,64}")

    fun allowed(route: String?): Boolean = when {
        route == null -> false
        route == "home" || route == "problems" || route == "devices" -> true
        route.startsWith("pair:") -> ID.matches(route.removePrefix("pair:"))
        route.startsWith("review:") -> ID.matches(route.removePrefix("review:"))
        else -> false
    }
}
