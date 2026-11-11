// The one app-wide banner (#70, design D8): of everything wrong at once, the single state that gets the banner slot
// above the detail pane, with its one fix. Pure, so every state can be tested; src/app.ts draws it.
//
// Most urgent first: the background service stopped (nothing else can be known), offline, a Google account that
// needs signing in again, Google Drive full, a pair whose Drive folder is gone, then the pairs' own loud states
// (approval, failed, conflict; `appBanner`). Only what the service or the system reports: nothing is guessed.

import { appBanner, errorKind, pairStatus, type AppBanner } from './format';
import type { Conn, DaemonStatus, PairState } from './types';

export type AppAlert =
  /** The service is starting (after Restart service). */
  | { kind: 'starting' }
  /** The service isn't running; `notFound` when the app couldn't find wasld at all, `message` when it failed. */
  | { kind: 'stopped'; notFound: boolean; message: string | null }
  /** The system says there is no network. */
  | { kind: 'offline' }
  /** A Google account needs signing in again; `count` Drive pairs stopped because of it. */
  | { kind: 'auth'; count: number }
  /** Google Drive refused uploads for lack of space. `pair` when one pair is affected. */
  | { kind: 'quota'; count: number; pair: PairState | null }
  /** A pair's Drive folder is gone (deleted, in the bin, or no longer shared). */
  | { kind: 'missing'; count: number; pair: PairState | null }
  /** The pairs' own loud states. */
  | { kind: 'pairs'; banner: AppBanner };

export interface AlertInput {
  conn: Conn;
  status: DaemonStatus | null;
  /** `navigator.onLine`. */
  online: boolean;
}

export function appAlert({ conn, status, online }: AlertInput): AppAlert | null {
  if (conn.state === 'starting') return { kind: 'starting' };
  if (conn.state === 'not_running') {
    return { kind: 'stopped', notFound: conn.message === 'wasld_not_found', message: conn.message === 'wasld_not_found' ? null : conn.message };
  }
  // Connecting: the detail pane says so; a banner flashing at every launch would only be noise.
  if (conn.state !== 'connected' || !status) return null;
  if (!online) return { kind: 'offline' };
  const signedOut = new Set(status.accounts.filter((a) => a.auth_error).map((a) => a.id));
  if (signedOut.size > 0) {
    const count = status.pairs.filter((p) => p.pair.route !== 'devices' && p.pair.account_id !== null && signedOut.has(p.pair.account_id)).length;
    return { kind: 'auth', count };
  }
  const quota = failedWith(status, 'Quota');
  if (quota.length) return { kind: 'quota', count: quota.length, pair: quota.length === 1 ? quota[0]! : null };
  const missing = failedWith(status, 'NotFound');
  if (missing.length) return { kind: 'missing', count: missing.length, pair: missing.length === 1 ? missing[0]! : null };
  const banner = appBanner(status);
  return banner ? { kind: 'pairs', banner } : null;
}

/** Where a state shows: on every screen (it concerns the whole app), or on All pairs only (the pairs' own). */
export function alertScope(a: AppAlert): 'app' | 'pairs' {
  return a.kind === 'missing' || a.kind === 'pairs' ? 'pairs' : 'app';
}

/** Pairs whose last sync failed for this reason. */
function failedWith(status: DaemonStatus, kind: string): PairState[] {
  return status.pairs.filter((p) => pairStatus(p).state === 'failed' && errorKind(p.pair.status.last_error_kind) === kind);
}

// ---------------------------------------------------------------- first run

/** The first run's steps (design S.onboarding), in order. Naming the computer and suggested folders wait for the
 *  service to report them. 'tray' chooses whether Wasl shows in the menu bar (macOS) or the notification area
 *  (Windows), the same setting as Settings › General (#71). 'preview' is New pair's sheet, which takes over. */
export const FIRST_RUN_STEPS = ['sign_in', 'folder', 'tray', 'preview'] as const;
export type FirstRunStep = (typeof FIRST_RUN_STEPS)[number];

/** The first run shows for a new person: no Google account and no pair, and not skipped or finished before. */
export function showFirstRun(status: DaemonStatus, done: boolean): boolean {
  return !done && status.accounts.length === 0 && status.pairs.length === 0;
}

/** Whether a status can decide the first run: 'wait' leaves the decision for a later status. Only a status read while
 *  connected, with its accounts and pairs filled in, decides (#108): the window could otherwise settle on All pairs
 *  from a status it never heard, or from one sent while the service was still starting. */
export type FirstRunDecision = 'show' | 'skip' | 'wait';

export function firstRunDecision(status: DaemonStatus | null, connected: boolean, done: boolean): FirstRunDecision {
  if (done) return 'skip';
  if (!connected || !status || !Array.isArray(status.accounts) || !Array.isArray(status.pairs)) return 'wait';
  return showFirstRun(status, false) ? 'show' : 'skip';
}

/** Where the first run starts: signing in, unless an account is already there. */
export function firstRunStart(status: DaemonStatus): FirstRunStep {
  return status.accounts.length > 0 ? 'folder' : 'sign_in';
}

/** A step's name in the step list: the tray step says "menu bar" on macOS and "notification area" on Windows. */
export function firstRunStepKey(step: FirstRunStep, platform: 'mac' | 'windows'): string {
  return step === 'tray' ? `first.step_tray_${platform === 'mac' ? 'mac' : 'win'}` : `first.step_${step}`;
}
