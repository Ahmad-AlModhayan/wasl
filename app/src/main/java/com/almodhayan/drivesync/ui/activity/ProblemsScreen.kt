@file:OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)

package com.almodhayan.drivesync.ui.activity

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.actions.PairActions
import com.almodhayan.drivesync.core.ErrorKind
import com.almodhayan.drivesync.core.Strings
import com.almodhayan.drivesync.data.db.AccountEntity
import com.almodhayan.drivesync.data.Db
import com.almodhayan.drivesync.data.db.ConflictEntity
import com.almodhayan.drivesync.data.db.FolderPairEntity
import com.almodhayan.drivesync.data.db.SyncEventEntity
import com.almodhayan.drivesync.sync.PlanSummary
import com.almodhayan.drivesync.sync.StatusText
import com.almodhayan.drivesync.ui.AppNav
import com.almodhayan.drivesync.ui.common.AppScreen
import com.almodhayan.drivesync.ui.common.LoadingState
import com.almodhayan.drivesync.ui.components.ChipSize
import com.almodhayan.drivesync.ui.components.Fmt
import com.almodhayan.drivesync.ui.components.LogLine
import com.almodhayan.drivesync.ui.components.LogLineRow
import com.almodhayan.drivesync.ui.components.MenuAction
import com.almodhayan.drivesync.ui.components.OverflowMenu
import com.almodhayan.drivesync.ui.components.SectionHeader
import com.almodhayan.drivesync.ui.components.StatusChip
import com.almodhayan.drivesync.ui.components.StatusIcon
import com.almodhayan.drivesync.ui.components.showMessage
import com.almodhayan.drivesync.ui.status.PairStatusResolver
import com.almodhayan.drivesync.ui.status.PairStatusUi
import com.almodhayan.drivesync.ui.theme.Ds
import com.almodhayan.drivesync.ui.theme.Size
import com.almodhayan.drivesync.ui.theme.Space
import com.almodhayan.drivesync.ui.theme.Sym
import com.almodhayan.drivesync.ui.theme.SyncStatus
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import javax.inject.Inject

/**
 * Problems (Screen06Problems): one list grouped by who acts. "Needs you" holds every loud item
 * (approvals, failures, conflicts, in the status precedence) with its single action; "Retrying on its
 * own" is listed but never badged.
 */
@HiltViewModel
class ProblemsViewModel @Inject constructor(
    private val db: Db,
    private val pairActions: PairActions,
    private val json: Json,
) : ViewModel() {

    /** Where a row or its button leads. */
    sealed interface Step {
        data class ReviewDeletions(val pairId: String) : Step
        data class ReviewFirstSync(val pairId: String) : Step
        data class OpenConflict(val conflictId: String) : Step
        data class OpenPair(val pairId: String) : Step
        data class ChooseFolder(val pairId: String) : Step
        data class Retry(val pairId: String, val pairName: String) : Step
        data class Resume(val pairId: String, val pairName: String) : Step
        data object SignIn : Step
        data object GrantAccess : Step
        data object FreeSpace : Step
        data object UnlockVault : Step
    }

    @Immutable
    data class Item(
        val key: String,
        val status: SyncStatus,
        val chipLabel: String,
        val title: String,
        val text: String?,
        val icon: ImageVector? = null,
        val actionLabel: String? = null,
        val action: Step? = null,
        val tonal: Boolean = false,
        val secondaryLabel: String? = null,
        val secondary: Step? = null,
        val open: Step? = null,
    )

    data class Warning(val id: Long, val runId: String, val line: LogLine)

    data class Ui(val needsYou: List<Item>, val retrying: List<Item>, val warnings: List<Warning>)

    val ui: StateFlow<Ui?> = combine(
        db.pairs().observeAll(),
        db.conflicts().observeOpen(),
        db.accounts().observeAll(),
        db.events().observeProblems(WARNING_LIMIT),
    ) { pairs, conflicts, accounts, events -> build(pairs, conflicts, accounts, events) }
        .flowOn(Dispatchers.IO)
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    fun retry(pairId: String) {
        pairActions.syncNow(pairId)
    }

    fun resume(pairId: String) {
        viewModelScope.launch {
            try {
                withContext(Dispatchers.IO) { pairActions.resume(pairId) }
            } catch (e: CancellationException) {
                throw e
            } catch (t: Throwable) {
                // The pair stays as it was; its row still offers the fix.
            }
        }
    }

    private suspend fun build(pairs: List<FolderPairEntity>, conflicts: List<ConflictEntity>, accounts: List<AccountEntity>, events: List<SyncEventEntity>): Ui {
        val approvals = ArrayList<Item>()
        val failures = ArrayList<Item>()
        val retrying = ArrayList<Item>()
        val signInCovered = HashSet<String>()
        for (p in pairs) {
            val blockedRow = if (p.pendingApproval) db.blockedPlans().get(p.id) else null
            val summary = blockedRow?.let { decode(it.summaryJson) }
            val st = PairStatusResolver.resolve(p, null, 0, summary)
            when {
                st.status == SyncStatus.Approval -> approvals += approvalItem(p, summary, blockedRow?.createdAt)
                st.status == SyncStatus.Failed -> {
                    failures += failedItem(p, st)
                    if (st.errorKind == ErrorKind.AUTH) signInCovered += p.accountId
                }
                st.retrying -> retrying += retryItem(p, st)
            }
        }
        for (a in accounts) if (a.authError != null && a.id !in signInCovered) failures += accountItem(a)
        val names = pairs.associate { it.id to it.name }
        val conflictItems = conflicts.map { conflictItem(it, names[it.pairId]) }
        val since = System.currentTimeMillis() - WARNING_DAYS * 86_400_000L
        val warnings = events.filter { it.at >= since }.map { e ->
            val line = logLineOf(e, logTime(e.at))
            Warning(e.id, e.runId, names[e.pairId]?.let { n -> line.copy(message = "$n · ${line.message}") } ?: line)
        }
        // Precedence: needs approval → failed → conflict.
        return Ui(approvals + failures + conflictItems, retrying, warnings)
    }

    private fun approvalItem(p: FolderPairEntity, s: PlanSummary?, at: Long?): Item {
        val kind = s?.blockedKind
        val n = s?.blockedCount ?: 0
        val title = when (kind) {
            "deletions", "wipe" -> Strings.plural(R.plurals.activity_problems_deletions_waiting, n, Fmt.count(n), p.name)
            "first-merge" -> Strings.get(R.string.activity_problems_first_sync_ok, p.name)
            else -> "${StatusText.approvalTitle(s)} · ${p.name}"
        }
        val text = if (s != null && s.blockedKind == "deletions" && at != null) {
            if (s.blockedSide == "drive") Strings.get(R.string.activity_problems_deleted_on_phone, atTime(at))
            else Strings.get(R.string.activity_problems_deleted_on_drive, atTime(at))
        } else {
            StatusText.approvalBody(p.name, s, p.pendingApprovalSummary)
        }
        val step = if (kind == "first-merge") Step.ReviewFirstSync(p.id) else Step.ReviewDeletions(p.id)
        return Item(
            key = "approval-${p.id}", status = SyncStatus.Approval, chipLabel = SyncStatus.Approval.label,
            title = title, text = text, actionLabel = StatusText.approvalAction(s), action = step, tonal = true, open = step,
        )
    }

    private fun failedItem(p: FolderPairEntity, st: PairStatusUi): Item {
        val kind = st.errorKind
        val fix = StatusText.fixFor(kind)
        val step: Step = when (fix) {
            StatusText.Fix.SIGN_IN -> Step.SignIn
            StatusText.Fix.GRANT_ACCESS -> Step.GrantAccess
            StatusText.Fix.FREE_SPACE -> Step.FreeSpace
            StatusText.Fix.CHOOSE_FOLDER -> Step.ChooseFolder(p.id)
            StatusText.Fix.UNLOCK_VAULT -> Step.UnlockVault
            StatusText.Fix.RETRY -> Step.Retry(p.id, p.name)
        }
        // A full Drive pauses the pair; once there's space it needs a nudge to carry on.
        val resume = fix == StatusText.Fix.FREE_SPACE && p.paused
        return Item(
            key = "failed-${p.id}", status = SyncStatus.Failed, chipLabel = SyncStatus.Failed.label,
            title = StatusText.failedTitle(p.name, kind), text = st.detail ?: Strings.get(R.string.activity_problems_stopped_before_finish),
            actionLabel = StatusText.fixLabel(fix), action = step,
            secondaryLabel = if (resume) Strings.get(R.string.common_try_again) else null, secondary = if (resume) Step.Resume(p.id, p.name) else null,
            open = Step.OpenPair(p.id),
        )
    }

    private fun accountItem(a: AccountEntity): Item = Item(
        key = "account-${a.id}", status = SyncStatus.Failed, chipLabel = SyncStatus.Failed.label,
        title = Strings.get(R.string.activity_problems_sign_in_again_as, a.id),
        text = Strings.get(R.string.activity_problems_sign_in_again_body),
        actionLabel = StatusText.fixLabel(StatusText.Fix.SIGN_IN), action = Step.SignIn, open = Step.SignIn,
    )

    private fun conflictItem(c: ConflictEntity, pairName: String?): Item {
        val name = c.key.substringAfterLast('/')
        val step = Step.OpenConflict(c.id)
        return Item(
            key = "conflict-${c.id}", status = SyncStatus.Conflict, chipLabel = SyncStatus.Conflict.label,
            title = listOfNotNull(name, pairName).joinToString(" · "),
            text = Strings.get(R.string.activity_problems_conflict_body),
            actionLabel = Strings.get(R.string.activity_problems_choose), action = step, open = step,
        )
    }

    private fun retryItem(p: FolderPairEntity, st: PairStatusUi): Item = Item(
        key = "retry-${p.id}", status = SyncStatus.Waiting, chipLabel = st.label, icon = st.icon ?: Sym.Schedule,
        title = p.name,
        text = listOfNotNull(st.detail, st.lastSync?.let { "$it." }).joinToString(" ").ifEmpty { null },
        open = Step.OpenPair(p.id),
    )

    private fun decode(s: String): PlanSummary? = runCatching { json.decodeFromString(PlanSummary.serializer(), s) }.getOrNull()

    companion object {
        private const val WARNING_LIMIT = 100
        private const val WARNING_DAYS = 7
    }
}

@Composable
fun ProblemsScreen(nav: AppNav) {
    val vm: ProblemsViewModel = hiltViewModel()
    val ui by vm.ui.collectAsStateWithLifecycle()
    val context = LocalContext.current
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    var showWarnings by rememberSaveable { mutableStateOf(false) }
    val syncHistoryLabel = stringResource(R.string.common_sync_history)
    val syncStatusLabel = stringResource(R.string.activity_sync_status)
    val diagnosticsLabel = stringResource(R.string.common_diagnostics)

    fun go(step: ProblemsViewModel.Step) {
        when (step) {
            is ProblemsViewModel.Step.ReviewDeletions -> nav.reviewDeletions(step.pairId)
            is ProblemsViewModel.Step.ReviewFirstSync -> nav.dryRun(step.pairId)
            is ProblemsViewModel.Step.OpenConflict -> nav.conflict(step.conflictId)
            is ProblemsViewModel.Step.OpenPair -> nav.pairDetail(step.pairId)
            is ProblemsViewModel.Step.ChooseFolder -> nav.pairEdit(step.pairId)
            is ProblemsViewModel.Step.Retry -> {
                vm.retry(step.pairId)
                scope.launch { snackbar.showMessage(Strings.get(R.string.activity_problems_syncing_again, step.pairName)) }
            }
            is ProblemsViewModel.Step.Resume -> {
                vm.resume(step.pairId)
                scope.launch { snackbar.showMessage(Strings.get(R.string.activity_problems_syncing_again, step.pairName)) }
            }
            ProblemsViewModel.Step.SignIn -> nav.accounts()
            ProblemsViewModel.Step.GrantAccess -> {
                if (!openAllFilesAccess(context)) scope.launch { snackbar.showMessage(Strings.get(R.string.activity_problems_allow_all_files)) }
            }
            ProblemsViewModel.Step.FreeSpace -> {
                if (!openUrl(context, DRIVE_STORAGE_URL)) scope.launch { snackbar.showMessage(Strings.get(R.string.activity_problems_no_storage_app)) }
            }
            ProblemsViewModel.Step.UnlockVault -> nav.vaults()
        }
    }

    AppScreen(
        title = stringResource(R.string.activity_problems_title),
        onBack = null,
        actions = {
            OverflowMenu(
                listOf(
                    MenuAction(syncHistoryLabel, Sym.History) { nav.history() },
                    MenuAction(syncStatusLabel, Sym.Sync) { nav.status() },
                    MenuAction(diagnosticsLabel, Sym.MonitorHeart) { nav.diagnostics() },
                ),
            )
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        val u = ui
        when {
            u == null -> LoadingState()
            u.needsYou.isEmpty() && u.retrying.isEmpty() && u.warnings.isEmpty() ->
                ProblemsAllClear(Modifier.padding(padding).fillMaxSize(), onDiagnostics = { nav.diagnostics() })
            // All clear, but recent warnings stay reachable under it.
            u.needsYou.isEmpty() && u.retrying.isEmpty() -> LazyColumn(Modifier.padding(padding).fillMaxSize(), contentPadding = PaddingValues(bottom = Space.s24)) {
                item(key = "clear") { ProblemsAllClear(Modifier.fillMaxWidth().padding(vertical = Space.s32), onDiagnostics = { nav.diagnostics() }) }
                warningsSection(u.warnings, showWarnings, onToggle = { showWarnings = !showWarnings }, onOpen = { nav.runDetail(it) })
            }
            else -> LazyColumn(Modifier.padding(padding).fillMaxSize(), contentPadding = PaddingValues(bottom = Space.s24)) {
                if (u.needsYou.isNotEmpty()) {
                    item(key = "h-needs") { SectionHeader(stringResource(R.string.activity_problems_needs_you, Fmt.count(u.needsYou.size))) }
                    items(u.needsYou, key = { it.key }) { item -> ProblemRow(item) { go(it) } }
                }
                if (u.retrying.isNotEmpty()) {
                    item(key = "h-retry") { SectionHeader(stringResource(R.string.activity_problems_retrying, Fmt.count(u.retrying.size))) }
                    items(u.retrying, key = { it.key }) { item -> ProblemRow(item) { go(it) } }
                }
                warningsSection(u.warnings, showWarnings, onToggle = { showWarnings = !showWarnings }, onOpen = { nav.runDetail(it) })
            }
        }
    }
}

/** One problem: its status, what happened in plain words, and the one action that resolves it. */
@Composable
private fun ProblemRow(item: ProblemsViewModel.Item, onStep: (ProblemsViewModel.Step) -> Unit) {
    val cs = MaterialTheme.colorScheme
    val open = item.open
    Row(
        Modifier.fillMaxWidth()
            .then(if (open != null) Modifier.clickable { onStep(open) } else Modifier)
            .heightIn(min = Size.listTwoLine)
            .padding(horizontal = Space.s16, vertical = Space.s12),
        horizontalArrangement = Arrangement.spacedBy(Space.s16),
    ) {
        // The chip below carries the word; the leading icon is the design's visual anchor.
        StatusIcon(item.status, Modifier.padding(top = Space.s2), size = 24.dp, icon = item.icon)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(Space.s2)) {
            Text(item.title, style = MaterialTheme.typography.bodyLarge, color = cs.onSurface)
            if (item.text != null) Text(item.text, style = MaterialTheme.typography.bodyMedium, color = cs.onSurfaceVariant)
            FlowRow(
                Modifier.padding(top = Space.s8),
                horizontalArrangement = Arrangement.spacedBy(Space.s8),
                verticalArrangement = Arrangement.spacedBy(Space.s8),
            ) {
                StatusChip(item.status, Modifier.align(Alignment.CenterVertically), label = item.chipLabel, icon = item.icon, size = ChipSize.Sm)
                val action = item.action
                val actionLabel = item.actionLabel
                if (action != null && actionLabel != null) {
                    val mod = Modifier.align(Alignment.CenterVertically).heightIn(min = Size.buttonCompact)
                    if (item.tonal) {
                        FilledTonalButton(onClick = { onStep(action) }, modifier = mod, contentPadding = PaddingValues(horizontal = Space.s16)) { Text(actionLabel) }
                    } else {
                        OutlinedButton(onClick = { onStep(action) }, modifier = mod, contentPadding = PaddingValues(horizontal = Space.s16)) { Text(actionLabel) }
                    }
                }
                val second = item.secondary
                val secondLabel = item.secondaryLabel
                if (second != null && secondLabel != null) {
                    TextButton(onClick = { onStep(second) }, modifier = Modifier.align(Alignment.CenterVertically).heightIn(min = Size.buttonCompact)) { Text(secondLabel) }
                }
            }
        }
    }
}

private fun LazyListScope.warningsSection(
    warnings: List<ProblemsViewModel.Warning>,
    expanded: Boolean,
    onToggle: () -> Unit,
    onOpen: (runId: String) -> Unit,
) {
    if (warnings.isEmpty()) return
    item(key = "h-warn") { ProblemsWarningsHeader(warnings.size, expanded, onToggle) }
    if (expanded) {
        items(warnings, key = { "w${it.id}" }) { w -> ProblemsWarningRow(w) { onOpen(w.runId) } }
    }
}

@Composable
private fun ProblemsWarningsHeader(count: Int, expanded: Boolean, onToggle: () -> Unit) {
    val state = stringResource(if (expanded) R.string.activity_problems_shown else R.string.activity_problems_hidden)
    SectionHeader(
        stringResource(R.string.activity_problems_recent_warnings, Fmt.count(count)),
        Modifier
            .clickable(onClickLabel = stringResource(if (expanded) R.string.activity_problems_hide_warnings else R.string.activity_problems_show_warnings), onClick = onToggle)
            .semantics { stateDescription = state },
        action = {
            Icon(
                Sym.KeyboardArrowDown,
                contentDescription = null,
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.rotate(if (expanded) 180f else 0f),
            )
        },
    )
}

@Composable
private fun ProblemsWarningRow(w: ProblemsViewModel.Warning, onOpen: () -> Unit) {
    Box(
        Modifier.fillMaxWidth()
            .clickable(onClickLabel = stringResource(R.string.activity_problems_open_sync), onClick = onOpen)
            .heightIn(min = Size.touchTarget)
            .padding(horizontal = Space.s16),
        contentAlignment = Alignment.CenterStart,
    ) {
        LogLineRow(w.line)
    }
}

/** Empty state: "Nothing needs you". */
@Composable
private fun ProblemsAllClear(modifier: Modifier, onDiagnostics: () -> Unit) {
    Column(
        modifier.padding(horizontal = Space.s32),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(Space.s16, Alignment.CenterVertically),
    ) {
        Box(Modifier.size(72.dp).clip(CircleShape).background(Ds.status.synced.container), contentAlignment = Alignment.Center) {
            Icon(Sym.TaskAlt, contentDescription = null, tint = Ds.status.synced.content, modifier = Modifier.size(36.dp))
        }
        Text(stringResource(R.string.activity_problems_all_clear_title), style = MaterialTheme.typography.titleLarge, color = MaterialTheme.colorScheme.onSurface, textAlign = TextAlign.Center)
        Text(
            stringResource(R.string.activity_problems_all_clear_body),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
        )
        TextButton(onClick = onDiagnostics) { ButtonLabel(stringResource(R.string.activity_problems_open_diagnostics), Sym.MonitorHeart) }
    }
}
