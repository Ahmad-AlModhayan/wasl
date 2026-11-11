// The sidebar (#63, design D3): the sections in the design's order with their badges, then every pair with its
// status glyph, like mailboxes in Mail. kahf-design's `.kf-desk-sidebar` draws it as a source list (28px rows, an
// accent selection that greys when the window is inactive), the same on both platforms (#124); Windows adds
// Settings pinned at the bottom (D5), since its Settings is a page in the window. app.css lays out the narrow states.

import { t } from '../i18n';
import { formatBytes, leastFree, overallLine, pairStatus, statusLabel } from '../format';
import { lang } from '../i18n';
import type { LineName } from '../icons';
import { fill, h, line, statusGlyph } from '../ui';
import type { App, Route } from '../app';
import type { PairState } from '../types';
import { bindContextMenu } from './contextMenu';
import { statusNeedsYou } from '../screens/problemsModel';

export type Section = 'pairs' | 'views' | 'devices' | 'problems' | 'agents' | 'history' | 'bin' | 'settings';

/** The sections, in the design's order: ⌘1–⌘7 go to them in this order. */
const BROWSE: Array<{ id: Section; label: string; icon: LineName }> = [
  { id: 'pairs', label: 'nav.pairs', icon: 'folder' },
  { id: 'views', label: 'nav.views', icon: 'eye' },
  { id: 'devices', label: 'nav.devices', icon: 'laptop' },
];
const REVIEW: Array<{ id: Section; label: string; icon: LineName }> = [
  { id: 'problems', label: 'nav.problems', icon: 'stop' },
  { id: 'agents', label: 'nav.agents', icon: 'spark' },
  { id: 'history', label: 'nav.history', icon: 'history' },
  { id: 'bin', label: 'nav.bin', icon: 'trash' },
];

/** The section a route belongs to; a pair's own route belongs to no section (its row is selected instead). */
export function sectionOf(route: Route): Section | null {
  switch (route.name) {
    case 'add':
    case 'preview':
      return 'pairs';
    case 'pair':
      return null;
    case 'run':
      return 'history';
    case 'addView':
      return 'views';
    default:
      return route.name;
  }
}

/** Loud items waiting for someone: what the Problems badge counts. */
export function problemCount(app: App): number {
  return statusNeedsYou(app.status);
}

function isWindows(): boolean {
  return document.documentElement.dataset.kfPlatform === 'windows';
}

/** "LifeOS, Conflict": what a screen reader says for a pair's row. */
export function pairRowName(p: PairState): string {
  return `${p.pair.name}, ${statusLabel(pairStatus(p), lang())}`;
}

export function renderSidebar(app: App, nav: HTMLElement): void {
  const current = sectionOf(app.route);
  const selectedPair = app.route.name === 'pair' ? app.route.id : null;
  const connected = app.conn.state === 'connected';
  const problems = problemCount(app);
  const agents = app.status?.agent_changes ?? 0;

  const item = (s: { id: Section; label: string; icon: LineName }, badge?: { count: number; text: string; loud?: boolean }) =>
    h(
      'li',
      null,
      h(
        'button',
        {
          type: 'button',
          class: 'kf-desk-sidebar-item',
          'data-key': `nav-${s.id}`,
          'aria-current': s.id === current ? 'page' : undefined,
          // The name carries the badge's count, which is hidden from screen readers.
          'aria-label': badge && badge.count > 0 ? `${t(s.label)}, ${badge.text}` : t(s.label),
          title: t(s.label),
          onclick: () => app.go({ name: s.id } as Route),
        },
        line(s.icon),
        h('span', { class: 'shell-label' }, t(s.label)),
        badge && badge.count > 0
          ? h('span', { class: `kf-desk-badge${badge.loud ? ' shell-badge-loud' : ''}`, 'aria-hidden': 'true' }, String(badge.count))
          : null,
      ),
    );

  const pairs = connected ? app.status?.pairs ?? [] : [];
  const q = app.query.trim().toLocaleLowerCase();
  const shown = q ? pairs.filter((p) => p.pair.name.toLocaleLowerCase().includes(q)) : pairs;
  const pairRow = (p: PairState) => {
    const v = pairStatus(p);
    const row = h(
      'button',
      {
        type: 'button',
        class: 'kf-desk-sidebar-item shell-pair',
        'data-key': `pair-${p.pair.id}`,
        'data-pair': p.pair.id,
        'aria-current': p.pair.id === selectedPair ? 'page' : undefined,
        'aria-label': pairRowName(p),
        title: p.pair.name,
        onclick: () => app.go({ name: 'pair', id: p.pair.id }),
      },
      line(p.pair.vault_id ? 'lock' : 'folder'),
      h('span', { class: 'shell-label' }, p.pair.name),
      statusGlyph(v, { pairId: p.pair.id }),
    );
    bindContextMenu(row, () => app.pairMenu(p.pair.id));
    return h('li', null, row);
  };

  fill(
    nav,
    // macOS: the traffic lights sit in this strip, over the sidebar (#61).
    isWindows() ? null : h('div', { class: 'chrome-strip', 'data-tauri-drag-region': '' }),
    h('ul', { class: 'shell-nav-list', role: 'list' }, BROWSE.map((s) => item(s))),
    h('h2', { class: 'kf-desk-sidebar-label shell-group' }, t('nav.review')),
    h(
      'ul',
      { class: 'shell-nav-list', role: 'list' },
      REVIEW.map((s) =>
        s.id === 'problems'
          ? item(s, { count: problems, text: t('nav.problems_count', { count: problems }), loud: true })
          : s.id === 'agents'
            ? item(s, { count: agents, text: t('nav.agents_count', { count: agents }) })
            : item(s),
      ),
    ),
    h(
      'div',
      { class: 'kf-desk-sidebar-label shell-group shell-group-pairs' },
      h('h2', { id: 'shell-pairs-head' }, t('nav.pair_group')),
      h(
        'button',
        {
          type: 'button',
          class: 'kf-desk-btn kf-desk-icon-btn shell-add',
          'data-key': 'nav-add',
          'aria-label': t('shell.new_pair'),
          title: t('shell.new_pair'),
          onclick: () => app.go({ name: 'add' }),
        },
        line('plus'),
      ),
    ),
    h(
      'ul',
      { class: 'shell-nav-list shell-pairs', role: 'list', 'aria-labelledby': 'shell-pairs-head' },
      shown.map(pairRow),
      q && pairs.length > 0 && shown.length === 0 ? h('li', { class: 'shell-no-match' }, t('shell.no_match', { query: app.query.trim() })) : null,
    ),
    footer(app),
    isWindows() ? h('ul', { class: 'shell-nav-list shell-pinned', role: 'list' }, item({ id: 'settings', label: 'nav.settings', icon: 'gear' })) : null,
  );
}

/** The sidebar's foot: the overall line (how many pairs are syncing and how far, or the app's state), then the free
 *  space on this computer (#103) when the daemon reports it. */
function footer(app: App): HTMLElement | null {
  const s = app.status;
  if (!s || app.conn.state !== 'connected' || s.pairs.length === 0) return null;
  const lines = [...statusLines(app), freeLine(s.pairs)].filter((x): x is HTMLElement => x !== null);
  return lines.length ? h('div', { class: 'shell-footer' }, lines) : null;
}

function statusLines(app: App): HTMLElement[] {
  const s = app.status!;
  const views = s.pairs.map(pairStatus);
  const running = views.filter((v) => v.state === 'syncing');
  if (running.length > 0) {
    const known = running.filter((v) => v.percent !== null);
    const percent = known.length ? Math.round(known.reduce((n, v) => n + (v.percent ?? 0), 0) / known.length) : null;
    return [
      h(
        'p',
        { class: 'shell-footer-line status-syncing' },
        line('syncing'),
        h('span', null, t('shell.footer_syncing', { done: running.length, total: s.pairs.length })),
      ),
      h(
        'div',
        { class: 'shell-footer-bar', 'aria-hidden': 'true' },
        h('i', { style: `inline-size:${percent ?? 30}%` }),
      ),
    ];
  }
  // A Google account that needs signing in again is why Drive stopped (design S.errors): the foot says so.
  if (s.accounts.some((a) => a.auth_error)) {
    return [h('p', { class: 'shell-footer-line' }, statusGlyph({ state: 'failed', paused: false, percent: null, count: 0 }), h('span', null, t('banner.auth')))];
  }
  // The glyph and the word come from the daemon's one overall state, so they never disagree.
  const overall = overallLine(s, lang());
  return overall ? [h('p', { class: 'shell-footer-line' }, statusGlyph(overall.view), h('span', null, overall.word))] : [];
}

/** "This Mac · 120 GB free": the fullest drive among the pairs' folders. Nothing when no pair reports it. */
function freeLine(pairs: readonly PairState[]): HTMLElement | null {
  const free = leastFree(pairs);
  if (free === null) return null;
  return h(
    'p',
    { class: 'shell-footer-line', 'data-key': 'footer-free' },
    line('laptop'),
    // The least free space of any pair's disk, which may be an external one, so it isn't named after this computer.
    h('span', { class: 'shell-footer-grow' }, t('shell.footer_free_label')),
    h('span', { class: 'shell-footer-num' }, t('shell.footer_free', { size: formatBytes(free, lang()) })),
  );
}

export function sidebarKeys(nav: HTMLElement): void {
  nav.addEventListener('keydown', (e) => {
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key) || e.altKey || e.metaKey || e.ctrlKey) return;
    const items = [...nav.querySelectorAll<HTMLButtonElement>('button.kf-desk-sidebar-item, button.shell-add')].filter(
      (b) => b.offsetParent !== null,
    );
    const i = items.indexOf(document.activeElement as HTMLButtonElement);
    if (i < 0) return;
    const next = e.key === 'Home' ? 0 : e.key === 'End' ? items.length - 1 : Math.min(items.length - 1, Math.max(0, i + (e.key === 'ArrowDown' ? 1 : -1)));
    items[next]?.focus();
    e.preventDefault();
  });
}
