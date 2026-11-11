// First run (#70, design S.onboarding): for a new person, with no Google account and no pair. A compact two-column
// layout fills the window: the numbered steps stay visible on the leading side, one step at a time on the other.
// Sign in to Google Drive (wasld's AddAccount opens the browser), choose a first folder, choose whether Wasl stays
// in the menu bar / notification area (#71, the Settings › General switch), then New pair's sheet
// (#65) takes over with the folder filled in and its default button, "Preview first sync", is the last step.
// "Skip for now" lands on the empty All pairs. Keyboard alone: Tab reaches every button, Return presses the
// default one, and each step puts the focus on its heading.
//
// Not here, because the service doesn't report them yet: naming this computer (wasld's `device_name` setting
// only names conflict copies; pairing uses the service's own name) and suggested folders with what they'd skip.

import { open } from '@tauri-apps/plugin-dialog';
import { api } from '../api';
import { t } from '../i18n';
import { FIRST_RUN_STEPS, firstRunStepKey, type FirstRunStep } from '../appState';
import { settingOn, TRAY_KEY, trayRow } from './settings';
import { deskButton, errorText, h, line, mark } from '../ui';

/** What the first run keeps across re-renders (a language change rebuilds it). */
export interface FirstRunState {
  step: FirstRunStep;
  folder: string | null;
  /** The account signed in during this run. */
  email: string | null;
  busy: boolean;
  error: string | null;
  /** Whether Wasl shows in the menu bar / notification area; null until read from the service. */
  tray: boolean | null;
}

export interface FirstRunHost {
  /** Leaves the first run: on to New pair with the folder chosen, or (null) skipped to the empty All pairs. */
  finishFirstRun(next: { folder: string | null } | null): void;
}

const isMac = () => document.documentElement.dataset.kfPlatform === 'mac';

export function newFirstRun(step: FirstRunStep): FirstRunState {
  return { step, folder: null, email: null, busy: false, error: null, tray: null };
}

/** The first run's whole window; `rerender` draws it again from `state`. */
export function firstRunView(host: FirstRunHost, state: FirstRunState, rerender: (focusTitle: boolean) => void): HTMLElement {
  const go = (step: FirstRunStep) => {
    state.step = step;
    state.error = null;
    rerender(true);
  };

  const signIn = async () => {
    if (state.busy) return;
    state.busy = true;
    state.error = null;
    rerender(false);
    try {
      const id = await api.addAccount();
      const accounts = await api.accounts().catch(() => []);
      state.email = accounts.find((a) => a.id === id)?.email ?? id;
      state.busy = false;
      go('folder');
    } catch (e) {
      state.busy = false;
      state.error = errorText(e);
      rerender(false);
    }
  };

  const chooseFolder = async () => {
    const picked = await open({ directory: true, multiple: false }).catch(() => null);
    if (typeof picked === 'string') {
      state.folder = picked;
      rerender(false);
      document.querySelector<HTMLElement>('[data-key="first-continue"]')?.focus();
    }
  };

  // The step list: numbered, done ones ticked; the current one is the page's step for a screen reader.
  const current = FIRST_RUN_STEPS.indexOf(state.step);
  const steps = h(
    'ol',
    { class: 'first-run-steps', 'aria-label': t('first.steps') },
    FIRST_RUN_STEPS.map((s, i) =>
      h(
        'li',
        { class: `first-run-step${i < current ? ' done' : i === current ? ' cur' : ''}`, 'aria-current': i === current ? 'step' : undefined },
        h('b', { 'aria-hidden': 'true' }, i < current ? '✓' : String(i + 1)),
        h('span', null, t(firstRunStepKey(s, isMac() ? 'mac' : 'windows'))),
        i < current ? h('span', { class: 'sr-title' }, t('first.step_done')) : null,
      ),
    ),
  );

  const side = h(
    'div',
    { class: 'first-run-side', 'data-tauri-drag-region': isMac() ? '' : undefined },
    h('div', { class: 'first-run-brand' }, mark(22), h('b', null, t('app.name'))),
    steps,
    h('p', { class: 'first-run-note' }, t('first.note')),
  );

  let title: string;
  let lead: string;
  let content: HTMLElement | null;
  let primary: HTMLButtonElement;
  let back: HTMLButtonElement | null = null;
  const next = t(isMac() ? 'first.continue' : 'first.next');

  if (state.step === 'sign_in') {
    title = t('first.sign_in_title');
    lead = t('first.sign_in_body');
    content = h(
      'div',
      { class: 'first-run-box' },
      line('cloud', { size: 36 }),
      h('b', null, 'Google Drive'),
      state.email
        ? h('p', { class: 'first-run-done' }, line('synced', { size: 20 }), h('span', null, t('first.signed_in')), h('bdi', { dir: 'ltr' }, state.email))
        : h('p', { class: 'first-run-muted' }, t('first.not_signed_in')),
    );
    primary = state.email
      ? deskButton(next, () => go('folder'), { primary: true, key: 'first-continue' })
      : deskButton(t(state.busy ? 'first.signing_in' : 'first.sign_in'), () => void signIn(), { primary: true, key: 'first-continue' });
    if (state.busy) primary.setAttribute('aria-busy', 'true');
  } else if (state.step === 'folder') {
    title = t('first.folder_title');
    lead = t('first.folder_body');
    content = h(
      'div',
      { class: 'first-run-box' },
      line('folder', { size: 36 }),
      state.folder
        ? h('p', { class: 'first-run-path' }, h('bdi', { dir: 'ltr' }, state.folder))
        : h('p', { class: 'first-run-muted' }, t(isMac() ? 'first.folder_none_mac' : 'first.folder_none_win')),
      deskButton(t(state.folder ? 'first.folder_change' : 'first.folder_choose'), () => void chooseFolder(), { key: 'first-choose' }),
    );
    back = deskButton(t('first.back'), () => go('sign_in'), { key: 'first-back' });
    primary = deskButton(next, () => go('tray'), { primary: true, key: 'first-continue' });
  } else {
    // Menu bar / notification area: the same switch as Settings › General, applied at once. It reads the stored
    // choice once; until then (or if the service can't say) it shows on, Wasl's default.
    if (state.tray === null) {
      state.tray = true;
      void api.setting(TRAY_KEY).then(
        (v) => {
          const on = settingOn(v);
          if (on !== state.tray) {
            state.tray = on;
            rerender(false);
          }
        },
        () => {},
      );
    }
    title = t(isMac() ? 'first.step_tray_mac' : 'first.step_tray_win');
    lead = t(isMac() ? 'first.tray_body_mac' : 'first.tray_body_win');
    content = h(
      'div',
      { class: 'settings-box first-run-settings' },
      trayRow({ on: state.tray, change: (on) => { state.tray = on; } }),
    );
    back = deskButton(t('first.back'), () => go('folder'), { key: 'first-back' });
    primary = deskButton(next, () => host.finishFirstRun({ folder: state.folder }), { primary: true, key: 'first-continue' });
  }

  const pane = h(
    'section',
    { class: 'first-run-pane', 'aria-labelledby': 'first-run-title' },
    h(
      'div',
      { class: 'first-run-body' },
      h('h1', { id: 'first-run-title', tabindex: '-1' }, title),
      h('p', { class: 'first-run-muted' }, lead),
      content,
      state.error ? h('p', { class: 'state-callout status-failed', role: 'alert' }, line('failed', { size: 20 }), h('span', { class: 'state-callout-text' }, state.error)) : null,
    ),
    h(
      'div',
      { class: 'first-run-foot' },
      deskButton(t('first.skip'), () => host.finishFirstRun(null), { plain: true, key: 'first-skip' }),
      h('span', { class: 'first-run-grow' }),
      back,
      primary,
    ),
  );

  const root = h('div', { class: 'first-run', 'data-step': state.step }, side, pane);
  // Return presses the default button, as in a native window, unless a button has the focus.
  root.addEventListener('keydown', (e) => {
    if (e.key !== 'Enter' || e.defaultPrevented || e.isComposing) return;
    if ((e.target as HTMLElement).closest('button, a, input, select, textarea')) return;
    e.preventDefault();
    primary.click();
  });
  return root;
}
