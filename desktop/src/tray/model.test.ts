import { describe, expect, it } from 'vitest';
import type { Conn, DaemonStatus, PairState, RunRecord } from '../types';
import { allPaused, headline, inFlight, isTrayWindow, moreInProblems, needsYou, recentRuns, trayRoute } from './model';
import { statusNeedsYou } from '../screens/problemsModel';

function pair(id: string, over: Partial<PairState> = {}, status: Partial<PairState['pair']['status']> = {}, paused = false): PairState {
  return {
    running: false,
    progress: null,
    open_conflicts: 0,
    blocked: false,
    ...over,
    pair: {
      id,
      name: `Pair ${id}`,
      account_id: 'a',
      local_root: `/Users/me/${id}`,
      drive_folder_id: 'f',
      remote_path: `My Drive/${id}`,
      method: 'TWO_WAY',
      enabled: true,
      paused,
      pause_reason: null,
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
  } as PairState;
}

function status(pairs: PairState[], extra: Partial<DaemonStatus> = {}): DaemonStatus {
  return { version: '', protocol: 1, overall: 'up_to_date', pairs, accounts: [{ id: 'a', email: 'me@example.com', auth_error: null }], ...extra };
}

const connected: Conn = { state: 'connected', version: '0.3.3' };

function run(id: string, pairId: string, st: RunRecord['status'], endedAt: number | null): RunRecord {
  return { id, pair_id: pairId, trigger: 'manual', status: st, stats: {} as RunRecord['stats'], message: null, started_at: (endedAt ?? 0) - 1000, ended_at: endedAt };
}

describe('which window this page is', () => {
  it('is the tray when Tauri names the window tray, whatever the address says', () => {
    expect(isTrayWindow('', 'tray')).toBe(true);
    expect(isTrayWindow('?window=tray', 'main')).toBe(false);
  });

  it('is the tray in a plain browser when the address asks for it', () => {
    expect(isTrayWindow('?window=tray&platform=mac', undefined)).toBe(true);
    expect(isTrayWindow('?platform=mac', undefined)).toBe(false);
  });
});

describe('the popover headline', () => {
  it('says how many pairs are syncing while any are, with the counts under it', () => {
    const h = headline(connected, status([pair('a', { running: true }), pair('b'), pair('c', { open_conflicts: 1 })]), 'en');
    expect(h.view.state).toBe('syncing');
    expect(h.title).toBe('Syncing 1 of 3 pairs');
    expect(h.detail).toBe('3 pairs · 1 syncing · 1 need you');
  });

  it('leads with what needs you when nothing is syncing', () => {
    const h = headline(connected, status([pair('a', {}, { last_status: 'FAILED' }), pair('b', { blocked: true })]), 'en');
    expect(h.view.state).toBe('approval');
    expect(h.title).toBe('2 need you');
  });

  it('counts a Drive sign-in as something that needs you', () => {
    const s = status([pair('a')], { accounts: [{ id: 'a', email: 'me@example.com', auth_error: 'expired' }] });
    expect(headline(connected, s, 'en')).toMatchObject({ view: { state: 'failed' }, title: '1 need you' });
  });

  it('says everything is synced, or paused, when it is', () => {
    expect(headline(connected, status([pair('a'), pair('b')]), 'en').title).toBe('Everything is synced');
    const paused = headline(connected, status([pair('a', {}, {}, true)]), 'en');
    expect(paused.view).toMatchObject({ state: 'waiting', paused: true });
    expect(paused.title).toBe('Paused');
  });

  it('says what the background service is doing before it is connected', () => {
    expect(headline({ state: 'not_running', message: null }, null, 'en')).toMatchObject({
      view: { state: 'failed' },
      title: "Wasl's background service isn't running",
      detail: null,
    });
    expect(headline({ state: 'connecting' }, null, 'ar').title).not.toBe('conn.connecting');
  });

  it('has no counts to show without pairs', () => {
    expect(headline(connected, status([]), 'en')).toMatchObject({ title: 'No folder pairs yet', detail: null });
  });
});

describe('what needs you', () => {
  it('lists a Drive sign-in, each loud pair and agent changes, most urgent first', () => {
    const s = status(
      [pair('c', { open_conflicts: 2 }), pair('f', {}, { last_status: 'FAILED' }), pair('ok'), pair('h', {}, { pending_approval: true })],
      { accounts: [{ id: 'g', email: 'me@example.com', auth_error: 'expired' }], agent_changes: 2 },
    );
    const items = needsYou(s, 'en');
    expect(items.map((i) => i.key)).toEqual(['pair:h', 'agents', 'account:g', 'pair:f', 'pair:c']);
    expect(items.find((i) => i.key === 'pair:c')).toMatchObject({ detail: '2 conflicts', route: 'problems' });
    expect(items.find((i) => i.key === 'pair:f')).toMatchObject({ detail: 'Failed', route: 'pair:f' });
    expect(items.find((i) => i.key === 'pair:h')?.route).toBe('pair:h');
    expect(items.find((i) => i.key === 'agents')).toMatchObject({ route: 'agents', detail: '2 waiting' });
    expect(items.find((i) => i.key === 'account:g')?.route).toBe('settings');
  });

  it('is empty when all is well or nothing is known yet', () => {
    expect(needsYou(status([pair('a'), pair('b', { running: true })]), 'en')).toEqual([]);
    expect(needsYou(null, 'en')).toEqual([]);
  });
});

describe('the rest of the popover', () => {
  it('shows a card for each pair syncing now', () => {
    expect(inFlight(status([pair('a', { running: true }), pair('b')])).map((p) => p.pair.id)).toEqual(['a']);
    expect(inFlight(null)).toEqual([]);
  });

  it('offers Resume all only when every pair is paused', () => {
    expect(allPaused(status([pair('a', {}, {}, true), pair('b', {}, {}, true)]))).toBe(true);
    expect(allPaused(status([pair('a', {}, {}, true), pair('b')]))).toBe(false);
    expect(allPaused(status([]))).toBe(false);
  });

  it('lists the last three finished runs, newest first, leaving out runs that never started', () => {
    const now = 10 * 60_000;
    const runs = [
      run('1', 'a', 'SUCCESS', now - 5 * 60_000),
      run('2', 'b', 'FAILED', now - 60_000),
      run('3', 'a', 'SKIPPED', now - 1000),
      run('4', 'a', 'RUNNING', null),
      run('5', 'gone', 'BLOCKED', now - 2 * 60_000),
      run('6', 'a', 'PARTIAL', now - 9 * 60_000),
    ];
    const recent = recentRuns(runs, status([pair('a'), pair('b')]), now, 'en');
    expect(recent.map((r) => r.id)).toEqual(['2', '5', '1']);
    expect(recent[0]).toMatchObject({ name: 'Pair b', view: { state: 'failed' }, outcome: 'Failed' });
    expect(recent[1]).toMatchObject({ name: 'Removed pair', view: { state: 'approval' } });
    expect(recent[2]?.view.state).toBe('synced');
  });

  it('gives Done with some errors the warning tone History gives it, not the synced one (#147 L9)', () => {
    const now = Date.UTC(2025, 9, 1, 12, 0);
    const recent = recentRuns([run('6', 'a', 'PARTIAL', now - 60_000)], status([pair('a')]), now, 'en');
    expect(recent[0]?.view.state).toBe('conflict');
  });

  it('says need you, not All synced, when only agent changes wait (#147 M1)', () => {
    const s = status([pair('a', {}, { last_status: 'SUCCESS' })], { agent_changes: 2, overall: 'needs_you' });
    expect(headline(connected, s, 'en').title).toBe('2 need you');
  });

  it('counts what is left in Problems in need-you items, not rows (#147 M1)', () => {
    const s = status(
      [pair('a', { open_conflicts: 3 }), pair('b', { blocked: true }), pair('c', {}, { last_status: 'FAILED' })],
      { agent_changes: 2, accounts: [{ id: 'me', email: 'me@example.com', auth_error: 'expired' }] },
    );
    const items = needsYou(s, 'en');
    expect(items).toHaveLength(5);
    // The three rows shown stand for some of the 8 items; the rest is said as a count of items.
    const shownCount = items.slice(0, 3).reduce((n, i) => n + i.count, 0);
    expect(moreInProblems(s, items, 3)).toBe(statusNeedsYou(s) - shownCount);
    expect(statusNeedsYou(s)).toBe(8);
    expect(moreInProblems(s, items, 5)).toBe(0);
  });

  it('opens the main window at the screen a route names', () => {
    expect(trayRoute('problems')).toEqual({ name: 'problems' });
    expect(trayRoute('agents')).toEqual({ name: 'agents' });
    expect(trayRoute('pair:abc')).toEqual({ name: 'pair', id: 'abc' });
    expect(trayRoute('pair:')).toBeNull();
    expect(trayRoute('elsewhere')).toBeNull();
  });
});
