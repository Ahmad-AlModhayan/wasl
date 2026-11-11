import { describe, expect, it } from 'vitest';
import type { ConflictRecord, DaemonStatus, PairState } from '../types';
import { failedPair, groupProblems, guardReason, heldForApproval, heldForPreview, needsYouCount, problemItems } from './problemsModel';
import { problemCount } from '../shell/sidebar';
import { pairsSummary } from '../format';
import { headline } from '../tray/model';
import type { App } from '../app';
import { nextAfter } from '../shell/nextFocus';

function pair(id: string, name: string, patch: (p: PairState) => void = () => {}): PairState {
  const p: PairState = {
    pair: {
      id, name, account_id: 'a', local_root: `/x/${id}`, drive_folder_id: 'f', remote_path: name, method: 'TWO_WAY', enabled: true, paused: false, pause_reason: null,
      status: { last_run_at: 1, last_success_at: 1, last_status: 'SUCCESS', last_error: null, last_error_kind: null, consecutive_failures: 0, pending_approval: false, approve_once: false, hold_for_preview: false },
    },
    running: false, progress: null, open_conflicts: 0, blocked: false,
  };
  patch(p);
  return p;
}

function status(pairs: PairState[], extra: Partial<DaemonStatus> = {}): DaemonStatus {
  return { version: '0', protocol: 1, accounts: [], pairs, overall: 'needs_you', ...extra };
}

const conflict = (id: string, pairId: string, at: number): ConflictRecord => ({ id, pair_id: pairId, key: `${id}.txt`, detected_at: at, message: '' });

describe('the Problems list', () => {
  const held = pair('p', 'Photos', (p) => { p.blocked = true; p.pair.status.last_status = 'BLOCKED'; });
  const failed = pair('f', 'Private', (p) => { p.pair.status.last_status = 'FAILED'; });
  const life = pair('l', 'LifeOS', (p) => { p.open_conflicts = 2; });

  it('groups held deletions, failed syncs, conflicts, accounts and agent changes in that order', () => {
    const s = status([life, failed, held], { accounts: [{ id: 'me', email: 'me@example.com', auth_error: 'expired' }], agent_changes: 3 });
    const items = problemItems(s, [conflict('c1', 'l', 10), conflict('c2', 'l', 20)], 'en');
    expect(items.map((i) => i.id)).toEqual(['deletion:p', 'failed:f', 'conflict:c2', 'conflict:c1', 'account:me', 'agents']);
    const groups = groupProblems(items, 3);
    expect(groups.map((g) => [g.kind, g.count])).toEqual([['deletion', 1], ['failed', 1], ['conflict', 2], ['account', 1], ['agents', 3]]);
    expect(needsYouCount(groups)).toBe(8);
  });

  it('counts a pair waiting for an OK as a held deletion, never also as failed', () => {
    const p = pair('x', 'X', (q) => { q.pair.status.pending_approval = true; q.pair.status.last_status = 'FAILED'; });
    expect(heldForApproval(p)).toBe(true);
    expect(failedPair(p)).toBe(false);
    expect(problemItems(status([p]), [], 'en').map((i) => i.kind)).toEqual(['deletion']);
  });

  it('lists a new pair held for its first preview after held deletions, and the badge counts it', () => {
    const fresh = pair('n', 'New', (q) => { q.pair.status.hold_for_preview = true; q.pair.status.last_status = 'FAILED'; });
    const s = status([fresh, held]);
    const items = problemItems(s, [], 'en');
    expect(items.map((i) => i.id)).toEqual(['deletion:p', 'preview:n']);
    expect(failedPair(fresh)).toBe(false);
    expect(groupProblems(items).map((g) => [g.kind, g.count])).toEqual([['deletion', 1], ['preview', 1]]);
    expect(problemCount({ status: s } as App)).toBe(needsYouCount(groupProblems(items)));
  });

  it('says one need-you count in the subtitle, the tray and the Problems badge (#147 M1)', () => {
    // A pair with two conflicts, a failed pair, an account to sign in and an agent changeset: what Problems lists.
    const s = status([life, failed], { accounts: [{ id: 'me', email: 'me@example.com', auth_error: 'expired' }], agent_changes: 1 });
    const n = problemCount({ status: s } as App);
    expect(n).toBe(5);
    expect(pairsSummary(s, 'en')).toBe('2 pairs · 5 need you');
    expect(headline({ state: 'connected', version: '0' }, s, 'en').title).toBe('5 need you');
  });

  it('leaves out a held preview while its first run is going, and a pair whose deletions are held', () => {
    expect(heldForPreview(pair('r', 'R', (q) => { q.pair.status.hold_for_preview = true; q.running = true; }))).toBe(false);
    expect(heldForPreview(pair('b', 'B', (q) => { q.pair.status.hold_for_preview = true; q.blocked = true; }))).toBe(false);
  });

  it('leaves out a failed pair while a new run is deciding it', () => {
    expect(failedPair(pair('r', 'R', (q) => { q.pair.status.last_status = 'FAILED'; q.running = true; }))).toBe(false);
  });

  it('sorts pairs by name in the window language and leaves out conflicts of removed pairs', () => {
    const a = pair('2', 'beta', (q) => { q.pair.status.last_status = 'FAILED'; });
    const b = pair('1', 'Alpha', (q) => { q.pair.status.last_status = 'FAILED'; });
    const items = problemItems(status([a, b]), [conflict('gone', 'removed', 5)], 'en');
    expect(items.map((i) => i.id)).toEqual(['failed:1', 'failed:2']);
  });

  it('is empty with nothing waiting, or without a status yet', () => {
    expect(problemItems(status([pair('ok', 'OK')]), [], 'en')).toEqual([]);
    expect(problemItems(null, [], 'en')).toEqual([]);
    expect(groupProblems([])).toEqual([]);
  });
});

describe('the focus after rows leave a list', () => {
  const order = ['a', 'b', 'c', 'd'];
  it('goes to the row after the last one that left', () => {
    expect(nextAfter(order, ['b'])).toBe('c');
    expect(nextAfter(order, ['a', 'c'])).toBe('d');
  });
  it('goes to the row before when the last rows left', () => {
    expect(nextAfter(order, ['d'])).toBe('c');
    expect(nextAfter(order, ['c', 'd'])).toBe('b');
  });
  it('goes nowhere when every row left, or none did', () => {
    expect(nextAfter(order, order)).toBeNull();
    expect(nextAfter(order, ['z'])).toBeNull();
  });
});

describe('guardReason (#203: Problems shows the guard reason in the app language)', () => {
  // The engine's sentences, as wasl-core/src/guards.rs writes them.
  const drive = 'This sync would delete 212 files on Drive, 6% of the folder. Drive still has every file until you decide.';
  const device = 'This sync would delete 30 files on this device, 40% of the folder. This device still has every file until you decide.';
  const wipeDrive = "The folder on this device is empty but was synced before. Wasl won't empty Drive to match it.";
  const wipeDevice = "The Drive folder is empty but was synced before. Wasl won't empty this device's folder to match it.";
  const merge = "The first sync merges two folders that both have files: 10 on this device and 12 on Drive. 3 files only on this device go to its recycle bin, so it matches Drive.";
  const mergeNone = 'The first sync merges two folders that both have files: 1 on this device and 2 on Drive. Nothing is deleted.';
  const latin = /[A-Za-z]{3,}/;

  it('words every guard sentence in Arabic', () => {
    for (const r of [drive, device, wipeDrive, wipeDevice, merge, mergeNone]) {
      const ar = guardReason('ar', r);
      expect(ar.replaceAll('Drive', ''), r).not.toMatch(latin); // Drive is a name, kept in Latin letters
      expect(ar.length).toBeGreaterThan(10);
    }
    expect(guardReason('ar', drive)).toContain('212');
    expect(guardReason('ar', drive)).toContain('6');
  });

  it('keeps the English wording in English', () => {
    for (const r of [drive, device, wipeDrive, wipeDevice, merge, mergeNone]) expect(guardReason('en', r)).toBe(r);
  });

  it('shows a reason it does not know as the engine wrote it', () => {
    expect(guardReason('ar', 'Something new from a later engine.')).toBe('Something new from a later engine.');
    expect(guardReason('ar', `${drive} And a new sentence.`)).toBe(`${drive} And a new sentence.`);
  });
});

describe('guardReason against the engine source (#203; the engine catalogue #155 replaces this)', () => {
  it('knows every guard sentence wasl-core/src/guards.rs writes, so a wording change there turns this red', async () => {
    const src: string = (await import('../../../engine/crates/wasl-core/src/guards.rs?raw')).default;
    // The reason literals: every string in guards.rs that names what the guard does.
    const literals = [...src.matchAll(/"([^"]*)"/g)]
      .map((m) => m[1]!)
      .filter((s) => /would delete|is empty but|first sync merges|Nothing is deleted|files only on/.test(s));
    expect(literals.length).toBeGreaterThanOrEqual(7);
    for (const lit of literals) {
      const sentence = lit.replace(/\{\w*\}/g, '7').trim();
      expect(guardReason('ar', sentence), sentence).not.toBe(sentence);
      expect(guardReason('en', sentence), sentence).toBe(sentence);
    }
  });
});
