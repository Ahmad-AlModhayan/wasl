import { describe, expect, it } from 'vitest';
import type { PairState } from '../types';
import { DEFAULT_SORT, directionKey, filterPairs, folderSize, lastSync, nextSort, parseSort, shownColumn, sortPairs, stepColumn, typeAhead } from './pairsModel';

const NOW = 1_800_000_000_000;

function pair(id: string, name: string, patch: (p: PairState) => void = () => undefined): PairState {
  const p: PairState = {
    pair: {
      id,
      name,
      account_id: 'acc',
      local_root: `/x/${id}`,
      drive_folder_id: null,
      remote_path: `My Drive/${id}`,
      method: 'TWO_WAY',
      route: 'drive',
      enabled: true,
      paused: false,
      pause_reason: null,
      status: {
        last_run_at: NOW - 60_000,
        last_success_at: NOW - 60_000,
        last_status: 'SUCCESS',
        last_error: null,
        last_error_kind: null,
        consecutive_failures: 0,
        pending_approval: false,
        approve_once: false,
        hold_for_preview: false,
      },
    },
    running: false,
    progress: null,
    open_conflicts: 0,
    blocked: false,
  };
  patch(p);
  return p;
}

const pairs = [
  pair('docs', 'Documents', (p) => (p.pair.status.last_success_at = NOW - 4 * 60_000)),
  pair('life', 'LifeOS', (p) => {
    p.open_conflicts = 1;
    p.pair.status.last_success_at = NOW - 12 * 60_000;
  }),
  pair('photos', 'Photos', (p) => {
    p.pair.method = 'UPLOAD_ONLY';
    p.pair.status.pending_approval = true;
    p.pair.status.last_success_at = NOW - 3 * 3600_000;
  }),
  pair('site', 'kahf-site', (p) => {
    p.pair.route = 'drive_and_devices';
    p.running = true;
  }),
  pair('wasl', 'wasl', (p) => {
    p.pair.route = 'devices';
    p.pair.status.last_status = null;
    p.pair.status.last_success_at = null;
    p.pair.status.last_run_at = null;
  }),
];

const names = (list: PairState[]) => list.map((p) => p.pair.id);

describe('the All pairs table', () => {
  it('sorts by name, ignoring case, and reverses on the second click', () => {
    expect(names(sortPairs(pairs, { by: 'name', dir: 'asc' }, 'en', NOW))).toEqual(['docs', 'site', 'life', 'photos', 'wasl']);
    expect(names(sortPairs(pairs, { by: 'name', dir: 'desc' }, 'en', NOW))).toEqual(['wasl', 'photos', 'life', 'site', 'docs']);
  });

  it('sorts by state with the most urgent first', () => {
    // Needs approval, then conflict, then syncing, then waiting, then synced.
    expect(names(sortPairs(pairs, { by: 'status', dir: 'asc' }, 'en', NOW))).toEqual(['photos', 'life', 'site', 'wasl', 'docs']);
  });

  it('sorts by direction by its words, ties by name', () => {
    expect(names(sortPairs(pairs, { by: 'direction', dir: 'asc' }, 'en', NOW))).toEqual(['wasl', 'docs', 'site', 'life', 'photos']);
  });

  it('sorts by last sync with the latest first and never-synced pairs last either way', () => {
    expect(names(sortPairs(pairs, { by: 'last', dir: 'desc' }, 'en', NOW))).toEqual(['site', 'docs', 'life', 'photos', 'wasl']);
    expect(names(sortPairs(pairs, { by: 'last', dir: 'asc' }, 'en', NOW))).toEqual(['photos', 'life', 'docs', 'site', 'wasl']);
  });

  it('keeps the same order for equal keys, so a live update never shuffles rows', () => {
    const twins = [pair('b', 'Same'), pair('a', 'Same')];
    expect(names(sortPairs(twins, { by: 'status', dir: 'asc' }, 'en', NOW))).toEqual(['a', 'b']);
    expect(names(sortPairs(twins, { by: 'status', dir: 'desc' }, 'en', NOW))).toEqual(['a', 'b']);
  });

  it('flips the same column and starts another at its first direction', () => {
    expect(nextSort(DEFAULT_SORT, 'name')).toEqual({ by: 'name', dir: 'desc' });
    expect(nextSort(DEFAULT_SORT, 'last')).toEqual({ by: 'last', dir: 'desc' });
    expect(nextSort({ by: 'last', dir: 'desc' }, 'status')).toEqual({ by: 'status', dir: 'asc' });
  });

  it('reads a stored sort and falls back to the name for anything else', () => {
    expect(parseSort('last:desc')).toEqual({ by: 'last', dir: 'desc' });
    expect(parseSort('size:asc')).toEqual({ by: 'size', dir: 'asc' });
    expect(parseSort('bytes:asc')).toEqual(DEFAULT_SORT);
    expect(parseSort(null)).toEqual(DEFAULT_SORT);
  });

  it('names the direction, with devices-only pairs as such', () => {
    expect(directionKey(pairs[0]!)).toBe('method.two_way');
    expect(directionKey(pairs[2]!)).toBe('method.upload_only');
    expect(directionKey(pairs[4]!)).toBe('pairs.devices_only');
  });

  it('counts a running pair as syncing now, and a never-synced pair as never', () => {
    expect(lastSync(pairs[3]!, NOW)).toBe(NOW);
    expect(lastSync(pairs[4]!, NOW)).toBeNull();
  });

  it('filters by the search text, ignoring case', () => {
    expect(names(filterPairs(pairs, ' LIFE '))).toEqual(['life']);
    expect(names(filterPairs(pairs, ''))).toEqual(names(pairs));
  });

  it('jumps to the next name starting with what was typed, wrapping around', () => {
    const list = ['Documents', 'kahf-site', 'LifeOS', 'Lists', 'wasl'];
    expect(typeAhead(list, 'l', 0, 'en')).toBe(2);
    expect(typeAhead(list, 'l', 3, 'en')).toBe(3);
    expect(typeAhead(list, 'l', 4, 'en')).toBe(2);
    expect(typeAhead(list, 'lis', 0, 'en')).toBe(3);
    expect(typeAhead(list, 'z', 0, 'en')).toBe(-1);
  });

  it('moves the focus over the columns that show, skipping Direction when the pane is narrow', () => {
    const narrow = [0, 1, 3];
    expect(stepColumn(narrow, 1, 1)).toBe(3);
    expect(stepColumn(narrow, 3, -1)).toBe(1);
    expect(stepColumn(narrow, 3, 1)).toBe(3);
    expect(stepColumn(narrow, 0, -1)).toBe(0);
    expect(stepColumn(narrow, 1, 'last')).toBe(3);
    expect(stepColumn(narrow, 3, 'first')).toBe(0);
    // From Direction, focused before the pane narrowed.
    expect(stepColumn(narrow, 2, 1)).toBe(3);
    expect(stepColumn(narrow, 2, -1)).toBe(1);
    expect(stepColumn([0, 1, 2, 3], 1, 1)).toBe(2);
  });

  it('keeps the tab stop on a column that shows', () => {
    expect(shownColumn([0, 1, 3], 2)).toBe(1);
    expect(shownColumn([0, 1, 3], 3)).toBe(3);
    expect(shownColumn([1, 3], 0)).toBe(1);
    expect(shownColumn([], 2)).toBe(2);
  });
});

describe('the Size column', () => {
  const sized = [
    pair('a', 'Alpha', (p) => (p.folder_bytes = 5_000)),
    pair('b', 'Beta', (p) => (p.folder_bytes = 9_000_000)),
    pair('c', 'Gamma'),
    pair('d', 'Delta', (p) => (p.folder_bytes = null)),
  ];

  it('starts with the largest folder first', () => {
    expect(nextSort(DEFAULT_SORT, 'size')).toEqual({ by: 'size', dir: 'desc' });
    expect(names(sortPairs(sized, { by: 'size', dir: 'desc' }, 'en', NOW))).toEqual(['b', 'a', 'd', 'c']);
  });

  it('keeps pairs of unknown size last in either direction, by name', () => {
    expect(names(sortPairs(sized, { by: 'size', dir: 'asc' }, 'en', NOW))).toEqual(['a', 'b', 'd', 'c']);
  });

  it("knows no size from an older daemon or before the first sync, and doesn't make one up", () => {
    expect(folderSize(sized[2]!)).toBeNull();
    expect(folderSize(sized[3]!)).toBeNull();
    expect(folderSize(pair('z', 'Zero', (p) => (p.folder_bytes = 0)))).toBe(0);
  });
});
