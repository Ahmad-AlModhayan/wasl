// Recycle bin (#67, design "Recycle bin", D4/D7): a table of what syncs deleted, newest first, with multi-select
// (shell/selection.ts, as All pairs). Restore is the default action (Return); Delete forever… (⌘⌫ / Delete) and
// Empty recycle bin… (toolbar) always ask first, naming the count. A restore can be undone from its toast or with
// ⌘Z / Ctrl+Z: the restored files go back into the bin (wasld's UndoBinRestore). ⌘F / Ctrl+F filters by name,
// folder or pair. The inspector shows the selected files.

import { api } from '../api';
import type { App } from '../app';
import { lang, locale, t } from '../i18n';
import { formatBytes, formatDate, formatDay, isolatePath } from '../format';
import { confirmDialog, errorBanner, errorText, h, line, toast, toastShowing } from '../ui';
import type { BinEntry } from '../types';
import { bindContextMenu, type MenuLine } from '../shell/contextMenu';
import { focusedNow, mayTakeFocus, nextAfter } from '../shell/nextFocus';
import { extendTo, move, NO_SELECTION, prune, selectAll, selectOne, toggle, type Selection } from '../shell/selection';
import { binFolder, binName, binSummary, failedSummary, filterBin, rebinned, restoreFailures, sortBin } from './binModel';
import '../styles/review.css';

const COLUMNS = ['name', 'folder', 'pair', 'from', 'deleted', 'size'] as const;

let sel: Selection = NO_SELECTION;
/** One restore, for its Undo: what came back, the toast that offers Undo, and the screen it was taken on. */
interface Restored {
  entries: BinEntry[];
  toast: number;
  route: object;
}
/** The last restore, for ⌘Z / Ctrl+Z: only while its toast shows and on the screen it was taken on. */
let lastRestore: Restored | null = null;
let appRef: App | null = null;
/** A row to focus after the next renders (the next row after a restore or a delete). */
let focusOnShow: string | null = null;
let focusUntil = 0;
/** The `data-key` of the control the action came from, which the re-render may hand the focus back to. */
let focusFrom: string | null = null;
/** ⌘A in the macOS app reaches the page as the menu's Select All: the table on screen takes it. */
let onSelectAll: (() => boolean) | null = null;

function isMac(): boolean {
  return document.documentElement.dataset.kfPlatform === 'mac';
}

export async function binScreen(app: App): Promise<HTMLElement> {
  appRef = app;
  app.onQuery(() => app.rerender());
  const title = h('h1', { class: 'sr-title', tabindex: '-1' }, t('bin.title'));
  let entries: BinEntry[];
  try {
    entries = await api.binEntries();
  } catch (e) {
    return h('div', { class: 'screen' }, title, errorBanner(e, () => app.rerender()));
  }
  if (app.selectedPairs().length) app.select([]);
  app.setCommands([
    { id: 'empty-bin', label: t('bin.empty_more'), icon: 'trash', run: () => void emptyAll(app, entries.length), disabled: entries.length === 0 },
  ]);
  if (entries.length === 0) {
    sel = NO_SELECTION;
    onSelectAll = null;
    app.setInspector(null);
    return h(
      'div',
      { class: 'review-screen bin-screen review-empty-screen' },
      h('section', { class: 'review-empty', 'aria-labelledby': 'bin-empty-title' },
        line('trash', { size: 36 }),
        h('h1', { id: 'bin-empty-title', tabindex: '-1' }, t('bin.empty_title')),
        h('p', null, t('bin.empty_body'))),
    );
  }

  const all = sortBin(entries, lang());
  const rows = filterBin(all, app.query, lang());
  const order = rows.map((b) => b.entry.id);
  sel = prune(sel, order);
  const chosen = () => rows.filter((b) => sel.ids.includes(b.entry.id));

  const table = h(
    'table',
    {
      class: 'kf-desk-table bin-table',
      role: 'grid',
      'aria-label': t('bin.table_label'),
      'aria-multiselectable': 'true',
      'aria-rowcount': String(rows.length + 1),
    },
    h('colgroup', null, COLUMNS.map((c) => h('col', { class: `bin-col-${c}` }))),
    h('thead', null, h('tr', { 'aria-rowindex': '1' }, COLUMNS.map((c) => h('th', { scope: 'col', class: `bin-col-${c}` }, t(`bin.col_${c}`))))),
    h(
      'tbody',
      null,
      rows.length
        ? rows.map((b, i) => row(app, b, i))
        : h('tr', null, h('td', { colspan: String(COLUMNS.length), class: 'bin-no-match' }, t('bin.no_match', { query: app.query.trim() }))),
    ),
  );
  const foot = h('footer', { class: 'review-foot bin-foot' });

  const setSel = (next: Selection, opts: { focus?: boolean } = {}) => {
    const before = sel.ids.length;
    sel = next;
    focusOnShow = null;
    for (const tr of table.querySelectorAll<HTMLElement>('tbody tr[data-entry]')) tr.setAttribute('aria-selected', String(sel.ids.includes(tr.dataset.entry!)));
    roving(table);
    renderFoot(app, foot, all, chosen(), order);
    app.setInspector(() => inspector(chosen()));
    if (sel.ids.length !== before && (sel.ids.length > 1 || before > 1)) app.announce(t('bin.selected', { count: sel.ids.length }));
    if (opts.focus) focusRow(table);
  };

  table.addEventListener('click', (e) => {
    const tr = (e.target as HTMLElement).closest<HTMLElement>('tbody tr[data-entry]');
    if (!tr) return;
    const id = tr.dataset.entry!;
    if (e.shiftKey) setSel(extendTo(sel, order, id), { focus: true });
    else if (isMac() ? e.metaKey : e.ctrlKey) setSel(toggle(sel, order, id), { focus: true });
    else setSel(selectOne(id), { focus: true });
  });
  table.addEventListener('mousedown', (e) => {
    if (e.shiftKey) e.preventDefault(); // Shift-click shouldn't select the rows' text
  });
  table.addEventListener('keydown', (e) => onKey(app, e, order, all, chosen, setSel));
  for (const tr of table.querySelectorAll<HTMLElement>('tbody tr[data-entry]')) {
    const id = tr.dataset.entry!;
    bindContextMenu(tr, () => {
      // A right click outside the selection selects that row first, as Finder and Explorer do.
      if (!sel.ids.includes(id)) setSel(selectOne(id));
      return rowMenu(app, chosen(), all, order);
    });
  }
  onSelectAll = () => {
    if (!table.isConnected || !table.contains(document.activeElement) || order.length === 0) return false;
    setSel(selectAll(order));
    return true;
  };

  roving(table);
  renderFoot(app, foot, all, chosen(), order);
  app.setInspector(() => inspector(chosen()));
  if (focusOnShow) {
    // Once the shell has placed its focus, and again after the daemon's own news of the change has re-rendered.
    setTimeout(applyFocusOnShow);
    setTimeout(applyFocusOnShow, 250);
  }
  return h('div', { class: 'review-screen bin-screen' }, title, h('div', { class: 'bin-scroll' }, table), foot);
}

// ---------------------------------------------------------------- rows

function row(app: App, b: BinEntry, i: number): HTMLElement {
  const e = b.entry;
  const name = binName(b);
  const folder = binFolder(b);
  const from = e.side === 'Drive' ? t('bin.from_drive') : t(isMac() ? 'detail.this_mac' : 'detail.this_pc');
  const cells: Record<(typeof COLUMNS)[number], Array<Node | string | null>> = {
    name: [line('doc', { size: 16 }), h('span', { class: 'bin-name', dir: 'auto' }, name)],
    folder: [folder ? h('bdi', { class: 'review-path', dir: 'ltr' }, isolatePath(folder)) : null],
    pair: [h('span', { dir: 'auto' }, e.pair_name ?? t('history.unknown_pair'))],
    from: [line(e.side === 'Drive' ? 'cloud' : 'laptop', { size: 16 }), h('span', null, from)],
    deleted: [h('span', { title: formatDate(e.deleted_at, lang()) }, app.ago(e.deleted_at))],
    size: [formatBytes(e.size, lang())],
  };
  return h(
    'tr',
    { class: 'bin-row', 'aria-rowindex': String(i + 2), 'aria-selected': String(sel.ids.includes(e.id)), 'data-entry': e.id },
    COLUMNS.map((c, ci) =>
      h(
        'td',
        { role: 'gridcell', class: `bin-col-${c}`, tabindex: '-1', ...(ci === 0 ? { 'data-key': `bin-${e.id}`, 'data-focus': '' } : {}) },
        h('span', { class: 'pairs-cell' }, cells[c]),
      ),
    ),
  );
}

/** One tab stop in the grid: the focused row's name. */
function roving(table: HTMLElement): void {
  for (const el of table.querySelectorAll<HTMLElement>('td[data-focus]')) el.tabIndex = -1;
  const cell = currentCell(table);
  if (cell) cell.tabIndex = 0;
}

function currentCell(table: HTMLElement): HTMLElement | null {
  const id = sel.focus ?? table.querySelector<HTMLElement>('tbody tr[data-entry]')?.dataset.entry;
  return id ? table.querySelector<HTMLElement>(`tbody tr[data-entry="${CSS.escape(id)}"] td[data-focus]`) : null;
}

function focusRow(table: HTMLElement): void {
  roving(table);
  currentCell(table)?.focus();
}

function applyFocusOnShow(): void {
  if (!focusOnShow || Date.now() > focusUntil) {
    focusOnShow = null;
    return;
  }
  const el = document.querySelector<HTMLElement>(`.bin-screen tr[data-entry="${CSS.escape(focusOnShow)}"] td[data-focus]`);
  if (el && document.activeElement === el) return;
  if (!mayTakeFocus(focusedNow(), focusFrom)) focusOnShow = null;
  else if (el) {
    el.tabIndex = 0;
    el.focus();
  }
}

/** The focus goes to `id`'s row once the re-renders that follow an action have placed theirs. */
function focusAfterRender(id: string | null): void {
  focusOnShow = id;
  focusFrom = focusedNow()?.key ?? null;
  focusUntil = Date.now() + 3000;
}

/** After rows leave: the next row gets the selection and, after the re-render, the focus. */
function moveOn(order: readonly string[], leaving: readonly string[]): void {
  const next = nextAfter(order, leaving);
  sel = next ? selectOne(next) : NO_SELECTION;
  focusAfterRender(next);
}

function onKey(
  app: App,
  e: KeyboardEvent,
  order: string[],
  all: BinEntry[],
  chosen: () => BinEntry[],
  setSel: (s: Selection, o?: { focus?: boolean }) => void,
): void {
  if (e.defaultPrevented || e.altKey) return;
  const mod = isMac() ? e.metaKey : e.ctrlKey;
  const mode = e.shiftKey ? 'extend' : mod ? 'focus' : 'select';
  const handled = () => {
    e.preventDefault();
    e.stopPropagation();
  };
  switch (e.key) {
    case 'ArrowDown':
    case 'ArrowUp':
      handled();
      setSel(move(sel, order, e.key === 'ArrowDown' ? 1 : -1, mode), { focus: true });
      return;
    case 'Home':
    case 'End':
      handled();
      setSel(move(sel, order, e.key === 'Home' ? 'first' : 'last', e.shiftKey ? 'extend' : 'select'), { focus: true });
      return;
    case 'PageDown':
    case 'PageUp':
      handled();
      setSel(move(sel, order, e.key === 'PageDown' ? 10 : -10, mode), { focus: true });
      return;
    case 'Enter':
      // Restore is the default action.
      handled();
      if (chosen().length) void restore(app, chosen(), all, order);
      return;
    case ' ':
      handled();
      if (mod && sel.focus) setSel(toggle(sel, order, sel.focus), { focus: true });
      else app.toggleInspector(false);
      return;
    case 'Escape':
      if (sel.ids.length > 1) {
        handled();
        setSel(sel.focus ? selectOne(sel.focus) : NO_SELECTION, { focus: true });
      }
      return;
    case 'Delete':
    case 'Backspace':
      // ⌘⌫ on macOS, Delete on Windows: Delete forever…, which asks first.
      if (e.key === 'Backspace' && !(isMac() && e.metaKey)) return;
      handled();
      if (chosen().length) void deleteForever(app, chosen(), order);
      return;
  }
  if (mod && !e.shiftKey && e.code === 'KeyA') {
    handled();
    setSel(selectAll(order), { focus: true });
  }
}

// ---------------------------------------------------------------- the footer, the menu and the inspector

function deskButton(text: string, run: () => void, key: string, opts: { primary?: boolean; danger?: boolean; icon?: 'undo' } = {}): HTMLButtonElement {
  return h(
    'button',
    { type: 'button', class: `kf-desk-btn${opts.primary ? ' kf-desk-btn-default' : ''}${opts.danger ? ' kf-desk-btn-danger' : ''}`, 'data-key': key, onclick: run },
    opts.icon ? line(opts.icon) : null,
    text,
  );
}

function renderFoot(app: App, foot: HTMLElement, all: BinEntry[], chosen: BinEntry[], order: string[]): void {
  if (chosen.length === 0) {
    const s = binSummary(all);
    foot.replaceChildren(h('p', { class: 'review-foot-note kf-tnum' }, t('bin.total', { count: s.count, size: formatBytes(s.bytes, lang()) })));
    return;
  }
  const s = binSummary(chosen);
  const restoreBtn = deskButton(t('bin.restore_n', { count: chosen.length }), () => void restore(app, chosen, all, order), 'bin-restore', { primary: true, icon: 'undo' });
  const deleteBtn = deskButton(t('bin.delete_more'), () => void deleteForever(app, chosen, order), 'bin-delete', { danger: true });
  foot.replaceChildren(
    h('p', { class: 'review-foot-note bin-foot-count kf-tnum' }, t('bin.selection', { count: s.count, size: formatBytes(s.bytes, lang()), date: formatDay(s.keptUntil!, lang()) })),
    // The default last, on the trailing side, on both platforms (#124).
    h('div', { class: 'review-foot-actions' }, deleteBtn, restoreBtn),
  );
}

function rowMenu(app: App, chosen: BinEntry[], all: BinEntry[], order: string[]): MenuLine[] {
  const lines: MenuLine[] = [{ text: t('bin.restore_n', { count: chosen.length }), icon: 'undo', run: () => void restore(app, chosen, all, order) }];
  const one = chosen.length === 1 ? chosen[0]!.entry : null;
  const pair = one ? app.status?.pairs.find((p) => p.pair.id === one.pair_id) : undefined;
  if (pair) {
    // The IPC reveals a pair's folder by its id, never a path; the file's own folder is shown in the inspector.
    lines.push('separator', {
      text: t(isMac() ? 'bin.show_pair_finder' : 'bin.show_pair_explorer', { name: pair.pair.name }),
      icon: 'reveal',
      run: () => app.showInFolder(pair.pair.id),
    }, { text: t('problems.open_pair', { name: pair.pair.name }), icon: 'folder', run: () => app.go({ name: 'pair', id: pair.pair.id }) });
  }
  lines.push('separator', { text: t('bin.delete_more'), icon: 'trash', danger: true, run: () => void deleteForever(app, chosen, order) });
  return lines;
}

function inspector(chosen: BinEntry[]): HTMLElement {
  if (chosen.length !== 1) {
    const s = binSummary(chosen);
    return h(
      'div',
      { class: 'shell-insp' },
      h('div', { class: 'shell-insp-group' },
        h('h2', { id: 'shell-inspector-title', class: 'shell-insp-title', tabindex: '-1' }, chosen.length ? t('bin.selected', { count: chosen.length }) : t('bin.title')),
        chosen.length ? h('p', { class: 'shell-insp-caption kf-tnum' }, formatBytes(s.bytes, lang())) : null),
    );
  }
  const e = chosen[0]!.entry;
  const fact = (label: string, value: Node | string) => [h('dt', null, label), h('dd', null, value)];
  return h(
    'div',
    { class: 'shell-insp' },
    h('div', { class: 'shell-insp-group' },
      h('h2', { id: 'shell-inspector-title', class: 'shell-insp-title', tabindex: '-1', dir: 'auto' }, binName(chosen[0]!)),
      h('p', { class: 'path shell-insp-path' }, isolatePath(e.key))),
    h(
      'dl',
      { class: 'review-facts bin-insp-facts' },
      fact(t('bin.col_pair'), e.pair_name ?? t('history.unknown_pair')),
      fact(t('bin.col_from'), e.side === 'Drive' ? t('bin.from_drive') : t(isMac() ? 'detail.this_mac' : 'detail.this_pc')),
      fact(t('bin.col_deleted'), formatDate(e.deleted_at, lang())),
      fact(t('bin.col_size'), formatBytes(e.size, lang())),
    ),
    h('p', { class: 'shell-insp-caption' }, t('bin.kept_until', { date: formatDay(e.expires_at, lang()) })),
  );
}

// ---------------------------------------------------------------- actions

async function restore(app: App, chosen: BinEntry[], all: BinEntry[], order: string[]): Promise<void> {
  const ids = chosen.map((b) => b.entry.id);
  try {
    const r = await api.restoreBin(ids);
    const fails = restoreFailures(r, all);
    const done: Restored | null = r.restored.length ? { entries: r.restored, toast: 0, route: app.route } : null;
    const undo = done ? { label: t('problems.undo'), run: () => void undoRestore(app, done) } : undefined;
    const text =
      fails.length === 0
        ? t('bin.restored', { count: r.restored.length })
        : r.restored.length === 0
          ? failedText(fails)
          : `${t('bin.restore_partial', { restored: r.restored.length, total: ids.length })} ${failedText(fails)}`;
    const shown = toast(text, undo);
    lastRestore = done && { ...done, toast: shown };
    // What couldn't come back stays selected; otherwise the next row.
    const failed = r.failed.map((f) => f.entry_id).filter((id) => order.includes(id));
    if (failed.length) sel = { ids: failed, anchor: failed[0]!, focus: failed[0]! };
    else moveOn(order, ids);
    app.rerender();
  } catch (e) {
    toast(errorText(e));
  }
}

/** "“a”, “b” and 3 more couldn't be restored: why" (the reason when they share one). */
function failedText(fails: Array<{ name: string; message: string }>): string {
  if (fails.length === 1) return t('bin.restore_failed_named', fails[0]!);
  const f = failedSummary(fails);
  const parts = f.names.map((name) => t('bin.quoted', { name }));
  if (f.more) parts.push(t('bin.n_more', { count: f.more }));
  const names = new Intl.ListFormat(locale(lang()), { type: 'conjunction' }).format(parts);
  return f.message ? t('bin.restore_failed_list_why', { names, message: f.message }) : t('bin.restore_failed_list', { names });
}

async function undoRestore(app: App, back: Restored): Promise<void> {
  if (lastRestore?.entries === back.entries) lastRestore = null;
  try {
    // Ids only: the app's Rust side keeps what the restore brought back and hands wasld those entries.
    await api.undoBinRestore(back.entries.map((b) => b.entry.id));
    toast(t('bin.restore_undone'));
    // wasld bins them again under new ids: found again by pair, side and key.
    const ids = rebinned(back.entries, await api.binEntries().catch(() => []));
    sel = { ids, anchor: ids[0] ?? null, focus: ids[0] ?? null };
    focusAfterRender(ids[0] ?? null);
    app.rerender();
  } catch (e) {
    toast(errorText(e));
  }
}

async function deleteForever(app: App, chosen: BinEntry[], order: string[]): Promise<void> {
  const one = chosen.length === 1;
  const ok = await confirmDialog({
    title: one ? t('bin.delete_title', { name: binName(chosen[0]!) }) : t('bin.delete_many_title', { count: chosen.length }),
    body: t('bin.delete_body'),
    confirm: one ? t('bin.delete_forever') : t('bin.delete_n_forever', { count: chosen.length }),
    danger: true,
  });
  if (!ok) return;
  const ids = chosen.map((b) => b.entry.id);
  try {
    toast(t('bin.deleted', { count: await api.deleteBinForever(ids) }));
    moveOn(order, ids);
    app.rerender();
  } catch (e) {
    toast(errorText(e));
  }
}

async function emptyAll(app: App, count: number): Promise<void> {
  // The count is checked again after the sheet: if more came in while it was open, it asks again with the new count.
  for (let named = count; ; ) {
    const ok = await confirmDialog({ title: t('bin.empty_confirm_title'), body: t('bin.empty_count', { count: named }), confirm: t('bin.empty'), danger: true });
    if (!ok) return;
    let now: number;
    try {
      now = (await api.binEntries()).length;
    } catch (e) {
      toast(errorText(e));
      return;
    }
    if (now <= named) break;
    named = now;
  }
  try {
    toast(t('bin.deleted', { count: await api.emptyBin() }));
    sel = NO_SELECTION;
    app.rerender();
  } catch (e) {
    toast(errorText(e));
  }
}

// ---------------------------------------------------------------- ⌘Z / Ctrl+Z and ⌘A from the macOS menu bar

document.addEventListener('keydown', (e) => {
  if (!lastRestore || !appRef || !document.querySelector('.bin-screen')) return;
  const mod = isMac() ? e.metaKey : e.ctrlKey;
  if (!mod || e.shiftKey || e.altKey || e.code !== 'KeyZ') return;
  if ((e.target as HTMLElement | null)?.closest('input, textarea, [contenteditable="true"]')) return;
  // Hours later, after another toast, or on another visit to the bin, ⌘Z undoes nothing.
  if (lastRestore.route !== appRef.route || !toastShowing(lastRestore.toast)) {
    lastRestore = null;
    return;
  }
  e.preventDefault();
  void undoRestore(appRef, lastRestore);
});

let pointerDown = false;
document.addEventListener('pointerdown', () => (pointerDown = true), true);
document.addEventListener('pointerup', () => (pointerDown = false), true);
document.addEventListener('selectstart', (e) => {
  if (!pointerDown && onSelectAll?.()) e.preventDefault();
});
