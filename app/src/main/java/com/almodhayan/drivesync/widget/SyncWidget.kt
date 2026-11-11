package com.almodhayan.drivesync.widget

import android.content.Context
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.unit.DpSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.glance.Button
import androidx.glance.GlanceId
import androidx.glance.GlanceModifier
import androidx.glance.GlanceTheme
import androidx.glance.LocalContext
import androidx.glance.LocalSize
import androidx.glance.action.ActionParameters
import androidx.glance.action.actionParametersOf
import androidx.glance.action.actionStartActivity
import androidx.glance.action.clickable
import androidx.glance.appwidget.GlanceAppWidget
import androidx.glance.appwidget.GlanceAppWidgetReceiver
import androidx.glance.appwidget.SizeMode
import androidx.glance.appwidget.action.ActionCallback
import androidx.glance.appwidget.action.actionRunCallback
import androidx.glance.appwidget.cornerRadius
import androidx.glance.appwidget.provideContent
import androidx.glance.background
import androidx.glance.layout.Alignment
import androidx.glance.layout.Column
import androidx.glance.layout.Row
import androidx.glance.layout.Spacer
import androidx.glance.layout.fillMaxSize
import androidx.glance.layout.fillMaxWidth
import androidx.glance.layout.padding
import androidx.glance.layout.width
import androidx.glance.text.FontWeight
import androidx.glance.text.Text
import androidx.glance.text.TextStyle
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.scheduler.SyncScheduler
import com.almodhayan.drivesync.sync.SyncStateHolder
import com.almodhayan.drivesync.ui.MainActivity
import dagger.hilt.EntryPoint
import dagger.hilt.InstallIn
import dagger.hilt.android.EntryPointAccessors
import dagger.hilt.components.SingletonComponent
import kotlinx.coroutines.flow.first

/** Hilt access for code that is instantiated by the system (widget, action callbacks). */
@EntryPoint
@InstallIn(SingletonComponent::class)
interface WidgetEntryPoint {
    fun db(): Db
    fun syncState(): SyncStateHolder
    fun scheduler(): SyncScheduler
}

internal fun widgetEntryPoint(context: Context): WidgetEntryPoint =
    EntryPointAccessors.fromApplication(context.applicationContext, WidgetEntryPoint::class.java)

/**
 * Home screen widget: title, one status line ("Syncing Camera 42%", "Last sync 14:32"), a problems
 * badge, and Sync now / Open buttons. The content observes live data while the Glance session is
 * open; [WidgetUpdater] refreshes it when sync state, pairs or conflicts change.
 */
class SyncWidget : GlanceAppWidget() {

    override val sizeMode: SizeMode = SizeMode.Responsive(setOf(COMPACT, FULL))

    override suspend fun provideGlance(context: Context, id: GlanceId) {
        val entry = widgetEntryPoint(context)
        val db = entry.db()
        val syncState = entry.syncState()
        val initialPairs = db.pairs().getAll()
        val initialConflicts = db.conflicts().observeOpenCount().first()
        provideContent {
            val pairs by db.pairs().observeAll().collectAsState(initialPairs)
            val running by syncState.running.collectAsState()
            val conflicts by db.conflicts().observeOpenCount().collectAsState(initialConflicts)
            val status = WidgetStatusBuilder.build(pairs, running, conflicts)
            GlanceTheme { WidgetContent(status) }
        }
    }

    companion object {
        val COMPACT = DpSize(180.dp, 60.dp)
        val FULL = DpSize(180.dp, 110.dp)
    }
}

class SyncWidgetReceiver : GlanceAppWidgetReceiver() {
    override val glanceAppWidget: GlanceAppWidget = SyncWidget()
}

/** "Sync now" button: one manual run of all active pairs. */
class SyncNowAction : ActionCallback {
    override suspend fun onAction(context: Context, glanceId: GlanceId, parameters: ActionParameters) {
        val entry = widgetEntryPoint(context)
        if (!entry.syncState().anyRunning) entry.scheduler().syncNow(null, "widget", manual = true)
        SyncWidget().update(context, glanceId)
    }
}

private val RouteKey = ActionParameters.Key<String>(MainActivity.EXTRA_ROUTE)

@Composable
private fun WidgetContent(status: WidgetStatus) {
    val compact = LocalSize.current.height < 100.dp
    Column(
        modifier = GlanceModifier.fillMaxSize()
            .background(GlanceTheme.colors.widgetBackground)
            .cornerRadius(16.dp)
            .padding(12.dp),
    ) {
        if (compact) {
            Row(modifier = GlanceModifier.fillMaxSize(), verticalAlignment = Alignment.CenterVertically) {
                Column(modifier = GlanceModifier.defaultWeight()) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        TitleText()
                        Spacer(GlanceModifier.width(8.dp))
                        ProblemsBadge(status)
                    }
                    StatusText(status)
                }
                Spacer(GlanceModifier.width(8.dp))
                SyncButton(status)
            }
        } else {
            Row(modifier = GlanceModifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                TitleText()
                Spacer(GlanceModifier.defaultWeight())
                ProblemsBadge(status)
            }
            StatusText(status, GlanceModifier.padding(top = 4.dp))
            Spacer(GlanceModifier.defaultWeight())
            Row(modifier = GlanceModifier.fillMaxWidth()) {
                SyncButton(status, GlanceModifier.defaultWeight())
                Spacer(GlanceModifier.width(8.dp))
                Button(text = LocalContext.current.getString(R.string.widget_open), onClick = actionStartActivity<MainActivity>(), modifier = GlanceModifier.defaultWeight())
            }
        }
    }
}

@Composable
private fun TitleText() {
    Text(
        LocalContext.current.getString(R.string.app_name),
        style = TextStyle(color = GlanceTheme.colors.onSurface, fontSize = 14.sp, fontWeight = FontWeight.Bold),
        maxLines = 1,
    )
}

@Composable
private fun StatusText(status: WidgetStatus, modifier: GlanceModifier = GlanceModifier) {
    Text(
        status.statusLine,
        modifier = modifier,
        style = TextStyle(color = GlanceTheme.colors.onSurfaceVariant, fontSize = 13.sp),
        maxLines = 2,
    )
}

@Composable
private fun ProblemsBadge(status: WidgetStatus) {
    val line = status.problemsLine ?: return
    Text(
        line,
        modifier = GlanceModifier.clickable(actionStartActivity<MainActivity>(actionParametersOf(RouteKey to "problems"))),
        style = TextStyle(color = GlanceTheme.colors.error, fontSize = 12.sp, fontWeight = FontWeight.Bold),
        maxLines = 1,
    )
}

@Composable
private fun SyncButton(status: WidgetStatus, modifier: GlanceModifier = GlanceModifier) {
    val context = LocalContext.current
    Button(
        text = context.getString(if (status.syncing) R.string.widget_syncing else R.string.common_sync_now),
        onClick = actionRunCallback<SyncNowAction>(),
        modifier = modifier,
        enabled = !status.syncing,
        maxLines = 1,
    )
}
