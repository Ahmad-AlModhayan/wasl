// A small DOM toolkit: h() for elements, icons, an accessible confirm dialog and a toast with an action.

import { LINES, WASL_MARK, type LineName } from './icons';
import { lang, t } from './i18n';
import { statusLabel, type StatusView } from './format';
import { isUiError } from './api';

type Child = Node | string | number | null | undefined | false | Child[];
type Props = Record<string, unknown> | null | undefined;

/** Creates an element. Props starting with "on" are listeners; class/aria/data/role and others are attributes;
 *  value/checked/disabled/selected are properties. */
export function h<K extends keyof HTMLElementTagNameMap>(tag: K, props?: Props, ...children: Child[]): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(props ?? {})) {
    if (v === undefined || v === null || v === false) continue;
    if (k.startsWith('on') && typeof v === 'function') {
      el.addEventListener(k.slice(2).toLowerCase(), v as EventListener);
    } else if (k === 'value' || k === 'checked' || k === 'disabled' || k === 'selected') {
      (el as unknown as Record<string, unknown>)[k] = v;
    } else {
      el.setAttribute(k, v === true ? '' : String(v));
    }
  }
  append(el, children);
  return el;
}

/** Replaces an element's children; null and false are skipped. */
export function fill(el: Element, ...children: Child[]): void {
  el.replaceChildren();
  append(el, children);
}

function append(el: Node, children: Child[]): void {
  for (const c of children) {
    if (c === null || c === undefined || c === false) continue;
    if (Array.isArray(c)) append(el, c);
    else el.appendChild(typeof c === 'object' ? c : document.createTextNode(String(c)));
  }
}

const SVG = 'http://www.w3.org/2000/svg';

/** A decorative line icon from the desktop shell's set (24×24, stroked). */
export function line(name: LineName, opts: { size?: number; mirror?: boolean } = {}): SVGSVGElement {
  const size = opts.size ?? 16;
  const svg = document.createElementNS(SVG, 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('width', String(size));
  svg.setAttribute('height', String(size));
  svg.setAttribute('aria-hidden', 'true');
  svg.setAttribute('focusable', 'false');
  svg.setAttribute('class', opts.mirror ? 'line-icon kf-mirror' : 'line-icon');
  svg.innerHTML = LINES[name]; // our own constant markup, not user data
  return svg;
}

/**
 * One of Wasl's six states by glyph (design D8): its shape says the state, its colour only repeats it. With
 * `label`, the word shows beside it; without, the glyph carries the word as its accessible name and tooltip.
 * Syncing turns unless motion is reduced; its word always carries the percentage. `pairId` lets live progress
 * update the word in place.
 */
export function statusGlyph(v: StatusView, opts: { label?: boolean; pairId?: string; size?: number } = {}): HTMLElement {
  const word = statusLabel(v, lang());
  const glyph = line(v.paused ? 'paused' : v.state, { size: opts.size });
  if (v.state === 'syncing') glyph.classList.add('status-spin');
  return h(
    'span',
    {
      class: `status-glyph status-${v.state}`,
      role: opts.label ? undefined : 'img',
      'aria-label': opts.label ? undefined : word,
      title: opts.label ? undefined : word,
      'data-status-for': opts.pairId,
    },
    glyph,
    opts.label ? h('span', { class: 'status-word' }, word) : null,
  );
}

export function mark(size = 28): SVGSVGElement {
  const svg = document.createElementNS(SVG, 'svg');
  svg.setAttribute('viewBox', WASL_MARK.viewBox);
  svg.setAttribute('width', String(size));
  svg.setAttribute('height', String(size));
  svg.setAttribute('aria-hidden', 'true');
  svg.setAttribute('focusable', 'false');
  svg.innerHTML = WASL_MARK.body; // our own constant markup, not user data
  return svg;
}

/** A button. `label` is the accessible name when only an icon shows. */
export function button(
  text: string,
  onclick: (e: MouseEvent) => void,
  opts: { kind?: string; icon?: LineName; mirror?: boolean; key?: string; disabled?: boolean; label?: string; small?: boolean } = {},
): HTMLButtonElement {
  return h(
    'button',
    {
      type: 'button',
      class: `kf-btn ${opts.small === false ? '' : 'kf-btn-sm'} kf-btn-${opts.kind ?? 'gray'}`,
      onclick,
      'data-key': opts.key,
      disabled: opts.disabled,
      'aria-label': opts.label,
    },
    opts.icon ? line(opts.icon, { mirror: opts.mirror }) : null,
    text,
  );
}

/** A status chip with a coloured dot. */
export function chip(text: string, tone: string): HTMLElement {
  return h('span', { class: `kf-status kf-status-compact ${tone}` }, h('span', { class: 'kf-dot', 'aria-hidden': 'true' }), text);
}

/** A desktop push button (kahf-design's `kf-desk-btn`); `primary` is the default button (filled). */
export function deskButton(
  text: string,
  onclick: (e: MouseEvent) => void,
  opts: { primary?: boolean; key?: string; icon?: LineName; plain?: boolean } = {},
): HTMLButtonElement {
  const cls = ['kf-desk-btn', opts.primary ? 'kf-desk-btn-default' : '', opts.plain ? 'kf-desk-btn-plain' : ''].filter(Boolean).join(' ');
  return h('button', { type: 'button', class: cls, onclick, 'data-key': opts.key }, opts.icon ? line(opts.icon) : null, text);
}

/**
 * A pane with nothing in it yet (#70, design S.empty): a line icon, what is missing, what to do, and up to two
 * buttons. The first action is the default one; it goes last, on the trailing side, on both platforms (#124).
 */
export function empty(iconName: LineName, title: string, body: string, ...actions: HTMLElement[]): HTMLElement {
  return h(
    'div',
    { class: 'state-empty' },
    line(iconName, { size: 36 }),
    h('h2', { class: 'state-empty-title' }, title),
    h('p', { class: 'state-empty-body' }, body),
    actions.length ? h('div', { class: 'state-empty-actions' }, [...actions].reverse()) : null,
  );
}

export function loading(text = t('common.loading')): HTMLElement {
  return h('p', { class: 'loading kf-muted', role: 'status' }, h('span', { class: 'spinner', 'aria-hidden': 'true' }), text);
}

/** Words for an error from a command. */
export function errorText(e: unknown): string {
  if (isUiError(e)) {
    const known = t(`errors.${e.kind}`);
    if (known !== `errors.${e.kind}`) return known;
    return t('common.error', { message: e.message });
  }
  return t('common.error', { message: String(e) });
}

/** A screen's own failure (it didn't load, an action failed), inside the pane: never the app-wide banner (D8). */
export function errorBanner(e: unknown, retry?: () => void): HTMLElement {
  return h(
    'div',
    { class: 'state-callout status-failed', role: 'alert' },
    line('failed', { size: 20 }),
    h('p', { class: 'state-callout-text' }, errorText(e)),
    retry ? deskButton(t('common.retry'), retry, { key: 'retry' }) : null,
  );
}

// ---------------------------------------------------------------- sheets (#65)

/** A sheet slides from under the toolbar with the default button last, on the trailing side: the macOS layout,
 *  on both platforms (#124). */
const SHEET_ORDER = ['cancel', 'confirm'] as const;

export interface SheetOptions<T> {
  title: string;
  /** A short line under the title; it describes the sheet to a screen reader. */
  body?: string;
  /** The fields. */
  content?: Child;
  /** The default button's words. */
  confirm: string;
  danger?: boolean;
  cancel?: string;
  /** Wider sheets carry forms (New pair). */
  wide?: boolean;
  /** Runs on the default button or Return. Return undefined to keep the sheet open (a refusal shown inline). */
  submit: () => T | undefined | Promise<T | undefined>;
  /** What Cancel, Esc and closing resolve to. */
  cancelled: T;
  /** Where focus starts: the first field, Cancel (questions about something permanent) or the default button. */
  focus?: 'first' | 'cancel' | 'confirm' | HTMLElement;
}

/** A modal sheet on <dialog>. showModal() makes the rest of the window inert, so focus stays inside; Esc cancels;
 *  focus returns to whatever opened it. Confirm, prompt and choose dialogs are built on it. */
export function sheet<T>(opts: SheetOptions<T>): Promise<T> {
  const before = document.activeElement as HTMLElement | null;
  // A live update may replace the invoker while the sheet is open (the toolbar rebuilds on each status, Pair detail
  // re-renders); its data-key finds the new one.
  const beforeKey = before?.closest<HTMLElement>('[data-key]')?.dataset.key;
  return new Promise((resolve) => {
    const id = `sheet-${Math.random().toString(36).slice(2)}`;
    const dlg = h('dialog', {
      class: `sheet sheet-top${opts.wide ? ' sheet-wide' : ''}`,
      'aria-labelledby': `${id}-title`,
      'aria-describedby': opts.body ? `${id}-body` : undefined,
    });
    let settled = false;
    let busy = false;
    let closedWhileBusy = false;
    const done = (value: T) => {
      if (settled) return;
      settled = true;
      dlg.close();
      dlg.remove();
      returnFocus(before, beforeKey);
      resolve(value);
    };
    dlg.addEventListener('cancel', (e) => {
      e.preventDefault();
      if (!busy) done(opts.cancelled);
    });
    // WebView2's close watcher closes the dialog on a second Esc without a cancel event. Settle then too; while a
    // submit runs, settle with its result once it finishes, so nothing it made is lost.
    dlg.addEventListener('close', () => {
      if (settled) return;
      if (busy) closedWhileBusy = true;
      else done(opts.cancelled);
    });
    const cancel = h('button', { type: 'button', class: 'kf-desk-btn', 'data-sheet': 'cancel', onclick: () => done(opts.cancelled) }, opts.cancel ?? t('common.cancel'));
    const setBusy = (on: boolean) => {
      busy = on;
      if (on) form.setAttribute('aria-busy', 'true');
      else form.removeAttribute('aria-busy');
      // Cancel waits too: the submit may already be creating the pair or the vault.
      confirm.disabled = on;
      cancel.disabled = on;
    };
    const confirm = h(
      'button',
      { type: 'submit', class: `kf-desk-btn ${opts.danger ? 'kf-desk-btn-danger' : 'kf-desk-btn-default'}`, 'data-sheet': 'confirm' },
      opts.confirm,
    );
    const buttons = { cancel, confirm };
    const form: HTMLFormElement = h(
      'form',
      {
        class: 'sheet-form',
        novalidate: true,
        onsubmit: (e: Event) => {
          e.preventDefault();
          if (busy) return;
          setBusy(true);
          void Promise.resolve()
            .then(() => opts.submit())
            .then(
              (value) => {
                setBusy(false);
                if (value !== undefined) done(value);
                else if (closedWhileBusy) done(opts.cancelled);
              },
              (err: unknown) => {
                setBusy(false);
                console.error(err);
                if (closedWhileBusy) done(opts.cancelled);
              },
            );
        },
      },
      h('div', { class: 'sheet-head' },
        h('h2', { id: `${id}-title`, class: 'sheet-title' }, opts.title),
        opts.body ? h('p', { id: `${id}-body`, class: 'sheet-body' }, opts.body) : null),
      opts.content ? h('div', { class: 'sheet-content' }, opts.content) : null,
      h('div', { class: 'sheet-foot' }, SHEET_ORDER.map((k) => buttons[k])),
    );
    dlg.appendChild(form);
    document.body.appendChild(dlg);
    dlg.showModal();
    const f = opts.focus ?? 'first';
    const first = form.querySelector<HTMLElement>('.sheet-content :is(input, select, textarea, button):not([disabled])');
    const target = f instanceof HTMLElement ? f : f === 'cancel' ? cancel : f === 'confirm' ? confirm : first ?? confirm;
    target.focus();
  });
}

/** Puts focus back on what opened a sheet. If a live update replaced it meanwhile, its successor with the same
 *  data-key takes focus; failing that, the screen's heading, so focus never falls to the page body. */
export function returnFocus(before: HTMLElement | null, key: string | undefined): void {
  if (before && before.isConnected && before !== document.body) {
    before.focus();
    return;
  }
  const again = key ? document.querySelector<HTMLElement>(`[data-key="${CSS.escape(key)}"]`) : null;
  (again ?? document.querySelector<HTMLElement>('main h1') ?? document.querySelector<HTMLElement>('h1'))?.focus();
}

/** A field row in a sheet: a label beside the control, on both platforms. `problem` is where a refusal
 *  about this field shows; the control names it with aria-describedby. */
export function sheetRow(label: string, control: Child, opts: { for?: string; problem?: HTMLElement; hint?: Child } = {}): HTMLElement {
  return h(
    'div',
    { class: 'sheet-row' },
    opts.for ? h('label', { class: 'sheet-label', for: opts.for }, label) : h('span', { class: 'sheet-label' }, label),
    h('div', { class: 'sheet-control' }, control, opts.hint ? h('p', { class: 'sheet-hint' }, opts.hint) : null, opts.problem ?? null),
  );
}

/** The place under a field where its refusal shows: announced politely, named by the field's aria-describedby. */
export function fieldProblem(id: string): { el: HTMLElement; show: (text: string | null) => void } {
  // Always rendered, even empty (sheets.css takes it out of the layout instead): a live region added to the
  // accessibility tree together with its first text often goes unread in WebKit.
  const el = h('p', { id, class: 'sheet-problem', 'aria-live': 'polite' });
  return {
    el,
    show: (text) => {
      el.replaceChildren(...(text ? [line('failed'), text] : []));
      el.closest('.sheet-row')?.classList.toggle('sheet-row-error', !!text);
      const scope = el.closest('dialog') ?? document;
      for (const c of scope.querySelectorAll<HTMLElement>(`[aria-describedby~="${CSS.escape(id)}"]`)) markInvalid(c, !!text);
    },
  };
}

/** aria-invalid goes on fields one types or picks in; buttons, checkboxes and radios that name the problem just
 *  describe it. */
function markInvalid(control: HTMLElement, on: boolean): void {
  if (!control.matches('input:not([type="checkbox"], [type="radio"]), select, textarea')) return;
  if (on) control.setAttribute('aria-invalid', 'true');
  else control.removeAttribute('aria-invalid');
}

/** Names `problem` in the aria-describedby of every control inside `box`, now and whenever the box redraws (the
 *  Drive picker rebuilds its controls as you browse), and marks fields invalid while it shows a refusal. */
export function describeControls(box: HTMLElement, problem: HTMLElement): void {
  const apply = () => {
    for (const c of box.querySelectorAll<HTMLElement>('input, select, textarea, button')) {
      const ids = (c.getAttribute('aria-describedby') ?? '').split(/\s+/).filter(Boolean);
      if (!ids.includes(problem.id)) c.setAttribute('aria-describedby', [...ids, problem.id].join(' '));
      markInvalid(c, problem.childNodes.length > 0);
    }
  };
  apply();
  new MutationObserver(apply).observe(box, { childList: true, subtree: true });
}

/** A question about something permanent. Resolves true on confirm; Esc and Cancel resolve false. */
export function confirmDialog(opts: { title: string; body: string; confirm: string; danger?: boolean }): Promise<boolean> {
  return sheet({ title: opts.title, body: opts.body, confirm: opts.confirm, danger: opts.danger, submit: () => true, cancelled: false, focus: 'cancel' });
}

/** Asks for one line of text. Resolves the trimmed text, or null on Cancel / Escape / empty. */
export function promptDialog(opts: {
  title: string;
  body: string;
  label: string;
  confirm: string;
  ltr?: boolean;
  /** Returns why the value can't be used (shown under the field, the sheet stays open), or null. */
  check?: (value: string) => string | null;
}): Promise<string | null> {
  const id = `prompt-${Math.random().toString(36).slice(2)}`;
  const problem = fieldProblem(`${id}-error`);
  const input = h('input', {
    id: `${id}-input`,
    type: 'text',
    dir: opts.ltr ? 'ltr' : 'auto',
    autocomplete: 'off',
    spellcheck: 'false',
    'aria-describedby': `${id}-error`,
    oninput: () => problem.show(null),
  });
  return sheet<string | null>({
    title: opts.title,
    body: opts.body,
    confirm: opts.confirm,
    content: sheetRow(opts.label, h('div', { class: 'kf-field' }, h('div', { class: 'kf-field-box' }, input)), { for: `${id}-input`, problem: problem.el }),
    cancelled: null,
    submit: () => {
      const value = input.value.trim();
      if (!value) return null;
      const why = opts.check ? opts.check(value) : null;
      if (why) {
        problem.show(why);
        input.setAttribute('aria-invalid', 'true');
        input.focus();
        return undefined;
      }
      return value;
    },
  });
}

/** Checkboxes. Resolves the values ticked, or null on Cancel / Escape. */
export function chooseDialog(opts: {
  title: string;
  body: string;
  options: { value: string; label: string; hint?: string; checked: boolean }[];
  confirm: string;
}): Promise<string[] | null> {
  const id = `choose-${Math.random().toString(36).slice(2)}`;
  const boxes = opts.options.map((o, i) => {
    const input = h('input', { type: 'checkbox', id: `${id}-${i}`, value: o.value });
    input.checked = o.checked;
    const row = h(
      'label',
      { class: 'choice', for: `${id}-${i}` },
      input,
      h('span', { class: 'choice-text' }, h('span', { dir: 'auto' }, o.label), o.hint ? h('span', { class: 'kf-muted' }, o.hint) : null),
    );
    return { input, row };
  });
  return sheet<string[] | null>({
    title: opts.title,
    body: opts.body,
    confirm: opts.confirm,
    content: h('fieldset', { class: 'choices' }, h('legend', { class: 'kf-sr-only' }, opts.title), boxes.map((b) => b.row)),
    cancelled: null,
    submit: () => boxes.filter((b) => b.input.checked).map((b) => b.input.value),
  });
}

let toastTimer: number | undefined;
let toastCount = 0;
/** The toast on screen, 0 when there is none. */
let toastShown = 0;

/**
 * A short message in the live region, with an optional action (Undo). Returns the toast's number: ⌘Z / Ctrl+Z
 * undoes only while the toast that offers Undo is on screen (`toastShowing`).
 */
export function toast(text: string, action?: { label: string; run: () => void }): number {
  const region = document.getElementById('toast');
  if (!region) return 0;
  window.clearTimeout(toastTimer);
  const id = ++toastCount;
  const close = () => {
    if (toastShown !== id) return;
    toastShown = 0;
    region.replaceChildren();
  };
  region.replaceChildren(
    h(
      'div',
      { class: 'kf-toast' },
      h('span', { class: 'kf-toast-text' }, text),
      action
        ? h(
            'button',
            {
              type: 'button',
              class: 'kf-toast-action',
              onclick: () => {
                close();
                action.run();
              },
            },
            action.label,
          )
        : null,
      h('button', { type: 'button', class: 'kf-toast-action', 'aria-label': t('common.close'), onclick: close }, line('close')),
    ),
  );
  toastShown = id;
  toastTimer = window.setTimeout(close, action ? 10000 : 6000);
  return id;
}

/** Whether toast `id` is still on screen: not timed out, closed, or replaced by another. */
export function toastShowing(id: number): boolean {
  return id !== 0 && toastShown === id;
}

/** Copies text to the clipboard and says so. */
export async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
    toast(t('common.copied'));
  } catch {
    toast(t('common.copy_failed'));
  }
}
