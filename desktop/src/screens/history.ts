// History (#67, design D4): every pair's syncs, newest first, with filters (pair, outcome, what started it, when)
// that combine, survive live updates and reset when the window opens again. The selected sync's details and
// events show in the detail pane beside the list; the `run` route (from Pair detail) is a selection here, not a
// pushed page. The filters are pure (historyModel.ts) and don't care where the runs come from.

import { api } from '../api';
import type { App } from '../app';
import { lang, locale, t } from '../i18n';
import { formatDate, runCounts } from '../format';
import { errorBanner, errorText, h, line } from '../ui';
import type { LineName } from '../icons';
import type { RunEvent, RunRecord, RunStatus } from '../types';
import { DATE_RANGES, filterRuns, filterShowing, isFiltered, NO_FILTER, oncePer, OUTCOMES, runSeconds, TRIGGERS, type HistoryFilter } from './historyModel';
import '../styles/review.css';

/** How many syncs History reads. */
const LIMIT = 500;

let filter: HistoryFilter = NO_FILTER;
let selectedRun: string | null = null;
const events = new Map<string, RunEvent[] | { error: string }>();
let refreshDetail: (() => void) | null = null;
/** A `run` route is applied on its first render only: live updates re-render with the same route. */
const firstRender = oncePer<object>();

/** Each outcome as one of Wasl's six glyphs; its word always goes with it. */
const GLYPH: Record<RunStatus, { state: string; icon: LineName }> = {
  SUCCESS: { state: 'synced', icon: 'synced' },
  RUNNING: { state: 'syncing', icon: 'syncing' },
  PARTIAL: { state: 'conflict', icon: 'conflict' },
  FAILED: { state: 'failed', icon: 'failed' },
  BLOCKED: { state: 'approval', icon: 'approval' },
  CANCELLED: { state: 'waiting', icon: 'paused' },
  SKIPPED: { state: 'waiting', icon: 'waiting' },
};

function outcomeWord(s: RunStatus): string {
  return t(`run_status.${s.toLowerCase()}`);
}

function triggerWord(trigger: string): string {
  const w = t(`history.trigger.${trigger}`);
  return w.startsWith('history.') ? trigger : w;
}

function outcomeGlyph(s: RunStatus, label: boolean): HTMLElement {
  const g = GLYPH[s];
  const word = outcomeWord(s);
  return h(
    'span',
    { class: `status-glyph status-${g.state}`, role: label ? undefined : 'img', 'aria-label': label ? undefined : word, title: label ? undefined : word },
    line(g.icon),
    label ? h('span', { class: 'status-word' }, word) : null,
  );
}

/** `runId`: opened from Pair detail's recent syncs; that sync is selected, with the filters cleared if they hide it. */
export async function historyScreen(app: App, runId: string | null = null): Promise<HTMLElement> {
  app.onQuery(() => app.rerender());
  const title = h('h1', { class: 'sr-title', tabindex: '-1' }, t('history.title'));
  let runs: RunRecord[];
  try {
    runs = await api.runs(null, LIMIT);
  } catch (e) {
    return h('div', { class: 'screen' }, title, errorBanner(e, () => app.rerender()));
  }
  if (runs.length === 0) {
    refreshDetail = null;
    if (app.selectedPairs().length) app.select([]);
    return h(
      'div',
      { class: 'review-screen history-screen review-empty-screen' },
      h('section', { class: 'review-empty', 'aria-labelledby': 'history-empty-title' },
        line('history', { size: 36 }),
        h('h1', { id: 'history-empty-title', tabindex: '-1' }, t('history.empty_title')),
        h('p', null, t('history.empty_body'))),
    );
  }

  const pairs = app.status?.pairs ?? [];
  const names = new Map(pairs.map((p) => [p.pair.id, p.pair.name]));
  const now = Date.now();
  const search = { query: app.query, names, words: triggerWord, lang: lang() };
  const opening = runId !== null && firstRender(app.route);
  if (opening) {
    selectedRun = runId;
    filter = filterShowing(runs, filter, runId, now, search);
  }
  const shown = filterRuns(runs, filter, now, search);
  const order = shown.map((r) => r.id);
  if (!selectedRun || !order.includes(selectedRun)) selectedRun = order[0] ?? null;
  // Running syncs are still writing events: read them again.
  for (const r of runs) if (r.ended_at === null) events.delete(r.id);

  const list = h('div', { class: 'review-list', role: 'listbox', 'aria-label': t('history.list_label') }, shown.map((r) => runItem(app, r, names)));
  const detailBox = h('div', { class: 'review-detail-box' });
  const showDetail = () => {
    const r = shown.find((x) => x.id === selectedRun);
    detailBox.replaceChildren(r ? runDetail(app, r, names) : noMatch(app));
  };
  refreshDetail = () => {
    if (!detailBox.isConnected) return;
    const inside = detailBox.contains(document.activeElement);
    showDetail();
    if (inside) detailBox.querySelector<HTMLElement>('h2')?.focus();
  };
  const choose = (id: string, focus: boolean) => {
    selectedRun = id;
    for (const el of list.querySelectorAll<HTMLElement>('[role="option"]')) {
      const on = el.dataset.id === id;
      el.setAttribute('aria-selected', String(on));
      el.tabIndex = on ? 0 : -1;
      if (on && focus) el.focus();
    }
    showDetail();
    syncShell(app, shown);
  };
  list.addEventListener('click', (e) => {
    const el = (e.target as HTMLElement).closest<HTMLElement>('[role="option"]');
    if (el) choose(el.dataset.id!, true);
  });
  list.addEventListener('keydown', (e) => {
    if (e.defaultPrevented || e.altKey || e.metaKey || e.ctrlKey || order.length === 0) return;
    const at = order.indexOf(selectedRun ?? '');
    const go = (i: number) => {
      e.preventDefault();
      choose(order[Math.max(0, Math.min(order.length - 1, i))]!, true);
    };
    if (e.key === 'ArrowDown') go(at + 1);
    else if (e.key === 'ArrowUp') go(at - 1);
    else if (e.key === 'Home') go(0);
    else if (e.key === 'End') go(order.length - 1);
    else if (e.key === 'PageDown') go(at + 10);
    else if (e.key === 'PageUp') go(at - 10);
    else if (e.key === 'Enter') {
      e.preventDefault();
      detailBox.querySelector<HTMLElement>('h2')?.focus();
    }
  });

  showDetail();
  syncShell(app, shown);
  if (opening) setTimeout(() => list.querySelector<HTMLElement>('[aria-selected="true"]')?.focus());
  return h(
    'div',
    { class: 'review-screen history-screen' },
    title,
    filterBar(app, runs, names, shown.length),
    h('div', { class: 'review-split' }, h('div', { class: 'review-master' }, list), detailBox),
  );
}

/** The inspector shows the selected sync's pair; ⌘R syncs it. */
function syncShell(app: App, shown: RunRecord[]): void {
  const r = shown.find((x) => x.id === selectedRun);
  const ids = r && app.status?.pairs.some((p) => p.pair.id === r.pair_id) ? [r.pair_id] : [];
  if (app.selectedPairs().map((p) => p.pair.id).join() !== ids.join()) app.select(ids);
  app.setCommands([{ id: 'sync', label: t('pairs.sync_now'), icon: 'sync', run: () => app.syncNow(), shortcut: 'sync_now', disabled: app.conn.state !== 'connected' }]);
}

// ---------------------------------------------------------------- the filters

function filterBar(app: App, runs: RunRecord[], names: Map<string, string>, count: number): HTMLElement {
  const set = (patch: Partial<HistoryFilter>) => {
    filter = { ...filter, ...patch };
    app.rerender();
  };
  const pick = (key: string, label: string, value: string, options: Array<[string, string]>, onPick: (v: string) => void) => {
    const sel = h(
      'select',
      { class: 'history-select', 'data-key': `history-${key}`, onchange: (e: Event) => onPick((e.target as HTMLSelectElement).value) },
      options.map(([v, text]) => h('option', { value: v }, text)),
    );
    sel.value = value;
    return h('label', { class: 'history-filter' }, h('span', { class: 'history-filter-label' }, label), sel);
  };
  // Pairs by name, plus a removed pair the filter still names.
  const pairIds = [...names.keys()].sort((a, b) => names.get(a)!.localeCompare(names.get(b)!, lang()));
  if (filter.pair && !names.has(filter.pair)) pairIds.push(filter.pair);
  const triggers = [...new Set([...TRIGGERS, ...runs.map((r) => r.trigger)])];
  return h(
    'div',
    { class: 'history-filters', role: 'group', 'aria-label': t('history.filters') },
    pick('pair', t('history.filter_pair'), filter.pair ?? '', [['', t('history.any_pair')], ...pairIds.map((id): [string, string] => [id, names.get(id) ?? t('history.unknown_pair')])], (v) => set({ pair: v || null })),
    pick('outcome', t('history.filter_outcome'), filter.outcome ?? '', [['', t('history.any_outcome')], ...OUTCOMES.map((o): [string, string] => [o, outcomeWord(o)])], (v) => set({ outcome: (v || null) as RunStatus | null })),
    pick('trigger', t('history.filter_trigger'), filter.trigger ?? '', [['', t('history.any_trigger')], ...triggers.map((tr): [string, string] => [tr, triggerWord(tr)])], (v) => set({ trigger: v || null })),
    pick('date', t('history.filter_date'), filter.date, DATE_RANGES.map((d): [string, string] => [d, t(`history.date.${d}`)]), (v) => set({ date: v as HistoryFilter['date'] })),
    h('span', { class: 'history-count kf-tnum', role: 'status' }, t('history.count', { count })),
    isFiltered(filter)
      ? h('button', { type: 'button', class: 'kf-desk-btn', 'data-key': 'history-clear', onclick: () => set(NO_FILTER) }, t('history.clear_filters'))
      : null,
  );
}

function noMatch(app: App): HTMLElement {
  return h(
    'section',
    { class: 'review-empty', 'aria-labelledby': 'history-no-match' },
    line('history', { size: 36 }),
    h('h2', { id: 'history-no-match', tabindex: '-1' }, t('history.no_match_title')),
    h('p', null, t('history.no_match_body')),
    isFiltered(filter)
      ? h('button', { type: 'button', class: 'kf-desk-btn', 'data-key': 'history-clear-empty', onclick: () => { filter = NO_FILTER; app.rerender(); } }, t('history.clear_filters'))
      : null,
  );
}

// ---------------------------------------------------------------- the list and the detail

function runItem(app: App, r: RunRecord, names: Map<string, string>): HTMLElement {
  const on = r.id === selectedRun;
  const name = names.get(r.pair_id) ?? t('history.unknown_pair');
  return h(
    'div',
    { role: 'option', class: 'review-item', 'aria-selected': String(on), tabindex: on ? '0' : '-1', 'data-id': r.id, 'data-key': `run-${r.id}` },
    outcomeGlyph(r.status, false),
    h(
      'span',
      { class: 'review-item-text' },
      h('span', { class: 'review-item-title', dir: 'auto' }, name),
      h('span', { class: 'review-item-sub kf-tnum' }, `${app.ago(r.started_at)} · ${triggerWord(r.trigger)}`),
      h('span', { class: 'review-item-sub' }, outcomeWord(r.status)),
    ),
  );
}

function duration(seconds: number): string {
  const minutes = seconds >= 120;
  return new Intl.NumberFormat(locale(lang()), { style: 'unit', unit: minutes ? 'minute' : 'second', unitDisplay: 'long', maximumFractionDigits: 0 }).format(
    minutes ? Math.round(seconds / 60) : seconds,
  );
}

function runDetail(app: App, r: RunRecord, names: Map<string, string>): HTMLElement {
  const name = names.get(r.pair_id) ?? t('history.unknown_pair');
  const counts = runCounts(r.stats);
  const secs = runSeconds(r);
  const fact = (label: string, value: string) => [h('dt', null, label), h('dd', null, value)];
  const known = names.has(r.pair_id);
  return h(
    'section',
    { class: 'review-detail', 'aria-labelledby': `run-title-${r.id}` },
    h(
      'div',
      { class: 'review-body' },
      h('div', { class: 'review-title-row' }, h('h2', { id: `run-title-${r.id}`, class: 'review-title', dir: 'auto', tabindex: '-1' }, name), outcomeGlyph(r.status, true)),
      h(
        'dl',
        { class: 'review-facts run-facts' },
        fact(t('run.started'), formatDate(r.started_at, lang())),
        fact(t('run.took'), secs === null ? t('run.still_running') : duration(secs)),
        fact(t('run.started_by'), triggerWord(r.trigger)),
        fact(t('run.changes'), counts.length ? counts.map(([k, n]) => t(k, { count: n })).join(' · ') : t('history.no_changes')),
      ),
      r.message ? h('p', { dir: 'auto' }, r.message) : null,
      known ? h('div', null, h('button', { type: 'button', class: 'review-link', 'data-key': 'run-open-pair', onclick: () => app.go({ name: 'pair', id: r.pair_id }) }, t('problems.open_pair', { name }))) : null,
      h('h3', { class: 'run-events-title' }, t('run.events')),
      eventList(app, r),
    ),
  );
}

function eventList(app: App, r: RunRecord): HTMLElement {
  const have = events.get(r.id);
  if (!have) {
    api.runEvents(r.id).then(
      (list) => events.set(r.id, list),
      (e: unknown) => events.set(r.id, { error: errorText(e) }),
    ).finally(() => {
      if (selectedRun === r.id) refreshDetail?.();
    });
    return h('p', { class: 'review-muted', role: 'status' }, t('common.loading'));
  }
  if (!Array.isArray(have)) return errorBanner({ kind: 'events', message: have.error }, () => { events.delete(r.id); app.rerender(); });
  if (have.length === 0) return h('p', { class: 'review-muted' }, t('run.empty'));
  const time = new Intl.DateTimeFormat(locale(lang()), { timeStyle: 'medium' });
  return h(
    'ol',
    { class: 'kf-group event-list' },
    have.map((e) =>
      h('li', { class: `kf-row event event-${e.level.toLowerCase()}` },
        line(e.level === 'Error' ? 'failed' : e.level === 'Warn' ? 'warning' : 'info'),
        h('span', { class: 'kf-row-text' },
          h('span', { class: 'kf-row-title', dir: 'auto' }, e.message),
          e.key ? h('bdi', { class: 'kf-row-sub review-path', dir: 'ltr' }, e.key) : null,
          e.detail ? h('span', { class: 'kf-row-sub', dir: 'auto' }, e.detail) : null),
        h('span', { class: 'kf-row-value kf-tnum' }, h('span', { class: 'kf-sr-only' }, t(`run.level.${e.level}`)), time.format(new Date(e.at))))),
  );
}
