import { describe, expect, it } from 'vitest';
import { acceptState, retryMinutes } from './agents';

describe('an accept waiting for its confirmation', () => {
  it('ends when the service accepted, or when it is no longer held or the window closed', () => {
    const deadline = 10_000;
    expect(acceptState('held', 1_000, deadline)).toBe('waiting');
    expect(acceptState('accepted', 1_000, deadline)).toBe('accepted');
    // Accepted just as the window closed still counts.
    expect(acceptState('accepted', deadline + 1, deadline)).toBe('accepted');
    expect(acceptState('held', deadline, deadline)).toBe('not_confirmed');
    // Reverted meanwhile (from the CLI, or this window).
    expect(acceptState('reverted', 1_000, deadline)).toBe('not_confirmed');
  });
});

describe('an accept asked for again too soon', () => {
  it('says how many whole minutes are left, at least one', () => {
    expect(retryMinutes(1)).toBe(1);
    expect(retryMinutes(60)).toBe(1);
    expect(retryMinutes(61)).toBe(2);
    expect(retryMinutes(30 * 60)).toBe(30);
    expect(retryMinutes(0)).toBe(1);
  });
});
