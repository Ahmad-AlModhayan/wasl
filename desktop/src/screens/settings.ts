// Settings (#68, design D5): General, Accounts, Sync, Notifications and Advanced. macOS shows one pane at a time in
// its own window (src/settingsWindow.ts); Windows shows them all on one page inside the main window. The rows are
// the same grouped form on both (#124). Every change applies at once; there is no Save button.

import { api, type UpdateCheck } from '../api';
import type { App } from '../app';
import { lang, t } from '../i18n';
import { signIn, signInButton, signInWaiting } from '../signIn';
import type { LineName } from '../icons';
import type { Accent, SettingsPane } from '../prefs';
import { PANES } from '../prefs';
import type { AppInfo, Conn, DaemonEvent, DaemonStatus } from '../types';
import { button, confirmDialog, errorText, h, line, toast } from '../ui';

/** What a pane needs from the window it is in: the main window's App, or the macOS Settings window. */
export interface SettingsHost {
  conn: Conn;
  status: DaemonStatus | null;
  info: AppInfo;
  langPref: string;
  accent: Accent;
  /** The system accent was chosen but couldn't be read (Windows), so Wasl navy shows. */
  accentMissing?: boolean;
  setLanguage(pref: string): Promise<void>;
  setAccent(accent: Accent): void;
  onEvent(fn: (e: DaemonEvent) => void): void;
}

/** Where the tray remembers the start-at-login choice (the CLI does the installing). */
const STARTUP_KEY = 'start_at_login';
/** "Show Wasl in the menu bar / notification area" (#71); the app reads it back when it connects (daemon.rs). */
export const TRAY_KEY = 'tray_visible';
/** Notifications (#71): all of them, then each category; on unless set to "false" (src-tauri/src/notify.rs). */
export const NOTIFY_KEY = 'notify';
export const NOTIFY_CATEGORIES = ['conflict', 'failed', 'deletion', 'pairing'] as const;

/** A stored on/off setting: on unless it says "false". */
export const settingOn = (value: string | null | undefined) => value !== 'false';

/** Each pane's toolbar icon (macOS). */
export const PANE_ICONS: Record<SettingsPane, LineName> = {
  general: 'gear',
  accounts: 'user',
  sync: 'sync',
  notifications: 'bell',
  advanced: 'pulse',
};

const mac = () => document.documentElement.dataset.kfPlatform === 'mac';

/** One setting: a form row (title, description, control at the end), the same on both platforms. */
function row(opts: { id: string; icon?: LineName; title: string; desc?: string; control?: Node | null }): HTMLElement {
  return h(
    'div',
    { class: 'settings-row' },
    opts.icon ? h('span', { class: 'settings-row-icon', 'aria-hidden': 'true' }, line(opts.icon, { size: 20 })) : null,
    h(
      'div',
      { class: 'settings-row-text' },
      h('span', { id: opts.id, class: 'settings-row-title' }, opts.title),
      opts.desc ? h('span', { id: `${opts.id}-desc`, class: 'settings-row-desc' }, opts.desc) : null,
    ),
    opts.control ?? null,
  );
}

function group(id: string, label: string | null, ...rows: Array<HTMLElement | null>): HTMLElement {
  return h(
    'section',
    { class: 'settings-group', 'aria-labelledby': label ? id : undefined },
    label ? h('h3', { id, class: 'settings-group-label' }, label) : null,
    h('div', { class: 'settings-box' }, rows),
  );
}

/** An on/off setting as a switch (the platform draws it: kahf-design's `kf-switch`). */
function toggle(opts: { key: string; labelledby: string; on: boolean; disabled?: boolean; change: (on: boolean) => Promise<void> | void }): HTMLButtonElement {
  const sw: HTMLButtonElement = h('button', {
    type: 'button',
    role: 'switch',
    class: 'kf-switch',
    'aria-checked': String(opts.on),
    'aria-labelledby': opts.labelledby,
    'aria-describedby': `${opts.labelledby}-desc`,
    'data-key': opts.key,
    disabled: opts.disabled,
    onclick: async () => {
      const next = sw.getAttribute('aria-checked') !== 'true';
      sw.disabled = true;
      try {
        await opts.change(next);
        sw.setAttribute('aria-checked', String(next));
      } catch (e) {
        toast(t('settings.change_failed', { message: errorText(e) }));
      }
      sw.disabled = false;
      sw.focus();
    },
  });
  return sw;
}

/** "Show Wasl in the menu bar / notification area" as a settings row: General and the first run's step use it.
 *  macOS keeps the Dock icon while it is hidden, so Wasl stays reachable. Applies at once and is remembered. */
export function trayRow(opts: { on: boolean; disabled?: boolean; change?: (on: boolean) => void }): HTMLElement {
  return row({
    id: 'set-tray',
    icon: 'inspector',
    title: t(mac() ? 'settings.tray_mac' : 'settings.tray_win'),
    desc: t(mac() ? 'settings.tray_body_mac' : 'settings.tray_body_win'),
    control: toggle({
      key: 'tray',
      labelledby: 'set-tray',
      on: opts.on,
      disabled: opts.disabled,
      change: async (next) => {
        await api.setSetting(TRAY_KEY, String(next));
        await api.setTrayVisible(next);
        opts.change?.(next);
      },
    }),
  });
}

async function generalPane(host: SettingsHost): Promise<HTMLElement[]> {
  const connected = host.conn.state === 'connected';
  // Start at login: only when the CLI that installs the service ships next to the app.
  let startup: HTMLElement | null = null;
  if (host.info.cli) {
    const on = connected ? (await api.setting(STARTUP_KEY).catch(() => null)) === 'true' : false;
    startup = row({
      id: 'set-startup',
      icon: 'bolt',
      title: t('settings.startup'),
      desc: t('settings.startup_body'),
      control: toggle({
        key: 'startup',
        labelledby: 'set-startup',
        on,
        disabled: !connected,
        change: async (next) => {
          await api.setStartAtLogin(next);
          await api.setSetting(STARTUP_KEY, String(next));
        },
      }),
    });
  }
  const trayShown = connected ? settingOn(await api.setting(TRAY_KEY).catch(() => null)) : true;
  const tray = trayRow({ on: trayShown, disabled: !connected });
  const langs: Array<[string, string]> = [
    ['system', t('settings.lang_system')],
    ['en', t('settings.lang_en')],
    ['ar', t('settings.lang_ar')],
  ];
  const select = h(
    'select',
    {
      class: 'kf-select settings-select',
      'aria-labelledby': 'set-lang',
      'data-key': 'language',
      onchange: (e: Event) => void host.setLanguage((e.target as HTMLSelectElement).value),
    },
    langs.map(([value, label]) =>
      h('option', { value, selected: host.langPref === value, lang: value === 'ar' ? 'ar' : value === 'en' ? 'en' : undefined }, label)),
  );
  const accent = row({
    id: 'set-accent',
    icon: 'eye',
    title: t(mac() ? 'settings.accent_mac' : 'settings.accent_win'),
    desc: host.accent === 'system' && host.accentMissing ? t('settings.accent_missing') : t('settings.accent_body'),
    control: toggle({
      key: 'accent',
      labelledby: 'set-accent',
      on: host.accent === 'system',
      change: (next) => host.setAccent(next ? 'system' : 'wasl'),
    }),
  });
  return [
    group('set-g-computer', t(mac() ? 'detail.this_mac' : 'detail.this_pc'), startup, tray),
    group('set-g-look', t('settings.appearance'), row({ id: 'set-lang', icon: 'spark', title: t('settings.language'), control: select }), accent),
  ].filter((g) => g.querySelector('.settings-row'));
}

function accountsPane(host: SettingsHost): HTMLElement[] {
  if (host.conn.state !== 'connected') return [group('set-g-accounts', null, row({ id: 'set-acc-off', title: t('settings.service_needed') }))];
  const accounts = host.status?.accounts ?? [];
  host.onEvent((e) => {
    if (e.event === 'account_added') toast(t('settings.account_added', { email: e.account.email }));
    if (e.event === 'account_failed') toast(t('add.account_failed', { message: e.message }));
  });
  const add = () => void signIn.start();
  const rows = accounts.length === 0
    ? [row({ id: 'set-acc-none', icon: 'user', title: t('settings.no_accounts') })]
    : accounts.map((a, i) =>
        row({
          id: `set-acc-${i}`,
          icon: 'user',
          title: a.email,
          desc: a.auth_error ? t('settings.reauth') : t('settings.account_ok'),
          control: h(
            'div',
            { class: 'settings-actions' },
            a.auth_error ? signInButton(button(t('banner.sign_in'), add, { kind: 'filled', key: `reauth-${a.id}` })) : null,
            button(t('settings.remove_account'), async () => {
              const ok = await confirmDialog({
                title: t('settings.remove_account_title', { email: a.email }),
                body: t('settings.remove_account_body'),
                confirm: t('settings.remove_account'),
                danger: true,
              });
              if (ok) await api.removeAccount(a.id).catch((e) => toast(errorText(e)));
            }, { kind: 'plain', key: `remove-acct-${a.id}`, label: t('settings.remove_account_for', { email: a.email }) }),
          ),
        }));
  // Emails read left to right inside Arabic.
  for (const title of rows.map((r) => r.querySelector('.settings-row-title'))) if (accounts.length) title?.setAttribute('dir', 'ltr');
  return [
    group('set-g-accounts', t('settings.accounts'), ...rows),
    h('div', { class: 'settings-foot' }, signInButton(button(t('settings.add_account'), add, { icon: 'plus', key: 'settings-add-account' })), signInWaiting()),
  ];
}

function syncPane(host: SettingsHost): HTMLElement[] {
  const pairs = host.status?.pairs ?? [];
  const paused = pairs.filter((p) => p.pair.paused).length;
  return [
    group('set-g-sync', t('settings.sync_defaults'),
      row({ id: 'set-sync-guard', icon: 'stop', title: t('settings.sync_guard'), desc: t('settings.sync_guard_body') }),
      row({ id: 'set-sync-bin', icon: 'trash', title: t('settings.sync_bin'), desc: t('settings.sync_bin_body') })),
    group('set-g-sync-now', null,
      row({ id: 'set-sync-pairs', icon: 'folder', title: t('settings.sync_pairs', { count: pairs.length }), desc: paused ? t('settings.sync_paused', { count: paused }) : t('settings.sync_pairs_body') })),
  ];
}

/** Notifications (#71): only for decisions and failures. One switch for all, then one per category. */
async function notificationsPane(host: SettingsHost): Promise<HTMLElement[]> {
  const connected = host.conn.state === 'connected';
  const read = async (key: string) => (connected ? settingOn(await api.setting(key).catch(() => null)) : true);
  const all = await read(NOTIFY_KEY);
  const categories: HTMLButtonElement[] = [];
  const master = toggle({
    key: 'notify',
    labelledby: 'set-notify',
    on: all,
    disabled: !connected,
    change: async (next) => {
      await api.setSetting(NOTIFY_KEY, String(next));
      for (const sw of categories) sw.disabled = !next;
    },
  });
  const rows: HTMLElement[] = [];
  for (const c of NOTIFY_CATEGORIES) {
    const sw = toggle({
      key: `notify-${c}`,
      labelledby: `set-notify-${c}`,
      on: await read(`notify_${c}`),
      disabled: !connected || !all,
      change: (next) => api.setSetting(`notify_${c}`, String(next)),
    });
    categories.push(sw);
    rows.push(row({ id: `set-notify-${c}`, title: t(`settings.notify_${c}`), desc: t(`settings.notify_${c}_body`), control: sw }));
  }
  return [
    group('set-g-notify', null, row({ id: 'set-notify', icon: 'bell', title: t('settings.notify_title'), desc: t('settings.notify_body'), control: master })),
    group('set-g-notify-kinds', t('settings.notify_group'), ...rows),
  ];
}

function sitePage(page: 'privacy' | 'terms'): HTMLElement {
  return button(t('settings.open_page'), () => void api.openSitePage(page, lang()).catch((e) => toast(errorText(e))), { small: true, key: `site-${page}`, label: t(`settings.${page}`) });
}

/** About's update line (#196): the last answer (checked once a day), and "Check for updates" to ask now. */
function updateControl(): HTMLElement {
  const state = h('span', { class: 'settings-value', role: 'status' }, t('settings.update_checking'));
  const download = h('span');
  const show = (u: UpdateCheck) => {
    state.textContent =
      u.state === 'newer' ? t('settings.update_newer', { version: u.version })
      : u.state === 'current' ? t('settings.update_current')
      : t('settings.update_failed');
    download.replaceChildren(
      ...(u.state === 'newer'
        ? [button(t('settings.update_download'), () => void api.openDownload(lang()).catch((e) => toast(errorText(e))), { small: true, kind: 'filled', key: 'update-download' })]
        : []),
    );
  };
  const check = (now: boolean) => {
    if (now) state.textContent = t('settings.update_checking');
    void api.checkUpdate(lang(), now).then(show, () => show({ state: 'failed' }));
  };
  check(false);
  return h('div', { class: 'settings-inline' }, state, download, button(t('settings.update_check'), () => check(true), { small: true, key: 'update-check' }));
}

function advancedPane(host: SettingsHost): HTMLElement[] {
  const service = host.conn.state === 'connected' ? (host.status?.version ?? host.conn.version) : t('settings.service_not_running');
  const value = (text: string) => h('span', { class: 'settings-value kf-tnum', dir: 'ltr' }, text);
  return [
    group('set-g-about', t('settings.about'),
      row({ id: 'set-app-version', icon: 'spark', title: t('settings.app_version'), control: value(host.info.version) }),
      row({ id: 'set-service-version', icon: 'pulse', title: t('settings.service_version'), control: value(service) })),
    group('set-g-updates', null, row({ id: 'set-updates', icon: 'sync', title: t('settings.updates'), control: updateControl() })),
    // SRS FR-R4 (#146): the privacy policy and the terms, on the Wasl site in the window's language.
    group('set-g-legal', null,
      row({ id: 'set-privacy', title: t('settings.privacy'), desc: t('settings.on_site'), control: sitePage('privacy') }),
      row({ id: 'set-terms', title: t('settings.terms'), desc: t('settings.on_site'), control: sitePage('terms') })),
  ];
}

/** One pane's groups. */
export async function settingsPane(host: SettingsHost, pane: SettingsPane): Promise<HTMLElement> {
  const body =
    pane === 'general' ? await generalPane(host)
    : pane === 'accounts' ? accountsPane(host)
    : pane === 'sync' ? syncPane(host)
    : pane === 'notifications' ? await notificationsPane(host)
    : advancedPane(host);
  return h('div', { class: 'settings-pane', 'data-pane': pane }, body);
}

/** The in-app page (Windows, D5): every pane on one page, each under its own heading. */
export async function settingsScreen(app: App): Promise<HTMLElement> {
  // The toolbar already shows the page title; the heading stays for screen readers and focus on arrival.
  const screen = h('div', { class: 'screen settings-page' }, h('h1', { class: 'sr-title', tabindex: '-1' }, t('settings.title')));
  app.setCommands([]);
  for (const pane of PANES) {
    const section = h('section', { class: 'settings-section', id: `settings-${pane}`, 'aria-labelledby': `settings-${pane}-h` },
      h('h2', { id: `settings-${pane}-h`, class: 'settings-section-title' }, t(`settings.pane_${pane}`)));
    section.append(await settingsPane(app, pane));
    screen.append(section);
  }
  return screen;
}
