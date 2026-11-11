import { describe, expect, it } from 'vitest';
import { alertScope, appAlert, FIRST_RUN_STEPS, firstRunDecision, firstRunStart, firstRunStepKey, showFirstRun } from './appState';
import en from './i18n/en.json';
import ar from './i18n/ar.json';
import { errorKind, failureText } from './format';
import type { Account, Conn, DaemonStatus, PairState } from './types';

function pair(id: string, status: Partial<PairState['pair']['status']> = {}, over: Partial<PairState['pair']> = {}): PairState {
  return {
    running: false,
    progress: null,
    open_conflicts: 0,
    blocked: false,
    pair: {
      id,
      name: id,
      account_id: 'a',
      local_root: `/Users/me/${id}`,
      drive_folder_id: 'f',
      remote_path: `My Drive/${id}`,
      method: 'TWO_WAY',
      enabled: true,
      paused: false,
      pause_reason: null,
      ...over,
      status: {
        last_run_at: null,
        last_success_at: null,
        last_status: 'SUCCESS',
        last_error: null,
        last_error_kind: null,
        consecutive_failures: 0,
        pending_approval: false,
        approve_once: false,
        hold_for_preview: false,
        ...status,
      },
    },
  };
}

const failed = (id: string, kind: string) => pair(id, { last_status: 'FAILED', last_error_kind: kind, last_error: 'x' });
const status = (pairs: PairState[], accounts: Account[] = [{ id: 'a', email: 'me@example.com', auth_error: null }]): DaemonStatus => ({
  version: '0.3.3',
  protocol: 1,
  accounts,
  pairs,
  overall: 'up_to_date',
});
const connected: Conn = { state: 'connected', version: '0.3.3' };

describe('appAlert', () => {
  it('says the service stopped, and whether it could not be found at all', () => {
    expect(appAlert({ conn: { state: 'not_running', message: null }, status: null, online: true })).toEqual({ kind: 'stopped', notFound: false, message: null });
    expect(appAlert({ conn: { state: 'not_running', message: 'wasld_not_found' }, status: null, online: true })).toEqual({ kind: 'stopped', notFound: true, message: null });
    expect(appAlert({ conn: { state: 'not_running', message: 'exit 1' }, status: null, online: false })).toEqual({ kind: 'stopped', notFound: false, message: 'exit 1' });
  });

  it('shows starting after Restart service, and nothing while first connecting', () => {
    expect(appAlert({ conn: { state: 'starting' }, status: null, online: true })).toEqual({ kind: 'starting' });
    expect(appAlert({ conn: { state: 'connecting' }, status: null, online: false })).toBeNull();
  });

  it('puts offline above everything the service reports', () => {
    const s = status([failed('p', 'AUTH')], [{ id: 'a', email: 'me@example.com', auth_error: 'invalid_grant' }]);
    expect(appAlert({ conn: connected, status: s, online: false })).toEqual({ kind: 'offline' });
  });

  it('counts the Drive pairs a signed-out account stopped, not the device-only ones', () => {
    const s = status(
      [pair('a1'), pair('a2'), pair('dev', {}, { route: 'devices' }), pair('other', {}, { account_id: 'b' })],
      [{ id: 'a', email: 'me@example.com', auth_error: 'invalid_grant' }, { id: 'b', email: 'you@example.com', auth_error: null }],
    );
    expect(appAlert({ conn: connected, status: s, online: true })).toEqual({ kind: 'auth', count: 2 });
  });

  it('names Google Drive full, with the pair when only one is affected', () => {
    const one = appAlert({ conn: connected, status: status([failed('p1', 'QUOTA'), pair('p2')]), online: true });
    expect(one).toMatchObject({ kind: 'quota', count: 1, pair: { pair: { id: 'p1' } } });
    const two = appAlert({ conn: connected, status: status([failed('p1', 'QUOTA'), failed('p2', 'Quota')]), online: true });
    expect(two).toMatchObject({ kind: 'quota', count: 2, pair: null });
  });

  it('names a Drive folder that is gone before the other failures', () => {
    const a = appAlert({ conn: connected, status: status([failed('p1', 'NOT_FOUND'), failed('p2', 'IO')]), online: true });
    expect(a).toMatchObject({ kind: 'missing', count: 1, pair: { pair: { id: 'p1' } } });
  });

  it('falls back to the pairs’ own loud state, and to nothing when all is well', () => {
    expect(appAlert({ conn: connected, status: status([failed('p1', 'IO')]), online: true })).toMatchObject({ kind: 'pairs', banner: { state: 'failed', count: 1 } });
    expect(appAlert({ conn: connected, status: status([pair('p1')]), online: true })).toBeNull();
  });

  it('shows the app-wide states on every screen and the pairs’ own on All pairs', () => {
    expect(alertScope({ kind: 'offline' })).toBe('app');
    expect(alertScope({ kind: 'auth', count: 1 })).toBe('app');
    expect(alertScope({ kind: 'missing', count: 1, pair: null })).toBe('pairs');
    expect(alertScope({ kind: 'pairs', banner: { state: 'conflict', count: 1, auth: false } })).toBe('pairs');
  });
});

describe('errorKind', () => {
  it('reads the service’s spellings as one', () => {
    expect(errorKind('NOT_FOUND')).toBe('NotFound');
    expect(errorKind('not_found')).toBe('NotFound');
    expect(errorKind('NotFound')).toBe('NotFound');
    expect(errorKind('AUTH')).toBe('Auth');
    expect(errorKind('IO')).toBe('Io');
    expect(errorKind('RATE_LIMIT')).toBe('RateLimit');
    expect(errorKind(null)).toBeNull();
  });

  it('lets a pair’s real failure kind find its words', () => {
    expect(failureText('QUOTA', 'Storage quota exceeded', 'en')).toBe('Google Drive is full.');
  });
});

describe('first run', () => {
  it('shows only with no account and no pair, until skipped or finished', () => {
    expect(showFirstRun(status([], []), false)).toBe(true);
    expect(showFirstRun(status([], []), true)).toBe(false);
    expect(showFirstRun(status([]), false)).toBe(false);
    expect(showFirstRun(status([pair('p')], []), false)).toBe(false);
  });

  it('decides only from a status read while connected, with accounts and pairs filled in', () => {
    expect(firstRunDecision(status([], []), true, false)).toBe('show');
    expect(firstRunDecision(status([], []), false, false)).toBe('wait');
    expect(firstRunDecision(null, true, false)).toBe('wait');
    const partial = { ...status([], []), accounts: undefined } as unknown as DaemonStatus;
    expect(firstRunDecision(partial, true, false)).toBe('wait');
    expect(firstRunDecision(status([pair('p')], []), true, false)).toBe('skip');
    expect(firstRunDecision(status([]), true, false)).toBe('skip');
  });

  it('never shows the first run again once it was skipped or finished, connected or not', () => {
    expect(firstRunDecision(status([], []), true, true)).toBe('skip');
    expect(firstRunDecision(null, false, true)).toBe('skip');
  });

  it('starts at signing in, or at the folder when an account is there', () => {
    expect(firstRunStart(status([], []))).toBe('sign_in');
    expect(firstRunStart(status([]))).toBe('folder');
  });

  it('asks about the menu bar or notification area after the folder and before the preview', () => {
    expect(FIRST_RUN_STEPS).toEqual(['sign_in', 'folder', 'tray', 'preview']);
  });

  it('names the tray step for each platform and every step in English and Arabic', () => {
    expect(firstRunStepKey('tray', 'mac')).toBe('first.step_tray_mac');
    expect(firstRunStepKey('tray', 'windows')).toBe('first.step_tray_win');
    expect(firstRunStepKey('folder', 'mac')).toBe('first.step_folder');
    for (const platform of ['mac', 'windows'] as const) {
      for (const step of FIRST_RUN_STEPS) {
        const key = firstRunStepKey(step, platform);
        expect((en as Record<string, string>)[key], key).toBeTruthy();
        expect((ar as Record<string, string>)[key], key).toBeTruthy();
      }
    }
  });
});
