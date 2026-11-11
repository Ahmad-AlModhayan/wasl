// The tray's popover (macOS) and flyout (Windows), design D6 (#69): its own small window (`tray`, made by
// src-tauri/src/popover.rs) showing this page. It leads with the overall state, then the syncs in flight, what needs
// you, the last few runs, and Pause all / Open Wasl; Sync now and More (the tray's native menu: Settings…, Quit) sit
// by the state. Esc closes it; Tab cycles inside it. It says how tall it is so the window fits it.

import './tray.css';
import { listen } from '@tauri-apps/api/event';
import { api } from '../api';
import { progressFraction, progressText } from '../format';
import { lang, resolveLang, setLang, t } from '../i18n';
import { h, line, mark, statusGlyph } from '../ui';
import type { Conn, DaemonEvent, DaemonStatus, PairState, Progress, RunRecord } from '../types';
import { allPaused, headline, inFlight, moreInProblems, needsYou, recentRuns, type Headline, type NeedItem } from './model';

/** Where the main window keeps the language choice (app.ts LANG_KEY); both windows share storage. */
const LANG_KEY = 'wasl.language';
/** How many "needs you" rows before the rest fold into "N more in Problems". */
const MAX_NEEDS = 3;

function readLang(): void {
  let pref: string | null = null;
  try {
    pref = localStorage.getItem(LANG_KEY);
  } catch {
    // Storage blocked: the system language.
  }
  setLang(resolveLang(pref ?? 'system', navigator.language));
}

class TrayPage {
  private conn: Conn = { state: 'connecting' };
  private status: DaemonStatus | null = null;
  private runs: RunRecord[] = [];
  private lastHeight = 0;
  // The header stays in the page across renders (only its words change), so a status push doesn't take focus off
  // the state, Sync now or More, and a screen reader doesn't read the state again.
  private readonly title = h('h1', { id: 'tray-title', class: 'tray-title', tabindex: '-1' });
  private readonly detail = h('p', { id: 'tray-detail', class: 'tray-detail' });
  private readonly syncButton = h(
    'button',
    { type: 'button', class: 'tray-icon-btn', 'data-key': 'sync', onclick: () => void api.syncAll().catch(() => undefined) },
    line('sync'),
  );
  private readonly moreButton = h(
    'button',
    { type: 'button', class: 'tray-icon-btn', 'aria-haspopup': 'menu', 'data-key': 'more', onclick: () => this.openMenu() },
    line('gear'),
  );
  private readonly header = h(
    'header',
    { class: 'tray-head' },
    h('span', { class: 'tray-mark' }, mark(20)),
    h('div', { class: 'tray-head-text' }, this.title, this.detail),
    this.syncButton,
    this.moreButton,
  );
  /** What the title shows now, so it is redrawn only when that changes. */
  private titleKey = '';
  private readonly root = h('div', { class: 'tray', role: 'dialog', 'aria-labelledby': 'tray-title', 'aria-describedby': 'tray-detail' }, this.header);

  async start(): Promise<void> {
    document.documentElement.classList.add('tray-page');
    readLang();
    document.body.replaceChildren(this.root);
    document.addEventListener('keydown', (e) => this.onKey(e));
    await listen<Conn>('daemon://state', (e) => {
      this.conn = e.payload;
      this.render();
    });
    await listen<DaemonStatus>('daemon://status', (e) => {
      this.status = e.payload;
      this.render();
    });
    await listen<DaemonEvent>('daemon://event', (e) => this.onEvent(e.payload));
    // Each opening: the language may have changed in the main window, and the runs since.
    await listen('tray://shown', () => void this.shown());
    this.conn = await api.daemonState().catch(() => this.conn);
    await this.reload();
    // Measured only once the content is in: the first height shows the window (popover.rs), so it never opens empty.
    new ResizeObserver(() => this.fit()).observe(this.root);
    this.focusTitle();
  }

  private async shown(): Promise<void> {
    readLang();
    await this.reload();
    this.focusTitle();
  }

  private async reload(): Promise<void> {
    if (this.conn.state === 'connected') {
      [this.status, this.runs] = await Promise.all([
        api.status().catch(() => this.status),
        api.runs(null, 20).catch(() => this.runs),
      ]);
    }
    this.render();
  }

  private onEvent(ev: DaemonEvent): void {
    if (ev.event === 'progress') this.patchProgress(ev.progress);
    else if (ev.event === 'run_finished') void this.reload();
  }

  /** Focus starts on the state each time the popover opens, so VoiceOver and Narrator read it first; Tab moves on
   * to the actions. */
  private focusTitle(): void {
    this.root.querySelector<HTMLElement>('#tray-title')?.focus();
  }

  private onKey(e: KeyboardEvent): void {
    if (e.key === 'Escape') {
      e.preventDefault();
      void api.trayHide();
      return;
    }
    if (e.key !== 'Tab') return;
    // Tab cycles inside the popover: it is its own window, with nothing before or after it.
    const items = [...this.root.querySelectorAll<HTMLElement>('button:not([disabled])')];
    if (items.length === 0) return;
    const i = items.indexOf(document.activeElement as HTMLElement);
    const next = e.shiftKey ? (i <= 0 ? items.length - 1 : i - 1) : i === -1 || i === items.length - 1 ? 0 : i + 1;
    items[next]!.focus();
    e.preventDefault();
  }

  /** Tells the window how tall the page is, so it fits without scrolling (up to the screen's height). */
  private fit(): void {
    const height = Math.ceil(this.root.getBoundingClientRect().height);
    if (height === this.lastHeight || height === 0) return;
    this.lastHeight = height;
    void api.trayFit(height).catch(() => undefined);
  }

  private render(): void {
    const l = lang();
    const active = document.activeElement as HTMLElement | null;
    const hadFocus = !!active && active !== this.root && this.root.contains(active);
    const focused = active?.dataset.key;
    const connected = this.conn.state === 'connected' && !!this.status;
    const hasPairs = connected && (this.status?.pairs.length ?? 0) > 0;
    this.renderHeader(headline(this.conn, this.status, l), hasPairs);
    // Everything under the header is drawn afresh.
    while (this.header.nextSibling) this.header.nextSibling.remove();
    const parts = [
      ...inFlight(this.status).map((p) => this.flightCard(p)),
      this.needsSection(needsYou(this.status, l)),
      connected ? this.recentSection() : null,
      h(
        'footer',
        { class: 'tray-foot' },
        h(
          'button',
          { type: 'button', class: 'kf-desk-btn', disabled: !hasPairs, 'data-key': 'pause', onclick: () => void this.togglePause() },
          line(allPaused(this.status) ? 'play' : 'pause'),
          t(allPaused(this.status) ? 'tray.resume_all' : 'tray.pause_all'),
        ),
        h('span', { class: 'tray-grow' }),
        h('button', { type: 'button', class: 'kf-desk-btn kf-desk-btn-default', 'data-key': 'open', onclick: () => void api.trayOpen(null) }, t('tray.open')),
      ),
    ];
    this.root.append(...parts.filter((p): p is HTMLElement => p !== null));
    // Focus on the header stays where it was. A redrawn control gets it back; if it is gone (a problem solved, a
    // button disabled), the state takes it rather than the page.
    if (!hadFocus || this.root.contains(document.activeElement)) return;
    const again = focused ? this.root.querySelector<HTMLElement>(`[data-key="${CSS.escape(focused)}"]:not([disabled])`) : null;
    if (again) again.focus();
    else this.focusTitle();
  }

  /** The state, its detail and the two icon buttons, changed in place. */
  private renderHeader(head: Headline, hasPairs: boolean): void {
    const key = [head.view.state, head.view.paused, head.title, lang()].join('|');
    if (key !== this.titleKey) {
      this.titleKey = key;
      this.title.replaceChildren(statusGlyph(head.view), h('span', null, head.title));
    }
    const detail = head.detail ?? '';
    if (this.detail.textContent !== detail) this.detail.textContent = detail;
    for (const [button, label] of [
      [this.syncButton, t('tray.sync_now')],
      [this.moreButton, t('tray.more')],
    ] as const) {
      button.setAttribute('aria-label', label);
      button.title = label;
    }
    this.syncButton.disabled = !hasPairs;
  }

  /** More: the tray's native menu (Settings…, Quit), opened under the button, also when pressed from the keyboard. */
  private openMenu(): void {
    const r = this.moreButton.getBoundingClientRect();
    const rtl = document.documentElement.dir === 'rtl';
    void api.trayMenu(rtl ? r.right : r.left, r.bottom).catch(() => undefined);
  }

  private flightCard(p: PairState): HTMLElement {
    const f = p.progress ? progressFraction(p.progress) : null;
    return h(
      'section',
      { class: 'tray-card', 'aria-label': p.pair.name, 'data-progress-for': p.pair.id },
      h(
        'div',
        { class: 'tray-row' },
        h('b', { class: 'tray-grow tray-ell' }, p.pair.name),
        h('span', { class: 'tray-num tray-mut', 'data-part': 'percent' }, f === null ? '' : `${Math.round(f * 100)}%`),
      ),
      h('div', { class: 'tray-bar', 'aria-hidden': 'true' }, h('i', { style: `inline-size:${f === null ? 30 : Math.round(f * 100)}%` })),
      h('p', { class: 'tray-mut tray-small tray-ell', 'data-part': 'text' }, p.progress ? progressText(p.progress, lang()) : ''),
    );
  }

  /** Live progress, in place: no re-render, so focus and the screen reader's place stay put. */
  private patchProgress(pr: Progress): void {
    const pair = this.status?.pairs.find((p) => p.pair.id === pr.pair_id);
    if (pair) pair.progress = pr;
    const card = this.root.querySelector<HTMLElement>(`[data-progress-for="${CSS.escape(pr.pair_id)}"]`);
    if (!card) return;
    const f = progressFraction(pr);
    const percent = card.querySelector('[data-part="percent"]');
    if (percent) percent.textContent = f === null ? '' : `${Math.round(f * 100)}%`;
    const bar = card.querySelector<HTMLElement>('.tray-bar > i');
    if (bar) bar.style.inlineSize = `${f === null ? 30 : Math.round(f * 100)}%`;
    const text = card.querySelector('[data-part="text"]');
    if (text) text.textContent = progressText(pr, lang());
  }

  private needsSection(items: NeedItem[]): HTMLElement | null {
    if (items.length === 0) return null;
    const shown = items.slice(0, MAX_NEEDS);
    const rest = moreInProblems(this.status, items, shown.length);
    return h(
      'section',
      { class: 'tray-section', 'aria-labelledby': 'tray-need' },
      h('h2', { id: 'tray-need', class: 'tray-label' }, t('tray.need_you')),
      h(
        'ul',
        { class: 'tray-needs', role: 'list' },
        shown.map((item) =>
          h(
            'li',
            null,
            h(
              'button',
              { type: 'button', class: `tray-need status-${item.view.state}`, 'data-key': item.key, onclick: () => void api.trayOpen(item.route) },
              statusGlyph(item.view),
              h('span', { class: 'tray-two tray-grow' }, h('b', { class: 'tray-ell' }, item.title), h('span', { class: 'tray-ell' }, item.detail)),
              line('forward', { mirror: true }),
            ),
          ),
        ),
      ),
      rest > 0
        ? h('button', { type: 'button', class: 'tray-link', 'data-key': 'more-problems', onclick: () => void api.trayOpen('problems') }, t('tray.more_problems', { count: rest }))
        : null,
    );
  }

  private recentSection(): HTMLElement {
    const runs = recentRuns(this.runs, this.status, Date.now(), lang());
    return h(
      'section',
      { class: 'tray-section', 'aria-labelledby': 'tray-recent' },
      h('h2', { id: 'tray-recent', class: 'tray-label' }, t('tray.recent')),
      runs.length === 0
        ? h('p', { class: 'tray-mut tray-small' }, t('history.empty_title'))
        : h(
            'ul',
            { class: 'tray-runs', role: 'list' },
            runs.map((r) =>
              h(
                'li',
                { class: 'tray-row' },
                statusGlyph(r.view),
                h('span', { class: 'tray-grow tray-ell' }, h('span', null, r.name), h('span', { class: 'tray-mut' }, ` · ${r.outcome}`)),
                h('span', { class: 'tray-mut tray-small tray-num' }, r.when),
              ),
            ),
          ),
    );
  }

  private async togglePause(): Promise<void> {
    const s = this.status;
    if (!s) return;
    const resume = allPaused(s);
    for (const p of s.pairs) {
      if (p.pair.paused === resume) await api.setPaused(p.pair.id, !resume).catch(() => undefined);
    }
    await this.reload();
  }
}

/** Mounts the popover page instead of the main window's shell. */
export async function mountTray(): Promise<void> {
  await new TrayPage().start();
}
