import { describe, expect, it } from 'vitest';
import type { BinEntry } from '../types';
import { binFolder, binName, binSummary, failedSummary, filterBin, rebinned, restoreFailures, sortBin } from './binModel';

function entry(id: string, key: string, deleted: number, patch: Partial<BinEntry['entry']> = {}): BinEntry {
  return {
    entry: { id, pair_id: 'p', side: 'Local', key, reason: '', location: '', deleted_at: deleted, expires_at: deleted + 1000, size: 10, display_name: null, pair_name: 'Documents', ...patch },
    version_no: 1,
  };
}

describe('the recycle bin table', () => {
  const a = entry('a', 'Finance/budget.xlsx', 300, { size: 2000, pair_name: 'Documents' });
  const b = entry('b', 'notes.txt', 100, { pair_name: 'LifeOS', expires_at: 150 });
  const c = entry('c', 'Drafts/intro.md', 300, { display_name: 'Intro.md' });

  it('names a file by its display name, else the last part of its path, and finds its folder', () => {
    expect(binName(a)).toBe('budget.xlsx');
    expect(binName(c)).toBe('Intro.md');
    expect(binFolder(a)).toBe('Finance');
    expect(binFolder(b)).toBe('');
  });

  it('lists the newest deletions first, ties by name', () => {
    expect(sortBin([b, c, a], 'en').map((x) => x.entry.id)).toEqual(['a', 'c', 'b']);
  });

  it('keeps what matches every word of a search in the name, folder or pair, ignoring case', () => {
    expect(filterBin([a, b, c], 'BUDGET', 'en').map((x) => x.entry.id)).toEqual(['a']);
    expect(filterBin([a, b, c], 'lifeos notes', 'en').map((x) => x.entry.id)).toEqual(['b']);
    expect(filterBin([a, b, c], 'drafts', 'en').map((x) => x.entry.id)).toEqual(['c']);
    expect(filterBin([a, b, c], '  ', 'en')).toHaveLength(3);
    expect(filterBin([a, b, c], 'nothing', 'en')).toEqual([]);
  });

  it('adds up a selection and says when the first of it leaves the bin', () => {
    expect(binSummary([a, b])).toEqual({ count: 2, bytes: 2010, keptUntil: 150 });
    expect(binSummary([])).toEqual({ count: 0, bytes: 0, keptUntil: null });
  });

  it('names what a restore could not bring back', () => {
    const result = { restored: [a], failed: [{ entry_id: 'b', message: 'Folder is gone' }, { entry_id: 'zz', message: 'x' }] };
    expect(restoreFailures(result, [a, b])).toEqual([{ name: 'notes.txt', message: 'Folder is gone' }, { name: 'zz', message: 'x' }]);
  });

  it('names every failure up to three, else the first two and how many more, with the reason they share', () => {
    const f = (name: string, message = 'Drive is offline') => ({ name, message });
    expect(failedSummary([f('a'), f('b'), f('c')])).toEqual({ names: ['a', 'b', 'c'], more: 0, message: 'Drive is offline' });
    expect(failedSummary([f('a'), f('b'), f('c'), f('d'), f('e')])).toEqual({ names: ['a', 'b'], more: 3, message: 'Drive is offline' });
    expect(failedSummary([f('a'), f('b', 'Folder is gone')]).message).toBeNull();
  });

  it('finds the files an undone restore put back under their new ids, the newest of each', () => {
    const older = entry('old', 'notes.txt', 50, { pair_name: 'LifeOS' });
    const back = entry('new-b', 'notes.txt', 900, { pair_name: 'LifeOS' });
    const backA = entry('new-a', 'Finance/budget.xlsx', 900);
    const otherSide = entry('drive-a', 'Finance/budget.xlsx', 950, { side: 'Drive' });
    expect(rebinned([a, b], [older, back, otherSide, backA, c])).toEqual(['new-a', 'new-b']);
    expect(rebinned([a], [c])).toEqual([]);
  });
});
