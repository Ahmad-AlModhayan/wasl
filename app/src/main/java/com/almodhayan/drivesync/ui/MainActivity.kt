@file:OptIn(androidx.compose.ui.ExperimentalComposeUiApi::class)

package com.almodhayan.drivesync.ui

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.testTagsAsResourceId
import androidx.compose.ui.semantics.semantics
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation.compose.currentBackStackEntryAsState
import androidx.navigation.compose.rememberNavController
import com.almodhayan.drivesync.data.prefs.AppSettings
import com.almodhayan.drivesync.data.prefs.SettingsRepository
import com.almodhayan.drivesync.ui.components.DsNavigationBar
import com.almodhayan.drivesync.ui.theme.DriveSyncTheme
import dagger.hilt.android.AndroidEntryPoint
import javax.inject.Inject

@AndroidEntryPoint
class MainActivity : ComponentActivity() {
    @Inject lateinit var settings: SettingsRepository

    private var pendingRoute by mutableStateOf<String?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        installSplashScreen()
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        pendingRoute = intent?.getStringExtra(EXTRA_ROUTE)?.takeIf(ExternalRoutes::allowed)
        setContent {
            val cfg by settings.settings.collectAsState(initial = null)
            val c = cfg ?: return@setContent
            DriveSyncTheme(theme = c.theme, dynamicColor = c.dynamicColor) { Root(c) }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        pendingRoute = intent.getStringExtra(EXTRA_ROUTE)?.takeIf(ExternalRoutes::allowed)
    }

    @OptIn(ExperimentalLayoutApi::class)
    @Composable
    private fun Root(cfg: AppSettings) {
        val controller = rememberNavController()
        val nav = remember(controller) { AppNav(controller) }
        val entry by controller.currentBackStackEntryAsState()
        val top = Routes.topLevel[entry?.destination?.route]
        val shell: ShellViewModel = hiltViewModel()
        val problems by shell.problemCount.collectAsStateWithLifecycle()
        Scaffold(
            // Expose testTags to adb/UiAutomator as resource ids for the release gate (black-box, release build).
            modifier = Modifier.semantics { testTagsAsResourceId = true },
            bottomBar = { if (top != null && cfg.onboardingDone) DsNavigationBar(current = top, problems = problems, onSelect = { nav.top(it) }) },
            contentWindowInsets = WindowInsets(0, 0, 0, 0),
            containerColor = MaterialTheme.colorScheme.surface,
        ) { p ->
            AppNavHost(nav, startDestination = if (cfg.onboardingDone) Routes.HOME else Routes.ONBOARDING, modifier = Modifier.padding(p).consumeWindowInsets(p))
        }
        val route = pendingRoute
        LaunchedEffect(route) {
            if (route != null && cfg.onboardingDone) { nav.route(route); pendingRoute = null }
        }
    }

    companion object { const val EXTRA_ROUTE = "route" }
}
