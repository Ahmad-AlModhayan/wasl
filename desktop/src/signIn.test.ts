import { describe, expect, it, vi } from 'vitest';
import { SignIn } from './signIn';

describe('Add Google account from Settings and Problems (#147 M4)', () => {
  it('waits with the browser link and starts one sign-in however often it is clicked', async () => {
    const add = vi.fn(async () => 'https://accounts.google.com/o/oauth2/auth?x');
    const s = new SignIn(add, () => {});
    const seen: boolean[] = [];
    s.subscribe(() => seen.push(s.waiting));
    const first = s.start();
    expect(s.waiting).toBe(true);
    await s.start();
    await first;
    expect(add).toHaveBeenCalledTimes(1);
    expect(s.url).toBe('https://accounts.google.com/o/oauth2/auth?x');
    expect(seen[0]).toBe(true);
  });

  it('stops waiting when the account arrives or fails, and a new click starts again', async () => {
    const add = vi.fn(async () => 'u');
    const s = new SignIn(add, () => {});
    await s.start();
    s.event({ event: 'account_failed', message: 'denied' });
    expect(s.waiting).toBe(false);
    expect(s.url).toBe('');
    await s.start();
    s.event({ event: 'account_added', account: { id: 'me', email: 'me@example.com', auth_error: null } });
    expect(s.waiting).toBe(false);
    expect(add).toHaveBeenCalledTimes(2);
    expect(s.url).toBe('');
  });

  it('clears the stored address once the sign-in ends, even when the account arrives before the address (#205)', async () => {
    let give: (u: string) => void = () => {};
    const add = vi.fn(() => new Promise<string>((r) => (give = r)));
    const s = new SignIn(add, () => {});
    const first = s.start();
    await s.start();
    s.event({ event: 'account_added', account: { id: 'me', email: 'me@example.com', auth_error: null } });
    give('https://accounts.google.com/o/oauth2/auth?late');
    await first;
    expect(add).toHaveBeenCalledTimes(1);
    expect(s.waiting).toBe(false);
    expect(s.url).toBe('');
  });

  it('says why when the sign-in could not start, and lets the person try again', async () => {
    const errors: unknown[] = [];
    const s = new SignIn(async () => { throw new Error('no browser'); }, (e) => errors.push(e));
    await s.start();
    expect(s.waiting).toBe(false);
    expect(errors).toHaveLength(1);
  });
});
