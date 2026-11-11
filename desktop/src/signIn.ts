// Adding a Google account from Settings and Problems (#147 M4): one sign-in at a time, with the same waiting state
// as the New pair sheet's drive picker (screens/drivePicker.ts): the buttons are disabled and the browser link is
// shown until the account arrives or fails. The state lives here, outside any screen, so a re-render keeps it, and
// each window feeds it every daemon event. The daemon runs one sign-in at a time (wasld `add_account`): a second
// call, from any window or app, gets the same address, so the wait ends on the daemon's own answer.

import { api } from './api';
import { errorText, h, toast } from './ui';
import { t } from './i18n';
import type { DaemonEvent } from './types';

export class SignIn {
  waiting = false;
  /** The sign-in address, for when the browser didn't open. */
  url = '';
  private subs = new Set<() => void>();
  private attempt = 0;

  constructor(
    private readonly add: () => Promise<string>,
    private readonly onError: (e: unknown) => void,
  ) {}

  subscribe(fn: () => void): void {
    this.subs.add(fn);
  }

  /** Starts a sign-in unless one is already waiting. */
  async start(): Promise<void> {
    if (this.waiting) return;
    this.waiting = true;
    this.url = '';
    const attempt = ++this.attempt;
    this.emit();
    // The account can arrive (or fail) before the address does: that wait is over and keeps no address (#205).
    const current = () => this.waiting && attempt === this.attempt;
    try {
      const url = await this.add();
      if (!current()) return;
      this.url = url;
    } catch (e) {
      if (!current()) return;
      this.end();
      this.onError(e);
      return;
    }
    this.emit();
  }

  /** A daemon event: the account arrived or failed, so the wait is over. */
  event(e: DaemonEvent): void {
    if (e.event === 'account_added' || e.event === 'account_failed') this.end();
  }

  private end(): void {
    if (!this.waiting) return;
    this.waiting = false;
    this.url = '';
    this.emit();
  }

  private emit(): void {
    this.subs.forEach((fn) => fn());
  }
}

/** This window's sign-in. */
export const signIn = new SignIn(() => api.addAccount(), (e) => toast(errorText(e)));

signIn.subscribe(() => {
  if (typeof document === 'undefined') return;
  document.querySelectorAll<HTMLButtonElement>('[data-sign-in]').forEach((b) => (b.disabled = signIn.waiting));
  document.querySelectorAll<HTMLElement>('[data-sign-in-wait]').forEach((el) => el.replaceChildren(...waitingChildren()));
});

/** Marks a button as starting a sign-in: disabled while one waits. */
export function signInButton<T extends HTMLButtonElement>(b: T): T {
  b.dataset.signIn = '';
  b.disabled = signIn.waiting;
  return b;
}

/** Where the waiting line and the browser link go; empty while nothing waits. */
export function signInWaiting(): HTMLElement {
  return h('div', { 'data-sign-in-wait': '' }, ...waitingChildren());
}

function waitingChildren(): HTMLElement[] {
  if (!signIn.waiting) return [];
  return [
    h(
      'div',
      { class: 'waiting', role: 'status' },
      h('p', null, h('span', { class: 'spinner', 'aria-hidden': 'true' }), t('add.waiting_google')),
      signIn.url
        ? h('label', { class: 'kf-field' }, h('span', { class: 'kf-field-help' }, t('add.signin_link')),
            h('span', { class: 'kf-field-box' }, h('input', { type: 'text', readonly: true, value: signIn.url, dir: 'ltr' })))
        : null,
    ),
  ];
}
