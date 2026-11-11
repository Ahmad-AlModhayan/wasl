package com.almodhayan.drivesync.ui.components

import android.app.Application
import androidx.activity.ComponentActivity
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.hasClickAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import com.almodhayan.drivesync.ui.theme.DriveSyncTheme
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * #149: compact action buttons and tags must grow with the text at the largest font size (font scale 2),
 * not keep a fixed 32dp / 20dp height that cuts the label off.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35], application = Application::class, qualifiers = "w360dp-h900dp-xxhdpi")
class LargeFontButtonsTest {
    @get:Rule val compose = createAndroidComposeRule<ComponentActivity>()

    private fun big(content: @Composable () -> Unit) = compose.setContent {
        val d = LocalDensity.current
        CompositionLocalProvider(LocalDensity provides Density(d.density, fontScale = 2f)) {
            DriveSyncTheme(theme = "light", dynamicColor = false) { content() }
        }
    }

    private fun buttonHeight(label: String) = compose.onNode(hasClickAction() and hasText(label)).fetchSemanticsNode().let { node ->
        with(compose.density) { node.size.height.toDp() }
    }

    @Test fun healthCheckButtonGrowsWithText() {
        big { HealthCheckRow(HealthState.Fail, "Account", "No Google account", actionLabel = "Add account", onAction = {}) }
        val h = buttonHeight("Add account")
        assertTrue("button is $h, clipped at a fixed height", h > 40.dp)
    }

    @Test fun conflictButtonsGrowWithText() {
        big { ConflictCard("a.txt", "Notes/a.txt", ConflictSideInfo("now", "1 KB"), ConflictSideInfo("now", "2 KB"), onChoose = {}) }
        val h = buttonHeight("Keep both")
        assertTrue("button is $h, clipped at a fixed height", h > 40.dp)
    }

    @Test fun tagGrowsWithText() {
        big { Tag("Current") }
        val h = with(compose.density) { compose.onNodeWithText("Current").fetchSemanticsNode().size.height.toDp() }
        assertTrue("tag is $h, clipped at a fixed height", h > 20.dp)
    }
}
