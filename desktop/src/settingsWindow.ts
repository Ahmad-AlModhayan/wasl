// The macOS Settings window (#68, design D5), entered as `?window=settings` from src/main.ts. Toolbar tabs in the
// title bar's place, the window title following the tab, one pane at a time, the window sized to the pane. It
// reopens on the last pane. Changes apply at once here and in the main window (src/prefs.ts).

import { listen } from '@tauri-apps/api/event';
import { LogicalSize, getCurrentWindow } from '@tauri-apps/api/window';
import { api } from './api';
import { resolveLang, setLang, t } from './i18n';
import { PANES, accentOf, applyAccent, onSettingChange, paneOf, paneSignature, readStored, shareSetting, type Accent, type SettingsPane } from './prefs';
import { PANE_ICONS, settingsPane, type SettingsHost } from './screens/settings';
import type { AppInfo, Conn, DaemonEvent, DaemonStatus } from './types';
import { signIn } from './signIn';
import { h, line } from './ui';

const WIDTH = 620;

export class SettingsWindow implements SettingsHost {
  conn: Conn = { state: 'connecting' };
  status: DaemonStatus | null = null;
  info: AppInfo = { version: '', cli: false };
  langPref = readStored('language') ?? 'system';
  accent: Accent = accentOf(readStored('accent'));
  private pane: SettingsPane = paneOf(readStored('pane'));
  private listeners = new Set<(e: DaemonEvent) => void>();
  private token = 0;
  private root = document.getElementById('app')!;

  async start(): Promise<void> {
    setLang(resolveLang(this.langPref, navigator.language));
    applyAccent(document.documentElement, this.accent);
    onSettingChange((key, value) => {
      if (key === 'language' && value !== this.langPref) {
        this.langPref = value;
        setLang(resolveLang(value, navigator.language));
        void this.render();
      } else if (key === 'accent') {
        this.accent = accentOf(value);
        applyAccent(document.documentElement, this.accent);
        void this.render();
      } else if (key === 'pane') this.show(paneOf(value));
    });
    this.info = await api.appInfo().catch(() => this.info);
    await listen<Conn>('daemon://state', (e) => {
      this.conn = e.payload;
      void this.render();
    });
    await listen<DaemonStatus>('daemon://status', (e) => {
      const first = !this.status;
      const shown = paneSignature(this.pane, this.status);
      this.status = e.payload;
      // A status comes every second while syncing; redraw only when what this pane shows changed.
      if (first || paneSignature(this.pane, this.status) !== shown) void this.render();
    });
    await listen<DaemonEvent>('daemon://event', (e) => {
      signIn.event(e.payload);
      this.listeners.forEach((fn) => fn(e.payload));
    });
    this.conn = await api.daemonState().catch(() => this.conn);
    if (this.conn.state === 'connected') this.status = await api.status().catch(() => null);
    await this.render();
  }

  onEvent(fn: (e: DaemonEvent) => void): void {
    this.listeners.add(fn);
  }

  async setLanguage(pref: string): Promise<void> {
    this.langPref = pref;
    shareSetting('language', pref);
    if (this.conn.state === 'connected') await api.setSetting('language', pref).catch(() => undefined);
    setLang(resolveLang(pref, navigator.language));
    await this.render();
  }

  setAccent(accent: Accent): void {
    this.accent = accent;
    shareSetting('accent', accent);
    applyAccent(document.documentElement, accent);
  }

  private show(pane: SettingsPane): void {
    if (pane === this.pane) return;
    this.pane = pane;
    shareSetting('pane', pane);
    void this.render(true);
  }

  private async render(focusTab = false): Promise<void> {
    const token = ++this.token;
    this.listeners.clear();
    const title = t(`settings.pane_${this.pane}`);
    const panel = await settingsPane(this, this.pane);
    if (token !== this.token) return;
    const tabs = h(
      'div',
      { class: 'settings-tabs', role: 'tablist', 'aria-label': t('settings.title') },
      PANES.map((p) =>
        h(
          'button',
          {
            type: 'button',
            role: 'tab',
            class: 'settings-tab',
            id: `tab-${p}`,
            'aria-selected': String(p === this.pane),
            'aria-controls': 'settings-panel',
            tabindex: p === this.pane ? '0' : '-1',
            'data-key': `tab-${p}`,
            onclick: () => this.show(p),
          },
          line(PANE_ICONS[p], { size: 20 }),
          h('span', null, t(`settings.pane_${p}`)),
        )),
    );
    tabs.addEventListener('keydown', (e) => this.onTabKey(e));
    document.title = title;
    // Keep keyboard focus across a redraw: the control with the same data-key takes it back.
    const focusKey = this.root.contains(document.activeElement)
      ? (document.activeElement as HTMLElement | null)?.closest<HTMLElement>('[data-key]')?.dataset.key
      : undefined;
    const focusInRoot = this.root.contains(document.activeElement) && document.activeElement !== this.root;
    this.root.replaceChildren(
      h(
        'div',
        { class: 'settings-window' },
        h('header', { class: 'settings-titlebar', 'data-tauri-drag-region': '' }, h('h1', { 'data-tauri-drag-region': '', tabindex: '-1' }, title), tabs),
        h('main', { class: 'settings-body', id: 'settings-panel', role: 'tabpanel', 'aria-labelledby': `tab-${this.pane}` }, panel),
        h('div', { id: 'toast', class: 'toast-region', role: 'status', 'aria-live': 'polite' }),
      ),
    );
    if (focusTab) this.root.querySelector<HTMLElement>(`#tab-${this.pane}`)?.focus();
    else if (focusKey || focusInRoot) {
      const again = focusKey ? this.root.querySelector<HTMLElement>(`[data-key="${CSS.escape(focusKey)}"]`) : null;
      (again ?? this.root.querySelector<HTMLElement>(`#tab-${this.pane}`))?.focus();
    }
    await this.fit(title);
  }

  /** Arrow keys move between tabs (mirrored in Arabic), Home and End jump to the ends. */
  private onTabKey(e: KeyboardEvent): void {
    const i = PANES.indexOf(this.pane);
    const rtl = document.documentElement.dir === 'rtl';
    const step = e.key === 'ArrowRight' ? (rtl ? -1 : 1) : e.key === 'ArrowLeft' ? (rtl ? 1 : -1) : 0;
    const next = e.key === 'Home' ? 0 : e.key === 'End' ? PANES.length - 1 : step ? (i + step + PANES.length) % PANES.length : -1;
    if (next < 0) return;
    e.preventDefault();
    this.show(PANES[next]);
  }

  /** The window takes the pane's height, and its title follows the tab (the Window menu lists it). */
  private async fit(title: string): Promise<void> {
    if (!('__TAURI_INTERNALS__' in window) || '__WASL_MOCK__' in window) return;
    const win = getCurrentWindow();
    const height = Math.ceil((this.root.firstElementChild as HTMLElement).scrollHeight);
    await win.setTitle(title).catch(() => undefined);
    await win.setSize(new LogicalSize(WIDTH, Math.min(Math.max(height, 240), 720))).catch(() => undefined);
  }
}
