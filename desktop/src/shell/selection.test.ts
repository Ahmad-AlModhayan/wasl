import { describe, expect, it } from 'vitest';
import { extendTo, move, NO_SELECTION, prune, sameIds, selectAll, selectOne, toggle } from './selection';

const order = ['a', 'b', 'c', 'd', 'e'];

describe('a list selection', () => {
  it('selects one row on a plain click and anchors there', () => {
    expect(selectOne('c')).toEqual({ ids: ['c'], anchor: 'c', focus: 'c' });
  });

  it('adds and removes rows with a toggle click, keeping the list order', () => {
    let s = selectOne('d');
    s = toggle(s, order, 'b');
    expect(s.ids).toEqual(['b', 'd']);
    s = toggle(s, order, 'd');
    expect(s.ids).toEqual(['b']);
    expect(s.anchor).toBe('d');
  });

  it('selects the range from the anchor on a Shift click, in either direction', () => {
    const s = selectOne('b');
    expect(extendTo(s, order, 'd').ids).toEqual(['b', 'c', 'd']);
    expect(extendTo(s, order, 'a').ids).toEqual(['a', 'b']);
    // The anchor stays, so a second Shift click replaces the range instead of growing it.
    expect(extendTo(extendTo(s, order, 'e'), order, 'c').ids).toEqual(['b', 'c']);
  });

  it('starts a range at the clicked row when there is no anchor yet', () => {
    expect(extendTo(NO_SELECTION, order, 'c')).toEqual({ ids: ['c'], anchor: 'c', focus: 'c' });
  });

  it('selects every row with select all', () => {
    expect(selectAll(order).ids).toEqual(order);
    expect(selectAll([])).toEqual(NO_SELECTION);
  });

  it('moves the selection with the arrows and stops at the ends', () => {
    let s = selectOne('a');
    s = move(s, order, 1, 'select');
    expect(s).toEqual(selectOne('b'));
    expect(move(selectOne('e'), order, 1, 'select').ids).toEqual(['e']);
    expect(move(selectOne('a'), order, -1, 'select').ids).toEqual(['a']);
    expect(move(selectOne('c'), order, 'last', 'select').ids).toEqual(['e']);
    expect(move(selectOne('c'), order, 'first', 'select').ids).toEqual(['a']);
  });

  it('starts at the first or last row when nothing has the focus', () => {
    expect(move(NO_SELECTION, order, 1, 'select').ids).toEqual(['a']);
    expect(move(NO_SELECTION, order, -1, 'select').ids).toEqual(['e']);
  });

  it('extends the range with Shift and the arrows', () => {
    let s = selectOne('b');
    s = move(s, order, 1, 'extend');
    s = move(s, order, 1, 'extend');
    expect(s.ids).toEqual(['b', 'c', 'd']);
    s = move(s, order, -3, 'extend');
    expect(s.ids).toEqual(['a', 'b']);
  });

  it('moves only the focus with the toggle key held, then toggles the focused row', () => {
    let s = selectOne('a');
    s = move(s, order, 2, 'focus');
    expect(s.ids).toEqual(['a']);
    expect(s.focus).toBe('c');
    s = toggle(s, order, s.focus!);
    expect(s.ids).toEqual(['a', 'c']);
  });

  it('drops rows that left the list and keeps the rest', () => {
    const s = prune({ ids: ['b', 'd'], anchor: 'd', focus: 'd' }, ['a', 'b', 'c']);
    expect(s).toEqual({ ids: ['b'], anchor: null, focus: 'b' });
    expect(prune(selectOne('z'), order)).toEqual(NO_SELECTION);
  });

  it('compares id lists in order', () => {
    expect(sameIds(['a', 'b'], ['a', 'b'])).toBe(true);
    expect(sameIds(['a', 'b'], ['b', 'a'])).toBe(false);
    expect(sameIds([], ['a'])).toBe(false);
  });
});
