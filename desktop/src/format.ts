// Pure formatting and state helpers (no DOM, no Tauri), so they are unit-tested.

import { locale, translate, type Lang } from './i18n';
import type { DaemonStatus, Overall, PairState, Progress, RunStats, SyncMode, ViewState } from './types';
import { statusNeedsYou } from './screens/problemsModel';

/** "1.5 MB" (decimal units). Units stay Latin in both languages, as on Android, with Western digits. */
export function formatBytes(bytes: number, lang: Lang): string {
  const units = ['B', 'KB', 'MB', 'GB', 'TB'] as const;
  let v = Math.max(0, bytes);
  let i = 0;
  while (v >= 1000 && i < units.length - 1) {
    v /= 1000;
    i++;
  }
  const n = new Intl.NumberFormat(locale(lang), { maximumFractionDigits: i === 0 || v >= 100 ? 0 : 1 }).format(v);
  // Isolated, so "1.5 MB" stays one left-to-right unit inside an Arabic sentence.
  return `\u2066${n} ${units[i]}\u2069`;
}

/** "1.2 TB free": the least free space among the pairs' drives, or null when none reports it (#103). The tightest
 *  drive is the one that fills first. */
export function leastFree(pairs: readonly PairState[]): number | null {
  let least: number | null = null;
  for (const p of pairs) if (typeof p.free_bytes === 'number' && (least === null || p.free_bytes < least)) least = p.free_bytes;
  return least;
}

/** A fact the inspector and Pair detail show under a label. */
export interface PairFact {
  label: string;
  value: string;
  hint?: string;
}

/** The folder's size at its last sync and how many files that sync's plan skipped (#103), each only when the
 *  daemon reports it: an older daemon leaves them out, and nothing is shown in their place. */
export function pairFacts(s: PairState, lang: Lang): PairFact[] {
  const out: PairFact[] = [];
  if (typeof s.folder_bytes === 'number') out.push({ label: translate(lang, 'inspector.size'), value: formatBytes(s.folder_bytes, lang) });
  if (typeof s.skipped === 'number') {
    out.push({ label: translate(lang, 'inspector.skipped'), value: translate(lang, 'preview.files', { count: s.skipped }), hint: translate(lang, 'inspector.skipped_hint') });
  }
  return out;
}

/**
 * A path as one left-to-right sequence whose folder names are each isolated: Arabic names keep
 * their own direction, and the order stays parent to child ("My Drive/العمل/التقارير" can't read
 * as "My Drive/التقارير/العمل").
 */
export function isolatePath(path: string): string {
  const parts = path.split(/([\\/])/);
  return '\u2066' + parts.map((p) => (p === '/' || p === '\\' ? p : `\u2068${p}\u2069`)).join('') + '\u2069';
}

/** The engine's error kinds the window words itself (in the window's language). */
const ERROR_KINDS = ['Auth', 'Permission', 'Quota', 'NotFound', 'Locked', 'Network', 'RateLimit', 'Server', 'Io', 'Integrity'];

/**
 * A failure kind in one spelling: a pair's comes as wasl-core's `NOT_FOUND`, a view's as wasl-ipc's `not_found`, the
 * dev mock's as `NotFound`; all become `NotFound`.
 */
export function errorKind(raw: string | null | undefined): string | null {
  if (!raw) return null;
  if (/^[A-Z][a-z]/.test(raw) && !raw.includes('_')) return raw;
  return raw
    .split('_')
    .filter(Boolean)
    .map((w) => w[0]!.toUpperCase() + w.slice(1).toLowerCase())
    .join('');
}

/** The service's words when a plain pair's Drive folder turns out to be a vault (wasld vaults.rs). */
const PLAIN_ON_VAULT = "This pair's Drive folder is an encrypted vault";
const PLAIN_ON_DAMAGED_VAULT = "This pair's Drive folder has a vault.json that is damaged";

/** The words for a failed run: by its kind when known, else the engine's own message. */
export function failureText(raw: string | null, message: string | null, lang: Lang): string | null {
  if (message?.startsWith(PLAIN_ON_VAULT)) return translate(lang, 'errors.plain_on_vault');
  if (message?.startsWith(PLAIN_ON_DAMAGED_VAULT)) return translate(lang, 'errors.plain_on_damaged_vault');
  const kind = errorKind(raw);
  if (kind && ERROR_KINDS.includes(kind)) return translate(lang, `errors.kind.${kind}`);
  return message;
}

/** "5 minutes ago", "yesterday", "in 2 hours". `then` and `now` are Unix milliseconds. */
export function formatRelative(then: number, now: number, lang: Lang): string {
  const rtf = new Intl.RelativeTimeFormat(locale(lang), { numeric: 'auto' });
  const s = Math.round((then - now) / 1000);
  const abs = Math.abs(s);
  if (abs < 45) return rtf.format(0, 'second');
  if (abs < 45 * 60) return rtf.format(Math.round(s / 60), 'minute');
  if (abs < 22 * 3600) return rtf.format(Math.round(s / 3600), 'hour');
  if (abs < 26 * 86400) return rtf.format(Math.round(s / 86400), 'day');
  if (abs < 320 * 86400) return rtf.format(Math.round(s / (30 * 86400)), 'month');
  return rtf.format(Math.round(s / (365 * 86400)), 'year');
}

export function formatDate(ms: number, lang: Lang): string {
  return new Intl.DateTimeFormat(locale(lang), { dateStyle: 'medium', timeStyle: 'short' }).format(new Date(ms));
}

/** The day alone, for dates where the hour means nothing (how long the bin keeps a file). */
export function formatDay(ms: number, lang: Lang): string {
  return new Intl.DateTimeFormat(locale(lang), { dateStyle: 'medium' }).format(new Date(ms));
}

/** How far a run is, 0..1, or null while it cannot say (listing, scanning). */
export function progressFraction(p: Progress): number | null {
  if (p.bytes_total > 0) return Math.min(1, p.bytes_done / p.bytes_total);
  if (p.total_actions > 0) return Math.min(1, p.done_actions / p.total_actions);
  return null;
}

/** One line about a running sync: phase, then counts. */
export function progressText(p: Progress, lang: Lang): string {
  const phase = translate(lang, `progress.phase.${p.phase}`);
  if (p.phase === 'transferring' && p.bytes_total > 0) {
    const bytes = translate(lang, 'progress.bytes', {
      done: formatBytes(p.bytes_done, lang),
      total: formatBytes(p.bytes_total, lang),
    });
    return `${phase} · ${bytes}`;
  }
  if (p.total_actions > 0) {
    return `${phase} · ${translate(lang, 'progress.files', { done: p.done_actions, total: p.total_actions })}`;
  }
  if (p.checked > 0) return `${phase} · ${translate(lang, 'progress.checked', { count: p.checked })}`;
  return phase;
}

export type PairUiState = 'syncing' | 'needs_you' | 'paused' | 'waiting_preview' | 'off' | 'up_to_date' | 'waiting';

/** The state a pair's chip shows, most urgent first. */
export function pairState(s: PairState): PairUiState {
  const st = s.pair.status;
  if (s.running) return 'syncing';
  if (s.blocked || s.open_conflicts > 0 || st.pending_approval || st.last_status === 'FAILED') return 'needs_you';
  if (s.pair.paused) return 'paused';
  if (st.hold_for_preview) return 'waiting_preview';
  if (!s.pair.enabled) return 'off';
  if (st.last_status === 'SUCCESS' || st.last_status === 'PARTIAL') return 'up_to_date';
  return 'waiting';
}

/** The Kahf tone class for a state. */
export function stateTone(state: PairUiState | Overall): string {
  switch (state) {
    case 'up_to_date':
      return 'kf-tone-positive';
    case 'syncing':
      return 'kf-tone-info';
    case 'needs_you':
      return 'kf-tone-critical';
    case 'waiting':
    case 'waiting_preview':
      return 'kf-tone-attention';
    default:
      return 'kf-tone-neutral';
  }
}

/** The i18n key of a sync method. */
export function methodKey(m: SyncMode): string {
  return `method.${m.toLowerCase()}`;
}

export function isMirror(m: SyncMode): boolean {
  return m === 'UPLOAD_MIRROR' || m === 'DOWNLOAD_MIRROR';
}

/** The parts of a run's counts worth showing, as [i18n key, count]. */
export function runCounts(s: RunStats): Array<[string, number]> {
  const parts: Array<[string, number]> = [
    ['history.uploaded', s.uploaded],
    ['history.downloaded', s.downloaded + s.exported],
    ['history.deleted', s.deleted_local + s.deleted_remote],
    ['history.conflicts', s.conflicts],
    ['history.errors', s.errors],
  ];
  return parts.filter(([, n]) => n > 0);
}

/**
 * Space on this computer, in binary units as the file managers show it ("20 GB" is 20 GiB): a view's cache
 * limit is set in whole GiB, so its own figures read as round numbers.
 */
export function formatSpace(bytes: number, lang: Lang): string {
  const units = ['B', 'KB', 'MB', 'GB', 'TB'] as const;
  let v = Math.max(0, bytes);
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  const n = new Intl.NumberFormat(locale(lang), { maximumFractionDigits: i === 0 || v >= 100 ? 0 : 1 }).format(v);
  return `\u2066${n} ${units[i]}\u2069`;
}

/** The cache limits a view offers, in GiB. */
export const VIEW_LIMITS_GB = [5, 10, 20, 50, 100, 250] as const;

export type ViewUiState = 'showing' | 'off' | 'needs_you' | 'starting';

/** The state a view's chip shows. */
export function viewState(s: ViewState): ViewUiState {
  if (!s.view.enabled) return 'off';
  if (s.view.last_error) return 'needs_you';
  return s.mounted ? 'showing' : 'starting';
}

export function viewTone(state: ViewUiState): string {
  switch (state) {
    case 'showing':
      return 'kf-tone-positive';
    case 'needs_you':
      return 'kf-tone-critical';
    case 'starting':
      return 'kf-tone-info';
    default:
      return 'kf-tone-neutral';
  }
}

/** Where a view goes: the folder `name` inside `parent` (a Windows or POSIX path); null without both. */
export function viewFolder(parent: string, name: string): string | null {
  const clean = name.trim().replace(/[\\/:*?"<>|]/g, '-');
  if (!parent || !clean || clean === '.' || clean === '..') return null;
  const sep = parent.includes('\\') && !parent.includes('/') ? '\\' : '/';
  return parent.replace(/[\\/]+$/, '') + sep + clean;
}

/** The last segment of a local path (Windows or POSIX), for a default pair name. */
export function baseName(path: string): string {
  const parts = path.replace(/[\\/]+$/, '').split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

// ---------------------------------------------------------------- Wasl's six states (design D8)

/**
 * Wasl's six states, most urgent first (docs/design/project/2-status-system.md). Paused is the Waiting state with
 * a pause glyph and its own word, not a seventh state.
 */
export const STATUS_STATES = ['approval', 'failed', 'conflict', 'syncing', 'waiting', 'synced'] as const;
export type StatusState = (typeof STATUS_STATES)[number];

/** What a pair's glyph and label show. */
export interface StatusView {
  state: StatusState;
  /** Waiting because someone paused it. */
  paused: boolean;
  /** Syncing: how far, 0–100, or null while it can't say (listing, scanning). */
  percent: number | null;
  /** Conflict: how many files. */
  count: number;
  /** Needs approval because the pair is new and held for its first preview (nothing moves until it's started),
   *  not because a guard stopped a run. */
  preview?: boolean;
  /** Waiting for these devices: a devices-only pair whose every peer is offline (#103). */
  waitingFor?: string[];
}

/** The loud states: a tonal chip, a Problems badge, a banner when they top the app. */
export function isLoud(state: StatusState): boolean {
  return state === 'approval' || state === 'failed' || state === 'conflict';
}

/**
 * A pair's state. A run in progress hides the last run's failure (the new run decides it); open conflicts and a
 * held deletion stay until someone answers them. A new pair held for its first preview needs approval (#64): it
 * waits on the person, not on Wasl.
 */
export function pairStatus(s: PairState): StatusView {
  const st = s.pair.status;
  const base = { paused: false, percent: null, count: 0 };
  if (st.pending_approval || s.blocked) return { ...base, state: 'approval' };
  if (st.hold_for_preview && !s.running) return { ...base, state: 'approval', preview: true };
  if (st.last_status === 'FAILED' && !s.running) return { ...base, state: 'failed' };
  if (s.open_conflicts > 0) return { ...base, state: 'conflict', count: s.open_conflicts };
  if (s.running) {
    const f = s.progress ? progressFraction(s.progress) : null;
    return { ...base, state: 'syncing', percent: f === null ? null : Math.round(f * 100) };
  }
  if (s.pair.paused) return { ...base, state: 'waiting', paused: true };
  const offline = offlinePeers(s);
  if (offline) return { ...base, state: 'waiting', waitingFor: offline };
  if (st.last_status === 'SUCCESS' || st.last_status === 'PARTIAL') {
    return { ...base, state: s.pair.enabled ? 'synced' : 'waiting' };
  }
  return { ...base, state: 'waiting' };
}

/**
 * The devices a pair waits for: a devices-only pair has nowhere else to sync, so when every device it is shared
 * with is offline it waits for them. Null otherwise, and when the daemon doesn't report peers (an older one).
 */
export function offlinePeers(s: PairState): string[] | null {
  const peers = s.peers ?? [];
  if (s.pair.route !== 'devices' || peers.length === 0 || peers.some((p) => p.online)) return null;
  return peers.map((p) => p.name);
}

/** The word for a state, in the window's language: "Synced", "Syncing 34%", "Paused", "2 conflicts",
 *  "Waiting for Office PC". */
export function statusLabel(v: StatusView, lang: Lang): string {
  switch (v.state) {
    case 'syncing':
      return v.percent === null ? translate(lang, 'status.syncing') : translate(lang, 'status.syncing_percent', { percent: v.percent });
    case 'waiting':
      if (v.paused) return translate(lang, 'status.paused');
      if (v.waitingFor?.length) {
        // A device without a name is "your other device", said once however many there are.
        const names = [...new Set(v.waitingFor.map((n) => n.trim() || translate(lang, 'status.other_device')))];
        const devices = new Intl.ListFormat(locale(lang), { type: 'conjunction' }).format(names.map((n) => `\u2068${n}\u2069`));
        return translate(lang, 'status.waiting_for', { devices });
      }
      return translate(lang, 'status.waiting');
    case 'conflict':
      return translate(lang, 'status.conflict', { count: Math.max(1, v.count) });
    default:
      return translate(lang, `status.${v.state}`);
  }
}

/** The most urgent of several states; null for none. */
export function topState(states: Iterable<StatusState>): StatusState | null {
  let best: number | null = null;
  for (const s of states) {
    const i = STATUS_STATES.indexOf(s);
    if (best === null || i < best) best = i;
  }
  return best === null ? null : STATUS_STATES[best]!;
}

/** The app-wide banner (one only): the top loud state, how many it covers, and whether a Drive sign-in is why. */
export interface AppBanner {
  state: 'approval' | 'failed' | 'conflict';
  count: number;
  /** A Google account needs signing in again: the one fix names that. */
  auth: boolean;
  /** Every pair waiting for approval is new and held for its first preview: the first one's id, whose preview
   *  the banner opens. */
  preview?: string;
}

export function appBanner(status: DaemonStatus): AppBanner | null {
  const views = status.pairs.map(pairStatus);
  const auth = status.accounts.some((a) => a.auth_error);
  const states = views.map((v) => v.state);
  if (auth) states.push('failed');
  const top = topState(states);
  if (top !== 'approval' && top !== 'failed' && top !== 'conflict') return null;
  const of = views.filter((v) => v.state === top);
  const count = top === 'conflict' ? of.reduce((n, v) => n + v.count, 0) : of.length;
  const held = top === 'approval' && of.every((v) => v.preview) ? status.pairs[views.findIndex((v) => v.preview)]?.pair.id : undefined;
  return { state: top, count, auth: top === 'failed' && auth, preview: held };
}

/**
 * The app's overall line (the sidebar's foot, the tray): the daemon's `overall` as a glyph and its word, from this
 * one mapping. Needs you takes the glyph of the most urgent loud pair (a Drive sign-in alone is a failure); null
 * with no pairs.
 */
export function overallLine(status: DaemonStatus, lang: Lang): { view: StatusView; word: string } | null {
  const view = (state: StatusState, paused = false): StatusView => ({ state, paused, percent: null, count: 0 });
  const word = translate(lang, `overall.${status.overall}`);
  // A pair held for its first preview needs approval, though the daemon calls that Waiting: when it is the only thing
  // asking (no other loud pair, no sign-in), the footer and the tray say Needs approval, like the inspector (#108).
  // Syncing and Paused keep their word: the daemon's overall then says what everything else is doing.
  const quiet = status.overall === 'waiting' || status.overall === 'up_to_date' || status.overall === 'needs_you';
  if (quiet && !status.accounts.some((a) => a.auth_error)) {
    const loud = status.pairs.map(pairStatus).filter((v) => isLoud(v.state));
    if (loud.length > 0 && loud.every((v) => v.state === 'approval' && v.preview)) {
      return { view: view('approval'), word: translate(lang, 'status.approval') };
    }
  }
  switch (status.overall) {
    case 'up_to_date':
      return { view: view('synced'), word };
    case 'syncing':
      return { view: view('syncing'), word };
    case 'paused':
      return { view: view('waiting', true), word };
    case 'waiting':
      return { view: view('waiting'), word };
    case 'needs_you': {
      const loud = topState(status.pairs.map((p) => pairStatus(p).state).filter(isLoud));
      if (loud) return { view: view(loud), word };
      // No pair is loud: an account that must sign in is a failure; agent changes held for review wait for an OK.
      const signIn = status.accounts.some((a) => a.auth_error);
      return { view: view(!signIn && (status.agent_changes ?? 0) > 0 ? 'approval' : 'failed'), word };
    }
    case 'idle':
      return null;
  }
}

/** The toolbar's subtitle on All pairs: "5 pairs · 1 syncing · 3 need you". */
export function pairsSummary(status: DaemonStatus, lang: Lang): string {
  const views = status.pairs.map(pairStatus);
  const parts = [translate(lang, 'shell.sub_pairs', { count: views.length })];
  const syncing = views.filter((v) => v.state === 'syncing').length;
  const loud = statusNeedsYou(status);
  if (syncing) parts.push(translate(lang, 'shell.sub_syncing', { count: syncing }));
  if (loud) parts.push(translate(lang, 'shell.sub_need_you', { count: loud }));
  return parts.join(' · ');
}

/** Pairs whose state changed since `before` (pair id → state), as "<name>, <state>" for a screen reader. */
export function stateChanges(before: Map<string, StatusState>, pairs: PairState[], lang: Lang): string[] {
  const out: string[] = [];
  for (const p of pairs) {
    const v = pairStatus(p);
    const was = before.get(p.pair.id);
    if (was !== undefined && was !== v.state) out.push(`${p.pair.name}, ${statusLabel(v, lang)}`);
  }
  return out;
}
