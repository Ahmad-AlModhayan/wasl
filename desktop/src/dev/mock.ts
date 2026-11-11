// Development only: lets the window run in a plain browser (no Tauri) for looking at screens in
// both languages and for screenshot tests. It answers the app's commands from data captured from a
// real wasld (`wasl status --json` and friends, with personal details replaced) plus a few made-up
// pairs so every state and screen has something to show. Never bundled into the app: main.ts loads
// it only in development builds (`vite dev`, or `npm run build:mock` for browser testers) when Tauri
// is absent.

import status from './status.json';
import runs from './runs.json';
import bin from './bin.json';
import accounts from './accounts.json';
import type { Account, BinEntry, Conn, DaemonStatus, DeviceInfo, OfferInfo, PairEdit, PairingStatus, PairSharing, PairState, VaultInfo, ViewState } from '../types';
import type { Platform } from '../platform';

const now = Date.now();
const real = (status as unknown as DaemonStatus).pairs[0];

function variant(id: string, name: string, patch: (p: PairState) => void): PairState {
  const p: PairState = JSON.parse(JSON.stringify(real));
  p.pair.id = id;
  p.pair.name = name;
  patch(p);
  return p;
}

/**
 * The design's example (#59): five pairs across a Mac and a Windows PC, every state present, plus a paused pair and
 * a locked vault's. Paths follow `?platform=` (set by `installMock`); the captured pair is Documents.
 */
const pairs: PairState[] = [
  variant('site', 'kahf-site', (p) => {
    p.pair.route = 'drive_and_devices';
    p.running = true;
    p.progress = {
      pair_id: 'site', run_id: 'r', phase: 'transferring', total_actions: 1940, done_actions: 1204,
      bytes_total: 186_000_000, bytes_done: 115_300_000, checked: 1940, errors: 0, current_file: 'content/posts/wasl-launch.mdx',
    };
  }),
  Object.assign(real, { pair: { ...real.pair, name: 'Documents' } }),
  variant('life', 'LifeOS', (p) => {
    p.open_conflicts = 1;
    p.pair.status.last_status = 'PARTIAL';
  }),
  variant('photos', 'أرشيف الصور', (p) => {
    p.pair.method = 'UPLOAD_ONLY';
    p.blocked = true;
    p.pair.status.pending_approval = true;
    p.pair.status.last_status = 'BLOCKED';
  }),
  variant('wasl', 'wasl', (p) => {
    p.pair.route = 'devices';
    p.pair.status.last_status = null;
    p.pair.status.last_success_at = null;
  }),
  variant('music', 'Music', (p) => {
    p.pair.paused = true;
    p.pair.pause_reason = 'Paused by you';
  }),
  variant('private', 'Private', (p) => {
    p.pair.vault_id = 'vault-private';
    p.pair.status.last_status = 'FAILED';
    p.pair.status.last_error = 'The vault Private is locked on this computer. Unlock it to sync this pair.';
    p.pair.status.last_error_kind = 'Locked';
  }),
];

/** The daemon's per-pair facts (#91, #103): size at the last sync, what its plan skipped, the devices it is shared
 *  with, and free space on the folder's drive. `wasl` waits for an offline Laptop; Music is left as an older daemon
 *  reports it (no facts), so the window shows nothing in their place. */
const FACTS: Record<string, Pick<PairState, 'folder_bytes' | 'skipped' | 'peers' | 'free_bytes'>> = {
  site: { folder_bytes: 186_000_000, skipped: 41_203, peers: [{ id: 'WASL-D9KL', name: 'Laptop', online: true }], free_bytes: 412_000_000_000 },
  [real.pair.id]: { folder_bytes: 12_400_000_000, skipped: 0, peers: [], free_bytes: 412_000_000_000 },
  life: { folder_bytes: 48_200_000, skipped: 3, peers: [], free_bytes: 412_000_000_000 },
  photos: { folder_bytes: 1_200_000_000_000, skipped: 12, peers: [], free_bytes: 1_800_000_000_000 },
  wasl: { folder_bytes: 92_000_000, skipped: null, peers: [{ id: 'WASL-D9KL', name: 'Laptop', online: false }], free_bytes: 412_000_000_000 },
  private: { folder_bytes: null, skipped: null, peers: [], free_bytes: 412_000_000_000 },
};
for (const p of pairs) Object.assign(p, FACTS[p.pair.id] ?? {});

/** Where each pair is on each platform: [local folder on a Mac, on a Windows PC, Drive folder]. */
const PLACES: Record<string, [string, string, string]> = {
  site: ['/Users/me/Projects/kahf-site', 'D:\\Projects\\kahf-site', 'My Drive/Projects/kahf-site'],
  [real.pair.id]: ['/Users/me/Documents', 'C:\\Users\\me\\Documents', 'My Drive/Documents'],
  life: ['/Users/me/Obsidian/LifeOS', 'D:\\Obsidian\\LifeOS', 'My Drive/Obsidian/LifeOS'],
  photos: ['/Volumes/Archive/Photos', 'E:\\Photos', 'My Drive/أرشيف الصور'],
  wasl: ['/Users/me/Projects/wasl', 'D:\\Projects\\wasl', ''],
  music: ['/Users/me/Music', 'D:\\Music', 'My Drive/Music'],
  private: ['/Users/me/Private', 'C:\\Users\\me\\Private', 'My Drive/Private'],
};

function placePairs(platform: Platform): void {
  for (const p of pairs) {
    const place = PLACES[p.pair.id];
    if (!place) continue;
    p.pair.local_root = platform === 'mac' ? place[0] : place[1];
    p.pair.remote_path = place[2];
  }
}

// ---------------------------------------------------------------- encrypted vaults

/** The passphrase every mock vault takes. */
const MOCK_PASSPHRASE = 'correct horse battery';
const VAULT_LOCKED =
  'This folder is an encrypted vault, locked on this computer. Unlock the vault to show its files. While it is locked, Wasl keeps none of its content here except the files you chose to keep on this computer.';

const vaults: VaultInfo[] = [
  {
    id: 'vault-private', name: 'Private', version: 2, unlocked: false, account_id: 'me@example.com',
    drive_folder_id: 'priv', drive_id: null, remote_path: 'My Drive/Private', created_at: now - 86_400_000 * 40,
  },
  {
    id: 'vault-phone', name: 'Phone vault', version: 1, unlocked: true, account_id: 'me@example.com',
    drive_folder_id: 'phone', drive_id: null, remote_path: 'My Drive/Phone vault', created_at: now - 86_400_000 * 400,
  },
];

function findVault(id: unknown): VaultInfo {
  const v = vaults.find((x) => x.id === id);
  if (!v) throw { kind: 'not_found', message: `No vault ${String(id)} on this computer` };
  return v;
}

/** What unlocking or locking a vault does to its pairs and views. */
function vaultChanged(v: VaultInfo): void {
  for (const p of pairs.filter((x) => x.pair.vault_id === v.id)) {
    p.pair.status.last_status = v.unlocked ? 'SUCCESS' : 'FAILED';
    p.pair.status.last_error = v.unlocked ? null : `The vault ${v.name} is locked on this computer. Unlock it to sync this pair.`;
    p.pair.status.last_error_kind = v.unlocked ? null : 'Locked';
    if (v.unlocked) p.pair.status.last_success_at = Date.now();
  }
  for (const s of viewStates.filter((x) => x.view.vault_id === v.id)) {
    s.mounted = v.unlocked && s.view.enabled;
    s.view.last_error = v.unlocked ? null : VAULT_LOCKED;
    s.last_error_kind = v.unlocked ? null : 'locked';
    s.files = v.unlocked ? 318 : 0;
    s.bytes = v.unlocked ? 2.4 * GIB : 0;
    if (!v.unlocked) s.cache_used = s.pinned_bytes;
  }
  changed();
  pushStatus();
}

const vaultAnswers: Record<string, (args: Record<string, unknown>) => unknown> = {
  vaults: () => vaults,
  unlock_vault: async (a) => {
    const v = findVault(a.id);
    await new Promise((r) => setTimeout(r, 1200)); // the key derivation takes a moment on purpose
    if (a.passphrase !== MOCK_PASSPHRASE) throw { kind: 'invalid', message: 'Wrong passphrase for this vault.' };
    v.unlocked = true;
    vaultChanged(v);
    return v;
  },
  lock_vault: (a) => {
    const v = findVault(a.id);
    v.unlocked = false;
    vaultChanged(v);
    return null;
  },
  create_vault: async (a) => {
    const n = a.vault as { accountId: string; folderId: string; driveId: string | null; remotePath: string; name: string; passphrase: string };
    await new Promise((r) => setTimeout(r, 1500));
    if ([...n.passphrase].length < 8) throw { kind: 'invalid', message: "A vault's passphrase needs at least 8 characters" };
    if (/Documents|Photos/.test(n.remotePath)) {
      throw { kind: 'invalid', message: `${n.remotePath} isn't empty. A new vault needs a new or empty Drive folder.` };
    }
    const v: VaultInfo = {
      id: `vault-${Date.now()}`, name: n.name, version: 2, unlocked: true, account_id: n.accountId,
      drive_folder_id: n.folderId, drive_id: n.driveId, remote_path: n.remotePath, created_at: Date.now(),
    };
    vaults.push(v);
    changed();
    return v;
  },
  create_pair: (a) => {
    const n = a.pair as { name: string; localRoot: string; remotePath: string; vaultId?: string | null };
    const id = `p-${Date.now()}`;
    pairs.push(
      variant(id, n.name, (p) => {
        p.pair.local_root = n.localRoot;
        p.pair.remote_path = n.remotePath;
        p.pair.vault_id = n.vaultId ?? null;
        p.pair.status.hold_for_preview = true;
        p.pair.status.last_status = null;
        p.pair.status.last_error = null;
      }),
    );
    pushStatus();
    return id;
  },
  update_pair: (a) => {
    const e = a.edit as PairEdit;
    const p = pairs.find((x) => x.pair.id === a.id);
    if (!p) throw { kind: 'not_found', message: 'That pair is gone' };
    if (!e.name.trim()) throw { kind: 'invalid', message: 'A pair needs a name' };
    // The service refuses a name only through its other rules; the mock refuses one to show a refusal inline.
    if (e.name.trim() === 'kahf-site' && p.pair.id !== 'site') {
      throw { kind: 'invalid', message: `${p.pair.local_root} is already synced by the pair kahf-site, or is in or around its folder. Two pairs on the same files can undo each other. Choose another folder.` };
    }
    // Another folder (#103): refused while the pair runs, otherwise it starts over, held for its preview.
    const moved = e.localRoot !== undefined || e.drive !== undefined;
    if (moved && p.running) throw { kind: 'invalid', message: `${p.pair.name} is syncing. Stop it or let it finish, then change its folders.` };
    p.pair.name = e.name.trim();
    p.pair.method = e.method;
    if (moved) {
      if (e.localRoot !== undefined) p.pair.local_root = e.localRoot;
      if (e.drive) {
        p.pair.account_id = e.drive.accountId;
        p.pair.drive_folder_id = e.drive.driveFolderId;
        p.pair.remote_path = e.drive.remotePath;
        p.pair.vault_id = e.drive.vaultId;
      }
      Object.assign(p.pair.status, { hold_for_preview: true, pending_approval: false, last_status: null, last_error: null, last_success_at: null });
      Object.assign(p, { blocked: false, open_conflicts: 0, folder_bytes: null, skipped: null });
    }
    pushStatus();
    return null;
  },
};

/** Whether the sample conflict is still open (resolving it, then Undo, brings it back). */
let conflictOpen = true;
function setConflictOpen(open: boolean): void {
  conflictOpen = open;
  const life = pairs.find((x) => x.pair.id === 'life');
  if (life) life.open_conflicts = open ? 1 : 0;
  changed();
  pushStatus();
}

const conflict = {
  id: 'c1', pair_id: 'life', key: 'Q3/summary.docx', detected_at: now - 3_600_000,
  policy: 'KeepBoth', local_copy_rel: null, remote_file_id: 'X', message: 'Changed on this computer and on Drive', resolved: false, resolved_at: null,
};

// ---------------------------------------------------------------- devices and pairing

/** Set by installMock: sends an event to the window as the Rust side would. */
let emit: (event: string, payload: unknown) => void = () => undefined;
const changed = () => emit('daemon://event', { event: 'changed' });
const pushStatus = () => emit('daemon://status', answers.status({}));

const devices: DeviceInfo[] = [
  { id: 'WASL-7KQ2-M4PX-R9TB-2HDA-6VNE-JC3W-Y5LF-8ZGU-QK4S-1XBM-T7PD-ACE', name: 'Study PC', kind: 'desktop', this_device: true, connected: false, path: null, added_at: now / 1000 - 86_400 * 30 },
  { id: 'WASL-B3NM-5QXA-ZK8T-W2RD-HF6J-PL4C-9VEY-SU7G-MD3K-QA2N-X8TB-RWF', name: 'هاتف أحمد', kind: 'phone', this_device: false, connected: true, path: 'lan', added_at: now / 1000 - 86_400 * 12 },
  { id: 'WASL-D9KL-2WQZ-T5MX-B8RA-JN4C-6HVE-PY3F-GU7S-K2DQ-ZM5A-W9XB-TRC', name: 'Laptop', kind: 'laptop', this_device: false, connected: false, path: null, added_at: now / 1000 - 86_400 * 3 },
  // Added by the phone, not kept on this computer yet (docs/INTRODUCTIONS.md §3.1).
  { id: 'WASL-H6QP-3ZRT-N8KB-V2XM-C5WF-L9DA-Y4JE-TG7S-P3MU-KB8Z-Q2RN-FXD', name: 'Tablet', this_device: false, connected: true, path: 'lan', added_at: now / 1000 - 3600, added_by: 'WASL-B3NM-5QXA-ZK8T-W2RD-HF6J-PL4C-9VEY-SU7G-MD3K-QA2N-X8TB-RWF', known: false },
];

// Introductions: folders other devices offer, and who each shared pair is shared with.
const offers: OfferInfo[] = [
  { share: '4f1c9a7e2b6d8053a1e7c94f0b2d6a38', from_device: devices[1]!.id, from_name: devices[1]!.name, name: 'Camera', files: 1240, bytes: 2_300_000_000, first_seen: now - 600_000, status: 'new' },
];
const sharings = new Map<string, PairSharing>();

const IDLE: PairingStatus = { stage: 'idle', inviting: false, code: null, expires_at: null, words: [], peer_name: null, confirmed_here: false, message: null };
let pairing: PairingStatus = { ...IDLE };
let pairingTimer: number | undefined;

function setPairing(p: Partial<PairingStatus>, after = 0): void {
  window.clearTimeout(pairingTimer);
  const apply = () => {
    pairing = { ...pairing, ...p };
    if (pairing.stage === 'paired' && !devices.some((d) => d.name === pairing.peer_name)) {
      devices.push({ id: 'WASL-F4TR-8NXC-K2WM-Q7BZ-5HDJ-YL3A-9PEV-GS6U-M4KT-XR2Q-B7NW-DAZ', name: pairing.peer_name ?? '?', this_device: false, connected: true, path: 'lan', added_at: Date.now() / 1000 });
    }
    if (pairing.stage === 'paired') pairing.peer_id = devices.find((d) => d.name === pairing.peer_name)?.id ?? null;
    changed();
  };
  if (after > 0) pairingTimer = window.setTimeout(apply, after);
  else apply();
}

/** A pairing that moves along by itself: waiting → confirm (words) → (after Yes) paired. */
function startPairing(inviting: boolean): void {
  pairing = {
    ...IDLE,
    stage: 'waiting',
    inviting,
    code: inviting ? 'WASL:AEBQBGKRWJ4FJ2PZ7SGX3MDEQ5KRAU2NQ3MZ6YJ4XNBCGVO5T2KSQPFL7HA3MBZWQY4EAQCAGAAAIAAAAA4OS' : null,
    expires_at: inviting ? Math.floor(Date.now() / 1000) + 600 : null,
  };
  setPairing({ stage: 'confirm', words: ['orbit', 'velvet', 'canyon', 'pilot'], peer_name: inviting ? 'Pixel 8' : 'Office PC' }, inviting ? 6000 : 2500);
}

const shareCodes = new Map<string, string>();

// ---------------------------------------------------------------- read-only views

const GIB = 2 ** 30;
const viewStates: ViewState[] = [
  {
    view: {
      id: 'v-archive', name: 'Archive', account_id: 'me@example.com', drive_folder_id: 'arch', drive_id: null,
      remote_path: 'My Drive/Archive', local_path: 'C:\\Users\\me\\Archive (Drive)', vault_id: null, cache_limit: 20 * GIB,
      enabled: true, created_at: now - 86_400_000 * 5, last_refresh_at: now - 120_000, last_error: null,
    },
    mounted: true, files: 1824, bytes: 96 * GIB, cache_used: 3.2 * GIB, pinned_bytes: 1.1 * GIB,
    local_changes: [{ path: 'notes/todo.txt', reason: 'only here: not in the listing', kind: 'only_here' }],
  },
  {
    view: {
      id: 'v-family', name: 'صور العائلة', account_id: 'me@example.com', drive_folder_id: 'fam', drive_id: null,
      remote_path: 'My Drive/العائلة/صور', local_path: 'D:\\Family photos', vault_id: null, cache_limit: 5 * GIB,
      enabled: true, created_at: now - 86_400_000, last_refresh_at: now - 600_000,
      last_error: "Can't reach Google Drive. It will try again in 5 minutes.",
    },
    mounted: false, files: 0, bytes: 0, cache_used: 0, pinned_bytes: 0, local_changes: [], last_error_kind: 'network',
  },
  {
    view: {
      id: 'v-private', name: 'Private', account_id: 'me@example.com', drive_folder_id: 'priv', drive_id: null,
      remote_path: 'My Drive/Private', local_path: 'C:\\Users\\me\\Private (Drive)', vault_id: 'vault-private', cache_limit: 20 * GIB,
      enabled: true, created_at: now - 86_400_000 * 2, last_refresh_at: now - 300_000, last_error: VAULT_LOCKED,
    },
    mounted: false, files: 0, bytes: 0, cache_used: 0.2 * GIB, pinned_bytes: 0.2 * GIB, local_changes: [], last_error_kind: 'locked',
  },
];

function findView(id: unknown): ViewState {
  const v = viewStates.find((s) => s.view.id === id);
  if (!v) throw { kind: 'not_found', message: `No view ${String(id)}` };
  return v;
}

const viewAnswers: Record<string, (args: Record<string, unknown>) => unknown> = {
  views: () => ({ supported: true, views: viewStates }),
  add_view: (a) => {
    const n = a.view as { name: string; localPath: string; accountId: string; driveFolderId: string; remotePath: string; driveId: string | null; cacheLimit: number | null };
    // A vault's folder is linked to its vault; while that is locked the view shows nothing.
    const vault = vaults.find((v) => v.remote_path === n.remotePath || (n.remotePath.toLowerCase().includes('vault') && v.id === 'vault-private'));
    const s: ViewState = {
      view: {
        id: `v-${Date.now()}`, name: n.name, account_id: n.accountId, drive_folder_id: n.driveFolderId, drive_id: n.driveId,
        remote_path: n.remotePath, local_path: n.localPath, vault_id: vault?.id ?? null, cache_limit: n.cacheLimit ?? 20 * GIB,
        enabled: true, created_at: Date.now(), last_refresh_at: Date.now(), last_error: vault && !vault.unlocked ? VAULT_LOCKED : null,
      },
      mounted: !vault || vault.unlocked, files: vault && !vault.unlocked ? 0 : 42, bytes: vault && !vault.unlocked ? 0 : 1.5 * GIB,
      cache_used: 0, pinned_bytes: 0, local_changes: [], last_error_kind: vault && !vault.unlocked ? 'locked' : null,
    };
    viewStates.push(s);
    changed();
    return s.view;
  },
  remove_view: (a) => {
    viewStates.splice(viewStates.indexOf(findView(a.id)), 1);
    changed();
    return null;
  },
  set_view_enabled: (a) => {
    const s = findView(a.id);
    s.view.enabled = Boolean(a.enabled);
    s.mounted = s.view.enabled && !s.view.last_error;
    changed();
    return null;
  },
  set_view_cache_limit: (a) => {
    findView(a.id).view.cache_limit = Number(a.limit);
    changed();
    return null;
  },
  refresh_view: (a) => {
    const s = findView(a.id);
    s.view.last_refresh_at = Date.now();
    changed();
    return null;
  },
  pin_path: (a) => {
    const s = viewStates.find((v) => String(a.path).startsWith(v.view.local_path));
    if (!s) throw { kind: 'not_found', message: `${String(a.path)} isn't in a view.` };
    s.pinned_bytes = String(a.path) === s.view.local_path ? s.bytes : s.pinned_bytes + 40_000_000;
    s.cache_used = Math.max(s.cache_used, s.pinned_bytes);
    changed();
    return null;
  },
  unpin_path: (a) => {
    const s = viewStates.find((v) => String(a.path).startsWith(v.view.local_path));
    if (!s) throw { kind: 'not_found', message: `${String(a.path)} isn't in a view.` };
    s.pinned_bytes = 0;
    s.cache_used = 0;
    changed();
    return null;
  },
};

// Agent changes (docs/AGENT_CHANGES.md): one Claude Code request waiting, one Gemini session still open, one
// decided last week.
type MockChangeset = {
  changeset: Record<string, unknown> & { id: string; status: string; actor: string };
  pair_name: string;
  entries: Array<Record<string, unknown>>;
};
const entry = (rel: string, kind: string, before: number | null, after: number | null) => ({
  rel, key: rel, kind, is_dir: false, before_size: before, before_mtime: before && now - 3_600_000,
  after_size: after, after_mtime: after && now - 60_000, source: kind === 'added' ? 'none' : 'saved', location: null, outcome: null,
});
const changesets: MockChangeset[] = [
  {
    changeset: { id: '19a1b2c3d4e-0', pair_id: 'p', actor: 'agent:claude-code', label: 'Fix the typo in the engine doc and add a test', scope: 'app/docs', session: 's1', status: 'held', started_at: now - 600_000, ended_at: now - 300_000, decided_at: null },
    pair_name: 'Work',
    entries: [
      entry('app/docs/06-engine.md', 'changed', 13_210, 13_288),
      entry('app/docs/12-test-fixtures.md', 'changed', 4_100, 4_420),
      entry('app/docs/notes-from-claude.md', 'added', null, 812),
      entry('app/docs/old-draft.md', 'deleted', 2_048, null),
    ],
  },
  {
    changeset: { id: '19a1b2c9999-1', pair_id: 'p', actor: 'agent:gemini', label: 'gemini', scope: 'notes/docs', session: null, status: 'open', started_at: now - 120_000, ended_at: null, decided_at: null },
    pair_name: 'Work',
    entries: [],
  },
  {
    changeset: { id: '19a0ffff000-2', pair_id: 'p', actor: 'agent:my-assistant', label: null, scope: 'wasl/docs', session: null, status: 'reverted', started_at: now - 6 * 86_400_000, ended_at: now - 6 * 86_400_000, decided_at: now - 5 * 86_400_000 },
    pair_name: 'Work',
    entries: [],
  },
];
const DIFF = `--- a/app/docs/06-engine.md
+++ b/app/docs/06-engine.md
@@ -1,4 +1,4 @@
 # 06 · Engine

-The most importnat code in the app.
+The most important code in the app.
 Goal: every job runs within a minute of its time.
`;
const agentAnswers: Record<string, (args: Record<string, unknown>) => unknown> = {
  agent_changesets: (a) => changesets.map((c) => c.changeset).filter((c) => a.all || c.status === 'held' || c.status === 'open'),
  agent_changeset: (a) => changesets.find((c) => c.changeset.id === a.id),
  agent_diff: (a) => (String(a.rel).endsWith('06-engine.md') ? DIFF : `--- a/${String(a.rel)}\n+++ b/${String(a.rel)}\n@@ -0,0 +1 @@\n+…\n`),
  agent_accept: (a) => {
    // The service's dialog: here, the person confirms after a moment.
    const c = changesets.find((x) => x.changeset.id === a.id)!;
    setTimeout(() => {
      c.changeset.status = 'accepted';
      c.changeset.decided_at = Date.now();
      changed();
    }, 1500);
    return { code: '042137', expires_in_secs: 120 };
  },
  agent_revert: (a) => {
    const c = changesets.find((x) => x.changeset.id === a.id)!;
    c.changeset.status = 'reverted';
    c.changeset.decided_at = Date.now();
    changed();
    return { reverted: c.entries.map((e) => e.rel), skipped: [], failed: [] };
  },
  agent_end: (a) => {
    const c = changesets.find((x) => x.changeset.id === a.id)!;
    c.changeset.status = 'held';
    c.changeset.ended_at = Date.now();
    c.entries = [entry('notes/docs/GEMINI_NOTES.md', 'added', null, 16)];
    changed();
    return c.changeset;
  },
  agent_setup: () => ({
    cli: 'C:/Program Files/Wasl/wasl.exe',
    claude_code_hooks: JSON.stringify({ hooks: { UserPromptSubmit: [{ hooks: [{ type: 'command', command: '"C:/Program Files/Wasl/wasl.exe" agent hook claude-code prompt' }] }] } }, null, 2),
  }),
};

// ---------------------------------------------------------------- sync runs

/** How long a made-up run takes. */
const RUN_MS = 4000;

/** A run as the service would show it: progress for a moment, then done. A locked vault's pair stays locked. */
function startRun(p: PairState): void {
  if (p.running || p.pair.paused) return;
  if (vaults.some((v) => v.id === p.pair.vault_id && !v.unlocked)) return;
  const runId = `r-${Date.now()}-${p.pair.id}`;
  p.running = true;
  p.progress = {
    pair_id: p.pair.id, run_id: runId, phase: 'transferring', total_actions: 12, done_actions: 3,
    bytes_total: 4_200_000, bytes_done: 1_100_000, checked: 12, errors: 0, current_file: 'notes.txt',
  };
  window.setTimeout(() => {
    if (p.progress?.run_id !== runId) return; // stopped meanwhile
    p.running = false;
    p.progress = null;
    p.pair.status.last_status = 'SUCCESS';
    p.pair.status.last_error = null;
    p.pair.status.last_error_kind = null;
    p.pair.status.last_success_at = Date.now();
    p.pair.status.hold_for_preview = false;
    changed();
    pushStatus();
  }, RUN_MS);
}

/** The system folder and file pickers (Tauri's dialog plugin): a folder in Documents, or a file in the folder. */
function pick(options: { directory?: boolean; defaultPath?: string } = {}): string {
  if (options.directory) return 'C:\\Users\\me\\Documents\\Reports';
  return `${options.defaultPath ?? 'C:\\Users\\me\\Documents'}\\notes\\todo.txt`;
}

// ---------------------------------------------------------------- accounts, recycle bin, settings

const accountList = accounts as Account[];
const binEntries = bin as unknown as BinEntry[];
/** The service's key-value settings (language, start at login), kept for this page's life. */
const settings = new Map<string, string>();

/** What the last restore brought back (the Rust side keeps it for Undo). */
let lastRestored: BinEntry[] = [];
let rebinCount = 0;

/** Takes entries out of the recycle bin; returns the entries taken. */
function takeFromBin(ids: string[]): BinEntry[] {
  const taken = binEntries.filter((b) => ids.includes(b.entry.id));
  for (const b of taken) binEntries.splice(binEntries.indexOf(b), 1);
  changed();
  return taken;
}

const otherAnswers: Record<string, (args: Record<string, unknown>) => unknown> = {
  delete_pair: (a) => {
    const i = pairs.findIndex((x) => x.pair.id === a.id);
    if (i >= 0) pairs.splice(i, 1);
    changed();
    pushStatus();
    return null;
  },
  /** Stop: the run ends where it is. */
  cancel: (a) => {
    const p = pairs.find((x) => x.pair.id === a.id);
    if (p) {
      p.running = false;
      p.progress = null;
    }
    pushStatus();
    return null;
  },
  approve_once: () => {
    pushStatus();
    return null;
  },
  /** The real service opens the browser for Google's sign-in; here the person signs in at once. */
  add_account: async () => {
    await new Promise((r) => setTimeout(r, 800));
    const email = `me${accountList.length + 1}@example.com`;
    accountList.push({ id: email, email, auth_error: null });
    changed();
    pushStatus();
    return email;
  },
  remove_account: (a) => {
    const i = accountList.findIndex((x) => x.id === a.id);
    if (i >= 0) accountList.splice(i, 1);
    changed();
    return null;
  },
  restore_bin: (a) => {
    // As the app's Rust side does: the last restore is kept for its Undo, which names it by ids.
    lastRestored = takeFromBin(a.ids as string[]);
    return { restored: lastRestored, failed: [] };
  },
  delete_bin_forever: (a) => takeFromBin(a.ids as string[]).length,
  empty_bin: () => takeFromBin(binEntries.map((b) => b.entry.id)).length,
  setting: (a) => settings.get(String(a.key)) ?? null,
  set_setting: (a) => {
    settings.set(String(a.key), String(a.value));
    return null;
  },
};

// ---------------------------------------------------------------- Problems, Recycle bin and History (#66, #67)

/** Photos archive's held plan: files removed on this computer that the next sync would delete on Drive. */
const heldFiles = [
  ['2019/Umrah', 'IMG_0412.jpg', 4_200_000], ['2019/Umrah', 'IMG_0413.jpg', 3_900_000], ['2019/Umrah', 'VID_0420.mp4', 188_000_000],
  ['2019/Eid', 'IMG_0501.heic', 2_100_000], ['2019/Eid', 'IMG_0502.heic', 2_300_000],
] as const;
function heldReview() {
  const items = [
    { key: '2019/Hajj', name: 'Hajj', folder: '2019', is_dir: true, files: 207, bytes: 905_000_000, on_drive: true },
    ...heldFiles.map(([folder, name, bytes]) => ({ key: `${folder}/${name}`, name, folder, is_dir: false, files: 1, bytes, on_drive: true })),
  ];
  return {
    summary: { deleteLocal: 0, deleteRemote: 212, sampleDeletes: items.slice(1).map((i) => `Drive: ${i.key}`) },
    items,
    files_on_drive: 212,
    files_on_device: 0,
    bytes: items.reduce((n, i) => n + i.bytes, 0),
  };
}
/** Clears Photos archive's stop, as approving or restoring does. */
function clearHold(id: unknown): void {
  const p = pairs.find((x) => x.pair.id === id);
  if (!p) return;
  p.blocked = false;
  p.pair.status.pending_approval = false;
  p.pair.status.last_status = 'SUCCESS';
  changed();
  pushStatus();
}

const day = 86_400_000;
const binSample: Array<[string, string, string, 'Local' | 'Drive', number, number]> = [
  ['site', 'kahf-site', 'drafts/old-intro.mdx', 'Local', 1_800_000, 14_000],
  [real.pair.id, 'Documents', 'Finance/budget-2025.xlsx', 'Drive', day, 2_300_000],
  ['life', 'LifeOS', 'Meetings/Meeting notes.md', 'Local', 3 * day, 6_000],
  ['photos', 'أرشيف الصور', '2018/IMG_0380.heic', 'Local', 5 * day, 3_100_000],
  [real.pair.id, 'Documents', 'Drafts/draft-v2.docx', 'Drive', 12 * day, 88_000],
  ['wasl', 'wasl', 'notes/wasl-notes.txt', 'Local', 20 * day, 2_000],
];
if (binEntries.length === 0) {
  binSample.forEach(([pairId, pairName, key, side, ago, size], i) =>
    binEntries.push({
      entry: {
        id: `bin-${i + 1}`, pair_id: pairId, side, key, reason: side === 'Drive' ? 'deleted on Drive' : 'deleted on this computer',
        location: `.wasl/bin/${i + 1}`, deleted_at: now - ago, expires_at: now - ago + 30 * day, size,
        display_name: key.split('/').pop() ?? key, pair_name: pairName,
      },
      version_no: 1,
    }),
  );
}

/** Syncs of every pair, with each outcome and trigger, for History's filters. */
const moreRuns = (() => {
  const shape: Array<[string, string, string, number, Partial<Record<string, number>>, string | null]> = [
    ['site', 'instant', 'SUCCESS', 0.2, { uploaded: 3 }, null],
    ['life', 'schedule', 'PARTIAL', 1, { downloaded: 2, conflicts: 1 }, 'One file changed on both sides'],
    ['photos', 'schedule', 'BLOCKED', 3, {}, 'This sync would delete 212 files on Drive, 6% of the folder. Drive still has every file until you decide.'],
    ['private', 'manual', 'FAILED', 5, { errors: 1 }, 'The vault is locked'],
    ['site', 'manual', 'SUCCESS', 26, { uploaded: 12, deleted_remote: 1 }, null],
    ['music', 'automation', 'CANCELLED', 50, {}, null],
    ['wasl', 'preview', 'SKIPPED', 200, {}, 'Waiting for the first-sync preview'],
    ['life', 'instant', 'SUCCESS', 24 * 9, { uploaded: 1 }, null],
  ];
  return shape.map(([pairId, trigger, st, hoursAgo, stats, message], i) => ({
    id: `run-${i + 1}`, pair_id: pairId, trigger, status: st,
    stats: { uploaded: 0, downloaded: 0, exported: 0, deleted_local: 0, deleted_remote: 0, conflicts: 0, skipped: 0, errors: 0, bytes_up: 0, bytes_down: 0, ...stats },
    message, started_at: now - hoursAgo * 3_600_000, ended_at: now - hoursAgo * 3_600_000 + 4_000,
  }));
})();

const reviewAnswers: Record<string, (args: Record<string, unknown>) => unknown> = {
  blocked_plan: (a) =>
    pairs.find((x) => x.pair.id === a.id)?.blocked
      ? { reason: 'This sync would delete 212 files on Drive, 6% of the folder. Drive still has every file until you decide.', summary: heldReview().summary, created_at: now - 3 * 3_600_000 }
      : null,
  deletion_review: async () => {
    await new Promise((r) => setTimeout(r, 600)); // the real call compares both sides again
    return heldReview();
  },
  restore_deletions: (a) => {
    clearHold(a.pairId);
    return 212;
  },
  approve_once: (a) => {
    clearHold(a.id);
    return null;
  },
  undo_bin_restore: (a) => {
    const ids = a.ids as string[];
    const back = lastRestored.filter((b) => ids.includes(b.entry.id));
    if (back.length === 0) throw { kind: 'not_found', message: "Nothing to undo: that restore isn't the last one" };
    lastRestored = lastRestored.filter((b) => !ids.includes(b.entry.id));
    // As wasld does: binned again, under new ids, deleted now.
    binEntries.push(...back.map((b) => ({ ...b, entry: { ...b.entry, id: `bin-u${++rebinCount}`, deleted_at: Date.now() } })));
    changed();
    return null;
  },
  runs: (a) => {
    const id = a.pairId as string | null;
    const limit = (a.limit as number | undefined) ?? 100;
    const all = [...runs, ...moreRuns].sort((x, y) => y.started_at - x.started_at);
    return (id ? all.filter((r) => r.pair_id === id) : all).slice(0, limit);
  },
  run_events: (a) => {
    const r = moreRuns.find((x) => x.id === a.runId);
    if (r?.status === 'FAILED') return [{ at: r.started_at + 1_000, level: 'Error', action: 'START', key: null, message: 'The vault is locked', detail: 'Unlock it to sync', bytes: 0 }];
    if (r?.status === 'BLOCKED') return [{ at: r.started_at + 2_000, level: 'Warn', action: 'GUARD', key: null, message: r.message, detail: null, bytes: 0 }];
    return [
      { at: now - 60_000, level: 'Info', action: 'UPLOAD', key: 'hello.txt', message: 'Uploaded', detail: null, bytes: 35 },
      { at: now - 59_000, level: 'Info', action: 'DELETE_REMOTE', key: 'big.bin', message: 'Moved to the bin on Drive', detail: null, bytes: 3_000_000 },
    ];
  },
};

const answers: Record<string, (args: Record<string, unknown>) => unknown> = {
  ...agentAnswers,
  ...otherAnswers,
  app_info: () => ({ version: '0.1.0', cli: true }),
  daemon_state: () => conn,
  // Restart service: starting, then connected (or, with no wasld to find, stopped again).
  start_daemon: () => {
    conn = { state: 'starting' };
    emit('daemon://state', conn);
    setTimeout(() => {
      conn = mockState === 'notfound' ? { state: 'not_running', message: 'wasld_not_found' } : { state: 'connected', version: VERSION };
      emit('daemon://state', conn);
      if (conn.state === 'connected') pushStatus();
    }, 1200);
    return null;
  },
  set_tray_text: () => null,
  tray_hide: () => null,
  tray_fit: () => null,
  tray_open: () => null,
  tray_menu: () => null,
  set_tray_visible: () => null,
  // A dropped path is a folder unless it ends in an extension (#71); nothing was dropped on the Dock.
  is_folder: (a) => !/\.[a-z0-9]+$/i.test(String(a.path)),
  take_opened_folder: () => null,
  set_menu_text: () => null,
  title_bar_double_click: () => null,
  // The traffic lights where tauri.macos.conf.json puts them (fakeMacWindow draws them).
  window_chrome: () => ({ lights: { ...MOCK_LIGHTS }, windowWidth: window.innerWidth, opaque: false }),
  set_start_at_login: () => null,
  // macOS Settings window (#68): a popup on the same page with ?window=settings.
  open_settings: () => {
    const q = new URLSearchParams(location.search);
    q.set('window', 'settings');
    window.open(`${location.pathname}?${q}`, 'wasl-settings', 'popup,width=620,height=520');
    return null;
  },
  // Windows: a sample accent, so the opt-in shows in the mock; macOS reads CSS AccentColor itself.
  system_accent: () => (document.documentElement.dataset.kfPlatform === 'windows' ? '#005fb8' : null),
  status: () => ({
    ...(status as object),
    accounts: accountList,
    overall: pairs.length ? 'needs_you' : 'idle',
    pairs,
    agent_changes: changesets.filter((c) => c.changeset.status === 'held').length,
  }),
  accounts: () => accountList,
  // One pair's recent syncs (Pair detail), or every pair's (History). The captured runs are Documents'; the
  // other pairs that have synced reuse them.
  runs: (a) => {
    const id = a.pairId as string | null;
    const limit = (a.limit as number | undefined) ?? 100;
    if (!id) return runs.slice(0, limit);
    const own = runs.filter((r) => r.pair_id === id);
    if (own.length) return own.slice(0, limit);
    if (!pairs.find((p) => p.pair.id === id)?.pair.status.last_status) return [];
    return runs.slice(0, limit).map((r) => ({ ...r, id: `${r.id}-${id}`, pair_id: id }));
  },
  run_events: () => [
    { at: now - 60_000, level: 'Info', action: 'UPLOAD', key: 'hello.txt', message: 'Uploaded', detail: null, bytes: 35 },
    { at: now - 59_000, level: 'Info', action: 'DELETE_REMOTE', key: 'big.bin', message: 'Moved to the bin on Drive', detail: null, bytes: 3_000_000 },
  ],
  bin_entries: () => binEntries,
  open_conflicts: () => (conflictOpen ? [conflict] : []),
  resolve_conflict: () => {
    setConflictOpen(false);
    return { conflict: conflict.id };
  },
  undo_conflict: () => {
    setConflictOpen(true);
    return null;
  },
  sync_all: () => {
    pairs.forEach(startRun);
    pushStatus();
    return null;
  },
  conflict_details: () => ({
    conflict, name: 'summary.docx', folder: 'Q3',
    device: { mtime_ms: now - 7_200_000, size: 48_211 }, drive: { mtime_ms: now - 5_400_000, size: 51_002 },
  }),
  preview: () => ({
    plan: {
      // camelCase, as wasl-core's PlanSummary is serialised.
      summary: {
        uploads: 12, uploadBytes: 4_200_000, downloads: 3, downloadBytes: 900_000, exports: 0, deleteLocal: 0, deleteRemote: 1,
        conflicts: 0, mkdirs: 2, skips: 0, localFiles: 40, remoteFiles: 31, warnings: [], blockedReason: null,
        sampleDeletes: ['Old/draft.txt'], unchanged: 25,
      },
      blocked: false,
      actions: [],
    },
    notes: [],
    sizes: {},
  }),
  blocked_plan: () => null,
  check_update: () => ({ state: 'newer', version: '0.5.0', url: 'https://wasl.kahflabs.com/' }),
  open_download: (a) => {
    console.info(`[mock] would open the download page (${a.lang})`);
    return null;
  },
  open_site_page: (a) => {
    console.info(`[mock] would open the ${a.page} page (${a.lang})`);
    return null;
  },
  show_pair_folder: (a) => {
    console.info(`[mock] would show ${pairs.find((p) => p.pair.id === a.id)?.pair.local_root ?? '?'} in the file manager`);
    return null;
  },
  drive_folders: () => [
    { id: 'a', name: 'Documents', drive_id: null },
    { id: 'b', name: 'Photos', drive_id: null },
    { id: 'c', name: 'العمل', drive_id: null },
  ],
  set_paused: (a) => {
    const p = pairs.find((x) => x.pair.id === a.id);
    if (p) {
      p.pair.paused = Boolean(a.paused);
      p.pair.pause_reason = p.pair.paused ? 'Paused by you' : null;
    }
    pushStatus();
    return null;
  },
  sync: (a) => {
    const p = pairs.find((x) => x.pair.id === a.id);
    if (p) startRun(p);
    pushStatus();
    return null;
  },
  resolve_drive_path: (a) => {
    const names = ['Documents', 'Photos', 'العمل'];
    const parts = String(a.path).trim().replace(/^\/+|\/+$/g, '').replace(/^My Drive\/?/, '').split('/').filter(Boolean);
    let current = { id: 'root', name: 'My Drive', drive_id: null as string | null };
    for (const part of parts) {
      if (!names.includes(part) && !a.create) {
        throw { kind: 'not_found', message: `There is no folder "${part}" in ${current.name} on Drive` };
      }
      current = { id: `${current.id}/${part}`, name: part, drive_id: null };
    }
    return current;
  },
  devices: () => ({ enabled: true, listen: '0.0.0.0:47100', devices }),
  remove_device: (a) => {
    const i = devices.findIndex((d) => d.id === a.id);
    if (i >= 0) devices.splice(i, 1);
    changed();
    return null;
  },
  connect_device: (a) => {
    if (!String(a.address).includes(':')) throw { kind: 'invalid', message: `Can't find ${String(a.address)}` };
    const d = devices.find((x) => !x.this_device);
    if (d) d.connected = true;
    changed();
    return d?.id ?? '';
  },
  share_pair: (a) => {
    const id = String(a.id);
    const given = typeof a.share === 'string' ? a.share.trim() : null;
    // Like ShareId::from_hex in the daemon: 32 hex digits or nothing.
    if (given !== null && !/^[0-9a-fA-F]{32}$/.test(given)) throw { kind: 'invalid', message: `Not a share code: ${given}` };
    const code = given ?? shareCodes.get(id) ?? Array.from({ length: 32 }, () => '0123456789abcdef'[Math.floor(Math.random() * 16)]).join('');
    shareCodes.set(id, code);
    const p = pairs.find((x) => x.pair.id === id);
    if (p) p.pair.route = 'drive_and_devices';
    setTimeout(pushStatus, 50);
    return code;
  },
  share_code: (a) => shareCodes.get(String(a.id)) ?? null,
  sharing: (a) => {
    const id = String(a.id);
    const code = shareCodes.get(id);
    if (!code) return null;
    return sharings.get(id) ?? { share: code, all_devices: true, devices: [] };
  },
  share_with: (a) => {
    const id = String(a.id);
    const code = shareCodes.get(id);
    if (!code) throw { kind: 'invalid', message: 'This pair does not sync with devices' };
    sharings.set(id, { share: code, all_devices: !!a.all, devices: (a.devices as string[]) ?? [] });
    changed();
    return null;
  },
  offers: () => offers,
  decline_offer: (a) => {
    const o = offers.find((x) => x.share === a.share && x.from_device === a.from);
    if (o) o.status = 'declined';
    changed();
    return null;
  },
  accept_offer: (a) => {
    const o = offers.find((x) => x.share === a.share && x.from_device === a.from);
    if (!o) throw { kind: 'not_found', message: 'That offer is gone' };
    o.status = 'accepted';
    const p = variant(`offer-${o.share.slice(0, 6)}`, String(a.name ?? o.name), (x) => {
      x.pair.local_root = String(a.localRoot);
      x.pair.remote_path = '';
      x.pair.route = 'devices';
    });
    pairs.push(p);
    shareCodes.set(p.pair.id, o.share);
    setTimeout(pushStatus, 50);
    return p.pair;
  },
  keep_device: (a) => {
    const d = devices.find((x) => x.id === a.id);
    if (d) d.known = true;
    changed();
    return null;
  },
  unshare_pair: (a) => {
    shareCodes.delete(String(a.id));
    const p = pairs.find((x) => x.pair.id === String(a.id));
    if (p) p.pair.route = 'drive';
    setTimeout(pushStatus, 50);
    return null;
  },
  invite_device: () => {
    startPairing(true);
    return pairing;
  },
  join_device: (a) => {
    if (!String(a.code).startsWith('WASL:')) throw { kind: 'invalid', message: 'That is not a Wasl pairing code.' };
    startPairing(false);
    changed();
    return null;
  },
  pairing_status: () => pairing,
  confirm_pairing: (a) => {
    if (a.accept) {
      pairing = { ...pairing, confirmed_here: true };
      setPairing({ stage: 'paired' }, 2500);
    } else {
      window.clearTimeout(pairingTimer);
      pairing = { ...pairing, stage: 'failed', message: "You said the words don't match. Nothing was added." };
    }
    changed();
    return pairing;
  },
  cancel_pairing: () => {
    window.clearTimeout(pairingTimer);
    pairing = { ...IDLE };
    changed();
    return null;
  },
  ...viewAnswers,
  ...vaultAnswers,
  ...reviewAnswers,
};

/** Whether the mock answers a command (the test keeps this in step with api.ts). */
export const isAnswered = (cmd: string): boolean => cmd in answers;

/** `?platform=mac|windows` and `?theme=light|dark` from the page's query: the platform and theme to show instead
 * of the browser's own, so every combination can be looked at and shot from one machine. */
export function devOverrides(search: string): { platform?: Platform; theme?: 'light' | 'dark'; state?: MockState } {
  const q = new URLSearchParams(search);
  const platform = q.get('platform');
  const theme = q.get('theme');
  const state = q.get('state') as MockState | null;
  return {
    platform: platform === 'mac' || platform === 'windows' ? platform : undefined,
    theme: theme === 'light' || theme === 'dark' ? theme : undefined,
    state: state && MOCK_STATES.includes(state) ? state : undefined,
  };
}

// ---------------------------------------------------------------- app states (#70)

/**
 * `?state=` puts the mock in one of the app's states, so every banner and the first run can be seen:
 * stopped (the service isn't running; Restart service starts it), notfound (no wasld to start), connecting,
 * offline (the system has no network), auth (a Google account needs signing in again), quota (Google Drive is
 * full for two pairs), missing (a pair's Drive folder is gone), first (a new person: no account, no pair) and
 * empty (signed in, nothing set up: no pair, view, other device or agent change). Kinds are spelled as wasld
 * sends them (`QUOTA`, `NOT_FOUND`).
 */
export const MOCK_STATES = ['stopped', 'notfound', 'connecting', 'offline', 'auth', 'quota', 'missing', 'first', 'empty'] as const;
export type MockState = (typeof MOCK_STATES)[number];

const VERSION = (status as { version: string }).version;
let mockState: MockState | null = null;
let conn: Conn = { state: 'connected', version: VERSION };

function fail(p: PairState | undefined, kind: string, message: string): void {
  if (!p) return;
  Object.assign(p.pair.status, { last_status: 'FAILED', last_error_kind: kind, last_error: message, consecutive_failures: 3 });
  p.running = false;
  p.progress = null;
}

export function setMockState(state: MockState): void {
  mockState = state;
  const pair = (id: string) => pairs.find((p) => p.pair.id === id);
  switch (state) {
    case 'stopped':
      conn = { state: 'not_running', message: null };
      break;
    case 'notfound':
      conn = { state: 'not_running', message: 'wasld_not_found' };
      break;
    case 'connecting':
      conn = { state: 'connecting' };
      break;
    case 'offline':
      Object.defineProperty(navigator, 'onLine', { configurable: true, get: () => false });
      break;
    case 'auth':
      accountList[0]!.auth_error = 'invalid_grant';
      for (const p of pairs.filter((x) => x.pair.route !== 'devices')) {
        p.pair.account_id = accountList[0]!.id;
        fail(p, 'AUTH', 'Google needs you to sign in again.');
      }
      break;
    case 'quota':
      fail(pair('site'), 'QUOTA', "The user's Drive storage quota has been exceeded.");
      fail(pair('life'), 'QUOTA', "The user's Drive storage quota has been exceeded.");
      break;
    case 'missing':
      fail(pair(real.pair.id), 'NOT_FOUND', 'The Drive folder for this pair no longer exists (My Drive/Documents). Edit the pair and choose a folder.');
      break;
    case 'first':
      accountList.splice(0);
      pairs.splice(0);
      // Shown again on every load here, though the app remembers a finished or skipped first run.
      try {
        localStorage.removeItem('wasl.firstRun');
      } catch {
        // No storage (tests): nothing remembered anyway.
      }
      break;
    case 'empty':
      pairs.splice(0);
      viewStates.splice(0);
      devices.splice(1);
      offers.splice(0);
      changesets.splice(0);
      break;
  }
}

/** macOS's traffic lights at `trafficLightPosition` (20, 18): three 14pt buttons, 20pt apart, 16pt frames. */
const MOCK_LIGHTS = { x: 20, y: 18, width: 54, height: 16 };

/**
 * Stands in for what macOS draws around the page: the traffic lights, where the overlay title bar leaves them, and
 * the design's window colour where the window effect would show through.
 */
export function fakeMacWindow(): void {
  document.documentElement.style.background = 'light-dark(#ecebe6, #1a1917)';
  const box = document.createElement('div');
  box.setAttribute('aria-hidden', 'true');
  box.style.cssText = `position:fixed;z-index:100;left:${MOCK_LIGHTS.x}px;top:${MOCK_LIGHTS.y}px;display:flex;gap:8px;padding:2px 1px;pointer-events:none`;
  for (const color of ['#ff5f57', '#febc2e', '#28c840']) {
    const dot = document.createElement('i');
    dot.style.cssText = `inline-size:12px;block-size:12px;border-radius:50%;background:${color};box-shadow:inset 0 0 0 0.5px rgba(0,0,0,.2)`;
    box.append(dot);
  }
  document.body.append(box);
}

export function installMock(platform: Platform = 'windows', label = 'main'): void {
  placePairs(platform);
  let next = 1;
  const callbacks = new Map<number, (payload: unknown) => void>();
  const listeners: { event: string; handler: number }[] = [];
  emit = (event: string, payload: unknown) => {
    for (const l of listeners.filter((x) => x.event === event)) {
      callbacks.get(l.handler)?.({ event, id: l.handler, payload: structuredClone(payload) });
    }
  };
  // Tells the shell's context menus (src/shell/contextMenu.ts) and the Windows title bar (src/chrome/windows.ts)
  // that this is the mock, not the app: no native menus, no window to drive.
  (window as unknown as Record<string, unknown>).__WASL_MOCK__ = true;
  // Lets a preview send the app's events by hand, e.g. a folder drop: tauri://drag-enter, then tauri://drag-drop.
  (window as unknown as Record<string, unknown>).__WASL_MOCK_EMIT__ = (event: string, payload: unknown) => emit(event, payload);
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
    transformCallback(cb: (payload: unknown) => void) {
      const id = next++;
      callbacks.set(id, cb);
      return id;
    },
    unregisterCallback(id: number) {
      callbacks.delete(id);
    },
    async invoke(cmd: string, args: Record<string, unknown> = {}) {
      if (cmd === 'plugin:event|listen') {
        const id = next++;
        listeners.push({ event: String(args.event), handler: Number(args.handler) });
        return id;
      }
      // The page's own announcements (src/prefs.ts) reach this window's listeners; other windows hear localStorage.
      if (cmd === 'plugin:event|emit') emit(String(args.event), args.payload);
      if (cmd.startsWith('plugin:event|')) return null;
      if (cmd === 'plugin:dialog|open') return pick(args.options as Parameters<typeof pick>[0]);
      if (cmd === 'daemon_state' && conn.state === 'connected') {
        // The Rust side pushes the status after connecting; do the same.
        setTimeout(() => emit('daemon://status', answers.status({})), 50);
      }
      const answer = answers[cmd];
      if (!answer) {
        console.info(`[mock] ${cmd} (no data; returns nothing)`, args);
        return null;
      }
      await new Promise((r) => setTimeout(r, 150)); // a little latency, like the real IPC
      return structuredClone(await answer(args));
    },
    metadata: { currentWindow: { label }, currentWebview: { windowLabel: label, label } },
  };
}
