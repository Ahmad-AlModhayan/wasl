import { describe, expect, it } from 'vitest';
import type { RunRecord, RunStatus } from '../types';
import { filterRuns, filterShowing, isFiltered, NO_FILTER, oncePer, rangeStart, runSeconds } from './historyModel';

const HOUR = 3_600_000;
const now = new Date(2026, 10, 1, 15, 0, 0).getTime();

function run(id: string, pair: string, status: RunStatus, trigger: string, hoursAgo: number, message: string | null = null): RunRecord {
  const stats = { uploaded: 0, downloaded: 0, exported: 0, deleted_local: 0, deleted_remote: 0, conflicts: 0, skipped: 0, errors: 0, bytes_up: 0, bytes_down: 0 };
  return { id, pair_id: pair, trigger, status, stats, message, started_at: now - hoursAgo * HOUR, ended_at: now - hoursAgo * HOUR + 4000 };
}

const runs = [
  run('1', 'docs', 'SUCCESS', 'instant', 1),
  run('2', 'life', 'PARTIAL', 'schedule', 20, 'One file changed on both sides'),
  run('3', 'docs', 'FAILED', 'manual', 30),
  run('4', 'photos', 'BLOCKED', 'schedule', 24 * 10),
  run('5', 'docs', 'SUCCESS', 'schedule', 24 * 40),
];
const ids = (rs: RunRecord[]) => rs.map((r) => r.id);

describe('History filters', () => {
  it('keep everything with no filter set', () => {
    expect(ids(filterRuns(runs, NO_FILTER, now))).toEqual(['1', '2', '3', '4', '5']);
    expect(isFiltered(NO_FILTER)).toBe(false);
  });

  it('filter by pair, outcome, trigger and date, each on its own', () => {
    expect(ids(filterRuns(runs, { ...NO_FILTER, pair: 'docs' }, now))).toEqual(['1', '3', '5']);
    expect(ids(filterRuns(runs, { ...NO_FILTER, outcome: 'SUCCESS' }, now))).toEqual(['1', '5']);
    expect(ids(filterRuns(runs, { ...NO_FILTER, trigger: 'schedule' }, now))).toEqual(['2', '4', '5']);
    expect(ids(filterRuns(runs, { ...NO_FILTER, date: 'today' }, now))).toEqual(['1']);
    expect(ids(filterRuns(runs, { ...NO_FILTER, date: 'week' }, now))).toEqual(['1', '2', '3']);
    expect(ids(filterRuns(runs, { ...NO_FILTER, date: 'month' }, now))).toEqual(['1', '2', '3', '4']);
  });

  it('combine: every filter set must hold', () => {
    const f = { pair: 'docs', outcome: 'SUCCESS' as const, trigger: 'schedule', date: 'any' as const };
    expect(ids(filterRuns(runs, f, now))).toEqual(['5']);
    expect(ids(filterRuns(runs, { ...f, date: 'month' }, now))).toEqual([]);
    expect(isFiltered(f)).toBe(true);
  });

  it('add the search field: words in the pair name, the message or the trigger', () => {
    const names = new Map([['docs', 'Documents'], ['life', 'LifeOS'], ['photos', 'Photos']]);
    const search = (query: string) => ({ query, names, words: (t: string) => (t === 'schedule' ? 'Scheduled' : t), lang: 'en' });
    expect(ids(filterRuns(runs, NO_FILTER, now, search('lifeos')))).toEqual(['2']);
    expect(ids(filterRuns(runs, NO_FILTER, now, search('both sides')))).toEqual(['2']);
    expect(ids(filterRuns(runs, { ...NO_FILTER, pair: 'docs' }, now, search('scheduled')))).toEqual(['5']);
  });

  it('start today at local midnight', () => {
    expect(rangeStart('today', now)).toBe(new Date(2026, 10, 1).getTime());
    expect(rangeStart('any', now)).toBeNull();
  });

  it('say how long a run took, or nothing while it runs', () => {
    expect(runSeconds(runs[0]!)).toBe(4);
    expect(runSeconds({ ...runs[0]!, ended_at: null })).toBeNull();
  });
});

describe('opening History on one sync', () => {
  it('keeps the filters when they show that sync', () => {
    const f = { ...NO_FILTER, pair: 'docs' };
    expect(filterShowing(runs, f, '3', now)).toBe(f);
  });

  it('clears the filters when they hide that sync', () => {
    expect(filterShowing(runs, { ...NO_FILTER, outcome: 'FAILED' }, '1', now)).toEqual(NO_FILTER);
  });

  it('applies a route once, so live updates keep the selection and the filters', () => {
    const first = oncePer<object>();
    const route = { name: 'run', id: '3' };
    expect(first(route)).toBe(true);
    expect(first(route)).toBe(false);
    expect(first(route)).toBe(false);
    // Opening the same sync again from Pair detail is a new route: it applies again.
    expect(first({ name: 'run', id: '3' })).toBe(true);
  });
});
