// The All pairs table's columns and sorting (#64). Pure (no DOM, no Tauri), so it is unit-tested.
//
// Size is the folder's size at its last sync (folder_bytes, #103); a pair never synced, or an older daemon that
// doesn't report it, shows a dash and sorts last.

import { locale, translate, type Lang } from '../i18n';
import { methodKey, pairStatus, STATUS_STATES } from '../format';
import type { PairState } from '../types';

export const COLUMNS = ['name', 'status', 'direction', 'last', 'size'] as const;
export type Column = (typeof COLUMNS)[number];
export type SortDir = 'asc' | 'desc';

export interface Sort {
  by: Column;
  dir: SortDir;
}

export const DEFAULT_SORT: Sort = { by: 'name', dir: 'asc' };

/** A header's first click: names A to Z, the most urgent state first, the latest sync first, the largest first. */
export function firstDir(col: Column): SortDir {
  return col === 'last' || col === 'size' ? 'desc' : 'asc';
}

/** The folder's size at its last sync, or null when the daemon doesn't know it (never synced, or an older daemon). */
export function folderSize(s: PairState): number | null {
  return typeof s.folder_bytes === 'number' ? s.folder_bytes : null;
}

/** A click on a header: the same column flips, another column starts at its first direction. */
export function nextSort(sort: Sort, col: Column): Sort {
  if (sort.by === col) return { by: col, dir: sort.dir === 'asc' ? 'desc' : 'asc' };
  return { by: col, dir: firstDir(col) };
}

/** A stored sort, or the default when it isn't one. */
export function parseSort(raw: string | null): Sort {
  const [by, dir] = (raw ?? '').split(':');
  if (COLUMNS.includes(by as Column) && (dir === 'asc' || dir === 'desc')) return { by: by as Column, dir };
  return DEFAULT_SORT;
}

/** Where the pair's files go: the i18n key of its direction ("Two-way", "Upload only", "Devices only"). */
export function directionKey(s: PairState): string {
  return s.pair.route === 'devices' ? 'pairs.devices_only' : methodKey(s.pair.method);
}

/** Whether it also (or only) syncs straight with this person's devices: a bolt instead of a cloud. */
export function syncsWithDevices(s: PairState): boolean {
  return !!s.pair.route && s.pair.route !== 'drive';
}

/** When it last synced, for the Last sync column; null if never. A running pair is syncing now. */
export function lastSync(s: PairState, now: number): number | null {
  if (s.running) return now;
  return s.pair.status.last_success_at ?? s.pair.status.last_run_at ?? null;
}

/**
 * The pairs in the table's order. Ties fall back to the name, then the id, so the order never jumps on a live
 * update. Pairs never synced stay last in either direction of Last sync and Size.
 */
export function sortPairs(pairs: readonly PairState[], sort: Sort, lang: Lang, now: number): PairState[] {
  const collator = new Intl.Collator(locale(lang), { sensitivity: 'base', numeric: true });
  const byName = (a: PairState, b: PairState) => collator.compare(a.pair.name, b.pair.name) || (a.pair.id < b.pair.id ? -1 : a.pair.id > b.pair.id ? 1 : 0);
  const sign = sort.dir === 'asc' ? 1 : -1;
  const key = (s: PairState): number | string | null => {
    switch (sort.by) {
      case 'name':
        return null;
      case 'status':
        return STATUS_STATES.indexOf(pairStatus(s).state);
      case 'direction':
        return translate(lang, directionKey(s));
      case 'last':
        return lastSync(s, now);
      case 'size':
        return folderSize(s);
    }
  };
  return [...pairs].sort((a, b) => {
    if (sort.by === 'name') return sign * byName(a, b);
    const ka = key(a);
    const kb = key(b);
    if (ka === null && kb !== null) return 1;
    if (kb === null && ka !== null) return -1;
    if (ka !== null && kb !== null && ka !== kb) {
      const c = typeof ka === 'string' && typeof kb === 'string' ? collator.compare(ka, kb) : (ka as number) - (kb as number);
      if (c !== 0) return sign * c;
    }
    return byName(a, b);
  });
}

/** Only the pairs whose name holds the search text, ignoring case (as the sidebar does). */
export function filterPairs(pairs: readonly PairState[], query: string): PairState[] {
  const q = query.trim().toLocaleLowerCase();
  return q ? pairs.filter((p) => p.pair.name.toLocaleLowerCase().includes(q)) : [...pairs];
}

/** Type-to-select: the first row, from `from` on and wrapping, whose name starts with what was typed. */
export function typeAhead(names: readonly string[], typed: string, from: number, lang: Lang): number {
  const collator = new Intl.Collator(locale(lang), { sensitivity: 'base' });
  const n = names.length;
  for (let k = 0; k < n; k++) {
    const i = (from + k) % n;
    const name = names[i]!;
    if (collator.compare(name.slice(0, typed.length), typed) === 0) return i;
  }
  return -1;
}

/**
 * Where the arrows, Home and End take the focus among the columns that show (Direction hides in a narrow pane):
 * one column on or back, or the first or last. From a hidden column it counts from where that column sits.
 */
export function stepColumn(shown: readonly number[], at: number, to: 1 | -1 | 'first' | 'last'): number {
  if (shown.length === 0) return at;
  if (to === 'first') return shown[0]!;
  if (to === 'last') return shown[shown.length - 1]!;
  if (to === 1) return shown.find((c) => c > at) ?? shown[shown.length - 1]!;
  return [...shown].reverse().find((c) => c < at) ?? shown[0]!;
}

/** A column that shows: `at` itself, or the nearest before it (the first, when none is before it). */
export function shownColumn(shown: readonly number[], at: number): number {
  if (shown.length === 0 || shown.includes(at)) return at;
  return [...shown].reverse().find((c) => c < at) ?? shown[0]!;
}
