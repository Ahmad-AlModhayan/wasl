// All pairs (#64, design D3/D4/D8): a sortable table of the pairs with multi-select, a bulk bar, and each row's
// context menu. Selecting fills the inspector; Return, a double click or the pair's sidebar row opens Pair detail.
//
// The table is an ARIA grid with one tab stop: the arrows move between cells (mirrored in Arabic), the selection
// follows the focus, Shift extends it, ⌘/Ctrl moves the focus alone and ⌘/Ctrl+Space toggles a row. Space shows or
// hides the inspector, Return opens the pair, letters jump to a name. Live updates re-render the screen; the
// selection, the sort and the focused cell live here, so they survive it.

import { api } from '../api';
import type { App } from '../app';
import { lang, t } from '../i18n';
import { formatBytes, isLoud, pairStatus } from '../format';
import { confirmDialog, errorText, h, line, statusGlyph, toast } from '../ui';
import type { PairState } from '../types';
import { bindContextMenu } from '../shell/contextMenu';
import { extendTo, move, NO_SELECTION, prune, sameIds, selectAll, selectOne, toggle, type Selection } from '../shell/selection';
import {
  COLUMNS,
  directionKey,
  filterPairs,
  folderSize,
  lastSync,
  nextSort,
  parseSort,
  shownColumn,
  sortPairs,
  stepColumn,
  syncsWithDevices,
  typeAhead,
  type Column,
} from './pairsModel';

const SORT_KEY = 'wasl.pairs.sort';

let sel: Selection = NO_SELECTION;
let sort = parseSort(readLocal(SORT_KEY));
/** The focused cell: its column, and whether it is in the header row. */
let focusCol = 0;
let focusHeader = false;
/** A row to focus once the table shows (coming back from Pair detail). */
let focusOnShow: string | null = null;
let typed = '';
let typedAt = 0;
/** ⌘A in the macOS app reaches the page as the menu's Select All: the table on screen takes it. */
let onSelectAll: (() => boolean) | null = null;
/** Watches the table's width, where Direction comes and goes; one at a time, for the table on screen. */
let resizer: ResizeObserver | null = null;

/** Coming back from Pair detail (Esc, Back): that pair's row is selected and focused. */
export function showInTable(id: string): void {
  sel = selectOne(id);
  focusHeader = false;
  focusOnShow = id;
}

function isMac(): boolean {
  return document.documentElement.dataset.kfPlatform === 'mac';
}

export function pairsScreen(app: App): HTMLElement {
  app.onQuery(() => app.rerender());
  const all = app.conn.state === 'connected' ? app.status?.pairs ?? [] : [];
  if (all.length === 0) {
    onSelectAll = null;
    if (app.selectedPairs().length) app.select([]);
    return emptyState(app);
  }
  const now = Date.now();
  const rows = sortPairs(filterPairs(all, app.query), sort, lang(), now);
  const order = rows.map((r) => r.pair.id);
  sel = prune(sel, order);
  if (!sameIds(app.selectedPairs().map((p) => p.pair.id), sel.ids)) app.select(sel.ids);

  const title = h('h1', { class: 'sr-title', tabindex: '-1' }, t('nav.pairs'));
  const bulk = h('div', { class: 'pairs-bulk', role: 'group' });
  const table = h(
    'table',
    {
      class: 'kf-desk-table pairs-table',
      role: 'grid',
      'aria-label': t('pairs.table_label'),
      'aria-multiselectable': 'true',
      'aria-rowcount': String(rows.length + 1),
      'aria-colcount': String(COLUMNS.length),
    },
    h('colgroup', null, COLUMNS.map((c) => h('col', { class: `pairs-col-${c}` }))),
    h('thead', null, h('tr', { 'aria-rowindex': '1' }, COLUMNS.map((c, i) => headerCell(app, c, i)))),
    h(
      'tbody',
      null,
      rows.length
        ? rows.map((s, i) => row(app, s, i, now))
        : h('tr', null, h('td', { colspan: String(COLUMNS.length), class: 'pairs-no-match' }, t('shell.no_match', { query: app.query.trim() }))),
    ),
  );

  /** Puts a selection on screen in place (no re-render), and tells the shell and a screen reader. */
  const setSel = (next: Selection, opts: { focus?: boolean } = {}) => {
    const before = sel.ids.length;
    sel = next;
    for (const tr of table.querySelectorAll<HTMLElement>('tbody tr[data-pair]')) {
      tr.setAttribute('aria-selected', String(sel.ids.includes(tr.dataset.pair!)));
    }
    roving(table);
    renderBulk(app, bulk, rows);
    app.select(sel.ids);
    if (sel.ids.length !== before && (sel.ids.length > 1 || before > 1)) app.announce(t('pairs.selected', { count: sel.ids.length }));
    if (opts.focus) focusCell(table);
  };

  table.addEventListener('click', (e) => {
    const target = e.target as HTMLElement;
    const cell = target.closest<HTMLElement>('[data-col]');
    if (cell) focusCol = Number(cell.dataset.col);
    const tr = target.closest<HTMLElement>('tbody tr[data-pair]');
    if (!tr) return;
    focusHeader = false;
    const id = tr.dataset.pair!;
    const toggleKey = isMac() ? e.metaKey : e.ctrlKey;
    if (e.shiftKey) setSel(extendTo(sel, order, id), { focus: true });
    else if (toggleKey) setSel(toggle(sel, order, id), { focus: true });
    else setSel(selectOne(id), { focus: true });
  });
  table.addEventListener('dblclick', (e) => {
    const tr = (e.target as HTMLElement).closest<HTMLElement>('tbody tr[data-pair]');
    if (tr) app.go({ name: 'pair', id: tr.dataset.pair! });
  });
  // Shift-click shouldn't select the rows' text.
  table.addEventListener('mousedown', (e) => {
    if (e.shiftKey) e.preventDefault();
  });
  table.addEventListener('keydown', (e) => onGridKey(app, e, table, order, rows, setSel));
  for (const tr of table.querySelectorAll<HTMLElement>('tbody tr[data-pair]')) {
    const id = tr.dataset.pair!;
    bindContextMenu(tr, () => {
      // A right click on a row outside the selection selects that row first, as Finder and Explorer do; on a row
      // of a multi-selection the menu acts on all of it, as the bulk bar does.
      if (!sel.ids.includes(id)) setSel(selectOne(id), { focus: true });
      return sel.ids.length > 1 ? app.pairsMenu(rows.filter((r) => sel.ids.includes(r.pair.id))) : app.pairMenu(id);
    });
  }
  onSelectAll = () => {
    if (!table.isConnected || !table.contains(document.activeElement) || order.length === 0) return false;
    setSel(selectAll(order));
    return true;
  };

  roving(table);
  renderBulk(app, bulk, rows);
  const scroll = h('div', { class: 'pairs-scroll' }, table);
  // Direction and Size hide when the pane narrows: the tab stop and the column count follow what shows.
  resizer?.disconnect();
  resizer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(() => onResize(table));
  resizer?.observe(scroll);
  if (focusOnShow) {
    const id = focusOnShow;
    focusOnShow = null;
    // After the shell has focused the new screen's heading.
    setTimeout(() => {
      if (sel.focus === id) focusCell(table);
    });
  }
  return h('div', { class: 'pairs-screen' }, title, scroll, bulk);
}

// ---------------------------------------------------------------- the table's parts

function headerCell(app: App, col: Column, i: number): HTMLElement {
  const sorted = sort.by === col;
  return h(
    'th',
    {
      scope: 'col',
      role: 'columnheader',
      class: `pairs-col-${col}`,
      'aria-sort': sorted ? (sort.dir === 'asc' ? 'ascending' : 'descending') : undefined,
    },
    h(
      'button',
      {
        type: 'button',
        class: 'pairs-sort',
        tabindex: '-1',
        'data-col': String(i),
        'data-key': `sort-${col}`,
        onclick: () => {
          sort = nextSort(sort, col);
          writeLocal(SORT_KEY, `${sort.by}:${sort.dir}`);
          focusHeader = true;
          focusCol = i;
          app.rerender();
        },
      },
      h('span', null, t(`pairs.col_${col}`)),
      sorted ? line(sort.dir === 'asc' ? 'sort_up' : 'sort_down', { size: 12 }) : null,
    ),
  );
}

function row(app: App, s: PairState, i: number, now: number): HTMLElement {
  const p = s.pair;
  const v = pairStatus(s);
  const glyph = statusGlyph(v, { label: true, pairId: p.id });
  if (isLoud(v.state)) glyph.classList.add('status-chip');
  const when = lastSync(s, now);
  const size = folderSize(s);
  const cells: Record<Column, Element[]> = {
    name: [line(p.vault_id ? 'lock' : 'folder', { size: 16 }), h('span', { class: 'pairs-name', dir: 'auto' }, p.name)],
    status: [glyph],
    direction: [line(syncsWithDevices(s) ? 'bolt' : 'cloud', { size: 16 }), h('span', null, t(directionKey(s)))],
    last: [h('span', null, s.running ? t('pairs.now') : when !== null ? app.ago(when) : t('pairs.never_synced'))],
    size: [size === null ? h('span', null, h('span', { 'aria-hidden': 'true' }, '—'), h('span', { class: 'kf-sr-only' }, t('pairs.size_unknown'))) : h('span', null, formatBytes(size, lang()))],
  };
  const tr = h(
    'tr',
    {
      class: 'pairs-row',
      'aria-rowindex': String(i + 2),
      'aria-selected': String(sel.ids.includes(p.id)),
      'data-pair': p.id,
    },
    COLUMNS.map((c, ci) =>
      h(
        'td',
        { role: 'gridcell', class: `pairs-col-${c}`, tabindex: '-1', 'data-col': String(ci), 'data-key': `cell-${p.id}-${c}` },
        h('span', { class: 'pairs-cell' }, cells[c]),
      ),
    ),
  );
  return tr;
}

/** The columns that show now, by index: all of them until the table is on screen. */
function shownColumns(table: HTMLElement): number[] {
  const all = COLUMNS.map((_, i) => i);
  if (!table.isConnected) return all;
  const heads = [...table.querySelectorAll<HTMLElement>('thead th')];
  const shown = all.filter((i) => heads[i] && getComputedStyle(heads[i]).display !== 'none');
  return shown.length ? shown : all;
}

/** One tab stop in the grid: the focused cell, in a column that shows. */
function roving(table: HTMLElement): void {
  const shown = shownColumns(table);
  focusCol = shownColumn(shown, focusCol);
  table.setAttribute('aria-colcount', String(shown.length));
  for (const el of table.querySelectorAll<HTMLElement>('[data-col]')) el.tabIndex = -1;
  const cell = currentCell(table);
  if (cell) cell.tabIndex = 0;
}

/** The pane's width changed: a focused Direction or Size cell that just hid hands the focus to the cell before it. */
function onResize(table: HTMLElement): void {
  if (!table.isConnected) return;
  const before = currentCell(table);
  const active = document.activeElement;
  const hadFocus = !!before && (active === before || (active === document.body && before.getClientRects().length === 0));
  roving(table);
  if (hadFocus && currentCell(table) !== before) currentCell(table)?.focus();
}

function currentCell(table: HTMLElement): HTMLElement | null {
  if (focusHeader) return table.querySelector<HTMLElement>(`thead [data-col="${focusCol}"]`);
  const id = sel.focus ?? table.querySelector<HTMLElement>('tbody tr[data-pair]')?.dataset.pair;
  if (!id) return table.querySelector<HTMLElement>(`thead [data-col="${focusCol}"]`);
  return table.querySelector<HTMLElement>(`tbody tr[data-pair="${CSS.escape(id)}"] [data-col="${focusCol}"]`);
}

function focusCell(table: HTMLElement): void {
  roving(table);
  currentCell(table)?.focus();
}

function onGridKey(
  app: App,
  e: KeyboardEvent,
  table: HTMLElement,
  order: string[],
  rows: PairState[],
  setSel: (s: Selection, o?: { focus?: boolean }) => void,
): void {
  if (e.defaultPrevented || e.altKey) return;
  const mod = isMac() ? e.metaKey : e.ctrlKey;
  const rtl = document.documentElement.dir === 'rtl';
  const rowMode = e.shiftKey ? 'extend' : mod ? 'focus' : 'select';
  const atFirstRow = sel.focus === null || order.indexOf(sel.focus) <= 0;
  const handled = (): void => {
    e.preventDefault();
    e.stopPropagation();
  };
  switch (e.key) {
    case 'ArrowDown':
      handled();
      if (focusHeader) {
        focusHeader = false;
        if (sel.focus === null && order[0]) setSel(mod ? { ...sel, focus: order[0] } : selectOne(order[0]), { focus: true });
        else focusCell(table);
      } else setSel(move(sel, order, 1, rowMode), { focus: true });
      return;
    case 'ArrowUp':
      handled();
      if (focusHeader) return;
      if (atFirstRow && !e.shiftKey) {
        focusHeader = true;
        focusCell(table);
      } else setSel(move(sel, order, -1, rowMode), { focus: true });
      return;
    case 'ArrowLeft':
    case 'ArrowRight': {
      handled();
      const forward = (e.key === 'ArrowRight') !== rtl;
      focusCol = stepColumn(shownColumns(table), focusCol, forward ? 1 : -1);
      focusCell(table);
      return;
    }
    case 'Home':
    case 'End':
      handled();
      if (mod || focusHeader) {
        if (mod && order.length) {
          focusHeader = false;
          setSel(move(sel, order, e.key === 'Home' ? 'first' : 'last', e.shiftKey ? 'extend' : 'select'), { focus: true });
        } else {
          focusCol = stepColumn(shownColumns(table), focusCol, e.key === 'Home' ? 'first' : 'last');
          focusCell(table);
        }
      } else {
        focusCol = stepColumn(shownColumns(table), focusCol, e.key === 'Home' ? 'first' : 'last');
        focusCell(table);
      }
      return;
    case 'PageDown':
    case 'PageUp':
      if (focusHeader || !order.length) return;
      handled();
      setSel(move(sel, order, e.key === 'PageDown' ? 10 : -10, rowMode), { focus: true });
      return;
    case 'Enter':
      if (focusHeader) return; // the header's button sorts
      handled();
      if (sel.focus) app.go({ name: 'pair', id: sel.focus });
      return;
    case ' ':
      if (focusHeader) return;
      handled();
      if (mod && sel.focus) setSel(toggle(sel, order, sel.focus), { focus: true });
      else app.toggleInspector(false);
      return;
    case 'Escape':
      // Esc clears a selection of several rows; with one or none it is the shell's (overlays).
      if (sel.ids.length > 1) {
        handled();
        setSel(sel.focus ? selectOne(sel.focus) : NO_SELECTION, { focus: true });
      }
      return;
  }
  if (mod && !e.shiftKey && e.code === 'KeyA') {
    handled();
    setSel(selectAll(order), { focus: true });
    return;
  }
  // Type to select: letters jump to the first name that starts with them.
  if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && e.key !== ' ' && !focusHeader) {
    const at = Date.now();
    typed = at - typedAt > 700 ? e.key : typed + e.key;
    typedAt = at;
    const names = rows.map((r) => r.pair.name);
    const from = typed.length === 1 && sel.focus ? order.indexOf(sel.focus) + 1 : Math.max(0, order.indexOf(sel.focus ?? ''));
    const i = typeAhead(names, typed, from, lang());
    if (i >= 0) {
      handled();
      setSel(selectOne(order[i]!), { focus: true });
    }
  }
}

// ---------------------------------------------------------------- the bulk bar

function renderBulk(app: App, bulk: HTMLElement, rows: PairState[]): void {
  const chosen = rows.filter((r) => sel.ids.includes(r.pair.id));
  bulk.hidden = chosen.length < 2;
  if (bulk.hidden) {
    bulk.replaceChildren();
    return;
  }
  const count = t('pairs.selected', { count: chosen.length });
  const allPaused = chosen.every((p) => p.pair.paused);
  const btn = (key: string, icon: Parameters<typeof line>[0], text: string, run: () => void, danger = false) =>
    h('button', { type: 'button', class: `kf-desk-btn${danger ? ' kf-desk-btn-danger' : ''}`, 'data-key': `bulk-${key}`, onclick: run }, line(icon), text);
  bulk.setAttribute('aria-label', count);
  bulk.replaceChildren(
    h('span', { class: 'pairs-bulk-count' }, count),
    btn('sync', 'sync', t('pairs.sync_now'), () => app.syncNow(chosen.map((p) => p.pair.id))),
    // One key for Pause and Resume, so a live update that flips it keeps the focus on it.
    btn('pause', allPaused ? 'play' : 'pause', t(allPaused ? 'pairs.resume' : 'pairs.pause'), () => void app.setPaused(chosen, !allPaused)),
    btn('remove', 'trash', t('pairs.remove_selected'), () => void removePairs(chosen), true),
  );
}

// ---------------------------------------------------------------- no pairs yet

function emptyState(app: App): HTMLElement {
  const add = h(
    'button',
    { type: 'button', class: 'kf-desk-btn kf-desk-btn-default', 'data-key': 'add-pair-empty', onclick: () => app.go({ name: 'add' }) },
    line('plus'),
    t('pairs.add_pair'),
  );
  const join = h('button', { type: 'button', class: 'kf-desk-btn', 'data-key': 'join-devices', onclick: () => app.go({ name: 'devices' }) }, t('pairs.join_devices'));
  return h(
    'div',
    { class: 'pairs-screen pairs-empty-screen' },
    h(
      'section',
      { class: 'pairs-empty', 'aria-labelledby': 'pairs-empty-title' },
      line('folder', { size: 36 }),
      h('h1', { id: 'pairs-empty-title', tabindex: '-1' }, t('pairs.empty_title')),
      h('p', null, t('pairs.empty_hint')),
      // The default button last, on the trailing side, on both platforms (#124).
      h('div', { class: 'pairs-empty-actions' }, join, add),
      // The empty state is the drop target (#71): the Dock icon takes folders on macOS, the window on both.
      h('p', { class: 'pairs-drop-hint' }, t(isMac() ? 'pairs.drop_hint_mac' : 'pairs.drop_hint_win')),
    ),
  );
}

// ---------------------------------------------------------------- removing

export async function removePair(id: string, name: string): Promise<void> {
  const ok = await confirmDialog({
    title: t('pairs.remove_title', { name }),
    body: t('pairs.remove_body'),
    confirm: t('pairs.remove'),
    danger: true,
  });
  if (!ok) return;
  try {
    await api.deletePair(id);
    toast(t('pairs.removed', { name }));
  } catch (e) {
    toast(errorText(e));
  }
}

/** Remove… on several pairs: one question with the count, then each pair. */
export async function removePairs(pairs: PairState[]): Promise<void> {
  if (pairs.length === 1) return removePair(pairs[0]!.pair.id, pairs[0]!.pair.name);
  const ok = await confirmDialog({
    title: t('pairs.remove_many_title', { count: pairs.length }),
    body: t('pairs.remove_many_body'),
    confirm: t('pairs.remove_many', { count: pairs.length }),
    danger: true,
  });
  if (!ok) return;
  const failed: string[] = [];
  for (const p of pairs) {
    try {
      await api.deletePair(p.pair.id);
    } catch (e) {
      failed.push(errorText(e));
    }
  }
  toast(failed.length ? failed[0]! : t('pairs.removed_many', { count: pairs.length }));
}

// ---------------------------------------------------------------- ⌘A from the macOS menu bar

// The menu's Select All starts a selection with no pointer down; a drag with the mouse is left alone.
let pointerDown = false;
document.addEventListener('pointerdown', () => (pointerDown = true), true);
document.addEventListener('pointerup', () => (pointerDown = false), true);
document.addEventListener('selectstart', (e) => {
  if (!pointerDown && onSelectAll?.()) e.preventDefault();
});

function readLocal(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeLocal(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Private storage unavailable: the sort holds until the window closes.
  }
}
