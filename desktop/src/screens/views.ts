// Views: Drive folders shown read-only on this computer (docs/ON_DEMAND.md §2.3). Each view's folders, state,
// space used, local changes Wasl left alone, and Keep a file here / Keep all / Free up space, Refresh,
// Turn off / on and Remove. Re-reads itself on `changed` events.

import { open } from '@tauri-apps/plugin-dialog';
import { api, isUiError } from '../api';
import type { App } from '../app';
import { lang, t } from '../i18n';
import { VIEW_LIMITS_GB, formatSpace, isolatePath, viewState, viewTone } from '../format';
import { button, chip, deskButton, empty, errorText, fill, h, line, toast } from '../ui';
import type { LocalChange, VaultInfo, ViewState, ViewsInfo } from '../types';
import { unlockDialog, vaultsById } from './vault';

export async function viewsScreen(app: App): Promise<HTMLElement> {
  const head = h(
    'header',
    { class: 'screen-head' },
    h('h1', { class: 'sr-title', tabindex: '-1' }, t('views.title')),
    h('div', { class: 'head-actions' }, button(t('views.add'), () => app.go({ name: 'addView' }), { kind: 'filled', icon: 'plus', key: 'add-view' })),
  );
  let info: ViewsInfo;
  try {
    info = await api.views();
  } catch (e) {
    return h('div', { class: 'screen' }, head, h('p', { class: 'field-error', role: 'alert' }, errorText(e)));
  }
  if (!info.supported) {
    return h(
      'div',
      { class: 'screen' },
      h('header', { class: 'screen-head' }, h('h1', { class: 'sr-title', tabindex: '-1' }, t('views.title'))),
      empty('cloud', t('views.unsupported_title'), t('views.unsupported_body')),
    );
  }
  const list = h('div', null);
  const screen = h('div', { class: 'screen' }, head, h('p', { class: 'kf-muted views-intro' }, t('views.intro')), list);
  const vaultsOf = (i: ViewsInfo) =>
    i.views.some((s) => s.view.vault_id) ? vaultsById() : Promise.resolve(new Map<string, VaultInfo>());
  render(app, list, info, await vaultsOf(info));

  let busy = false;
  app.onEvent((e) => {
    if (e.event !== 'changed' || busy || !screen.isConnected) return;
    busy = true;
    void api
      .views()
      .then(async (next) => {
        const vaults = await vaultsOf(next);
        if (screen.isConnected) render(app, list, next, vaults);
      })
      .catch(() => undefined) // the connection panel takes over when the daemon goes away
      .finally(() => (busy = false));
  });
  return screen;
}

function render(app: App, list: HTMLElement, info: ViewsInfo, vaults: Map<string, VaultInfo>): void {
  const focusKey = (document.activeElement as HTMLElement | null)?.closest<HTMLElement>('[data-key]')?.dataset.key;
  const hadFocus = list.contains(document.activeElement);
  if (info.views.length === 0) {
    fill(
      list,
      empty('eye', t('views.empty_title'), t('views.empty_body'),
        deskButton(t('views.add'), () => app.go({ name: 'addView' }), { primary: true, icon: 'plus', key: 'add-view-empty' })),
    );
  } else {
    fill(list, h('ul', { class: 'pair-list', role: 'list' }, info.views.map((s) => viewCard(app, s, vaults))));
  }
  if (hadFocus) {
    const again = focusKey ? list.querySelector<HTMLElement>(`[data-key="${CSS.escape(focusKey)}"]`) : null;
    (again ?? document.querySelector<HTMLElement>('#main h1'))?.focus();
  }
}

function viewCard(app: App, s: ViewState, vaults: Map<string, VaultInfo>): HTMLElement {
  const v = s.view;
  const vault = v.vault_id ? vaults.get(v.vault_id) : undefined;
  const locked = !!v.vault_id && !vault?.unlocked;
  const state = viewState(s);
  const titleId = `view-${v.id}`;
  const l = lang();
  const limitId = `view-limit-${v.id}`;
  const limits = VIEW_LIMITS_GB.map((g) => g * 2 ** 30);
  const limit = h(
    'select',
    {
      id: limitId,
      class: 'select',
      'data-key': `limit-${v.id}`,
      onchange: (e: Event) => {
        const bytes = Number((e.target as HTMLSelectElement).value);
        void run(async () => {
          await api.setViewCacheLimit(v.id, bytes);
          toast(t('views.limit_set', { name: v.name, size: formatSpace(bytes, lang()) }));
        });
      },
    },
    (limits.includes(v.cache_limit) ? limits : [...limits, v.cache_limit].sort((a, b) => a - b)).map((b) =>
      h('option', { value: String(b), selected: b === v.cache_limit }, formatSpace(b, l)),
    ),
  );
  const actions = h(
    'div',
    { class: 'card-actions', role: 'group', 'aria-label': t('views.actions_for', { name: v.name }) },
    locked && vault && v.enabled
      ? button(t('vault.unlock'), () => void unlockDialog(vault), { kind: 'filled', icon: 'key', key: `unlock-${v.id}` })
      : null,
    s.mounted
      ? button(t('views.keep_file'), () => void keepFile(s), { kind: 'tinted', icon: 'check_circle', key: `keep-file-${v.id}` })
      : null,
    s.mounted
      ? button(t('views.keep_all'), () => void pinAll(s, true), { icon: 'synced', key: `keep-all-${v.id}` })
      : null,
    s.mounted && s.cache_used > 0
      ? button(t('views.free_space'), () => void pinAll(s, false), { icon: 'cloud_off', key: `free-${v.id}` })
      : null,
    v.enabled
      ? button(t('views.refresh'), () => void run(async () => {
          await api.refreshView(v.id);
          toast(t('views.refreshed_toast', { name: v.name }));
        }), { icon: 'retry', key: `refresh-${v.id}` })
      : null,
    v.enabled
      ? button(t('views.turn_off'), () => void run(() => api.setViewEnabled(v.id, false)), { icon: 'pause', key: `onoff-${v.id}` })
      : button(t('views.turn_on'), () => void run(() => api.setViewEnabled(v.id, true)), { icon: 'play', key: `onoff-${v.id}` }),
    button(t('views.remove'), () => void removeView(s), { kind: 'plain', icon: 'trash', key: `remove-view-${v.id}` }),
  );
  return h(
    'li',
    { class: 'kf-card pair-card', 'aria-labelledby': titleId },
    h(
      'div',
      { class: 'pair-top' },
      h('h2', { id: titleId, class: 'pair-name' }, v.name),
      chip(t(`views.state_${state}`), viewTone(state)),
      chip(t('views.read_only'), 'kf-tone-neutral'),
      v.vault_id ? chip(t(locked ? 'vault.chip_locked' : 'vault.chip'), locked ? 'kf-tone-attention' : 'kf-tone-neutral') : null,
    ),
    h(
      'dl',
      { class: 'pair-folders' },
      h('div', null, h('dt', null, line('laptop'), t('pairs.local_label')), h('dd', { class: 'path', dir: 'ltr' }, isolatePath(v.local_path))),
      h('div', null, h('dt', null, line('synced'), t('pairs.drive_label')), h('dd', { class: 'path', dir: 'ltr' }, isolatePath(v.remote_path))),
    ),
    s.mounted
      ? h('p', { class: 'kf-muted' }, t('views.contents', { files: t('views.files', { count: s.files }), size: formatSpace(s.bytes, l) }))
      : null,
    h('p', { class: 'pair-last kf-muted' }, v.last_refresh_at ? t('views.refreshed', { when: app.ago(v.last_refresh_at) }) : t('views.never_refreshed')),
    v.last_error && v.enabled
      ? h('p', { class: 'pair-error', role: 'note' }, line('failed'), h('span', { dir: 'auto' }, viewErrorText(s.last_error_kind ?? null, v.last_error)))
      : null,
    h(
      'div',
      { class: 'view-space' },
      h('p', null, t('views.space', { used: formatSpace(s.cache_used, l), pinned: formatSpace(s.pinned_bytes, l) })),
      h('div', { class: 'kf-field' }, h('label', { class: 'kf-field-label', for: limitId }, t('views.limit_label')), h('div', { class: 'kf-field-box' }, limit)),
    ),
    s.local_changes.length > 0 ? localChanges(s) : null,
    actions,
  );
}

function localChanges(s: ViewState): HTMLElement {
  return h(
    'details',
    { class: 'view-changes' },
    h('summary', null, line('info'), t('views.local_changes', { count: s.local_changes.length })),
    h(
      'ul',
      { class: 'plain-list' },
      s.local_changes.slice(0, 50).map((c) =>
        h('li', null, h('span', { class: 'path', dir: 'ltr' }, isolatePath(c.path)), ' ', h('span', { class: 'kf-muted', dir: 'auto' }, changeText(c))),
      ),
    ),
  );
}

/** Why something in a view was left alone, in the app's words when the service says its kind. */
export function changeText(c: LocalChange): string {
  return c.kind ? t(`views.change_${c.kind}`) : c.reason;
}

/** Errors from view commands: the daemon's own words, which say what to do. */
function viewError(e: unknown): string {
  if (isUiError(e) && !['not_running', 'disconnected', 'untrusted', 'version', 'auth', 'network'].includes(e.kind)) {
    return t('common.error', { message: e.message });
  }
  return errorText(e);
}

async function run(f: () => Promise<unknown>): Promise<void> {
  try {
    await f();
  } catch (e) {
    toast(viewError(e));
  }
}

const VIEW_ERRORS: Record<string, string> = {
  network: 'errors.kind.Network',
  auth: 'errors.auth',
  not_found: 'errors.kind.NotFound',
  permission: 'errors.kind.Permission',
  locked: 'views.error_vault',
};

export function viewErrorText(kind: string | null, message: string): string {
  const key = kind ? VIEW_ERRORS[kind] : undefined;
  return key ? t(key) : message;
}

async function keepFile(s: ViewState): Promise<void> {
  const picked = await open({ defaultPath: s.view.local_path, multiple: false, directory: false }).catch(() => null);
  if (typeof picked !== 'string' || !picked) return;
  await run(async () => {
    await api.pinPath(picked);
    toast(t('views.kept', { name: picked }));
  });
}

async function pinAll(s: ViewState, keep: boolean): Promise<void> {
  await run(async () => {
    if (keep) {
      await api.pinPath(s.view.local_path);
      toast(t('views.kept', { name: s.view.name }));
    } else {
      await api.unpinPath(s.view.local_path);
      toast(t('views.freed', { name: s.view.name }));
    }
  });
}

/** Asks how to remove a view: its placeholders go (pinned files stay), or every file is kept. */
function removeDialog(name: string): Promise<{ keepFiles: boolean } | null> {
  const before = document.activeElement as HTMLElement | null;
  return new Promise((resolve) => {
    const id = `dlg-${Math.random().toString(36).slice(2)}`;
    const dlg = h('dialog', { class: 'dialog', 'aria-labelledby': `${id}-title`, 'aria-describedby': `${id}-body` });
    const keep = h('input', { id: `${id}-keep`, type: 'checkbox' });
    const done = (value: { keepFiles: boolean } | null) => {
      dlg.close();
      dlg.remove();
      before?.focus();
      resolve(value);
    };
    dlg.addEventListener('cancel', (e) => {
      e.preventDefault();
      done(null);
    });
    const cancel = button(t('common.cancel'), () => done(null), { kind: 'gray' });
    fill(
      dlg,
      h('h2', { id: `${id}-title`, class: 'dialog-title' }, t('views.remove_title', { name })),
      h('p', { id: `${id}-body`, class: 'dialog-body' }, t('views.remove_body')),
      h('label', { class: 'check-row' }, keep, t('views.remove_keep_all')),
      h('div', { class: 'dialog-actions' }, cancel, button(t('views.remove'), () => done({ keepFiles: keep.checked }), { kind: 'danger' })),
    );
    document.body.appendChild(dlg);
    dlg.showModal();
    cancel.focus();
  });
}

async function removeView(s: ViewState): Promise<void> {
  const choice = await removeDialog(s.view.name);
  if (!choice) return;
  if (choice.keepFiles) toast(t('views.removing_keep', { name: s.view.name }));
  await run(async () => {
    await api.removeView(s.view.id, choice.keepFiles);
    toast(t('views.removed', { name: s.view.name }));
  });
}
