// The window's state and shell (#63): the sidebar, the toolbar, the detail pane and the inspector (src/shell), the
// connection panel, live updates from the daemon, the keyboard, and the tray's and the macOS menu bar's words.
//
// Mount points for the screens that replace the old ones (#64–#71): each route's screen fills the detail pane
// (`<main>`); `setCommands` gives the toolbar its commands, `setInspector` the inspector its content, and `select`
// tells the shell which pairs a screen has selected (Sync now, Pause and Show in Finder act on them). `query` is
// the search field's text and `onQuery` reports it as it changes.

import { listen } from '@tauri-apps/api/event';
import { api, isUiError } from './api';
import { lang, resolveLang, setLang, t } from './i18n';
import {
  formatRelative,
  overallLine,
  pairsSummary,
  pairStatus,
  progressFraction,
  progressText,
  stateChanges,
  statusLabel,
  type StatusState,
} from './format';
import { button, errorBanner, errorText, h, line, loading, toast } from './ui';
import { alertScope, appAlert, firstRunDecision, firstRunStart, type AppAlert } from './appState';
import type { LineName } from './icons';
import { GO_ROUTES, MENU_EVENT, isMenuAction, menuText, type MenuAction } from './chrome/menu';
import type { AppInfo, Conn, DaemonEvent, DaemonStatus, PairState, Progress } from './types';
import { platformOf, type Platform } from './platform';
import { accentOf, applyAccent, onSettingChange, readStored, shareSetting, type Accent, type SettingsPane } from './prefs';
import { pairRowName, renderSidebar, sectionOf, sidebarKeys } from './shell/sidebar';
import { renderToolbar, type Command } from './shell/toolbar';
import { renderInspector } from './shell/inspector';
import { reloadsWebview, shortcutFor, type ShellAction } from './shell/keys';
import type { MenuLine } from './shell/contextMenu';
import { pairsScreen, removePair, removePairs, showInTable } from './screens/pairs';
import { pairDetailScreen } from './screens/pairDetail';
import { openPairSheet } from './screens/addPair';
import { previewScreen } from './screens/preview';
import { problemsScreen } from './screens/problems';
import { signIn } from './signIn';
import { historyScreen } from './screens/history';
import { agentsScreen } from './screens/agents';
import { binScreen } from './screens/bin';
import { settingsScreen } from './screens/settings';
import { devicesScreen } from './screens/devices';
import { viewsScreen } from './screens/views';
import { openViewSheet } from './screens/addView';
import { firstRunView, newFirstRun, type FirstRunState } from './screens/firstRun';
import { dropMessage, dropVerdict, mountFolderDrop, type DropVerdict } from './drop';
import { trayRoute } from './tray/model';
import { notifyText } from './notifyText';

export type Route =
  | { name: 'pairs' }
  | { name: 'pair'; id: string }
  | { name: 'add' }
  | { name: 'preview'; id: string }
  | { name: 'problems' }
  | { name: 'history' }
  | { name: 'run'; id: string; title: string }
  | { name: 'bin' }
  | { name: 'agents' }
  | { name: 'devices' }
  | { name: 'views' }
  | { name: 'addView' }
  | { name: 'settings' };

/** What makes each screen re-read: the daemon's status, or "something changed" events. */
const LIVE: Record<Route['name'], Array<'status' | 'changed'>> = {
  pairs: ['status', 'changed'], // changed: who a pair is shared with (docs/INTRODUCTIONS.md §3.2)
  pair: ['status', 'changed'],
  add: [],
  preview: [],
  problems: ['status', 'changed'],
  history: ['changed'],
  run: ['changed'],
  bin: ['changed'],
  agents: ['changed'],
  devices: [], // re-reads its own parts on `changed`, so a half-typed code survives
  views: [], // re-reads its list on `changed`, keeping focus
  addView: [],
  settings: ['status'],
};

/** Screens that work without the daemon. */
const OFFLINE_OK: Array<Route['name']> = ['settings'];

/** What the banner slot shows for a state (#70). */
interface BannerView {
  tone: 'approval' | 'failed' | 'conflict' | 'waiting';
  icon: LineName;
  title: string;
  body: string;
  action?: { label: string; run: () => void };
  busy?: boolean;
}

const LANG_KEY = 'wasl.language';
const FIRST_RUN_KEY = 'wasl.firstRun';
const INSPECTOR_KEY = 'wasl.inspector';
const SIDEBAR_KEY = 'wasl.sidebar';

/** At this width and below the inspector lies over the pane instead of beside it. */
const NARROW_INSPECTOR = '(max-width: 900px)';

/** How often a screen reader hears about state changes, at most. */
const ANNOUNCE_MS = 4000;

export class App {
  conn: Conn = { state: 'connecting' };
  status: DaemonStatus | null = null;
  info: AppInfo = { version: '', cli: false };
  route: Route = { name: 'pairs' };
  langPref = 'system';
  /** Selection and focus colour: Wasl navy, or the person's system accent (#68). */
  accent: Accent = accentOf(readStored('accent'));
  /** Windows: the system accent as Rust reads it (WebView2 has no CSS AccentColor). */
  private systemColor: string | null = null;
  /** Windows: the system accent was chosen but Rust couldn't read it, so Wasl navy shows. */
  accentMissing = false;
  /** The search field's text. */
  query = '';
  /** The person's choice for a wide window, remembered. */
  private inspectorWide = readLocal(INSPECTOR_KEY) !== 'closed';
  /** A narrow window shows the inspector over the pane only when asked, until the next navigation. */
  private inspectorOverlay = false;

  private listeners = new Set<(e: DaemonEvent) => void>();
  private queryListeners = new Set<(q: string) => void>();
  private renderToken = 0;
  private shell!: HTMLElement;
  private nav!: HTMLElement;
  private toolbar!: HTMLElement;
  private banner!: HTMLElement;
  /** The banner's state last drawn, so a state is read out once. */
  private bannerKey = '';
  /** The first run, while it shows (#70). */
  private firstRun: FirstRunState | null = null;
  private firstRunChecked = false;
  /** Counts connection changes, so a status read on one connection isn't applied on the next (#108). */
  private connSeq = 0;
  /** A Dock folder taken before the first status, routed once the first-run check has run (#71). */
  private pendingFolder: string | null = null;
  private main!: HTMLElement;
  private inspector!: HTMLElement;
  private live!: HTMLElement;
  /** auto: shown unless the window is narrow; hidden or shown: the person chose. */
  private sidebarMode: 'auto' | 'hidden' | 'shown' = readLocal(SIDEBAR_KEY) === 'hidden' ? 'hidden' : 'auto';
  private screenCommands: Command[] | null = null;
  private screenInspector: (() => HTMLElement) | null = null;
  private selection: string[] = [];
  private lastStates = new Map<string, StatusState>();
  private pendingAnnounce: string[] = [];
  private announceTimer: ReturnType<typeof setTimeout> | undefined;
  private lastAnnounce = 0;

  async start(): Promise<void> {
    this.langPref = readLocal(LANG_KEY) ?? 'system';
    setLang(resolveLang(this.langPref, navigator.language));
    this.buildShell();
    // Until the service answers, the pane says it is connecting.
    void this.renderMain(false);
    this.bindChromeSearch();
    document.addEventListener('keydown', (e) => this.onKey(e));
    trackInputModality();
    // Both platforms grey the selection while the window is in the background (kahf-design's .kf-desk-inactive).
    window.addEventListener('blur', () => document.documentElement.classList.add('kf-desk-inactive'));
    window.addEventListener('focus', () => {
      document.documentElement.classList.remove('kf-desk-inactive');
      // The person may have changed their accent while Wasl was in the background.
      if (this.accent === 'system') void this.refreshAccent();
    });
    void this.refreshAccent();
    // Offline is the system's word (#70): the banner follows it.
    window.addEventListener('online', () => this.renderChrome());
    window.addEventListener('offline', () => this.renderChrome());
    onSettingChange((key, value) => this.onSharedSetting(key, value));
    // Crossing the narrow widths changes what the toggles say.
    for (const q of ['(max-width: 760px)', '(max-width: 1007px)', '(max-width: 640px)', NARROW_INSPECTOR]) {
      window.matchMedia(q).addEventListener('change', () => this.renderChrome());
    }
    this.info = await api.appInfo().catch(() => this.info);
    // The daily update check (#196): silent unless a newer version is out; a failed check shows only in About.
    void api.checkUpdate(lang(), false).then((u) => {
      if (u.state === 'newer') toast(t('update.out_toast', { version: u.version }), { label: t('update.open_settings'), run: () => this.openSettings('advanced') });
    }, () => {});
    await listen<Conn>('daemon://state', (e) => this.onConn(e.payload));
    await listen<DaemonStatus>('daemon://status', (e) => this.onStatus(e.payload));
    await listen<DaemonEvent>('daemon://event', (e) => this.onDaemonEvent(e.payload));
    await listen<string>(MENU_EVENT, (e) => this.onMenuAction(e.payload));
    await listen<string>('tray://open', (e) => this.onTrayOpen(e.payload));
    this.onConn(await api.daemonState());
    // A folder dropped on the window, or on the Dock icon (#71).
    await mountFolderDrop({
      verdict: () => this.dropVerdict(),
      takeFolder: (folder) => this.takeFolder(folder),
      announce: (text) => this.announce(text),
    }).catch(() => undefined);
    await listen('app://opened-folder', () => void this.takeOpenedFolder());
    void this.takeOpenedFolder();
  }

  /** What a dropped folder would do now. Ready means connected and past the first-run check (the first status), so
   * a new person's folder goes to the first run rather than to a New pair sheet with the first run behind it. */
  private dropVerdict(): DropVerdict {
    return dropVerdict({
      ready: this.conn.state === 'connected' && this.firstRunChecked,
      dialogOpen: !!document.querySelector('dialog[open]'),
      firstRunStep: this.firstRun?.step ?? null,
    });
  }

  /** A folder dropped on the window or the Dock icon: New pair with it, or the first run's folder (#71). */
  takeFolder(folder: string): void {
    const verdict = this.dropVerdict();
    const message = dropMessage(verdict);
    if (message) toast(message);
    if (verdict !== 'take' && verdict !== 'kept') return;
    if (this.firstRun) {
      this.firstRun.folder = folder;
      // Past the folder step, go back to it so the dropped folder shows; before it, the folder step will show it.
      if (this.firstRun.step === 'tray') {
        this.firstRun.step = 'folder';
        this.renderFirstRun(true);
      } else if (this.firstRun.step === 'folder') this.renderFirstRun(false);
      return;
    }
    void this.newPair(folder);
  }

  /** A folder dropped on the Dock icon. At a cold start it waits for the first status (and its first-run check). */
  private async takeOpenedFolder(): Promise<void> {
    const folder = await api.takeOpenedFolder().catch(() => null);
    if (!folder) return;
    if (this.firstRunChecked) this.takeFolder(folder);
    else this.pendingFolder = folder;
  }

  // ------------------------------------------------------------ navigation and rendering

  go(route: Route): void {
    // Making something opens a sheet over the current screen, never a page (D4, #65).
    if (route.name === 'add') return void this.newPair();
    if (route.name === 'addView') return void openViewSheet(this);
    this.route = route;
    // A new screen starts with the shell's own commands, inspector and selection.
    this.screenCommands = null;
    this.screenInspector = null;
    this.selection = route.name === 'pair' ? [route.id] : [];
    this.inspectorOverlay = false;
    // A narrow window's sidebar was shown to pick something: it steps aside again.
    if (this.sidebarMode === 'shown' && this.sidebarNarrow()) this.sidebarMode = 'auto';
    this.renderChrome();
    // The selection changed with the route: the menu bar's Show in Finder and Pause or Resume follow it.
    this.pushMenu();
    void this.renderMain(true);
  }

  /** The New pair sheet; `local` is a folder already chosen (dropped on the window, #71). */
  newPair(local?: string): Promise<void> {
    return openPairSheet(this, { local });
  }

  /** The Edit pair sheet for one pair. */
  editPair(id: string): Promise<void> {
    const edit = this.status?.pairs.find((p) => p.pair.id === id);
    return edit ? openPairSheet(this, { edit }) : Promise.resolve();
  }

  /** Re-reads the current screen, keeping focus and scroll where they were. */
  rerender(): void {
    void this.renderMain(false);
  }

  /** Daemon events for the current screen; cleared when the screen changes. */
  onEvent(fn: (e: DaemonEvent) => void): void {
    this.listeners.add(fn);
  }

  /** The search field's text as it changes, for the current screen; registered again on every render. */
  onQuery(fn: (q: string) => void): void {
    this.queryListeners.add(fn);
  }

  setQuery(q: string): void {
    this.query = q;
    this.renderNav();
    // A copy: a listener that re-renders the screen registers its successor while this runs.
    for (const fn of [...this.queryListeners]) fn(q);
  }

  /** The toolbar's commands for the current screen, instead of the shell's own. */
  setCommands(commands: Command[] | null): void {
    this.screenCommands = commands;
    this.renderToolbar();
  }

  /** The inspector's content for the current screen, instead of the selected pair. */
  setInspector(content: (() => HTMLElement) | null): void {
    this.screenInspector = content;
    this.renderInspector();
  }

  /** The pairs the current screen has selected. */
  select(pairIds: string[]): void {
    this.selection = pairIds;
    this.renderInspector();
    this.renderToolbar();
    this.pushMenu();
  }

  /** Tells a screen reader something now, politely (a selection count, say). */
  announce(text: string): void {
    this.live.textContent = '';
    // A fresh text node, so the same words twice are read twice.
    requestAnimationFrame(() => (this.live.textContent = text));
  }

  /** Esc and Back on Pair detail: the table, with the pair's row selected and focused. */
  backToPairs(): void {
    if (this.route.name === 'pair') showInTable(this.route.id);
    this.go({ name: 'pairs' });
  }

  /** The selected pairs that still exist. */
  selectedPairs(): PairState[] {
    const pairs = this.status?.pairs ?? [];
    return this.selection.map((id) => pairs.find((p) => p.pair.id === id)).filter((p): p is PairState => !!p);
  }

  /** What Sync now and Pause act on: the selection, or every pair. */
  private targets(): PairState[] {
    const sel = this.selectedPairs();
    return sel.length ? sel : this.status?.pairs ?? [];
  }

  setAccent(accent: Accent): void {
    this.accent = accent;
    shareSetting('accent', accent);
    void this.refreshAccent();
  }

  /** Applies the accent; on Windows asks Rust for the system colour first. Without one Windows stays navy, and the
   *  Settings page says so. */
  private async refreshAccent(): Promise<void> {
    if (this.accent === 'system' && !isMac()) this.systemColor = await api.systemAccent().catch(() => null);
    const missing = this.accent === 'system' && !applyAccent(document.documentElement, this.accent, this.systemColor, isMac());
    if (missing !== this.accentMissing) {
      this.accentMissing = missing;
      if (this.route.name === 'settings') void this.renderMain(false);
    }
  }

  /** Another window (the macOS Settings window) changed a setting: apply it here too, without a reload. */
  private onSharedSetting(key: string, value: string): void {
    if (key === 'accent' && accentOf(value) !== this.accent) {
      this.accent = accentOf(value);
      void this.refreshAccent();
      if (this.route.name === 'settings') void this.renderMain(false);
    } else if (key === 'language' && value !== this.langPref) {
      this.langPref = value;
      setLang(resolveLang(value, navigator.language));
      this.buildShell();
      void this.renderMain(false);
      this.pushTray();
      this.pushMenu();
    }
  }

  /** Settings (D5): its own window on macOS, the in-app page on Windows; `pane` picks where it opens. */
  openSettings(pane?: SettingsPane): void {
    if (isMac()) {
      if (pane) shareSetting('pane', pane);
      void api.openSettings().catch((e) => toast(errorText(e)));
      return;
    }
    if (this.route.name !== 'settings') this.go({ name: 'settings' });
    if (pane) requestAnimationFrame(() => setTimeout(() => document.getElementById(`settings-${pane}`)?.scrollIntoView({ block: 'start' }), 300));
  }

  async setLanguage(pref: string): Promise<void> {
    this.langPref = pref;
    writeLocal(LANG_KEY, pref);
    shareSetting('language', pref);
    if (this.conn.state === 'connected') await api.setSetting('language', pref).catch(() => undefined);
    setLang(resolveLang(pref, navigator.language));
    this.buildShell();
    void this.renderMain(false);
    this.pushTray();
    this.pushMenu();
  }

  /** Windows: #62's title-bar search box is the shell's search field there, outside the shell the app rebuilds. */
  private bindChromeSearch(): void {
    const input = chromeSearch();
    if (!input) return;
    input.removeAttribute('tabindex');
    input.autocomplete = 'off';
    input.spellcheck = false;
    input.value = this.query;
    input.addEventListener('input', () => this.setQuery(input.value));
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Escape' && input.value) {
        e.stopPropagation();
        input.value = '';
        this.setQuery('');
      }
    });
  }

  private buildShell(): void {
    document.title = t('app.name');
    const root = document.getElementById('app')!;
    this.nav = h('nav', { class: 'kf-desk-sidebar shell-sidebar', id: 'shell-sidebar', 'aria-label': t('nav.label') });
    sidebarKeys(this.nav);
    this.toolbar = h('header', { class: 'shell-toolbar' });
    this.banner = h('div', { class: 'shell-banner-slot' });
    this.main = h('main', { id: 'main', class: 'main', tabindex: '-1' });
    this.inspector = h('aside', { class: 'kf-desk-inspector shell-inspector', 'aria-labelledby': 'shell-inspector-title' });
    this.live = h('div', { class: 'sr-title', 'aria-live': 'polite', role: 'status' });
    this.shell = h(
      'div',
      // macOS: the gutters around the floating sidebar move the window too (only the bare shell, not its children).
      { class: 'shell', 'data-tauri-drag-region': isMac() ? '' : undefined },
      this.nav,
      // A narrow window shows the sidebar over the pane; a click beside it puts it away.
      h('div', { class: 'shell-scrim', 'aria-hidden': 'true', onclick: () => this.toggleSidebar() }),
      h(
        'div',
        { class: 'pane' },
        this.toolbar,
        h('div', { class: 'shell-body' }, h('div', { class: 'shell-detail' }, this.banner, this.main), this.inspector),
      ),
    );
    if (this.firstRun) return this.renderFirstRun(false);
    root.replaceChildren(
      h('a', { class: 'skip', href: '#main' }, t('app.skip')),
      this.shell,
      this.live,
      h('div', { id: 'toast', class: 'toast-region', role: 'status', 'aria-live': 'polite' }),
    );
    this.renderChrome();
  }

  /** Everything around the detail pane. */
  private renderChrome(): void {
    if (!this.shell || this.firstRun) return;
    this.shell.dataset.sidebarView = this.sidebarView();
    this.renderNav();
    this.renderToolbar();
    this.renderBanner();
    this.renderInspector();
  }

  private renderNav(): void {
    // Keep focus on the same row across a re-render (live status updates arrive while someone is in the list).
    const focusKey = this.nav.contains(document.activeElement)
      ? (document.activeElement as HTMLElement | null)?.dataset.key
      : undefined;
    renderSidebar(this, this.nav);
    if (focusKey) this.nav.querySelector<HTMLElement>(`[data-key="${CSS.escape(focusKey)}"]`)?.focus();
  }

  private renderToolbar(): void {
    // The search field stays in place across renders (toolbar.ts), so its caret and an IME composition survive.
    const focusKey = this.toolbar.contains(document.activeElement) ? (document.activeElement as HTMLElement | null)?.dataset.key : undefined;
    renderToolbar(this, this.toolbar);
    if (focusKey && !this.toolbar.contains(document.activeElement)) this.toolbar.querySelector<HTMLElement>(`[data-key="${CSS.escape(focusKey)}"]`)?.focus();
  }

  private renderInspector(): void {
    if (!this.inspectorOpen) {
      this.inspector.hidden = true;
      this.inspector.replaceChildren();
      return;
    }
    this.inspector.hidden = false;
    const focusKey = this.inspector.contains(document.activeElement) ? (document.activeElement as HTMLElement | null)?.dataset.key : undefined;
    renderInspector(this, this.inspector);
    if (focusKey) this.inspector.querySelector<HTMLElement>(`[data-key="${CSS.escape(focusKey)}"]`)?.focus();
  }

  /**
   * The one app-wide banner (#70, design D8): the most urgent state only, with its one fix (src/appState.ts). States
   * that concern the whole app show on every screen; the pairs' own show on All pairs.
   */
  private renderBanner(): void {
    const a = appAlert({ conn: this.conn, status: this.status, online: navigator.onLine });
    const shown = a && (alertScope(a) === 'app' || this.route.name === 'pairs') ? this.alertView(a) : null;
    // Live updates rebuild it: its button keeps the focus.
    const hadFocus = this.banner.contains(document.activeElement);
    const key = shown ? `${shown.tone}|${shown.title}|${shown.body}|${shown.action?.label ?? ''}` : '';
    if (!shown) {
      this.banner.replaceChildren();
      this.bannerKey = '';
      if (hadFocus) this.focusPane('main');
      return;
    }
    const action = shown.action;
    this.banner.replaceChildren(
      h(
        'section',
        { class: `shell-banner status-${shown.tone}`, 'aria-label': shown.title, 'aria-busy': shown.busy ? 'true' : undefined },
        line(shown.icon, { size: 20 }),
        h('div', { class: 'shell-banner-text' }, h('p', { class: 'shell-banner-title' }, shown.title), h('p', null, shown.body)),
        action ? button(action.label, action.run, { kind: 'filled', key: 'banner-action' }) : null,
      ),
    );
    // A new state is read out once, not on every live update.
    if (key !== this.bannerKey) {
      if (this.bannerKey !== '' || a!.kind !== 'pairs') this.announce(`${shown.title}${/[.…!?]$/.test(shown.title) ? '' : '.'} ${shown.body}`);
      this.bannerKey = key;
    }
    if (hadFocus) this.banner.querySelector<HTMLElement>('[data-key="banner-action"]')?.focus();
  }

  /** A state's words, glyph and fix. */
  private alertView(a: AppAlert): BannerView {
    const show = (p: PairState | null, many: () => void) =>
      p ? { label: t('alert.show_pair'), run: () => this.go({ name: 'pair', id: p.pair.id }) } : { label: t('banner.review'), run: many };
    const review = () => this.go({ name: 'problems' });
    switch (a.kind) {
      case 'starting':
        return { tone: 'waiting', icon: 'syncing', title: t('alert.starting'), body: t('alert.starting_body'), busy: true };
      case 'stopped':
        return {
          tone: 'failed',
          icon: 'failed',
          title: t('alert.stopped'),
          body: a.notFound ? t('conn.not_found') : a.message ? t('conn.failed', { message: a.message }) : t('alert.stopped_body'),
          action: { label: t(a.notFound || a.message ? 'common.retry' : 'alert.restart'), run: () => void api.startDaemon().catch((e) => toast(errorText(e))) },
        };
      case 'offline':
        return { tone: 'waiting', icon: 'waiting', title: t('alert.offline'), body: t('alert.offline_body') };
      case 'auth':
        return {
          tone: 'failed',
          icon: 'failed',
          title: a.count ? t('alert.auth', { count: a.count }) : t('banner.auth'),
          body: a.count ? t('alert.auth_body') : t('banner.auth_body'),
          action: { label: t('banner.sign_in'), run: () => this.openSettings('accounts') },
        };
      case 'quota':
        return {
          tone: 'failed',
          icon: 'failed',
          title: t('alert.quota'),
          body: a.pair ? t('alert.quota_body_pair', { name: a.pair.pair.name }) : t('alert.quota_body', { count: a.count }),
          action: show(a.pair, review),
        };
      case 'missing':
        return {
          tone: 'failed',
          icon: 'failed',
          title: a.pair ? t('alert.missing_pair', { name: a.pair.pair.name }) : t('alert.missing', { count: a.count }),
          body: t('alert.missing_body'),
          action: show(a.pair, review),
        };
      case 'pairs': {
        const b = a.banner;
        const held = b.preview;
        return {
          tone: b.state,
          icon: b.state,
          title: b.auth ? t('banner.auth') : held ? t('banner.preview', { count: b.count }) : t(`banner.${b.state}`, { count: b.count }),
          body: b.auth ? t('banner.auth_body') : held ? t('banner.preview_body') : t(`banner.${b.state}_body`),
          action: b.auth
            ? { label: t('banner.sign_in'), run: () => this.openSettings('accounts') }
            : held
              ? { label: t('pairs.preview_and_start'), run: () => this.go({ name: 'preview', id: held }) }
              : { label: t('banner.review'), run: review },
        };
      }
    }
  }

  /** The toolbar's title and subtitle for the current route. */
  titles(): { title: string; subtitle: string | null } {
    const r = this.route;
    if (r.name === 'pair') {
      const p = this.status?.pairs.find((x) => x.pair.id === r.id);
      return { title: p?.pair.name ?? t('nav.pairs'), subtitle: p ? statusLabel(pairStatus(p), lang()) : null };
    }
    const sec = sectionOf(r) ?? 'pairs';
    const s = sec === 'pairs' && r.name === 'pairs' && this.conn.state === 'connected' ? this.status : null;
    const subtitle = s ? (s.pairs.length ? pairsSummary(s, lang()) : t('shell.sub_none')) : null;
    return { title: t(`nav.${sec}`), subtitle };
  }

  /** The toolbar's commands: the screen's own, or the shell's (Pause, Sync now, New pair as the primary). */
  commands(): Command[] {
    if (this.screenCommands) return this.screenCommands;
    const connected = this.conn.state === 'connected';
    const targets = this.targets();
    const one = this.route.name === 'pair';
    // With pairs selected, Pause and Sync now act on them, so Pause says so ("Pause", not "Pause all").
    const scoped = one || this.selectedPairs().length > 0;
    const allPaused = targets.length > 0 && targets.every((p) => p.pair.paused);
    const cmds: Command[] = [
      {
        id: 'pause',
        label: scoped ? t(allPaused ? 'pairs.resume' : 'pairs.pause') : t(allPaused ? 'tray.resume_all' : 'tray.pause_all'),
        icon: allPaused ? 'play' : 'pause',
        run: () => this.setPaused(targets, !allPaused),
        disabled: !connected || targets.length === 0,
        shortcut: 'toggle_pause',
      },
      {
        id: 'sync',
        label: t('pairs.sync_now'),
        icon: 'sync',
        run: () => this.syncNow(),
        disabled: !connected || targets.length === 0,
        shortcut: 'sync_now',
      },
    ];
    if (one && targets[0]) {
      const id = targets[0].pair.id;
      cmds.push({ id: 'reveal', label: this.revealLabel(), icon: 'reveal', run: () => this.showInFolder(id), shortcut: 'show_in_finder' });
    } else {
      cmds.push({ id: 'new', label: t('shell.new_pair'), icon: 'plus', run: () => void this.newPair(), primary: true, shortcut: 'new_pair' });
    }
    return cmds;
  }

  inspectorContent(): HTMLElement | null {
    return this.screenInspector ? this.screenInspector() : null;
  }

  // ------------------------------------------------------------ pair actions (toolbar, menus, inspector)

  revealLabel(): string {
    return t(isMac() ? 'shell.show_in_finder' : 'shell.show_in_explorer');
  }

  async setPaused(pairs: PairState[], paused: boolean): Promise<void> {
    try {
      await Promise.all(pairs.map((p) => api.setPaused(p.pair.id, paused)));
    } catch (e) {
      toast(errorText(e));
    }
  }

  /** Sync now: the selected pairs, or every pair with nothing selected. */
  syncNow(ids: string[] = this.selectedPairs().map((p) => p.pair.id)): void {
    const run = ids.length ? Promise.all(ids.map((id) => api.sync(id, 'manual'))) : api.syncAll();
    run.catch((e: unknown) => toast(errorText(e)));
  }

  showInFolder(id: string): void {
    api.showPairFolder(id).catch((e: unknown) => toast(errorText(e)));
  }

  /** A pair's context menu: on its sidebar row and its row in the All pairs table. */
  pairMenu(id: string): MenuLine[] {
    const p = this.status?.pairs.find((x) => x.pair.id === id);
    if (!p) return [];
    const paused = p.pair.paused;
    return [
      { text: t('shell.open'), icon: 'folder', run: () => this.go({ name: 'pair', id }) },
      'separator',
      { text: t('pairs.sync_now'), icon: 'sync', shortcut: 'sync_now', run: () => this.syncNow([id]) },
      { text: t(paused ? 'pairs.resume' : 'pairs.pause'), icon: paused ? 'play' : 'pause', shortcut: 'toggle_pause', run: () => void this.setPaused([p], !paused) },
      { text: t('detail.preview'), icon: 'eye', run: () => this.go({ name: 'preview', id }) },
      { text: t('pairsheet.edit'), icon: 'gear', run: () => void this.editPair(id) },
      'separator',
      { text: this.revealLabel(), icon: 'reveal', shortcut: 'show_in_finder', run: () => this.showInFolder(id) },
      'separator',
      { text: t('shell.remove_pair'), icon: 'trash', danger: true, run: () => void removePair(id, p.pair.name) },
    ];
  }

  /** The context menu on a row of a multi-selection: what the bulk bar does, for all of them. */
  pairsMenu(pairs: PairState[]): MenuLine[] {
    const paused = pairs.length > 0 && pairs.every((p) => p.pair.paused);
    return [
      { text: t('pairs.sync_now'), icon: 'sync', shortcut: 'sync_now', run: () => this.syncNow(pairs.map((p) => p.pair.id)) },
      { text: t(paused ? 'pairs.resume' : 'pairs.pause'), icon: paused ? 'play' : 'pause', shortcut: 'toggle_pause', run: () => void this.setPaused(pairs, !paused) },
      'separator',
      { text: t('pairs.remove_selected'), icon: 'trash', danger: true, run: () => void removePairs(pairs) },
    ];
  }

  // ------------------------------------------------------------ sidebar, inspector, panes

  private sidebarNarrow(): boolean {
    return window.matchMedia('(max-width: 760px)').matches;
  }

  /**
   * How the sidebar shows now, the same on both platforms (#124): beside the pane, or collapsed at 760px and below.
   * Opened in a narrow window, it lies over the pane.
   */
  sidebarView(): 'full' | 'overlay' | 'none' {
    const narrow = this.sidebarNarrow();
    if (this.sidebarMode === 'shown' && narrow) return 'overlay';
    return this.sidebarMode === 'hidden' || narrow ? 'none' : 'full';
  }

  /** Whether the sidebar shows now. */
  sidebarVisible(): boolean {
    return this.sidebarView() !== 'none';
  }

  toggleSidebar(): void {
    if (this.sidebarVisible()) this.sidebarMode = this.sidebarNarrow() ? 'auto' : 'hidden';
    else this.sidebarMode = this.sidebarNarrow() ? 'shown' : 'auto';
    if (!this.sidebarNarrow()) writeLocal(SIDEBAR_KEY, this.sidebarMode === 'hidden' ? 'hidden' : 'auto');
    this.renderChrome();
    this.pushMenu();
    if (this.sidebarMode === 'shown') this.focusPane('sidebar');
  }

  /** Whether the inspector shows now. */
  get inspectorOpen(): boolean {
    if (this.route.name === 'settings') return false; // Settings is a page of its own (D5)
    return window.matchMedia(NARROW_INSPECTOR).matches ? this.inspectorOverlay : this.inspectorWide;
  }

  /** `focus`: move the keyboard into it when it opens (not for Space in the table, which keeps its place). */
  toggleInspector(focus = true): void {
    if (window.matchMedia(NARROW_INSPECTOR).matches) {
      this.inspectorOverlay = !this.inspectorOverlay;
    } else {
      this.inspectorWide = !this.inspectorWide;
      writeLocal(INSPECTOR_KEY, this.inspectorWide ? 'open' : 'closed');
    }
    this.renderChrome();
    this.pushMenu();
    if (focus && this.inspectorOpen) this.focusPane('inspector');
  }

  /** Esc puts away what lies over the pane: the sidebar or the inspector opened in a narrow window. */
  private closeOverlays(): boolean {
    if (this.sidebarView() === 'overlay') {
      this.toggleSidebar();
      this.focusPane('main');
      return true;
    }
    if (this.inspectorOverlay && window.matchMedia(NARROW_INSPECTOR).matches) {
      this.toggleInspector();
      this.focusPane('main');
      return true;
    }
    return false;
  }

  /** F6 and Shift+F6 (Windows): sidebar, then the list, then the detail. */
  private cyclePane(by: 1 | -1): void {
    const panes = (['sidebar', 'main', 'inspector'] as const).filter((p) => (p === 'inspector' ? this.inspectorOpen : p === 'sidebar' ? this.nav.offsetParent !== null : true));
    const active = document.activeElement;
    const at = this.nav.contains(active) ? 'sidebar' : this.inspector.contains(active) ? 'inspector' : 'main';
    const i = panes.indexOf(at as never);
    this.focusPane(panes[(i + by + panes.length) % panes.length]!);
  }

  private focusPane(pane: 'sidebar' | 'main' | 'inspector'): void {
    if (pane === 'sidebar') {
      (this.nav.querySelector<HTMLElement>('[aria-current="page"]') ?? this.nav.querySelector<HTMLElement>('button'))?.focus();
    } else if (pane === 'inspector') {
      this.inspector.querySelector<HTMLElement>('h2, button')?.focus();
    } else {
      (this.main.querySelector<HTMLElement>('h1') ?? this.main).focus();
    }
  }

  // ------------------------------------------------------------ the keyboard (src/shell/keys.ts)

  private onKey(e: KeyboardEvent): void {
    if (e.defaultPrevented || e.isComposing) return;
    // Windows: F5 and Ctrl+R would reload the webview and lose what was typed, dialogs and fields included.
    const reload = reloadsWebview(e, platform());
    if (reload) e.preventDefault();
    // The first run has no shell: its own buttons and Return only.
    if (this.firstRun) return;
    // A modal dialog has the keyboard to itself.
    if (document.querySelector('dialog[open]')) return;
    if (e.key === 'Escape' && !e.altKey && !e.ctrlKey && !e.metaKey && this.closeOverlays()) {
      e.preventDefault();
      return;
    }
    const target = e.target as HTMLElement | null;
    const typing = !!target?.closest('input, textarea, select, [contenteditable="true"]');
    // Pair detail: Esc, ⌘[ (macOS) or Alt+Left (Windows) goes back to the table.
    if (this.route.name === 'pair' && !typing && isBack(e)) {
      e.preventDefault();
      this.backToPairs();
      return;
    }
    const s = shortcutFor(e, platform(), { nativeMenu: isMac() && realTauri(), typing });
    if (!s) return;
    if (!reload) e.preventDefault();
    this.runAction(s.action);
  }

  runAction(action: ShellAction): void {
    if (action === 'find') {
      // macOS: the toolbar's field; Windows: the title bar's (#62).
      const input = (document.getElementById('shell-search') as HTMLInputElement | null) ?? chromeSearch();
      input?.focus();
      input?.select();
    } else if (action === 'next_pane' || action === 'prev_pane') {
      this.cyclePane(action === 'next_pane' ? 1 : -1);
    } else {
      this.menuHandlers()[action]?.();
    }
  }

  private async renderMain(navigated: boolean): Promise<void> {
    if (this.firstRun) return;
    const token = ++this.renderToken;
    // The screen registers its own again on every render.
    this.listeners.clear();
    this.queryListeners.clear();
    const scroll = this.main.scrollTop;

    let content: HTMLElement;
    if (this.conn.state !== 'connected' && !OFFLINE_OK.includes(this.route.name)) {
      content = this.paneWithoutService();
    } else {
      if (navigated) this.main.replaceChildren(loading());
      try {
        content = await this.screen();
      } catch (e) {
        console.error(e);
        // A service error says what went wrong; anything else is a bug: logged, not shown raw.
        const shown = isUiError(e) ? e : { kind: 'screen', message: String(e) };
        content = h('div', { class: 'screen' }, errorBanner(shown, () => this.rerender()));
      }
    }
    if (token !== this.renderToken) return; // a newer render won
    // Read focus only now: a sheet may have closed while the screen loaded and handed focus back to the old content.
    const focusKey = (document.activeElement as HTMLElement | null)?.closest<HTMLElement>('[data-key]')?.dataset.key;
    const focusInMain = this.main.contains(document.activeElement);
    this.main.replaceChildren(content);
    if (navigated) {
      this.main.scrollTop = 0;
      this.main.querySelector<HTMLElement>('h1')?.focus();
    } else {
      this.main.scrollTop = scroll;
      const again = focusKey ? document.querySelector<HTMLElement>(`[data-key="${CSS.escape(focusKey)}"]`) : null;
      if (again) again.focus();
      else if (focusInMain) this.main.querySelector<HTMLElement>('h1')?.focus();
    }
  }

  private screen(): Promise<HTMLElement> | HTMLElement {
    const r = this.route;
    switch (r.name) {
      case 'pairs':
        return pairsScreen(this);
      case 'pair':
        return pairDetailScreen(this, r.id);
      case 'add':
        return pairsScreen(this); // a sheet now (go() opens it); kept so old links land somewhere
      case 'preview':
        return previewScreen(this, r.id);
      case 'problems':
        return problemsScreen(this);
      case 'history':
        return historyScreen(this);
      case 'run':
        // A sync from Pair detail: selected in History's list, its details beside it (#67).
        return historyScreen(this, r.id);
      case 'bin':
        return binScreen(this);
      case 'agents':
        return agentsScreen(this);
      case 'devices':
        return devicesScreen(this);
      case 'views':
        return viewsScreen(this);
      case 'addView':
        return viewsScreen(this); // a sheet now (go() opens it)
      case 'settings':
        return settingsScreen(this);
    }
  }

  /**
   * The detail pane while the service isn't connected: connecting says so; stopped or starting leaves the words and
   * the fix to the banner (one banner, D8) and keeps the pane quiet.
   */
  private paneWithoutService(): HTMLElement {
    const c = this.conn;
    const title = h('h1', { tabindex: '-1', class: 'sr-title' }, this.titles().title);
    if (c.state === 'connecting' || c.state === 'connected') {
      return h('section', { class: 'conn-panel', 'aria-busy': 'true' }, title, loading(t('conn.connecting')));
    }
    return h('section', { class: 'conn-panel' }, title, h('p', { class: 'kf-muted' }, t('alert.pane_stopped')));
  }

  // ------------------------------------------------------------ the first run (#70)

  private renderFirstRun(focusTitle: boolean): void {
    if (!this.firstRun) return;
    document.title = t('first.window_title');
    // Windows: the title bar's search has nothing to search yet.
    document.documentElement.classList.add('wasl-first-run');
    const root = document.getElementById('app')!;
    root.replaceChildren(
      firstRunView(this, this.firstRun, (f) => this.renderFirstRun(f)),
      this.live,
      h('div', { id: 'toast', class: 'toast-region', role: 'status', 'aria-live': 'polite' }),
    );
    if (focusTitle) document.getElementById('first-run-title')?.focus();
  }

  /** Leaves the first run for the window: New pair with the folder chosen, or the empty All pairs (skipped). */
  finishFirstRun(next: { folder: string | null } | null): void {
    if (!this.firstRun) return;
    this.firstRun = null;
    document.documentElement.classList.remove('wasl-first-run');
    writeLocal(FIRST_RUN_KEY, 'done');
    this.route = { name: 'pairs' };
    this.buildShell();
    void this.renderMain(true);
    if (next) void this.newPair(next.folder ?? undefined);
  }

  // ------------------------------------------------------------ the daemon

  private onConn(conn: Conn): void {
    const was = this.conn.state;
    this.conn = conn;
    if (conn.state !== 'connected') this.status = null;
    this.renderChrome();
    if (was !== conn.state) {
      this.connSeq++;
      if (conn.state === 'connected') {
        void this.adoptDaemonLanguage();
        void this.readStatus();
      }
      void this.renderMain(false);
    }
    this.pushTray();
    this.pushMenu();
  }

  /** The daemon's stored language wins over this window's cached one. */
  private async adoptDaemonLanguage(): Promise<void> {
    const pref = await api.setting('language').catch(() => null);
    if (pref && pref !== this.langPref) {
      this.langPref = pref;
      shareSetting('language', pref);
      setLang(resolveLang(pref, navigator.language));
      this.buildShell();
      void this.renderMain(false);
      this.pushTray();
      this.pushMenu();
    }
  }

  /** A new person (no account, no pair) gets the first run once, decided by the first full status read while
   *  connected; a status from before the connection, or without accounts and pairs, leaves it for the next (#108). */
  private checkFirstRun(): void {
    if (this.firstRunChecked) return;
    const decision = firstRunDecision(this.status, this.conn.state === 'connected', readLocal(FIRST_RUN_KEY) === 'done');
    if (decision === 'wait') return;
    this.firstRunChecked = true;
    if (decision === 'show' && this.status) {
      this.firstRun = newFirstRun(firstRunStart(this.status));
      this.renderFirstRun(true);
    }
    const pending = this.pendingFolder;
    this.pendingFolder = null;
    if (pending) this.takeFolder(pending);
  }

  /** Reads the status once on connecting: the service's first status can be sent before this window listens for
   *  it (the window loads after the app connects), and the next one may be half a minute away (#108). */
  private async readStatus(): Promise<void> {
    const seq = this.connSeq;
    const current = () => this.connSeq === seq && this.conn.state === 'connected';
    // A service still starting may not answer yet: try again soon, not at the next quiet refresh.
    for (let wait = 250; current() && !(this.status && this.firstRunChecked); wait = Math.min(wait * 2, 4000)) {
      const status = await api.status().catch(() => null);
      if (!current()) return;
      if (status) {
        // A status heard meanwhile is newer than this one: keep it, and only let it decide the first run.
        if (!this.status) this.onStatus(status);
        else this.checkFirstRun();
        return;
      }
      await new Promise((r) => setTimeout(r, wait));
    }
  }

  private onStatus(status: DaemonStatus): void {
    this.status = status;
    this.checkFirstRun();
    this.announceChanges(status);
    this.renderChrome();
    if (LIVE[this.route.name].includes('status')) this.rerender();
    this.pushTray();
    this.pushMenu();
  }

  private onDaemonEvent(e: DaemonEvent): void {
    if (e.event === 'progress') this.patchProgress(e.progress);
    signIn.event(e);
    // The screen hears the event before it re-reads, so it can drop what it kept (Pair detail's devices).
    for (const fn of [...this.listeners]) fn(e);
    if (e.event === 'changed' && LIVE[this.route.name].includes('changed')) this.rerender();
  }

  /** Moves a pair's progress bars (Pair detail and the inspector can both show one) without re-rendering. */
  private patchProgress(p: Progress): void {
    const f = progressFraction(p);
    for (const box of document.querySelectorAll<HTMLElement>(`[data-progress-for="${CSS.escape(p.pair_id)}"]`)) {
      const bar = box.querySelector<HTMLElement>('[role="progressbar"]');
      const fill = box.querySelector<HTMLElement>('.kf-progress > span');
      const text = box.querySelector<HTMLElement>('.progress-text');
      if (bar && fill) {
        if (f === null) {
          bar.removeAttribute('aria-valuenow');
          fill.style.inlineSize = '30%';
          bar.classList.add('indeterminate');
        } else {
          bar.setAttribute('aria-valuenow', String(Math.round(f * 100)));
          fill.style.inlineSize = `${Math.round(f * 100)}%`;
          bar.classList.remove('indeterminate');
        }
      }
      if (text) text.textContent = progressText(p, lang());
    }
    this.patchGlyphs(p);
  }

  /** "Syncing 34%" on the pair's glyphs and its sidebar row, in place: no re-render, no lost focus. */
  private patchGlyphs(p: Progress): void {
    const s = this.status?.pairs.find((x) => x.pair.id === p.pair_id);
    if (!s) return;
    s.progress = p;
    const word = statusLabel(pairStatus(s), lang());
    for (const g of document.querySelectorAll<HTMLElement>(`[data-status-for="${CSS.escape(p.pair_id)}"]`)) {
      const label = g.querySelector('.status-word');
      if (label) label.textContent = word;
      else {
        g.setAttribute('aria-label', word);
        g.title = word;
      }
    }
    this.nav.querySelector(`[data-pair="${CSS.escape(p.pair_id)}"]`)?.setAttribute('aria-label', pairRowName(s));
  }

  /** Tells a screen reader, politely and at most every few seconds, which pairs changed state. */
  private announceChanges(status: DaemonStatus): void {
    const changes = stateChanges(this.lastStates, status.pairs, lang());
    this.lastStates = new Map(status.pairs.map((p) => [p.pair.id, pairStatus(p).state]));
    if (!changes.length) return;
    this.pendingAnnounce = [...this.pendingAnnounce.filter((c) => !changes.some((n) => n.split(', ')[0] === c.split(', ')[0])), ...changes];
    if (this.announceTimer) return;
    const wait = Math.max(0, this.lastAnnounce + ANNOUNCE_MS - Date.now());
    this.announceTimer = setTimeout(() => {
      this.announceTimer = undefined;
      this.lastAnnounce = Date.now();
      this.live.textContent = this.pendingAnnounce.join('. ');
      this.pendingAnnounce = [];
    }, wait);
  }

  /** The tray's words follow the window's language: its tooltip and its right-click menu (#69). */
  private pushTray(): void {
    const s = this.status;
    let state: string;
    if (this.conn.state === 'connected') state = s ? overallLine(s, lang())?.word ?? t('overall.idle') : t('conn.connecting');
    else if (this.conn.state === 'starting') state = t('conn.starting');
    else if (this.conn.state === 'connecting') state = t('conn.connecting');
    else state = t('conn.not_running_title');
    const mac = isMac();
    void api
      .setTrayText({
        notify: notifyText(),
        tooltip: t('tray.tooltip', { state }),
        open: t('tray.open'),
        pauseAll: t('tray.pause_all'),
        syncNow: t('tray.sync_now'),
        settings: t(mac ? 'tray.settings_mac' : 'tray.settings'),
        quit: t(mac ? 'tray.quit' : 'tray.exit'),
      })
      .catch(() => undefined);
  }

  /** The tray's popover or menu asked for a screen (`tray://open`, #69); Settings goes where the menu bar's goes. */
  private onTrayOpen(route: string): void {
    if (route === 'settings') return this.onMenuAction('settings');
    const r = trayRoute(route);
    if (r) this.go(r);
  }

  // ------------------------------------------------------------ the macOS menu bar (#61)

  private lastMenu = '';

  /** What each menu item does in this window; items without an entry stay disabled. The keyboard runs the same
   *  handlers on Windows and in a browser (src/shell/keys.ts). */
  private menuHandlers(): Partial<Record<MenuAction, () => void>> {
    const connected = this.conn.state === 'connected';
    const handlers: Partial<Record<MenuAction, () => void>> = {
      // macOS: the separate Settings window (D5, #68); Windows: the page pinned at the foot of the navigation.
      settings: () => this.openSettings(),
      new_pair: () => void this.newPair(),
      toggle_sidebar: () => this.toggleSidebar(),
      toggle_inspector: () => this.toggleInspector(),
    };
    for (const id of Object.keys(GO_ROUTES) as Array<keyof typeof GO_ROUTES>) {
      const name = GO_ROUTES[id];
      handlers[id] = () => this.go({ name });
    }
    const targets = connected ? this.targets() : [];
    if (targets.length > 0) {
      // Sync Now and Pause act on the selected pairs, or every pair with nothing selected.
      handlers.sync_now = () => this.syncNow();
      const allPaused = targets.every((p) => p.pair.paused);
      handlers.toggle_pause = () => void this.setPaused(targets, !allPaused);
    }
    const sel = connected ? this.selectedPairs() : [];
    if (sel.length === 1) handlers.show_in_finder = () => this.showInFolder(sel[0]!.pair.id);
    return handlers;
  }

  /** Menu words that follow the window's state: Hide or Show, Pause or Resume. */
  private menuWords(): Record<string, string> {
    const targets = this.conn.state === 'connected' ? this.targets() : [];
    return {
      toggle_sidebar: t(this.sidebarVisible() ? 'menu.toggle_sidebar' : 'menu.show_sidebar'),
      toggle_inspector: t(this.inspectorOpen ? 'menu.hide_inspector' : 'menu.toggle_inspector'),
      toggle_pause: t(targets.length > 0 && targets.every((p) => p.pair.paused) ? 'menu.resume' : 'menu.toggle_pause'),
    };
  }

  private onMenuAction(id: string): void {
    if (!isMenuAction(id)) return;
    // A modal sheet or dialog has the window to itself, as it has the keyboard (onKey): no ⌘1–7, Sync Now or Pause
    // behind it, and no second sheet on top. Settings opens its own window on macOS, so it still works.
    if (id !== 'settings' && document.querySelector('dialog[open]')) return;
    // A menu command during the first run leaves it for the window it acts on.
    if (this.firstRun) this.finishFirstRun(null);
    this.menuHandlers()[id]?.();
  }

  /** The menu bar's words follow the window's language; its items follow what the window can do now. */
  private pushMenu(): void {
    if (!isMac()) return;
    const text = menuText(Object.keys(this.menuHandlers()) as MenuAction[], this.menuWords());
    const key = JSON.stringify(text);
    if (key === this.lastMenu) return;
    this.lastMenu = key;
    void api.setMenuText(text).catch(() => {
      this.lastMenu = '';
    });
  }

  /** "5 minutes ago" in the current language. */
  ago(ms: number): string {
    return formatRelative(ms, Date.now(), lang());
  }
}

function isMac(): boolean {
  return document.documentElement.dataset.kfPlatform === 'mac';
}

/** The Windows title bar's search box (src/chrome/windows.ts), when the window draws one. */
function chromeSearch(): HTMLInputElement | null {
  return document.querySelector<HTMLInputElement>('input[data-chrome-search]');
}

/** Back: Esc on its own, ⌘[ on macOS, Alt+Left on Windows (mirrored to Alt+Right in Arabic). */
function isBack(e: KeyboardEvent): boolean {
  if (e.key === 'Escape') return !e.altKey && !e.ctrlKey && !e.metaKey && !e.shiftKey;
  if (isMac()) return e.metaKey && !e.altKey && !e.ctrlKey && e.code === 'BracketLeft';
  const back = document.documentElement.dir === 'rtl' ? 'ArrowRight' : 'ArrowLeft';
  return e.altKey && !e.ctrlKey && !e.metaKey && !e.shiftKey && e.key === back;
}

function platform(): Platform {
  return isMac() ? 'mac' : platformOf('');
}

/** A real Tauri window, not the dev mock standing in for one. */
function realTauri(): boolean {
  return '__TAURI_INTERNALS__' in window && !('__WASL_MOCK__' in window);
}

function readLocal(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

/** Marks the root with how the person last moved: 'pointer' or 'keyboard'. A heading focused by script after a
 *  click (tabindex="-1") then shows no ring: WebKit matches :focus-visible for script focus even after a mouse
 *  click, because a click on a button doesn't focus it on macOS (#108). */
function trackInputModality(): void {
  const root = document.documentElement;
  const set = (m: 'pointer' | 'keyboard') => {
    if (root.dataset.input !== m) root.dataset.input = m;
  };
  document.addEventListener('pointerdown', () => set('pointer'), true);
  document.addEventListener(
    'keydown',
    (e) => {
      if (!['Shift', 'Control', 'Alt', 'Meta'].includes(e.key)) set('keyboard');
    },
    true,
  );
}

function writeLocal(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Private storage unavailable: the daemon's setting still holds it.
  }
}
