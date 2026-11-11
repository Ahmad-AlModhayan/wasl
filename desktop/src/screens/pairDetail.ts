// Pair detail (#64, design D4): one pair in the detail pane, opened from the All pairs table or the pair's sidebar
// row. The toolbar carries its name, state, Pause, Sync now and Show in Finder; this pane shows what needs doing
// (a held preview, a held deletion, a conflict, a failure and its fix), where the pair syncs, its direction and
// vault, recent syncs linking to History, sharing with this person's devices, and Remove. Esc or Back returns to
// the table with the row still selected (app.ts).
//
// The folder's size and the last sync's skipped count show when the daemon reports them (#103); when it syncs isn't
// in the daemon's status yet, so this pane leaves it out rather than show a guess.

import { api } from '../api';
import type { App } from '../app';
import { lang, t } from '../i18n';
import { errorKind, failureText, formatDate, isolatePath, methodKey, pairFacts, pairStatus, progressFraction, progressText, runCounts } from '../format';
import { button, copyText, errorText, h, line, loading, promptDialog, statusGlyph, toast } from '../ui';
import type { DeviceInfo, PairState, RunRecord, VaultInfo } from '../types';
import type { LineName } from '../icons';
import { chooseDevices, sharedWithText } from './introductions';
import { unlockDialog, vaultsById } from './vault';
import { removePair } from './pairs';
import { syncsWithDevices } from './pairsModel';

/** Whether device sync runs (sharing shows only then). Read on the first render and again after a `changed`
 *  event, not on every live update. */
let devicesEnabled: boolean | null = null;

function readDevicesEnabled(): Promise<boolean> {
  return api
    .devices()
    .then((d) => (devicesEnabled = d.enabled))
    .catch(() => (devicesEnabled = false));
}

interface Extra {
  vault: VaultInfo | undefined;
  runs: RunRecord[] | null;
  share: { code: string | null; withText: string | null; devices: DeviceInfo[] } | null;
}

/** What the pane reads besides the status, kept while the pair's status doesn't move, so live updates stay cheap. */
let cache: { key: string; extra: Extra } | null = null;

function cacheKey(s: PairState): string {
  const p = s.pair;
  return JSON.stringify([p.id, p.status.last_run_at, p.status.last_status, s.running, p.route ?? null, p.vault_id ?? null, devicesEnabled]);
}

/** Forgets what was read, after an action here changed it. */
function bust(app: App): void {
  cache = null;
  app.rerender();
}

async function readExtra(s: PairState): Promise<Extra> {
  const key = cacheKey(s);
  if (cache?.key === key) return cache.extra;
  const p = s.pair;
  const shared = !!devicesEnabled && syncsWithDevices(s);
  const [vault, runs, share] = await Promise.all([
    p.vault_id ? vaultsById().then((m) => m.get(p.vault_id!)).catch(() => undefined) : Promise.resolve(undefined),
    api.runs(p.id, 5).catch(() => null),
    shared
      ? Promise.all([api.shareCode(p.id).catch(() => null), api.sharing(p.id).catch(() => null), api.devices().catch(() => null)]).then(([code, w, d]) => {
          const devices = d?.devices ?? [];
          return { code, withText: w ? sharedWithText(w.all_devices, w.devices, devices) : null, devices: sharedDevices(w, devices) };
        })
      : Promise.resolve(null),
  ]);
  const extra = { vault, runs, share };
  cache = { key, extra };
  return extra;
}

/** The devices a pair goes to: the ones chosen, or every other known device. */
function sharedDevices(w: { all_devices: boolean; devices: string[] } | null, devices: DeviceInfo[]): DeviceInfo[] {
  if (!w) return [];
  const others = devices.filter((d) => !d.this_device);
  return w.all_devices ? others.filter((d) => d.known !== false) : others.filter((d) => w.devices.includes(d.id));
}

function isMac(): boolean {
  return document.documentElement.dataset.kfPlatform === 'mac';
}

export async function pairDetailScreen(app: App, id: string): Promise<HTMLElement> {
  const found = app.status?.pairs.find((p) => p.pair.id === id);
  if (!found) {
    queueMicrotask(() => app.go({ name: 'pairs' })); // removed meanwhile
    return loading();
  }
  // Something changed (sharing, devices): read devices and the rest again. The shell runs this before it
  // re-renders the screen for the same event.
  app.onEvent((e) => {
    if (e.event === 'changed') {
      devicesEnabled = null;
      cache = null;
    }
  });
  if (devicesEnabled === null) await readDevicesEnabled();
  const extra = await readExtra(found);
  // The status may have moved while we read.
  const s = app.status?.pairs.find((p) => p.pair.id === id) ?? found;
  const p = s.pair;
  return h(
    'div',
    { class: 'pd' },
    // The name only: a live update focuses a new heading, and the state would be read again every second. State
    // changes reach a screen reader once, through the shell's live region.
    h('h1', { class: 'sr-title', tabindex: '-1' }, p.name),
    h(
      'button',
      { type: 'button', class: 'kf-desk-btn pd-back', 'data-key': 'pd-back', onclick: () => app.backToPairs() },
      line('back', { mirror: true }),
      t('nav.pairs'),
    ),
    callout(app, s, extra.vault),
    route(app, s, extra),
    s.running && s.progress ? progress(s) : null,
    facts(app, s, extra.vault),
    devicesEnabled ? sharing(app, s, extra.share) : null,
    recentRuns(app, s, extra.runs),
    h(
      'div',
      { class: 'pd-foot' },
      h('button', { type: 'button', class: 'kf-desk-btn', 'data-key': 'pd-edit', onclick: () => void app.editPair(p.id) }, line('gear'), t('pairsheet.edit')),
      p.status.hold_for_preview
        ? null
        : h('button', { type: 'button', class: 'kf-desk-btn', 'data-key': 'pd-preview', onclick: () => app.go({ name: 'preview', id: p.id }) }, line('eye'), t('detail.preview')),
      h(
        'button',
        { type: 'button', class: 'kf-desk-btn kf-desk-btn-danger', 'data-key': 'pd-remove', onclick: () => void removePair(p.id, p.name) },
        line('trash'),
        t('shell.remove_pair'),
      ),
    ),
  );
}

// ---------------------------------------------------------------- what needs doing

function deskButton(text: string, run: () => void, opts: { key: string; icon?: LineName; primary?: boolean }): HTMLButtonElement {
  return h(
    'button',
    { type: 'button', class: `kf-desk-btn${opts.primary ? ' kf-desk-btn-default' : ''}`, 'data-key': opts.key, onclick: run },
    opts.icon ? line(opts.icon) : null,
    text,
  );
}

/** The one thing that needs the person, most urgent first, with its fix (the Errors tab's message for a failure). */
function callout(app: App, s: PairState, vault: VaultInfo | undefined): HTMLElement | null {
  const p = s.pair;
  const st = p.status;
  const v = pairStatus(s);
  const box = (state: string, title: string, body: string | null, action: HTMLElement | null) =>
    h(
      'section',
      { class: `pd-callout status-${state}`, 'aria-label': title },
      line(state as LineName, { size: 20 }),
      h('div', { class: 'pd-callout-text' }, h('p', { class: 'pd-callout-title' }, title), body ? h('p', { dir: 'auto' }, body) : null),
      action,
    );
  if (v.state === 'approval' && !v.preview) {
    return box('approval', t('detail.approval_title'), t('banner.approval_body'), deskButton(t('banner.review'), () => app.go({ name: 'problems' }), { key: 'pd-review', primary: true }));
  }
  if (v.state === 'failed') {
    const why = failureText(st.last_error_kind, st.last_error, lang()) ?? t('banner.failed_body');
    let fix: HTMLElement;
    if (errorKind(st.last_error_kind) === 'Locked' && vault && !vault.unlocked) {
      fix = deskButton(t('vault.unlock'), () => void unlockDialog(vault).then((ok) => ok && bust(app)), { key: 'pd-unlock', icon: 'lock', primary: true });
    } else if (errorKind(st.last_error_kind) === 'Auth') {
      fix = deskButton(t('banner.sign_in'), () => app.openSettings('accounts'), { key: 'pd-sign-in', primary: true });
    } else {
      fix = deskButton(t('common.retry'), () => app.syncNow([p.id]), { key: 'pd-retry', icon: 'sync', primary: true });
    }
    return box('failed', t('detail.failed_title'), why, fix);
  }
  if (v.state === 'conflict') {
    return box(
      'conflict',
      t('banner.conflict', { count: v.count }),
      t('banner.conflict_body'),
      deskButton(t('banner.review'), () => app.go({ name: 'problems' }), { key: 'pd-review', primary: true }),
    );
  }
  if (v.preview) {
    // Needs approval, as the table says: a new pair waits for its first preview (#64).
    return box(
      'approval',
      t('detail.hold_title'),
      t('detail.hold_body'),
      deskButton(t('pairs.preview_and_start'), () => app.go({ name: 'preview', id: p.id }), { key: 'pd-start', icon: 'eye', primary: true }),
    );
  }
  return null;
}

// ---------------------------------------------------------------- where it syncs

function route(app: App, s: PairState, extra: Extra): HTMLElement {
  const p = s.pair;
  const where = p.route ?? 'drive';
  const node = (icon: LineName, title: string, caption: HTMLElement | null, ...more: Array<HTMLElement | null>) =>
    h('li', { class: 'pd-node' }, line(icon, { size: 20 }), h('p', { class: 'pd-node-title' }, title), caption, ...more);
  const link = (text: string, live = false) => h('li', { class: `pd-link${live ? ' pd-link-live' : ''}`, 'aria-hidden': 'true' }, h('span', null, text), h('hr'));
  const path = (text: string) => h('p', { class: 'path pd-node-caption' }, isolatePath(text));
  const here = node(
    'laptop',
    t(isMac() ? 'detail.this_mac' : 'detail.this_pc'),
    path(p.local_root),
    h(
      'button',
      { type: 'button', class: 'kf-desk-btn pd-reveal', 'data-key': 'pd-reveal', onclick: () => app.showInFolder(p.id) },
      line('reveal'),
      app.revealLabel(),
    ),
  );
  const parts: HTMLElement[] = [here];
  if (where !== 'devices') {
    parts.push(link(t(methodKey(p.method)), s.running));
    parts.push(
      node(
        p.vault_id ? 'lock' : 'cloud',
        'Google Drive',
        p.remote_path ? path(p.remote_path) : null,
        p.vault_id ? h('p', { class: 'pd-node-caption' }, t('detail.vault_encrypted')) : null,
      ),
    );
  }
  if (where !== 'drive') {
    parts.push(link(t('detail.direct'), s.running && where === 'devices'));
    const devices = extra.share?.devices ?? [];
    parts.push(
      node(
        'bolt',
        t('inspector.devices'),
        extra.share?.withText ? h('p', { class: 'pd-node-caption', dir: 'auto' }, extra.share.withText) : null,
        devices.length
          ? h(
              'ul',
              { class: 'pd-devices', role: 'list' },
              devices.map((d) =>
                h(
                  'li',
                  null,
                  h('span', { dir: 'auto' }, d.name),
                  h('span', { class: `pd-online${d.connected ? ' is-on' : ''}` }, t(d.connected ? 'devices.connected' : 'devices.not_connected')),
                ),
              ),
            )
          : null,
      ),
    );
  }
  // The diagram's words, for a screen reader, in one sentence.
  const said = [t(isMac() ? 'detail.this_mac' : 'detail.this_pc'), where !== 'devices' ? 'Google Drive' : null, where !== 'drive' ? t('inspector.devices') : null]
    .filter(Boolean)
    .join(' · ');
  return h('section', { class: 'pd-route-box', 'aria-label': t('detail.route', { route: said }) }, h('ul', { class: 'pd-route', role: 'list' }, parts));
}

function progress(s: PairState): HTMLElement {
  const pr = s.progress!;
  const f = progressFraction(pr);
  const pct = f === null ? null : Math.round(f * 100);
  return h(
    'div',
    { class: 'progress-box pd-progress', 'data-progress-for': s.pair.id },
    h(
      'div',
      {
        class: `kf-progress${pct === null ? ' indeterminate' : ''}`,
        role: 'progressbar',
        'aria-label': t('progress.label', { name: s.pair.name }),
        'aria-valuemin': '0',
        'aria-valuemax': '100',
        'aria-valuenow': pct === null ? undefined : String(pct),
      },
      h('span', { style: `inline-size: ${pct ?? 30}%` }),
    ),
    h(
      'div',
      { class: 'pd-progress-row' },
      h('p', { class: 'progress-text kf-tnum' }, progressText(pr, lang())),
      h(
        'button',
        { type: 'button', class: 'kf-desk-btn', 'data-key': 'pd-stop', onclick: () => void api.cancel(s.pair.id).catch((e: unknown) => toast(errorText(e))) },
        t('pairs.cancel'),
      ),
    ),
  );
}

/** Numbers before prose: its state, direction, last sync, size, what it skipped and vault. */
function facts(app: App, s: PairState, vault: VaultInfo | undefined): HTMLElement {
  const p = s.pair;
  const last = p.status.last_success_at ?? p.status.last_run_at;
  const fact = (label: string, value: HTMLElement | string, caption?: string | null, action?: HTMLElement | null) =>
    h('div', { class: 'pd-fact' }, h('dt', null, label), h('dd', null, value), caption ? h('dd', { class: 'pd-fact-caption' }, caption) : null, action ?? null);
  const locked = !!p.vault_id && !vault?.unlocked;
  return h(
    'dl',
    { class: 'pd-facts' },
    fact(t('detail.status'), statusGlyph(pairStatus(s), { label: true, pairId: p.id })),
    fact(t('pairs.col_direction'), t(p.route === 'devices' ? 'pairs.devices_only' : methodKey(p.method)), p.route === 'devices' ? null : t(`${methodKey(p.method)}_desc`)),
    fact(t('pairs.col_last'), s.running ? t('pairs.now') : last ? app.ago(last) : t('pairs.never_synced')),
    ...pairFacts(s, lang()).map((f) => fact(f.label, h('span', { class: 'pd-num' }, f.value), f.hint)),
    p.vault_id
      ? fact(
          t('vault.chip'),
          t(locked ? 'detail.vault_locked' : 'detail.vault_unlocked'),
          null,
          locked && vault && pairStatus(s).state !== 'failed'
            ? deskButton(t('vault.unlock'), () => void unlockDialog(vault).then((ok) => ok && bust(app)), { key: 'pd-vault-unlock', icon: 'lock' })
            : null,
        )
      : null,
  );
}

// ---------------------------------------------------------------- recent syncs

function recentRuns(app: App, s: PairState, runs: RunRecord[] | null): HTMLElement {
  const head = h(
    'div',
    { class: 'pd-section-head' },
    h('h2', { id: 'pd-runs-title' }, t('detail.recent')),
    h('button', { type: 'button', class: 'kf-desk-btn pd-plain', 'data-key': 'pd-history', onclick: () => app.go({ name: 'history' }) }, t('detail.all_history')),
  );
  let body: HTMLElement;
  if (runs === null) body = h('p', { class: 'pd-muted' }, t('detail.runs_unavailable'));
  else if (runs.length === 0) body = h('p', { class: 'pd-muted' }, t('detail.no_runs'));
  else {
    body = h(
      'table',
      { class: 'kf-desk-table pd-runs', 'aria-labelledby': 'pd-runs-title' },
      h('thead', null, h('tr', null, h('th', { scope: 'col' }, t('detail.when')), h('th', { scope: 'col' }, t('detail.what')), h('th', { scope: 'col' }, t('pairs.col_status')))),
      h(
        'tbody',
        null,
        runs.map((r) => {
          const when = formatDate(r.started_at, lang());
          const counts = runCounts(r.stats);
          return h(
            'tr',
            null,
            h(
              'td',
              null,
              h(
                'button',
                {
                  type: 'button',
                  class: 'pd-run-link',
                  'data-key': `pd-run-${r.id}`,
                  'aria-label': t('history.open', { name: s.pair.name, when }),
                  onclick: () => app.go({ name: 'run', id: r.id, title: `${s.pair.name} · ${when}` }),
                },
                when,
              ),
            ),
            h('td', { class: 'kf-tnum' }, counts.length ? counts.map(([k, n]) => t(k, { count: n })).join(' · ') : t('history.no_changes')),
            h('td', null, t(`run_status.${r.status.toLowerCase()}`)),
          );
        }),
      ),
    );
  }
  return h('section', { class: 'pd-section', 'aria-labelledby': 'pd-runs-title' }, head, body);
}

// ---------------------------------------------------------------- sync with my devices

function sharing(app: App, s: PairState, share: Extra['share']): HTMLElement {
  const p = s.pair;
  const shared = syncsWithDevices(s);
  const sw = h('button', {
    type: 'button',
    role: 'switch',
    class: 'kf-switch',
    'aria-checked': String(shared),
    'aria-labelledby': 'pd-share-label',
    'data-key': 'pd-share',
    // Busy via aria-busy, not `disabled`: a disabled button drops focus, and the re-render puts it back by key.
    onclick: () =>
      void run(async () => {
        if (sw.getAttribute('aria-busy') === 'true') return;
        sw.setAttribute('aria-busy', 'true');
        try {
          if (shared) {
            await api.unsharePair(p.id);
            toast(t('pairs.unshared', { name: p.name }));
          } else {
            await api.sharePair(p.id, null);
            toast(t('pairs.shared', { name: p.name }));
          }
        } finally {
          sw.removeAttribute('aria-busy');
          bust(app);
        }
      }),
  });
  const code = shared ? share?.code ?? null : null;
  return h(
    'section',
    { class: 'pd-section', 'aria-labelledby': 'pd-share-title' },
    h('div', { class: 'pd-section-head' }, h('h2', { id: 'pd-share-title' }, t('detail.sharing'))),
    h(
      'div',
      { class: 'pd-share' },
      h('div', { class: 'share-switch' }, sw, h('span', { id: 'pd-share-label' }, t('pairs.share'))),
      shared && share?.withText
        ? h(
            'div',
            { class: 'share-with' },
            h('span', { class: 'pd-muted', dir: 'auto' }, share.withText),
            deskButton(t('intro.choose'), () => void chooseDevices(p.id, p.name).then(() => bust(app)), { key: 'pd-share-with', icon: 'laptop' }),
          )
        : null,
      code
        ? h(
            'div',
            { class: 'share-code' },
            h('span', { class: 'pd-muted', id: 'pd-share-code' }, t('pairs.share_code')),
            h('code', { dir: 'ltr', 'aria-labelledby': 'pd-share-code' }, code),
            button(t('common.copy'), () => void copyText(code), { kind: 'plain', icon: 'copy', key: 'pd-share-copy', label: t('pairs.share_copy_for', { name: p.name }) }),
          )
        : shared
          ? h('span', { class: 'pd-muted share-code' }, t('pairs.share_code_elsewhere'))
          : null,
      h('div', null, deskButton(t('pairs.share_use_code'), () => void useCode(app, p.id, p.name), { key: 'pd-share-use' })),
    ),
  );
}

/** A share code as the service writes it: 16 bytes in hex (wasl-net `ShareId`). */
export function isShareCode(v: string): boolean {
  return /^[0-9a-f]{32}$/i.test(v.trim());
}

async function useCode(app: App, id: string, name: string): Promise<void> {
  const code = await promptDialog({
    title: t('pairs.share_use_title', { name }),
    body: t('pairs.share_use_body'),
    label: t('pairs.share_use_label'),
    confirm: t('pairs.share_use_confirm'),
    ltr: true,
    check: (v) => (isShareCode(v) ? null : t('pairs.share_code_invalid')),
  });
  if (!code) return;
  await run(async () => {
    await api.sharePair(id, code);
    toast(t('pairs.share_joined', { name }));
    bust(app);
  });
}

async function run(f: () => Promise<unknown>): Promise<void> {
  try {
    await f();
  } catch (e) {
    toast(errorText(e));
  }
}
