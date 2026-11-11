// Devices: this computer and the other devices of the group (Connected / Not connected, Remove), adding a
// device by showing a pairing code (QR and text) or joining with one, the four-word check both screens show,
// and connecting by address. The pairing's stages come from the daemon (`pairing_status`, re-read on every
// `changed` event); a status line announces each stage and focus follows the flow.

import { api, isUiError } from '../api';
import type { App } from '../app';
import { t } from '../i18n';
import { encodeQr, qrPath } from '../qr';
import { button, chip, confirmDialog, copyText, empty, errorText, fill, h, line, toast } from '../ui';
import type { DeviceInfo, DevicesInfo, OfferInfo, PairingStatus } from '../types';
import { isNew, newDeviceParts, offerToNewDevice, renderOffers } from './introductions';

const SVG = 'http://www.w3.org/2000/svg';

export async function devicesScreen(app: App): Promise<HTMLElement> {
  const head = h('header', { class: 'screen-head' }, h('h1', { class: 'sr-title', tabindex: '-1' }, t('devices.title')));
  let info: DevicesInfo;
  try {
    info = await api.devices();
  } catch (e) {
    return h('div', { class: 'screen' }, head, h('p', { class: 'field-error', role: 'alert' }, errorText(e)));
  }
  if (!info.enabled) {
    return h(
      'div',
      { class: 'screen' },
      head,
      empty('laptop', t('devices.off_title'), t('devices.off_body')),
    );
  }

  const list = h('div', null);
  // Folders other devices offer this computer (docs/INTRODUCTIONS.md §3.3); hidden while there are none.
  const offerList = h('div', null);
  const offerSection = h(
    'section',
    { class: 'kf-card form-section', 'aria-labelledby': 'dev-offers', hidden: true },
    h('h2', { id: 'dev-offers', class: 'section-title' }, t('intro.offers_title')),
    offerList,
  );
  const pairingBody = h('div', { class: 'pairing' });
  // Announces each stage once; the panel itself is not live, so the code and QR aren't read out.
  const announcer = h('p', { class: 'kf-sr-only', role: 'status', 'aria-live': 'polite' });
  const screen = h(
    'div',
    { class: 'screen' },
    head,
    h('section', { class: 'kf-card form-section', 'aria-labelledby': 'dev-list' }, h('h2', { id: 'dev-list', class: 'section-title' }, t('devices.yours')), list),
    offerSection,
    h(
      'section',
      { class: 'kf-card form-section', 'aria-labelledby': 'dev-add' },
      h('h2', { id: 'dev-add', class: 'section-title' }, t('devices.add_title')),
      announcer,
      pairingBody,
    ),
    connectSection(),
  );

  const panel = new PairingPanel(app, pairingBody, announcer);
  renderList(list, info);
  renderOffers(app, offerSection, offerList, await api.offers().catch((): OfferInfo[] => []));
  const status = await api.pairingStatus().catch(() => null);
  panel.show(status, false);

  // Re-read on changes (pairing stages, devices joining, leaving, connecting). A slow poll backs the
  // events up while a pairing is open, and stops when the screen is gone.
  let busy = false;
  const refresh = async () => {
    if (busy || !screen.isConnected) return;
    busy = true;
    try {
      const [d, s, o] = await Promise.all([api.devices(), api.pairingStatus(), api.offers().catch((): OfferInfo[] => [])]);
      if (!screen.isConnected) return;
      renderList(list, d);
      renderOffers(app, offerSection, offerList, o);
      panel.show(s, true);
    } catch {
      // The connection panel takes over when the daemon goes away.
    } finally {
      busy = false;
    }
  };
  app.onEvent((e) => {
    if (e.event === 'changed') void refresh();
  });
  const poll = window.setInterval(() => {
    if (!screen.isConnected) window.clearInterval(poll);
    else if (panel.open) void refresh();
  }, 3000);
  return screen;
}

// ---------------------------------------------------------------- the list

/** `WASL-7KQ2-M4PX…`: enough to tell devices apart; the whole ID is in the title. */
export function shortId(id: string): string {
  const parts = id.split('-');
  return parts.length > 3 ? `${parts.slice(0, 3).join('-')}…` : id;
}

function renderList(list: HTMLElement, info: DevicesInfo): void {
  const focusKey = (document.activeElement as HTMLElement | null)?.closest<HTMLElement>('[data-key]')?.dataset.key;
  const hadFocus = list.contains(document.activeElement);
  const me = info.devices.filter((d) => d.this_device);
  const others = info.devices.filter((d) => !d.this_device);
  const port = info.listen?.match(/:(\d+)$/)?.[1];
  fill(
    list,
    h('ul', { class: 'plain-list device-list', role: 'list' }, [...me, ...others].map((d) => deviceRow(d, info.devices))),
    others.length === 0 ? empty('laptop', t('devices.empty_title'), t('devices.empty_body')) : null,
    port ? h('p', { class: 'kf-muted device-port' }, t('devices.listening', { port })) : null,
  );
  if (hadFocus) {
    const again = focusKey ? list.querySelector<HTMLElement>(`[data-key="${CSS.escape(focusKey)}"]`) : null;
    (again ?? document.querySelector<HTMLElement>('#main h1'))?.focus();
  }
}

/** The device's icon by its kind: a desktop, a laptop or a phone; a computer when the kind isn't known. */
export function deviceIcon(d: Pick<DeviceInfo, 'kind'>): 'desktop' | 'laptop' | 'phone' {
  return d.kind === 'laptop' || d.kind === 'phone' ? d.kind : 'desktop';
}

function deviceRow(d: DeviceInfo, all: DeviceInfo[]): HTMLElement {
  const nameId = `dev-${d.id}`;
  // Added by another device and not kept here yet: who added it, Keep beside Remove (docs/INTRODUCTIONS.md §3.1).
  const fresh = isNew(d) ? newDeviceParts(d, all) : null;
  return h(
    'li',
    { class: 'device', 'aria-labelledby': nameId },
    line(deviceIcon(d)),
    h(
      'span',
      { class: 'device-text' },
      h('span', { id: nameId, class: 'device-name', dir: 'auto' }, d.name),
      h('span', { class: 'device-id kf-muted', dir: 'ltr', title: d.id }, shortId(d.id)),
      fresh?.note,
    ),
    d.this_device
      ? chip(t('devices.this_computer'), 'kf-tone-info')
      : fresh
        ? fresh.chip
        : chip(
          d.connected
            ? d.path
              ? t('devices.connected_via', { path: t(`devices.path_${d.path}`) })
              : t('devices.connected')
            : t('devices.not_connected'),
          d.connected ? 'kf-tone-positive' : 'kf-tone-neutral',
        ),
    fresh?.keep,
    d.this_device
      ? null
      : button(t('devices.remove'), () => void removeDevice(d), {
          kind: 'plain',
          icon: 'trash',
          key: `remove-dev-${d.id}`,
          label: t('devices.remove_for', { name: d.name }),
        }),
  );
}

async function removeDevice(d: DeviceInfo): Promise<void> {
  const ok = await confirmDialog({
    title: t('devices.remove_title', { name: d.name }),
    body: t('devices.remove_body', { id: d.id }),
    confirm: t('devices.remove'),
    danger: true,
  });
  if (!ok) return;
  try {
    await api.removeDevice(d.id);
    toast(t('devices.removed', { name: d.name }));
  } catch (e) {
    toast(deviceError(e));
  }
}

// ---------------------------------------------------------------- pairing

/** The daemon's English failure messages, in the window's language when known. */
const FAILURES: Record<string, string> = {
  'The pairing code expired. Make a new one.': 'pairing.fail_expired',
  'The other device took too long to answer.': 'pairing.fail_timeout',
  "The code didn't match. Make a new one and try again.": 'pairing.fail_code',
  'Pairing was cancelled on one of the devices.': 'pairing.fail_cancelled',
  'Pairing failed. Make a new code and try again.': 'pairing.fail_other',
  "You said the words don't match. Nothing was added.": 'pairing.fail_words',
  'A pairing is already open. Finish or cancel it first.': 'pairing.fail_busy',
  'That is not a Wasl pairing code.': 'pairing.fail_not_code',
  'That code was made on this computer.': 'pairing.fail_own_code',
};

function failureText(message: string | null): string {
  if (!message) return t('pairing.fail_other');
  const key = FAILURES[message];
  return key ? t(key) : message;
}

/** Errors from device commands: the daemon's message (its kinds are about Drive, not devices). */
function deviceError(e: unknown): string {
  if (isUiError(e) && !['not_running', 'disconnected', 'untrusted', 'version'].includes(e.kind)) {
    const key = e.message ? FAILURES[e.message] : undefined;
    return key ? t(key) : t('common.error', { message: failureText(e.message) });
  }
  return errorText(e);
}

class PairingPanel {
  /** A pairing is under way (the poll runs). */
  open = false;
  private key = '';
  private timer: number | undefined;

  constructor(
    private app: App,
    private body: HTMLElement,
    private announcer: HTMLElement,
  ) {}

  /** Draws the stage. `announce` moves focus and says it when the stage changed (not on first draw). */
  show(s: PairingStatus | null, announce: boolean): void {
    const stage = s?.stage ?? 'idle';
    const key = `${stage}|${s?.inviting}|${s?.confirmed_here}|${s?.code ?? ''}`;
    if (key === this.key) return;
    const focusWasHere = this.body.contains(document.activeElement) || document.activeElement === document.body;
    this.key = key;
    this.open = stage === 'waiting' || stage === 'confirm';
    window.clearInterval(this.timer);

    let focus: HTMLElement | null = null;
    let say = '';
    if (!s || stage === 'idle') {
      fill(this.body, this.idle());
    } else if (stage === 'waiting' && s.inviting) {
      const [view, first] = this.code(s);
      fill(this.body, view);
      focus = first;
      say = t('pairing.code_ready');
    } else if (stage === 'waiting') {
      const msg = h('p', { tabindex: '-1', class: 'pairing-lead' }, t('pairing.reaching'));
      fill(this.body, h('div', { class: 'pairing-wait' }, h('span', { class: 'spinner', 'aria-hidden': 'true' }), msg), this.cancelRow());
      focus = msg;
      say = t('pairing.reaching');
    } else if (stage === 'confirm' && !s.confirmed_here) {
      const lead = h('h3', { tabindex: '-1', class: 'pairing-lead' }, t('pairing.words_lead'));
      fill(
        this.body,
        lead,
        words(s.words, true),
        h('p', { class: 'pairing-ask' }, t('pairing.words_ask')),
        h(
          'div',
          { class: 'pairing-actions' },
          button(t('pairing.yes'), () => void this.confirm(true), { kind: 'filled', icon: 'check', key: 'pair-yes', small: false }),
          button(t('pairing.no'), () => void this.confirm(false), { kind: 'gray', icon: 'close', key: 'pair-no', small: false }),
          button(t('pairing.cancel'), () => void this.cancel(), { kind: 'plain', key: 'pair-cancel', small: false }),
        ),
      );
      focus = lead;
      say = `${t('pairing.words_lead')} ${s.words.join(', ')}. ${t('pairing.words_ask')}`;
    } else if (stage === 'confirm') {
      const msg = h('p', { tabindex: '-1', class: 'pairing-lead' }, t('pairing.waiting_other'));
      fill(this.body, words(s.words, false), h('div', { class: 'pairing-wait' }, h('span', { class: 'spinner', 'aria-hidden': 'true' }), msg), this.cancelRow());
      focus = msg;
      say = t('pairing.waiting_other');
    } else if (stage === 'paired') {
      const name = s.peer_name ?? '?';
      const msg = h('p', { tabindex: '-1', class: 'kf-banner-title' }, t('pairing.paired', { name }));
      fill(
        this.body,
        h('div', { class: 'kf-banner kf-tone-positive' }, h('div', { class: 'kf-banner-icon', 'aria-hidden': 'true' }, line('check_circle', { size: 20 })), msg),
        h('p', { class: 'kf-muted' }, t(s.inviting ? 'intro.paired_next' : 'pairing.paired_next')),
        h(
          'div',
          { class: 'pairing-actions' },
          // The device that showed the code may introduce the new one to its folders: none ticked (§3.4).
          s.inviting && s.peer_id
            ? button(t('intro.offer_button', { name }), () => void offerToNewDevice(s.peer_id!, name), { kind: 'filled', icon: 'folder', key: 'pair-offer', small: false })
            : null,
          button(t('pairing.to_pairs'), () => {
            void api.cancelPairing().catch(() => undefined);
            this.app.go({ name: 'pairs' });
          }, { kind: s.inviting && s.peer_id ? 'gray' : 'filled', key: 'pair-to-pairs', small: false }),
          button(t('pairing.done'), () => void this.reset(), { kind: 'gray', key: 'pair-done', small: false }),
        ),
      );
      focus = msg;
      say = t('pairing.paired', { name });
    } else {
      const text = failureText(s.message);
      const msg = h('p', { tabindex: '-1', class: 'kf-banner-title', dir: 'auto' }, text);
      fill(
        this.body,
        h('div', { class: 'kf-banner kf-tone-critical' }, h('div', { class: 'kf-banner-icon', 'aria-hidden': 'true' }, line('failed', { size: 20 })), msg),
        h('div', { class: 'pairing-actions' }, button(t('pairing.try_again'), () => void this.reset(), { kind: 'filled', icon: 'retry', key: 'pair-again', small: false })),
      );
      focus = msg;
      say = text;
    }

    if (announce) {
      if (say) this.announcer.textContent = say;
      if (focus && focusWasHere) focus.focus();
      else if (!s || stage === 'idle') {
        if (focusWasHere) this.body.querySelector<HTMLElement>('[data-key="pair-invite"]')?.focus();
      }
    }
  }

  private idle(): HTMLElement[] {
    const input = h('input', {
      id: 'join-code',
      class: 'code-input',
      type: 'text',
      dir: 'ltr',
      autocomplete: 'off',
      autocapitalize: 'characters',
      spellcheck: 'false',
      'aria-describedby': 'join-help',
      'data-key': 'join-code',
    });
    const error = h('p', { id: 'join-error', class: 'field-error', role: 'alert' });
    const join = async () => {
      const code = input.value.trim().toUpperCase();
      if (!code) {
        error.textContent = t('pairing.join_empty');
        input.setAttribute('aria-invalid', 'true');
        input.setAttribute('aria-describedby', 'join-help join-error');
        input.focus();
        return;
      }
      error.textContent = '';
      input.removeAttribute('aria-invalid');
      try {
        await api.joinDevice(code);
        this.show(await api.pairingStatus(), true);
      } catch (e) {
        error.textContent = deviceError(e);
        input.setAttribute('aria-invalid', 'true');
        input.setAttribute('aria-describedby', 'join-help join-error');
        input.focus();
      }
    };
    return [
      h(
        'div',
        { class: 'pairing-option' },
        h('h3', { class: 'option-title' }, t('pairing.invite_title')),
        h('p', { class: 'kf-muted' }, t('pairing.invite_body')),
        button(t('pairing.invite'), () => void this.invite(), { kind: 'filled', icon: 'plus', key: 'pair-invite', small: false }),
      ),
      h(
        'form',
        {
          class: 'pairing-option',
          onsubmit: (e: Event) => {
            e.preventDefault();
            void join();
          },
        },
        h('h3', { class: 'option-title' }, t('pairing.join_title')),
        h('label', { for: 'join-code', class: 'kf-field-label' }, t('pairing.join_label')),
        h('p', { id: 'join-help', class: 'kf-field-help' }, t('pairing.join_help')),
        h(
          'div',
          { class: 'inline-field' },
          h('div', { class: 'kf-field-box' }, input),
          h('button', { type: 'submit', class: 'kf-btn kf-btn-sm kf-btn-tinted', 'data-key': 'pair-join' }, line('key'), t('pairing.join')),
        ),
        error,
      ),
    ];
  }

  /** The code as a QR and as text, with Copy, the time left and Cancel. Returns the view and what to focus. */
  private code(s: PairingStatus): [HTMLElement, HTMLElement] {
    const code = s.code ?? '';
    const lead = h('p', { tabindex: '-1', class: 'pairing-lead' }, t('pairing.show_code'));
    const left = h('p', { class: 'kf-muted kf-tnum pairing-expiry' });
    const tick = () => {
      if (!left.isConnected && this.timer !== undefined) {
        window.clearInterval(this.timer);
        return;
      }
      const secs = s.expires_at ? Math.max(0, Math.round(s.expires_at - Date.now() / 1000)) : null;
      left.textContent =
        secs === null ? '' : secs > 0 ? t('pairing.expires_in', { time: clock(secs) }) : t('pairing.expired');
    };
    tick();
    this.timer = window.setInterval(tick, 1000);
    const view = h(
      'div',
      { class: 'pairing-code' },
      lead,
      h(
        'div',
        { class: 'code-layout' },
        qrSvg(code),
        h(
          'div',
          { class: 'code-text' },
          h('p', { class: 'kf-field-label', id: 'code-label' }, t('pairing.code_label')),
          h('code', { class: 'pair-code', dir: 'ltr', 'aria-labelledby': 'code-label' }, code),
          h(
            'div',
            { class: 'pairing-actions' },
            button(t('common.copy'), () => void copyText(code), { icon: 'copy', key: 'pair-copy', label: t('pairing.copy_code') }),
            button(t('pairing.cancel'), () => void this.cancel(), { kind: 'plain', key: 'pair-cancel' }),
          ),
          left,
          h('p', { class: 'kf-muted' }, t('pairing.code_once')),
        ),
      ),
    );
    return [view, lead];
  }

  private cancelRow(): HTMLElement {
    return h('div', { class: 'pairing-actions' }, button(t('pairing.cancel'), () => void this.cancel(), { kind: 'plain', key: 'pair-cancel', small: false }));
  }

  private async invite(): Promise<void> {
    try {
      this.show(await api.inviteDevice(), true);
    } catch (e) {
      toast(deviceError(e));
    }
  }

  private async confirm(accept: boolean): Promise<void> {
    try {
      this.show(await api.confirmPairing(accept), true);
    } catch (e) {
      toast(deviceError(e));
    }
  }

  private async cancel(): Promise<void> {
    try {
      await api.cancelPairing();
      this.announcer.textContent = t('pairing.cancelled');
      this.show(await api.pairingStatus(), true);
    } catch (e) {
      toast(deviceError(e));
    }
  }

  /** Back to the start after a pairing ended. */
  private async reset(): Promise<void> {
    await api.cancelPairing().catch(() => undefined);
    this.show(await api.pairingStatus().catch(() => null), true);
  }
}

function words(list: string[], large: boolean): HTMLElement {
  return h(
    'ol',
    { class: `pair-words${large ? ' large' : ''}`, lang: 'en', dir: 'ltr', 'aria-label': t('pairing.words_label') },
    list.map((w) => h('li', null, w)),
  );
}

/** m:ss with Western digits. */
function clock(secs: number): string {
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `${m}:${String(s).padStart(2, '0')}`;
}

function qrSvg(text: string): Element {
  let qr;
  try {
    qr = encodeQr(text);
  } catch {
    return h('p', { class: 'kf-muted' }, t('pairing.qr_too_long'));
  }
  const n = qr.size + 8;
  const svg = document.createElementNS(SVG, 'svg');
  svg.setAttribute('viewBox', `0 0 ${n} ${n}`);
  svg.setAttribute('class', 'qr');
  svg.setAttribute('role', 'img');
  svg.setAttribute('aria-label', t('pairing.qr_label'));
  svg.setAttribute('shape-rendering', 'crispEdges');
  const bg = document.createElementNS(SVG, 'rect');
  bg.setAttribute('width', String(n));
  bg.setAttribute('height', String(n));
  bg.setAttribute('fill', '#fff');
  const path = document.createElementNS(SVG, 'path');
  path.setAttribute('d', qrPath(qr));
  path.setAttribute('fill', '#000');
  svg.append(bg, path);
  return svg;
}

// ---------------------------------------------------------------- connect by address

function connectSection(): HTMLElement {
  const input = h('input', {
    id: 'connect-address',
    type: 'text',
    dir: 'ltr',
    autocomplete: 'off',
    spellcheck: 'false',
    placeholder: '192.168.1.20:47100',
    'aria-describedby': 'connect-help',
    'data-key': 'connect-address',
  });
  const error = h('p', { id: 'connect-error', class: 'field-error', role: 'alert' });
  const submit = h('button', { type: 'submit', class: 'kf-btn kf-btn-sm kf-btn-gray', 'data-key': 'connect' }, line('link'), t('devices.connect'));
  const fail = (text: string) => {
    error.textContent = text;
    input.setAttribute('aria-invalid', 'true');
    input.setAttribute('aria-describedby', 'connect-help connect-error');
    input.focus();
  };
  return h(
    'section',
    { class: 'kf-card form-section', 'aria-labelledby': 'dev-connect' },
    h('h2', { id: 'dev-connect', class: 'section-title' }, t('devices.connect_title')),
    h(
      'form',
      {
        onsubmit: async (e: Event) => {
          e.preventDefault();
          const address = input.value.trim();
          if (!/^.+:\d{1,5}$/.test(address)) return fail(t('devices.connect_invalid'));
          error.textContent = '';
          input.removeAttribute('aria-invalid');
          input.setAttribute('aria-describedby', 'connect-help');
          submit.disabled = true;
          fill(submit, h('span', { class: 'spinner', 'aria-hidden': 'true' }), t('devices.connecting'));
          try {
            const id = await api.connectDevice(address);
            toast(t('devices.connected_to', { id: shortId(id), address }));
            input.value = '';
            input.focus();
          } catch (err) {
            fail(deviceError(err));
          } finally {
            submit.disabled = false;
            fill(submit, line('link'), t('devices.connect'));
          }
        },
      },
      h('label', { for: 'connect-address', class: 'kf-field-label' }, t('devices.connect_label')),
      h('p', { id: 'connect-help', class: 'kf-field-help' }, t('devices.connect_help')),
      h('div', { class: 'inline-field' }, h('div', { class: 'kf-field-box' }, input), submit),
      error,
    ),
  );
}
