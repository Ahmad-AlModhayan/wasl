// History's filters (#67), pure: pair, outcome, what started the sync, and when. They combine, and they work on
// any list of runs, so History can later read the op-log (#48) instead of the daemon's run table.

import type { RunRecord, RunStatus } from '../types';

export type DateRange = 'any' | 'today' | 'week' | 'month';
export const DATE_RANGES: readonly DateRange[] = ['any', 'today', 'week', 'month'];
export const OUTCOMES: readonly RunStatus[] = ['SUCCESS', 'PARTIAL', 'FAILED', 'BLOCKED', 'CANCELLED', 'SKIPPED', 'RUNNING'];
export const TRIGGERS = ['manual', 'schedule', 'instant', 'preview', 'automation'] as const;

export interface HistoryFilter {
  /** A pair's id; null for every pair. */
  pair: string | null;
  outcome: RunStatus | null;
  trigger: string | null;
  date: DateRange;
}

export const NO_FILTER: HistoryFilter = { pair: null, outcome: null, trigger: null, date: 'any' };

export function isFiltered(f: HistoryFilter): boolean {
  return f.pair !== null || f.outcome !== null || f.trigger !== null || f.date !== 'any';
}

const DAY = 86_400_000;

/** The earliest start a date range keeps (Unix ms); null for any time. Today starts at local midnight. */
export function rangeStart(range: DateRange, now: number): number | null {
  switch (range) {
    case 'any':
      return null;
    case 'today': {
      const d = new Date(now);
      d.setHours(0, 0, 0, 0);
      return d.getTime();
    }
    case 'week':
      return now - 7 * DAY;
    case 'month':
      return now - 30 * DAY;
  }
}

/**
 * The runs every filter keeps, plus the search field's words (in the pair's name, the run's message or its
 * trigger's words). `names` gives each pair's name; `words` each trigger's words in the window's language.
 */
export function filterRuns(
  runs: readonly RunRecord[],
  f: HistoryFilter,
  now: number,
  search: { query: string; names: ReadonlyMap<string, string>; words: (trigger: string) => string; lang: string } | null = null,
): RunRecord[] {
  const from = rangeStart(f.date, now);
  const words = search ? search.query.trim().toLocaleLowerCase(search.lang).split(/\s+/).filter(Boolean) : [];
  return runs.filter((r) => {
    if (f.pair !== null && r.pair_id !== f.pair) return false;
    if (f.outcome !== null && r.status !== f.outcome) return false;
    if (f.trigger !== null && r.trigger !== f.trigger) return false;
    if (from !== null && r.started_at < from) return false;
    if (words.length && search) {
      const text = [search.names.get(r.pair_id) ?? '', r.message ?? '', search.words(r.trigger)].join('\n').toLocaleLowerCase(search.lang);
      if (!words.every((w) => text.includes(w))) return false;
    }
    return true;
  });
}

/** How long a run took, in seconds; null while it runs. */
export function runSeconds(r: RunRecord): number | null {
  return r.ended_at === null ? null : Math.max(0, Math.round((r.ended_at - r.started_at) / 1000));
}

/**
 * History opened on one sync (the `run` route, from Pair detail's recent syncs): the filters stay, unless they
 * hide that sync; then they clear.
 */
export function filterShowing(
  runs: readonly RunRecord[],
  f: HistoryFilter,
  runId: string,
  now: number,
  search: Parameters<typeof filterRuns>[3] = null,
): HistoryFilter {
  return filterRuns(runs, f, now, search).some((r) => r.id === runId) ? f : NO_FILTER;
}

/**
 * True the first time it is given `key`, false after that. History applies a `run` route once (the route object
 * is new on each navigation), so live updates keep the person's own selection, filters and focus.
 */
export function oncePer<K extends object>(): (key: K) => boolean {
  const seen = new WeakSet<K>();
  return (key) => {
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  };
}
