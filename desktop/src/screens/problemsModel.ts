// What Problems lists (#66), grouped by kind in the status system's order of urgency: held deletions and new pairs
// held for their first preview (Needs approval), failed syncs, conflicts, then accounts that must sign in and agent
// changes held for review. Pure, so
// the grouping is unit-tested and the list agrees with the sidebar's Problems badge.

import type { ConflictRecord, DaemonStatus, PairState } from '../types';
import { translate, type Lang, type Params } from '../i18n';

export type ProblemKind = 'deletion' | 'preview' | 'failed' | 'conflict' | 'account' | 'agents';

export const KIND_ORDER: readonly ProblemKind[] = ['deletion', 'preview', 'failed', 'conflict', 'account', 'agents'];

export interface Problem {
  /** Unique in the list: "<kind>:<what>". */
  id: string;
  kind: ProblemKind;
  /** The pair it belongs to (deletion, preview, failed, conflict). */
  pairId: string | null;
  /** The conflict, the account or nothing. */
  ref: string | null;
}

export interface ProblemGroup {
  kind: ProblemKind;
  items: Problem[];
  /** What the group stands for in the Problems badge: agent changes count each changeset. */
  count: number;
}

/** A guard stopped the pair's last run, or it waits for an OK: the deletion review. */
export function heldForApproval(p: PairState): boolean {
  return p.blocked || p.pair.status.pending_approval;
}

/** A new pair held for its first preview (#64): nothing syncs until the person looks at the plan and starts it. */
export function heldForPreview(p: PairState): boolean {
  return !heldForApproval(p) && !p.running && p.pair.status.hold_for_preview;
}

/** The last run failed and no run is deciding it again. */
export function failedPair(p: PairState): boolean {
  return !heldForApproval(p) && !heldForPreview(p) && !p.running && p.pair.status.last_status === 'FAILED';
}

/** The items one pair puts in Problems: each open conflict, plus the pair itself when it is held or failed. */
export function pairNeedsYou(p: PairState): number {
  return p.open_conflicts + (heldForApproval(p) || heldForPreview(p) || failedPair(p) ? 1 : 0);
}

/**
 * How many things need the person: the items Problems lists, with conflicts and agent changesets counted one each.
 * The one "need you" count: the Problems badge, the All pairs subtitle and the tray all say it (#147 M1).
 */
export function statusNeedsYou(s: DaemonStatus | null): number {
  if (!s) return 0;
  const pairs = s.pairs.reduce((n, p) => n + pairNeedsYou(p), 0);
  return pairs + s.accounts.filter((a) => a.auth_error).length + (s.agent_changes ?? 0);
}

/** Every problem, in list order: by kind, then by pair name, newest conflicts first. */
export function problemItems(status: DaemonStatus | null, conflicts: readonly ConflictRecord[], lang: string): Problem[] {
  if (!status) return [];
  const byName = (a: PairState, b: PairState) => a.pair.name.localeCompare(b.pair.name, lang) || a.pair.id.localeCompare(b.pair.id);
  const pairs = [...status.pairs].sort(byName);
  const known = new Set(status.pairs.map((p) => p.pair.id));
  const items: Problem[] = [
    ...pairs.filter(heldForApproval).map((p) => ({ id: `deletion:${p.pair.id}`, kind: 'deletion' as const, pairId: p.pair.id, ref: null })),
    ...pairs.filter(heldForPreview).map((p) => ({ id: `preview:${p.pair.id}`, kind: 'preview' as const, pairId: p.pair.id, ref: null })),
    ...pairs.filter(failedPair).map((p) => ({ id: `failed:${p.pair.id}`, kind: 'failed' as const, pairId: p.pair.id, ref: null })),
    ...[...conflicts]
      .filter((c) => known.has(c.pair_id))
      .sort((a, b) => b.detected_at - a.detected_at || a.id.localeCompare(b.id))
      .map((c) => ({ id: `conflict:${c.id}`, kind: 'conflict' as const, pairId: c.pair_id, ref: c.id })),
    ...status.accounts
      .filter((a) => a.auth_error)
      .map((a) => ({ id: `account:${a.id}`, kind: 'account' as const, pairId: null, ref: a.id })),
  ];
  if ((status.agent_changes ?? 0) > 0) items.push({ id: 'agents', kind: 'agents', pairId: null, ref: null });
  return items;
}

/** The list's groups, in KIND_ORDER, leaving out empty ones. */
export function groupProblems(items: readonly Problem[], agentChanges = 0): ProblemGroup[] {
  return KIND_ORDER.map((kind) => {
    const of = items.filter((p) => p.kind === kind);
    return { kind, items: of, count: kind === 'agents' ? (of.length ? agentChanges : 0) : of.length };
  }).filter((g) => g.items.length > 0);
}

/** How many things need the person: the sum the sidebar's badge shows. */
export function needsYouCount(groups: readonly ProblemGroup[]): number {
  return groups.reduce((n, g) => n + g.count, 0);
}

/**
 * The guard's reason for a held plan, in the app's language (#203). The daemon keeps only the engine's English
 * sentence (wasl-core guards.rs) and no code, so each known sentence is matched and worded from the desktop's own
 * strings. A reason with any sentence this doesn't know is shown as the engine wrote it. The engine catalogue (#155)
 * replaces this.
 */
const GUARD_SENTENCES: Array<[RegExp, (m: RegExpMatchArray) => [string, Params?]]> = [
  [/^This sync would delete (\d+) files on Drive, (\d+)% of the folder\.$/, (m) => ['guard.delete_drive', { count: +m[1]!, percent: +m[2]! }]],
  [/^Drive still has every file until you decide\.$/, () => ['guard.delete_drive_keep']],
  [/^This sync would delete (\d+) files on this device, (\d+)% of the folder\.$/, (m) => ['guard.delete_device', { count: +m[1]!, percent: +m[2]! }]],
  [/^This device still has every file until you decide\.$/, () => ['guard.delete_device_keep']],
  [/^The folder on this device is empty but was synced before\. Wasl won't empty Drive to match it\.$/, () => ['guard.wipe_drive']],
  [/^The Drive folder is empty but was synced before\. Wasl won't empty this device's folder to match it\.$/, () => ['guard.wipe_device']],
  [/^The first sync merges two folders that both have files: (\d+) on this device and (\d+) on Drive\.$/, (m) => ['guard.merge', { here: +m[1]!, drive: +m[2]! }]],
  [/^Nothing is deleted\.$/, () => ['guard.merge_nothing']],
  [/^(\d+) files only on this device go to its recycle bin, so it matches Drive\.$/, (m) => ['guard.merge_bin_here', { count: +m[1]! }]],
  [/^(\d+) files only on Drive go to Drive's bin, so it matches this device\.$/, (m) => ['guard.merge_bin_drive', { count: +m[1]! }]],
];

export function guardReason(lang: Lang, reason: string): string {
  // Sentences end at ". " not followed by a lowercase letter; the wipe reasons keep their two sentences together.
  const sentences = reason.trim().split(/(?<=\.) (?=[A-Z0-9])/);
  const out: string[] = [];
  for (let i = 0; i < sentences.length; i++) {
    let hit: [string, Params?] | null = null;
    for (const take of [2, 1]) {
      const s = sentences.slice(i, i + take).join(' ');
      if (take === 2 && i + 1 >= sentences.length) continue;
      const rule = GUARD_SENTENCES.find(([re]) => re.test(s));
      if (rule) {
        hit = rule[1](s.match(rule[0])!);
        i += take - 1;
        break;
      }
    }
    if (!hit) return reason;
    out.push(translate(lang, hit[0], hit[1]));
  }
  return out.join(' ');
}
