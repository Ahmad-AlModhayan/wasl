// Introductions (docs/INTRODUCTIONS.md): devices another device added wait here as "new" until the person
// keeps them, folders other devices offer this computer (Accept picks the folder, then the usual
// first-sync preview), choosing which devices a pair is shared with, and offering folders to a device
// just paired. Offers are data from other devices, shown as text and never acted on without a click.
// Sentences take the window's direction; names inside them come isolated by t(), so either script reads right.

import { open } from '@tauri-apps/plugin-dialog';
import { api } from '../api';
import type { App } from '../app';
import { formatBytes } from '../format';
import { lang, t } from '../i18n';
import { button, chooseDialog, chip, errorText, fill, h, line, toast } from '../ui';
import type { DeviceInfo, OfferInfo, PairState } from '../types';

/** The device that added `d`, by name when this computer knows it. */
export function addedByName(d: DeviceInfo, all: DeviceInfo[]): string | null {
  if (!d.added_by) return null;
  return all.find((o) => o.id === d.added_by)?.name ?? d.added_by;
}

/** A device another one added, not kept here yet: it gets nothing shared with "all my devices". */
export function isNew(d: DeviceInfo): boolean {
  return !d.this_device && d.known === false;
}

/** The chip and buttons a new device shows in the list: Keep (here) beside Remove (everywhere). */
export function newDeviceParts(d: DeviceInfo, all: DeviceInfo[]): { chip: HTMLElement; note: HTMLElement; keep: HTMLElement } {
  const by = addedByName(d, all);
  return {
    chip: chip(t('intro.new'), 'kf-tone-attention'),
    note: h('span', { class: 'device-added kf-muted' }, by ? t('intro.added_by', { name: by }) : t('intro.added_elsewhere')),
    keep: button(t('intro.keep'), () => void keep(d), { kind: 'filled', icon: 'check', key: `keep-dev-${d.id}`, label: t('intro.keep_for', { name: d.name }) }),
  };
}

async function keep(d: DeviceInfo): Promise<void> {
  try {
    await api.keepDevice(d.id);
    toast(t('intro.kept', { name: d.name }));
  } catch (e) {
    toast(errorText(e));
  }
}

// ---------------------------------------------------------------- offers

/** The offers waiting for an answer (declined and accepted ones stay out of the way). */
export function waitingOffers(offers: OfferInfo[]): OfferInfo[] {
  return offers.filter((o) => o.status === 'new');
}

/** Fills `section` with the waiting offers, or hides it when there are none. */
export function renderOffers(app: App, section: HTMLElement, list: HTMLElement, offers: OfferInfo[]): void {
  const waiting = waitingOffers(offers);
  section.hidden = waiting.length === 0;
  fill(list, h('ul', { class: 'offer-list', role: 'list' }, waiting.map((o) => offerRow(app, o))));
}

function offerRow(app: App, o: OfferInfo): HTMLElement {
  const nameId = `offer-${o.from_device}-${o.share}`;
  return h(
    'li',
    { class: 'offer', 'aria-labelledby': nameId },
    line('folder'),
    h(
      'span',
      { class: 'device-text' },
      h('span', { id: nameId, class: 'device-name' }, t('intro.offers', { device: o.from_name, folder: o.name })),
      h('span', { class: 'device-added kf-muted' }, t('intro.offer_size', { count: o.files, size: formatBytes(o.bytes, lang()) })),
    ),
    h(
      'span',
      { class: 'device-actions' },
      button(t('intro.accept'), () => void accept(app, o), { kind: 'filled', icon: 'folder', key: `accept-${nameId}`, label: t('intro.accept_for', { folder: o.name }) }),
      button(t('intro.decline'), () => void decline(o), { kind: 'plain', icon: 'close', key: `decline-${nameId}`, label: t('intro.decline_for', { folder: o.name }) }),
    ),
  );
}

/** Accept: the person picks where the folder goes; the pair then waits for its first-sync preview. */
async function accept(app: App, o: OfferInfo): Promise<void> {
  const picked = await open({ directory: true, multiple: false, title: t('intro.pick_title', { folder: o.name }) }).catch(() => null);
  if (typeof picked !== 'string' || !picked) return;
  try {
    const pair = await api.acceptOffer(o.share, o.from_device, picked, o.name);
    toast(t('intro.accepted', { folder: o.name }));
    app.go({ name: 'preview', id: pair.id });
  } catch (e) {
    toast(errorText(e));
  }
}

async function decline(o: OfferInfo): Promise<void> {
  try {
    await api.declineOffer(o.share, o.from_device);
    toast(t('intro.declined', { folder: o.name, device: o.from_name }));
  } catch (e) {
    toast(errorText(e));
  }
}

// ---------------------------------------------------------------- sharing a pair with devices

const ALL = '*';

/** "All my devices", or the names of the devices a pair is shared with. */
export function sharedWithText(all: boolean, ids: string[], devices: DeviceInfo[]): string {
  const names = ids.map((id) => devices.find((d) => d.id === id)?.name ?? id);
  if (all) return names.length ? t('intro.with_all_and', { names: names.join(', ') }) : t('intro.with_all');
  return names.length ? t('intro.with_names', { names: names.join(', ') }) : t('intro.with_none');
}

/** Lets the person choose who a pair is shared with. Each device ticked is offered it. */
export async function chooseDevices(pairId: string, pairName: string): Promise<void> {
  try {
    const [sharing, info] = await Promise.all([api.sharing(pairId), api.devices()]);
    const others = info.devices.filter((d) => !d.this_device);
    const listed = new Set(sharing?.devices ?? []);
    const picked = await chooseDialog({
      title: t('intro.choose_title', { name: pairName }),
      body: t('intro.choose_body'),
      options: [
        { value: ALL, label: t('intro.all_devices'), hint: t('intro.all_devices_hint'), checked: sharing?.all_devices ?? false },
        ...others.map((d) => ({ value: d.id, label: d.name, hint: isNew(d) ? t('intro.new') : undefined, checked: listed.has(d.id) })),
      ],
      confirm: t('intro.choose_confirm'),
    });
    if (!picked) return;
    await api.shareWith(pairId, picked.includes(ALL), picked.filter((v) => v !== ALL));
    toast(t('intro.choose_done', { name: pairName }));
  } catch (e) {
    toast(errorText(e));
  }
}

// ---------------------------------------------------------------- after pairing

/** The pairs that sync with devices: the ones that can be offered. */
export function offerablePairs(pairs: PairState[]): PairState[] {
  return pairs.filter((s) => !!s.pair.route && s.pair.route !== 'drive');
}

/** On the device that showed the code, once pairing is done: offer the new device folders, none ticked. */
export async function offerToNewDevice(peerId: string, peerName: string): Promise<void> {
  try {
    const pairs = offerablePairs((await api.status()).pairs);
    if (pairs.length === 0) {
      toast(t('intro.offer_none'));
      return;
    }
    const picked = await chooseDialog({
      title: t('intro.offer_title', { name: peerName }),
      body: t('intro.offer_body', { name: peerName }),
      options: pairs.map((s) => ({ value: s.pair.id, label: s.pair.name, checked: false })),
      confirm: t('intro.offer_confirm'),
    });
    if (!picked?.length) return;
    for (const id of picked) {
      const sharing = await api.sharing(id);
      const devices = new Set(sharing?.devices ?? []);
      devices.add(peerId);
      await api.shareWith(id, sharing?.all_devices ?? false, [...devices]);
    }
    toast(t('intro.offered', { count: picked.length, name: peerName }));
  } catch (e) {
    toast(errorText(e));
  }
}
