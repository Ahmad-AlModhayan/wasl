// What the tray's popover (macOS) and flyout (Windows) show, design D6 and D8 (#69): the overall state first, then
// the syncs in flight, what needs you, and the last few runs. Pure functions over the daemon's status, so the page
// (popover.ts) only lays them out.

import { appBanner, formatRelative, isLoud, pairStatus, pairsSummary, statusLabel, STATUS_STATES, type StatusState, type StatusView } from '../format';
import { translate, type Lang } from '../i18n';
import type { Route } from '../app';
import type { Conn, DaemonStatus, PairState, RunRecord, RunStatus } from '../types';
import { pairNeedsYou, statusNeedsYou } from '../screens/problemsModel';

/** The popover's window label (src-tauri/src/popover.rs). */
export const TRAY_LABEL = 'tray';

/**
 * Whether this page is the tray's popover: the Tauri window's label, or `?window=tray` in a plain browser (the dev
 * mock), so both surfaces can be looked at without the app.
 */
export function isTrayWindow(search: string, label: string | undefined): boolean {
  if (label !== undefined) return label === TRAY_LABEL;
  return new URLSearchParams(search).get('window') === TRAY_LABEL;
}

/** The first thing the popover says, and what a screen reader reads first. */
export interface Headline {
  view: StatusView;
  title: string;
  detail: string | null;
}

const calm = (state: StatusState, paused = false): StatusView => ({ state, paused, percent: null, count: 0 });

export function headline(conn: Conn, status: DaemonStatus | null, lang: Lang): Headline {
  if (conn.state !== 'connected' || !status) {
    const key =
      conn.state === 'starting' ? 'conn.starting' : conn.state === 'not_running' ? 'conn.not_running_title' : 'conn.connecting';
    return { view: calm(conn.state === 'not_running' ? 'failed' : 'waiting'), title: translate(lang, key), detail: null };
  }
  if (status.pairs.length === 0) return { view: calm('waiting'), title: translate(lang, 'overall.idle'), detail: null };
  const views = status.pairs.map(pairStatus);
  const detail = pairsSummary(status, lang);
  const running = views.filter((v) => v.state === 'syncing').length;
  if (running > 0) {
    return {
      view: calm('syncing'),
      title: translate(lang, 'shell.footer_syncing', { done: running, total: views.length }),
      detail,
    };
  }
  const banner = appBanner(status);
  // The same count as the Problems badge (#147 M1): agent changes alone need the person too.
  const loud = statusNeedsYou(status);
  if (banner || loud > 0) {
    return { view: calm(banner?.state ?? 'approval'), title: translate(lang, 'nav.problems_count', { count: Math.max(1, loud) }), detail };
  }
  if (views.every((v) => v.paused)) return { view: calm('waiting', true), title: translate(lang, 'overall.paused'), detail };
  if (views.every((v) => v.state === 'synced')) return { view: calm('synced'), title: translate(lang, 'tray.all_synced'), detail };
  return { view: calm('waiting'), title: translate(lang, 'overall.waiting'), detail };
}

/** Something that needs a decision, and the screen in the main window that answers it. */
export interface NeedItem {
  key: string;
  view: StatusView;
  title: string;
  detail: string;
  /** For `tray_open`: `problems`, `agents`, `settings` or `pair:<id>`. */
  route: string;
  /** The need-you items the row stands for (a pair's conflicts, each agent changeset). */
  count: number;
}

/** What "N more in Problems" says when the tray shows the first `shown` rows: need-you items, not rows. */
export function moreInProblems(status: DaemonStatus | null, items: readonly NeedItem[], shown: number): number {
  return Math.max(0, statusNeedsYou(status) - items.slice(0, shown).reduce((n, i) => n + i.count, 0));
}

/** A Drive sign-in, each pair in a loud state and agent changes waiting, most urgent first (D8's order). */
export function needsYou(status: DaemonStatus | null, lang: Lang): NeedItem[] {
  if (!status) return [];
  const items: NeedItem[] = [];
  for (const a of status.accounts) {
    if (a.auth_error) {
      items.push({ key: `account:${a.id}`, view: calm('failed'), title: translate(lang, 'banner.auth'), detail: a.email, route: 'settings', count: 1 });
    }
  }
  for (const p of status.pairs) {
    const v = pairStatus(p);
    if (!isLoud(v.state)) continue;
    items.push({
      key: `pair:${p.pair.id}`,
      view: v,
      title: p.pair.name,
      detail: statusLabel(v, lang),
      // Conflicts are answered side by side in Problems; a held deletion or a failure on the pair itself.
      route: v.state === 'conflict' ? 'problems' : `pair:${p.pair.id}`,
      count: Math.max(1, pairNeedsYou(p)),
    });
  }
  const agents = status.agent_changes ?? 0;
  if (agents > 0) {
    items.push({
      key: 'agents',
      view: calm('approval'),
      title: translate(lang, 'nav.agents'),
      detail: translate(lang, 'nav.agents_count', { count: agents }),
      route: 'agents',
      count: agents,
    });
  }
  const rank = (s: StatusState) => STATUS_STATES.indexOf(s);
  return items.map((item, i) => ({ item, i })).sort((a, b) => rank(a.item.view.state) - rank(b.item.view.state) || a.i - b.i).map((x) => x.item);
}

/** Pairs syncing now, for the in-flight cards. */
export function inFlight(status: DaemonStatus | null): PairState[] {
  return status ? status.pairs.filter((p) => p.running) : [];
}

/** Every pair paused: the popover offers Resume all. */
export function allPaused(status: DaemonStatus | null): boolean {
  return !!status && status.pairs.length > 0 && status.pairs.every((p) => p.pair.paused);
}

/** A finished run in the popover's short list. */
export interface RecentRun {
  id: string;
  name: string;
  view: StatusView;
  outcome: string;
  when: string;
}

const RUN_STATE: Partial<Record<RunStatus, StatusState>> = {
  SUCCESS: 'synced',
  PARTIAL: 'conflict', // as History (screens/history.ts): done, but some items failed
  FAILED: 'failed',
  BLOCKED: 'approval',
  CANCELLED: 'waiting',
};

/** The last few finished runs, newest first; runs that did not start are left out. */
export function recentRuns(runs: RunRecord[], status: DaemonStatus | null, now: number, lang: Lang, max = 3): RecentRun[] {
  const names = new Map((status?.pairs ?? []).map((p) => [p.pair.id, p.pair.name]));
  return runs
    .filter((r) => r.ended_at !== null && RUN_STATE[r.status] !== undefined)
    .sort((a, b) => (b.ended_at ?? 0) - (a.ended_at ?? 0))
    .slice(0, max)
    .map((r) => ({
      id: r.id,
      name: names.get(r.pair_id) ?? translate(lang, 'history.unknown_pair'),
      view: calm(RUN_STATE[r.status]!),
      outcome: translate(lang, `run_status.${r.status.toLowerCase()}`),
      when: formatRelative(r.ended_at ?? r.started_at, now, lang),
    }));
}

/** The main window's screen for a route the popover or the tray menu sent (`tray://open`). */
export function trayRoute(route: string): Route | null {
  if (route === 'problems' || route === 'agents' || route === 'settings' || route === 'devices') return { name: route };
  if (route.startsWith('pair:') && route.length > 5) return { name: 'pair', id: route.slice(5) };
  return null;
}
