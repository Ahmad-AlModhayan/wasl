// Problems (#66, design D4/D8): a list of what needs the person, grouped by kind, and the selected item resolved in
// a detail pane beside it, never on a pushed page. Held deletions list the files with Restore (the default) and
// Delete (approve once, never the default); conflicts compare both versions (conflict.ts); failed syncs give
// their one fix; accounts sign in again; agent changes link to their own section.
//
// The list is a listbox: ↑/↓, Home and End move the selection and the detail follows; Return moves the focus into
// the detail's heading, so it never answers anything. After an answer the next item is selected and focused.
// ⌘Z / Ctrl+Z undoes the last conflict answer, as the toast's Undo does.

import { api } from '../api';
import type { App } from '../app';
import { lang, t } from '../i18n';
import { failureText, formatBytes, isolatePath } from '../format';
import { errorBanner, errorText, h, line, statusGlyph, toast, toastShowing } from '../ui';
import type { ConflictRecord, DeletionReview, PairState, PendingDeletion, PlanSummary } from '../types';
import type { LineName } from '../icons';
import { bindContextMenu, type MenuLine } from '../shell/contextMenu';
import { focusedNow, mayTakeFocus, nextAfter } from '../shell/nextFocus';
import { conflictChoices, conflictDetail, isAnswering, resolveConflict, runUndo, type ConflictLoad, type UndoStep } from './conflict';
import { signIn, signInButton, signInWaiting } from '../signIn';
import { groupProblems, guardReason, needsYouCount, problemItems, type Problem } from './problemsModel';
import { unlockDialog, vaultsById } from './vault';
import '../styles/review.css';

/** A held plan as the daemon keeps it: the guard's reason and the plan's summary. */
interface HeldPlan {
  reason: string | null;
  summary: PlanSummary | null;
  created_at: number | null;
}

type ReviewState = { key: string; state: 'loading' } | { key: string; state: 'ok'; review: DeletionReview } | { key: string; state: 'error'; message: string };

let selectedId: string | null = null;
/** An item to focus once the list shows (the next one after an answer). */
let focusOnShow: string | null = null;
let focusUntil = 0;
/** The `data-key` of the control the answer came from, which the re-render may hand the focus back to. */
let focusFrom: string | null = null;
/**
 * The last conflict answer, for ⌘Z / Ctrl+Z: only while its toast shows and on the screen it was taken on (the
 * route object is new on each navigation). A deletion answer, which has no undo, clears it.
 */
let lastUndo: (UndoStep & { route: object }) | null = null;
/** Each conflict's two versions, read when it is selected. A failed read is tried again on the next live update. */
const conflictLoads = new Map<string, ConflictLoad>();
const reviews = new Map<string, ReviewState>();
/** Puts the selected item's detail on screen again (a review finished loading). */
let refreshDetail: (() => void) | null = null;

function isMac(): boolean {
  return document.documentElement.dataset.kfPlatform === 'mac';
}

function heldPlan(v: unknown): HeldPlan | null {
  if (!v || typeof v !== 'object') return null;
  const o = v as Record<string, unknown>;
  return {
    reason: typeof o.reason === 'string' ? o.reason : null,
    summary: o.summary && typeof o.summary === 'object' ? (o.summary as PlanSummary) : null,
    created_at: typeof o.created_at === 'number' ? o.created_at : null,
  };
}

export async function problemsScreen(app: App): Promise<HTMLElement> {
  const title = h('h1', { class: 'sr-title', tabindex: '-1' }, t('problems.title'));
  let conflicts: ConflictRecord[];
  try {
    conflicts = await api.openConflicts();
  } catch (e) {
    return h('div', { class: 'screen' }, title, errorBanner(e, () => app.rerender()));
  }
  appRef = app;
  const items = problemItems(app.status, conflicts, lang());
  const pairs = new Map((app.status?.pairs ?? []).map((p) => [p.pair.id, p]));
  const held = new Map<string, HeldPlan | null>(
    await Promise.all(
      items.filter((p) => p.kind === 'deletion').map(async (p) => [p.pairId!, heldPlan(await api.blockedPlan(p.pairId!).catch(() => null))] as const),
    ),
  );
  for (const [id, load] of conflictLoads) if (load.state === 'error' || !conflicts.some((c) => c.id === id)) conflictLoads.delete(id);

  if (items.length === 0) {
    selectedId = null;
    refreshDetail = null;
    if (app.selectedPairs().length) app.select([]);
    app.setCommands(null);
    return emptyState();
  }

  const order = items.map((p) => p.id);
  if (!selectedId || !order.includes(selectedId)) selectedId = order[0]!;
  const ctx: Ctx = { app, items, order, pairs, held, conflicts: new Map(conflicts.map((c) => [c.id, c])) };
  const groups = groupProblems(items, app.status?.agent_changes ?? 0);

  const list = h('div', { class: 'review-list', role: 'listbox', 'aria-label': t('problems.title') });
  const summary = h('p', { class: 'review-list-summary' }, t('problems.needs_you', { count: needsYouCount(groups) }));
  for (const g of groups) {
    const gid = `pg-${g.kind}`;
    list.append(
      h(
        'div',
        { role: 'group', 'aria-labelledby': gid, class: 'review-group' },
        h('div', { id: gid, class: 'review-group-label', role: 'presentation' }, `${t(`problems.group.${g.kind}`)} · ${g.count.toLocaleString(lang())}`),
        g.items.map((p) => listItem(ctx, p)),
      ),
    );
  }
  const detailBox = h('div', { class: 'review-detail-box' });
  const showDetail = () => detailBox.replaceChildren(detail(ctx, items.find((p) => p.id === selectedId)!));
  refreshDetail = () => {
    if (!detailBox.isConnected) return;
    const key = (document.activeElement as HTMLElement | null)?.closest<HTMLElement>('[data-key]')?.dataset.key;
    const inside = detailBox.contains(document.activeElement);
    showDetail();
    if (inside) (detailBox.querySelector<HTMLElement>(`[data-key="${CSS.escape(key ?? '')}"]`) ?? detailBox.querySelector<HTMLElement>('h2'))?.focus();
  };

  const choose = (id: string, focus: boolean) => {
    selectedId = id;
    focusOnShow = null;
    for (const el of list.querySelectorAll<HTMLElement>('[role="option"]')) {
      const on = el.dataset.id === id;
      el.setAttribute('aria-selected', String(on));
      el.tabIndex = on ? 0 : -1;
      if (on && focus) el.focus();
    }
    showDetail();
    syncShell(ctx);
  };
  list.addEventListener('click', (e) => {
    const el = (e.target as HTMLElement).closest<HTMLElement>('[role="option"]');
    if (el) choose(el.dataset.id!, true);
  });
  list.addEventListener('keydown', (e) => {
    if (e.defaultPrevented || e.altKey || e.metaKey || e.ctrlKey) return;
    const at = order.indexOf(selectedId ?? '');
    const go = (i: number) => {
      e.preventDefault();
      choose(order[Math.max(0, Math.min(order.length - 1, i))]!, true);
    };
    if (e.key === 'ArrowDown') go(at + 1);
    else if (e.key === 'ArrowUp') go(at - 1);
    else if (e.key === 'Home') go(0);
    else if (e.key === 'End') go(order.length - 1);
    else if (e.key === 'Enter') {
      // Into the detail, onto its heading: Return from the list never answers anything.
      e.preventDefault();
      detailBox.querySelector<HTMLElement>('h2')?.focus();
    }
  });

  showDetail();
  syncShell(ctx);
  if (focusOnShow) setTimeout(applyFocusOnShow);
  return h('div', { class: 'review-screen problems-screen' }, title, h('div', { class: 'review-split' }, h('div', { class: 'review-master' }, summary, list), detailBox));
}

interface Ctx {
  app: App;
  items: Problem[];
  order: string[];
  pairs: Map<string, PairState>;
  held: Map<string, HeldPlan | null>;
  conflicts: Map<string, ConflictRecord>;
}

/** The toolbar and the inspector follow the selected item's pair: ⌘R syncs it, the inspector shows it. */
function syncShell(ctx: Ctx): void {
  const p = ctx.items.find((x) => x.id === selectedId);
  const ids = p?.pairId ? [p.pairId] : [];
  const now = ctx.app.selectedPairs().map((x) => x.pair.id);
  if (now.join() !== ids.join()) ctx.app.select(ids);
  ctx.app.setCommands([{ id: 'sync', label: t('pairs.sync_now'), icon: 'sync', run: () => ctx.app.syncNow(), shortcut: 'sync_now', disabled: ctx.app.conn.state !== 'connected' }]);
}

/** After an answer: the next item gets the selection and the focus. */
function answered(ctx: Ctx, id: string): void {
  selectedId = nextAfter(ctx.order, [id]);
  focusAfterRender(selectedId);
  ctx.app.rerender();
}

/**
 * Puts the focus on `focusOnShow` after the shell has placed its own. The daemon's news of the answer re-renders
 * the screen once or twice more, and the shell then focuses the heading: each of those renders tries again, until
 * the person moves the focus somewhere else or a few seconds pass.
 */
function applyFocusOnShow(): void {
  if (!focusOnShow || Date.now() > focusUntil) {
    focusOnShow = null;
    return;
  }
  const el = document.querySelector<HTMLElement>(`.problems-screen [data-id="${CSS.escape(focusOnShow)}"]`);
  if (el && document.activeElement === el) return;
  if (!mayTakeFocus(focusedNow(), focusFrom)) focusOnShow = null;
  else el?.focus();
}

function focusAfterRender(id: string | null): void {
  focusOnShow = id;
  focusFrom = focusedNow()?.key ?? null;
  focusUntil = Date.now() + 3000;
}

// ---------------------------------------------------------------- the list

function pairName(ctx: Ctx, id: string | null): string {
  return (id && ctx.pairs.get(id)?.pair.name) || t('history.unknown_pair');
}

function deletionCount(s: PlanSummary | null | undefined): number {
  return s ? s.deleteLocal + s.deleteRemote : 0;
}

function itemText(ctx: Ctx, p: Problem): { glyph: HTMLElement; title: string; sub: string } {
  const glyph = (state: 'approval' | 'failed' | 'conflict', icon?: LineName) => {
    const g = statusGlyph({ state, paused: false, percent: null, count: 1 });
    if (icon) g.replaceChildren(line(icon));
    return g;
  };
  switch (p.kind) {
    case 'deletion': {
      const plan = ctx.held.get(p.pairId!);
      const n = deletionCount(plan?.summary);
      const when = plan?.created_at ? ctx.app.ago(plan.created_at) : null;
      return {
        glyph: glyph('approval', 'stop'),
        title: n > 0 ? t('problems.item_deletion', { count: n }) : t('problems.item_stopped'),
        sub: when ? t('problems.item_sub_stopped', { name: pairName(ctx, p.pairId), when }) : pairName(ctx, p.pairId),
      };
    }
    case 'preview':
      return { glyph: glyph('approval', 'eye'), title: t('problems.item_preview', { name: pairName(ctx, p.pairId) }), sub: t('problems.item_sub_preview') };
    case 'failed': {
      const st = ctx.pairs.get(p.pairId!)?.pair.status;
      return {
        glyph: glyph('failed'),
        title: t('problems.item_failed', { name: pairName(ctx, p.pairId) }),
        sub: st?.last_run_at ? t('problems.item_sub_failed', { when: ctx.app.ago(st.last_run_at) }) : (failureText(st?.last_error_kind ?? null, st?.last_error ?? null, lang()) ?? ''),
      };
    }
    case 'conflict': {
      const c = ctx.conflicts.get(p.ref!)!;
      const file = c.key.split('/').pop() ?? c.key;
      return { glyph: glyph('conflict'), title: t('problems.item_conflict', { file }), sub: `${pairName(ctx, p.pairId)} · ${ctx.app.ago(c.detected_at)}` };
    }
    case 'account': {
      const a = ctx.app.status?.accounts.find((x) => x.id === p.ref);
      return { glyph: glyph('failed'), title: t('problems.account_title', { email: a?.email ?? '' }), sub: t('problems.item_sub_account') };
    }
    case 'agents':
      return { glyph: glyph('approval', 'spark'), title: t('problems.agent_changes_title', { count: ctx.app.status?.agent_changes ?? 0 }), sub: t('problems.item_sub_agents') };
  }
}

function listItem(ctx: Ctx, p: Problem): HTMLElement {
  const { glyph, title, sub } = itemText(ctx, p);
  const on = p.id === selectedId;
  const el = h(
    'div',
    { role: 'option', class: 'review-item', 'aria-selected': String(on), tabindex: on ? '0' : '-1', 'data-id': p.id, 'data-key': `problem-${p.id}` },
    glyph,
    h('span', { class: 'review-item-text' }, h('span', { class: 'review-item-title', dir: 'auto' }, title), h('span', { class: 'review-item-sub', dir: 'auto' }, sub)),
  );
  bindContextMenu(el, () => {
    if (selectedId !== p.id) el.click();
    return itemMenu(ctx, p);
  });
  return el;
}

function itemMenu(ctx: Ctx, p: Problem): MenuLine[] {
  const app = ctx.app;
  const lines: MenuLine[] = [];
  if (p.kind === 'conflict') {
    const c = ctx.conflicts.get(p.ref!)!;
    for (const o of conflictChoices()) {
      lines.push({
        text: o.label,
        disabled: isAnswering(c.id),
        run: () => void resolveConflict(c, o.choice, o.label, () => undoLast(app)).then((step) => step && conflictAnswered(ctx, p, step)),
      });
    }
  } else if (p.kind === 'preview') {
    const id = p.pairId!;
    lines.push({ text: t('pairs.preview_and_start'), icon: 'eye', run: () => app.go({ name: 'preview', id }) });
  } else if (p.kind === 'failed') {
    lines.push({ text: t('common.retry'), icon: 'sync', run: () => app.syncNow([p.pairId!]) });
  } else if (p.kind === 'account') {
    lines.push({ text: t('problems.sign_in_again'), run: startSignIn });
  } else if (p.kind === 'agents') {
    lines.push({ text: t('problems.agent_changes_open'), icon: 'spark', run: () => app.go({ name: 'agents' }) });
  }
  if (p.pairId && ctx.pairs.has(p.pairId)) {
    const id = p.pairId;
    if (lines.length) lines.push('separator');
    lines.push(
      { text: t('problems.open_pair', { name: pairName(ctx, id) }), icon: 'folder', run: () => app.go({ name: 'pair', id }) },
      { text: app.revealLabel(), icon: 'reveal', shortcut: 'show_in_finder', run: () => app.showInFolder(id) },
    );
  }
  return lines;
}

function conflictAnswered(ctx: Ctx, p: Problem, step: UndoStep): void {
  lastUndo = { label: step.label, toast: step.toast, route: ctx.app.route, run: async () => {
    await step.run();
    // The conflict comes back: select it again.
    selectedId = p.id;
    focusAfterRender(p.id);
  } };
  conflictLoads.delete(p.ref!);
  answered(ctx, p.id);
}

function startSignIn(): void {
  void signIn.start();
}

// ---------------------------------------------------------------- the detail pane

function detail(ctx: Ctx, p: Problem): HTMLElement {
  switch (p.kind) {
    case 'deletion':
      return deletionDetail(ctx, p);
    case 'conflict': {
      const c = ctx.conflicts.get(p.ref!)!;
      return conflictDetail(ctx.app, c, conflictLoad(c.id), pairName(ctx, p.pairId), (step) => conflictAnswered(ctx, p, step), () => undoLast(ctx.app));
    }
    case 'preview': {
      const id = p.pairId!;
      return simpleDetail('approval', t('problems.item_preview', { name: pairName(ctx, id) }), t('detail.hold_body'), [
        deskButton(t('pairs.preview_and_start'), () => ctx.app.go({ name: 'preview', id }), 'preview-start', true),
        deskButton(t('problems.open_pair', { name: pairName(ctx, id) }), () => ctx.app.go({ name: 'pair', id }), 'preview-open'),
      ], 'eye');
    }
    case 'failed':
      return failedDetail(ctx, p);
    case 'account': {
      const a = ctx.app.status?.accounts.find((x) => x.id === p.ref);
      const box = simpleDetail('failed', t('problems.account_title', { email: a?.email ?? '' }), a?.auth_error ?? null, [signInButton(deskButton(t('problems.sign_in_again'), startSignIn, 'account-sign-in', true))]);
      box.querySelector('.review-body')?.append(signInWaiting());
      return box;
    }
    case 'agents':
      return simpleDetail('approval', t('problems.agent_changes_title', { count: ctx.app.status?.agent_changes ?? 0 }), t('problems.agent_changes_body'), [
        deskButton(t('problems.agent_changes_open'), () => ctx.app.go({ name: 'agents' }), 'agents-open', true),
      ], 'spark');
  }
}

function deskButton(text: string, run: () => void, key: string, primary = false, danger = false): HTMLButtonElement {
  return h('button', { type: 'button', class: `kf-desk-btn${primary ? ' kf-desk-btn-default' : ''}${danger ? ' kf-desk-btn-danger' : ''}`, 'data-key': key, onclick: run }, text);
}

/** Buttons with the default last, on the trailing side, on both platforms (#124). */
function defaultLast(buttons: HTMLElement[], primary: HTMLElement): HTMLElement[] {
  return [...buttons.filter((b) => b !== primary), primary];
}

function head(state: 'approval' | 'failed' | 'conflict', title: string, icon?: LineName): HTMLElement {
  const glyph = statusGlyph({ state, paused: false, percent: null, count: 1 }, { label: true });
  if (icon) glyph.firstElementChild?.replaceWith(line(icon));
  return h('div', { class: 'review-title-row' }, h('h2', { class: 'review-title', dir: 'auto', tabindex: '-1' }, title), glyph);
}

function simpleDetail(state: 'approval' | 'failed', title: string, body: string | null, buttons: HTMLElement[], icon?: LineName): HTMLElement {
  return h(
    'section',
    { class: 'review-detail', 'aria-label': title },
    h('div', { class: 'review-body' }, head(state, title, icon), body ? h('p', { dir: 'auto' }, body) : null),
    h('footer', { class: 'review-foot' }, h('span', { class: 'review-foot-note' }), h('div', { class: 'review-foot-actions' }, defaultLast(buttons, buttons[0]!))),
  );
}

function failedDetail(ctx: Ctx, p: Problem): HTMLElement {
  const s = ctx.pairs.get(p.pairId!)!;
  const st = s.pair.status;
  const id = s.pair.id;
  const why = failureText(st.last_error_kind, st.last_error, lang()) ?? t('banner.failed_body');
  let fix: HTMLButtonElement;
  if (st.last_error_kind === 'Locked' && s.pair.vault_id) {
    fix = deskButton(t('vault.unlock'), () => {
      void vaultsById().then((m) => m.get(s.pair.vault_id!)).then((v) => v && unlockDialog(v)).then((ok) => ok && ctx.app.syncNow([id]));
    }, 'failed-unlock', true);
  } else if (st.last_error_kind === 'Auth') {
    fix = signInButton(deskButton(t('banner.sign_in'), startSignIn, 'failed-sign-in', true));
  } else {
    fix = deskButton(t('common.retry'), () => ctx.app.syncNow([id]), 'failed-retry', true);
  }
  const open = deskButton(t('problems.open_pair', { name: s.pair.name }), () => ctx.app.go({ name: 'pair', id }), 'failed-open');
  return h(
    'section',
    { class: 'review-detail', 'aria-label': t('problems.item_failed', { name: s.pair.name }) },
    h('div', { class: 'review-body' }, head('failed', t('problems.item_failed', { name: s.pair.name })), h('p', { dir: 'auto' }, why)),
    h('footer', { class: 'review-foot' }, h('span', { class: 'review-foot-note' }), h('div', { class: 'review-foot-actions' }, defaultLast([fix, open], fix))),
  );
}

/** Reads a conflict's two versions the first time it is selected; the detail says "Reading…" until they arrive. */
function conflictLoad(id: string): ConflictLoad {
  const have = conflictLoads.get(id);
  if (have) return have;
  const loading: ConflictLoad = { state: 'loading' };
  conflictLoads.set(id, loading);
  api.conflictDetails(id).then(
    (details) => conflictLoads.get(id) === loading && conflictLoads.set(id, { state: 'ok', details }),
    (e: unknown) => conflictLoads.get(id) === loading && conflictLoads.set(id, { state: 'error', message: errorText(e) }),
  ).finally(() => refreshDetail?.());
  return loading;
}

/** Starts the full review once per held plan; the detail shows the summary until it arrives. */
function reviewFor(pairId: string, plan: HeldPlan | null): ReviewState {
  const key = `${pairId}:${plan?.created_at ?? ''}`;
  const have = reviews.get(pairId);
  if (have && have.key === key) return have;
  const loading: ReviewState = { key, state: 'loading' };
  reviews.set(pairId, loading);
  api.deletionReview(pairId).then(
    (review) => reviews.get(pairId) === loading && reviews.set(pairId, { key, state: 'ok', review }),
    (e: unknown) => reviews.get(pairId) === loading && reviews.set(pairId, { key, state: 'error', message: errorText(e) }),
  ).finally(() => refreshDetail?.());
  return loading;
}

/** "Drive: a.txt" / "device: a.txt" from a plan's sample (wasl-core's planner). */
function sampleItem(s: string): PendingDeletion {
  const m = /^(Drive|device): (.*)$/.exec(s);
  const key = m ? m[2]! : s;
  return { key, name: key.split('/').pop() ?? key, folder: '', is_dir: false, files: 1, bytes: -1, on_drive: m?.[1] === 'Drive' };
}

function deletionDetail(ctx: Ctx, p: Problem): HTMLElement {
  const app = ctx.app;
  const s = ctx.pairs.get(p.pairId!)!;
  const id = s.pair.id;
  const name = s.pair.name;
  const plan = ctx.held.get(id) ?? null;
  const review = reviewFor(id, plan);
  const full = review.state === 'ok' ? review.review : null;
  const onDrive = full ? full.files_on_drive : (plan?.summary?.deleteRemote ?? 0);
  const onDevice = full ? full.files_on_device : (plan?.summary?.deleteLocal ?? 0);
  const count = onDrive + onDevice;
  // Stopped for another reason (no deletions, known from the plan or the review): approve it, nothing to restore.
  const nothingDeleted = count === 0 && (full !== null || plan?.summary != null);
  const where = onDevice === 0 && onDrive > 0 ? 'drive' : onDrive === 0 && onDevice > 0 ? 'device' : 'both';
  const n = { count: count.toLocaleString(lang()) };
  const restoreText = t(where === 'drive' ? 'deletions.restore_here' : where === 'device' ? 'deletions.restore_drive' : 'deletions.restore', n);
  const deleteText = nothingDeleted
    ? t('deletions.approve_once')
    : t(where === 'drive' ? 'deletions.delete_drive' : where === 'device' ? 'deletions.delete_here' : 'deletions.delete', n);

  const busy = (on: boolean) => foot.querySelectorAll('button').forEach((b) => (b.disabled = on));
  const restore = deskButton(restoreText, async () => {
    busy(true);
    try {
      const back = await api.restoreDeletions(id);
      lastUndo = null;
      toast(t('deletions.restored', { count: back }));
      reviews.delete(id);
      answered(ctx, p.id);
    } catch (e) {
      toast(errorText(e));
      busy(false);
    }
  }, 'deletion-restore', true);
  // Approve once: the plan runs as it is. Never the default, and never on Return from the list.
  const remove = deskButton(deleteText, async () => {
    busy(true);
    try {
      await api.approveOnce(id);
      lastUndo = null;
      toast(t('problems.approved', { name }));
      reviews.delete(id);
      answered(ctx, p.id);
      await api.sync(id, 'manual');
    } catch (e) {
      toast(errorText(e));
      busy(false);
    }
  }, 'deletion-approve', false, !nothingDeleted);
  const foot = h(
    'footer',
    { class: 'review-foot' },
    h('p', { class: 'review-foot-note' }, nothingDeleted ? null : t('deletions.footnote')),
    h('div', { class: 'review-foot-actions' }, nothingDeleted ? [remove] : defaultLast([remove, restore], restore)),
  );

  let files: HTMLElement | null;
  if (nothingDeleted) {
    files = null; // no files to list: the plan deletes nothing
  } else if (full) {
    files = fileTable(full.items);
  } else if (review.state === 'error') {
    const sample = (plan?.summary?.sampleDeletes ?? []).map(sampleItem);
    // Read again on request: a failed review is kept until the plan changes otherwise.
    const retry = h('button', { type: 'button', class: 'review-link', 'data-key': 'deletion-retry', onclick: () => {
      reviews.delete(id);
      refreshDetail?.();
    } }, t('common.retry'));
    files = h('div', null,
      sample.length ? fileTable(sample) : null,
      h('p', { class: 'review-muted', dir: 'auto' }, t('deletions.sample_note', { shown: sample.length, count, message: review.message }), ' ', retry));
  } else {
    files = h('p', { class: 'review-muted', role: 'status' }, t('deletions.loading'));
  }
  const title = count > 0 ? t('deletions.title', { count, name }) : t('problems.item_stopped');
  return h(
    'section',
    { class: 'review-detail', 'aria-label': title },
    h(
      'div',
      { class: 'review-body' },
      head('approval', title),
      h('p', null, t(nothingDeleted ? 'deletions.stopped_body' : 'deletions.body')),
      plan?.reason ? h('p', { class: 'review-muted', dir: 'auto' }, guardReason(lang(), plan.reason)) : null,
      files,
      h('div', null, h('button', { type: 'button', class: 'review-link', 'data-key': 'deletion-open', onclick: () => app.go({ name: 'pair', id }) }, t('problems.open_pair', { name }))),
    ),
    foot,
  );
}

// ---------------------------------------------------------------- the files a held plan would delete

/** Above this many rows only the rows in view are on the page. */
const VIRTUAL_FROM = 200;

function fileRow(d: PendingDeletion, i: number): HTMLElement {
  const where = t(d.on_drive ? 'deletions.where_drive' : 'deletions.where_device');
  return h(
    'div',
    { role: 'row', class: 'review-file', 'aria-rowindex': String(i + 2) },
    h(
      'span',
      { role: 'cell', class: 'review-file-name' },
      line(d.is_dir ? 'folder' : 'doc', { size: 16 }),
      h('bdi', { class: 'review-path', dir: 'ltr', title: d.key }, isolatePath(d.is_dir ? `${d.key}/` : d.key)),
    ),
    h('span', { role: 'cell', class: 'review-muted' }, d.is_dir ? `${where} · ${t('deletions.folder_files', { count: d.files })}` : where),
    h('span', { role: 'cell', class: 'review-file-size kf-tnum' }, d.bytes >= 0 ? formatBytes(d.bytes, lang()) : ''),
  );
}

function fileTable(items: PendingDeletion[]): HTMLElement {
  const header = h(
    'div',
    { role: 'row', class: 'review-file review-file-head', 'aria-rowindex': '1' },
    h('span', { role: 'columnheader' }, t('deletions.col_file')),
    h('span', { role: 'columnheader' }, t('deletions.col_where')),
    h('span', { role: 'columnheader', class: 'review-file-size' }, t('deletions.col_size')),
  );
  const body = h('div', { role: 'rowgroup', class: 'review-files-body' });
  const table = h('div', { role: 'table', class: 'review-files', 'aria-label': t('deletions.col_file'), 'aria-rowcount': String(items.length + 1) }, h('div', { role: 'rowgroup' }, header), body);
  if (items.length <= VIRTUAL_FROM) {
    body.append(...items.map(fileRow));
    return table;
  }
  // Long lists: a fixed row height, and only the rows in view (plus a margin) on the page.
  body.classList.add('review-files-virtual');
  const spacer = h('div', { class: 'review-files-spacer' });
  body.append(spacer);
  let drawn = '';
  const draw = () => {
    const row = parseFloat(getComputedStyle(body).getPropertyValue('--review-file-row')) || 28;
    spacer.style.blockSize = `${items.length * row}px`;
    const from = Math.max(0, Math.floor(body.scrollTop / row) - 10);
    const to = Math.min(items.length, Math.ceil((body.scrollTop + body.clientHeight) / row) + 10);
    if (drawn === `${from}:${to}`) return;
    drawn = `${from}:${to}`;
    spacer.replaceChildren(
      ...items.slice(from, to).map((d, k) => {
        const r = fileRow(d, from + k);
        r.style.insetBlockStart = `${(from + k) * row}px`;
        return r;
      }),
    );
  };
  body.addEventListener('scroll', () => requestAnimationFrame(draw), { passive: true });
  requestAnimationFrame(draw);
  return table;
}

// ---------------------------------------------------------------- empty

function emptyState(): HTMLElement {
  return h(
    'div',
    { class: 'review-screen problems-screen review-empty-screen' },
    h(
      'section',
      { class: 'review-empty', 'aria-labelledby': 'problems-empty-title' },
      line('synced', { size: 36 }),
      h('h1', { id: 'problems-empty-title', tabindex: '-1' }, t('problems.empty_title')),
      h('p', null, t('problems.empty_body')),
    ),
  );
}

// ---------------------------------------------------------------- ⌘Z / Ctrl+Z

/** Undo from the toast's button (the toast is the one on screen) or from ⌘Z / Ctrl+Z (checked there). */
function undoLast(app: App): void {
  const step = lastUndo;
  lastUndo = null;
  if (step) void runUndo(step).then(() => app.rerender());
}

let appRef: App | null = null;

document.addEventListener('keydown', (e) => {
  if (!lastUndo || !appRef || !document.querySelector('.problems-screen')) return;
  // Hours later, or after another answer's toast, ⌘Z undoes nothing.
  if (lastUndo.route !== appRef.route || !toastShowing(lastUndo.toast)) {
    lastUndo = null;
    return;
  }
  const mod = isMac() ? e.metaKey : e.ctrlKey;
  if (!mod || e.shiftKey || e.altKey || e.code !== 'KeyZ') return;
  const el = e.target as HTMLElement | null;
  if (el?.closest('input, textarea, [contenteditable="true"]')) return;
  e.preventDefault();
  undoLast(appRef);
});
