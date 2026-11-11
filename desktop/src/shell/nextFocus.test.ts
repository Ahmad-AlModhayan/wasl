import { describe, expect, it } from 'vitest';
import { mayTakeFocus, nextAfter } from './nextFocus';

describe('the row after rows leave', () => {
  it('is the first one after the last that left', () => {
    expect(nextAfter(['a', 'b', 'c', 'd'], ['b', 'c'])).toBe('d');
  });

  it('is the one before when the last rows left', () => {
    expect(nextAfter(['a', 'b', 'c'], ['c'])).toBe('b');
  });

  it('is nothing when every row left or none did', () => {
    expect(nextAfter(['a', 'b'], ['a', 'b'])).toBeNull();
    expect(nextAfter(['a', 'b'], ['z'])).toBeNull();
  });
});

describe('moving the focus on after an action', () => {
  it('takes the focus from the heading, the page, or nothing', () => {
    expect(mayTakeFocus({ tag: 'H1', key: null }, 'bin-restore')).toBe(true);
    expect(mayTakeFocus({ tag: 'BODY', key: null }, null)).toBe(true);
    expect(mayTakeFocus(null, null)).toBe(true);
  });

  it('takes the focus from the button that was pressed, handed back by the re-render', () => {
    expect(mayTakeFocus({ tag: 'BUTTON', key: 'bin-restore' }, 'bin-restore')).toBe(true);
    expect(mayTakeFocus({ tag: 'BUTTON', key: 'bin-delete' }, 'bin-delete')).toBe(true);
  });

  it('leaves the focus where the person moved it', () => {
    expect(mayTakeFocus({ tag: 'INPUT', key: 'search' }, 'bin-restore')).toBe(false);
    expect(mayTakeFocus({ tag: 'BUTTON', key: 'bin-delete' }, 'bin-restore')).toBe(false);
    expect(mayTakeFocus({ tag: 'BUTTON', key: null }, null)).toBe(false);
  });
});
