// The Recycle bin's table (#67), pure: what a search keeps, what the selection adds up to, and what a restore
// failed to bring back, by name.

import type { BinEntry, RestoreResult } from '../types';

export function binName(b: BinEntry): string {
  return b.entry.display_name ?? b.entry.key.split('/').pop() ?? b.entry.key;
}

/** The folder the file was in, inside its pair ("" at the pair's top). */
export function binFolder(b: BinEntry): string {
  const k = b.entry.key;
  const i = k.lastIndexOf('/');
  return i < 0 ? '' : k.slice(0, i);
}

/** Newest deletions first; ties by name, then id, so live updates never shuffle rows. */
export function sortBin(entries: readonly BinEntry[], lang: string): BinEntry[] {
  return [...entries].sort(
    (a, b) => b.entry.deleted_at - a.entry.deleted_at || binName(a).localeCompare(binName(b), lang) || a.entry.id.localeCompare(b.entry.id),
  );
}

/** ⌘F / Ctrl+F: entries whose name, folder or pair contains every word typed, ignoring case. */
export function filterBin(entries: readonly BinEntry[], query: string, lang: string): BinEntry[] {
  const words = query.trim().toLocaleLowerCase(lang).split(/\s+/).filter(Boolean);
  if (words.length === 0) return [...entries];
  return entries.filter((b) => {
    const text = [binName(b), b.entry.key, b.entry.pair_name ?? ''].join('\n').toLocaleLowerCase(lang);
    return words.every((w) => text.includes(w));
  });
}

export interface BinSummary {
  count: number;
  bytes: number;
  /** The soonest any of them leaves the bin for good; null for none. */
  keptUntil: number | null;
}

export function binSummary(entries: readonly BinEntry[]): BinSummary {
  return {
    count: entries.length,
    bytes: entries.reduce((n, b) => n + b.entry.size, 0),
    keptUntil: entries.length ? Math.min(...entries.map((b) => b.entry.expires_at)) : null,
  };
}

/** What a restore couldn't bring back, by the file's name (the entry's id when it's no longer listed). */
export function restoreFailures(result: RestoreResult, entries: readonly BinEntry[]): Array<{ name: string; message: string }> {
  const byId = new Map(entries.map((b) => [b.entry.id, b]));
  return result.failed.map((f) => {
    const b = byId.get(f.entry_id);
    return { name: b ? binName(b) : f.entry_id, message: f.message };
  });
}

/**
 * A partial restore's failures for its toast: every name when there are up to three, else the first two and how
 * many more; the reason when they all share one.
 */
export function failedSummary(fails: ReadonlyArray<{ name: string; message: string }>): { names: string[]; more: number; message: string | null } {
  const names = fails.map((f) => f.name);
  const shown = names.length <= 3 ? names : names.slice(0, 2);
  const message = fails.length && fails.every((f) => f.message === fails[0]!.message) ? fails[0]!.message : null;
  return { names: shown, more: names.length - shown.length, message };
}

const slot = (b: BinEntry) => `${b.entry.pair_id}\n${b.entry.side}\n${b.entry.key}`;

/**
 * The entries an undone restore put back in the bin. wasld bins the files again under new ids, so each is found by
 * its pair, side and key: the newest entry there. In the order of `restored`; files not back are left out.
 */
export function rebinned(restored: readonly BinEntry[], now: readonly BinEntry[]): string[] {
  const want = new Set(restored.map(slot));
  const newest = new Map<string, BinEntry>();
  for (const b of now) {
    const k = slot(b);
    if (!want.has(k)) continue;
    const have = newest.get(k);
    if (!have || b.entry.deleted_at > have.entry.deleted_at || (b.entry.deleted_at === have.entry.deleted_at && b.version_no > have.version_no)) newest.set(k, b);
  }
  return [...new Set(restored.map((b) => newest.get(slot(b))?.entry.id).filter((id): id is string => id !== undefined))];
}
