import { describe, expect, it } from 'vitest';
import { appBanner, baseName, leastFree, offlinePeers, pairFacts, failureText, formatBytes, formatDate, formatDay, formatSpace, isolatePath, formatRelative, isLoud, isMirror, methodKey, overallLine, pairState, pairStatus, pairsSummary, progressFraction, progressText, runCounts, stateChanges, statusLabel, topState, viewFolder, viewState, viewTone } from './format';
import type { DaemonStatus } from './types';
import type { PairState, Progress, RunStats, ViewState } from './types';

function pair(over: Partial<PairState> = {}, status: Partial<PairState['pair']['status']> = {}, paused = false): PairState {
  return {
    running: false,
    progress: null,
    open_conflicts: 0,
    blocked: false,
    ...over,
    pair: {
      id: 'p1',
      name: 'Photos',
      account_id: 'a',
      local_root: 'C:\\Photos',
      drive_folder_id: 'f',
      remote_path: 'My Drive/Photos',
      method: 'TWO_WAY',
      enabled: true,
      paused,
      pause_reason: null,
      status: {
        last_run_at: null,
        last_success_at: null,
        last_status: null,
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

const progress = (p: Partial<Progress>): Progress => ({
  pair_id: 'p1',
  run_id: 'r1',
  phase: 'transferring',
  total_actions: 0,
  done_actions: 0,
  bytes_total: 0,
  bytes_done: 0,
  checked: 0,
  errors: 0,
  current_file: null,
  ...p,
});

/** What a reader sees: the text without its invisible isolate marks. */
const plain = (s: string | null) => (s ?? '').replace(/[\u2066-\u2069]/g, '');

describe('formatBytes', () => {
  it('uses decimal units, Latin unit names and Western digits in both languages', () => {
    expect(plain(formatBytes(0, 'en'))).toBe('0 B');
    expect(plain(formatBytes(1500, 'en'))).toBe('1.5 KB');
    expect(plain(formatBytes(2_500_000, 'en'))).toBe('2.5 MB');
    expect(plain(formatBytes(250_000_000, 'en'))).toBe('250 MB');
    expect(plain(formatBytes(1500, 'ar'))).toBe('1.5 KB');
    // Isolated, so it stays one left-to-right unit inside an Arabic sentence.
    expect(formatBytes(1500, 'ar').startsWith('\u2066')).toBe(true);
  });
});

describe('isolatePath', () => {
  it('keeps parent-to-child order with each folder name isolated', () => {
    const p = isolatePath('My Drive/العمل/التقارير');
    expect(plain(p)).toBe('My Drive/العمل/التقارير');
    expect(p).toBe('\u2066\u2068My Drive\u2069/\u2068العمل\u2069/\u2068التقارير\u2069\u2069');
    expect(plain(isolatePath('D:\\Work\\تقارير'))).toBe('D:\\Work\\تقارير');
  });
});

describe('failureText', () => {
  it('words known kinds in the window language and passes others through', () => {
    expect(failureText('Network', "Couldn't reach Google Drive", 'ar')).toBe('تعذّر الوصول إلى Google Drive. ستُعاد المحاولة.');
    expect(failureText('Auth', 'x', 'en')).toBe('Google needs you to sign in again.');
    expect(failureText(null, 'engine words', 'ar')).toBe('engine words');
    expect(failureText('Other', 'engine words', 'en')).toBe('engine words');
  });

  it('says when a plain pair stopped at a vault folder, instead of a generic integrity failure', () => {
    const vault = "This pair's Drive folder is an encrypted vault, but the pair isn't linked to it, so Wasl synced nothing.";
    expect(failureText('Integrity', vault, 'en')).toContain('Remove the pair and add it again');
    expect(failureText('Integrity', vault, 'ar')).toContain('خزنة مشفّرة');
    const damaged = "This pair's Drive folder has a vault.json that is damaged or too large to read.";
    expect(failureText('Integrity', damaged, 'en')).toContain('damaged vault.json');
    expect(failureText('Locked', 'The vault Private is locked on this computer.', 'en')).toBe(
      'The vault is locked on this computer. Unlock it to sync.',
    );
  });
});

describe('formatRelative', () => {
  const now = 1_700_000_000_000;
  it('says how long ago', () => {
    expect(formatRelative(now - 10_000, now, 'en')).toBe('now');
    expect(formatRelative(now - 5 * 60_000, now, 'en')).toBe('5 minutes ago');
    expect(formatRelative(now - 86_400_000, now, 'en')).toBe('yesterday');
    expect(formatRelative(now - 5 * 60_000, now, 'ar')).toMatch(/5/);
  });
});

describe('pairState', () => {
  it('puts the most urgent state first', () => {
    expect(pairState(pair({ running: true, open_conflicts: 2 }))).toBe('syncing');
    expect(pairState(pair({ open_conflicts: 1 }, {}, true))).toBe('needs_you');
    expect(pairState(pair({}, { last_status: 'FAILED' }))).toBe('needs_you');
    expect(pairState(pair({ blocked: true }))).toBe('needs_you');
    expect(pairState(pair({}, { last_status: 'SUCCESS' }, true))).toBe('paused');
    expect(pairState(pair({}, { hold_for_preview: true }))).toBe('waiting_preview');
    expect(pairState(pair({}, { last_status: 'SUCCESS' }))).toBe('up_to_date');
    expect(pairState(pair())).toBe('waiting');
  });
});

describe('progress', () => {
  it('prefers bytes, then actions, else unknown', () => {
    expect(progressFraction(progress({ bytes_total: 200, bytes_done: 50 }))).toBe(0.25);
    expect(progressFraction(progress({ total_actions: 4, done_actions: 1 }))).toBe(0.25);
    expect(progressFraction(progress({ phase: 'listing_remote' }))).toBeNull();
  });

  it('describes the phase and counts', () => {
    expect(plain(progressText(progress({ bytes_total: 2_000_000, bytes_done: 1_000_000 }), 'en'))).toBe('Copying files · 1 MB of 2 MB');
    expect(plain(progressText(progress({ phase: 'planning', total_actions: 10, done_actions: 3 }), 'en'))).toBe('Comparing folders… · Files: 3 of 10');
    expect(plain(progressText(progress({ phase: 'scanning_local', checked: 42 }), 'en'))).toBe('Checking this computer… · Files checked: 42');
  });
});

describe('small helpers', () => {
  it('names methods and spots mirrors', () => {
    expect(methodKey('UPLOAD_MIRROR')).toBe('method.upload_mirror');
    expect(isMirror('DOWNLOAD_MIRROR')).toBe(true);
    expect(isMirror('TWO_WAY')).toBe(false);
  });

  it('keeps only the counts a run had', () => {
    const s: RunStats = { uploaded: 2, downloaded: 1, exported: 1, deleted_local: 0, deleted_remote: 0, conflicts: 0, skipped: 5, errors: 0, bytes_up: 0, bytes_down: 0 };
    expect(runCounts(s)).toEqual([
      ['history.uploaded', 2],
      ['history.downloaded', 2],
    ]);
  });

  it('takes a folder name from Windows and POSIX paths', () => {
    expect(baseName('C:\\Users\\me\\Photos\\')).toBe('Photos');
    expect(baseName('/home/me/Documents')).toBe('Documents');
  });
});

describe('views', () => {
  const state = (over: Partial<ViewState['view']> = {}, mounted = true): ViewState => ({
    view: {
      id: 'v1', name: 'Photos', account_id: 'a', drive_folder_id: 'f', drive_id: null, remote_path: 'My Drive/Photos',
      local_path: 'C:\\Views\\Photos', vault_id: null, cache_limit: 20 * 2 ** 30, enabled: true, created_at: 0,
      last_refresh_at: 1, last_error: null, ...over,
    },
    mounted, files: 3, bytes: 300, cache_used: 0, pinned_bytes: 0, local_changes: [],
  });

  it('shows space in the units file managers use', () => {
    expect(formatSpace(20 * 2 ** 30, 'en')).toBe('\u206620 GB\u2069');
    expect(formatSpace(1536 * 2 ** 20, 'en')).toBe('\u20661.5 GB\u2069');
    expect(formatSpace(512, 'ar')).toBe('\u2066512 B\u2069');
  });

  it('says what state a view is in', () => {
    expect(viewState(state())).toBe('showing');
    expect(viewState(state({}, false))).toBe('starting');
    expect(viewState(state({ last_error: 'offline' }))).toBe('needs_you');
    expect(viewState(state({ enabled: false, last_error: 'offline' }))).toBe('off');
    expect(viewTone('showing')).toBe('kf-tone-positive');
    expect(viewTone('needs_you')).toBe('kf-tone-critical');
  });

  it('puts a new view folder inside the chosen place', () => {
    expect(viewFolder('C:\\Users\\me', 'Photos (Drive)')).toBe('C:\\Users\\me\\Photos (Drive)');
    expect(viewFolder('/home/me/', 'Photos')).toBe('/home/me/Photos');
    expect(viewFolder('/home/me', 'a/b:c')).toBe('/home/me/a-b-c');
    expect(viewFolder('/home/me', '  ')).toBeNull();
    expect(viewFolder('', 'Photos')).toBeNull();
    expect(viewFolder('/home/me', '..')).toBeNull();
  });
});

describe("Wasl's six states", () => {
  const prog: Progress = { pair_id: 'p1', run_id: 'r', phase: 'transferring', total_actions: 10, done_actions: 3, bytes_total: 200, bytes_done: 68, checked: 0, errors: 0, current_file: null };

  it('maps every pair state to one of the six, most urgent first', () => {
    expect(pairStatus(pair({}, { last_status: 'SUCCESS' })).state).toBe('synced');
    expect(pairStatus(pair({ running: true, progress: prog })).state).toBe('syncing');
    expect(pairStatus(pair()).state).toBe('waiting');
    expect(pairStatus(pair({}, { pending_approval: true, last_status: 'FAILED' })).state).toBe('approval');
    expect(pairStatus(pair({ blocked: true })).state).toBe('approval');
    expect(pairStatus(pair({ open_conflicts: 2 }, { last_status: 'FAILED' })).state).toBe('failed');
    expect(pairStatus(pair({ open_conflicts: 2, running: true }, { last_status: 'SUCCESS' }))).toMatchObject({ state: 'conflict', count: 2 });
  });

  it('lets a new run hide the last run’s failure', () => {
    expect(pairStatus(pair({ running: true, progress: prog }, { last_status: 'FAILED' })).state).toBe('syncing');
  });

  it('shows paused as Waiting with its own word, not a seventh state', () => {
    const v = pairStatus(pair({}, { last_status: 'SUCCESS' }, true));
    expect(v).toMatchObject({ state: 'waiting', paused: true });
    expect(statusLabel(v, 'en')).toBe('Paused');
    expect(statusLabel(v, 'ar')).toBe('متوقف مؤقتًا');
  });

  it('shows a new pair held for its first preview as Needs approval, until its first sync runs (#64)', () => {
    const held = pairStatus(pair({}, { hold_for_preview: true }));
    expect(held).toMatchObject({ state: 'approval', preview: true });
    expect(statusLabel(held, 'en')).toBe('Needs approval');
    expect(pairStatus(pair({}, { hold_for_preview: true }, true)).state).toBe('approval');
    expect(pairStatus(pair({ running: true }, { hold_for_preview: true })).state).toBe('syncing');
    expect(pairStatus(pair({ blocked: true }, { hold_for_preview: true })).preview).toBeUndefined();
  });

  it('shows a pair that is turned off as Waiting', () => {
    const off = pair({}, { last_status: 'SUCCESS' });
    off.pair.enabled = false;
    expect(pairStatus(off).state).toBe('waiting');
  });

  it('says how far a sync is in the label, so reduced motion still reads it', () => {
    expect(statusLabel(pairStatus(pair({ running: true, progress: prog })), 'en')).toBe('Syncing 34%');
    expect(statusLabel(pairStatus(pair({ running: true, progress: null })), 'en')).toBe('Syncing');
    expect(statusLabel(pairStatus(pair({ open_conflicts: 3 })), 'en')).toBe('3 conflicts');
    expect(statusLabel(pairStatus(pair({ open_conflicts: 1 })), 'en')).toBe('Conflict');
  });

  it('has a word in both languages for each state', () => {
    for (const state of ['synced', 'waiting', 'approval', 'failed'] as const) {
      const v = { state, paused: false, percent: null, count: 0 };
      expect(statusLabel(v, 'en')).not.toMatch(/^status\./);
      expect(statusLabel(v, 'ar')).not.toBe(statusLabel(v, 'en'));
    }
  });

  it('is loud only for approval, failure and conflict', () => {
    expect(['approval', 'failed', 'conflict', 'syncing', 'waiting', 'synced'].filter((s) => isLoud(s as never))).toEqual(['approval', 'failed', 'conflict']);
  });

  it('picks the most urgent state', () => {
    expect(topState(['synced', 'conflict', 'syncing', 'failed'])).toBe('failed');
    expect(topState(['waiting', 'approval'])).toBe('approval');
    expect(topState([])).toBeNull();
  });

  function status(pairs: PairState[], authError = false): DaemonStatus {
    return { version: '', protocol: 1, overall: 'needs_you', pairs, accounts: [{ id: 'a', email: 'x', auth_error: authError ? 'expired' : null }] };
  }

  it('puts one banner over the app, for the top loud state only', () => {
    expect(appBanner(status([pair({}, { last_status: 'SUCCESS' }), pair({ running: true })]))).toBeNull();
    expect(appBanner(status([pair({ open_conflicts: 2 }), pair({ open_conflicts: 1 })]))).toEqual({ state: 'conflict', count: 3, auth: false });
    expect(appBanner(status([pair({ open_conflicts: 2 }), pair({}, { last_status: 'FAILED' })]))).toEqual({ state: 'failed', count: 1, auth: false });
    expect(appBanner(status([pair({}, { last_status: 'SUCCESS' })], true))).toEqual({ state: 'failed', count: 0, auth: true });
    expect(appBanner(status([pair({ blocked: true })], true))?.state).toBe('approval');
  });

  it('opens the first held preview from the banner only when every pair waiting for approval is new', () => {
    const held = pair({}, { hold_for_preview: true });
    held.pair.id = 'new';
    expect(appBanner(status([pair({}, { last_status: 'SUCCESS' }), held]))).toEqual({ state: 'approval', count: 1, auth: false, preview: 'new' });
    expect(appBanner(status([held, pair({ blocked: true })]))?.preview).toBeUndefined();
  });

  it('draws the overall line’s glyph and word from the daemon’s one overall state', () => {
    const at = (overall: DaemonStatus['overall'], pairs: PairState[] = [pair()]) => overallLine({ ...status(pairs), overall }, 'en');
    expect(at('up_to_date')).toEqual({ view: { state: 'synced', paused: false, percent: null, count: 0 }, word: 'Synced' });
    expect(at('paused')?.view).toMatchObject({ state: 'waiting', paused: true });
    expect(at('paused')?.word).toBe('Paused');
    expect(at('needs_you', [pair({ open_conflicts: 1 }), pair({}, { last_status: 'SUCCESS' })])).toMatchObject({ view: { state: 'conflict' }, word: 'Needs you' });
    expect(at('needs_you', [pair({}, { last_status: 'SUCCESS' })])?.view.state).toBe('failed');
    const quiet = [pair({}, { last_status: 'SUCCESS' })];
    expect(overallLine({ ...status(quiet), overall: 'needs_you', agent_changes: 2 }, 'en')?.view.state).toBe('approval');
    expect(overallLine({ ...status(quiet), overall: 'needs_you', agent_changes: 2, accounts: [{ id: 'a', email: 'a@example.com', auth_error: 'expired' }] }, 'en')?.view.state).toBe('failed');
    expect(at('idle', [])).toBeNull();
  });

  it('says Needs approval, not Waiting, while a pair is held for its first preview', () => {
    const held = pair({}, { hold_for_preview: true });
    for (const overall of ['waiting', 'up_to_date', 'needs_you'] as const) {
      expect(overallLine({ ...status([held, pair()]), overall }, 'en')).toEqual({
        view: { state: 'approval', paused: false, percent: null, count: 0 },
        word: 'Needs approval',
      });
    }
    expect(overallLine({ ...status([held]), overall: 'waiting' }, 'ar')?.word).toBe('بانتظار موافقتك');
    // A failed pair, or a Drive sign-in, still leads.
    const failed = pair({}, { last_status: 'FAILED' });
    expect(overallLine({ ...status([held, failed]), overall: 'needs_you' }, 'en')?.word).toBe('Needs you');
    const signIn = { ...status([held], true), overall: 'needs_you' as const };
    expect(overallLine(signIn, 'en')?.word).toBe('Needs you');
    // Another pair syncing, or everything paused, keeps the daemon's word.
    expect(overallLine({ ...status([held, pair({ running: true })]), overall: 'syncing' }, 'en')?.word).toBe('Syncing');
    expect(overallLine({ ...status([held]), overall: 'paused' }, 'en')?.word).toBe('Paused');
    // Once its preview has started, the pair is just syncing.
    expect(overallLine({ ...status([{ ...held, running: true }]), overall: 'syncing' }, 'en')?.word).toBe('Syncing');
    expect(overallLine({ ...status([pair()]), overall: 'up_to_date' }, 'ar')?.word).toBe('تمت المزامنة');
  });

  it('sums up the pairs for the toolbar', () => {
    const s = status([pair({ running: true }), pair({}, { last_status: 'SUCCESS' }), pair({ open_conflicts: 1 }), pair({ blocked: true })]);
    expect(pairsSummary(s, 'en')).toBe('4 pairs · 1 syncing · 2 need you');
    expect(pairsSummary(status([pair({}, { last_status: 'SUCCESS' })]), 'en')).toBe('1 pair');
  });

  it('announces a pair only when its state changes, not as its progress moves', () => {
    const before = new Map([['p1', 'syncing' as const]]);
    expect(stateChanges(before, [pair({ running: true, progress: prog })], 'en')).toEqual([]);
    expect(stateChanges(before, [pair({}, { last_status: 'SUCCESS' })], 'en')).toEqual(['Photos, Synced']);
    expect(stateChanges(new Map(), [pair()], 'en')).toEqual([]);
  });
});

describe('a day without its hour', () => {
  it('names the day the way the full date does, with no time', () => {
    const at = new Date(2026, 11, 1, 20, 14).getTime();
    expect(formatDay(at, 'en')).toBe('Dec 1, 2026');
    expect(formatDate(at, 'en').startsWith(formatDay(at, 'en'))).toBe(true);
    expect(formatDay(at, 'ar')).not.toMatch(/[:٫]|\d{1,2}:\d{2}/);
  });
});

describe('the daemon\'s pair facts (#103)', () => {
  const devicesOnly = (peers: PairState['peers'], status: Partial<PairState['pair']['status']> = { last_status: 'SUCCESS' }) => {
    const s = pair({ peers }, status);
    s.pair.route = 'devices';
    return s;
  };

  it('says a devices-only pair waits for its devices when every one is offline', () => {
    const s = devicesOnly([{ id: 'l', name: 'Laptop', online: false }]);
    expect(offlinePeers(s)).toEqual(['Laptop']);
    expect(pairStatus(s).state).toBe('waiting');
    expect(plain(statusLabel(pairStatus(s), 'en'))).toBe('Waiting for Laptop');
    expect(plain(statusLabel(pairStatus(s), 'ar'))).toBe('بانتظار Laptop');
  });

  it('names every offline device in the language\'s own list', () => {
    const s = devicesOnly([{ id: 'l', name: 'Laptop', online: false }, { id: 'p', name: 'Office PC', online: false }]);
    expect(plain(statusLabel(pairStatus(s), 'en'))).toBe('Waiting for Laptop and Office PC');
  });

  it('says "your other device" for a device without a name, once', () => {
    const s = devicesOnly([{ id: 'l', name: '', online: false }, { id: 'p', name: ' ', online: false }]);
    expect(plain(statusLabel(pairStatus(s), 'en'))).toBe('Waiting for your other device');
    expect(plain(statusLabel(pairStatus(s), 'ar'))).toBe('بانتظار جهازك الآخر');
    const mixed = devicesOnly([{ id: 'l', name: 'Laptop', online: false }, { id: 'p', name: '', online: false }]);
    expect(plain(statusLabel(pairStatus(mixed), 'en'))).toBe('Waiting for Laptop and your other device');
  });

  it("doesn't wait while one device is online, for a pair that also syncs with Drive, or without peers", () => {
    expect(offlinePeers(devicesOnly([{ id: 'l', name: 'Laptop', online: false }, { id: 'p', name: 'PC', online: true }]))).toBeNull();
    expect(offlinePeers(pair({ peers: [{ id: 'l', name: 'Laptop', online: false }] }))).toBeNull();
    expect(offlinePeers(devicesOnly(undefined))).toBeNull();
    expect(offlinePeers(devicesOnly([]))).toBeNull();
    expect(pairStatus(devicesOnly(undefined)).state).toBe('synced');
  });

  it('keeps a louder state and a pause ahead of waiting for a device', () => {
    const offline = [{ id: 'l', name: 'Laptop', online: false }];
    expect(pairStatus(devicesOnly(offline, { last_status: 'FAILED' })).state).toBe('failed');
    expect(pairStatus({ ...devicesOnly(offline), open_conflicts: 2 }).state).toBe('conflict');
    const paused = devicesOnly(offline);
    paused.pair.paused = true;
    expect(pairStatus(paused)).toMatchObject({ state: 'waiting', paused: true });
    expect(pairStatus(paused).waitingFor).toBeUndefined();
  });

  it('shows the size and the skipped count only when the daemon reports them', () => {
    expect(pairFacts(pair(), 'en')).toEqual([]);
    expect(pairFacts(pair({ folder_bytes: null, skipped: null }), 'en')).toEqual([]);
    const facts = pairFacts(pair({ folder_bytes: 186_000_000, skipped: 1 }), 'en');
    expect(facts.map((f) => [f.label, plain(f.value)])).toEqual([
      ['Size', '186 MB'],
      ['Skipped in the last sync', '1 file'],
    ]);
    expect(pairFacts(pair({ skipped: 0 }), 'en').map((f) => f.value)).toEqual(['0 files']);
  });

  it('shows the least free space among the pairs, or nothing when none reports it', () => {
    expect(leastFree([pair(), pair({ free_bytes: null })])).toBeNull();
    expect(leastFree([pair({ free_bytes: 9e11 }), pair(), pair({ free_bytes: 4e11 })])).toBe(4e11);
    expect(leastFree([pair({ free_bytes: 0 })])).toBe(0);
  });
});
